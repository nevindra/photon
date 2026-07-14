//! The `UptimeStore` boundary + an in-memory fake for tests. SQLite impl lives in `sqlite.rs`.

pub mod sqlite;

use crate::model::*;
use photon_core::PhotonError;
use std::collections::HashMap;
use std::sync::Mutex;

#[async_trait::async_trait]
pub trait UptimeStore: Send + Sync + 'static {
    async fn list_monitors(&self) -> Result<Vec<Monitor>, PhotonError>;
    async fn get_monitor(&self, id: &str) -> Result<Option<Monitor>, PhotonError>;
    async fn create_monitor(&self, input: MonitorInput) -> Result<Monitor, PhotonError>;
    async fn update_monitor(
        &self,
        id: &str,
        input: MonitorInput,
    ) -> Result<Option<Monitor>, PhotonError>;
    async fn delete_monitor(&self, id: &str) -> Result<bool, PhotonError>;
    async fn set_enabled(&self, id: &str, enabled: bool) -> Result<Option<Monitor>, PhotonError>;
    async fn append_heartbeat(&self, hb: Heartbeat) -> Result<(), PhotonError>;
    async fn set_monitor_state(
        &self,
        id: &str,
        state: MonitorState,
        at: i64,
        latency_ms: u32,
    ) -> Result<(), PhotonError>;
    async fn heartbeats(&self, id: &str, since: i64) -> Result<Vec<Heartbeat>, PhotonError>;
    async fn uptime_pct(&self, id: &str, since: i64) -> Result<f64, PhotonError>;
    async fn open_incident(
        &self,
        id: &str,
        started_at: i64,
        cause: String,
    ) -> Result<i64, PhotonError>;
    async fn open_incident_id(&self, id: &str) -> Result<Option<i64>, PhotonError>;
    async fn close_incident(&self, incident_id: i64, ended_at: i64) -> Result<(), PhotonError>;
    async fn incidents(&self, id: &str, limit: u32) -> Result<Vec<Incident>, PhotonError>;
    async fn prune_heartbeats(&self, before: i64) -> Result<u64, PhotonError>;
    /// Delete closed incidents (`ended_at` set and `< before`). Open incidents are kept.
    async fn prune_incidents(&self, before: i64) -> Result<u64, PhotonError>;
    /// Manifest-free storage summary (counts + heartbeat time span).
    async fn stats(&self) -> Result<UptimeStats, PhotonError>;
}

/// Manifest-free storage summary for the uptime signal (SQLite counts).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct UptimeStats {
    pub monitor_count: u64,
    pub heartbeat_count: u64,
    pub incident_count: u64,
    pub oldest_heartbeat_ts: Option<i64>,
    pub newest_heartbeat_ts: Option<i64>,
}

/// Build a `Monitor` from an input payload, assigning id/timestamps/initial state.
pub fn monitor_from_input(id: MonitorId, input: MonitorInput, now: i64) -> Monitor {
    Monitor {
        id,
        name: input.name,
        check_type: input.check_type,
        target: input.target,
        interval_secs: input.interval_secs,
        timeout_secs: input.timeout_secs,
        retries: input.retries,
        http_method: input.http_method,
        expect_status: input.expect_status,
        keyword: input.keyword,
        ignore_tls: input.ignore_tls,
        follow_redirects: input.follow_redirects,
        webhook_url: input.webhook_url,
        enabled: input.enabled,
        last_state: MonitorState::Pending,
        last_check_at: None,
        last_latency_ms: None,
        created_at: now,
        updated_at: now,
    }
}

#[derive(Default)]
struct Inner {
    monitors: HashMap<MonitorId, Monitor>,
    heartbeats: Vec<Heartbeat>,
    incidents: Vec<Incident>,
    next_incident: i64,
}

#[derive(Default)]
pub struct MemStore {
    inner: Mutex<Inner>,
}

