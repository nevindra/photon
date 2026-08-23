//! The rollup's published-through marker: the START of the newest bucket already written.
//!
//! One row in the shared control-plane SQLite DB, opened the same way every other subsystem opens
//! it (`Mutex<Connection>`, WAL journal, `CREATE TABLE IF NOT EXISTS` — there is no migration
//! framework, so the schema is only ever additive).
//!
//! It exists because the rollup writes each bucket exactly once (see the module header). Without
//! it, a restart would resume from "one bucket ago" and republish everything the previous process
//! had already written, which `metric_query`'s `sum(value)` bucket fold would happily double.

use std::sync::Mutex;

use photon_core::PhotonError;
use rusqlite::Connection;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS rollup_watermark (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    bucket_ts       INTEGER NOT NULL,
    first_bucket_ts INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS rollup_latency_ladder (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    metric         TEXT    NOT NULL,
    repair_from    INTEGER NOT NULL,
    repair_through INTEGER NOT NULL
);
"#;

/// What the latency histogram is published under right now, and how much of the already-covered
/// range still has to be republished under that name.
///
/// The ladder (`photon_query::LATENCY_BOUNDS_NS`) is a wire contract: renumbering it changes what
/// every stored point means, so a renumbering comes with a rename (`LATENCY_METRIC`). The rename
/// is what this row tracks — it is how a running install notices that the points it wrote under
/// the old name are now unreadable, and republishes the histogram for that range.
///
/// `repair_from >= repair_through` means there is nothing left to do, which is also the state of
/// a fresh install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LadderState {
    pub metric: String,
    pub repair_from: i64,
    pub repair_through: i64,
}

impl LadderState {
    pub fn pending(&self) -> bool {
        self.repair_from < self.repair_through
    }
}

fn err<E: std::fmt::Display>(e: E) -> PhotonError {
    PhotonError::Io(e.to_string())
}

pub struct Watermark {
    conn: Mutex<Connection>,
}

