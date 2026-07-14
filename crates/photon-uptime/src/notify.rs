//! State-change notifications. v1 = a generic JSON webhook. Delivery failures are logged
//! and retried a couple of times, never fatal to the scheduler.

use crate::model::{CheckResult, Monitor, MonitorState, Transition};
use photon_core::PhotonError;
use std::time::Duration;

pub struct NotifyEvent<'a> {
    pub monitor: &'a Monitor,
    pub transition: Transition,
    pub at: i64,
    pub result: &'a CheckResult,
}

#[async_trait::async_trait]
pub trait Notifier: Send + Sync {
    async fn notify(&self, ev: &NotifyEvent<'_>) -> Result<(), PhotonError>;
}

pub struct WebhookNotifier {
    client: reqwest::Client,
    global_url: Option<String>,
}

impl WebhookNotifier {
    pub fn new(global_url: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client");
        Self { client, global_url }
    }
    pub fn resolve_url(&self, m: &Monitor) -> Option<String> {
        m.webhook_url.clone().or_else(|| self.global_url.clone())
    }
}

#[async_trait::async_trait]
impl Notifier for WebhookNotifier {
    async fn notify(&self, ev: &NotifyEvent<'_>) -> Result<(), PhotonError> {
        let Some(url) = self.resolve_url(ev.monitor) else {
            return Ok(());
        };
        let state = match ev.transition {
            Transition::WentDown => MonitorState::Down,
            Transition::Recovered => MonitorState::Up,
        };
        let body = serde_json::json!({
            "monitor": { "id": ev.monitor.id, "name": ev.monitor.name, "target": ev.monitor.target },
            "state": state,
            "at": ev.at,
            "error": ev.result.error,
            "latency_ms": ev.result.latency_ms,
        });
        // Up to 3 attempts; log and give up rather than blocking the scheduler. Delivery is
        // detached into its own task so `notify()` returns immediately and never stalls the
        // scheduler's select! loop on a slow or unreachable webhook.
        let client = self.client.clone();
        tokio::spawn(async move {
            for attempt in 1..=3u32 {
                match client.post(&url).json(&body).send().await {
                    Ok(r) if r.status().is_success() => return,
                    Ok(r) => eprintln!(
                        "uptime webhook {url}: HTTP {} (attempt {attempt})",
                        r.status()
                    ),
                    Err(e) => eprintln!("uptime webhook {url}: {e} (attempt {attempt})"),
                }
                if attempt < 3 {
                    tokio::time::sleep(Duration::from_millis(200 * attempt as u64)).await;
                }
            }
        });
        Ok(())
    }
}

/// Test double: records (monitor_id, transition) per call.
#[derive(Default)]
pub struct FakeNotifier {
    pub calls: std::sync::Mutex<Vec<(String, Transition)>>,
}

#[async_trait::async_trait]
impl Notifier for FakeNotifier {
    async fn notify(&self, ev: &NotifyEvent<'_>) -> Result<(), PhotonError> {
        self.calls
            .lock()
            .unwrap()
            .push((ev.monitor.id.clone(), ev.transition));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CheckResult, CheckType, Monitor, MonitorState, Transition};

    fn mon(webhook: Option<&str>) -> Monitor {
        Monitor {
            id: "m1".into(),
            name: "api".into(),
            check_type: CheckType::Http,
            target: "https://x.test".into(),
            interval_secs: 30,
            timeout_secs: 5,
            retries: 2,
            http_method: None,
            expect_status: None,
            keyword: None,
            ignore_tls: false,
            follow_redirects: true,
            webhook_url: webhook.map(String::from),
            enabled: true,
            last_state: MonitorState::Up,
            last_check_at: None,
            last_latency_ms: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[tokio::test]
    async fn fake_notifier_records_events() {
        let n = FakeNotifier::default();
        let m = mon(None);
        let res = CheckResult {
            ok: false,
            latency_ms: 0,
            status_code: None,
            error: Some("boom".into()),
        };
        let ev = NotifyEvent {
            monitor: &m,
            transition: Transition::WentDown,
            at: 123,
            result: &res,
        };
        n.notify(&ev).await.unwrap();
        let calls = n.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], ("m1".to_string(), Transition::WentDown));
    }

    #[test]
    fn resolves_url_per_monitor_then_global() {
        let n = WebhookNotifier::new(Some("https://global.test".into()));
        assert_eq!(
            n.resolve_url(&mon(Some("https://per.test"))).as_deref(),
            Some("https://per.test")
        );
        assert_eq!(
            n.resolve_url(&mon(None)).as_deref(),
            Some("https://global.test")
        );
        let n2 = WebhookNotifier::new(None);
        assert_eq!(n2.resolve_url(&mon(None)), None);
    }
}
