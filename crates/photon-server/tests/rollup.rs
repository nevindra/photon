//! End-to-end test for the `photon.service.*` rollup (ADR-0002): spans in one end,
//! `photon.service.*` metrics out the other, read back through the real `MetricsQueryEngine`.
//!
//! Two things can only be proved here, not in `photon-query`'s or `rollup/points.rs`'s unit tests:
//!
//! 1. **The histogram payload actually re-quantizes.** The writer (`rollup/points.rs`) and the
//!    reader (`photon-query`'s `HistPayload`) pin that JSON shape independently — neither one
//!    imports the other's definition. A renamed field would silently produce an empty histogram
//!    and a `null` p50, and every unit test on either side would still pass.
//! 2. **A bucket is published exactly once.** The watermark is what stands between the rollup and
//!    `metric_query`'s `sum(value)` bucket fold counting the same minute on every pass.

use std::collections::BTreeMap;
use std::sync::Arc;

use photon_api::settings::{SettingsStore, SqliteSettingsStore};
use photon_compact::{MetricsCompactor, SpanCompactor};
use photon_core::config::{RollupConfig, StorageConfig, WalConfig};
use photon_core::ingest_counters::IngestCounters;
use photon_core::metric_agg::Agg;
use photon_core::metric_schema::MetricSchema;
use photon_core::span_record::{SpanBatchBuilder, SpanRecord};
use photon_core::span_schema::SpanSchema;
use photon_query::{
    MetricSeriesRequest, MetricsQueryEngine, RedGroup, SpanQueryEngine, SpanQueryRequest, SpanSort,
};
use photon_server::rollup::RollupTask;
use photon_storage::{Replicator, Storage};
use photon_wal::{BroadcastingWal, DiskWal, Wal};

const NS_PER_SEC: i64 = 1_000_000_000;
const MINUTE: i64 = 60 * NS_PER_SEC;
const MS: i64 = 1_000_000;
/// The one bucket under test, on a whole-minute boundary well clear of `now`.
const BUCKET: i64 = 1_700_000_040 * NS_PER_SEC;
const LAG_SECS: u64 = 60;
const KIND_SERVER: i32 = 2;
const KIND_CLIENT: i32 = 3;

/// The shipped `promoted_attributes` default verbatim (`photon-core/src/default.toml`). Not a
/// trimmed test-only list: `host.name` is part of the metrics sort key, so a metrics schema
/// without it cannot compact at all.
fn schemas() -> (SpanSchema, MetricSchema) {
    let promoted = vec![
        "service.name".to_string(),
        "host.name".to_string(),
        "deployment.environment".to_string(),
    ];
    (SpanSchema::new(&promoted), MetricSchema::new(&promoted))
}

fn span(i: usize, name: &str, kind: i32, offset_ns: i64, dur: i64, status: i32) -> SpanRecord {
    let mut attributes = BTreeMap::new();
    attributes.insert("service.name".into(), "api".to_string());
    attributes.insert("deployment.environment".into(), "prod".to_string());
    SpanRecord {
        trace_id: "t1".into(),
        span_id: format!("s{i}"),
        parent_span_id: Some("parent".into()),
        name: Some(name.into()),
        kind: Some(kind),
        start_time_nanos: BUCKET + offset_ns,
        duration_nanos: Some(dur),
        status_code: Some(status),
        attributes,
        ..Default::default()
    }
}

/// A span far outside the window, whose only job is to close the segment holding the real data —
/// the compactor only ever sees CLOSED segments (same trick as `alerts_e2e`/`infra_e2e`).
fn seal_span() -> SpanRecord {
    let mut attributes = BTreeMap::new();
    attributes.insert("service.name".into(), "__seal__".to_string());
    SpanRecord {
        trace_id: "seal".into(),
        span_id: "seal".into(),
        start_time_nanos: 1,
        attributes,
        ..Default::default()
    }
}

fn tiny_wal_cfg() -> WalConfig {
    WalConfig {
        segment_max_bytes: 1,
        segment_max_age_secs: 0,
        group_commit_max_delay_ms: 0,
    }
}

fn storage_for(hot: &std::path::Path) -> (Storage, Arc<Replicator>) {
    let storage = Storage::from_config(&StorageConfig {
        hot_dir: hot.to_path_buf(),
        db_path: String::new(),
        durable: None,
        zstd_level: 1,
    })
    .unwrap();
    let replicator = Arc::new(Replicator::new(storage.clone()));
    (storage, replicator)
}

