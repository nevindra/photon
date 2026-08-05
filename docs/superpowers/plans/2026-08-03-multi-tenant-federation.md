# Multi-Tenant Federation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One central Photon monitors many customer Photon installs (tenants) that push to it egress-only over OTLP — `summary` mode (synthetic health metrics) by default, opt-in `full` mode (raw OTLP mirror), with per-tenant tokens, server-side tenant stamping, a tenant board on Home, and tenant-scoped drill-down through the existing views.

**Architecture:** Tenant-side: an optional `[federation]` config block turns on a summary pusher task (periodic OTLP metrics via reqwest) and, in `full` mode, a bounded best-effort tee of incoming OTLP batches forwarded to central. Central-side: a `tenants` registry (SQLite, clone of `rum_apps`) mints per-tenant tokens; ingest resolves token → tenant and stamps a `tenant` resource attribute server-side (client labels never trusted); `tenant` rides the existing promoted-attributes machinery so filtering works in all three query engines with zero engine changes. UI: Home gains a conditional tenant board; `tenant` becomes a fifth `ScopeType` whose filter term is appended to the `q` grammar by the query composables.

**Tech Stack:** Rust (axum, tonic, rusqlite, reqwest+rustls, prost/opentelemetry-proto 0.27), Vue 3 + TanStack Query, SQLite control-plane DB.

## Global Constraints

- Never bump co-pinned deps (`arrow 53`/`datafusion 43`/`parquet 53`/`object_store 0.11`, `opentelemetry-proto 0.27`/`tonic 0.12`/`prost 0.13`).
- Ingest ack = local WAL fsync. Federation must NEVER block or fail local ingest (best-effort, bounded, drop-oldest with counters).
- Tenant identity comes ONLY from token resolution at central; any client-supplied `tenant` attribute is overwritten (trust boundary).
- Control-plane SQLite has no migration framework: new TABLE only, never ALTER an existing table.
- `PhotonError`: use existing variants; never edit the enum.
- Frontend: bun, no Pinia, new files may be `<script setup lang="ts">`, gated by `bun run type-check`.
- Chat artifacts in English; commits use Conventional Commits.
- Config default: federation absent = disabled. Mode default `summary`.
- Prom remote-write (`/api/v1/write`) accepts the LOCAL token only — tenant tokens get 401 there (v1 limitation, documented).
- Metric namespace pushed by tenants: `photon.federation.*`. Attribute stamped by central: `tenant`.
- Docs must be updated in the same change (CLAUDE.md hook reminds).

---

### Task 1: `[federation]` config block `[sonnet-4.6]`

**Files:**
- Modify: `crates/photon-core/src/config.rs` (root `Config` at `:9`, env overrides in `apply_env_overrides` at `:270`, checks in `validate()` at `:399`)

**Interfaces:**
- Produces: `Config.federation: Option<FederationConfig>`; `FederationConfig { endpoint: String, token: String, mode: FederationMode, interval_secs: u64, queue_batches: usize }`; `enum FederationMode { Summary, Full }` (serde `lowercase`).

- [ ] **Step 1: Write failing tests** in the existing `#[cfg(test)]` module of `config.rs`:

```rust
#[test]
fn federation_absent_is_none() {
    let cfg = parse_min_toml(""); // reuse the module's existing minimal-toml helper
    assert!(cfg.federation.is_none());
}

#[test]
fn federation_parses_and_defaults() {
    let cfg = parse_min_toml(r#"
[federation]
endpoint = "https://central.example.com"
token = "tk_tenant_abc"
"#);
    let f = cfg.federation.unwrap();
    assert_eq!(f.mode, FederationMode::Summary);
    assert_eq!(f.interval_secs, 30);
    assert_eq!(f.queue_batches, 1024);
}

#[test]
fn federation_rejects_empty_endpoint() {
    let err = parse_min_toml_err(r#"
[federation]
endpoint = ""
token = "t"
"#);
    assert!(err.to_string().contains("federation"));
}

#[test]
fn federation_env_overrides() {
    // PHOTON_FEDERATION_ENDPOINT / _TOKEN / _MODE create-or-override the block,
    // mirroring how PHOTON_STORAGE_DURABLE_ENDPOINT toggles [storage.durable].
    let cfg = parse_with_env(&[("PHOTON_FEDERATION_ENDPOINT", "https://c"),
                               ("PHOTON_FEDERATION_TOKEN", "tk"),
                               ("PHOTON_FEDERATION_MODE", "full")]);
    assert_eq!(cfg.federation.unwrap().mode, FederationMode::Full);
}
```

(If `parse_min_toml`/`parse_with_env` helpers don't exist under those names, use whatever the module's existing tests use — do not invent a parallel helper.)

- [ ] **Step 2: Run** `cargo test -p photon-core federation` — expect FAIL (no `federation` field).
- [ ] **Step 3: Implement.** Add after `alerts` in `Config`:

```rust
#[serde(default)]
pub federation: Option<FederationConfig>,
```

