//! RUM Web-Vitals aggregates over the metrics store: per-vital p75 + good/needs/poor rating
//! distribution for an app, and a breakdown of LCP/INP/CLS p75 grouped by a single dimension.
//! Built on the metrics `survivors_df` + `metric_base_predicate` + `approx_percentile_cont`
//! (t-digest), following `red.rs` / `span_latency.rs`.
//!
//! Web Vitals are stored as gauge metrics named `web_vitals.{lcp,inp,cls,fcp,ttfb,route_change}`
//! with a Float64 `value`; `service.name` is a promoted column, the dimensional attributes
//! (`device.type`, `browser.name`, `browser.route`, ...) live in the attributes map. The rating
//! thresholds come from `photon_core::rum::thresholds` — one source of truth shared with the UI.

use std::collections::BTreeMap;

use arrow::array::{Array, Float64Array, Int64Array, StringArray};
use datafusion::functions::core::expr_fn::get_field;
use datafusion::functions_aggregate::expr_fn::{approx_percentile_cont, avg, count, sum};
use datafusion::prelude::{col, lit, when, Expr};

use photon_core::metric_schema;
use photon_core::rum::{
    thresholds, ATTR_LCP_ELEMENT, ATTR_ROUTE, ATTR_SERVICE, METRIC_LCP_ELEMENT_RENDER_DELAY,
    METRIC_LCP_RESOURCE_LOAD_DELAY, METRIC_LCP_RESOURCE_LOAD_TIME, METRIC_LCP_TTFB,
    METRIC_ROUTE_CHANGE, METRIC_VIEW_DURATION,
};
use photon_core::PhotonError;

use crate::col_ref;
use crate::metric_engine::{metric_base_predicate, MetricRequest};
use crate::MetricsQueryEngine;

/// The five Core Web Vitals metric names plus the SPA soft-navigation `route_change` gauge, in
/// display order. Each has a `thresholds` entry.
const VITALS: [&str; 6] = [
    "web_vitals.lcp",
    "web_vitals.inp",
    "web_vitals.cls",
    "web_vitals.fcp",
    "web_vitals.ttfb",
    METRIC_ROUTE_CHANGE,
];

/// Per-vital summary for one app over one window: the 75th-percentile value plus the Google
/// Core-Web-Vitals rating distribution (good/needs-improvement/poor sample counts). `p75` and the
/// rating bands are in the vital's own unit (ms, except CLS which is unitless).
#[derive(Debug, Clone, PartialEq)]
pub struct VitalSummary {
    /// Metric name, e.g. `web_vitals.lcp`.
    pub metric: String,
    /// 75th percentile of `value` (t-digest approximation). The headline "score" for a vital.
    pub p75: f64,
    /// Total samples in the window (good + needs + poor).
    pub count: i64,
    /// Samples rated "good" (`value <= good_max`).
    pub good: i64,
    /// Samples rated "needs improvement" (`good_max < value <= poor_min`).
    pub needs: i64,
    /// Samples rated "poor" (`value > poor_min`).
    pub poor: i64,
}

/// One breakdown row: an app's LCP/INP/CLS p75 for a single value of a grouping dimension
/// (e.g. `device.type = "mobile"`). A vital that has no samples in the group is `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct BreakdownRow {
    /// The dimension value this row aggregates (e.g. `"mobile"`, `"Chrome"`, `"/checkout"`).
    pub key: String,
    /// The largest per-metric sample count seen in the group across LCP/INP/CLS and
    /// `view_duration` (one point per finalized view — the true pageview count when present;
    /// the vitals keep old data from SDKs that predate `view_duration` counted).
    pub pageviews: i64,
    pub lcp_p75: Option<f64>,
    pub inp_p75: Option<f64>,
    pub cls_p75: Option<f64>,
}