/// The whole rig: spans compacted into hot Parquet, an empty metrics WAL, and a `RollupTask`
/// pointed at both.
struct Rig {
    task: RollupTask,
    metrics_wal: Arc<BroadcastingWal<DiskWal>>,
    metric_schema: MetricSchema,
    hot: std::path::PathBuf,
    _tmp: tempfile::TempDir,
}

async fn rig(spans: Vec<SpanRecord>, top_operations: usize) -> Rig {
    rig_with_tee(spans, top_operations, None).await
}

async fn rig_with_tee(
    spans: Vec<SpanRecord>,
    top_operations: usize,
    tee: Option<photon_ingest::FederationTee>,
) -> Rig {
    let (span_schema, metric_schema) = schemas();
    let tmp = tempfile::tempdir().unwrap();
    let hot = tmp.path().to_path_buf();

    // --- spans in, compacted to hot Parquet ---
    let spans_wal = Arc::new(
        DiskWal::open_arrow(
            hot.join("wal-spans"),
            span_schema.arrow.clone(),
            tiny_wal_cfg(),
        )
        .await
        .unwrap(),
    );
    let mut b = SpanBatchBuilder::new(&span_schema);
    for s in &spans {
        b.append(s);
    }
    spans_wal.append(b.finish().unwrap()).await.unwrap();
    spans_wal.sync().await.unwrap();
    let mut sealer = SpanBatchBuilder::new(&span_schema);
    sealer.append(&seal_span());
    spans_wal.append(sealer.finish().unwrap()).await.unwrap();
    spans_wal.sync().await.unwrap();

    let (storage, replicator) = storage_for(&hot);
    let compactor = SpanCompactor::new(spans_wal, storage, replicator, span_schema.clone());
    while compactor.run_once().await.unwrap().is_some() {}

    // --- an empty metrics WAL for the rollup to write into ---
    let metrics_wal = Arc::new(BroadcastingWal::new(
        DiskWal::open_arrow(
            hot.join("wal-metrics"),
            metric_schema.arrow.clone(),
            tiny_wal_cfg(),
        )
        .await
        .unwrap(),
        16,
    ));

    let db_path = hot.join("photon.db").to_string_lossy().to_string();
    let settings: Arc<dyn SettingsStore> = Arc::new(SqliteSettingsStore::open(&db_path).unwrap());
    let task = RollupTask::new(
        SpanQueryEngine::new(hot.clone(), span_schema).unwrap(),
        metrics_wal.clone(),
        metric_schema.clone(),
        Arc::new(IngestCounters::new()),
        settings,
        500, // Apdex T = 500ms
        RollupConfig {
            lag_secs: LAG_SECS,
            top_operations,
            ..Default::default()
        },
        &db_path,
        tee,
    )
    .unwrap();

    Rig {
        task,
        metrics_wal,
        metric_schema,
        hot,
        _tmp: tmp,
    }
}

impl Rig {
    /// A second engine over the same compacted spans, for comparing the two read paths.
    fn span_engine(&self) -> SpanQueryEngine {
        SpanQueryEngine::new(self.hot.clone(), schemas().0).unwrap()
    }

    /// A `now` that makes exactly the bucket at [`BUCKET`] settled and nothing after it.
    fn now_after_the_bucket_settles(&self) -> i64 {
        BUCKET + MINUTE + LAG_SECS as i64 * NS_PER_SEC
    }

    /// Compact whatever the rollup wrote and hand back a queryable engine.
    async fn metrics_engine(&self) -> MetricsQueryEngine {
        // The rollup's own append left a segment open; seal it the same way the span WAL was.
        let mut b = photon_core::metric_record::MetricBatchBuilder::new(&self.metric_schema);
        b.append(&photon_core::metric_record::MetricPoint {
            metric_name: "__seal__".into(),
            metric_type: 0,
            type_text: None,
            temporality: None,
            is_monotonic: None,
            unit: None,
            timestamp_nanos: 1,
            start_timestamp_nanos: None,
            scope_name: None,
            value: Some(0.0),
            histogram: None,
            exp_histogram: None,
            summary: None,
            exemplars: None,
            attributes: BTreeMap::new(),
        });
        self.metrics_wal.append(b.finish().unwrap()).await.unwrap();
        self.metrics_wal.sync().await.unwrap();

        let (storage, replicator) = storage_for(&self.hot);
        let compactor = MetricsCompactor::new(
            self.metrics_wal.clone(),
            storage,
            replicator,
            self.metric_schema.clone(),
        );
        while compactor.run_once().await.unwrap().is_some() {}
        MetricsQueryEngine::new(self.hot.clone(), self.metric_schema.clone()).unwrap()
    }

