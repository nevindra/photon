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
"#;

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
}
