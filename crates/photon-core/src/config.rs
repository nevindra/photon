use crate::metric_schema;
use crate::schema;
use crate::span_schema;
use crate::PhotonError;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub ingest: IngestConfig,
    pub storage: StorageConfig,
    pub retention: RetentionConfig,
    pub schema: SchemaConfig,
    pub wal: WalConfig,
    pub auth: AuthConfig,
    #[serde(default)]
    pub uptime: UptimeConfig,
    #[serde(default)]
    pub apm: ApmConfig,
    #[serde(default)]
    pub live: LiveConfig,
    #[serde(default)]
    pub alerts: AlertsConfig,
    #[serde(default)]
    pub rollup: RollupConfig,
    #[serde(default)]
    pub query: QueryConfig,
    #[serde(default)]
    pub federation: Option<FederationConfig>,
    /// `[deploy]` — Docker fleet management (managed agents, stacks, templates, rollouts).
    #[serde(default)]
    pub deploy: DeployConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IngestConfig {
    pub token: String,
    pub http_addr: String,
    pub grpc_addr: String,
    /// WS4 backpressure: max concurrently in-flight logs ingest requests (HTTP + gRPC
    /// combined) before excess requests wait for a permit instead of piling decoded batches
    /// on the heap. Peak in-flight decoded-batch memory is bounded by roughly
    /// `max_in_flight * max decoded batch size`; default 256 is what B1 sized the fix against
    /// (an unbounded conc=128 saturate run OOM-killed the server before this bound existed).
    #[serde(default = "IngestConfig::default_max_in_flight")]
    pub max_in_flight: usize,
    /// Max request body size, in bytes, accepted by the ingest front doors. Enforced on the
    /// **decompressed** stream: HTTP applies it as `DefaultBodyLimit::max` sitting *inside* the
    /// gzip request-decompression layer (so a small gzip bomb can't blow past it), and the gRPC
    /// side mirrors it via `max_decoding_message_size` so the two front doors agree instead of
    /// silently disagreeing (axum defaulted to 2 MiB, tonic to 4 MiB — an exporter retries a 413
    /// forever). The Prometheus remote-write receiver reuses it as a snappy decompress cap.
    /// Default ~16 MiB. `PHOTON_INGEST_MAX_BODY_BYTES` overrides.
    #[serde(default = "IngestConfig::default_max_body_bytes")]
    pub max_body_bytes: usize,
    /// Cap on the byte length of ONE long-tail attribute value, applied while OTLP is mapped
    /// into Arrow (see [`crate::attr_limit`]). `0` disables it.
    ///
    /// It is enforced at the front door on purpose: an oversized value cut here never reaches
    /// the WAL, so it costs nothing to `fsync`, compact, replicate, or decode later. Measured on
    /// a production span corpus, capping every value cost the file 0.2% at 8 KiB, 2.9% at 4 KiB,
    /// **10.8% at the 2 KiB default**, 20.6% at 1 KiB and 30.8% at 512 B — nearly all of it one
    /// key, `db.query.text`, averaging 1,618 bytes on 16% of spans.
    ///
    /// **Truncation is not reversible.** Raise it if whole SQL statements matter more than the
    /// bytes; set `0` to keep the old unlimited behavior. Promoted attributes are never
    /// truncated — the read path groups on them.
    /// `PHOTON_INGEST_MAX_ATTRIBUTE_VALUE_BYTES` overrides.
    #[serde(default = "IngestConfig::default_max_attribute_value_bytes")]
    pub max_attribute_value_bytes: usize,
}

impl IngestConfig {
    fn default_max_in_flight() -> usize {
        256
    }
    fn default_max_body_bytes() -> usize {
        16 * 1024 * 1024
    }
    fn default_max_attribute_value_bytes() -> usize {
        2048
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    pub hot_dir: PathBuf,
    /// Path to the shared control-plane SQLite database (UI users + uptime monitors).
    /// Always present — shared by auth and the always-on uptime subsystem.
    #[serde(default = "StorageConfig::default_db_path")]
    pub db_path: String,
    #[serde(default)]
    pub durable: Option<DurableConfig>,
    /// zstd compression level the compactors use when streaming Parquet files to disk. Range
    /// `1..=19` (validated below).
    ///
    /// **Default 6, and the level costs the read path nothing.** Decoding every column of the
    /// same production corpus took 88 ms at level 1, 88 ms at level 6 and 86 ms at level 12:
    /// zstd's decompression speed barely moves with the level, so what a higher level buys in
    /// size it charges to compaction CPU, not to queries. Measured against level 1 on merged
    /// files: logs −1.6%, spans −5.5%, metrics −12.5%.
    ///
    /// Level 3 is the one to skip — on this corpus it lands within 0.2% of level 1 while costing
    /// more, which is why the ladder here jumps straight to 6.
    #[serde(default = "StorageConfig::default_zstd_level")]
    pub zstd_level: i32,
}

impl StorageConfig {
    fn default_db_path() -> String {
        "./data/photon.db".to_string()
    }