```rust
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationConfig {
    pub endpoint: String,
    pub token: String,
    #[serde(default)]
    pub mode: FederationMode,
    #[serde(default = "default_federation_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_federation_queue")]
    pub queue_batches: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FederationMode { #[default] Summary, Full }

fn default_federation_interval() -> u64 { 30 }
fn default_federation_queue() -> usize { 1024 }
```

In `apply_env_overrides`, follow the `storage.durable` pattern (`config.rs:330-359`): `PHOTON_FEDERATION_ENDPOINT` presence creates the block if absent; then `PHOTON_FEDERATION_TOKEN`, `PHOTON_FEDERATION_MODE` (`"summary"|"full"`), `PHOTON_FEDERATION_INTERVAL_SECS`, `PHOTON_FEDERATION_QUEUE_BATCHES` override fields. In `validate()`: if `Some`, `endpoint` and `token` must be non-empty, `interval_secs >= 5`, `queue_batches >= 16`.

- [ ] **Step 4: Run** `cargo test -p photon-core federation` — expect PASS. Also `cargo test -p photon-core` (no regressions).
- [ ] **Step 5: Commit** `feat(core): optional [federation] config block`

---

### Task 2: multi-token resolution + tenant stamping at ingest `[opus-4.7]`

Trust-boundary task. The single choke point is `check_bearer_token` (`crates/photon-ingest/src/auth.rs:15`), called inline as the first statement of all 7 handlers.

**Files:**
- Modify: `crates/photon-ingest/src/auth.rs`
- Modify: `crates/photon-ingest/src/lib.rs` (`IngestServer` at `:63`, state construction in `serve` at `:132-203`)
- Modify: `crates/photon-ingest/src/http.rs:47-97`, `trace_http.rs:48`, `metrics_http.rs:50`, `promrw_http.rs:64`, `grpc.rs:29`, `grpc_trace.rs`, `grpc_metrics.rs`
- Modify: `crates/photon-core/src/lib.rs` (or a small `federation.rs` module in photon-core) — shared alias only
- Modify: `crates/photon-server/src/main.rs:372` (pass the map)

**Interfaces:**
- Produces (photon-core): `pub type TenantTokenMap = std::sync::Arc<std::sync::RwLock<std::collections::HashMap<String, String>>>;` (token → tenant name; pure type alias, no I/O — allowed in photon-core).
- Produces (photon-ingest auth.rs):

```rust
pub enum Auth { Local, Tenant(String), Denied }

/// Resolve a bearer header against the local token and the tenant token map.
/// Constant-time compare for the local token (existing check_bearer_token);
/// tenant lookup is a HashMap get on the exact token string.
pub fn resolve_bearer(header: Option<&str>, local: &str, tenants: &TenantTokenMap) -> Auth
```

- Produces (mapping stamp helper, in `auth.rs` or `mapping.rs`):

```rust
/// Insert-or-overwrite the `tenant` resource attribute. Client-supplied values never survive.
pub fn stamp_tenant(attrs: &mut Vec<opentelemetry_proto::tonic::common::v1::KeyValue>, tenant: &str)
```

- `IngestServer` gains `pub tenant_tokens: TenantTokenMap` (photon-server passes `TenantTokenMap::default()` when federation registry is absent — empty map = tenant auth simply never matches; zero behavior change).

- [ ] **Step 1: Write failing tests** in `auth.rs` tests module:

```rust
#[test]
fn resolve_local_token() {
    let map = TenantTokenMap::default();
    assert!(matches!(resolve_bearer(Some("Bearer secret"), "secret", &map), Auth::Local));
}

#[test]
fn resolve_tenant_token() {
    let map = TenantTokenMap::default();
    map.write().unwrap().insert("tk_tenant_x".into(), "divtik".into());
    match resolve_bearer(Some("Bearer tk_tenant_x"), "secret", &map) {
        Auth::Tenant(t) => assert_eq!(t, "divtik"),
        _ => panic!("expected tenant"),
    }
}

#[test]
fn resolve_denies_unknown_and_missing() {
    let map = TenantTokenMap::default();
    assert!(matches!(resolve_bearer(Some("Bearer nope"), "secret", &map), Auth::Denied));
    assert!(matches!(resolve_bearer(None, "secret", &map), Auth::Denied));
}

#[test]
fn stamp_tenant_overwrites_client_label() {
    let mut attrs = vec![kv("tenant", "spoofed"), kv("service.name", "api")];
    stamp_tenant(&mut attrs, "divtik");
    let vals: Vec<_> = attrs.iter().filter(|a| a.key == "tenant").collect();
    assert_eq!(vals.len(), 1);
    assert_eq!(string_value(&vals[0]), "divtik"); // helper: unwrap AnyValue::StringValue
}
```

- [ ] **Step 2: Run** `cargo test -p photon-ingest auth` — expect FAIL.
- [ ] **Step 3: Implement** `Auth`, `resolve_bearer` (keep `check_bearer_token` as the constant-time local branch; tenant branch: strip exact `"Bearer "` prefix, `HashMap::get`), and `stamp_tenant` (retain-remove existing `key == "tenant"`, push a new string KeyValue).
- [ ] **Step 4: Wire the 7 handlers.** Each per-signal state struct (`HttpState`, `TraceHttpState`, `MetricsHttpState`, `PromRwHttpState`, and the 3 gRPC services) gains `tenant_tokens: TenantTokenMap`, populated in `IngestServer::serve`. Handler pattern (logs shown; traces/metrics identical, post-decode pre-map):

