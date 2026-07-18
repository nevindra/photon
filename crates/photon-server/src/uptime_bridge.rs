//! Bridges uptime monitor up/down transitions onto the shared alert channels + incident history.
//!
//! Uptime runs its own Up/Down state machine and its own per-monitor / global webhook delivery
//! (`photon_uptime::notify::WebhookNotifier`). This adapter *wraps* that legacy notifier — so a
//! monitor's own `webhook_url` (and the `[uptime].webhook_url` global) keeps firing — and, in
//! addition, records the transition in the shared alerts incident store and delivers the alert
//! payload to every channel listed in `monitor.channel_ids`.
//!
//! Mapping: an uptime monitor going **Down** opens a `triggered` alert incident (synthetic rule id
//! `uptime:<monitor.id>`, empty series key); **Recovered** closes it (`resolved`). The monitor's
//! own vocabulary stays Up/Down; only the alert *incident* it drives uses the alerts payload
//! `status` ("triggered"/"resolved").

use std::sync::Arc;

use async_trait::async_trait;
use photon_alerts::model::Severity;
use photon_alerts::notify::{Notifier as AlertNotifier, NotifyStatus};
use photon_alerts::store::AlertStore;
use photon_core::PhotonError;
use photon_uptime::model::{Monitor, Transition};
use photon_uptime::notify::{Notifier, NotifyEvent, WebhookNotifier};

/// A down monitor is a genuine outage — its incident is opened at `Critical` severity.
const UPTIME_SEVERITY: Severity = Severity::Critical;

/// The `photon_uptime::notify::Notifier` the scheduler drives. It fans each transition out to (1)
/// the legacy uptime webhook path and (2) the shared alerts store + channels.
pub struct UptimeAlertBridge {
    /// Preserves the legacy per-monitor (and `[uptime].webhook_url` global) delivery path.
    webhook: WebhookNotifier,
    /// The shared alerts incident store — `uptime:<id>` incidents live here alongside real rules'
    /// incidents (the `alert_incidents` table has no FK to `alert_rules`, so a synthetic rule id
    /// is fine).
    store: Arc<dyn AlertStore>,
    /// The shared alerts webhook delivery (per-channel, optional HMAC-signed, detached + retried).
    alerts_notifier: Arc<dyn AlertNotifier>,
}

impl UptimeAlertBridge {
    pub fn new(
        webhook: WebhookNotifier,
        store: Arc<dyn AlertStore>,
        alerts_notifier: Arc<dyn AlertNotifier>,
    ) -> Self {
        Self {
            webhook,
            store,
            alerts_notifier,
        }
    }

    /// The synthetic alerts rule id for a monitor's incidents: `uptime:<monitor.id>`.
    fn rule_id(monitor: &Monitor) -> String {
        format!("uptime:{}", monitor.id)
    }
}

/// Build the alert webhook payload for one uptime transition. Mirrors the shape of
/// `photon_alerts::notify::build_payload` (status / rule / series / condition / value / …) so an
/// uptime alert and a metric/log/trace alert look the same to a downstream webhook receiver.
fn build_payload(
    monitor: &Monitor,
    status: NotifyStatus,
    at: i64,
    incident_id: i64,
    error: Option<&str>,
) -> serde_json::Value {
    let status_str = match status {
        NotifyStatus::Triggered => "triggered",
        NotifyStatus::Resolved => "resolved",
    };
    serde_json::json!({
        "status": status_str,
        "rule": {
            "id": UptimeAlertBridge::rule_id(monitor),
            "name": monitor.name,
            "severity": UPTIME_SEVERITY,
            "signal": "uptime",
        },
        "series": {
            "monitor.id": monitor.id,
            "monitor.name": monitor.name,
            "target": monitor.target,
        },
        "condition": format!("{} down", monitor.name),
        "value": if matches!(status, NotifyStatus::Triggered) { 1.0 } else { 0.0 },
        "threshold": 1.0,
        "at": at,
        "incident_id": incident_id,
        "error": error,
    })
}

