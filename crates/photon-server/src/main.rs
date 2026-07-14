//! photon-server: the single binary. Wires ingest + WAL + compactor + query + API together.
//!
//! Implemented per the `photon-server` section of
//! `docs/superpowers/plans/2026-07-01-photon-interface-contracts.md`.
//!
//! # Modes
//!
//! * `photon-server hash-password <password>` — print an argon2 PHC hash (random OS salt), a
//!   convenience for scripted/manual use. UI users live in the SQLite store, created via the
//!   in-app onboarding + Settings, not in config.
//! * `photon-server [config.toml]` — load the config (path from `argv[1]`, else the
//!   `PHOTON_CONFIG` env var, else `photon.toml`) and run the server: an OTLP ingest front
//!   end (gRPC + HTTP) writing to the WAL, a background compactor draining closed WAL
//!   segments into Parquet (also replaying any segments recovered on startup) and flushing
//!   hot -> durable replication, and a REST/UI API over the query engine.

/// jemalloc returns freed pages to the OS (glibc retained them — B1 measured post-idle ≈ peak)
/// and speeds the multithreaded, allocation-heavy ingest path. Process-global; libraries untouched.
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use std::net::SocketAddr;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Duration;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHasher};

use photon_api::settings::{SettingsStore, SqliteSettingsStore};
use photon_api::{
    signal_from_path, ApiServer, DataAdmin, LiveHub, PurgeCommand, ReplicationStatus,
    RetentionAtomics, SqliteUsageStore, UsageStore,
};
use photon_compact::{Compactor, MetricsCompactor, SpanCompactor};
use photon_core::config::Config;
use photon_core::ingest_counters::IngestCounters;
use photon_core::metric_schema::MetricSchema;
use photon_core::schema::LogSchema;
use photon_core::span_schema::SpanSchema;
use photon_ingest::IngestServer;
use photon_query::{MetricsQueryEngine, QueryEngine, SpanQueryEngine};
use photon_storage::{Replicator, Storage};
use photon_wal::{BroadcastingWal, DiskWal, Wal};