/// The average of each LCP attribution sub-part (in ms) plus the most-common LCP element for a
/// route — the "why is LCP slow" panel's data. Every field is `None` when its gauge/attribute has
/// no samples in the window (an SDK without the opt-in attribution module emits none of them).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LcpAttribution {
    /// AVG of `web_vitals.lcp.ttfb`.
    pub ttfb: Option<f64>,
    /// AVG of `web_vitals.lcp.resource_load_delay`.
    pub resource_load_delay: Option<f64>,
    /// AVG of `web_vitals.lcp.resource_load_time`.
    pub resource_load_time: Option<f64>,
    /// AVG of `web_vitals.lcp.element_render_delay`.
    pub element_render_delay: Option<f64>,
    /// Most-common `lcp.element` value among the route's `web_vitals.lcp` points (facet top-1).
    pub top_element: Option<String>,
}

/// Which vital a breakdown pass fills in. Copy so the same value is reused across the batch/row
/// loops without moving it.
#[derive(Debug, Clone, Copy)]
enum Vital {
    Lcp,
    Inp,
    Cls,
    /// `web_vitals.view_duration` — one point per finalized view, so its sample count is the true
    /// pageview count. Contributes only to `pageviews`; it has no p75 column in `BreakdownRow`.
    ViewDuration,
}

/// A conditional `SUM(0/1)` over rows whose `value` is non-null and lands in the rating band
/// bounded by `(lo, hi]` (either bound optional): `lo` is an exclusive lower bound, `hi` an
/// inclusive upper bound. Yields Int64. The CASE arms are both Int64 literals, so `otherwise`
/// cannot fail.
fn rating_count(value: &Expr, lo: Option<f64>, hi: Option<f64>) -> Expr {
    let mut cond = value.clone().is_not_null();
    if let Some(l) = lo {
        cond = cond.and(value.clone().gt(lit(l)));
    }
    if let Some(h) = hi {
        cond = cond.and(value.clone().lt_eq(lit(h)));
    }
    sum(when(cond, lit(1_i64))
        .otherwise(lit(0_i64))
        .expect("rating-count CASE has Int64 arms and cannot fail"))
}

/// The row predicate shared by every attribution read: the metric/time base predicate, AND
/// `service.name = service`, AND (when `route` is `Some`) `attributes['browser.route'] = route`
/// (collapsed through `IS TRUE` exactly like `rum_breakdown`'s route filter).
fn attribution_predicate(req: &MetricRequest, service: &str, route: Option<&str>) -> Expr {
    let mut pred = metric_base_predicate(req).and(col_ref(ATTR_SERVICE).eq(lit(service)));
    if let Some(r) = route {
        pred = pred.and(
            get_field(col_ref(metric_schema::ATTRIBUTES), ATTR_ROUTE)
                .eq(lit(r))
                .is_true(),
        );
    }
    pred
}

