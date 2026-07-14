//! Behavioural tests for `photon-wal`, exercised through the public contract surface
//! (`DiskWal` + the `Wal` trait) only.

use arrow::array::{Array, StringArray, TimestampNanosecondArray};
use arrow::record_batch::RecordBatch;
use photon_core::config::WalConfig;
use photon_core::record::{LogRecord, RecordBatchBuilder};
use photon_core::schema::LogSchema;
use photon_core::segment::SegmentId;
use photon_core::PhotonError;
use photon_wal::{DiskWal, Wal};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

fn schema() -> LogSchema {
    LogSchema::new(&["service.name".to_string()])
}

fn cfg(max_bytes: u64, max_age_secs: u64, delay_ms: u64) -> WalConfig {
    WalConfig {
        segment_max_bytes: max_bytes,
        segment_max_age_secs: max_age_secs,
        group_commit_max_delay_ms: delay_ms,
    }
}

/// Build a batch from `(timestamp_nanos, service.name, body)` rows.
fn batch(schema: &LogSchema, rows: &[(i64, &str, &str)]) -> RecordBatch {
    let mut b = RecordBatchBuilder::new(schema);
    for (ts, svc, body) in rows {
        let mut attributes = BTreeMap::new();
        attributes.insert("service.name".to_string(), svc.to_string());
        b.append(&LogRecord {
            timestamp_nanos: *ts,
            body: Some(body.to_string()),
            attributes,
            ..Default::default()
        });
    }
    b.finish().unwrap()
}

/// Extract `(timestamp, body)` pairs from a batch for order-preserving comparison.
fn rows_of(batch: &RecordBatch) -> Vec<(i64, String)> {
    let ts = batch
        .column_by_name("timestamp")
        .unwrap()
        .as_any()
        .downcast_ref::<TimestampNanosecondArray>()
        .unwrap();
    let body = batch
        .column_by_name("body")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    (0..batch.num_rows())
        .map(|i| (ts.value(i), body.value(i).to_string()))
        .collect()
}

fn all_rows(batches: &[RecordBatch]) -> Vec<(i64, String)> {
    batches.iter().flat_map(rows_of).collect()
}

// 1. append -> rotate -> read_segment round-trips the exact rows.
#[tokio::test]
async fn append_rotate_read_roundtrips_rows() {
    let dir = tempfile::tempdir().unwrap();
    let schema = schema();
    // A 1-byte cap forces every append to rotate into its own closed segment.
    let wal = DiskWal::open(dir.path(), schema.clone(), cfg(1, 3600, 5))
        .await
        .unwrap();

    let inputs = vec![
        batch(&schema, &[(10, "api", "alpha"), (11, "api", "beta")]),
        batch(&schema, &[(20, "web", "gamma")]),
        batch(&schema, &[(30, "db", "delta"), (31, "db", "epsilon")]),
    ];
    let expected: Vec<(i64, String)> = inputs.iter().flat_map(rows_of).collect();

    for b in &inputs {
        wal.append(b.clone()).await.unwrap();
    }
    wal.sync().await.unwrap(); // barrier so every rotation has settled

    let closed = wal.list_closed_segments().unwrap();
    assert_eq!(closed.len(), 3, "each batch should rotate: {closed:?}");
    assert!(
        closed.windows(2).all(|w| w[0] < w[1]),
        "closed segments must be ascending: {closed:?}"
    );

    let mut got = Vec::new();
    for id in closed {
        got.extend(all_rows(&wal.read_segment(id).await.unwrap()));
    }
    assert_eq!(got, expected);
}