/// How often the background compactor wakes to drain closed WAL segments and flush
/// replication.
const COMPACT_INTERVAL: Duration = Duration::from_secs(2);
/// Run a small-file `merge_once` pass every Nth compactor tick.
const MERGE_EVERY_TICKS: u64 = 5;
/// Run a whole-file retention purge every Nth compactor tick (~1 hour at COMPACT_INTERVAL 2s).
const PURGE_EVERY_TICKS: u64 = 1800;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    // Subcommand: hash a password so operators can fill in `[[auth.users]]`.
    if args.get(1).map(String::as_str) == Some("hash-password") {
        let password = args
            .get(2)
            .ok_or("usage: photon-server hash-password <password>")?;
        println!("{}", hash_password(password)?);
        return Ok(());
    }

    // Subcommand: container health probe. Must run before config load (no config needed).
    if args.get(1).map(String::as_str) == Some("healthcheck") {
        let api_addr =
            std::env::var("PHOTON_API_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
        std::process::exit(if healthcheck(&api_addr) { 0 } else { 1 });
    }

    // Effective config: baked defaults / photon.toml / an explicit file, overlaid with PHOTON_*
    // env vars, then validated. See photon_core::config::Config::load.
    let explicit = args
        .get(1)
        .cloned()
        .or_else(|| std::env::var("PHOTON_CONFIG").ok());
    let cfg = Config::load(explicit)?;

    // Shared building blocks.
    let schema = LogSchema::new(&cfg.schema.promoted_attributes);
    let storage = Storage::from_config(&cfg.storage)?;
    let wal_inner = DiskWal::open(
        cfg.storage.hot_dir.join("wal"),
        schema.clone(),
        cfg.wal.clone(),
    )
    .await?;
    // Wrapped in `BroadcastingWal` so appended batches fan out to the live-tail SSE path (BE-4)
    // after the WAL fsync acks; `Wal` is delegated straight through, so ingest/compactor are
    // unaffected. `.sender()` is handed to the `LiveHub` below.
    let wal = Arc::new(BroadcastingWal::new(wal_inner, cfg.live.broadcast_capacity));
    let logs_tx = wal.sender();
    let replicator = Replicator::new(storage.clone());
    let query = QueryEngine::new(cfg.storage.hot_dir.clone(), schema.clone())?;

    // Spans pipeline (parallel to logs). Reuses the same promoted attributes (service.name is
    // required and present); spans get their own WAL dir, object prefix, and manifest.
    let span_schema = SpanSchema::new(&cfg.schema.promoted_attributes);
    let spans_wal_inner = DiskWal::open_arrow(
        cfg.storage.hot_dir.join("wal-traces"),
        span_schema.arrow.clone(),
        cfg.wal.clone(),
    )
    .await?;
    let spans_wal = Arc::new(BroadcastingWal::new(
        spans_wal_inner,
        cfg.live.broadcast_capacity,
    ));
    let spans_tx = spans_wal.sender();

    // Metrics pipeline (parallel to logs + spans). Same promoted attributes; its own WAL dir,
    // object prefix, and manifest. Also wrapped in `BroadcastingWal` — purely so its concrete
    // type matches `wal`/`spans_wal` (`IngestServer<W>` is monomorphic: `wal`, `spans_wal`, and
    // `metrics_wal` all share one `W: Wal`). Metrics itself does not stream (global constraint:
    // "Metrics does not stream" — the frontend's `MetricsExplorer` polls instead), so its
    // `.sender()` is never taken and no receiver ever subscribes; functionally untouched.
    let metric_schema = MetricSchema::new(&metrics_promoted(&cfg.schema.promoted_attributes));
    let metrics_wal_inner = DiskWal::open_arrow(
        cfg.storage.hot_dir.join("wal-metrics"),
        metric_schema.arrow.clone(),
        cfg.wal.clone(),
    )
    .await?;
    let metrics_wal = Arc::new(BroadcastingWal::new(
        metrics_wal_inner,
        cfg.live.broadcast_capacity,
    ));

    // Runtime settings (per-signal retention days) persist in the shared control-plane SQLite DB.
    let settings: Arc<dyn SettingsStore> =
        Arc::new(SqliteSettingsStore::open(&cfg.storage.db_path)?);

    // Shared usage/replication monitoring state (the `/data` page). `counters` are the per-signal
    // ingest tallies (incremented at every WAL-append ack); `usage` is the SQLite usage store that
    // records 60s footprint+counter samples and successfully-replicated object sizes.
    let counters = Arc::new(IngestCounters::new());
    let usage: Arc<dyn UsageStore> = Arc::new(SqliteUsageStore::open(&cfg.storage.db_path)?);

    // photon-api cannot depend on photon-storage, so expose the replicator to the API through the
    // `ReplicationStatus` trait. A newtype is required by the orphan rule: neither the trait nor
    // `Replicator` is local, so we cannot `impl ReplicationStatus for Replicator` directly.
    struct ReplStatus(Replicator);
    impl ReplicationStatus for ReplStatus {
        fn configured(&self) -> bool {
            self.0.durable_configured()
        }
        fn pending(&self) -> usize {
            self.0.pending()
        }
    }
    let repl_status: Arc<dyn ReplicationStatus> = Arc::new(ReplStatus(replicator.clone()));

    // RUM write path: vitals -> the existing metrics WAL, errors -> the existing logs WAL (no
    // new WAL/schema/compactor — see "Global Constraints" in the RUM plan). `photon-api` cannot
    // depend on `photon-wal`, so this concrete sink is defined here and handed in as
    // `Arc<dyn photon_api::RumSink>`.
    struct RumWalSink {
        metrics_wal: Arc<BroadcastingWal<DiskWal>>,
        logs_wal: Arc<BroadcastingWal<DiskWal>>,
        metric_schema: MetricSchema,
        log_schema: LogSchema,
        // Vitals count under the `metrics` signal, errors under `logs` — they're really metric
        // points / log records under the hood (see the module doc above), so folding them into
        // the existing signal counters is what makes RUM show up in `/api/usage/series` for free.
        counters: Arc<IngestCounters>,
    }

    #[async_trait::async_trait]
    impl photon_api::RumSink for RumWalSink {
        async fn ingest_vitals(
            &self,
            points: Vec<photon_core::metric_record::MetricPoint>,
        ) -> Result<(), photon_core::PhotonError> {
            let mut b = photon_core::metric_record::MetricBatchBuilder::with_capacity(
                &self.metric_schema,
                points.len(),
            );
            for p in &points {
                b.append(p);
            }
            let batch = b.finish()?;
            let rows = batch.num_rows() as u64;
            let bytes = batch.get_array_memory_size() as u64;
            self.metrics_wal.append(batch).await?;
            self.counters.metrics.add(rows, bytes);
            Ok(())
        }

        async fn ingest_errors(
            &self,
            records: Vec<photon_core::record::LogRecord>,
        ) -> Result<(), photon_core::PhotonError> {
            let mut b = photon_core::record::RecordBatchBuilder::with_capacity(
                &self.log_schema,
                records.len(),
            );
            for r in &records {
                b.append(r);
            }
            let batch = b.finish()?;
            let rows = batch.num_rows() as u64;
            let bytes = batch.get_array_memory_size() as u64;
            self.logs_wal.append(batch).await?;
            self.counters.logs.add(rows, bytes);
            Ok(())
        }
    }

    // Channel: the replicator's `on_durable(path, bytes)` callback -> the recorder task below.
    let (repl_tx, repl_rx) = tokio::sync::mpsc::channel::<(String, u64)>(1024);

    // Replication recorder: persist each successfully-replicated object's byte size, attributed to
    // its signal by object-path prefix. This tracks durable footprint in a table separate from the
    // manifest — the compactor stays the sole manifest writer.
    {
        let usage = usage.clone();
        let mut rx = repl_rx;
        tokio::spawn(async move {
            while let Some((path, bytes)) = rx.recv().await {
                if let Some(sig) = signal_from_path(&path) {
                    let _ = usage.record_replicated(&path, sig, bytes, now_ms()).await;
                }
            }
        });
    }

    // Seed the live retention atomics: SQLite override wins, else the config default. Uptime
    // uses its own `[uptime].retention_days` (defaulted when the section is omitted).
    let uptime_default = cfg.uptime.retention_days;
    let retention = Arc::new(RetentionAtomics {
        logs: AtomicU32::new(seed_retention(settings.as_ref(), "logs", cfg.retention.days).await),
        traces: AtomicU32::new(
            seed_retention(settings.as_ref(), "traces", cfg.retention.days).await,
        ),
        metrics: AtomicU32::new(
            seed_retention(settings.as_ref(), "metrics", cfg.retention.days).await,
        ),
        uptime: AtomicU32::new(seed_retention(settings.as_ref(), "uptime", uptime_default).await),
    });

    // Purge command channels (capacity 1): the API routes a manual purge to the owning compactor
    // — the sole manifest writer for its signal — and awaits the reply.
    let (tx_logs, rx_logs) = tokio::sync::mpsc::channel::<PurgeCommand>(1);
    let (tx_traces, rx_traces) = tokio::sync::mpsc::channel::<PurgeCommand>(1);
    let (tx_metrics, rx_metrics) = tokio::sync::mpsc::channel::<PurgeCommand>(1);

    // Background compactor: drains closed WAL segments (incl. those recovered on startup =
    // WAL replay) into Parquet, occasionally merges small files, applies hourly retention, then
    // flushes replication. Also serves on-demand purge commands from the API.
    if std::env::var("PHOTON_DISABLE_COMPACTION").is_err() {
        spawn_compactor(
            wal.clone(),
            storage.clone(),
            replicator.clone(),
            schema.clone(),
            rx_logs,
            retention.clone(),
            repl_tx.clone(),
        );
    } else {
        eprintln!(
            "PHOTON_DISABLE_COMPACTION set — logs compactor disabled; WAL will not be compacted"
        );
    }
    spawn_span_compactor(
        spans_wal.clone(),
        storage.clone(),
        replicator.clone(),
        span_schema.clone(),
        rx_traces,
        retention.clone(),
    );
    spawn_metric_compactor(
        metrics_wal.clone(),
        storage.clone(),
        replicator.clone(),
        metric_schema.clone(),
        rx_metrics,
        retention.clone(),
    );

    // Resolve bind addresses.
    let grpc_addr: SocketAddr = cfg
        .ingest
        .grpc_addr
        .parse()
        .map_err(|e| format!("invalid ingest.grpc_addr {:?}: {e}", cfg.ingest.grpc_addr))?;
    let http_addr: SocketAddr = cfg
        .ingest
        .http_addr
        .parse()
        .map_err(|e| format!("invalid ingest.http_addr {:?}: {e}", cfg.ingest.http_addr))?;
    let api_addr_str =
        std::env::var("PHOTON_API_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let api_addr: SocketAddr = api_addr_str
        .parse()
        .map_err(|e| format!("invalid PHOTON_API_ADDR {api_addr_str:?}: {e}"))?;

    let ingest = IngestServer::new(
        wal.clone(),
        spans_wal.clone(),
        metrics_wal.clone(),
        cfg.ingest.token.clone(),
        schema.clone(),
        span_schema.clone(),
        metric_schema.clone(),
        cfg.ingest.max_in_flight,
        counters.clone(),
    );
    let span_query = SpanQueryEngine::new(cfg.storage.hot_dir.clone(), span_schema.clone())?;
    let metrics_query =
        MetricsQueryEngine::new(cfg.storage.hot_dir.clone(), metric_schema.clone())?;

    // Background usage sampler: every 60s, snapshot each signal's footprint + ingest counters into
    // the usage store and prune old samples. The query engines derive `Clone` (and share their
    // manifest caches via `Arc`), so clone them here — the originals move into `ApiServer::new`.
    spawn_usage_sampler(
        usage.clone(),
        counters.clone(),
        query.clone(),
        span_query.clone(),
        metrics_query.clone(),
    );

    // The shared control-plane SQLite DB holds UI users (always) and uptime data (optional).
    let user_store: std::sync::Arc<dyn photon_api::users::UserStore> = std::sync::Arc::new(
        photon_api::users::SqliteUserStore::open(&cfg.storage.db_path)?,
    );

    // Uptime monitoring is always on: the scheduler + prune tasks run and the uptime tables are
    // opened in the shared control-plane SQLite. With no monitors configured it stays idle (no probes).
    let uptime_api = spawn_uptime(&cfg.uptime, &cfg.storage.db_path, retention.clone())?;

    // Data & retention admin handle: purge channels into the compactors, the live retention
    // atomics, and the persistence store. `uptime_enabled` gates the `uptime` retention key.
    let data_admin = DataAdmin {
        purge_logs: tx_logs,
        purge_traces: tx_traces,
        purge_metrics: tx_metrics,
        retention: retention.clone(),
        settings: settings.clone(),
        uptime_enabled: true,
        apdex_default_ms: cfg.apm.default_apdex_threshold_ms,
    };
    // Live-tail hub: fans the logs/spans `BroadcastingWal` senders out to `/api/stream/*`,
    // gated by a global connection semaphore sized from `[live].max_connections`.
    let live_hub = LiveHub::new(logs_tx, spans_tx, cfg.live.clone());

    // RUM subsystem: apps live in the control-plane SQLite DB (managed in the UI), not config.
    // Always enabled — an unregistered beacon is 403'd by the handler; the store starts empty.
    let rum_api = {
        let rum_store: Arc<dyn photon_api::rum_apps::RumAppStore> = Arc::new(
            photon_api::rum_apps::SqliteRumAppStore::open(&cfg.storage.db_path)?,
        );
        let sink: Arc<dyn photon_api::RumSink> = Arc::new(RumWalSink {
            metrics_wal: metrics_wal.clone(),
            logs_wal: wal.clone(),
            metric_schema: metric_schema.clone(),
            log_schema: schema.clone(),
            counters: counters.clone(),
        });
        Some(photon_api::RumApi::new(rum_store, sink).await)
    };

    let api = ApiServer::new(
        query,
        span_query,
        metrics_query,
        user_store,
        &cfg.auth.session_secret,
    )
    .with_uptime(Some(uptime_api))
    .with_data_admin(Some(data_admin))
    .with_live_hub(live_hub)
    .with_usage(usage.clone(), repl_status.clone())
    .with_rum(rum_api);

    println!("photon-server listening: otlp-grpc={grpc_addr} otlp-http={http_addr} api={api_addr}");

    // Run both front ends concurrently; the process lives until one of them exits.
    tokio::try_join!(ingest.serve(grpc_addr, http_addr), api.serve(api_addr))?;

    Ok(())
}

/// Print an argon2id PHC hash of `password` using an OS-random salt.
fn hash_password(password: &str) -> Result<String, Box<dyn std::error::Error>> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| format!("failed to hash password: {e}"))?
        .to_string();
    Ok(hash)
}