    fn default_zstd_level() -> i32 {
        6
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DurableConfig {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    /// S3 credentials. Optional: some on-prem stores (or IAM/env-based setups) don't need
    /// them inline. When present they are passed to the S3 client.
    #[serde(default)]
    pub access_key_id: Option<String>,
    #[serde(default)]
    pub secret_access_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RetentionConfig {
    pub days: u32,
}

/// Upper bound on any per-signal retention `days` (100 years). `days` feeds directly into
/// `days * 86_400_000_000_000` nanosecond cutoff arithmetic in the retention purge loops;
/// anything above ~106_751 days overflows `i64`, so this cap keeps every accepted value far
/// inside the safe range (the loops additionally use saturating arithmetic as defense in depth).
pub const MAX_RETENTION_DAYS: u32 = 36_500;

#[derive(Debug, Clone, Deserialize)]
pub struct SchemaConfig {
    pub promoted_attributes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WalConfig {
    pub segment_max_bytes: u64,
    pub segment_max_age_secs: u64,
    pub group_commit_max_delay_ms: u64,
}

/// Human (UI) authentication. Distinct from the ingest service token. Users themselves live in
/// the `[storage].db_path` SQLite database, not in config.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    /// Secret used to sign session cookies. Must be non-empty and long enough to be secure.
    pub session_secret: String,
}

/// `[uptime]` — optional *tuning* for the always-on uptime subsystem. The subsystem always
/// runs; omitting the section just accepts the defaults below.
#[derive(Debug, Clone, Deserialize)]
pub struct UptimeConfig {
    #[serde(default = "UptimeConfig::default_retention_days")]
    pub retention_days: u32,
    #[serde(default)]
    pub webhook_url: Option<String>,
    #[serde(default = "UptimeConfig::default_interval")]
    pub default_interval: String,
    #[serde(default = "UptimeConfig::default_timeout")]
    pub default_timeout: String,
    #[serde(default = "UptimeConfig::default_worker_concurrency")]
    pub worker_concurrency: usize,
}

impl UptimeConfig {
    fn default_retention_days() -> u32 {
        30
    }
    fn default_interval() -> String {
        "60s".into()
    }
    fn default_timeout() -> String {
        "10s".into()
    }
    fn default_worker_concurrency() -> usize {
        32
    }
}

impl Default for UptimeConfig {
    fn default() -> Self {
        Self {
            retention_days: Self::default_retention_days(),
            webhook_url: None,
            default_interval: Self::default_interval(),
            default_timeout: Self::default_timeout(),
            worker_concurrency: Self::default_worker_concurrency(),
        }
    }
}

/// Default Apdex satisfied-threshold T (milliseconds) when a service has no per-service
/// override. tolerating = T..4T, frustrated = >4T.
pub const DEFAULT_APDEX_THRESHOLD_MS: u32 = 500;

/// `[apm]` — optional. Holds the global default Apdex threshold used by the Services (APM)
/// view for services without a per-service override.
#[derive(Debug, Clone, Deserialize)]
pub struct ApmConfig {
    #[serde(default = "ApmConfig::default_apdex_threshold_ms")]
    pub default_apdex_threshold_ms: u32,
}

impl ApmConfig {
    fn default_apdex_threshold_ms() -> u32 {
        DEFAULT_APDEX_THRESHOLD_MS
    }
}

impl Default for ApmConfig {
    fn default() -> Self {
        Self {
            default_apdex_threshold_ms: DEFAULT_APDEX_THRESHOLD_MS,
        }
    }
}

/// Live-tail streaming knobs (SSE). All optional; defaults make it work out of the box.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct LiveConfig {
    /// Per-signal broadcast channel depth before a slow subscriber sees `Lagged`. Each slot pins
    /// one ingested batch (`Arc<RecordBatch>`) in memory until it's overwritten by a newer send,
    /// and only while at least one SSE client is connected — so this is a direct memory/laggy-
    /// subscriber trade-off, not just a buffering knob. A lagging subscriber that falls behind
    /// past capacity gets `Lagged` and skips the gap, which live tail tolerates by design.
    pub broadcast_capacity: usize,
    /// Coalescing flush cadence for the SSE stream, milliseconds.
    pub flush_interval_ms: u64,
    /// Max rows emitted per flush; excess in the window is dropped (newest kept) and reported via `rate`.
    pub max_rows_per_flush: usize,
    /// Max concurrent SSE connections across both endpoints; excess gets 503.
    pub max_connections: usize,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            broadcast_capacity: 128,
            flush_interval_ms: 250,
            max_rows_per_flush: 200,
            max_connections: 32,
        }
    }
}

/// `[alerts]` — optional *tuning* for the system-wide webhook alert engine. The subsystem
/// always runs; omitting the section just accepts the defaults below.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlertsConfig {
    /// Default per-rule evaluation cadence when a rule doesn't override it.
    #[serde(default = "AlertsConfig::default_interval")]
    pub interval_default: String, // e.g. "60s"
    /// Max concurrent rule evaluations in flight.
    #[serde(default = "AlertsConfig::default_worker_concurrency")]
    pub worker_concurrency: usize,
}
impl AlertsConfig {
    fn default_interval() -> String {
        "60s".into()
    }
    fn default_worker_concurrency() -> usize {
        16
    }
}
/// `[deploy]` — Docker fleet management. Always on; the section tunes where the secret key and
/// the agent binaries live and which origin agents should dial.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeployConfig {
    /// Set `false` to mount none of the `/api/agents|stacks|templates|rollouts|audit` routes.
    #[serde(default = "DeployConfig::default_enabled")]
    pub enabled: bool,
    /// AES-256 key file for secret variables at rest (R8.5). Generated on first start when
    /// absent. Default: `deploy.key` beside `[storage].db_path`.
    #[serde(default)]
    pub secret_key_file: Option<String>,
    /// Directory of `photon-agent-<arch>` binaries served by `GET /api/agent/download` and pushed
    /// by the SSH bootstrap. Default: `agents/` beside `[storage].db_path`. A `photon-agent`
    /// beside the `photon-server` binary is also found for the server's own architecture.
    #[serde(default)]
    pub agent_binaries_dir: Option<String>,
    /// The URL agents dial (this Photon as the customer LAN sees it), e.g.
    /// `https://photon.example`. Default: derived from each request's Host header.
    #[serde(default)]
    pub server_origin: Option<String>,
    /// The OTLP/HTTP origin the sync response hands agents for their metrics, e.g.
    /// `http://photon.example:4318`. Default: `server_origin` with the port of `[ingest].http_addr`.
    #[serde(default)]
    pub ingest_origin: Option<String>,
    /// Enable the SSH bootstrap installer (`POST /api/agents/bootstrap`).
    #[serde(default = "DeployConfig::default_enabled")]
    pub ssh_bootstrap: bool,
}

impl DeployConfig {
    fn default_enabled() -> bool {
        true
    }
}

impl Default for DeployConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            secret_key_file: None,
            agent_binaries_dir: None,
            server_origin: None,
            ingest_origin: None,
            ssh_bootstrap: true,
        }
    }
}

impl Default for AlertsConfig {
    fn default() -> Self {
        Self {
            interval_default: Self::default_interval(),
            worker_concurrency: Self::default_worker_concurrency(),
        }
    }
}

/// `[rollup]` — optional, always on. The background pass that publishes `photon.service.*`
/// metrics from spans (`docs/adr/0002-red-rollups-are-published-as-metrics.md`), so the Backend
/// pages can read a wide window without re-scanning raw spans.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollupConfig {
    /// How often a pass runs. Clamped to a 15s floor: a pass costs three DataFusion aggregates
    /// over the lag window, and running them faster than that buys nothing a 60s bucket can show.
    #[serde(default = "RollupConfig::default_interval_secs")]
    pub interval_secs: u64,
    /// How much recent time each pass recomputes and re-emits before considering it settled.
    /// Spans arriving later than this never reach the rollup — the rollup's one documented
    /// inaccuracy (see `docs/subsystems/rollup.md`).
    #[serde(default = "RollupConfig::default_lag_secs")]
    pub lag_secs: u64,
    /// Operation-level series kept per `(service, environment)`; the rest fold into `__other__`.
    /// `0` disables the operation level and keeps only per-service series.
    #[serde(default = "RollupConfig::default_top_operations")]
    pub top_operations: usize,
    /// Set false to stop publishing entirely. The Backend read paths then always read raw spans,
    /// which is correct but slower over wide windows.
    #[serde(default = "RollupConfig::default_enabled")]
    pub enabled: bool,
    /// How far back a first run reaches to publish buckets for spans that are already on disk.
    /// `0` disables the backfill and the rollup only ever covers time from its first start.
    ///
    /// Without this the rollup could only describe the future. Every install with existing data —
    /// a fresh control-plane DB over a restored hot tier, an upgrade from a build without the
    /// rollup, a node re-seeded from a replica — reported no coverage for its own history, so
    /// every Backend page over a window wider than "since we started" silently fell back to
    /// scanning raw spans. That fallback is correct and it is slow in exactly the way the rollup
    /// exists to prevent: on 300M spans, `/api/services/overview` took 23.1 s over 7 days and
    /// 3.4 s over 24 hours, against 63 ms off the rollup.
    ///
    /// The catch-up walks forward one [`RollupConfig::CATCH_UP_SLICE_SECS`] slice at a time,
    /// writing the watermark after each, so it is resumable and a restart mid-catch-up does not
    /// restart it. It is bounded by what is on disk: slices whose spans have aged out of retention
    /// prune to nothing and cost a manifest lookup.
    #[serde(default = "RollupConfig::default_backfill_days")]
    pub backfill_days: u64,
}
impl RollupConfig {
    /// One pass per bucket. Anything faster re-derives the same numbers.
    pub const MIN_INTERVAL_SECS: u64 = 15;
    /// The rollup's bucket width, in seconds. NOT configurable: it is baked into every emitted
    /// timestamp, and two nodes disagreeing about it would make a federated series unreadable.
    pub const BUCKET_SECS: u64 = 60;