// 2. group commit: many concurrent appends all resolve, coalesce into few fsyncs, and all
//    data is present after reopen.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn group_commit_coalesces_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let schema = schema();
    // Huge cap => no rotation; a generous window so concurrent appends batch together.
    let wal = Arc::new(
        DiskWal::open(dir.path(), schema.clone(), cfg(1 << 30, 3600, 25))
            .await
            .unwrap(),
    );

    let n: usize = 100;
    let mut handles = Vec::new();
    for i in 0..n {
        let w = wal.clone();
        let s = schema.clone();
        handles.push(tokio::spawn(async move {
            w.append(batch(&s, &[(i as i64, "svc", "x")])).await
        }));
    }
    for h in handles {
        h.await.unwrap().unwrap();
    }

    let rounds = wal.commit_rounds();
    assert!(
        rounds >= 1 && rounds < n as u64,
        "{n} appends should coalesce into fewer than {n} fsyncs, got {rounds}"
    );

    wal.sync().await.unwrap();
    drop(wal);

    // Reopen: the active segment is sealed on open, so every appended row is now readable.
    let wal2 = DiskWal::open(dir.path(), schema.clone(), cfg(1 << 30, 3600, 25))
        .await
        .unwrap();
    let mut seen = BTreeSet::new();
    for id in wal2.list_closed_segments().unwrap() {
        for (ts, _) in all_rows(&wal2.read_segment(id).await.unwrap()) {
            seen.insert(ts);
        }
    }
    let expected: BTreeSet<i64> = (0..n as i64).collect();
    assert_eq!(seen, expected);
}

// 3a. torn-tail recovery via truncation: the final frame is chopped mid-payload.
#[tokio::test]
async fn recovers_after_torn_tail_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let schema = schema();
    {
        let wal = DiskWal::open(dir.path(), schema.clone(), cfg(1 << 30, 3600, 2))
            .await
            .unwrap();
        for i in 0..5i64 {
            wal.append(batch(&schema, &[(i, "svc", "body")]))
                .await
                .unwrap();
        }
        wal.sync().await.unwrap();
    } // drop the writer

    // seg 0 holds 5 frames; lop 3 bytes off the end to tear the final one.
    let seg0 = dir.path().join(SegmentId(0).filename());
    let len = std::fs::metadata(&seg0).unwrap().len();
    let f = std::fs::OpenOptions::new().write(true).open(&seg0).unwrap();
    f.set_len(len - 3).unwrap();
    drop(f);

    let wal2 = DiskWal::open(dir.path(), schema.clone(), cfg(1 << 30, 3600, 2))
        .await
        .unwrap();
    assert_eq!(wal2.list_closed_segments().unwrap(), vec![SegmentId(0)]);
    let rows = all_rows(&wal2.read_segment(SegmentId(0)).await.unwrap());
    assert_eq!(
        rows,
        vec![
            (0, "body".to_string()),
            (1, "body".to_string()),
            (2, "body".to_string()),
            (3, "body".to_string()),
        ],
        "torn 5th frame dropped, first four recovered"
    );
}

// 3b. torn-tail recovery via crc mismatch: the final frame's payload byte is flipped.
#[tokio::test]
async fn recovers_after_torn_tail_crc_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let schema = schema();
    {
        let wal = DiskWal::open(dir.path(), schema.clone(), cfg(1 << 30, 3600, 2))
            .await
            .unwrap();
        for i in 0..3i64 {
            wal.append(batch(&schema, &[(i, "svc", "body")]))
                .await
                .unwrap();
        }
        wal.sync().await.unwrap();
    }

    let seg0 = dir.path().join(SegmentId(0).filename());
    let mut bytes = std::fs::read(&seg0).unwrap();
    let last = bytes.len() - 1; // last byte of the final frame's payload
    bytes[last] ^= 0xFF;
    std::fs::write(&seg0, &bytes).unwrap();

    let wal2 = DiskWal::open(dir.path(), schema.clone(), cfg(1 << 30, 3600, 2))
        .await
        .unwrap();
    let rows = all_rows(&wal2.read_segment(SegmentId(0)).await.unwrap());
    assert_eq!(
        rows,
        vec![(0, "body".to_string()), (1, "body".to_string())],
        "crc-broken 3rd frame dropped, first two recovered"
    );
}