```rust
let auth = resolve_bearer(auth_header, &state.token, &state.tenant_tokens);
if matches!(auth, Auth::Denied) { return unauthorized(); }
// ... existing permit + decode ...
if let Auth::Tenant(name) = &auth {
    for rl in &mut req.resource_logs {
        if let Some(res) = rl.resource.as_mut() { stamp_tenant(&mut res.attributes, name); }
        else { rl.resource = Some(Resource { attributes: vec![kv("tenant", name)], ..Default::default() }); }
    }
}
```

gRPC: same stamping on `request.into_inner()` (already decoded). **promrw:** `Auth::Tenant(_) => 401` with body `"tenant tokens not accepted on remote-write"`.

- [ ] **Step 5: Handler-level test** (in the existing http tests): POST `/v1/logs` with a tenant token, then assert the appended `RecordBatch` (via the test WAL fake the module already uses) contains `tenant` in the attributes map / promoted column with the resolved name, even when the request body pre-set `tenant=spoofed`.
- [ ] **Step 6: Run** `cargo test -p photon-ingest` — expect PASS. `cargo clippy -p photon-ingest --all-targets`.
- [ ] **Step 7: Commit** `feat(ingest): per-tenant token resolution + server-side tenant stamping`

---

### Task 3: tenant registry store (central control plane) `[sonnet-4.6]`

Clone of `crates/photon-api/src/rum_apps.rs` (the newest/cleanest store). New TABLE, never ALTER.

**Files:**
- Create: `crates/photon-api/src/tenants.rs`
- Modify: `crates/photon-api/src/lib.rs` (module decl; `AppStateInner` field + `with_tenant_store` builder beside `lib.rs:158-204`)

**Interfaces:**
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Tenant {
    pub name: String,          // PK, [a-z0-9-]{1,64}
    pub token: String,         // UNIQUE, minted server-side
    pub ui_url: Option<String>,// link-out for summary-mode tenants
    pub created_at: i64,       // unix ms
}

#[async_trait]
pub trait TenantStore: Send + Sync {
    async fn list(&self) -> Result<Vec<Tenant>, PhotonError>;
    async fn create(&self, t: &Tenant) -> Result<(), PhotonError>;          // conflict -> PhotonError as rum_apps does
    async fn update(&self, name: &str, ui_url: Option<&str>) -> Result<bool, PhotonError>;
    async fn rotate_token(&self, name: &str, new_token: &str) -> Result<bool, PhotonError>;
    async fn delete(&self, name: &str) -> Result<bool, PhotonError>;
}

