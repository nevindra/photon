//! photon-uptime: active uptime monitoring — schedules HTTP(S)/TCP/ICMP probes, records
//! up/down + latency into embedded SQLite, tracks incidents, fires webhook alerts.
//! See `docs/superpowers/specs/2026-07-04-uptime-monitoring-design.md`.

pub mod model;
pub mod notify;
pub mod probe;
pub mod scheduler;
pub mod state;
pub mod store;

pub use model::*;
pub use store::UptimeStore;
