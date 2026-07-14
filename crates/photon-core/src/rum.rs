//! RUM (Real-User Monitoring) domain logic: the browser beacon shape, its mapping onto Photon's
//! existing signals (Web Vitals → gauge metrics, JS errors → logs), and pure enrichment (UA
//! classification, error fingerprinting). No I/O — callers supply `now_nanos`; testable as tables.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::metric_record::MetricPoint;
use crate::metric_schema::metric_type;
use crate::record::LogRecord;

// ---- frozen contract: attribute keys (see the plan's Global Constraints) ------------------
pub const ATTR_SERVICE: &str = "service.name";
pub const ATTR_ROUTE: &str = "browser.route";
pub const ATTR_PATH: &str = "url.path";
pub const ATTR_DEVICE: &str = "device.type";
pub const ATTR_BROWSER: &str = "browser.name";
pub const ATTR_CONNECTION: &str = "network.connection";
pub const ATTR_SESSION: &str = "session.id";
pub const ATTR_VIEW: &str = "view.id";
pub const ATTR_EXC_TYPE: &str = "exception.type";
pub const ATTR_EXC_MSG: &str = "exception.message";
pub const ATTR_EXC_STACK: &str = "exception.stacktrace";
pub const ATTR_ERR_KIND: &str = "error.kind";
pub const ATTR_FINGERPRINT: &str = "rum.error.fingerprint";

// ---- LCP/INP/CLS attribution (Task F1) ---------------------------------------------------
// String attributes attached to the *main* vital point when the beacon carries `attr`.
pub const ATTR_LCP_ELEMENT: &str = "lcp.element";
pub const ATTR_LCP_URL: &str = "lcp.url";
pub const ATTR_INP_TARGET: &str = "inp.target";
pub const ATTR_CLS_SOURCE: &str = "cls.source";

// Extra gauge metrics derived from LCP/INP numeric sub-parts (unit `ms`, same base attributes
// as the main vital point).
pub const METRIC_LCP_TTFB: &str = "web_vitals.lcp.ttfb";
pub const METRIC_LCP_RESOURCE_LOAD_DELAY: &str = "web_vitals.lcp.resource_load_delay";
pub const METRIC_LCP_RESOURCE_LOAD_TIME: &str = "web_vitals.lcp.resource_load_time";
pub const METRIC_LCP_ELEMENT_RENDER_DELAY: &str = "web_vitals.lcp.element_render_delay";
pub const METRIC_INP_INPUT_DELAY: &str = "web_vitals.inp.input_delay";
pub const METRIC_INP_PROCESSING_DURATION: &str = "web_vitals.inp.processing_duration";
pub const METRIC_INP_PRESENTATION_DELAY: &str = "web_vitals.inp.presentation_delay";

pub const ERROR_SEVERITY_NUMBER: i32 = 17; // OTEL ERROR
pub const ERROR_SEVERITY_TEXT: &str = "ERROR";

/// Map a `web-vitals` metric code (LCP/INP/CLS/FCP/TTFB, case-insensitive) to its Photon metric
/// name. Unknown codes return `None` (dropped defensively).
pub fn metric_name_for(code: &str) -> Option<&'static str> {
    match code.to_ascii_uppercase().as_str() {
        "LCP" => Some("web_vitals.lcp"),
        "INP" => Some("web_vitals.inp"),
        "CLS" => Some("web_vitals.cls"),
        "FCP" => Some("web_vitals.fcp"),
        "TTFB" => Some("web_vitals.ttfb"),
        _ => None,
    }
}

/// Unit for a vital's metric: CLS is unitless (`"1"`), the rest are milliseconds.
pub fn unit_for(code: &str) -> &'static str {
    if code.eq_ignore_ascii_case("CLS") {
        "1"
    } else {
        "ms"
    }
}