impl MetricsQueryEngine {
    /// Per-vital p75 + good/needs/poor rating distribution for `service` over `[start_ns, end_ns]`.
    /// One global aggregate per vital name; vitals with no samples in the window are omitted (as
    /// they already are once pruning drops files whose bloom lacks the metric).
    pub async fn rum_vitals(
        &self,
        service: &str,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Vec<VitalSummary>, PhotonError> {
        let mut out = Vec::new();
        for name in VITALS {
            let (good, poor) =
                thresholds(name).expect("every VITALS entry has a thresholds() entry");
            let req = MetricRequest {
                metric: name.to_string(),
                start_ts_nanos: start_ns,
                end_ts_nanos: end_ns,
                filter: None,
                host: None,
            };
            let Some(df) = self.survivors_df(&req).await? else {
                continue;
            };
            let value = col_ref(metric_schema::VALUE);
            let svc = col_ref(ATTR_SERVICE).eq(lit(service));

            let batches = df
                .filter(metric_base_predicate(&req).and(svc))
                .map_err(|e| PhotonError::Query(format!("rum_vitals filter: {e}")))?
                .aggregate(
                    vec![],
                    vec![
                        count(lit(1_i64)).alias("n"),
                        approx_percentile_cont(value.clone(), lit(0.75_f64), None).alias("p75"),
                        rating_count(&value, None, Some(good)).alias("good"),
                        rating_count(&value, Some(good), Some(poor)).alias("needs"),
                        rating_count(&value, Some(poor), None).alias("poor"),
                    ],
                )
                .map_err(|e| PhotonError::Query(format!("rum_vitals aggregate: {e}")))?
                .collect()
                .await
                .map_err(|e| PhotonError::Query(format!("rum_vitals collect: {e}")))?;

            // A global (no-GROUP-BY) aggregate yields exactly one row; find the first non-empty
            // batch and read row 0.
            let Some(b) = batches.iter().find(|b| b.num_rows() > 0) else {
                continue;
            };
            let i64_at = |c: usize| -> i64 {
                b.column(c)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .map(|a| if a.is_null(0) { 0 } else { a.value(0) })
                    .unwrap_or(0)
            };
            let n = i64_at(0);
            if n == 0 {
                continue; // no samples for this vital in the window
            }
            let p75 = b
                .column(1)
                .as_any()
                .downcast_ref::<Float64Array>()
                .and_then(|a| if a.is_null(0) { None } else { Some(a.value(0)) })
                .unwrap_or(0.0);
            out.push(VitalSummary {
                metric: name.to_string(),
                p75,
                count: n,
                good: i64_at(2),
                needs: i64_at(3),
                poor: i64_at(4),
            });
        }
        Ok(out)
    }

    /// LCP/INP/CLS p75 for `service`, grouped by a single `dimension` (a promoted column or a
    /// map attribute, resolved via `resolve_group_cols`). One grouped aggregate per vital,
    /// merged on the group key. A group's `pageviews` is the largest per-vital sample count seen
    /// for it; a vital absent from a group leaves its `*_p75` as `None`.
    ///
    /// `route`, when `Some(r)`, scopes every group to rows whose `browser.route` attribute equals
    /// `r` — used by the page-detail view to break a single page down by another dimension (e.g.
    /// device). `None` leaves the breakdown app-wide.
    pub async fn rum_breakdown(
        &self,
        service: &str,
        dimension: &str,
        start_ns: i64,
        end_ns: i64,
        route: Option<&str>,
    ) -> Result<Vec<BreakdownRow>, PhotonError> {
        // Resolve the single grouping dimension to a column Expr (promoted col or attributes[key]).
        let dim = self
            .resolve_group_cols(&[dimension.to_string()])?
            .pop()
            .ok_or_else(|| {
                PhotonError::Query(format!(
                    "rum_breakdown: could not resolve dimension `{dimension}`"
                ))
            })?;

        let mut rows: BTreeMap<String, BreakdownRow> = BTreeMap::new();
        // `view_duration` is in the merge so soft navigations count as pageviews: a clean soft
        // view (no layout shift, no slow interaction) emits NO LCP/INP/CLS point — only its
        // finalizing `view_duration` — and would otherwise be invisible in the pages breakdown.
        for (name, which) in [
            ("web_vitals.lcp", Vital::Lcp),
            ("web_vitals.inp", Vital::Inp),
            ("web_vitals.cls", Vital::Cls),
            (METRIC_VIEW_DURATION, Vital::ViewDuration),
        ] {
            let req = MetricRequest {
                metric: name.to_string(),
                start_ts_nanos: start_ns,
                end_ts_nanos: end_ns,
                filter: None,
                host: None,
            };
            let Some(df) = self.survivors_df(&req).await? else {
                continue;
            };
            let value = col_ref(metric_schema::VALUE);
            let svc = col_ref(ATTR_SERVICE).eq(lit(service));
            let mut pred = metric_base_predicate(&req).and(svc);
            if let Some(r) = route {
                // Scope to one page: `attributes['browser.route'] = r` (map attribute, collapsed
                // through `IS TRUE` exactly like the map-attribute predicates in `predicate.rs`).
                pred = pred.and(
                    get_field(col_ref(metric_schema::ATTRIBUTES), ATTR_ROUTE)
                        .eq(lit(r))
                        .is_true(),
                );
            }

            let batches = df
                .filter(pred)
                .map_err(|e| PhotonError::Query(format!("rum_breakdown filter: {e}")))?
                .aggregate(
                    vec![dim.clone().alias("k")],
                    vec![
                        count(lit(1_i64)).alias("n"),
                        approx_percentile_cont(value.clone(), lit(0.75_f64), None).alias("p75"),
                    ],
                )
                .map_err(|e| PhotonError::Query(format!("rum_breakdown aggregate: {e}")))?
                .collect()
                .await
                .map_err(|e| PhotonError::Query(format!("rum_breakdown collect: {e}")))?;

            for b in &batches {
                let keys = b
                    .column(0)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or_else(|| PhotonError::Query("rum_breakdown: key not Utf8".into()))?;
                let ns = b
                    .column(1)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .ok_or_else(|| PhotonError::Query("rum_breakdown: count not Int64".into()))?;
                let ps = b
                    .column(2)
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .ok_or_else(|| PhotonError::Query("rum_breakdown: p75 not Float64".into()))?;
                for i in 0..b.num_rows() {
                    if keys.is_null(i) {
                        continue; // a null group key is not a real dimension value
                    }
                    let key = keys.value(i).to_string();
                    let entry = rows.entry(key.clone()).or_insert_with(|| BreakdownRow {
                        key,
                        pageviews: 0,
                        lcp_p75: None,
                        inp_p75: None,
                        cls_p75: None,
                    });
                    let n = if ns.is_null(i) { 0 } else { ns.value(i) };
                    entry.pageviews = entry.pageviews.max(n);
                    let p75 = if ps.is_null(i) {
                        None
                    } else {
                        Some(ps.value(i))
                    };
                    match which {
                        Vital::Lcp => entry.lcp_p75 = p75,
                        Vital::Inp => entry.inp_p75 = p75,
                        Vital::Cls => entry.cls_p75 = p75,
                        Vital::ViewDuration => {} // pageview counting only (via the max above)
                    }
                }
            }
        }
        Ok(rows.into_values().collect())
    }

    /// Per-page LCP attribution for `service` over `[start_ns, end_ns]`: the AVG of each of the
    /// four LCP sub-part gauges (`web_vitals.lcp.{ttfb,resource_load_delay,resource_load_time,
    /// element_render_delay}`) plus the most-common `lcp.element` among the route's
    /// `web_vitals.lcp` points. `route`, when `Some(r)`, scopes every read to
    /// `attributes['browser.route'] = r` — the page-detail "why is LCP slow" panel. A sub-part
    /// with no samples (e.g. the SDK ran without attribution) is left `None`.
    pub async fn rum_lcp_attribution(
        &self,
        service: &str,
        route: Option<&str>,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<LcpAttribution, PhotonError> {
        Ok(LcpAttribution {
            ttfb: self
                .avg_lcp_subpart(METRIC_LCP_TTFB, service, route, start_ns, end_ns)
                .await?,
            resource_load_delay: self
                .avg_lcp_subpart(
                    METRIC_LCP_RESOURCE_LOAD_DELAY,
                    service,
                    route,
                    start_ns,
                    end_ns,
                )
                .await?,
            resource_load_time: self
                .avg_lcp_subpart(
                    METRIC_LCP_RESOURCE_LOAD_TIME,
                    service,
                    route,
                    start_ns,
                    end_ns,
                )
                .await?,
            element_render_delay: self
                .avg_lcp_subpart(
                    METRIC_LCP_ELEMENT_RENDER_DELAY,
                    service,
                    route,
                    start_ns,
                    end_ns,
                )
                .await?,
            top_element: self
                .top_lcp_element(service, route, start_ns, end_ns)
                .await?,
        })
    }

    /// AVG of one LCP sub-part gauge (`metric`) for `service` (+ optional `route`). `None` when the
    /// metric has no surviving files or no matching rows in the window.
    async fn avg_lcp_subpart(
        &self,
        metric: &str,
        service: &str,
        route: Option<&str>,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Option<f64>, PhotonError> {
        let req = MetricRequest {
            metric: metric.to_string(),
            start_ts_nanos: start_ns,
            end_ts_nanos: end_ns,
            filter: None,
            host: None,
        };
        let Some(df) = self.survivors_df(&req).await? else {
            return Ok(None);
        };
        let value = col_ref(metric_schema::VALUE);
        let batches = df
            .filter(attribution_predicate(&req, service, route))
            .map_err(|e| PhotonError::Query(format!("rum_lcp_attribution filter: {e}")))?
            .aggregate(vec![], vec![avg(value).alias("avg")])
            .map_err(|e| PhotonError::Query(format!("rum_lcp_attribution aggregate: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("rum_lcp_attribution collect: {e}")))?;
        // A global (no-GROUP-BY) AVG yields one row; a metric with no matching rows yields a NULL.
        let Some(b) = batches.iter().find(|b| b.num_rows() > 0) else {
            return Ok(None);
        };
        Ok(b.column(0)
            .as_any()
            .downcast_ref::<Float64Array>()
            .and_then(|a| if a.is_null(0) { None } else { Some(a.value(0)) }))
    }

    /// The most-common `lcp.element` attribute value among `web_vitals.lcp` points for `service`
    /// (+ optional `route`) — a facet-style group-by-value + count-desc, take top 1 (modeled on
    /// `facet.rs`). `None` when no LCP point in the window carries an `lcp.element`.
    async fn top_lcp_element(
        &self,
        service: &str,
        route: Option<&str>,
        start_ns: i64,
        end_ns: i64,
    ) -> Result<Option<String>, PhotonError> {
        let req = MetricRequest {
            metric: "web_vitals.lcp".to_string(),
            start_ts_nanos: start_ns,
            end_ts_nanos: end_ns,
            filter: None,
            host: None,
        };
        let Some(df) = self.survivors_df(&req).await? else {
            return Ok(None);
        };
        let element = get_field(col_ref(metric_schema::ATTRIBUTES), ATTR_LCP_ELEMENT);
        let batches = df
            .filter(attribution_predicate(&req, service, route))
            .map_err(|e| PhotonError::Query(format!("top_lcp_element filter: {e}")))?
            .aggregate(
                vec![element.alias("el")],
                vec![count(lit(1_i64)).alias("n")],
            )
            .map_err(|e| PhotonError::Query(format!("top_lcp_element aggregate: {e}")))?
            .filter(col("el").is_not_null())
            .map_err(|e| PhotonError::Query(format!("top_lcp_element not-null: {e}")))?
            .sort(vec![
                col("n").sort(false, false), // count desc
                col("el").sort(true, false), // value asc — stable tiebreak
            ])
            .map_err(|e| PhotonError::Query(format!("top_lcp_element sort: {e}")))?
            .limit(0, Some(1))
            .map_err(|e| PhotonError::Query(format!("top_lcp_element limit: {e}")))?
            .collect()
            .await
            .map_err(|e| PhotonError::Query(format!("top_lcp_element collect: {e}")))?;
        for b in &batches {
            if b.num_rows() == 0 {
                continue;
            }
            let els = b
                .column(0)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| PhotonError::Query("top_lcp_element: element not Utf8".into()))?;
            if !els.is_null(0) {
                return Ok(Some(els.value(0).to_string()));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use photon_core::metric_record::{MetricBatchBuilder, MetricPoint};
    use photon_core::metric_schema::{metric_type, MetricSchema};

    /// One gauge web-vitals point: `service.name` promoted, `device.type` in the attributes map.
    fn vp(name: &str, service: &str, device: &str, value: f64) -> MetricPoint {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".to_string(), service.to_string());
        attributes.insert("device.type".to_string(), device.to_string());
        MetricPoint {
            metric_name: name.to_string(),
            metric_type: metric_type::GAUGE,
            timestamp_nanos: 1_000,
            value: Some(value),
            attributes,
            ..Default::default()
        }
    }

    /// An engine whose `survivors_df` serves a single hand-built metrics batch (the `from_batch`
    /// test seam — no pruning, no Parquet). `service.name` is the only promoted attribute.
    fn engine_with_points(points: Vec<MetricPoint>) -> MetricsQueryEngine {
        let schema = MetricSchema::new(&["service.name".to_string()]);
        let mut b = MetricBatchBuilder::new(&schema);
        for p in &points {
            b.append(p);
        }
        MetricsQueryEngine::from_batch(schema, b.finish().unwrap())
    }

    #[tokio::test]
    async fn vitals_p75_and_rating_distribution() {
        let engine = engine_with_points(vec![
            vp("web_vitals.lcp", "web", "mobile", 2000.0),
            vp("web_vitals.lcp", "web", "mobile", 2600.0),
            vp("web_vitals.lcp", "web", "desktop", 3000.0),
            vp("web_vitals.lcp", "web", "mobile", 4300.0),
        ]);
        let out = engine.rum_vitals("web", 0, i64::MAX).await.unwrap();
        let lcp = out.iter().find(|v| v.metric == "web_vitals.lcp").unwrap();
        // p75 is a t-digest approximation over 4 points; assert it lands in the upper region of
        // the distribution (pulled up past 3000 by the 4300 tail) rather than pinning an exact
        // value. DataFusion 43 returns 3650 here; the brief's `abs(p75 - 4300) < 500` is tighter
        // than the real approximation supports.
        assert!(
            lcp.p75 > 3000.0 && lcp.p75 <= 4300.0,
            "p75 should land in the upper distribution region, got {}",
            lcp.p75
        );
        assert_eq!(lcp.good + lcp.needs + lcp.poor, 4);
        assert_eq!(lcp.poor, 1); // the 4300 value is > 4000
                                 // good_max=2500 -> only 2000 is good; needs band (2500,4000] -> 2600 & 3000.
        assert_eq!(lcp.good, 1);
        assert_eq!(lcp.needs, 2);
        assert_eq!(lcp.count, 4);
        // Only LCP has samples; the other four vitals are omitted (no data).
        assert_eq!(out.len(), 1);
    }

    #[tokio::test]
    async fn route_change_scorecard_is_produced() {
        // Mirrors `vitals_p75_and_rating_distribution` above, but for the SPA soft-navigation
        // `web_vitals.route_change` gauge (thresholds (1000, 3000) per `photon_core::rum::thresholds`).
        let engine = engine_with_points(vec![
            vp("web_vitals.route_change", "web", "mobile", 800.0),
            vp("web_vitals.route_change", "web", "mobile", 2500.0),
        ]);
        let out = engine.rum_vitals("web", 0, i64::MAX).await.unwrap();
        let rc = out
            .iter()
            .find(|v| v.metric == "web_vitals.route_change")
            .expect("route_change scorecard");
        assert!(rc.p75.is_finite());
        assert_eq!(rc.count, 2);
        assert_eq!(rc.good + rc.needs + rc.poor, 2);
        assert_eq!(rc.good, 1); // 800 <= good_max(1000)
        assert_eq!(rc.needs, 1); // 1000 < 2500 <= poor_min(3000)
        assert_eq!(rc.poor, 0);
    }

    #[tokio::test]
    async fn breakdown_groups_by_device() {
        let engine = engine_with_points(vec![
            vp("web_vitals.lcp", "web", "mobile", 5000.0),
            vp("web_vitals.lcp", "web", "desktop", 2000.0),
        ]);
        let rows = engine
            .rum_breakdown("web", "device.type", 0, i64::MAX, None)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        let mobile = rows.iter().find(|r| r.key == "mobile").unwrap();
        assert!(mobile.lcp_p75.unwrap() > 4000.0);
        // INP/CLS have no samples, so those columns stay None.
        assert_eq!(mobile.inp_p75, None);
        assert_eq!(mobile.cls_p75, None);
        assert_eq!(mobile.pageviews, 1);
        let desktop = rows.iter().find(|r| r.key == "desktop").unwrap();
        assert!(desktop.lcp_p75.unwrap() < 4000.0);
    }

    /// One gauge web-vitals point carrying a `browser.route`, so route-scoped breakdowns can be
    /// exercised (the base `vp` helper omits it).
    fn vp_route(name: &str, service: &str, route: &str, device: &str, value: f64) -> MetricPoint {
        let mut p = vp(name, service, device, value);
        p.attributes
            .insert("browser.route".to_string(), route.to_string());
        p
    }

    #[tokio::test]
    async fn breakdown_route_filter_scopes_to_one_page() {
        let engine = engine_with_points(vec![
            vp_route("web_vitals.lcp", "web", "/checkout", "mobile", 5000.0),
            vp_route("web_vitals.lcp", "web", "/checkout", "desktop", 2000.0),
            vp_route("web_vitals.lcp", "web", "/home", "mobile", 1000.0),
        ]);
        // No route filter: both pages' devices show up (mobile appears on both routes).
        let all = engine
            .rum_breakdown("web", "device.type", 0, i64::MAX, None)
            .await
            .unwrap();
        let mobile_all = all.iter().find(|r| r.key == "mobile").unwrap();
        assert_eq!(mobile_all.pageviews, 2); // /checkout + /home

        // Scoped to `/checkout`: only its two rows are counted; /home's mobile row is excluded.
        let scoped = engine
            .rum_breakdown("web", "device.type", 0, i64::MAX, Some("/checkout"))
            .await
            .unwrap();
        assert_eq!(scoped.len(), 2); // mobile + desktop, both on /checkout
        let mobile = scoped.iter().find(|r| r.key == "mobile").unwrap();
        assert_eq!(mobile.pageviews, 1); // only the /checkout mobile row
        assert!(mobile.lcp_p75.unwrap() > 4000.0);
    }

    #[tokio::test]
    async fn breakdown_counts_soft_views_without_core_vitals_as_pageviews() {
        // A clean SPA soft navigation emits NO LCP/INP/CLS — only its finalizing
        // `view_duration` (and usually `route_change`, which the breakdown ignores).
        // Such a route must still appear in the by-route pages breakdown.
        let engine = engine_with_points(vec![
            // hard landing on /home: LCP + a view_duration
            vp_route("web_vitals.lcp", "web", "/home", "desktop", 2000.0),
            vp_route("web_vitals.view_duration", "web", "/home", "desktop", 12000.0),
            // two clean soft views of /settings: view_duration only
            vp_route("web_vitals.view_duration", "web", "/settings", "desktop", 3000.0),
            vp_route("web_vitals.view_duration", "web", "/settings", "desktop", 4500.0),
        ]);
        let rows = engine
            .rum_breakdown("web", "browser.route", 0, i64::MAX, None)
            .await
            .unwrap();

        // /settings shows up purely from view_duration, with no vital p75s.
        let settings = rows.iter().find(|r| r.key == "/settings").expect(
            "route with only view_duration samples must appear in the pages breakdown",
        );
        assert_eq!(settings.pageviews, 2);
        assert_eq!(settings.lcp_p75, None);
        assert_eq!(settings.inp_p75, None);
        assert_eq!(settings.cls_p75, None);

        // /home merges both sources: max(1 lcp, 1 view_duration) = 1, and keeps its LCP p75.
        let home = rows.iter().find(|r| r.key == "/home").unwrap();
        assert_eq!(home.pageviews, 1);
        assert!(home.lcp_p75.is_some());
    }

    // ---- Task F2: LCP attribution ---------------------------------------------------------

    /// A gauge point with `service.name` promoted and an arbitrary long-tail attribute set.
    fn mp(name: &str, service: &str, value: f64, attrs: &[(&str, &str)]) -> MetricPoint {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".to_string(), service.to_string());
        for (k, v) in attrs {
            attributes.insert(k.to_string(), v.to_string());
        }
        MetricPoint {
            metric_name: name.to_string(),
            metric_type: metric_type::GAUGE,
            timestamp_nanos: 1_000,
            value: Some(value),
            attributes,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn lcp_attribution_averages_subparts_and_top_element() {
        let engine = engine_with_points(vec![
            // Main LCP points carrying `lcp.element` — #hero appears twice, #banner once.
            mp("web_vitals.lcp", "web", 4300.0, &[("lcp.element", "#hero")]),
            mp("web_vitals.lcp", "web", 4000.0, &[("lcp.element", "#hero")]),
            mp(
                "web_vitals.lcp",
                "web",
                2000.0,
                &[("lcp.element", "#banner")],
            ),
            // Sub-part gauges: ttfb averages to 150; the rest have a single sample each.
            mp("web_vitals.lcp.ttfb", "web", 100.0, &[]),
            mp("web_vitals.lcp.ttfb", "web", 200.0, &[]),
            mp("web_vitals.lcp.resource_load_delay", "web", 30.0, &[]),
            mp("web_vitals.lcp.resource_load_time", "web", 900.0, &[]),
            mp("web_vitals.lcp.element_render_delay", "web", 50.0, &[]),
        ]);
        let a = engine
            .rum_lcp_attribution("web", None, 0, i64::MAX)
            .await
            .unwrap();
        assert_eq!(a.ttfb, Some(150.0));
        assert_eq!(a.resource_load_delay, Some(30.0));
        assert_eq!(a.resource_load_time, Some(900.0));
        assert_eq!(a.element_render_delay, Some(50.0));
        assert_eq!(a.top_element.as_deref(), Some("#hero"));
    }

    #[tokio::test]
    async fn lcp_attribution_missing_subparts_are_none() {
        // Only a main LCP point (no `lcp.element`, no sub-part gauges) → all sub-parts + element None.
        let engine = engine_with_points(vec![mp("web_vitals.lcp", "web", 3000.0, &[])]);
        let a = engine
            .rum_lcp_attribution("web", None, 0, i64::MAX)
            .await
            .unwrap();
        assert_eq!(a, LcpAttribution::default());
    }

    #[tokio::test]
    async fn lcp_attribution_route_scoped_excludes_other_pages() {
        let engine = engine_with_points(vec![
            // /checkout: ttfb {100, 300} avg 200; element #hero (x2).
            mp(
                "web_vitals.lcp",
                "web",
                4300.0,
                &[("browser.route", "/checkout"), ("lcp.element", "#hero")],
            ),
            mp(
                "web_vitals.lcp",
                "web",
                4000.0,
                &[("browser.route", "/checkout"), ("lcp.element", "#hero")],
            ),
            mp(
                "web_vitals.lcp.ttfb",
                "web",
                100.0,
                &[("browser.route", "/checkout")],
            ),
            mp(
                "web_vitals.lcp.ttfb",
                "web",
                300.0,
                &[("browser.route", "/checkout")],
            ),
            // /home: a lower ttfb + a different element that must be excluded when scoped.
            mp(
                "web_vitals.lcp",
                "web",
                1000.0,
                &[("browser.route", "/home"), ("lcp.element", "#logo")],
            ),
            mp(
                "web_vitals.lcp.ttfb",
                "web",
                20.0,
                &[("browser.route", "/home")],
            ),
        ]);
        let a = engine
            .rum_lcp_attribution("web", Some("/checkout"), 0, i64::MAX)
            .await
            .unwrap();
        assert_eq!(a.ttfb, Some(200.0)); // only /checkout's {100,300}, not /home's 20
        assert_eq!(a.top_element.as_deref(), Some("#hero")); // /home's #logo excluded
    }
}