/// Container health probe: connect to the API port on loopback. Liveness = the API socket is
/// accepting connections. Parses the port out of `api_addr` (e.g. "0.0.0.0:8080") and dials
/// 127.0.0.1:<port> with a short timeout. No shell/curl needed — works on distroless.
fn healthcheck(api_addr: &str) -> bool {
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;

    let Some(port) = api_addr
        .rsplit(':')
        .next()
        .and_then(|p| p.parse::<u16>().ok())
    else {
        return false;
    };
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_ok()
}

/// The metrics schema always promotes host.name (the compactor sort key requires it), even if
/// the operator's promoted_attributes omits it. This keeps the metrics WAL, compactor, and
/// query engine schemas consistent without forcing every operator config to list it explicitly.
fn metrics_promoted(promoted: &[String]) -> Vec<String> {
    let mut v = promoted.to_vec();
    if !v.iter().any(|a| a == "host.name") {
        v.push("host.name".to_string());
    }
    v
}

/// Wall-clock time as Unix nanoseconds — the unit Parquet `timestamp` (and the purge cutoff) use.
fn now_nanos() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i64
}

/// Wall-clock time as Unix milliseconds — the unit the usage store and `/usage/series` use.
fn now_ms() -> i64 {
    now_nanos() / 1_000_000
}