/// Google Core Web Vitals rating thresholds `(good_max, poor_min)`. Shared by the query layer's
/// rating distribution and the UI. Units match the metric (ms; CLS unitless).
pub fn thresholds(metric_name: &str) -> Option<(f64, f64)> {
    match metric_name {
        "web_vitals.lcp" => Some((2500.0, 4000.0)),
        "web_vitals.inp" => Some((200.0, 500.0)),
        "web_vitals.cls" => Some((0.1, 0.25)),
        "web_vitals.fcp" => Some((1800.0, 3000.0)),
        "web_vitals.ttfb" => Some((800.0, 1800.0)),
        _ => None,
    }
}

/// Coarse device + browser classification from a User-Agent string. Deliberately small and
/// dependency-free: mobile/tablet/desktop + a browser family. Order matters (Edge before Chrome,
/// Chrome before Safari).
pub struct Ua {
    pub device: &'static str,
    pub browser: &'static str,
}

pub fn parse_ua(ua: &str) -> Ua {
    let device = if ua.contains("iPad") || ua.contains("Tablet") {
        "tablet"
    } else if ua.contains("Mobi") || ua.contains("Android") || ua.contains("iPhone") {
        "mobile"
    } else {
        "desktop"
    };
    let browser = if ua.contains("Edg") {
        "Edge"
    } else if ua.contains("Firefox") || ua.contains("FxiOS") {
        "Firefox"
    } else if ua.contains("Chrome") || ua.contains("CriOS") {
        "Chrome"
    } else if ua.contains("Safari") {
        "Safari"
    } else {
        "Other"
    };
    Ua { device, browser }
}