impl MemStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl UptimeStore for MemStore {
    async fn list_monitors(&self) -> Result<Vec<Monitor>, PhotonError> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .monitors
            .values()
            .cloned()
            .collect())
    }
    async fn get_monitor(&self, id: &str) -> Result<Option<Monitor>, PhotonError> {
        Ok(self.inner.lock().unwrap().monitors.get(id).cloned())
    }
    async fn create_monitor(&self, input: MonitorInput) -> Result<Monitor, PhotonError> {
        let m = monitor_from_input(uuid::Uuid::new_v4().to_string(), input, now_ms());
        self.inner
            .lock()
            .unwrap()
            .monitors
            .insert(m.id.clone(), m.clone());
        Ok(m)
    }
    async fn update_monitor(
        &self,
        id: &str,
        input: MonitorInput,
    ) -> Result<Option<Monitor>, PhotonError> {
        let mut g = self.inner.lock().unwrap();
        let Some(existing) = g.monitors.get(id).cloned() else {
            return Ok(None);
        };
        let mut m = monitor_from_input(id.to_string(), input, now_ms());
        m.created_at = existing.created_at;
        m.last_state = existing.last_state;
        m.last_check_at = existing.last_check_at;
        m.last_latency_ms = existing.last_latency_ms;
        m.enabled = existing.enabled;
        g.monitors.insert(id.to_string(), m.clone());
        Ok(Some(m))
    }
    async fn delete_monitor(&self, id: &str) -> Result<bool, PhotonError> {
        Ok(self.inner.lock().unwrap().monitors.remove(id).is_some())
    }
    async fn set_enabled(&self, id: &str, enabled: bool) -> Result<Option<Monitor>, PhotonError> {
        let mut g = self.inner.lock().unwrap();
        if let Some(m) = g.monitors.get_mut(id) {
            m.enabled = enabled;
            m.updated_at = now_ms();
            Ok(Some(m.clone()))
        } else {
            Ok(None)
        }
    }
    async fn append_heartbeat(&self, hb: Heartbeat) -> Result<(), PhotonError> {
        self.inner.lock().unwrap().heartbeats.push(hb);
        Ok(())
    }
    async fn set_monitor_state(
        &self,
        id: &str,
        state: MonitorState,
        at: i64,
        latency_ms: u32,
    ) -> Result<(), PhotonError> {
        let mut g = self.inner.lock().unwrap();
        if let Some(m) = g.monitors.get_mut(id) {
            m.last_state = state;
            m.last_check_at = Some(at);
            m.last_latency_ms = Some(latency_ms);
        }
        Ok(())
    }
    async fn heartbeats(&self, id: &str, since: i64) -> Result<Vec<Heartbeat>, PhotonError> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .heartbeats
            .iter()
            .filter(|h| h.monitor_id == id && h.ts >= since)
            .cloned()
            .collect())
    }
    async fn uptime_pct(&self, id: &str, since: i64) -> Result<f64, PhotonError> {
        let g = self.inner.lock().unwrap();
        let hs: Vec<_> = g
            .heartbeats
            .iter()
            .filter(|h| h.monitor_id == id && h.ts >= since)
            .collect();
        if hs.is_empty() {
            return Ok(100.0);
        }
        let up = hs.iter().filter(|h| h.ok).count();
        Ok(up as f64 * 100.0 / hs.len() as f64)
    }
    async fn open_incident(
        &self,
        id: &str,
        started_at: i64,
        cause: String,
    ) -> Result<i64, PhotonError> {
        let mut g = self.inner.lock().unwrap();
        let iid = g.next_incident;
        g.next_incident += 1;
        g.incidents.push(Incident {
            id: iid,
            monitor_id: id.to_string(),
            started_at,
            ended_at: None,
            cause,
        });
        Ok(iid)
    }
    async fn open_incident_id(&self, id: &str) -> Result<Option<i64>, PhotonError> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .incidents
            .iter()
            .find(|i| i.monitor_id == id && i.ended_at.is_none())
            .map(|i| i.id))
    }
    async fn close_incident(&self, incident_id: i64, ended_at: i64) -> Result<(), PhotonError> {
        let mut g = self.inner.lock().unwrap();
        if let Some(i) = g.incidents.iter_mut().find(|i| i.id == incident_id) {
            i.ended_at = Some(ended_at);
        }
        Ok(())
    }
    async fn incidents(&self, id: &str, limit: u32) -> Result<Vec<Incident>, PhotonError> {
        let g = self.inner.lock().unwrap();
        let mut v: Vec<_> = g
            .incidents
            .iter()
            .filter(|i| i.monitor_id == id)
            .cloned()
            .collect();
        v.sort_by_key(|b| std::cmp::Reverse(b.started_at));
        v.truncate(limit as usize);
        Ok(v)
    }
    async fn prune_heartbeats(&self, before: i64) -> Result<u64, PhotonError> {
        let mut g = self.inner.lock().unwrap();
        let n = g.heartbeats.len();
        g.heartbeats.retain(|h| h.ts >= before);
        Ok((n - g.heartbeats.len()) as u64)
    }
    async fn prune_incidents(&self, before: i64) -> Result<u64, PhotonError> {
        let mut g = self.inner.lock().unwrap();
        let n = g.incidents.len();
        g.incidents
            .retain(|i| i.ended_at.is_none_or(|ended| ended >= before));
        Ok((n - g.incidents.len()) as u64)
    }
    async fn stats(&self) -> Result<UptimeStats, PhotonError> {
        let g = self.inner.lock().unwrap();
        let oldest = g.heartbeats.iter().map(|h| h.ts).min();
        let newest = g.heartbeats.iter().map(|h| h.ts).max();
        Ok(UptimeStats {
            monitor_count: g.monitors.len() as u64,
            heartbeat_count: g.heartbeats.len() as u64,
            incident_count: g.incidents.len() as u64,
            oldest_heartbeat_ts: oldest,
            newest_heartbeat_ts: newest,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CheckType;

    fn input() -> MonitorInput {
        serde_json::from_str(
            r#"{"name":"api","type":"http","target":"https://x.test",
            "interval_secs":30,"timeout_secs":5,"retries":2}"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn crud_roundtrip() {
        let s = MemStore::new();
        let m = s.create_monitor(input()).await.unwrap();
        assert_eq!(m.check_type, CheckType::Http);
        assert_eq!(m.last_state, MonitorState::Pending);
        assert_eq!(s.list_monitors().await.unwrap().len(), 1);

        let mut edit = input();
        edit.name = "renamed".into();
        let up = s.update_monitor(&m.id, edit).await.unwrap().unwrap();
        assert_eq!(up.name, "renamed");

        assert!(!s.set_enabled(&m.id, false).await.unwrap().unwrap().enabled);
        assert!(s.delete_monitor(&m.id).await.unwrap());
        assert!(s.list_monitors().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn update_monitor_preserves_enabled_when_paused() {
        let s = MemStore::new();
        let m = s.create_monitor(input()).await.unwrap();
        assert!(!s.set_enabled(&m.id, false).await.unwrap().unwrap().enabled);

        // The edit payload omits `enabled`, so it deserializes to the `default_true` default —
        // mirroring the frontend's PATCH, which never sends `enabled`. update_monitor must not
        // let that default silently re-enable a paused monitor.
        let mut edit = input();
        edit.name = "renamed".into();
        let up = s.update_monitor(&m.id, edit).await.unwrap().unwrap();
        assert_eq!(up.name, "renamed");
        assert!(!up.enabled, "editing a paused monitor must keep it paused");
    }

    #[tokio::test]
    async fn heartbeats_incidents_and_uptime() {
        let s = MemStore::new();
        let m = s.create_monitor(input()).await.unwrap();
        s.append_heartbeat(Heartbeat {
            monitor_id: m.id.clone(),
            ts: 1000,
            ok: true,
            latency_ms: 5,
            status_code: Some(200),
            error: None,
        })
        .await
        .unwrap();
        s.append_heartbeat(Heartbeat {
            monitor_id: m.id.clone(),
            ts: 2000,
            ok: false,
            latency_ms: 0,
            status_code: None,
            error: Some("boom".into()),
        })
        .await
        .unwrap();
        assert_eq!(s.heartbeats(&m.id, 0).await.unwrap().len(), 2);
        assert!((s.uptime_pct(&m.id, 0).await.unwrap() - 50.0).abs() < 1e-6);

        let iid = s.open_incident(&m.id, 2000, "boom".into()).await.unwrap();
        assert_eq!(s.open_incident_id(&m.id).await.unwrap(), Some(iid));
        s.close_incident(iid, 3000).await.unwrap();
        assert_eq!(s.open_incident_id(&m.id).await.unwrap(), None);
        assert_eq!(
            s.incidents(&m.id, 10).await.unwrap()[0].ended_at,
            Some(3000)
        );

        assert_eq!(s.prune_heartbeats(1500).await.unwrap(), 1); // drops ts=1000
    }
}
