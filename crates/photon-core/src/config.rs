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
}

impl IngestConfig {
    fn default_max_in_flight() -> usize {
        256
    }
    fn default_max_body_bytes() -> usize {
        16 * 1024 * 1024
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
}

impl StorageConfig {
    fn default_db_path() -> String {
        "./data/photon.db".to_string()
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
    /// Per-signal broadcast channel depth before a slow subscriber sees `Lagged`.
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
            broadcast_capacity: 1024,
            flush_interval_ms: 250,
            max_rows_per_flush: 200,
            max_connections: 32,
        }
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
        if self.retention.days == 0 {
            return Err(PhotonError::Config("retention.days must be > 0".into()));
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
        if self.auth.session_secret.trim().is_empty() {
            return Err(PhotonError::Config(
                "auth.session_secret must be set".into(),
            ));
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
        assert_eq!(cfg.live.broadcast_capacity, 1024);
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
}
