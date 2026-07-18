//! The seam between the alert engine and the signal data. Implemented in `photon-server` over
//! the three query engines + the uptime store; faked in tests. Keeps `photon-alerts` free of
//! any dependency on `photon-query`.
use crate::model::{Condition, SeriesSample};
use async_trait::async_trait;
use photon_core::PhotonError;

#[async_trait]
pub trait ConditionSource: Send + Sync + 'static {
    /// Sample `cond` as of `now_ms`, returning one value per evaluated series (empty `group_by`
    /// → a single series with an empty key). `Ok(vec![])` = "nothing matched/crossed" (a valid
    /// result that drives resolves); `Err` = "could not evaluate this tick" (state left unchanged).
    async fn sample(&self, cond: &Condition, now_ms: i64)
        -> Result<Vec<SeriesSample>, PhotonError>;
}