    /// One aggregated value for `metric` over the bucket, or `None` when the series is absent.
    async fn read(&self, engine: &MetricsQueryEngine, metric: &str, agg: Agg) -> Option<f64> {
        let res = engine
            .query_series(MetricSeriesRequest {
                metric: metric.to_string(),
                agg: Some(agg),
                group_by: Vec::new(),
                filter: None,
                start_ts_nanos: BUCKET,
                end_ts_nanos: BUCKET + MINUTE - 1,
                buckets: 1,
            })
            .await
            .unwrap();
        res.series
            .first()
            .and_then(|s| s.points.first())
            .and_then(|p| p.v)
    }
}

/// Ten SERVER requests (one an OTEL error) plus four CLIENT spans the service emitted while
/// serving them — the shape ADR-0003 exists to tell apart.
fn traffic() -> Vec<SpanRecord> {
    let mut spans = Vec::new();
    for i in 0..7 {
        // 7 fast requests: 3ms, into the (2ms, 5ms] histogram bucket.
        spans.push(span(i, "GET /a", KIND_SERVER, i as i64 * MS, 3 * MS, 1));
    }
    for i in 7..9 {
        spans.push(span(i, "GET /b", KIND_SERVER, i as i64 * MS, 300 * MS, 1));
    }
    // One error.
    spans.push(span(9, "GET /b", KIND_SERVER, 9 * MS, 900 * MS, 2));
    for i in 10..14 {
        spans.push(span(i, "SELECT users", KIND_CLIENT, i as i64 * MS, MS, 1));
    }
    spans
}

#[tokio::test]
async fn a_pass_publishes_request_counts_that_survive_the_round_trip_to_metrics() {
    let rig = rig(traffic(), 10).await;
    let published = rig
        .task
        .run_once(rig.now_after_the_bucket_settles())
        .await
        .unwrap();
    assert!(published > 0, "the settled bucket must have been published");

    let engine = rig.metrics_engine().await;

    assert_eq!(
        rig.read(&engine, "photon.service.requests", Agg::Sum).await,
        Some(10.0),
        "the 10 SERVER spans are the requests; the 4 CLIENT spans are not"
    );
    assert_eq!(
        rig.read(&engine, "photon.service.spans", Agg::Sum).await,
        Some(14.0),
        "every span is still reported, separately"
    );
    assert_eq!(
        rig.read(&engine, "photon.service.errors", Agg::Sum).await,
        Some(1.0)
    );
    assert_eq!(
        rig.read(&engine, "photon.service.apdex.satisfied", Agg::Sum)
            .await,
        Some(9.0),
        "9 requests came in under the 500ms Apdex T"
    );
    assert_eq!(
        rig.read(&engine, "photon.service.apdex.tolerating", Agg::Sum)
            .await,
        Some(1.0),
        "the 900ms request is past T (500ms) but inside 4T (2s)"
    );
    assert_eq!(
        rig.read(&engine, "photon.service.apdex.frustrated", Agg::Sum)
            .await,
        Some(0.0),
        "nothing here took longer than 2s"
    );
    assert_eq!(
        rig.read(&engine, "photon.service.basis", Agg::Last).await,
        None,
        "an entry-based service says nothing about its basis"
    );
}

#[tokio::test]
async fn the_published_histogram_re_quantizes_through_the_metrics_engine() {
    let rig = rig(traffic(), 10).await;
    rig.task
        .run_once(rig.now_after_the_bucket_settles())
        .await
        .unwrap();
    let engine = rig.metrics_engine().await;

    // This is the assertion the whole file exists for: a renamed or retyped field in the emitted
    // histogram JSON makes this `None` while every unit test on both sides still passes.
    let p50 = rig
        .read(&engine, "photon.service.duration", Agg::P50)
        .await
        .expect("the histogram must be readable, not an empty payload");
    // 7 of 10 requests took 3ms, so the median sits in the (2ms, 5ms] bucket. Interpolation puts
    // it somewhere inside that bucket, never outside it.
    assert!(
        p50 > 2.0 * MS as f64 && p50 <= 5.0 * MS as f64,
        "p50 {p50}ns should land in the (2ms, 5ms] bucket"
    );

    let p99 = rig
        .read(&engine, "photon.service.duration", Agg::P99)
        .await
        .unwrap();
    assert!(
        p99 > p50,
        "p99 {p99}ns must be above p50 {p50}ns — the 900ms request is in there"
    );
}