pub struct SqliteTenantStore { /* conn: Mutex<Connection> */ }
impl SqliteTenantStore {
    pub fn open(path: &str) -> Result<Self, PhotonError>;
    #[cfg(test)] pub fn open_in_memory() -> Result<Self, PhotonError>;
}
pub fn validate_tenant_name(name: &str) -> Result<(), String>;
pub fn mint_tenant_token() -> String; // format!("tk_tenant_{}", uuid::Uuid::new_v4().simple())
```

SCHEMA: `CREATE TABLE IF NOT EXISTS tenants (name TEXT PRIMARY KEY, token TEXT NOT NULL UNIQUE, ui_url TEXT, created_at INTEGER NOT NULL);` with the same PRAGMAs as `rum_apps.rs:98` (`journal_mode=WAL`, `busy_timeout=5000`).

- [ ] **Step 1: Write failing tests** (in-memory store): create→list roundtrip; duplicate name errors; rotate_token changes token and returns true / false for unknown name; delete; `validate_tenant_name` rejects empty, uppercase, spaces, >64 chars; `mint_tenant_token` starts with `tk_tenant_` and is unique across two calls.
- [ ] **Step 2: Run** `cargo test -p photon-api tenants` — FAIL.
- [ ] **Step 3: Implement** by copying `rum_apps.rs` structure (error mapping through the local `err` → `PhotonError::Io` helper).
- [ ] **Step 4: Run** — PASS. **Step 5: Commit** `feat(api): tenants registry store (SQLite)`

---

### Task 4: tenant API routes + live token-map refresh `[sonnet-4.6]`

**Files:**
- Modify: `crates/photon-api/src/tenants.rs` (handlers), `crates/photon-api/src/lib.rs` (routes beside the rum_apps routes at `:278-283`)
- Modify: `crates/photon-server/src/main.rs` (instantiate beside `SqliteRumAppStore` at `:442-444`; create the shared `TenantTokenMap`, hand it to BOTH `IngestServer` and the API layer)

**Interfaces:**
- Consumes: `TenantStore` (Task 3), `TenantTokenMap` (Task 2).
- Produces routes (session-cookie-authed like everything else): `GET/POST /api/tenants`, `PATCH/DELETE /api/tenants/:name`, `POST /api/tenants/:name/rotate-token`.
- Produces: `TenantApi::new(store: Arc<dyn TenantStore>, tokens: TenantTokenMap) -> Result<Self, PhotonError>` which loads the map at startup, and `reload_tokens()` called after every mutation (wholesale rebuild, exactly `RumApi::reload_cache` at `rum.rs:76`). GET list responses REDACT the token to its last 4 chars (`"…abcd"`); the full token is returned ONCE in the POST/rotate response bodies (`201 {tenant}` / `200 {token}`), mirroring the rum minted-key flow.

- [ ] **Step 1: Failing handler tests** using the crate's existing axum test harness pattern (as the rum app handlers are tested): create → 201 with full token; list → token redacted; create duplicate → 409; rotate → 200 new token, old token gone from the shared map, new token present; delete → 204 and token removed from map; invalid name → 400.
- [ ] **Step 2: Run** — FAIL. **Step 3: Implement** handlers (copy `create_app`/`rotate_app_key`/`delete_app` shapes from `rum.rs:294/392/408`). **Step 4: Run** `cargo test -p photon-api` — PASS.
- [ ] **Step 5: Wire photon-server:** in `main.rs`, `let tenant_tokens = TenantTokenMap::default();` before ingest construction; pass a clone into `IngestServer` (Task 2 field) and into `ApiServer` via `with_tenant_store(store, tenant_tokens.clone())`. `cargo build` green.
- [ ] **Step 6: Commit** `feat(api,server): tenant registry routes + live ingest token map`

---

### Task 5: tenant-side summary pusher `[sonnet-4.6]`

**Files:**
- Create: `crates/photon-server/src/federation/mod.rs`, `crates/photon-server/src/federation/otlp.rs`
- Modify: `crates/photon-server/src/main.rs` (spawn beside `spawn_usage_sampler`, i.e. in the `:387-411` region), `crates/photon-server/Cargo.toml` (promote `reqwest` from dev-deps to deps, `default-features = false, features = ["rustls-tls"]`)

**Interfaces:**
- Consumes: `Config.federation` (Task 1); ingest counters (`counters.<signal>.snapshot()` as used by `spawn_usage_sampler` at `main.rs:755-803`); `storage_stats()` on the three query engines; `AlertStore::list_open_incidents()`.
- Produces: `federation::spawn_summary_pusher(cfg: FederationConfig, deps: SummaryDeps, stats: Arc<FederationStats>) -> JoinHandle<()>` and

```rust
/// Shared, lock-free-ish push telemetry read by the /api/federation/status seam (Task 7).
#[derive(Default)]
pub struct FederationStats {
    pub last_push_ms: AtomicI64,      // 0 = never
    pub last_error: Mutex<Option<String>>,
    pub pushed: AtomicU64,
    pub dropped: AtomicU64,           // full-mode tee drops (Task 6 writes it)
    pub queued: AtomicU64,            // full-mode tee queue depth (Task 6)
}
```

- `federation/otlp.rs`: copy `to_otlp`/`to_metric`/`kv` (~70 lines) from `crates/photon-agent/src/otlp.rs:15-100` (photon-agent has no lib target — copying is the decided trade; leave a one-line comment naming the source). Emitted metrics, resource attr `service.name = "photon"` plus attr `mode = "summary"|"full"`:
  - `photon.federation.up` gauge = 1
  - `photon.federation.ingest.rows` monotonic sum, attr `signal` ∈ logs|traces|metrics (cumulative counter snapshots — central differences them)
  - `photon.federation.ingest.bytes` monotonic sum, attr `signal`
  - `photon.federation.incidents.open` gauge
  - `photon.federation.disk.hot_bytes` gauge, attr `signal`

- [ ] **Step 1: Failing unit test** for the payload builder (pure fn `build_summary(snapshot: &SummarySnapshot, mode: FederationMode) -> ExportMetricsServiceRequest` where `SummarySnapshot { rows: [(String,u64);3], bytes: [(String,u64);3], open_incidents: u64, hot_bytes: [(String,u64);3] }`): assert metric names, `up == 1`, `mode` attribute present, one `signal` datapoint per signal.
- [ ] **Step 2:** `cargo test -p photon-server federation` — FAIL. **Step 3: Implement** builder + the loop (copy the `spawn_usage_sampler` interval shape at `main.rs:755`; reqwest POST `{endpoint}/v1/metrics`, `bearer_auth(token)`, `Content-Type: application/x-protobuf`, 10s timeout; on error → `stats.last_error`, on 2xx → `stats.last_push_ms`; NEVER propagate errors, loop forever; skipped intervals are fine per the shared understanding). Spawn only `if let Some(fed) = cfg.federation.clone()`.
- [ ] **Step 4:** `cargo test -p photon-server federation` PASS; `cargo build` green.
- [ ] **Step 5: Commit** `feat(server): federation summary pusher (OTLP metrics, best-effort)`

---

### Task 6: full-mode tee + forwarder `[opus-4.7]`

Touches the ack path — the tee must be provably non-blocking (`try_send` only).

**Files:**
- Modify: `crates/photon-ingest/src/lib.rs` (tee type + `IngestServer` field), `http.rs`, `trace_http.rs`, `metrics_http.rs`, `grpc.rs`, `grpc_trace.rs`, `grpc_metrics.rs`
- Modify: `crates/photon-server/src/federation/mod.rs` (forwarder drain task), `crates/photon-server/src/main.rs` (wiring)

**Interfaces:**
- Produces (photon-ingest):

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TeeSignal { Logs, Traces, Metrics }

/// Bounded, non-blocking tee of decompressed OTLP protobuf payloads.
/// try_send only — a full queue increments `dropped` and returns; never awaits.
#[derive(Clone)]
pub struct FederationTee {
    tx: tokio::sync::mpsc::Sender<(TeeSignal, bytes::Bytes)>,
    pub dropped: Arc<AtomicU64>,
}
impl FederationTee {
    pub fn channel(capacity: usize) -> (Self, mpsc::Receiver<(TeeSignal, Bytes)>);
    pub fn offer(&self, signal: TeeSignal, payload: Bytes); // try_send; drop-oldest is approximated by drop-newest + counter (documented)
}
```

