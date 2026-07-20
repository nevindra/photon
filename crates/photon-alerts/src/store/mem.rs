//! In-memory `AlertStore` fake for tests — mirrors `SqliteAlertStore`'s semantics without disk
//! I/O. Backed by `Mutex<HashMap>`s + an incident-id counter (mirrors `photon-uptime`'s
//! `MemStore`).
use crate::model::*;
use crate::store::{gen_id, AlertStore};
use async_trait::async_trait;
use photon_core::PhotonError;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
struct Inner {
    channels: HashMap<ChannelId, Channel>,
    rules: HashMap<RuleId, Rule>,
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

#[async_trait]
impl AlertStore for MemStore {
    // ---- channels ----
    async fn list_channels(&self) -> Result<Vec<Channel>, PhotonError> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .channels
            .values()
            .cloned()
            .collect())
    }
    async fn get_channel(&self, id: &str) -> Result<Option<Channel>, PhotonError> {
        Ok(self.inner.lock().unwrap().channels.get(id).cloned())
    }
    async fn create_channel(&self, input: ChannelInput) -> Result<Channel, PhotonError> {
        let now = now_ms();
        let ch = Channel {
            id: gen_id("ch"),
            name: input.name,
            kind: input.config.kind(),
            config: input.config,
            created_at: now,
            updated_at: now,
        };
        self.inner
            .lock()
            .unwrap()
            .channels
            .insert(ch.id.clone(), ch.clone());
        Ok(ch)
    }
    async fn update_channel(
        &self,
        id: &str,
        input: ChannelInput,
    ) -> Result<Option<Channel>, PhotonError> {
        let mut g = self.inner.lock().unwrap();
        let Some(existing) = g.channels.get(id).cloned() else {
            return Ok(None);
        };
        let ch = Channel {
            id: id.to_string(),
            name: input.name,
            kind: input.config.kind(),
            config: input.config,
            created_at: existing.created_at,
            updated_at: now_ms(),
        };
        g.channels.insert(id.to_string(), ch.clone());
        Ok(Some(ch))
    }
    async fn delete_channel(&self, id: &str) -> Result<bool, PhotonError> {
        Ok(self.inner.lock().unwrap().channels.remove(id).is_some())
    }

    // ---- rules ----
    async fn list_rules(&self) -> Result<Vec<Rule>, PhotonError> {
        Ok(self.inner.lock().unwrap().rules.values().cloned().collect())
    }
    async fn get_rule(&self, id: &str) -> Result<Option<Rule>, PhotonError> {
        Ok(self.inner.lock().unwrap().rules.get(id).cloned())
    }
    async fn create_rule(&self, input: RuleInput) -> Result<Rule, PhotonError> {
        let now = now_ms();
        let r = Rule {
            id: gen_id("rule"),
            name: input.name,
            description: input.description,
            enabled: input.enabled,
            condition: input.condition,
            for_secs: input.for_secs,
            interval_secs: input.interval_secs,
            severity: input.severity,
            channel_ids: input.channel_ids,
            created_at: now,
            updated_at: now,
        };
        self.inner
            .lock()
            .unwrap()
            .rules
            .insert(r.id.clone(), r.clone());
        Ok(r)
    }
    async fn update_rule(&self, id: &str, input: RuleInput) -> Result<Option<Rule>, PhotonError> {
        let mut g = self.inner.lock().unwrap();
        let Some(existing) = g.rules.get(id).cloned() else {
            return Ok(None);
        };
        let r = Rule {
            id: id.to_string(),
            name: input.name,
            description: input.description,
            enabled: input.enabled,
            condition: input.condition,
            for_secs: input.for_secs,
            interval_secs: input.interval_secs,
            severity: input.severity,
            channel_ids: input.channel_ids,
            created_at: existing.created_at,
            updated_at: now_ms(),
        };
        g.rules.insert(id.to_string(), r.clone());
        Ok(Some(r))
    }
    async fn delete_rule(&self, id: &str) -> Result<bool, PhotonError> {
        Ok(self.inner.lock().unwrap().rules.remove(id).is_some())
    }
    async fn set_rule_enabled(&self, id: &str, enabled: bool) -> Result<Option<Rule>, PhotonError> {
        let mut g = self.inner.lock().unwrap();
        if let Some(r) = g.rules.get_mut(id) {
            r.enabled = enabled;
            r.updated_at = now_ms();
            Ok(Some(r.clone()))
        } else {
            Ok(None)
        }
    }

    // ---- incidents ----
    async fn open_incident(
        &self,
        rule_id: &str,
        series_key: &str,
        started_at: i64,
        value: f64,
        severity: Severity,
        summary: &str,
    ) -> Result<i64, PhotonError> {
        let mut g = self.inner.lock().unwrap();
        let id = g.next_incident;
        g.next_incident += 1;
        g.incidents.push(Incident {
            id,
            rule_id: rule_id.to_string(),
            series_key: series_key.to_string(),
            started_at,
            ended_at: None,
            peak_value: value,
            severity,
            summary: summary.to_string(),
        });
        Ok(id)
    }
    async fn bump_incident_peak(&self, incident_id: i64, value: f64) -> Result<(), PhotonError> {
        let mut g = self.inner.lock().unwrap();
        if let Some(i) = g.incidents.iter_mut().find(|i| i.id == incident_id) {
            if value > i.peak_value {
                i.peak_value = value;
            }
        }
        Ok(())
    }
    async fn close_incident(&self, incident_id: i64, ended_at: i64) -> Result<(), PhotonError> {
        let mut g = self.inner.lock().unwrap();
        if let Some(i) = g.incidents.iter_mut().find(|i| i.id == incident_id) {
            i.ended_at = Some(ended_at);
        }
        Ok(())
    }
    async fn open_incident_for(
        &self,
        rule_id: &str,
        series_key: &str,
    ) -> Result<Option<i64>, PhotonError> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .incidents
            .iter()
            .filter(|i| i.rule_id == rule_id && i.series_key == series_key && i.ended_at.is_none())
            .max_by_key(|i| i.started_at)
            .map(|i| i.id))
    }
    async fn list_open_incidents(&self) -> Result<Vec<Incident>, PhotonError> {
        let g = self.inner.lock().unwrap();
        let mut v: Vec<_> = g
            .incidents
            .iter()
            .filter(|i| i.ended_at.is_none())
            .cloned()
            .collect();
        v.sort_by_key(|i| std::cmp::Reverse(i.started_at));
        Ok(v)
    }
    async fn list_incidents(
        &self,
        status: Option<&str>,
        rule_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Incident>, PhotonError> {
        let g = self.inner.lock().unwrap();
        let mut v: Vec<_> = g
            .incidents
            .iter()
            .filter(|i| match status {
                Some("triggered") => i.ended_at.is_none(),
                Some("resolved") => i.ended_at.is_some(),
                _ => true,
            })
            .filter(|i| rule_id.is_none_or(|rid| i.rule_id == rid))
            .cloned()
            .collect();
        v.sort_by_key(|i| std::cmp::Reverse(i.started_at));
        v.truncate(limit as usize);
        Ok(v)
    }
    async fn prune_incidents(&self, before: i64) -> Result<u64, PhotonError> {
        let mut g = self.inner.lock().unwrap();
        let n = g.incidents.len();
        g.incidents
            .retain(|i| i.ended_at.is_none_or(|ended| ended >= before));
        Ok((n - g.incidents.len()) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cond() -> Condition {
        Condition::Metrics(MetricCondition {
            metric_name: "m".into(),
            label_filters: Default::default(),
            group_by: vec![],
            agg: MetricAgg::Avg,
            window_secs: 60,
            cmp: Cmp::Gt,
            threshold: 1.0,
        })
    }
    fn rule_input() -> RuleInput {
        RuleInput {
            name: "r".into(),
            description: None,
            enabled: true,
            condition: cond(),
            for_secs: 0,
            interval_secs: 60,
            severity: Severity::Warning,
            channel_ids: vec![],
        }
    }

    #[tokio::test]
    async fn create_rule_then_list_rules_returns_it() {
        let s = MemStore::new();
        let r = s.create_rule(rule_input()).await.unwrap();
        let rules = s.list_rules().await.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, r.id);
        assert_eq!(rules[0].name, "r");
    }

    #[tokio::test]
    async fn channel_crud_roundtrips() {
        let s = MemStore::new();
        let input = ChannelInput {
            name: "ops".into(),
            config: ChannelConfig::Webhook {
                url: "http://x".into(),
                secret: None,
                headers: None,
            },
        };
        let ch = s.create_channel(input).await.unwrap();
        assert_eq!(s.list_channels().await.unwrap().len(), 1);
        assert!(s.get_channel(&ch.id).await.unwrap().is_some());
        assert!(s.delete_channel(&ch.id).await.unwrap());
        assert!(s.list_channels().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn incident_open_close_and_rebuild() {
        let s = MemStore::new();
        let id = s
            .open_incident("r1", "host=web-01", 100, 0.94, Severity::Warning, "avg>0.9")
            .await
            .unwrap();
        assert_eq!(
            s.open_incident_for("r1", "host=web-01").await.unwrap(),
            Some(id)
        );
        assert_eq!(s.list_open_incidents().await.unwrap().len(), 1);
        s.bump_incident_peak(id, 0.99).await.unwrap();
        s.close_incident(id, 200).await.unwrap();
        assert_eq!(
            s.open_incident_for("r1", "host=web-01").await.unwrap(),
            None
        );
        assert_eq!(
            s.list_incidents(Some("resolved"), None, 10)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            s.list_incidents(Some("triggered"), None, 10)
                .await
                .unwrap()
                .len(),
            0
        );
    }
}
