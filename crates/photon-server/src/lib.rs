//! Library surface of the `photon-server` crate.
//!
//! `photon-server` is primarily the server binary (`src/main.rs`), but a few internal seams need to
//! be exercised by the crate's integration tests (`tests/`), which can only link against a library
//! target — not the binary. `alerts_source::EngineConditionSource` is the alerts `ConditionSource`
//! implemented over the three query engines (it is the *only* place `photon-alerts` and
//! `photon-query` meet); `tests/alerts_e2e.rs` drives its real value extraction over a populated
//! engine, so it is re-exported here. The binary keeps its own `mod alerts_source;`.
pub mod alerts_source;
