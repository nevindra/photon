//! Domain types for data retention / manual purge. `PurgeReport` is the outcome of a
//! compactor `purge_before` and travels back to the API over the purge command channel.

use serde::{Deserialize, Serialize};

/// How much a purge removed. `Default` = nothing removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurgeReport {
    pub files_removed: u64,
    pub rows_removed: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purge_report_json_round_trips() {
        let r = PurgeReport {
            files_removed: 3,
            rows_removed: 42,
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: PurgeReport = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
        assert_eq!(
            PurgeReport::default(),
            PurgeReport {
                files_removed: 0,
                rows_removed: 0
            }
        );
    }
}