/// Seed one retention atomic: prefer the SQLite-persisted override, else `default`. A store
/// error is treated as "no override" so a missing/empty settings row falls back to the config.
async fn seed_retention(settings: &dyn SettingsStore, signal: &str, default: u32) -> u32 {
    settings
        .get_retention(signal)
        .await
        .ok()
        .flatten()
        .unwrap_or(default)
}

/// Spawn the background compaction + replication loop.
///
/// The compactor and the periodic `spawn`-based replication flush share one [`Replicator`]
/// (its queue lives behind an `Arc<Mutex<_>>`, so clones enqueue into and drain from the same
/// queue). `Replicator::spawn` drains whatever is queued and then exits, so it is re-spawned
/// every tick.
fn spawn_compactor(
    wal: Arc<BroadcastingWal<DiskWal>>,
    storage: Storage,
    replicator: Replicator,
    schema: LogSchema,
    mut purge_rx: tokio::sync::mpsc::Receiver<PurgeCommand>,
    retention: Arc<RetentionAtomics>,
    repl_tx: tokio::sync::mpsc::Sender<(String, u64)>,
) {
    tokio::spawn(async move {
        let compactor = Compactor::new(wal, storage, Arc::new(replicator.clone()), schema);
        let mut tick: u64 = 0;
        loop {
            tokio::select! {
                _ = tokio::time::sleep(COMPACT_INTERVAL) => {
                    // Drain every closed segment currently available (this also processes segments
                    // recovered from a prior run on the first pass = WAL replay).
                    loop {
                        match compactor.run_once().await {
                            Ok(Some(_seg)) => continue,
                            Ok(None) => break,
                            Err(e) => {
                                eprintln!("compactor: run_once failed: {e}");
                                break;
                            }
                        }
                    }

                    tick = tick.wrapping_add(1);
                    if tick.is_multiple_of(MERGE_EVERY_TICKS) {
                        if let Err(e) = compactor.merge_once().await {
                            eprintln!("compactor: merge_once failed: {e}");
                        }
                    }

                    // Hourly retention: purge whole Parquet files older than the live cutoff.
                    if tick.is_multiple_of(PURGE_EVERY_TICKS) {
                        let days = retention.logs.load(std::sync::atomic::Ordering::Relaxed) as i64;
                        let cutoff = now_nanos() - days * 86_400_000_000_000;
                        if let Err(e) = compactor.purge_before(cutoff).await {
                            eprintln!("compactor: purge failed: {e}");
                        }
                    }

                    // Flush hot -> durable replication. `spawn` drains-then-exits, so re-spawn each
                    // tick; the handle is intentionally detached. Each successfully-replicated
                    // object's (path, byte size) is forwarded to the recorder task for the usage
                    // store (a full channel simply drops the sample — try_send never blocks).
                    let tx = repl_tx.clone();
                    replicator.clone().spawn(move |path, bytes| {
                        let _ = tx.try_send((path, bytes));
                    });
                }
                // On-demand purge from the API: run it and reply with the report.
                Some(cmd) = purge_rx.recv() => {
                    let res = compactor.purge_before(cmd.cutoff_nanos).await;
                    let _ = cmd.reply.send(res);
                }
            }
        }
    });
}