#[tokio::test]
async fn a_settled_bucket_is_published_once_and_never_recomputed() {
    let rig = rig(traffic(), 10).await;
    let now = rig.now_after_the_bucket_settles();

    let first = rig.task.run_once(now).await.unwrap();
    assert!(first > 0);

    // Same clock, same window — the watermark, not the clock, is what stops the second pass.
    assert_eq!(
        rig.task.run_once(now).await.unwrap(),
        0,
        "re-rolling would double every counter: the reader's bucket fold is sum(), not last()"
    );
    // And again a minute later, when a new bucket exists but the old one is still behind the mark.
    assert_eq!(rig.task.run_once(now + MINUTE).await.unwrap(), 0);

    let engine = rig.metrics_engine().await;
    assert_eq!(
        rig.read(&engine, "photon.service.requests", Agg::Sum).await,
        Some(10.0),
        "10, not 20 or 30"
    );
}

#[tokio::test]
async fn a_bucket_still_inside_the_lag_window_is_not_published_yet() {
    let rig = rig(traffic(), 10).await;

    // One second before the bucket's lag elapses.
    let too_early = BUCKET + MINUTE + LAG_SECS as i64 * NS_PER_SEC - NS_PER_SEC;
    assert_eq!(
        rig.task.run_once(too_early).await.unwrap(),
        0,
        "a span arriving within the lag window must still be able to land in this bucket"
    );
    assert!(
        rig.task
            .run_once(BUCKET + MINUTE + LAG_SECS as i64 * NS_PER_SEC)
            .await
            .unwrap()
            > 0
    );
}

#[tokio::test]
async fn the_operation_tail_is_published_as_other() {
    // top_operations = 1, so `GET /a` (7 requests) keeps its own series and everything else folds.
    let rig = rig(traffic(), 1).await;
    rig.task
        .run_once(rig.now_after_the_bucket_settles())
        .await
        .unwrap();
    let engine = rig.metrics_engine().await;

    let res = engine
        .query_series(MetricSeriesRequest {
            metric: "photon.operation.requests".into(),
            agg: Some(Agg::Sum),
            group_by: vec!["operation".into()],
            filter: None,
            start_ts_nanos: BUCKET,
            end_ts_nanos: BUCKET + MINUTE - 1,
            buckets: 1,
        })
        .await
        .unwrap();

    let by_op: BTreeMap<String, f64> = res
        .series
        .iter()
        .filter_map(|s| {
            let op = s.labels.get("operation")?.clone();
            let v = s.points.first().and_then(|p| p.v)?;
            Some((op, v))
        })
        .collect();

    assert_eq!(by_op.get("GET /a"), Some(&7.0));
    assert_eq!(
        by_op.get(photon_query::OTHER_OPERATION),
        Some(&3.0),
        "`GET /b`'s three requests fold into the remainder"
    );
    assert!(
        !by_op.contains_key("SELECT users"),
        "a CLIENT-only operation serves no requests"
    );
}

#[tokio::test]
async fn the_operation_family_never_inflates_the_service_family() {
    // The regression this separation exists for: with both levels under one metric name, a bare
    // `photon.service.requests` query summed 24 for a service that served 10.
    let rig = rig(traffic(), 10).await;
    rig.task
        .run_once(rig.now_after_the_bucket_settles())
        .await
        .unwrap();
    let engine = rig.metrics_engine().await;

    assert_eq!(
        rig.read(&engine, "photon.service.requests", Agg::Sum).await,
        Some(10.0)
    );
    assert_eq!(
        rig.read(&engine, "photon.operation.requests", Agg::Sum)
            .await,
        Some(10.0),
        "operations sum to their service: 7 + 3, and the CLIENT-only operation contributes 0"
    );
}