    /// The widest span of time one pass rolls up. A pass reads raw spans, so an uncapped one
    /// after a long outage (or on a first run with a backfill horizon) would put a single
    /// multi-day span scan on the read path and write one enormous metrics batch. Capping it
    /// costs more passes, which is what passes are for.
    pub const CATCH_UP_SLICE_SECS: i64 = 3600;

    fn default_interval_secs() -> u64 {
        60
    }
    /// A month, matching the default `[retention].days`: the rollup can then answer every window
    /// whose data this node still holds.
    ///
    /// Deliberately NOT "the widest window the UI offers". Coverage starts at `now - backfill_days`
    /// and the widest built-in window ENDS at now, so sizing the two the same puts every 7-day
    /// query exactly on the boundary — a query starting a minute earlier than coverage is
    /// ineligible and silently falls back to a raw span scan. The backfill must reach further back
    /// than the windows it is meant to serve. Slices with no data left prune to nothing and cost a
    /// manifest lookup, so over-reaching is cheap and under-reaching is not.
    fn default_backfill_days() -> u64 {
        30
    }
    fn default_lag_secs() -> u64 {
        600
    }
    fn default_top_operations() -> usize {
        50
    }
    fn default_enabled() -> bool {
        true
    }

    /// `interval_secs`, floored at [`Self::MIN_INTERVAL_SECS`].
    pub fn interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.interval_secs.max(Self::MIN_INTERVAL_SECS))
    }

    /// `lag_secs`, floored at one whole bucket — a lag shorter than a bucket would leave the
    /// newest bucket permanently unwritten.
    pub fn lag_secs(&self) -> u64 {
        self.lag_secs.max(Self::BUCKET_SECS)
    }
}
impl Default for RollupConfig {
    fn default() -> Self {
        Self {
            interval_secs: Self::default_interval_secs(),
            lag_secs: Self::default_lag_secs(),
            top_operations: Self::default_top_operations(),
            enabled: Self::default_enabled(),
            backfill_days: Self::default_backfill_days(),
        }
    }
}

/// `[query]` — optional. Tunes the read path's one global resource: the DataFusion memory pool
/// every query engine shares (`photon_query::session`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryConfig {
    /// Ceiling, in bytes, on the memory ALL concurrent queries may hold at once — not a per-query
    /// allowance. Omit (the default) to size it from the machine: a quarter of total RAM, clamped
    /// to [`QueryConfig::MIN_MEMORY_POOL_BYTES`]..=[`QueryConfig::MAX_MEMORY_POOL_BYTES`], falling
    /// back to the floor when total RAM cannot be read.
    ///
    /// It used to be a hard-coded 512 MiB regardless of the host, which is the wrong shape for a
    /// number that has to cover a whole machine's read path: it was generous on a 2 GB VM and
    /// absurd on a 30 GB server, where a 7-day histogram query failed with "Resources exhausted"
    /// while the box had 21 GB free.
    ///
    /// Raising this does not make an unbounded query safe — spillable operators (sorts, grouped
    /// aggregations) degrade to disk at their fair share of the pool, and the pool stays the loud
    /// backstop for the unspillable ones. It sets how much work fits in RAM before either happens.
    #[serde(default)]
    pub memory_pool_bytes: Option<u64>,
}

impl QueryConfig {
    /// Never size the pool below this. Under ~256 MiB, ordinary dashboard fan-out spills on every
    /// query and the read path is slow for no benefit.
    pub const MIN_MEMORY_POOL_BYTES: u64 = 256 * 1024 * 1024;
    /// Never size the pool above this without being told to. Photon shares the box with whatever
    /// it is monitoring; taking 16 GB on a 64 GB host because we can is not a default.
    pub const MAX_MEMORY_POOL_BYTES: u64 = 4 * 1024 * 1024 * 1024;
    /// Fraction of total RAM the auto-sized pool takes.
    pub const MEMORY_POOL_RAM_DIVISOR: u64 = 4;

    /// The pool size to use, given the machine's total RAM (`None` when it cannot be read).
    /// An explicit `memory_pool_bytes` wins outright — including a deliberately small one, which
    /// is how you pin a low-memory node — otherwise auto-size and clamp.
    pub fn memory_pool_bytes(&self, total_ram_bytes: Option<u64>) -> u64 {
        if let Some(explicit) = self.memory_pool_bytes {
            return explicit.max(1);
        }
        match total_ram_bytes {
            Some(total) => (total / Self::MEMORY_POOL_RAM_DIVISOR)
                .clamp(Self::MIN_MEMORY_POOL_BYTES, Self::MAX_MEMORY_POOL_BYTES),
            None => Self::MIN_MEMORY_POOL_BYTES,
        }
    }
}

/// `[federation]` — optional. Absent by default; presence turns this node into a tenant that
/// pushes to a central Photon (`summary` mode: synthetic health metrics; `full`: best-effort
/// OTLP tee) over the given `endpoint`/`token`.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationConfig {
    pub endpoint: String,
    pub token: String,
    #[serde(default)]
    pub mode: FederationMode,
    /// Which signals full mode mirrors. `None` → all four. Only valid with `mode = "full"` —
    /// summary mirrors nothing, so `signals` under summary is a config error, not a silent no-op.
    #[serde(default)]
    pub signals: Option<Vec<FederationSignal>>,
    #[serde(default = "FederationConfig::default_interval_secs")]
    pub interval_secs: u64,
    #[serde(default = "FederationConfig::default_queue_batches")]
    pub queue_batches: usize,
    /// Ship this Site's `photon.service.*` / `photon.operation.*` rollup to central.
    ///
    /// On by default, and it applies in BOTH modes. A `summary`-mode Site otherwise tells central
    /// almost nothing about its services, which would make the cross-Site Services index work only
    /// for Sites that can afford `full` mode. And `full` mode does not cover it either: the tee
    /// mirrors accepted OTLP batches, but the rollup writes straight to the local metrics WAL and
    /// never passes through OTLP ingest — so without this flag its points reach central in neither
    /// mode. Central stamps `tenant` on arrival exactly as it does for every other metric.
    #[serde(default = "FederationConfig::default_push_service_rollup")]
    pub push_service_rollup: bool,
}

