//! Alert delivery. Renders each alert into its channel's provider-native request (`format::render`)
//! then POSTs it. `deliver` is fire-and-forget (detached, retried ≤3×, non-fatal); `deliver_once`
//! is a single awaited POST used by the synchronous channel-test route.
use crate::format::{render, AlertEvent};
use crate::model::Channel;
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub enum NotifyStatus {
    Triggered,
    Resolved,
}
impl NotifyStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            NotifyStatus::Triggered => "triggered",
            NotifyStatus::Resolved => "resolved",
        }
    }
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
    /// Fire-and-forget: returns immediately; render + POST + retries happen in a detached task.
    async fn deliver(&self, channel: &Channel, event: AlertEvent);
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

impl WebhookNotifier {
    /// Render + POST exactly once, awaited, returning the real outcome. Used by the channel-test
    /// route so the UI can show whether a preset's URL/token actually works.
    pub async fn deliver_once(&self, channel: &Channel, event: &AlertEvent) -> Result<(), String> {
        let req = render(channel, event);
        let mut b = self
            .client
            .post(&req.url)
            .header("content-type", req.content_type)
            .body(req.body);
        for (k, v) in req.headers {
            b = b.header(k, v);
        }
        match b.send().await {
            Ok(r) if r.status().is_success() => Ok(()),
            Ok(r) => {
                let code = r.status();
                let snippet = r.text().await.unwrap_or_default();
                let snippet: String = snippet.chars().take(200).collect();
                Err(format!("HTTP {code}: {snippet}"))
            }
            Err(e) => Err(e.to_string()),
        }
    }
}

#[async_trait]
impl Notifier for WebhookNotifier {
    async fn deliver(&self, channel: &Channel, event: AlertEvent) {
        let client = self.client.clone();
        let req = render(channel, &event);
        tokio::spawn(async move {
            for attempt in 1..=3u32 {
                let mut b = client
                    .post(&req.url)
                    .header("content-type", req.content_type)
                    .body(req.body.clone());
                for (k, v) in &req.headers {
                    b = b.header(k, v);
                }
                match b.send().await {
                    Ok(r) if r.status().is_success() => return,
                    Ok(r) => eprintln!(
                        "alert webhook {}: HTTP {} (attempt {attempt})",
                        req.url,
                        r.status()
                    ),
                    Err(e) => eprintln!("alert webhook {}: {e} (attempt {attempt})", req.url),
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
    async fn deliver(&self, channel: &Channel, event: AlertEvent) {
        self.calls
            .lock()
            .unwrap()
            .push((channel.id.clone(), event.status.as_str().to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{build_generic_payload, AlertEvent};
    use crate::model::*;

    fn ev() -> AlertEvent {
        AlertEvent {
            rule_id: "r1".into(),
            rule_name: "High CPU".into(),
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

    #[test]
    fn generic_payload_has_stable_shape() {
        let p = build_generic_payload(&ev());
        assert_eq!(p["status"], "triggered");
        assert_eq!(p["rule"]["name"], "High CPU");
        assert_eq!(p["series"]["host.name"], "web-01");
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
    async fn fake_notifier_records_status() {
        let n = FakeNotifier::default();
        let ch = Channel {
            id: "c1".into(),
            name: "ops".into(),
            kind: ChannelKind::Webhook,
            config: ChannelConfig::Webhook {
                url: "http://x".into(),
                secret: None,
                headers: None,
            },
            created_at: 0,
            updated_at: 0,
        };
        let mut resolved = ev();
        resolved.status = NotifyStatus::Resolved;
        n.deliver(&ch, resolved).await;
        assert_eq!(
            n.calls.lock().unwrap().as_slice(),
            &[("c1".to_string(), "resolved".to_string())]
        );
    }
}