/// Group errors into issues: a stable 64-bit hex hash of `type + normalized(message) + frame`.
/// Normalization masks digit runs so "chunk 12"/"chunk 99" collapse to one issue.
pub fn fingerprint(err_type: &str, message: &str, top_frame: Option<&str>) -> String {
    let mut norm = String::with_capacity(message.len());
    let mut in_digits = false;
    for ch in message.chars() {
        if ch.is_ascii_digit() {
            if !in_digits {
                norm.push('#');
                in_digits = true;
            }
        } else {
            norm.push(ch);
            in_digits = false;
        }
    }
    // FNV-1a 64-bit — deterministic across platforms/versions (unlike DefaultHasher's seed).
    let mut h: u64 = 0xcbf29ce484222325;
    for byte in err_type
        .bytes()
        .chain(b"\x1f".iter().copied())
        .chain(norm.bytes())
        .chain(b"\x1f".iter().copied())
        .chain(top_frame.unwrap_or("").bytes())
    {
        h ^= byte as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// Validate/normalize an incoming pageview trace id to canonical 32-hex lowercase. Returns `None`
/// for anything that isn't exactly 32 hex digits, or the all-zero id (W3C-invalid) — so a malformed
/// client can't poison the native `trace_id` column.
fn normalize_trace_id(s: &str) -> Option<String> {
    if s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    if lower.bytes().all(|b| b == b'0') {
        return None;
    }
    Some(lower)
}

#[derive(Debug, Clone, Deserialize)]
pub struct Beacon {
    pub app: String,
    pub key: String,
    #[serde(default)]
    pub session: String,
    #[serde(default)]
    pub view: BeaconView,
    #[serde(default)]
    pub ctx: BeaconCtx,
    #[serde(default)]
    pub vitals: Vec<BeaconVital>,
    #[serde(default)]
    pub errors: Vec<BeaconError>,
    /// Pageview-scoped W3C trace id (32-hex), when the SDK's opt-in `tracing` module is active.
    /// Old SDKs omit it. Normalized/validated in `beacon_to_log_records`.
    #[serde(default)]
    pub trace: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BeaconView {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub route: String,
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BeaconCtx {
    #[serde(default)]
    pub ua: String,
    #[serde(default)]
    pub conn: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BeaconVital {
    pub n: String,
    pub v: f64,
    /// Optional attribution payload (Task F1), only present when the SDK's opt-in
    /// `attribution` module ran. Shape depends on `n`:
    /// - LCP: `{ element?, url?, ttfb?, rld?, rlt?, erd? }` (numeric sub-parts in ms)
    /// - INP: `{ target?, id?, pd?, pr? }` (numeric sub-parts in ms)
    /// - CLS: `{ source? }`
    ///
    /// Absent on beacons from SDKs without attribution enabled — backward compatible.
    #[serde(default)]
    pub attr: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BeaconError {
    #[serde(default)]
    pub kind: String,
    #[serde(rename = "type", default)]
    pub ty: String,
    #[serde(default)]
    pub msg: String,
    #[serde(default)]
    pub stack: String,
    #[serde(default)]
    pub src: String,
    #[serde(default)]
    pub line: Option<i64>,
}

/// Shared context attributes stamped onto every row from a beacon.
fn common_attrs(b: &Beacon, service_name: &str) -> BTreeMap<String, String> {
    let ua = parse_ua(&b.ctx.ua);
    let mut a = BTreeMap::new();
    a.insert(ATTR_SERVICE.into(), service_name.to_string());
    if !b.view.route.is_empty() {
        a.insert(ATTR_ROUTE.into(), b.view.route.clone());
    }
    if !b.view.path.is_empty() {
        a.insert(ATTR_PATH.into(), b.view.path.clone());
    }
    a.insert(ATTR_DEVICE.into(), ua.device.into());
    a.insert(ATTR_BROWSER.into(), ua.browser.into());
    if !b.ctx.conn.is_empty() {
        a.insert(ATTR_CONNECTION.into(), b.ctx.conn.clone());
    }
    if !b.session.is_empty() {
        a.insert(ATTR_SESSION.into(), b.session.clone());
    }
    if !b.view.id.is_empty() {
        a.insert(ATTR_VIEW.into(), b.view.id.clone());
    }
    a
}

/// Read a string sub-part defensively out of an `attr` JSON object; missing/wrong-typed keys are
/// `None` rather than an error.
fn attr_str(attr: &serde_json::Value, key: &str) -> Option<String> {
    attr.get(key)?.as_str().map(str::to_string)
}

/// Read a numeric sub-part defensively out of an `attr` JSON object.
fn attr_num(attr: &serde_json::Value, key: &str) -> Option<f64> {
    attr.get(key)?.as_f64()
}

/// Push a `ms`-unit gauge point for a numeric attribution sub-part, if present. Carries the same
/// base (context) attributes as the main vital point it's derived from.
fn push_subpart_gauge(
    points: &mut Vec<MetricPoint>,
    attr: &serde_json::Value,
    key: &str,
    metric_name: &'static str,
    base_attrs: &BTreeMap<String, String>,
    now_nanos: i64,
) {
    if let Some(v) = attr_num(attr, key) {
        points.push(MetricPoint {
            metric_name: metric_name.to_string(),
            metric_type: metric_type::GAUGE,
            unit: Some("ms".to_string()),
            timestamp_nanos: now_nanos,
            value: Some(v),
            attributes: base_attrs.clone(),
            ..MetricPoint::default()
        });
    }
}

/// Vitals → gauge `MetricPoint`s. Timestamp = server receive time (`now_nanos`); client clocks are
/// untrusted. Unknown vital codes are skipped.
///
/// When a vital carries an `attr` attribution payload (Task F1, SDK opt-in), LCP/INP numeric
/// sub-parts become their own `ms` gauge points (same base attributes as the main point), and
/// LCP/INP/CLS string sub-parts (`element`/`url`/`target`/`source`) are attached as extra
/// attributes on the *main* vital point. Missing sub-part keys are skipped defensively.
pub fn beacon_to_metric_points(b: &Beacon, service_name: &str, now_nanos: i64) -> Vec<MetricPoint> {
    let base = common_attrs(b, service_name);
    let mut points = Vec::with_capacity(b.vitals.len());
    for vt in &b.vitals {
        let Some(name) = metric_name_for(&vt.n) else {
            continue;
        };
        let mut attrs = base.clone();
        if let Some(attr) = &vt.attr {
            match vt.n.to_ascii_uppercase().as_str() {
                "LCP" => {
                    if let Some(el) = attr_str(attr, "element") {
                        attrs.insert(ATTR_LCP_ELEMENT.into(), el);
                    }
                    if let Some(url) = attr_str(attr, "url") {
                        attrs.insert(ATTR_LCP_URL.into(), url);
                    }
                    push_subpart_gauge(
                        &mut points,
                        attr,
                        "ttfb",
                        METRIC_LCP_TTFB,
                        &base,
                        now_nanos,
                    );
                    push_subpart_gauge(
                        &mut points,
                        attr,
                        "rld",
                        METRIC_LCP_RESOURCE_LOAD_DELAY,
                        &base,
                        now_nanos,
                    );
                    push_subpart_gauge(
                        &mut points,
                        attr,
                        "rlt",
                        METRIC_LCP_RESOURCE_LOAD_TIME,
                        &base,
                        now_nanos,
                    );
                    push_subpart_gauge(
                        &mut points,
                        attr,
                        "erd",
                        METRIC_LCP_ELEMENT_RENDER_DELAY,
                        &base,
                        now_nanos,
                    );
                }
                "INP" => {
                    if let Some(target) = attr_str(attr, "target") {
                        attrs.insert(ATTR_INP_TARGET.into(), target);
                    }
                    push_subpart_gauge(
                        &mut points,
                        attr,
                        "id",
                        METRIC_INP_INPUT_DELAY,
                        &base,
                        now_nanos,
                    );
                    push_subpart_gauge(
                        &mut points,
                        attr,
                        "pd",
                        METRIC_INP_PROCESSING_DURATION,
                        &base,
                        now_nanos,
                    );
                    push_subpart_gauge(
                        &mut points,
                        attr,
                        "pr",
                        METRIC_INP_PRESENTATION_DELAY,
                        &base,
                        now_nanos,
                    );
                }
                "CLS" => {
                    if let Some(source) = attr_str(attr, "source") {
                        attrs.insert(ATTR_CLS_SOURCE.into(), source);
                    }
                }
                _ => {}
            }
        }
        points.push(MetricPoint {
            metric_name: name.to_string(),
            metric_type: metric_type::GAUGE,
            unit: Some(unit_for(&vt.n).to_string()),
            timestamp_nanos: now_nanos,
            value: Some(vt.v),
            attributes: attrs,
            ..MetricPoint::default()
        });
    }
    points
}

/// Errors → ERROR `LogRecord`s with a stable fingerprint attribute for issue grouping.
pub fn beacon_to_log_records(b: &Beacon, service_name: &str, now_nanos: i64) -> Vec<LogRecord> {
    let base = common_attrs(b, service_name);
    let trace_id = b.trace.as_deref().and_then(normalize_trace_id);
    b.errors
        .iter()
        .map(|e| {
            let frame = if e.src.is_empty() {
                None
            } else if let Some(line) = e.line {
                Some(format!("{}:{}", e.src, line))
            } else {
                Some(e.src.clone())
            };
            let fp = fingerprint(&e.ty, &e.msg, frame.as_deref());
            let mut attrs = base.clone();
            attrs.insert(ATTR_ERR_KIND.into(), e.kind.clone());
            attrs.insert(ATTR_EXC_TYPE.into(), e.ty.clone());
            attrs.insert(ATTR_EXC_MSG.into(), e.msg.clone());
            if !e.stack.is_empty() {
                attrs.insert(ATTR_EXC_STACK.into(), e.stack.clone());
            }
            attrs.insert(ATTR_FINGERPRINT.into(), fp);
            LogRecord {
                timestamp_nanos: now_nanos,
                severity_number: Some(ERROR_SEVERITY_NUMBER),
                severity_text: Some(ERROR_SEVERITY_TEXT.to_string()),
                body: Some(e.msg.clone()),
                trace_id: trace_id.clone(),
                attributes: attrs,
                ..LogRecord::default()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ua_classifies_device_and_browser() {
        let m =
            parse_ua("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit Safari");
        assert_eq!(m.device, "mobile");
        assert_eq!(m.browser, "Safari");
        let d = parse_ua("Mozilla/5.0 (Windows NT 10.0) AppleWebKit Chrome/120 Safari/537");
        assert_eq!(d.device, "desktop");
        assert_eq!(d.browser, "Chrome");
        let e = parse_ua("... Chrome/120 ... Edg/120");
        assert_eq!(e.browser, "Edge");
        let t = parse_ua("Mozilla/5.0 (iPad; CPU OS 17) Safari");
        assert_eq!(t.device, "tablet");
    }

    #[test]
    fn fingerprint_is_stable_and_normalizes_numbers() {
        let a = fingerprint(
            "ChunkLoadError",
            "Loading chunk 12 failed",
            Some("app.js:214"),
        );
        let b = fingerprint(
            "ChunkLoadError",
            "Loading chunk 99 failed",
            Some("app.js:214"),
        );
        assert_eq!(a, b, "digit runs are masked -> same issue");
        let c = fingerprint("TypeError", "Loading chunk 12 failed", Some("app.js:214"));
        assert_ne!(a, c, "different type -> different issue");
        assert_eq!(a.len(), 16); // 64-bit hex
    }

    #[test]
    fn metric_name_maps_known_vitals_only() {
        assert_eq!(metric_name_for("LCP"), Some("web_vitals.lcp"));
        assert_eq!(metric_name_for("cls"), Some("web_vitals.cls"));
        assert_eq!(metric_name_for("bogus"), None);
    }

    fn sample() -> Beacon {
        serde_json::from_str(
            r#"{
            "app":"web","key":"pk_1","session":"s1",
            "view":{"id":"v1","route":"/checkout","path":"/checkout"},
            "ctx":{"ua":"Mozilla/5.0 (iPhone) Safari","conn":"4g"},
            "vitals":[{"n":"LCP","v":4300},{"n":"CLS","v":0.06}],
            "errors":[{"kind":"exception","type":"TypeError","msg":"x is undefined","stack":"...","src":"a.js","line":10}]
        }"#,
        )
        .unwrap()
    }

    #[test]
    fn maps_vitals_to_gauge_points() {
        let pts = beacon_to_metric_points(&sample(), "web", 1_000);
        assert_eq!(pts.len(), 2);
        let lcp = pts
            .iter()
            .find(|p| p.metric_name == "web_vitals.lcp")
            .unwrap();
        assert_eq!(lcp.metric_type, metric_type::GAUGE);
        assert_eq!(lcp.value, Some(4300.0));
        assert_eq!(lcp.unit.as_deref(), Some("ms"));
        assert_eq!(lcp.attributes.get(ATTR_SERVICE).unwrap(), "web");
        assert_eq!(lcp.attributes.get(ATTR_ROUTE).unwrap(), "/checkout");
        assert_eq!(lcp.attributes.get(ATTR_DEVICE).unwrap(), "mobile");
        assert_eq!(lcp.attributes.get(ATTR_BROWSER).unwrap(), "Safari");
        let cls = pts
            .iter()
            .find(|p| p.metric_name == "web_vitals.cls")
            .unwrap();
        assert_eq!(cls.unit.as_deref(), Some("1"));
    }

    #[test]
    fn maps_errors_to_log_records() {
        let recs = beacon_to_log_records(&sample(), "web", 1_000);
        assert_eq!(recs.len(), 1);
        let r = &recs[0];
        assert_eq!(r.severity_number, Some(ERROR_SEVERITY_NUMBER));
        assert_eq!(r.body.as_deref(), Some("x is undefined"));
        assert_eq!(r.attributes.get(ATTR_SERVICE).unwrap(), "web");
        assert_eq!(r.attributes.get(ATTR_EXC_TYPE).unwrap(), "TypeError");
        assert!(r.attributes.contains_key(ATTR_FINGERPRINT));
    }

    #[test]
    fn sets_trace_id_from_valid_beacon_trace() {
        let mut b = sample();
        b.trace = Some("4BF92F3577B34DA6A3CE929D0E0E4736".to_string()); // uppercase → normalized
        let recs = beacon_to_log_records(&b, "web", 1_000);
        assert_eq!(recs.len(), 1);
        assert_eq!(
            recs[0].trace_id.as_deref(),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
    }

    #[test]
    fn drops_malformed_trace_id() {
        for bad in [
            "not-hex",
            "abc",
            &"f".repeat(31),
            &"f".repeat(33),
            &"0".repeat(32),
        ] {
            let mut b = sample();
            b.trace = Some(bad.to_string());
            assert_eq!(
                beacon_to_log_records(&b, "web", 1_000)[0].trace_id,
                None,
                "bad={bad}"
            );
        }
    }

    #[test]
    fn beacon_without_trace_still_parses_and_has_no_trace_id() {
        let recs = beacon_to_log_records(&sample(), "web", 1_000); // sample() JSON omits `trace`
        assert_eq!(recs[0].trace_id, None);
    }

    // ---- Task F1: attribution -------------------------------------------------------------

    fn sample_with_lcp_attr() -> Beacon {
        serde_json::from_str(
            r##"{
            "app":"web","key":"pk_1","session":"s1",
            "view":{"id":"v1","route":"/checkout","path":"/checkout"},
            "ctx":{"ua":"Mozilla/5.0 (iPhone) Safari","conn":"4g"},
            "vitals":[{"n":"LCP","v":4300,"attr":{
                "element":"#hero","url":"https://x.test/img.png",
                "ttfb":120,"rld":30,"rlt":900,"erd":50
            }}]
        }"##,
        )
        .unwrap()
    }

    #[test]
    fn beacon_without_attr_still_parses_and_is_unchanged() {
        let b = sample();
        assert!(
            b.vitals.iter().all(|v| v.attr.is_none()),
            "beacons without `attr` must still deserialize (backward compatible)"
        );
        let pts = beacon_to_metric_points(&b, "web", 1_000);
        // Exactly the LCP + CLS points from the base sample — no sub-part gauges appear.
        assert_eq!(pts.len(), 2);
        assert!(pts
            .iter()
            .all(|p| !p.metric_name.contains(".lcp.") && !p.metric_name.contains(".inp.")));
    }

    #[test]
    fn lcp_attribution_emits_subpart_gauges_and_element_attrs() {
        let pts = beacon_to_metric_points(&sample_with_lcp_attr(), "web", 1_000);
        // main LCP point + 4 sub-part gauges.
        assert_eq!(pts.len(), 5);

        let lcp = pts
            .iter()
            .find(|p| p.metric_name == "web_vitals.lcp")
            .unwrap();
        assert_eq!(lcp.value, Some(4300.0));
        assert_eq!(lcp.attributes.get(ATTR_LCP_ELEMENT).unwrap(), "#hero");
        assert_eq!(
            lcp.attributes.get(ATTR_LCP_URL).unwrap(),
            "https://x.test/img.png"
        );

        let ttfb = pts
            .iter()
            .find(|p| p.metric_name == METRIC_LCP_TTFB)
            .unwrap();
        assert_eq!(ttfb.value, Some(120.0));
        assert_eq!(ttfb.unit.as_deref(), Some("ms"));
        assert_eq!(ttfb.metric_type, metric_type::GAUGE);
        assert_eq!(ttfb.attributes.get(ATTR_SERVICE).unwrap(), "web");
        assert_eq!(ttfb.attributes.get(ATTR_ROUTE).unwrap(), "/checkout");

        let rld = pts
            .iter()
            .find(|p| p.metric_name == METRIC_LCP_RESOURCE_LOAD_DELAY)
            .unwrap();
        assert_eq!(rld.value, Some(30.0));
        let rlt = pts
            .iter()
            .find(|p| p.metric_name == METRIC_LCP_RESOURCE_LOAD_TIME)
            .unwrap();
        assert_eq!(rlt.value, Some(900.0));
        let erd = pts
            .iter()
            .find(|p| p.metric_name == METRIC_LCP_ELEMENT_RENDER_DELAY)
            .unwrap();
        assert_eq!(erd.value, Some(50.0));
    }

    #[test]
    fn lcp_attribution_missing_subpart_keys_are_skipped_defensively() {
        let b: Beacon = serde_json::from_str(
            r##"{"app":"web","key":"pk_1","vitals":[
                {"n":"LCP","v":4300,"attr":{"element":"#hero","ttfb":120}}
            ]}"##,
        )
        .unwrap();
        let pts = beacon_to_metric_points(&b, "web", 1_000);
        // main LCP point + only the one present sub-part (ttfb).
        assert_eq!(pts.len(), 2);
        assert!(pts.iter().any(|p| p.metric_name == METRIC_LCP_TTFB));
        assert!(!pts
            .iter()
            .any(|p| p.metric_name == METRIC_LCP_RESOURCE_LOAD_DELAY));
        assert!(!pts
            .iter()
            .any(|p| p.metric_name == METRIC_LCP_RESOURCE_LOAD_TIME));
        assert!(!pts
            .iter()
            .any(|p| p.metric_name == METRIC_LCP_ELEMENT_RENDER_DELAY));
    }

    #[test]
    fn inp_attribution_emits_phase_gauges_and_target_attr() {
        let b: Beacon = serde_json::from_str(
            r#"{"app":"web","key":"pk_1","vitals":[
                {"n":"INP","v":250,"attr":{"target":"button.submit","id":40,"pd":120,"pr":30}}
            ]}"#,
        )
        .unwrap();
        let pts = beacon_to_metric_points(&b, "web", 1_000);
        assert_eq!(pts.len(), 4); // main + 3 phases

        let inp = pts
            .iter()
            .find(|p| p.metric_name == "web_vitals.inp")
            .unwrap();
        assert_eq!(
            inp.attributes.get(ATTR_INP_TARGET).unwrap(),
            "button.submit"
        );
        let input_delay = pts
            .iter()
            .find(|p| p.metric_name == METRIC_INP_INPUT_DELAY)
            .unwrap();
        assert_eq!(input_delay.value, Some(40.0));
        let processing = pts
            .iter()
            .find(|p| p.metric_name == METRIC_INP_PROCESSING_DURATION)
            .unwrap();
        assert_eq!(processing.value, Some(120.0));
        let presentation = pts
            .iter()
            .find(|p| p.metric_name == METRIC_INP_PRESENTATION_DELAY)
            .unwrap();
        assert_eq!(presentation.value, Some(30.0));
    }

    #[test]
    fn cls_attribution_attaches_source_attr_with_no_submetrics() {
        let b: Beacon = serde_json::from_str(
            r#"{"app":"web","key":"pk_1","vitals":[
                {"n":"CLS","v":0.15,"attr":{"source":"div.banner"}}
            ]}"#,
        )
        .unwrap();
        let pts = beacon_to_metric_points(&b, "web", 1_000);
        assert_eq!(pts.len(), 1); // CLS has no sub-part gauges, only the `cls.source` attribute
        let cls = &pts[0];
        assert_eq!(cls.metric_name, "web_vitals.cls");
        assert_eq!(cls.attributes.get(ATTR_CLS_SOURCE).unwrap(), "div.banner");
    }
}
