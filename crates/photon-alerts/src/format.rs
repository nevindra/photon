//! Provider-native rendering of an alert into a delivery request. Pure (no network): each preset
//! maps an `AlertEvent` + a `Channel`'s `config` to a `RenderedRequest { url, body, headers }`.
//! The Generic formatter reproduces the documented stable JSON body (golden-tested); Discord/
//! Telegram render their own native shapes. HMAC signing for the Generic kind happens here so the
//! signature covers the exact bytes we POST.
use crate::model::{Channel, ChannelConfig, Severity};
use crate::notify::{sign, NotifyStatus};

/// Everything a formatter needs about one triggered/resolved evaluation. Denormalized (no `Rule`)
/// so the test path can synthesize a representative event without a stored rule.
#[derive(Clone, Debug)]
pub struct AlertEvent {
    pub rule_id: String,
    pub rule_name: String,
    pub severity: Severity,
    pub signal: String,
    pub condition_summary: String,
    pub labels: serde_json::Value,
    pub status: NotifyStatus,
    pub value: f64,
    pub threshold: f64,
    pub started_at: i64,
    pub at: i64,
    pub incident_id: i64,
}

/// A fully-rendered HTTP request the notifier will POST.
#[derive(Clone, Debug)]
pub struct RenderedRequest {
    pub url: String,
    pub body: Vec<u8>,
    pub content_type: &'static str,
    pub headers: Vec<(String, String)>,
}

fn severity_str(s: Severity) -> &'static str {
    match s {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Critical => "critical",
    }
}

/// The documented generic webhook JSON body — the single source of truth for the Webhook kind and
/// the shape external integrations parse. Do NOT change field names/order semantics.
pub fn build_generic_payload(ev: &AlertEvent) -> serde_json::Value {
    serde_json::json!({
        "status": ev.status.as_str(),
        "rule": { "id": ev.rule_id, "name": ev.rule_name,
                  "severity": severity_str(ev.severity), "signal": ev.signal },
        "series": ev.labels,
        "condition": ev.condition_summary,
        "value": ev.value, "threshold": ev.threshold,
        "started_at": ev.started_at, "at": ev.at, "incident_id": ev.incident_id,
    })
}

/// Discord embed color (decimal RGB): resolved = green; else by severity.
fn discord_color(ev: &AlertEvent) -> u32 {
    match ev.status {
        NotifyStatus::Resolved => 0x2ECC71,
        NotifyStatus::Triggered => match ev.severity {
            Severity::Critical => 0xE74C3C,
            Severity::Warning => 0xF39C12,
            Severity::Info => 0x3498DB,
        },
    }
}

fn status_emoji(status: NotifyStatus) -> &'static str {
    match status {
        NotifyStatus::Triggered => "🔴",
        NotifyStatus::Resolved => "✅",
    }
}

/// Render the human-readable series label (`"k=v · k=v"`, or `"—"` when aggregate).
fn series_line(labels: &serde_json::Value) -> String {
    match labels.as_object() {
        Some(m) if !m.is_empty() => m
            .iter()
            .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("")))
            .collect::<Vec<_>>()
            .join(" · "),
        _ => "—".into(),
    }
}