/// Spawn the background spans compaction loop (drains the spans WAL into `data-spans/` Parquet
/// and the spans manifest). Mirrors [`spawn_compactor`]; replication is flushed by the logs
/// loop's shared `Replicator`, so this loop only compacts + merges.
fn spawn_span_compactor(
    wal: Arc<BroadcastingWal<DiskWal>>,
    storage: Storage,
    replicator: Replicator,
    schema: SpanSchema,
    mut purge_rx: tokio::sync::mpsc::Receiver<PurgeCommand>,
    retention: Arc<RetentionAtomics>,
) {
    tokio::spawn(async move {
        let compactor = SpanCompactor::new(wal, storage, Arc::new(replicator), schema);
        let mut tick: u64 = 0;
        loop {
            tokio::select! {
                _ = tokio::time::sleep(COMPACT_INTERVAL) => {
                    loop {
                        match compactor.run_once().await {
                            Ok(Some(_seg)) => continue,
                            Ok(None) => break,
                            Err(e) => {
                                eprintln!("span-compactor: run_once failed: {e}");
                                break;
                            }
                        }
                    }
                    tick = tick.wrapping_add(1);
                    if tick.is_multiple_of(MERGE_EVERY_TICKS) {
                        if let Err(e) = compactor.merge_once().await {
                            eprintln!("span-compactor: merge_once failed: {e}");
                        }
                    }
                    if tick.is_multiple_of(PURGE_EVERY_TICKS) {
                        let days = retention.traces.load(std::sync::atomic::Ordering::Relaxed) as i64;
                        let cutoff = now_nanos() - days * 86_400_000_000_000;
                        if let Err(e) = compactor.purge_before(cutoff).await {
                            eprintln!("span-compactor: purge failed: {e}");
                        }
                    }
                }
                Some(cmd) = purge_rx.recv() => {
                    let res = compactor.purge_before(cmd.cutoff_nanos).await;
                    let _ = cmd.reply.send(res);
                }
            }
        }
    });
}