// 4. rotation: a small segment cap spreads data across multiple closed segments with no loss.
#[tokio::test]
async fn rotation_splits_data_across_segments() {
    let dir = tempfile::tempdir().unwrap();
    let schema = schema();
    let wal = DiskWal::open(dir.path(), schema.clone(), cfg(256, 3600, 3))
        .await
        .unwrap();

    let mut expected = Vec::new();
    for i in 0..10i64 {
        let b = batch(&schema, &[(i, "svc", "payload-body")]);
        expected.extend(rows_of(&b));
        wal.append(b).await.unwrap();
    }
    wal.sync().await.unwrap();

    let closed = wal.list_closed_segments().unwrap();
    assert!(
        closed.len() > 1,
        "small cap should produce multiple segments, got {}",
        closed.len()
    );
    assert!(closed.windows(2).all(|w| w[0] < w[1]));
    drop(wal);

    // Reopen seals the trailing active segment; the full sequence must be recoverable.
    let wal2 = DiskWal::open(dir.path(), schema.clone(), cfg(256, 3600, 3))
        .await
        .unwrap();
    let mut got = Vec::new();
    for id in wal2.list_closed_segments().unwrap() {
        got.extend(all_rows(&wal2.read_segment(id).await.unwrap()));
    }
    assert_eq!(got, expected);
}

// 5. reopen: append, drop, reopen from the same dir; closed segments stay readable, and
//    remove_segment is idempotent.
#[tokio::test]
async fn reopen_keeps_closed_segments_readable() {
    let dir = tempfile::tempdir().unwrap();
    let schema = schema();
    let expected: Vec<(i64, String)>;
    {
        let wal = DiskWal::open(dir.path(), schema.clone(), cfg(1 << 30, 3600, 5))
            .await
            .unwrap();
        let inputs = vec![
            batch(&schema, &[(1, "a", "one")]),
            batch(&schema, &[(2, "b", "two"), (3, "b", "three")]),
        ];
        expected = inputs.iter().flat_map(rows_of).collect();
        for b in &inputs {
            wal.append(b.clone()).await.unwrap();
        }
        wal.sync().await.unwrap();
    } // drop shuts down the writer task

    let wal2 = DiskWal::open(dir.path(), schema.clone(), cfg(1 << 30, 3600, 5))
        .await
        .unwrap();
    assert_eq!(
        wal2.list_closed_segments().unwrap(),
        vec![SegmentId(0)],
        "the prior active segment is sealed on reopen"
    );
    let got = all_rows(&wal2.read_segment(SegmentId(0)).await.unwrap());
    assert_eq!(got, expected);

    // remove_segment deletes and is idempotent.
    wal2.remove_segment(SegmentId(0)).unwrap();
    wal2.remove_segment(SegmentId(0)).unwrap();
    assert!(wal2.list_closed_segments().unwrap().is_empty());
    assert!(
        wal2.read_segment(SegmentId(0)).await.is_err(),
        "removed segment file should be gone"
    );
}

// The API is usable purely through the `Wal` trait (as ingest/compact will consume it).
#[tokio::test]
async fn works_through_the_wal_trait() {
    async fn drive<W: Wal>(w: &W, b: RecordBatch) -> Result<(), PhotonError> {
        w.append(b).await?;
        w.sync().await
    }

    let dir = tempfile::tempdir().unwrap();
    let schema = schema();
    let wal = DiskWal::open(dir.path(), schema.clone(), cfg(1, 3600, 2))
        .await
        .unwrap();
    drive(&wal, batch(&schema, &[(7, "svc", "trait")]))
        .await
        .unwrap();

    let closed = <DiskWal as Wal>::list_closed_segments(&wal).unwrap();
    assert!(!closed.is_empty());
    let rows = all_rows(
        &<DiskWal as Wal>::read_segment(&wal, closed[0])
            .await
            .unwrap(),
    );
    assert_eq!(rows, vec![(7, "trait".to_string())]);
}
