//! photon-index: skip index (token bloom + min/max), pure and sync.
//!
//! Implemented per the `photon-index` section of
//! `docs/superpowers/plans/2026-07-01-photon-interface-contracts.md`.

mod bloom;
mod skip_index;
mod tokenize;

pub use skip_index::SkipIndex;
pub use tokenize::tokenize;