- `IngestServer` gains `pub federation_tee: Option<FederationTee>`.
- HTTP handlers: after successful auth + decode, `if let Some(t) = &state.federation_tee { t.offer(TeeSignal::Logs, body.clone()) }` (`Bytes` clone is refcounted, zero-copy — seam identified at `http.rs:77`; do it BEFORE `drop(body)` at `http.rs:83`). gRPC handlers: `t.offer(sig, req.encode_to_vec().into())` (prost re-encode — gRPC never sees raw bytes).
- Produces (photon-server): `federation::spawn_forwarder(cfg: FederationConfig, rx: mpsc::Receiver<(TeeSignal, Bytes)>, stats: Arc<FederationStats>) -> JoinHandle<()>` — drains rx, POSTs to `{endpoint}/v1/{logs|traces|metrics}` with the tenant bearer token, ≤3 attempts with 250ms/1s/4s backoff, then drops + increments `stats.dropped`; updates `stats.queued` from a shared depth gauge each loop.

- [ ] **Step 1: Failing tee unit tests** (photon-ingest): `offer` beyond capacity does not block (run under `tokio::time::timeout(100ms)`) and increments `dropped`; payload arrives byte-identical on rx.
- [ ] **Step 2:** FAIL. **Step 3: Implement** tee + handler hooks (tee only fires for `Auth::Local` AND `Auth::Tenant`? No — tenant-side Photon has no tenant tokens; tee fires for every accepted batch. One-line comment: the tee runs on tenant installs, the token map runs on central; both features coexist harmlessly).
- [ ] **Step 4: Forwarder test** (photon-server): spin an in-process axum stub capturing POSTs; assert path `/v1/traces` for `TeeSignal::Traces`, bearer header, body passthrough; assert a stub returning 500 five times → payload dropped, `stats.dropped == 1`, loop alive (next payload delivered when stub recovers).
- [ ] **Step 5:** Wire in `main.rs`: `if let Some(fed) = &cfg.federation` and `fed.mode == FederationMode::Full` → `FederationTee::channel(fed.queue_batches)`, tee into `IngestServer`, `spawn_forwarder`. Summary pusher (Task 5) runs in BOTH modes.
- [ ] **Step 6:** `cargo test -p photon-ingest -p photon-server` PASS, `cargo clippy --all-targets` clean.
- [ ] **Step 7: Commit** `feat(ingest,server): full-mode OTLP tee + best-effort forwarder`

---

### Task 7: federation status seam + endpoint (tenant-side UI) `[sonnet-4.6]`

**Files:**
- Create: `crates/photon-api/src/federation.rs`
- Modify: `crates/photon-api/src/lib.rs` (route + `with_federation_status` builder), `crates/photon-server/src/main.rs` (impl + wiring)

**Interfaces:**
- Consumes: `FederationStats` (Task 5/6).
- Produces (photon-api seam, same shape as `ReplicationStatus` at `usage.rs:54` — photon-api cannot dep photon-server):

```rust
pub trait FederationStatus: Send + Sync {
    fn snapshot(&self) -> Option<FederationStatusSnapshot>; // None = federation disabled
}
#[derive(serde::Serialize, Clone, PartialEq, Debug)]
pub struct FederationStatusSnapshot {
    pub mode: String,            // "summary" | "full"
    pub endpoint: String,
    pub last_push_ms: i64,       // 0 = never
    pub last_error: Option<String>,
    pub pushed: u64,
    pub dropped: u64,
    pub queued: u64,
}
```

- Route `GET /api/federation/status` → `200 {snapshot}` or `200 {"enabled": false}` shape: `{"enabled": bool, "status": Option<Snapshot>}`.
- photon-server implements the trait as a newtype over `(Option<FederationConfig>, Arc<FederationStats>)` exactly like `ReplStatus` at `main.rs:168-176`.

- [ ] **Step 1: Failing handler test** with an in-memory fake `FederationStatus` (both enabled and disabled cases). **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** PASS. **Step 5: Commit** `feat(api,server): GET /api/federation/status`