/// Spawn the background metrics compaction loop (drains the metrics WAL into `data-metrics/`
/// Parquet and the metrics manifest). Mirrors [`spawn_span_compactor`]; replication is flushed
/// by the logs loop's shared `Replicator`, so this loop only compacts + merges.
fn spawn_metric_compactor(
    wal: Arc<BroadcastingWal<DiskWal>>,
    storage: Storage,
    replicator: Replicator,
    schema: MetricSchema,
    mut purge_rx: tokio::sync::mpsc::Receiver<PurgeCommand>,
    retention: Arc<RetentionAtomics>,
) {
    tokio::spawn(async move {
        let compactor = MetricsCompactor::new(wal, storage, Arc::new(replicator), schema);
        let mut tick: u64 = 0;
        loop {
            tokio::select! {
                _ = tokio::time::sleep(COMPACT_INTERVAL) => {
                    loop {
                        match compactor.run_once().await {
                            Ok(Some(_seg)) => continue,
                            Ok(None) => break,
                            Err(e) => {
                                eprintln!("metric-compactor: run_once failed: {e}");
                                break;
                            }
                        }
                    }
                    tick = tick.wrapping_add(1);
                    if tick.is_multiple_of(MERGE_EVERY_TICKS) {
                        if let Err(e) = compactor.merge_once().await {
                            eprintln!("metric-compactor: merge_once failed: {e}");
                        }
                    }
                    if tick.is_multiple_of(PURGE_EVERY_TICKS) {
                        let days = retention.metrics.load(std::sync::atomic::Ordering::Relaxed) as i64;
                        let cutoff = now_nanos() - days * 86_400_000_000_000;
                        if let Err(e) = compactor.purge_before(cutoff).await {
                            eprintln!("metric-compactor: purge failed: {e}");
                        }
                    }
                }
                Some(cmd) = purge_rx.recv() => {
                    let res = compactor.purge_before(cmd.cutoff_nanos).await;
                    let _ = cmd.reply.send(res);
                }
            }
        }
    });
}