impl FederationConfig {
    fn default_interval_secs() -> u64 {
        30
    }
    fn default_queue_batches() -> usize {
        1024
    }
    fn default_push_service_rollup() -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FederationSignal {
    Logs,
    Traces,
    Metrics,
    Rum,
}

impl FederationSignal {
    pub const ALL: [FederationSignal; 4] = [
        FederationSignal::Logs,
        FederationSignal::Traces,
        FederationSignal::Metrics,
        FederationSignal::Rum,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            FederationSignal::Logs => "logs",
            FederationSignal::Traces => "traces",
            FederationSignal::Metrics => "metrics",
            FederationSignal::Rum => "rum",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FederationMode {
    #[default]
    Summary,
    Full,
}

fn parse_bool_env(name: &str, v: &str) -> Result<bool, PhotonError> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(PhotonError::Config(format!(
            "{name}: expected true/false, got '{other}'"
        ))),
    }
}

impl Config {
    /// Parse TOML into a `Config` **without** running semantic validation. Used by the
    /// layered `load` path, which parses a base document, overlays env vars, and validates
    /// once at the end. Prefer `from_toml_str` for a one-shot parse+validate.
    pub fn parse_unvalidated(s: &str) -> Result<Config, PhotonError> {
        toml::from_str(s).map_err(|e| PhotonError::Config(e.to_string()))
    }