---

### Task 8: central `GET /api/tenants/summary` `[sonnet-4.6]`

Curated per-tenant endpoint (the infra vertical precedent, `crates/photon-api/src/infra.rs`) — one round trip for the whole Home board instead of N `metrics/query` calls.

**Files:**
- Modify: `crates/photon-api/src/tenants.rs`, `crates/photon-api/src/lib.rs` (route)

**Interfaces:**
- Consumes: `TenantStore` (Task 3), `state.metrics_query` (`MetricSeriesRequest`/`query_series` as used by `metrics.rs:184`).
- Produces `GET /api/tenants/summary` → `Json<Vec<TenantSummary>>`:

```rust
#[derive(serde::Serialize)]
pub struct TenantSummary {
    pub name: String,
    pub mode: Option<String>,        // from the `mode` attr of photon.federation.up; None = never reported
    pub status: String,              // "up" | "stale" | "down"
    pub last_seen_ms: i64,           // 0 = never
    pub ingest_rows_per_sec: f64,    // differenced from photon.federation.ingest.rows over the window
    pub open_incidents: f64,         // latest photon.federation.incidents.open
    pub hot_bytes: f64,              // sum of latest photon.federation.disk.hot_bytes across signals
    pub ui_url: Option<String>,
    pub spark: Vec<(i64, f64)>,      // (ms, rows/sec) points for the card sparkline
}
```

