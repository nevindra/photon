//! photon-index: skip index (token bloom + min/max), pure and sync.
//!
//! Its place in the crate graph is described in `docs/architecture.md`.

mod bloom;
mod skip_index;
mod tokenize;

pub use skip_index::SkipIndex;
pub use tokenize::{interior_tokens, tokenize};