/// Minimal HTML escaping for Telegram `parse_mode: HTML` (only these three are required).
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn render(channel: &Channel, ev: &AlertEvent) -> RenderedRequest {
    match &channel.config {
        ChannelConfig::Webhook {
            url,
            secret,
            headers,
        } => {
            let body = serde_json::to_vec(&build_generic_payload(ev)).unwrap_or_default();
            let mut hdrs = Vec::new();
            if let Some(s) = secret {
                hdrs.push(("X-Photon-Signature".to_string(), sign(s, &body)));
            }
            if let Some(serde_json::Value::Object(map)) = headers {
                for (k, v) in map {
                    if let Some(v) = v.as_str() {
                        hdrs.push((k.clone(), v.to_string()));
                    }
                }
            }
            RenderedRequest {
                url: url.clone(),
                body,
                content_type: "application/json",
                headers: hdrs,
            }
        }
        ChannelConfig::Discord { webhook_url } => {
            let verb = match ev.status {
                NotifyStatus::Triggered => "Triggered",
                NotifyStatus::Resolved => "Resolved",
            };
            let payload = serde_json::json!({
                "embeds": [{
                    "title": format!("{} {verb} · {}", status_emoji(ev.status), ev.rule_name),
                    "description": ev.condition_summary,
                    "color": discord_color(ev),
                    "fields": [
                        { "name": "Value", "value": ev.value.to_string(), "inline": true },
                        { "name": "Threshold", "value": ev.threshold.to_string(), "inline": true },
                        { "name": "Severity", "value": severity_str(ev.severity), "inline": true },
                        { "name": "Series", "value": series_line(&ev.labels), "inline": false },
                    ],
                }],
            });
            RenderedRequest {
                url: webhook_url.clone(),
                body: serde_json::to_vec(&payload).unwrap_or_default(),
                content_type: "application/json",
                headers: Vec::new(),
            }
        }
        ChannelConfig::Telegram { bot_token, chat_id } => {
            let verb = match ev.status {
                NotifyStatus::Triggered => "Triggered",
                NotifyStatus::Resolved => "Resolved",
            };
            let text = format!(
                "{} <b>{} · {}</b>\n{}\nValue: {}  (threshold {})\nSeverity: {} · Series: {}",
                status_emoji(ev.status),
                verb,
                html_escape(&ev.rule_name),
                html_escape(&ev.condition_summary),
                ev.value,
                ev.threshold,
                severity_str(ev.severity),
                html_escape(&series_line(&ev.labels)),
            );
            let payload =
                serde_json::json!({ "chat_id": chat_id, "text": text, "parse_mode": "HTML" });
            RenderedRequest {
                url: format!("https://api.telegram.org/bot{bot_token}/sendMessage"),
                body: serde_json::to_vec(&payload).unwrap_or_default(),
                content_type: "application/json",
                headers: Vec::new(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ChannelKind;

    fn ev() -> AlertEvent {
        AlertEvent {
            rule_id: "r1".into(),
            rule_name: "web-01 high CPU".into(),
            severity: Severity::Warning,
            signal: "metrics".into(),
            condition_summary: "Avg(system.cpu.utilization) > 0.9".into(),
            labels: serde_json::json!({ "host.name": "web-01" }),
            status: NotifyStatus::Triggered,
            value: 0.94,
            threshold: 0.9,
            started_at: 1000,
            at: 2000,
            incident_id: 7,
        }
    }
    fn ch(config: ChannelConfig) -> Channel {
        Channel {
            id: "c1".into(),
            name: "n".into(),
            kind: config.kind(),
            config,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn generic_body_matches_documented_shape() {
        // GOLDEN: the exact stable shape external integrations parse. Do not loosen.
        let p = build_generic_payload(&ev());
        assert_eq!(p["status"], "triggered");
        assert_eq!(p["rule"]["id"], "r1");
        assert_eq!(p["rule"]["name"], "web-01 high CPU");
        assert_eq!(p["rule"]["severity"], "warning");
        assert_eq!(p["rule"]["signal"], "metrics");
        assert_eq!(p["series"]["host.name"], "web-01");
        assert_eq!(p["condition"], "Avg(system.cpu.utilization) > 0.9");
        assert_eq!(p["value"], 0.94);
        assert_eq!(p["threshold"], 0.9);
        assert_eq!(p["started_at"], 1000);
        assert_eq!(p["at"], 2000);
        assert_eq!(p["incident_id"], 7);
    }

    #[test]
    fn webhook_render_signs_and_merges_headers() {
        let c = ch(ChannelConfig::Webhook {
            url: "https://hook".into(),
            secret: Some("shh".into()),
            headers: Some(serde_json::json!({ "X-A": "1" })),
        });
        let r = render(&c, &ev());
        assert_eq!(r.url, "https://hook");
        assert!(r
            .headers
            .iter()
            .any(|(k, v)| k == "X-Photon-Signature" && v.starts_with("sha256=")));
        assert!(r.headers.iter().any(|(k, v)| k == "X-A" && v == "1"));
    }

    #[test]
    fn discord_render_builds_embed_with_severity_color() {
        let c = ch(ChannelConfig::Discord {
            webhook_url: "https://discord.com/api/webhooks/1/x".into(),
        });
        let r = render(&c, &ev());
        assert_eq!(r.url, "https://discord.com/api/webhooks/1/x");
        let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        let embed = &v["embeds"][0];
        assert_eq!(embed["color"], 0xF39C12); // warning amber
        assert!(embed["title"]
            .as_str()
            .unwrap()
            .contains("Triggered · web-01 high CPU"));
        assert_eq!(embed["fields"][3]["value"], "host.name=web-01");
    }

    #[test]
    fn telegram_render_constructs_bot_url_and_html_text() {
        let c = ch(ChannelConfig::Telegram {
            bot_token: "123:abc".into(),
            chat_id: "-100".into(),
        });
        let r = render(&c, &ev());
        assert_eq!(r.url, "https://api.telegram.org/bot123:abc/sendMessage");
        let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(v["chat_id"], "-100");
        assert_eq!(v["parse_mode"], "HTML");
        assert!(v["text"].as_str().unwrap().contains("&gt;")); // '>' in the summary was escaped
    }

    #[test]
    fn resolved_discord_is_green_regardless_of_severity() {
        let mut e = ev();
        e.status = NotifyStatus::Resolved;
        e.severity = Severity::Critical;
        let c = ch(ChannelConfig::Discord {
            webhook_url: "https://discord.com/api/webhooks/1/x".into(),
        });
        let r = render(&c, &e);
        let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(v["embeds"][0]["color"], 0x2ECC71);
        // Sanity: the kind derivation used by construction is Discord.
        assert_eq!(c.kind, ChannelKind::Discord);
    }
}
