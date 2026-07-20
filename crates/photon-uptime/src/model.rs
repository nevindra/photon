//! Pure domain types for the uptime vertical. No I/O.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Unix milliseconds. Single source of "now" for the whole vertical.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

pub type MonitorId = String;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckType {
    Http,
    Tcp,
    Icmp,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MonitorState {
    Pending,
    Up,
    Down,
}

fn default_true() -> bool {
    true
}

/// The persisted, UI-visible monitor (includes denormalized current state).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Monitor {
    pub id: MonitorId,
    pub name: String,
    #[serde(rename = "type")]
    pub check_type: CheckType,
    pub target: String,
    pub interval_secs: u32,
    pub timeout_secs: u32,
    pub retries: u32,
    pub http_method: Option<String>,
    pub expect_status: Option<String>,
    pub keyword: Option<String>,
    pub ignore_tls: bool,
    pub follow_redirects: bool,
    pub webhook_url: Option<String>,
    /// Alert channel ids (from the shared alerts store) to notify on up/down transitions, in
    /// addition to the legacy `webhook_url`. Persisted as a JSON array; absent ⇒ empty.
    #[serde(default)]
    pub channel_ids: Vec<String>,
    pub enabled: bool,
    pub last_state: MonitorState,
    pub last_check_at: Option<i64>,
    pub last_latency_ms: Option<u32>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Create/update payload from the API (server assigns id/timestamps/state).
#[derive(Clone, Debug, Deserialize)]
pub struct MonitorInput {
    pub name: String,
    #[serde(rename = "type")]
    pub check_type: CheckType,
    pub target: String,
    pub interval_secs: u32,
    pub timeout_secs: u32,
    pub retries: u32,
    #[serde(default)]
    pub http_method: Option<String>,
    #[serde(default)]
    pub expect_status: Option<String>,
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub ignore_tls: bool,
    #[serde(default = "default_true")]
    pub follow_redirects: bool,
    #[serde(default)]
    pub webhook_url: Option<String>,
    #[serde(default)]
    pub channel_ids: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// The raw outcome of one probe. A failed probe is data, not an error.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckResult {
    pub ok: bool,
    pub latency_ms: u32,
    pub status_code: Option<u16>,
    pub error: Option<String>,
}

/// One persisted check result row.
#[derive(Clone, Debug, Serialize)]
pub struct Heartbeat {
    pub monitor_id: MonitorId,
    pub ts: i64,
    pub ok: bool,
    pub latency_ms: u32,
    pub status_code: Option<u16>,
    pub error: Option<String>,
}

/// One DOWN period. `ended_at == None` ⇒ ongoing.
#[derive(Clone, Debug, Serialize)]
pub struct Incident {
    pub id: i64,
    pub monitor_id: MonitorId,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub cause: String,
}

/// Scheduler-held runtime state per monitor (not persisted directly; rebuilt on startup).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RuntimeState {
    pub state: MonitorState,
    pub consecutive_failures: u32,
}

impl RuntimeState {
    pub fn pending() -> Self {
        Self {
            state: MonitorState::Pending,
            consecutive_failures: 0,
        }
    }
    pub fn from_state(state: MonitorState) -> Self {
        Self {
            state,
            consecutive_failures: 0,
        }
    }
}

/// A change worth acting on (open/close an incident + notify).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Transition {
    WentDown,
    Recovered,
}

/// Live-reload messages from the API to the scheduler.
#[derive(Clone, Debug)]
pub enum SchedulerCommand {
    /// Monitor created or edited (enabled flag included) — (re)schedule it.
    Upsert(Box<Monitor>),
    /// Monitor deleted — drop it from the schedule.
    Remove(MonitorId),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checktype_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&CheckType::Http).unwrap(), "\"http\"");
        assert_eq!(serde_json::to_string(&CheckType::Icmp).unwrap(), "\"icmp\"");
    }

    #[test]
    fn monitor_input_defaults() {
        let j = r#"{"name":"api","type":"http","target":"https://x.test",
                    "interval_secs":60,"timeout_secs":10,"retries":3}"#;
        let m: MonitorInput = serde_json::from_str(j).unwrap();
        assert_eq!(m.check_type, CheckType::Http);
        assert!(m.enabled); // default true
        assert!(m.follow_redirects); // default true
        assert!(!m.ignore_tls); // default false
    }

    #[test]
    fn now_ms_is_positive() {
        assert!(now_ms() > 0);
    }
}