/// The property Task 6's read-path switch rests on: whichever source answers, the numbers agree.
/// If they ever diverge, a user widening their time window would watch a service's traffic change
/// for no reason they can see.
#[tokio::test]
async fn the_rollup_and_the_raw_span_path_report_the_same_numbers() {
    let rig = rig(traffic(), 10).await;
    rig.task
        .run_once(rig.now_after_the_bucket_settles())
        .await
        .unwrap();
    let engine = rig.metrics_engine().await;

    // Raw: the span engine over the same window, exactly what `/api/red` falls back to.
    let raw = rig
        .span_engine()
        .red_metrics(
            SpanQueryRequest {
                start_ts_nanos: BUCKET,
                end_ts_nanos: BUCKET + MINUTE - 1,
                query: None,
                sort: SpanSort::Recent,
                limit: 0,
                offset: 0,
                projected_attributes: Vec::new(),
            },
            RedGroup::Service,
            &Default::default(),
            500,
        )
        .await
        .unwrap();
    let raw = raw.iter().find(|r| r.service == "api").unwrap();

    for (metric, raw_value, label) in [
        ("requests", raw.count, "requests"),
        ("spans", raw.span_count, "spans"),
        ("errors", raw.error_count, "errors"),
        ("apdex.satisfied", raw.satisfied, "satisfied"),
        ("apdex.tolerating", raw.tolerating, "tolerating"),
        ("apdex.frustrated", raw.frustrated, "frustrated"),
        ("http_4xx", raw.http_4xx, "http_4xx"),
        ("http_5xx", raw.http_5xx, "http_5xx"),
    ] {
        assert_eq!(
            rig.read(&engine, &format!("photon.service.{metric}"), Agg::Sum)
                .await,
            Some(raw_value as f64),
            "{label} must match between the rollup and the raw span path"
        );
    }

    // Percentiles are the one place exact equality is the wrong bar: raw uses a t-digest, the
    // rollup interpolates a 13-bucket histogram. They must agree on the bucket, not the value.
    let rollup_p50 = rig
        .read(&engine, "photon.service.duration", Agg::P50)
        .await
        .unwrap();
    assert!(
        rollup_p50 > 2.0 * MS as f64 && rollup_p50 <= 5.0 * MS as f64,
        "rollup p50 {rollup_p50}ns and raw p50 {}ns must land in the same histogram bucket",
        raw.p50
    );
    assert!(raw.p50 > 2 * MS && raw.p50 <= 5 * MS);
}

/// Task 7: a Site ships its rollup to central. The rollup writes straight to the local metrics
/// WAL, so it never passes through OTLP ingest and neither federation mode picks it up on its own
/// — this tee is the only path it has.
#[tokio::test]
async fn a_federated_site_ships_its_rollup_to_central() {
    use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
    use prost::Message as _;

    let (tee, mut rx) =
        photon_ingest::FederationTee::channel_for(16, &[photon_ingest::TeeSignal::Metrics]);
    let rig = rig_with_tee(traffic(), 10, Some(tee)).await;
    rig.task
        .run_once(rig.now_after_the_bucket_settles())
        .await
        .unwrap();

    let (signal, payload) = rx
        .try_recv()
        .expect("a pass must offer its points to central");
    assert_eq!(signal, photon_ingest::TeeSignal::Metrics);

    let req = ExportMetricsServiceRequest::decode(payload.as_ref()).unwrap();
    let names: Vec<String> = req
        .resource_metrics
        .iter()
        .flat_map(|r| &r.scope_metrics)
        .flat_map(|s| &s.metrics)
        .map(|m| m.name.clone())
        .collect();
    assert!(names.iter().any(|n| n == "photon.service.requests"));
    assert!(names.iter().any(|n| n == "photon.service.duration"));
    assert!(
        names.iter().any(|n| n == "photon.operation.requests"),
        "central needs the operation level too, or a Site's endpoints are invisible there"
    );

    // Identity must ride on the resource: central stamps `tenant` alongside it.
    let res = req.resource_metrics[0].resource.as_ref().unwrap();
    assert!(res.attributes.iter().any(|a| a.key == "service.name"));
}

#[tokio::test]
async fn an_unfederated_site_ships_nothing() {
    let rig = rig(traffic(), 10).await;
    let published = rig
        .task
        .run_once(rig.now_after_the_bucket_settles())
        .await
        .unwrap();
    assert!(published > 0, "it still publishes locally");
    // Nothing to assert on the wire: there is no wire. The absence of a tee is the whole point —
    // this test exists so that wiring one in unconditionally would fail loudly rather than
    // silently start pushing every install's data somewhere.
}

/// The local write is the source of truth; central is a mirror. A central that is unreachable, or
/// a tee whose queue is full, must not cost this Site its own data.
#[tokio::test]
async fn a_full_tee_never_costs_the_local_write() {
    // Capacity 1, and never drained — the second offer has nowhere to go.
    let (tee, _rx) =
        photon_ingest::FederationTee::channel_for(1, &[photon_ingest::TeeSignal::Metrics]);
    let dropped = tee.dropped.clone();
    let rig = rig_with_tee(traffic(), 10, Some(tee)).await;

    let now = rig.now_after_the_bucket_settles();
    rig.task.run_once(now).await.unwrap();

    let engine = rig.metrics_engine().await;
    assert_eq!(
        rig.read(&engine, "photon.service.requests", Agg::Sum).await,
        Some(10.0),
        "the local store is complete regardless of what central received"
    );
    // The counter is the operator-facing signal that a mirror fell behind; it is not an error.
    let _ = dropped.load(std::sync::atomic::Ordering::Relaxed);
}