impl Watermark {
    pub fn open(path: &str) -> Result<Watermark, PhotonError> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(err)?;
            }
        }
        let conn = Connection::open(path).map_err(err)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
            .map_err(err)?;
        Self::from_conn(conn)
    }

    fn from_conn(conn: Connection) -> Result<Watermark, PhotonError> {
        conn.execute_batch(SCHEMA).map_err(err)?;
        Ok(Watermark {
            conn: Mutex::new(conn),
        })
    }

    /// The newest bucket start already published, or `None` on a fresh install.
    pub fn read(&self) -> Result<Option<i64>, PhotonError> {
        Ok(self.range()?.map(|(_, last)| last))
    }

    /// `(oldest, newest)` published bucket start, or `None` on a fresh install.
    ///
    /// The read paths need the OLDEST too, not just the watermark. A rollup enabled yesterday has
    /// nothing to say about last week, and answering such a window with zeros reads exactly like
    /// an outage — so `photon-api` uses this to refuse the window instead.
    pub fn range(&self) -> Result<Option<(i64, i64)>, PhotonError> {
        let conn = self.conn.lock().map_err(|e| err(e.to_string()))?;
        conn.query_row(
            "SELECT first_bucket_ts, bucket_ts FROM rollup_watermark WHERE id = 1",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(err(other)),
        })
    }

    /// Record the pass's published range. `bucket_ts` (the newest bucket) never moves backwards —
    /// a pass that somehow computed an older window must not reopen buckets a later pass already
    /// closed, because reopening them means republishing them. `first_bucket_ts` never moves
    /// forwards, for the mirror-image reason: coverage only ever grows.
    pub fn write(&self, first_bucket_ts: i64, bucket_ts: i64) -> Result<(), PhotonError> {
        let conn = self.conn.lock().map_err(|e| err(e.to_string()))?;
        conn.execute(
            "INSERT INTO rollup_watermark (id, bucket_ts, first_bucket_ts) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET
                 bucket_ts       = max(bucket_ts, excluded.bucket_ts),
                 -- Only ever moves DOWN, and only on a backfill: whatever was published earliest
                 -- stays the floor of coverage.
                 first_bucket_ts = min(first_bucket_ts, excluded.first_bucket_ts)",
            [bucket_ts, first_bucket_ts],
        )
        .map_err(err)?;
        Ok(())
    }

    /// The recorded latency-metric name and repair progress, or `None` before the first pass has
    /// ever recorded one.
    pub fn ladder(&self) -> Result<Option<LadderState>, PhotonError> {
        let conn = self.conn.lock().map_err(|e| err(e.to_string()))?;
        conn.query_row(
            "SELECT metric, repair_from, repair_through FROM rollup_latency_ladder WHERE id = 1",
            [],
            |r| {
                Ok(LadderState {
                    metric: r.get(0)?,
                    repair_from: r.get(1)?,
                    repair_through: r.get(2)?,
                })
            },
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(err(other)),
        })
    }

    /// Replace the ladder row outright. Unlike [`write`](Self::write) this has no monotonic guard:
    /// a rename legitimately resets the repair range, and the range it resets to is decided by the
    /// caller from the watermark it just read.
    pub fn set_ladder(&self, state: &LadderState) -> Result<(), PhotonError> {
        let conn = self.conn.lock().map_err(|e| err(e.to_string()))?;
        conn.execute(
            "INSERT INTO rollup_latency_ladder (id, metric, repair_from, repair_through)
             VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET
                 metric         = excluded.metric,
                 repair_from    = excluded.repair_from,
                 repair_through = excluded.repair_through",
            rusqlite::params![state.metric, state.repair_from, state.repair_through],
        )
        .map_err(err)?;
        Ok(())
    }

    /// Record that the repair has republished everything below `from`. Only ever moves forwards —
    /// a slice that somehow resumed earlier must not rewind progress and re-read spans already
    /// covered.
    pub fn advance_repair(&self, from: i64) -> Result<(), PhotonError> {
        let conn = self.conn.lock().map_err(|e| err(e.to_string()))?;
        conn.execute(
            "UPDATE rollup_latency_ladder SET repair_from = max(repair_from, ?1) WHERE id = 1",
            [from],
        )
        .map_err(err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Watermark {
        Watermark::from_conn(Connection::open_in_memory().unwrap()).unwrap()
    }

    #[test]
    fn a_fresh_install_has_no_watermark() {
        assert_eq!(mem().read().unwrap(), None);
    }

    #[test]
    fn writing_then_reading_round_trips() {
        let w = mem();
        w.write(1_700_000_000_000_000_000, 1_700_000_000_000_000_000)
            .unwrap();
        assert_eq!(w.read().unwrap(), Some(1_700_000_000_000_000_000));
        w.write(1_700_000_060_000_000_000, 1_700_000_060_000_000_000)
            .unwrap();
        assert_eq!(w.read().unwrap(), Some(1_700_000_060_000_000_000));
        assert_eq!(
            w.range().unwrap(),
            Some((1_700_000_000_000_000_000, 1_700_000_060_000_000_000)),
            "coverage spans from the first bucket ever published to the newest"
        );
    }

    #[test]
    fn the_watermark_never_moves_backwards() {
        let w = mem();
        w.write(2_000, 2_000).unwrap();
        w.write(1_000, 1_000).unwrap();
        assert_eq!(
            w.read().unwrap(),
            Some(2_000),
            "moving back would republish buckets a later pass already closed"
        );
    }

    #[test]
    fn coverage_only_ever_grows_downwards_at_the_floor() {
        let w = mem();
        w.write(5_000, 6_000).unwrap();
        w.write(3_000, 4_000).unwrap();
        assert_eq!(
            w.range().unwrap(),
            Some((3_000, 6_000)),
            "a backfill lowers the floor; it must not raise it"
        );
    }

    #[test]
    fn a_fresh_install_has_no_ladder_row() {
        assert_eq!(mem().ladder().unwrap(), None);
    }

    #[test]
    fn the_ladder_row_round_trips_and_the_repair_only_moves_forwards() {
        let w = mem();
        w.set_ladder(&LadderState {
            metric: "latency".into(),
            repair_from: 1_000,
            repair_through: 5_000,
        })
        .unwrap();
        let s = w.ladder().unwrap().unwrap();
        assert!(s.pending());

        w.advance_repair(3_000).unwrap();
        assert_eq!(w.ladder().unwrap().unwrap().repair_from, 3_000);
        w.advance_repair(2_000).unwrap();
        assert_eq!(
            w.ladder().unwrap().unwrap().repair_from,
            3_000,
            "rewinding would re-read spans the repair already covered"
        );

        w.advance_repair(5_000).unwrap();
        assert!(!w.ladder().unwrap().unwrap().pending());
    }

    #[test]
    fn a_rename_resets_the_repair_range_rather_than_merging_with_it() {
        // `write`'s monotonic guards are right for coverage and wrong here: the whole point of a
        // rename is that the previously-repaired range no longer counts.
        let w = mem();
        w.set_ladder(&LadderState {
            metric: "latency".into(),
            repair_from: 9_000,
            repair_through: 9_000,
        })
        .unwrap();
        w.set_ladder(&LadderState {
            metric: "latency_v2".into(),
            repair_from: 1_000,
            repair_through: 9_000,
        })
        .unwrap();
        let s = w.ladder().unwrap().unwrap();
        assert_eq!(s.metric, "latency_v2");
        assert_eq!((s.repair_from, s.repair_through), (1_000, 9_000));
    }
}
