//! Alert delivery. v1 = a generic JSON webhook per channel, optional HMAC-SHA256 signature and
//! custom headers. Delivery is detached, retried ≤3×, non-fatal — never blocks the eval loop.
use crate::model::{Channel, Rule};
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::Duration;

#[derive(Clone, Copy)]
pub enum NotifyStatus {
    Triggered,
    Resolved,
}
impl NotifyStatus {
    fn as_str(self) -> &'static str {
        match self {
            NotifyStatus::Triggered => "triggered",
            NotifyStatus::Resolved => "resolved",
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_payload(
    rule: &Rule,
    labels: &serde_json::Value,
    status: NotifyStatus,
    value: f64,
    threshold: f64,
    started_at: i64,
    at: i64,
    incident_id: i64,
) -> serde_json::Value {
    serde_json::json!({
        "status": status.as_str(),
        "rule": { "id": rule.id, "name": rule.name,
                  "severity": rule.severity, "signal": rule.condition.signal() },
        "series": labels,
        "condition": rule.condition.summary(),
        "value": value, "threshold": threshold,
        "started_at": started_at, "at": at, "incident_id": incident_id,
    })
}

/// `sha256=<hex>` HMAC of `body` under `secret`.
pub fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(body);
    let bytes = mac.finalize().into_bytes();
    let mut hex = String::from("sha256=");
    for b in bytes {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

#[async_trait]
pub trait Notifier: Send + Sync {
    /// Fire-and-forget: returns immediately; delivery + retries happen in a detached task.
    async fn deliver(&self, channel: &Channel, payload: serde_json::Value);
}

pub struct WebhookNotifier {
    client: reqwest::Client,
}
impl WebhookNotifier {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client"),
        }
    }
}
impl Default for WebhookNotifier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Notifier for WebhookNotifier {
    async fn deliver(&self, channel: &Channel, payload: serde_json::Value) {
        let client = self.client.clone();
        let url = channel.url.clone();
        let secret = channel.secret.clone();
        let headers = channel.headers.clone();
        let body = serde_json::to_vec(&payload).unwrap_or_default();
        tokio::spawn(async move {
            for attempt in 1..=3u32 {
                let mut req = client
                    .post(&url)
                    .header("content-type", "application/json")
                    .body(body.clone());
                if let Some(s) = &secret {
                    req = req.header("X-Photon-Signature", sign(s, &body));
                }
                if let Some(serde_json::Value::Object(map)) = &headers {
                    for (k, v) in map {
                        if let Some(v) = v.as_str() {
                            req = req.header(k, v);
                        }
                    }
                }
                match req.send().await {
                    Ok(r) if r.status().is_success() => return,
                    Ok(r) => eprintln!(
                        "alert webhook {url}: HTTP {} (attempt {attempt})",
                        r.status()
                    ),
                    Err(e) => eprintln!("alert webhook {url}: {e} (attempt {attempt})"),
                }
                if attempt < 3 {
                    tokio::time::sleep(Duration::from_millis(200 * attempt as u64)).await;
                }
            }
        });
    }
}

/// Test double: records `(channel_id, status_string)` per `deliver`.
#[derive(Default)]
pub struct FakeNotifier {
    pub calls: std::sync::Mutex<Vec<(String, String)>>,
}
#[async_trait]
impl Notifier for FakeNotifier {
    async fn deliver(&self, channel: &Channel, payload: serde_json::Value) {
        let status = payload
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        self.calls
            .lock()
            .unwrap()
            .push((channel.id.clone(), status));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn rule() -> Rule {
        Rule {
            id: "r1".into(),
            name: "High CPU".into(),
            description: None,
            enabled: true,
            condition: Condition::Metrics(MetricCondition {
                metric_name: "system.cpu.utilization".into(),
                label_filters: Default::default(),
                group_by: vec!["host.name".into()],
                agg: MetricAgg::Avg,
                window_secs: 300,
                cmp: Cmp::Gt,
                threshold: 0.9,
            }),
            for_secs: 300,
            interval_secs: 60,
            severity: Severity::Warning,
            channel_ids: vec!["c1".into()],
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn payload_has_stable_shape_and_status() {
        let labels = serde_json::json!({ "host.name": "web-01" });
        let p = build_payload(
            &rule(),
            &labels,
            NotifyStatus::Triggered,
            0.94,
            0.90,
            1000,
            2000,
            7,
        );
        assert_eq!(p["status"], "triggered");
        assert_eq!(p["rule"]["name"], "High CPU");
        assert_eq!(p["rule"]["signal"], "metrics");
        assert_eq!(p["series"]["host.name"], "web-01");
        assert_eq!(p["value"], 0.94);
        assert_eq!(p["threshold"], 0.90);
        assert_eq!(p["incident_id"], 7);
    }

    #[test]
    fn hmac_signature_is_deterministic() {
        let sig = sign("shhh", br#"{"a":1}"#);
        assert_eq!(sig, sign("shhh", br#"{"a":1}"#));
        assert!(sig.starts_with("sha256="));
        assert_ne!(sig, sign("other", br#"{"a":1}"#));
    }

    #[tokio::test]
    async fn fake_notifier_records() {
        let n = FakeNotifier::default();
        let ch = Channel {
            id: "c1".into(),
            name: "ops".into(),
            kind: ChannelKind::Webhook,
            url: "http://x".into(),
            secret: None,
            headers: None,
            created_at: 0,
            updated_at: 0,
        };
        n.deliver(&ch, serde_json::json!({"status":"resolved"}))
            .await;
        assert_eq!(
            n.calls.lock().unwrap().as_slice(),
            &[("c1".to_string(), "resolved".to_string())]
        );
    }
}