/// How often the usage sampler snapshots per-signal footprint + ingest counters.
const USAGE_SAMPLE_INTERVAL: Duration = Duration::from_secs(60);
/// Usage samples older than this window (30 days) are pruned each sampler tick.
const USAGE_RETENTION_MS: i64 = 30 * 86_400_000;

/// Spawn the background usage sampler. Every [`USAGE_SAMPLE_INTERVAL`] it snapshots each signal's
/// on-disk footprint (`storage_stats`), durable footprint, and cumulative ingest counters into the
/// usage store, then prunes samples older than [`USAGE_RETENTION_MS`].
///
/// Resilient by design (a crash here must not take down ingest): a `storage_stats` error skips just
/// that signal, and insert/prune errors are logged — the task never panics.
fn spawn_usage_sampler(
    usage: Arc<dyn UsageStore>,
    counters: Arc<IngestCounters>,
    query: QueryEngine,
    span_query: SpanQueryEngine,
    metrics_query: MetricsQueryEngine,
) {
    use photon_api::UsageSampleRow;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(USAGE_SAMPLE_INTERVAL);
        loop {
            tick.tick().await;
            let now = now_ms();
            let samples = [
                ("logs", query.storage_stats(), counters.logs.snapshot()),
                (
                    "traces",
                    span_query.storage_stats(),
                    counters.traces.snapshot(),
                ),
                (
                    "metrics",
                    metrics_query.storage_stats(),
                    counters.metrics.snapshot(),
                ),
            ];
            for (sig, stats, (irows, ibytes)) in samples {
                let Ok(s) = stats else { continue }; // never crash: skip this signal on error
                let durable = usage.durable_bytes(sig).await.unwrap_or(0);
                let row = UsageSampleRow {
                    ts_ms: now,
                    signal: sig.to_string(),
                    hot_bytes: Some(s.bytes),
                    durable_bytes: Some(durable),
                    total_rows: Some(s.total_rows),
                    file_count: Some(s.file_count),
                    ingest_rows: Some(irows),
                    ingest_bytes: Some(ibytes),
                };
                if let Err(e) = usage.insert_sample(&row).await {
                    eprintln!("usage sample {sig}: {e}");
                }
            }
            if let Err(e) = usage.prune_samples(now - USAGE_RETENTION_MS).await {
                eprintln!("usage prune: {e}");
            }
        }
    });
}

