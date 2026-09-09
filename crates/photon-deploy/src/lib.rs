//! Docker fleet management — the pure domain (see `docs/subsystems/deploy.md`).
//!
//! `model` and `proto` are always compiled (photon-agent depends on this crate with default
//! features off for exactly those two); everything else — manifests, rendering, compose lint,
//! drift, preflight, rollout planning, the `DeployStore` trait — sits behind the `domain` feature
//! and is what photon-api and photon-server use. Nothing here does I/O.

pub mod model;
pub mod proto;
pub mod registry;

#[cfg(feature = "domain")]
pub mod compose;
#[cfg(feature = "domain")]
pub mod diff;
#[cfg(feature = "domain")]
pub mod ids;
#[cfg(feature = "domain")]
pub mod plan;
#[cfg(feature = "domain")]
pub mod portainer;
#[cfg(feature = "domain")]
pub mod preflight;
#[cfg(feature = "domain")]
pub mod render;
#[cfg(feature = "domain")]
pub mod store;

/// Milliseconds since the Unix epoch — the control-plane clock.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
