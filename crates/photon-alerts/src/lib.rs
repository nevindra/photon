//! System-wide alert engine: per-signal rules → webhook channels. Pure domain, a state machine,
//! a SQLite store, and delivery; the evaluation loop is generic over the `ConditionSource` seam
//! (implemented in `photon-server` over the query engines).
pub mod model;
pub mod notify;
pub mod scheduler;
pub mod source;
pub mod state;
pub mod store;
