//! The alerts persistence seam. Real impl: `SqliteAlertStore` (`sqlite.rs`); test fake: `MemStore`.
use crate::model::{Channel, ChannelInput, Incident, Rule, RuleInput, Severity};
use async_trait::async_trait;
use photon_core::PhotonError;

pub mod mem;
pub mod sqlite;

/// Generate a unique id as `<prefix>-<now_ms>-<counter>`. A counter+timestamp scheme (mirroring
/// the id shape sketched for `photon-uptime`'s monitor ids) rather than a UUID, so this crate
/// doesn't need to add the `uuid` dependency just for id generation. The counter guarantees
/// uniqueness even when two ids are minted within the same millisecond.
pub(crate) fn gen_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{n}", crate::model::now_ms())
}

#[async_trait]
pub trait AlertStore: Send + Sync + 'static {
    // channels
    async fn list_channels(&self) -> Result<Vec<Channel>, PhotonError>;
    async fn get_channel(&self, id: &str) -> Result<Option<Channel>, PhotonError>;
    async fn create_channel(&self, input: ChannelInput) -> Result<Channel, PhotonError>;
    async fn update_channel(
        &self,
        id: &str,
        input: ChannelInput,
    ) -> Result<Option<Channel>, PhotonError>;
    async fn delete_channel(&self, id: &str) -> Result<bool, PhotonError>;
    // rules
    async fn list_rules(&self) -> Result<Vec<Rule>, PhotonError>;
    async fn get_rule(&self, id: &str) -> Result<Option<Rule>, PhotonError>;
    async fn create_rule(&self, input: RuleInput) -> Result<Rule, PhotonError>;
    async fn update_rule(&self, id: &str, input: RuleInput) -> Result<Option<Rule>, PhotonError>;
    async fn delete_rule(&self, id: &str) -> Result<bool, PhotonError>;
    async fn set_rule_enabled(&self, id: &str, enabled: bool) -> Result<Option<Rule>, PhotonError>;
    // incidents
    async fn open_incident(
        &self,
        rule_id: &str,
        series_key: &str,
        started_at: i64,
        value: f64,
        severity: Severity,
        summary: &str,
    ) -> Result<i64, PhotonError>;
    async fn bump_incident_peak(&self, incident_id: i64, value: f64) -> Result<(), PhotonError>;
    async fn close_incident(&self, incident_id: i64, ended_at: i64) -> Result<(), PhotonError>;
    /// The open incident id for a (rule, series), if any.
    async fn open_incident_for(
        &self,
        rule_id: &str,
        series_key: &str,
    ) -> Result<Option<i64>, PhotonError>;
    /// All currently-open incidents — used to rebuild `Triggered` state on startup.
    async fn list_open_incidents(&self) -> Result<Vec<Incident>, PhotonError>;
    /// `status`: `Some("triggered")` (ended_at IS NULL), `Some("resolved")`, or `None` (all).
    async fn list_incidents(
        &self,
        status: Option<&str>,
        rule_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Incident>, PhotonError>;
    async fn prune_incidents(&self, before: i64) -> Result<u64, PhotonError>;
}