    /// Overlay `PHOTON_*` environment variables onto this config, in place. `get` is an
    /// injected lookup (real env in production, a fixed map in tests) so this stays pure and
    /// table-testable. Numeric/parse failures return `PhotonError::Config` naming the variable.
    /// `[storage.durable]` is (re)constructed iff `PHOTON_DURABLE_ENDPOINT` is present.
    pub fn apply_env_overrides(
        &mut self,
        get: impl Fn(&str) -> Option<String>,
    ) -> Result<(), PhotonError> {
        fn parse_var<T: std::str::FromStr>(name: &str, val: &str) -> Result<T, PhotonError> {
            val.trim()
                .parse::<T>()
                .map_err(|_| PhotonError::Config(format!("invalid value for {name}: {val:?}")))
        }

        if let Some(v) = get("PHOTON_INGEST_TOKEN") {
            self.ingest.token = v;
        }
        if let Some(v) = get("PHOTON_INGEST_HTTP_ADDR") {
            self.ingest.http_addr = v;
        }
        if let Some(v) = get("PHOTON_INGEST_GRPC_ADDR") {
            self.ingest.grpc_addr = v;
        }
        if let Some(v) = get("PHOTON_INGEST_MAX_IN_FLIGHT") {
            self.ingest.max_in_flight = parse_var("PHOTON_INGEST_MAX_IN_FLIGHT", &v)?;
        }
        if let Some(v) = get("PHOTON_INGEST_MAX_BODY_BYTES") {
            self.ingest.max_body_bytes = parse_var("PHOTON_INGEST_MAX_BODY_BYTES", &v)?;
        }
        if let Some(v) = get("PHOTON_STORAGE_HOT_DIR") {
            self.storage.hot_dir = PathBuf::from(v);
        }
        if let Some(v) = get("PHOTON_STORAGE_DB_PATH") {
            self.storage.db_path = v;
        }
        if let Some(v) = get("PHOTON_RETENTION_DAYS") {
            self.retention.days = parse_var("PHOTON_RETENTION_DAYS", &v)?;
        }
        if let Some(v) = get("PHOTON_PROMOTED_ATTRIBUTES") {
            self.schema.promoted_attributes = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        if let Some(v) = get("PHOTON_INGEST_MAX_ATTRIBUTE_VALUE_BYTES") {
            self.ingest.max_attribute_value_bytes =
                parse_var("PHOTON_INGEST_MAX_ATTRIBUTE_VALUE_BYTES", &v)?;
        }
        if let Some(v) = get("PHOTON_DEPLOY_ENABLED") {
            self.deploy.enabled = parse_bool_env("PHOTON_DEPLOY_ENABLED", &v)?;
        }
        if let Some(v) = get("PHOTON_DEPLOY_SERVER_ORIGIN") {
            self.deploy.server_origin = Some(v).filter(|s| !s.trim().is_empty());
        }
        if let Some(v) = get("PHOTON_DEPLOY_INGEST_ORIGIN") {
            self.deploy.ingest_origin = Some(v).filter(|s| !s.trim().is_empty());
        }
        if let Some(v) = get("PHOTON_DEPLOY_SECRET_KEY_FILE") {
            self.deploy.secret_key_file = Some(v).filter(|s| !s.trim().is_empty());
        }
        if let Some(v) = get("PHOTON_DEPLOY_AGENT_BINARIES_DIR") {
            self.deploy.agent_binaries_dir = Some(v).filter(|s| !s.trim().is_empty());
        }
        if let Some(v) = get("PHOTON_DEPLOY_SSH_BOOTSTRAP") {
            self.deploy.ssh_bootstrap = parse_bool_env("PHOTON_DEPLOY_SSH_BOOTSTRAP", &v)?;
        }
        if let Some(v) = get("PHOTON_SESSION_SECRET") {
            self.auth.session_secret = v;
        }
        if let Some(v) = get("PHOTON_WAL_SEGMENT_MAX_BYTES") {
            self.wal.segment_max_bytes = parse_var("PHOTON_WAL_SEGMENT_MAX_BYTES", &v)?;
        }
        if let Some(v) = get("PHOTON_WAL_SEGMENT_MAX_AGE_SECS") {
            self.wal.segment_max_age_secs = parse_var("PHOTON_WAL_SEGMENT_MAX_AGE_SECS", &v)?;
        }
        if let Some(v) = get("PHOTON_WAL_GROUP_COMMIT_MAX_DELAY_MS") {
            self.wal.group_commit_max_delay_ms =
                parse_var("PHOTON_WAL_GROUP_COMMIT_MAX_DELAY_MS", &v)?;
        }
        if let Some(v) = get("PHOTON_APM_DEFAULT_APDEX_THRESHOLD_MS") {
            self.apm.default_apdex_threshold_ms =
                parse_var("PHOTON_APM_DEFAULT_APDEX_THRESHOLD_MS", &v)?;
        }
        if let Some(v) = get("PHOTON_ROLLUP_ENABLED") {
            self.rollup.enabled = parse_var("PHOTON_ROLLUP_ENABLED", &v)?;
        }
        if let Some(v) = get("PHOTON_ROLLUP_INTERVAL_SECS") {
            self.rollup.interval_secs = parse_var("PHOTON_ROLLUP_INTERVAL_SECS", &v)?;
        }
        if let Some(v) = get("PHOTON_ROLLUP_LAG_SECS") {
            self.rollup.lag_secs = parse_var("PHOTON_ROLLUP_LAG_SECS", &v)?;
        }
        if let Some(v) = get("PHOTON_ROLLUP_TOP_OPERATIONS") {
            self.rollup.top_operations = parse_var("PHOTON_ROLLUP_TOP_OPERATIONS", &v)?;
        }
        if let Some(v) = get("PHOTON_ROLLUP_BACKFILL_DAYS") {
            self.rollup.backfill_days = parse_var("PHOTON_ROLLUP_BACKFILL_DAYS", &v)?;
        }
        if let Some(v) = get("PHOTON_QUERY_MEMORY_POOL_BYTES") {
            self.query.memory_pool_bytes = Some(parse_var("PHOTON_QUERY_MEMORY_POOL_BYTES", &v)?);
        }
        // Durable tier: presence of the endpoint is the toggle. bucket/region are then
        // required (env, or inherited from a base file's [storage.durable]); keys optional.
        if let Some(endpoint) = get("PHOTON_DURABLE_ENDPOINT").filter(|e| !e.trim().is_empty()) {
            let existing = self.storage.durable.as_ref();
            let bucket = get("PHOTON_DURABLE_BUCKET")
                .or_else(|| existing.map(|d| d.bucket.clone()))
                .ok_or_else(|| {
                    PhotonError::Config(
                        "PHOTON_DURABLE_BUCKET is required when PHOTON_DURABLE_ENDPOINT is set"
                            .into(),
                    )
                })?;
            let region = get("PHOTON_DURABLE_REGION")
                .or_else(|| existing.map(|d| d.region.clone()))
                .ok_or_else(|| {
                    PhotonError::Config(
                        "PHOTON_DURABLE_REGION is required when PHOTON_DURABLE_ENDPOINT is set"
                            .into(),
                    )
                })?;
            let access_key_id = get("PHOTON_DURABLE_ACCESS_KEY_ID")
                .or_else(|| existing.and_then(|d| d.access_key_id.clone()));
            let secret_access_key = get("PHOTON_DURABLE_SECRET_ACCESS_KEY")
                .or_else(|| existing.and_then(|d| d.secret_access_key.clone()));
            self.storage.durable = Some(DurableConfig {
                endpoint,
                bucket,
                region,
                access_key_id,
                secret_access_key,
            });
        }
        // Federation: presence of the endpoint is the toggle, same pattern as
        // `[storage.durable]` above. token is then required (env, or inherited from a base
        // file's `[federation]`); mode/interval/queue_batches are optional overrides.
        if let Some(endpoint) = get("PHOTON_FEDERATION_ENDPOINT").filter(|e| !e.trim().is_empty()) {
            let existing = self.federation.as_ref();
            let token = get("PHOTON_FEDERATION_TOKEN")
                .or_else(|| existing.map(|f| f.token.clone()))
                .ok_or_else(|| {
                    PhotonError::Config(
                        "PHOTON_FEDERATION_TOKEN is required when PHOTON_FEDERATION_ENDPOINT is set"
                            .into(),
                    )
                })?;
            let mode = match get("PHOTON_FEDERATION_MODE") {
                Some(m) => match m.trim() {
                    "summary" => FederationMode::Summary,
                    "full" => FederationMode::Full,
                    other => {
                        return Err(PhotonError::Config(format!(
                            "invalid value for PHOTON_FEDERATION_MODE: {other:?}"
                        )))
                    }
                },
                None => existing.map(|f| f.mode).unwrap_or_default(),
            };
            let interval_secs = match get("PHOTON_FEDERATION_INTERVAL_SECS") {
                Some(v) => parse_var("PHOTON_FEDERATION_INTERVAL_SECS", &v)?,
                None => existing
                    .map(|f| f.interval_secs)
                    .unwrap_or_else(FederationConfig::default_interval_secs),
            };
            let queue_batches = match get("PHOTON_FEDERATION_QUEUE_BATCHES") {
                Some(v) => parse_var("PHOTON_FEDERATION_QUEUE_BATCHES", &v)?,
                None => existing
                    .map(|f| f.queue_batches)
                    .unwrap_or_else(FederationConfig::default_queue_batches),
            };
            // Comma-separated, e.g. `PHOTON_FEDERATION_SIGNALS=traces,metrics`. Validity
            // (mode = full, non-empty, no dups) is checked in validate() with the file path.
            let signals = match get("PHOTON_FEDERATION_SIGNALS") {
                Some(v) => Some(
                    v.split(',')
                        .map(|s| match s.trim() {
                            "logs" => Ok(FederationSignal::Logs),
                            "traces" => Ok(FederationSignal::Traces),
                            "metrics" => Ok(FederationSignal::Metrics),
                            "rum" => Ok(FederationSignal::Rum),
                            other => Err(PhotonError::Config(format!(
                                "invalid value in PHOTON_FEDERATION_SIGNALS: {other:?}"
                            ))),
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                None => existing.and_then(|f| f.signals.clone()),
            };
            let push_service_rollup = match get("PHOTON_FEDERATION_PUSH_SERVICE_ROLLUP") {
                Some(v) => parse_var("PHOTON_FEDERATION_PUSH_SERVICE_ROLLUP", &v)?,
                None => existing
                    .map(|f| f.push_service_rollup)
                    .unwrap_or_else(FederationConfig::default_push_service_rollup),
            };
            self.federation = Some(FederationConfig {
                endpoint,
                token,
                mode,
                signals,
                interval_secs,
                queue_batches,
                push_service_rollup,
            });
        }

        Ok(())
    }

    /// Load the effective config: choose a base document, overlay `PHOTON_*` env vars, validate.
    ///
    /// Base selection:
    /// * `explicit` = `Some(path)` (from `argv[1]` or `$PHOTON_CONFIG`) → the file **must**
    ///   exist (a typo errors, no silent fallback).
    /// * `explicit` = `None` and `./photon.toml` exists → that file.
    /// * otherwise → the baked-in `default.toml` (container path).
    pub fn load(explicit: Option<String>) -> Result<Config, PhotonError> {
        Self::load_with(explicit, |k| std::env::var(k).ok())
    }

    /// `load` with an injected env lookup (for tests).
    pub fn load_with(
        explicit: Option<String>,
        get: impl Fn(&str) -> Option<String>,
    ) -> Result<Config, PhotonError> {
        const DEFAULT_CONFIG: &str = include_str!("default.toml");
        let base = match explicit {
            Some(path) => std::fs::read_to_string(&path)
                .map_err(|e| PhotonError::Config(format!("failed to read config {path:?}: {e}")))?,
            None => std::fs::read_to_string("photon.toml")
                .unwrap_or_else(|_| DEFAULT_CONFIG.to_string()),
        };
        let mut cfg = Self::parse_unvalidated(&base)?;
        cfg.apply_env_overrides(&get)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn from_toml_str(s: &str) -> Result<Config, PhotonError> {
        let config = Self::parse_unvalidated(s)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), PhotonError> {
        if let Some(fed) = &self.federation {
            if let Some(signals) = &fed.signals {
                if fed.mode != FederationMode::Full {
                    return Err(PhotonError::Config(
                        "federation.signals requires mode = `full` — summary mirrors nothing, so a signal list under summary would be silently ignored"
                            .into(),
                    ));
                }
                if signals.is_empty() {
                    return Err(PhotonError::Config(
                        "federation.signals must not be empty — omit it to mirror all signals, or use mode = `summary` to mirror nothing"
                            .into(),
                    ));
                }
                let mut seen = HashSet::new();
                for s in signals {
                    if !seen.insert(*s) {
                        return Err(PhotonError::Config(format!(
                            "duplicate federation signal: {:?}",
                            s.as_str()
                        )));
                    }
                }
            }
        }
        if self.retention.days == 0 {
            return Err(PhotonError::Config("retention.days must be > 0".into()));
        }
        if self.retention.days > MAX_RETENTION_DAYS {
            return Err(PhotonError::Config(format!(
                "retention.days must be <= {MAX_RETENTION_DAYS} (100 years)"
            )));
        }
        if !self
            .schema
            .promoted_attributes
            .iter()
            .any(|a| a == "service.name")
        {
            return Err(PhotonError::Config(
                "schema.promoted_attributes must include \"service.name\" \
                 (it is the primary sort key)"
                    .into(),
            ));
        }
        // A promoted attribute becomes its own Arrow column, so it must be unique and must
        // not collide with a fixed column name — otherwise the schema would carry two fields
        // with the same name and `column_by_name` would silently resolve to the first.
        let mut seen = HashSet::new();
        for attr in &self.schema.promoted_attributes {
            if !seen.insert(attr) {
                return Err(PhotonError::Config(format!(
                    "duplicate promoted attribute: {attr:?}"
                )));
            }
            // A promoted attribute becomes its own column in all three signal schemas
            // (logs, spans, metrics), so it must not collide with any of their fixed
            // column names — not just logs'.
            if schema::FIXED_COLUMNS.contains(&attr.as_str())
                || span_schema::SPAN_FIXED_COLUMNS.contains(&attr.as_str())
                || metric_schema::METRIC_FIXED_COLUMNS.contains(&attr.as_str())
            {
                return Err(PhotonError::Config(format!(
                    "promoted attribute {attr:?} collides with a reserved fixed column name"
                )));
            }
        }
        if self.ingest.token.trim().is_empty() {
            return Err(PhotonError::Config("ingest.token must be set".into()));
        }
        if self.storage.hot_dir.as_os_str().is_empty() {
            return Err(PhotonError::Config("storage.hot_dir must be set".into()));
        }
        if self.storage.db_path.trim().is_empty() {
            return Err(PhotonError::Config(
                "storage.db_path must be non-empty".into(),
            ));
        }
        if !(1..=19).contains(&self.storage.zstd_level) {
            return Err(PhotonError::Config(format!(
                "storage.zstd_level must be between 1 and 19 (got {})",
                self.storage.zstd_level
            )));
        }
        if self.auth.session_secret.trim().is_empty() {
            return Err(PhotonError::Config(
                "auth.session_secret must be set".into(),
            ));
        }
        if let Some(f) = &self.federation {
            if f.endpoint.trim().is_empty() {
                return Err(PhotonError::Config(
                    "federation.endpoint must be non-empty".into(),
                ));
            }
            if f.token.trim().is_empty() {
                return Err(PhotonError::Config(
                    "federation.token must be non-empty".into(),
                ));
            }
            if f.interval_secs < 5 {
                return Err(PhotonError::Config(
                    "federation.interval_secs must be >= 5".into(),
                ));
            }
            if f.queue_batches < 16 {
                return Err(PhotonError::Config(
                    "federation.queue_batches must be >= 16".into(),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
[ingest]
token = "secret"
http_addr = "0.0.0.0:4318"
grpc_addr = "0.0.0.0:4317"

[storage]
hot_dir = "/var/lib/photon/hot"

[retention]
days = 30

[schema]
promoted_attributes = ["service.name", "host.name"]

[wal]
segment_max_bytes = 134217728
segment_max_age_secs = 60
group_commit_max_delay_ms = 5

[auth]
session_secret = "a-long-random-session-signing-secret"
"#;

    #[test]
    fn parses_valid_config() {
        let c = Config::from_toml_str(VALID).unwrap();
        assert_eq!(c.retention.days, 30);
        assert_eq!(c.storage.hot_dir, PathBuf::from("/var/lib/photon/hot"));
        assert!(c.storage.durable.is_none());
    }

    #[test]
    fn storage_db_path_defaults_when_omitted() {
        let c = Config::from_toml_str(VALID).unwrap();
        assert_eq!(c.storage.db_path, "./data/photon.db");
    }

    #[test]
    fn ingest_max_in_flight_defaults_when_omitted() {
        let c = Config::from_toml_str(VALID).unwrap();
        assert_eq!(c.ingest.max_in_flight, 256);
    }

    #[test]
    fn ingest_max_in_flight_can_be_overridden() {
        let toml = VALID.replace(
            "grpc_addr = \"0.0.0.0:4317\"",
            "grpc_addr = \"0.0.0.0:4317\"\nmax_in_flight = 64",
        );
        let c = Config::from_toml_str(&toml).unwrap();
        assert_eq!(c.ingest.max_in_flight, 64);
    }

    #[test]
    fn ingest_max_body_bytes_defaults_when_omitted() {
        let c = Config::from_toml_str(VALID).unwrap();
        assert_eq!(c.ingest.max_body_bytes, 16 * 1024 * 1024);
    }

    #[test]
    fn ingest_max_body_bytes_env_override_applies() {
        let mut cfg = Config::parse_unvalidated(include_str!("default.toml")).unwrap();
        let env = env_of(&[("PHOTON_INGEST_MAX_BODY_BYTES", "1048576")]);
        cfg.apply_env_overrides(&env).unwrap();
        assert_eq!(cfg.ingest.max_body_bytes, 1_048_576);
    }

    #[test]
    fn storage_db_path_can_be_overridden() {
        let toml = VALID.replace(
            "hot_dir = \"/var/lib/photon/hot\"",
            "hot_dir = \"/var/lib/photon/hot\"\ndb_path = \"/srv/photon/state.db\"",
        );
        let c = Config::from_toml_str(&toml).unwrap();
        assert_eq!(c.storage.db_path, "/srv/photon/state.db");
    }

    #[test]
    fn rejects_empty_db_path() {
        let toml = VALID.replace(
            "hot_dir = \"/var/lib/photon/hot\"",
            "hot_dir = \"/var/lib/photon/hot\"\ndb_path = \"\"",
        );
        let err = Config::from_toml_str(&toml).unwrap_err();
        assert!(err.to_string().contains("db_path"));
    }

    #[test]
    fn rejects_missing_service_name_promotion() {
        let bad = VALID.replace(r#"["service.name", "host.name"]"#, r#"["host.name"]"#);
        let err = Config::from_toml_str(&bad).unwrap_err();
        assert!(err.to_string().contains("service.name"));
    }

    #[test]
    fn rejects_zero_retention() {
        let bad = VALID.replace("days = 30", "days = 0");
        let err = Config::from_toml_str(&bad).unwrap_err();
        assert!(err.to_string().contains("retention"));
    }

    #[test]
    fn rejects_retention_days_above_cap() {
        // A user typing a huge number (e.g. "999999999" to mean "forever") must be rejected, not
        // silently accepted and later overflow the i64 cutoff arithmetic in the retention loops.
        let bad = VALID.replace("days = 30", "days = 999999999");
        let err = Config::from_toml_str(&bad).unwrap_err();
        assert!(err.to_string().contains("retention"));
    }

    #[test]
    fn rejects_duplicate_promoted_attribute() {
        let bad = VALID.replace(
            r#"["service.name", "host.name"]"#,
            r#"["service.name", "host.name", "host.name"]"#,
        );
        let err = Config::from_toml_str(&bad).unwrap_err();
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn rejects_promoted_attribute_colliding_with_fixed_column() {
        let bad = VALID.replace(
            r#"["service.name", "host.name"]"#,
            r#"["service.name", "body"]"#,
        );
        let err = Config::from_toml_str(&bad).unwrap_err();
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn rejects_promoted_attribute_colliding_with_metric_fixed_column() {
        let bad = VALID.replace(
            r#"["service.name", "host.name"]"#,
            r#"["service.name", "value"]"#,
        );
        let err = Config::from_toml_str(&bad).unwrap_err();
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn storage_zstd_level_defaults_to_6_when_omitted() {
        let c = Config::from_toml_str(VALID).unwrap();
        assert_eq!(c.storage.zstd_level, 6);
    }

    #[test]
    fn storage_zstd_level_accepts_a_valid_override() {
        let toml = VALID.replace(
            "hot_dir = \"/var/lib/photon/hot\"",
            "hot_dir = \"/var/lib/photon/hot\"\nzstd_level = 3",
        );
        let c = Config::from_toml_str(&toml).unwrap();
        assert_eq!(c.storage.zstd_level, 3);
    }

    #[test]
    fn rejects_zstd_level_below_range() {
        let bad = VALID.replace(
            "hot_dir = \"/var/lib/photon/hot\"",
            "hot_dir = \"/var/lib/photon/hot\"\nzstd_level = 0",
        );
        let err = Config::from_toml_str(&bad).unwrap_err();
        assert!(err.to_string().contains("zstd_level"));
    }

    #[test]
    fn rejects_zstd_level_above_range() {
        let bad = VALID.replace(
            "hot_dir = \"/var/lib/photon/hot\"",
            "hot_dir = \"/var/lib/photon/hot\"\nzstd_level = 20",
        );
        let err = Config::from_toml_str(&bad).unwrap_err();
        assert!(err.to_string().contains("zstd_level"));
    }

    #[test]
    fn rejects_empty_session_secret() {
        let bad = VALID.replace(
            r#"session_secret = "a-long-random-session-signing-secret""#,
            r#"session_secret = """#,
        );
        let err = Config::from_toml_str(&bad).unwrap_err();
        assert!(err.to_string().contains("session_secret"));
    }

    #[test]
    fn apm_defaults_when_omitted_and_parses_when_present() {
        let cfg = Config::from_toml_str(VALID).unwrap();
        assert_eq!(cfg.apm.default_apdex_threshold_ms, 500);

        let with_apm = format!("{VALID}\n[apm]\ndefault_apdex_threshold_ms = 750\n");
        let cfg2 = Config::from_toml_str(&with_apm).unwrap();
        assert_eq!(cfg2.apm.default_apdex_threshold_ms, 750);
    }

    #[test]
    fn parse_unvalidated_skips_validation() {
        // days = 0 is invalid, but parse_unvalidated must NOT reject it.
        let toml = VALID.replace("days = 30", "days = 0");
        let cfg = Config::parse_unvalidated(&toml).unwrap();
        assert_eq!(cfg.retention.days, 0);
        // from_toml_str (parse + validate) still rejects it.
        assert!(Config::from_toml_str(&toml).is_err());
    }

    /// Build an env lookup closure from a fixed set of pairs (no process env touched).
    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k: &str| {
            pairs
                .iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn default_toml_parses_but_fails_validation_without_secrets() {
        const DEFAULT_CONFIG: &str = include_str!("default.toml");
        // Well-formed: parses.
        let cfg = Config::parse_unvalidated(DEFAULT_CONFIG).unwrap();
        assert_eq!(cfg.storage.hot_dir, PathBuf::from("/var/lib/photon/hot"));
        // Unsafe as-is (empty secrets): validation refuses it.
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("token") || err.contains("session_secret"));
    }

    #[test]
    fn env_overrides_apply_and_win_over_base() {
        let mut cfg = Config::parse_unvalidated(include_str!("default.toml")).unwrap();
        let env = env_of(&[
            ("PHOTON_INGEST_TOKEN", "tok"),
            (
                "PHOTON_SESSION_SECRET",
                "a-32-byte-minimum-session-secret!!",
            ),
            ("PHOTON_RETENTION_DAYS", "7"),
            ("PHOTON_STORAGE_HOT_DIR", "/data/hot"),
            (
                "PHOTON_PROMOTED_ATTRIBUTES",
                "service.name, host.name, deployment.environment",
            ),
        ]);
        cfg.apply_env_overrides(&env).unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.ingest.token, "tok");
        assert_eq!(cfg.retention.days, 7);
        assert_eq!(cfg.storage.hot_dir, PathBuf::from("/data/hot"));
        assert_eq!(cfg.schema.promoted_attributes.len(), 3);
        assert_eq!(cfg.schema.promoted_attributes[2], "deployment.environment");
    }

    #[test]
    fn env_numeric_parse_error_names_the_variable() {
        let mut cfg = Config::parse_unvalidated(include_str!("default.toml")).unwrap();
        let env = env_of(&[("PHOTON_RETENTION_DAYS", "notanumber")]);
        let err = cfg.apply_env_overrides(&env).unwrap_err().to_string();
        assert!(err.contains("PHOTON_RETENTION_DAYS"));
    }

    #[test]
    fn durable_enabled_by_endpoint_with_bucket_and_region() {
        let mut cfg = Config::parse_unvalidated(include_str!("default.toml")).unwrap();
        let env = env_of(&[
            ("PHOTON_DURABLE_ENDPOINT", "http://garage:3900"),
            ("PHOTON_DURABLE_BUCKET", "photon"),
            ("PHOTON_DURABLE_REGION", "garage"),
            ("PHOTON_DURABLE_ACCESS_KEY_ID", "GK123"),
            ("PHOTON_DURABLE_SECRET_ACCESS_KEY", "secret"),
        ]);
        cfg.apply_env_overrides(&env).unwrap();
        let d = cfg.storage.durable.expect("durable enabled");
        assert_eq!(d.endpoint, "http://garage:3900");
        assert_eq!(d.bucket, "photon");
        assert_eq!(d.region, "garage");
        assert_eq!(d.access_key_id.as_deref(), Some("GK123"));
    }

    #[test]
    fn durable_endpoint_without_bucket_errors() {
        let mut cfg = Config::parse_unvalidated(include_str!("default.toml")).unwrap();
        let env = env_of(&[("PHOTON_DURABLE_ENDPOINT", "http://garage:3900")]);
        let err = cfg.apply_env_overrides(&env).unwrap_err().to_string();
        assert!(err.contains("PHOTON_DURABLE_BUCKET"));
    }

    #[test]
    fn empty_durable_endpoint_leaves_durable_off() {
        let mut cfg = Config::parse_unvalidated(include_str!("default.toml")).unwrap();
        let env = env_of(&[("PHOTON_DURABLE_ENDPOINT", "")]);
        cfg.apply_env_overrides(&env).unwrap();
        assert!(cfg.storage.durable.is_none());
    }

    #[test]
    fn uptime_defaults_when_section_absent() {
        // default.toml has no [uptime] section; the subsystem is always on, so the field
        // parses to UptimeConfig::default() rather than being disabled.
        let cfg = Config::parse_unvalidated(include_str!("default.toml")).unwrap();
        assert_eq!(cfg.uptime.retention_days, 30);
        assert_eq!(cfg.uptime.default_interval, "60s");
    }

    #[test]
    fn validate_rejects_empty_ingest_token() {
        let bad = VALID.replace(r#"token = "secret""#, r#"token = """#);
        let err = Config::from_toml_str(&bad).unwrap_err().to_string();
        assert!(err.contains("token"));
    }

    #[test]
    fn load_with_explicit_missing_path_errors() {
        let err = Config::load_with(Some("/no/such/photon.toml".into()), |_| None).unwrap_err();
        assert!(err.to_string().contains("failed to read config"));
    }

    #[test]
    fn live_config_defaults_when_absent() {
        // VALID has no [live] section; it still parses, using defaults.
        let cfg: Config = toml::from_str(VALID).expect("parses without [live]");
        assert_eq!(cfg.live.broadcast_capacity, 128);
        assert_eq!(cfg.live.flush_interval_ms, 250);
        assert_eq!(cfg.live.max_rows_per_flush, 200);
        assert_eq!(cfg.live.max_connections, 32);
    }

    #[test]
    fn live_config_overrides_parse() {
        let with_live =
            format!("{VALID}\n[live]\nbroadcast_capacity = 4096\nmax_connections = 8\n");
        let cfg: Config = toml::from_str(&with_live).expect("parses with [live]");
        assert_eq!(cfg.live.broadcast_capacity, 4096);
        assert_eq!(cfg.live.max_connections, 8);
        assert_eq!(cfg.live.flush_interval_ms, 250); // untouched → default
    }

    #[test]
    fn federation_absent_is_none() {
        let cfg = Config::from_toml_str(VALID).unwrap();
        assert!(cfg.federation.is_none());
    }

    #[test]
    fn federation_parses_and_defaults() {
        let with_federation = format!(
            "{VALID}\n[federation]\nendpoint = \"https://central.example.com\"\ntoken = \"tk_tenant_abc\"\n"
        );
        let cfg = Config::from_toml_str(&with_federation).unwrap();
        let f = cfg.federation.unwrap();
        assert_eq!(f.mode, FederationMode::Summary);
        assert_eq!(f.interval_secs, 30);
        assert_eq!(f.queue_batches, 1024);
    }

    #[test]
    fn federation_rejects_empty_endpoint() {
        let with_federation = format!("{VALID}\n[federation]\nendpoint = \"\"\ntoken = \"t\"\n");
        let err = Config::from_toml_str(&with_federation).unwrap_err();
        assert!(err.to_string().contains("federation"));
    }

    #[test]
    fn federation_signals_subset_parses_under_full() {
        let toml = format!(
            "{VALID}\n[federation]\nendpoint = \"https://c\"\ntoken = \"tk\"\nmode = \"full\"\nsignals = [\"traces\", \"rum\"]\n"
        );
        let f = Config::from_toml_str(&toml).unwrap().federation.unwrap();
        assert_eq!(
            f.signals,
            Some(vec![FederationSignal::Traces, FederationSignal::Rum])
        );
    }

    #[test]
    fn federation_signals_under_summary_is_a_config_error() {
        let toml = format!(
            "{VALID}\n[federation]\nendpoint = \"https://c\"\ntoken = \"tk\"\nsignals = [\"traces\"]\n"
        );
        let err = Config::from_toml_str(&toml).unwrap_err();
        assert!(err.to_string().contains("requires mode"));
    }

    #[test]
    fn federation_signals_empty_or_duplicated_rejected() {
        let empty = format!(
            "{VALID}\n[federation]\nendpoint = \"https://c\"\ntoken = \"tk\"\nmode = \"full\"\nsignals = []\n"
        );
        assert!(Config::from_toml_str(&empty)
            .unwrap_err()
            .to_string()
            .contains("must not be empty"));
        let dup = format!(
            "{VALID}\n[federation]\nendpoint = \"https://c\"\ntoken = \"tk\"\nmode = \"full\"\nsignals = [\"traces\", \"traces\"]\n"
        );
        assert!(Config::from_toml_str(&dup)
            .unwrap_err()
            .to_string()
            .contains("duplicate"));
        let dup_rum = format!(
            "{VALID}\n[federation]\nendpoint = \"https://c\"\ntoken = \"tk\"\nmode = \"full\"\nsignals = [\"rum\", \"rum\"]\n"
        );
        assert!(Config::from_toml_str(&dup_rum)
            .unwrap_err()
            .to_string()
            .contains("duplicate"));
    }

    #[test]
    fn federation_signals_env_override_parses_csv() {
        let mut cfg = Config::parse_unvalidated(include_str!("default.toml")).unwrap();
        let env = env_of(&[
            ("PHOTON_FEDERATION_ENDPOINT", "https://c"),
            ("PHOTON_FEDERATION_TOKEN", "tk"),
            ("PHOTON_FEDERATION_MODE", "full"),
            ("PHOTON_FEDERATION_SIGNALS", "traces, metrics, rum"),
        ]);
        cfg.apply_env_overrides(&env).unwrap();
        assert_eq!(
            cfg.federation.unwrap().signals,
            Some(vec![
                FederationSignal::Traces,
                FederationSignal::Metrics,
                FederationSignal::Rum
            ])
        );
    }

    #[test]
    fn federation_env_overrides() {
        let mut cfg = Config::parse_unvalidated(include_str!("default.toml")).unwrap();
        let env = env_of(&[
            ("PHOTON_FEDERATION_ENDPOINT", "https://c"),
            ("PHOTON_FEDERATION_TOKEN", "tk"),
            ("PHOTON_FEDERATION_MODE", "full"),
        ]);
        cfg.apply_env_overrides(&env).unwrap();
        assert_eq!(cfg.federation.unwrap().mode, FederationMode::Full);
    }
}