#[async_trait]
impl Notifier for UptimeAlertBridge {
    async fn notify(&self, ev: &NotifyEvent<'_>) -> Result<(), PhotonError> {
        // 1) Legacy path: the monitor's own `webhook_url` / the `[uptime].webhook_url` global.
        //    Non-fatal (delivery is detached inside), and never propagated.
        let _ = self.webhook.notify(ev).await;

        // 2) Bridge onto the shared alerts store + channels.
        let monitor = ev.monitor;
        let rule_id = Self::rule_id(monitor);
        let at = ev.at;

        let (status, incident_id) = match ev.transition {
            Transition::WentDown => {
                let summary = format!("{} down", monitor.name);
                let iid = self
                    .store
                    .open_incident(&rule_id, "", at, 1.0, UPTIME_SEVERITY, &summary)
                    .await?;
                (NotifyStatus::Triggered, iid)
            }
            Transition::Recovered => {
                // Close the currently-open incident for this monitor, if any.
                let iid = match self.store.open_incident_for(&rule_id, "").await? {
                    Some(iid) => {
                        self.store.close_incident(iid, at).await?;
                        iid
                    }
                    None => 0,
                };
                (NotifyStatus::Resolved, iid)
            }
        };

        if monitor.channel_ids.is_empty() {
            return Ok(());
        }
        let payload = build_payload(monitor, status, at, incident_id, ev.result.error.as_deref());
        for cid in &monitor.channel_ids {
            match self.store.get_channel(cid).await {
                Ok(Some(ch)) => self.alerts_notifier.deliver(&ch, payload.clone()).await,
                Ok(None) => {} // channel deleted since the monitor referenced it — skip.
                Err(e) => eprintln!("uptime->alerts: channel {cid} lookup failed: {e}"),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use photon_alerts::notify::FakeNotifier;
    use photon_alerts::store::mem::MemStore;
    use photon_uptime::model::{CheckResult, CheckType, MonitorState};

    fn monitor(channel_ids: Vec<String>) -> Monitor {
        Monitor {
            id: "m1".into(),
            name: "api".into(),
            check_type: CheckType::Http,
            target: "https://x.test".into(),
            interval_secs: 30,
            timeout_secs: 5,
            retries: 1,
            http_method: None,
            expect_status: None,
            keyword: None,
            ignore_tls: false,
            follow_redirects: true,
            webhook_url: None,
            channel_ids,
            enabled: true,
            last_state: MonitorState::Up,
            last_check_at: None,
            last_latency_ms: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn down_result() -> CheckResult {
        CheckResult {
            ok: false,
            latency_ms: 0,
            status_code: None,
            error: Some("boom".into()),
        }
    }

    #[test]
    fn payload_mirrors_alerts_shape() {
        let m = monitor(vec![]);
        let p = build_payload(&m, NotifyStatus::Triggered, 2000, 7, Some("boom"));
        assert_eq!(p["status"], "triggered");
        assert_eq!(p["rule"]["id"], "uptime:m1");
        assert_eq!(p["rule"]["signal"], "uptime");
        assert_eq!(p["rule"]["severity"], "critical");
        assert_eq!(p["series"]["target"], "https://x.test");
        assert_eq!(p["incident_id"], 7);
        assert_eq!(p["error"], "boom");
    }

    #[tokio::test]
    async fn down_opens_incident_and_up_closes_it() {
        let store: Arc<dyn AlertStore> = Arc::new(MemStore::new());
        let notifier: Arc<dyn AlertNotifier> = Arc::new(FakeNotifier::default());
        let bridge =
            UptimeAlertBridge::new(WebhookNotifier::new(None), store.clone(), notifier.clone());
        let m = monitor(vec![]);

        // WentDown → an incident is opened for uptime:m1.
        let res = down_result();
        bridge
            .notify(&NotifyEvent {
                monitor: &m,
                transition: Transition::WentDown,
                at: 1000,
                result: &res,
            })
            .await
            .unwrap();
        assert!(store
            .open_incident_for("uptime:m1", "")
            .await
            .unwrap()
            .is_some());

        // Recovered → the open incident is closed.
        let ok = CheckResult {
            ok: true,
            latency_ms: 7,
            status_code: Some(200),
            error: None,
        };
        bridge
            .notify(&NotifyEvent {
                monitor: &m,
                transition: Transition::Recovered,
                at: 2000,
                result: &ok,
            })
            .await
            .unwrap();
        assert_eq!(
            store.open_incident_for("uptime:m1", "").await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn delivers_payload_to_each_channel() {
        use photon_alerts::model::ChannelInput;
        let store: Arc<dyn AlertStore> = Arc::new(MemStore::new());
        let fake = Arc::new(FakeNotifier::default());
        let notifier: Arc<dyn AlertNotifier> = fake.clone();
        let ch = store
            .create_channel(
                serde_json::from_value::<ChannelInput>(serde_json::json!({
                    "name": "ops", "type": "webhook", "url": "http://x"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let bridge = UptimeAlertBridge::new(WebhookNotifier::new(None), store.clone(), notifier);
        let m = monitor(vec![ch.id.clone()]);
        let res = down_result();
        bridge
            .notify(&NotifyEvent {
                monitor: &m,
                transition: Transition::WentDown,
                at: 1000,
                result: &res,
            })
            .await
            .unwrap();
        let calls = fake.calls.lock().unwrap();
        assert_eq!(calls.as_slice(), &[(ch.id, "triggered".to_string())]);
    }
}
