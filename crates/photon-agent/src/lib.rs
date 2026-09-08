//! photon-agent as a library, so integration tests (`tests/`) and the `main.rs` binary share one
//! module tree. Nothing here is a public API for other crates; `photon-server` never depends on
//! it. The binary is `main.rs`.
//!
//! Two independent jobs live here:
//! - the resource sampler (`sample`/`sysinfo_sampler`/`gpu`/`otlp`/`send`): host + GPU metrics
//!   posted as OTLP on a fixed interval, unchanged since 0.1;
//! - the Docker fleet-management control channel (`sync` + `docker/` + `state`/`upgrade`):
//!   one long-polled `POST /api/agent/sync` carrying what the host looks like and what the agent
//!   did, answered with what it should be. Outbound only — no listening socket (R1.2).

pub mod clock;
pub mod config;
pub mod docker;
pub mod gpu;
pub mod install;
pub mod otlp;
pub mod sample;
pub mod send;
pub mod state;
pub mod sync;
pub mod sysinfo_sampler;
pub mod upgrade;