Implementation: for each tenant, `query_series` on the four `photon.federation.*` metrics over the last 15 minutes with filter `tenant="{name}"` (the stamped promoted/mapped attribute), ~30 buckets. Staleness thresholds — `up` if `now - last_seen <= 120s`, `stale` <= 600s, else `down`; hardcoded consts with a comment (central doesn't know tenant intervals; revisit if configurable intervals land). Rows/sec = last-minus-first of the cumulative sum ÷ window (clamp at 0 for counter resets).

- [ ] **Step 1: Failing handler test**: fake TenantStore with 2 tenants; a fake/mock metrics engine path — reuse how `metrics.rs` handlers are tested in this crate (in-memory engine over a temp hot dir, seeded via the ingest mapping helpers); one tenant with fresh `photon.federation.up` points → `status == "up"`, correct rate; one tenant with no data → `status == "down"`, `last_seen_ms == 0`.
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** PASS. **Step 5: Commit** `feat(api): curated /api/tenants/summary for the Home board`

---

### Task 9: end-to-end federation test (central in-process) `[sonnet-4.6]`

**Files:**
- Create: `crates/photon-server/tests/federation_e2e.rs` (copy the server-boot scaffolding from `crates/photon-server/tests/e2e.rs`)

Central config for the test includes `promoted_attributes = ["service.name", "host.name", "tenant"]`.

- [ ] **Step 1: Write the test** (it drives everything already built, so it goes straight to green — the e2e is the verification layer, not TDD of new code):

```text
1. Boot a full photon-server ("central") on ephemeral ports.
2. Session-auth, POST /api/tenants {name:"divtik"} → capture minted token.
3. POST /v1/logs (OTLP protobuf) with `Authorization: Bearer <tenant token>` and a
   client-spoofed resource attr tenant="cpin".
4. Poll /api/search for the log row → assert it carries tenant="divtik" (stamp wins).
5. POST /v1/metrics with the tenant token: a photon.federation.up=1 gauge + cumulative
   ingest.rows points (built with the Task 5 builder).
6. GET /api/tenants/summary → divtik status "up", mode reported, rate >= 0.
7. POST /v1/logs with an unknown token → 401. POST /api/v1/write with the tenant token → 401.
8. POST /api/tenants/divtik/rotate-token → old token now 401 on /v1/logs, new token 2xx.
```

- [ ] **Step 2: Run** `cargo test -p photon-server --test federation_e2e` — PASS (fix whatever it flushes out).
- [ ] **Step 3: Commit** `test(server): federation e2e — stamping, summary, rotation`

---

### Task 10: frontend API client + tenants queries `[sonnet-4.6]`

**Files:**
- Modify: `frontend/src/lib/core/api.ts` (beside the rum-apps methods at `:1346-1398`)
- Create: `frontend/src/lib/tenants/tenantsQueries.ts`
- Modify: `frontend/src/lib/core/mock.ts` (mock fixtures so the mock-fallback path keeps working)

**Interfaces:**
- Produces `api` methods (each with the standard try/catch → mock-fallback wrapper, real 4xx rethrown): `tenants()`, `tenantsSummary()`, `createTenant(name, uiUrl?)`, `updateTenant(name, uiUrl)`, `rotateTenantToken(name)`, `deleteTenant(name)`, `federationStatus()`.
- Produces composables (clone `lib/rum/rumQueries.ts` conventions — `toValue` inputs, `computed` queryKey, mutations that never reject and branch on `res.ok === false` → error toast, else invalidate + success toast):

```ts
export const tenantsQueryKey = () => ['tenants']
export const tenantsSummaryQueryKey = () => ['tenants', 'summary']
export function useTenants()
export function useTenantsSummary()            // refetchInterval: 15_000
export function useCreateTenant()
export function useUpdateTenant()
export function useRotateTenantToken()
export function useDeleteTenant()
export function useFederationStatus()          // refetchInterval: 30_000
export interface TenantSummary { name: string; mode: string | null; status: 'up'|'stale'|'down';
  last_seen_ms: number; ingest_rows_per_sec: number; open_incidents: number; hot_bytes: number;
  ui_url: string | null; spark: [number, number][] }
```

- [ ] **Step 1: Failing vitest** for the composables' key builders + a mocked-api `useTenantsSummary` happy path (follow whatever pattern existing `*Queries` tests use; if none exist for queries, test the pure key builders + the TenantSummary mock fixture shape).
- [ ] **Step 2:** `cd frontend && bun run test` — FAIL. **Step 3:** Implement. **Step 4:** `bun run test` + `bun run type-check` PASS.
- [ ] **Step 5: Commit** `feat(frontend): tenants api client + query composables`

---

### Task 11: TenantCard + Home tenant board `[sonnet-4.6]`

**Files:**
- Create: `frontend/src/components/tenants/TenantCard.vue` (`<script setup lang="ts">`)
- Modify: `frontend/src/views/HomeView.vue`, `frontend/src/views/HomeView.test.ts`

**Interfaces:**
- Consumes: `useTenantsSummary()` + `TenantSummary` (Task 10); `Card`/`StatusDot`/`Sparkline`/`Badge` primitives; `HostCard` (`components/infra/HostCard.vue`) as the structural template (Card `interactive role="button" tabindex="0"`, truncated title, `mt-auto border-t` relative-time footer).
- Produces: `TenantCard` — `defineProps<{ tenant: TenantSummary }>()`, `defineEmits<{ select: [tenant: TenantSummary] }>()`. Layout: name + mode `Badge` (`summary`/`full`) + `StatusDot` (tone success/warning/error by status); rows: ingest rate, open incidents, hot bytes; `Sparkline :points="tenant.spark.map(p => p[1])"`; footer "last seen X ago". `status !== 'up'` → destructive border tint + "Unreachable — no heartbeat" line (the Arcane offline-card treatment). Summary-mode card also renders an "Open UI ↗" anchor (`tenant.ui_url`, `target="_blank"`, `@click.stop`).
- HomeView: fourth section between the KPI strip and the trend charts:

```html
<section v-if="tenants.length" data-testid="home-tenants" class="flex flex-col gap-2">
  <div class="flex items-center justify-between">
    <h3 class="text-xs font-semibold uppercase tracking-wider text-muted-foreground">Tenants</h3>
  </div>
  <div class="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
    <TenantCard v-for="t in tenants" :key="t.name" :tenant="t" @select="openTenant" />
  </div>
</section>
```

`openTenant` behavior comes in Task 12 (stub: no-op for full-mode until then, `window.open(ui_url)` for summary-mode).

- [ ] **Step 1: Failing component tests** (extend `HomeView.test.ts` + new `TenantCard.test.ts`): board hidden when summary returns `[]`; renders one card per tenant; down-tenant shows unreachable treatment; summary-mode card shows the external link.
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** `bun run test` + `bun run type-check` PASS.
- [ ] **Step 5: Commit** `feat(frontend): tenant board on Home`

---

### Task 12: tenant scope — fifth ScopeType wired into real filtering `[sonnet-4.6]`

Scope today is decorative (chip + URL only; nothing consumes it). This task makes `tenant` the first scope type that actually filters, by appending a grammar term to the `q`/`filter` string each signal already sends.

**Files:**
- Modify: `frontend/src/lib/core/context.ts` (type + helper), `frontend/src/lib/logs/logsQueries.ts`, `frontend/src/lib/traces/*` (the composables that pass `q` to the span search API), `frontend/src/lib/metrics/metricsQueries.ts` (the `filter` string in `QuerySpec`), `frontend/src/lib/logs/fields.ts` (+ `frontend/src/lib/traces/spanFields.ts`, `frontend/src/lib/metrics/metricFields.ts`: `tenant` autocomplete entry), `frontend/src/views/HomeView.vue` (`openTenant`)

**Interfaces:**
- Produces in `context.ts`:

```ts
export type ScopeType = 'service' | 'rumApp' | 'host' | 'monitor' | 'tenant'

/** Grammar term the active scope contributes to backend queries; null when scope adds no filter. */
export function scopeQueryTerm(): string | null {
  const s = scope.value
  return s?.type === 'tenant' ? `tenant:${s.id}` : null
}
```

- Each modified composable appends the term where the query string is assembled, e.g. `const effectiveQ = computed(() => [toValue(q), scopeQueryTerm()].filter(Boolean).join(' '))` — and uses `effectiveQ` in BOTH the queryKey and the request, so scope changes refetch.
- `openTenant` (HomeView): full-mode → `setScope({ type: 'tenant', id: t.name, label: t.name }); router.push(correlate({ path: '/logs' }))`; summary-mode → `window.open(t.ui_url ?? '#', '_blank')`.
- Field catalogs: `{ name: 'tenant', description: 'Federated tenant (stamped by central)', kind: 'match' }`.

- [ ] **Step 1: Failing tests**: `scopeQueryTerm` returns `tenant:divtik` for a tenant scope, null for service scope / no scope; a logs composable test asserting the request `q` contains the tenant term when scope is set (mock `api`).
- [ ] **Step 2:** FAIL. **Step 3:** Implement across the listed composables. **Step 4:** `bun run test` + `bun run type-check` PASS. Manually spot-check `bun run dev`: set a tenant scope via a Home card, confirm Logs/Traces/Metrics narrow and the ContextBar chip clears it.
- [ ] **Step 5: Commit** `feat(frontend): tenant scope filters logs/traces/metrics via query grammar`

---

### Task 13: tenants management UI (/data tab) + federation status panel `[sonnet-4.6]`

**Files:**
- Create: `frontend/src/components/data/DataTenants.vue`, `frontend/src/components/tenants/TenantManageDialog.vue`
- Modify: `frontend/src/views/DataView.vue` (`TABS = ['overview','storage','retention','tenants','delete']` + tab body), `frontend/src/components/data/DataOverview.vue` (federation status strip)

**Interfaces:**
- Consumes: Task 10 composables; `RumManageAppsDialog` (`components/rum/RumManageAppsDialog.vue`) as the structural template — list with rotate (`KeyRound`) / delete (`Trash2`) icon buttons, create `<form>` with `FormField`/`Input` (fields: name, optional UI URL), and the minted-token panel shown ONCE after create/rotate with a copy-ready `[federation]` TOML snippet:

```toml
[federation]
endpoint = "<this photon's public URL>"
token = "tk_tenant_…"
mode = "summary"   # set to "full" to mirror raw telemetry
```

- `DataTenants.vue`: table of tenants (name, redacted token, ui_url, created) + the dialog trigger; empty state "Register your first tenant".
- Federation status strip (tenant-side view of Task 7): in `DataOverview.vue`, `v-if="fed?.enabled"` panel showing mode `Badge`, endpoint, last push relative time, pushed/dropped/queued counters, `last_error` in destructive text when set. Uses `useFederationStatus()`.

- [ ] **Step 1: Failing tests**: DataView renders the tenants tab; DataTenants empty state; create flow surfaces the minted token panel (mocked mutation); status strip hidden when `enabled: false`.
- [ ] **Step 2:** FAIL. **Step 3:** Implement. **Step 4:** `bun run test` + `bun run type-check` PASS.
- [ ] **Step 5: Commit** `feat(frontend): tenants management tab + federation status panel`

---

### Task 14: docs sync `[haiku-4.5]`

**Files:**
- Create: `docs/subsystems/federation.md` (architecture: modes, token flow, stamping, tee/forwarder semantics, staleness thresholds, metric names, the promrw limitation, config reference, API routes)
- Modify: `CLAUDE.md` (crate-graph notes for the new photon-server `federation` module + photon-ingest auth change; route list; `[federation]` config; frontend tenant scope), `docs/architecture.md` (API surface: `/api/tenants*`, `/api/federation/status`; data-flow note for the tee), `docs/frontend.md` (tenant board, fifth ScopeType, /data tenants tab), `photon.example.toml` (commented `[federation]` example), `docs/conventions.md` only if a new convention emerged.

- [ ] **Step 1:** Write `federation.md` following the structure of an existing subsystem doc (e.g. `docs/subsystems/infra.md`).
- [ ] **Step 2:** Update the cross-cutting docs; re-verify internal links resolve.
- [ ] **Step 3: Commit** `docs: federation subsystem + cross-doc sync`

---

## Self-review notes

- **Spec coverage:** push-only egress (T5/T6), two modes w/ default summary (T1/T5/T6), OTLP reuse (T2/T5/T6), registry + minted tokens + server-side stamping (T2/T3/T4), best-effort bounded w/ drop counters (T5/T6), config-file mode + read-only status UI (T1/T7/T13), Home board + Arcane-style cards (T11), reuse existing views via tenant scope→`q` (T12), manage UI in /data (T13), promrw limitation documented (T2/T14), retention untouched (central retention applies automatically — mirrored data flows the normal pipeline; no task needed), full-mode also pushes summary (T5 runs in both modes, wired in T6 Step 5).
- **Type consistency:** `TenantTokenMap` (core) consumed by T2/T4; `FederationStats` (T5) written by T6, read by T7; `TenantSummary` field names match Rust serde (snake_case) in T8 and TS in T10; `FederationMode::{Summary,Full}` lowercase serde matches `PHOTON_FEDERATION_MODE` values and the UI badges.
- **Known deliberate gaps (out of scope per shared understanding):** no per-tenant retention, no central-controlled mode, no guaranteed delivery, no per-tenant RBAC, no skip-index min/max range for `tenant` (bloom + promoted column only — conservative pruning keeps correctness; add a sidecar v3 only if tenant-filtered scans get slow), Services view not tenant-filtered in v1 (spans ARE, via /traces; a `tenant` filter on the RED endpoints is a natural follow-up).