/// If `[uptime]` is configured, open the store, spawn the scheduler + an hourly retention
/// prune, and return the `UptimeApi` to hand to the API layer.
fn spawn_uptime(
    cfg: &photon_core::config::UptimeConfig,
    db_path: &str,
    retention: Arc<RetentionAtomics>,
) -> Result<photon_api::uptime::UptimeApi, Box<dyn std::error::Error>> {
    use photon_api::uptime::UptimeApi;
    use photon_uptime::model::SchedulerCommand;
    use photon_uptime::notify::WebhookNotifier;
    use photon_uptime::probe::NetworkProber;
    use photon_uptime::scheduler;
    use photon_uptime::store::sqlite::SqliteStore;
    use photon_uptime::store::UptimeStore;
    use tokio::sync::mpsc;

    let store = Arc::new(SqliteStore::open(db_path)?);
    let (cmd_tx, cmd_rx) = mpsc::channel::<SchedulerCommand>(256);
    let prober = Arc::new(NetworkProber::new());
    let notifier = Arc::new(WebhookNotifier::new(cfg.webhook_url.clone()));
    let concurrency = cfg.worker_concurrency;

    {
        let store = store.clone();
        tokio::spawn(async move {
            scheduler::run(store, prober, notifier, cmd_rx, concurrency).await;
        });
    }
    // Retention prune once an hour. Reads the live `retention.uptime` atomic (edited via the
    // Data & Retention API), pruning both heartbeats and resolved incidents older than the cutoff.
    {
        let store = store.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                tick.tick().await;
                let days = retention.uptime.load(std::sync::atomic::Ordering::Relaxed) as i64;
                let before = photon_uptime::model::now_ms() - days * 86_400_000;
                if let Err(e) = store.prune_heartbeats(before).await {
                    eprintln!("uptime prune: {e}");
                }
                if let Err(e) = store.prune_incidents(before).await {
                    eprintln!("uptime prune incidents: {e}");
                }
            }
        });
    }

    Ok(UptimeApi {
        store: store as Arc<dyn photon_uptime::store::UptimeStore>,
        cmd_tx,
        retention_days: cfg.retention_days,
    })
}

#[cfg(test)]
mod tests {
    use super::{healthcheck, metrics_promoted};
    use std::net::TcpListener;

    #[test]
    fn metrics_promoted_injects_host_name_when_absent() {
        let promoted = vec!["service.name".to_string()];
        assert_eq!(
            metrics_promoted(&promoted),
            vec!["service.name".to_string(), "host.name".to_string()]
        );
    }

    #[test]
    fn metrics_promoted_no_duplicate_when_already_present() {
        let promoted = vec!["service.name".to_string(), "host.name".to_string()];
        assert_eq!(metrics_promoted(&promoted), promoted);
    }

    #[test]
    fn metrics_promoted_preserves_order_and_other_attributes() {
        let promoted = vec![
            "service.name".to_string(),
            "deployment.environment".to_string(),
        ];
        assert_eq!(
            metrics_promoted(&promoted),
            vec![
                "service.name".to_string(),
                "deployment.environment".to_string(),
                "host.name".to_string(),
            ]
        );
    }

    #[test]
    fn metrics_promoted_preserves_existing_host_name_position() {
        let promoted = vec![
            "service.name".to_string(),
            "host.name".to_string(),
            "deployment.environment".to_string(),
        ];
        assert_eq!(metrics_promoted(&promoted), promoted);
    }

    #[test]
    fn healthcheck_true_when_port_open() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // Any host form is fine; healthcheck connects to loopback on the parsed port.
        assert!(healthcheck(&format!("0.0.0.0:{port}")));
    }

    #[test]
    fn healthcheck_false_when_port_closed() {
        // Bind then drop to obtain a very-likely-free port.
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        assert!(!healthcheck(&format!("127.0.0.1:{port}")));
    }

    #[test]
    fn healthcheck_false_on_garbage_addr() {
        assert!(!healthcheck("not-an-address"));
    }
}
