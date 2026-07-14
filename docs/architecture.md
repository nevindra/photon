# Architecture

The authoritative map of Photon's backend. Read this before changing a crate's public surface, the
write/read path, or anything touching storage and durability.

## The shape in one breath

Photon is a **single Rust binary** = a Cargo **workspace of small crates**. The crate graph _is_ the
architecture: dependencies point one way, toward `photon-core`, and the compiler forbids cycles and
cross-boundary reach. One `photon-server` process runs the OTLP receivers, three per-signal compactor
loops, the query engine, the uptime scheduler, the background replicator, and the axum API that also
serves the embedded Vue UI.

It carries **five signals** — logs, traces (spans), metrics, uptime, and RUM — plus data
retention/purge, usage/storage accounting, live-tail SSE, and hot→durable replication.

> **Signal isolation by duplication.** Logs, spans, and metrics each get their **own** WAL, Arrow
> schema, compactor, and manifest object. This is a deliberate, documented "accepted structural cost"
> so adding a signal never destabilizes the logs path (they don't share machinery — only the
> signal-agnostic `Wal`/`DiskWal` type and the streaming/fsync helpers in
> `photon-compact/src/stream.rs`). **RUM and uptime add no new storage engine:** RUM reuses the
> metrics + logs machinery (Web Vitals → gauge metrics, JS errors → logs); uptime is a self-contained
> SQLite vertical.

## The crate graph

```
photon-core   ← leaf: domain types only, no I/O. Everything depends on it; it depends on nothing internal.
  ↑  ↑  ↑  ↑
  │  │  │  └── photon-wal      → core        group-commit WAL (the durability boundary)
  │  │  └───── photon-index    → core        skip-index format (bloom + min/max), pure
  │  └──────── photon-storage  → core        hot + durable object stores + background replicator
  └─────────── photon-uptime   → core        self-contained SQLite monitoring vertical

photon-compact → core, wal, index, storage   WAL segment → sorted Parquet + skip index → manifest
photon-query   → core, index, storage        manifest + skip-index pruning, then DataFusion
photon-ingest  → core, wal                    OTLP gRPC + HTTP receivers, OTLP→record mapping
photon-api     → core, query, uptime          axum REST + session auth + embedded Vue UI
photon-loadgen → ingest                        dev-only OTLP load generator
photon-agent   → (standalone, no internal deps) host/GPU sampler, OTLP/HTTP client
photon-server  → all of the above             the binary: config, wiring, task supervision
```

**Notable edges:** `photon-api` does **not** depend on `photon-wal` — so it defines **trait seams**
(`RumSink`, `UsageStore`, `ReplicationStatus`, `DataAdmin`, `UserStore`) that `photon-server`
implements over the real WALs. `photon-query` does **not** depend on `photon-compact`/`photon-wal` at
runtime (dev-dep only). `photon-loadgen` depends only on `photon-ingest` (reuses its OTLP mapping).
`photon-server` is the sole crate that touches every layer.

## Crate reference

| Crate | Purpose | Key public types |
|---|---|---|
| **photon-core** | Shared domain types, no I/O. Owns `PhotonError` (one enum, a variant per crate). Modules per signal: logs (`schema.rs`, `record.rs`), spans (`span_schema.rs`, `span_record.rs`), metrics (`metric_schema.rs`, `metric_record.rs`, `metric_agg.rs`), RUM (`rum.rs`), plus `manifest.rs`, `segment.rs`, `config.rs`, `retention.rs`, `ingest_counters.rs`. | `PhotonError`, `LogRecord`, `Span*`, `MetricPoint`, `Manifest`, `SegmentId`, `Config` |
| **photon-wal** | Durable WAL with group commit; `append` resolves only after the coalesced `fsync` covering its bytes (the ack boundary). Signal-agnostic — one type backs the logs, spans, and metrics WALs. | trait **`Wal`** (append/sync/list_closed_segments/read_segment/remove_segment); `DiskWal`; `BroadcastingWal` (fans appends to live-tail SSE) |
| **photon-index** | Pure/sync **skip index — a bloom filter + min/max ranges, NOT an inverted index.** Per-file variants for logs (bloom over tokenized `body`), spans (bloom over `name` + whole `trace_id`), metrics (bloom over whole `metric_name`). Binary `.idx` sidecar. | struct **`SkipIndex`** (not a trait), `tokenize` |
| **photon-storage** | Hot (always local) + optional durable S3-compatible object store; owns the per-segment object-path scheme; background hot→durable replicator that flips a manifest entry's `durable=true` after upload. | **`Storage`**, **`Replicator`** (concrete types — there is no `BlobStore` trait) |
| **photon-compact** | Drains closed WAL segments → sorted zstd Parquet + skip-index sidecar → manifest → enqueue replication; `merge_once` consolidates small files; `purge_before` enforces retention. **Three parallel compactors, one per signal.** | `Compactor` (logs), `SpanCompactor`, `MetricsCompactor` |
| **photon-query** | Prune (manifest time overlap + skip-index bloom/min-max — our code) then read only surviving **local** Parquet with DataFusion 43. **Three engines, one per signal.** | `QueryEngine` (logs), `SpanQueryEngine` (traces), `MetricsQueryEngine` (metrics + RUM vitals) |
| **photon-ingest** | OTLP receivers for all three write signals: gRPC (`LogsService`/`TraceService`/`MetricsService`) + HTTP (`/v1/logs`, `/v1/traces`, `/v1/metrics`); plus a Prometheus remote-write 1.0 receiver (`POST /api/v1/write`, snappy+protobuf → the metrics WAL). Holds three WALs + three schemas sharing one bearer token; per-signal `max_in_flight` semaphore. | `IngestServer`; pure cores `otlp_logs_to_records`, `otlp_traces_to_spans`, `otlp_metrics_to_points` |
| **photon-api** | axum REST + embedded Vue UI + argon2 signed-cookie sessions. `ApiServer` holds the three query engines + optional builder-attached subsystems (`with_uptime`, `with_data_admin`, `with_live_hub`, `with_usage`, `with_rum`) — each `None` ⇒ its routes 404. | `ApiServer`; trait seams `RumSink`, `RumAppStore`, `UserStore`, `UsageStore`, `ReplicationStatus`, `DataAdmin` |
| **photon-server** | The single binary. Wires everything; spawns ingest, **three** compactor loops, the usage sampler, the uptime scheduler + retention, the replicator flush, and builds `ApiServer`. Also `hash-password <pw>` and `healthcheck` subcommands. | `main` |
| **photon-loadgen** | Standalone OTLP/HTTP load generator (`logs`/`traces`/`metrics` subcommands): steady-rate soak or max-concurrency ceiling, reporting throughput/ack-latency. | `main` |
| **photon-agent** | Standalone host/GPU resource-metrics agent (its own binary, like `photon-loadgen`): samples the host (`sysinfo`) + NVIDIA GPU (`nvml-wrapper`, `gpu` feature default-on, graceful fallback when no driver is present), maps to OTLP `system.*`/`system.gpu.*` metrics tagged `host.name`/`host.id`/`os.type`, and POSTs them to `/v1/metrics`. Not compiled by `cargo build -p photon-server`. | `main` |
| **photon-uptime** | Self-contained active-monitoring vertical: schedules HTTP(S)/TCP/ICMP probes, records up/down + latency to embedded SQLite, tracks incidents, fires webhook alerts. | trait **`UptimeStore`**; `probe`/`scheduler`/`state`/`notify` |

> **Trait seams vs. structs — get the names right.** The real trait boundaries are `Wal`,
> `UptimeStore`, and the photon-api seams `RumSink`/`RumAppStore`/`UserStore`/`UsageStore`/
> `ReplicationStatus`/`DataAdmin`. `SkipIndex` is a **struct**, and storage exposes concrete
> `Storage` + `Replicator` — **there is no `BlobStore` trait** (older docs claimed one; they are
> wrong).

## Write path (per signal)

All three write signals share the same **shape** but fully separate machinery (separate WAL, schema,
compactor, manifest). Merged-compaction segment IDs use the top bit so they can't collide with
WAL-allocated IDs.

| Signal | Receiver | WAL | Compactor sort key | Parquet dir | Skip index |
|---|---|---|---|---|---|
| **Logs** | `LogsService` / `POST /v1/logs` | `wal` | `(service.name, timestamp)` | `data/` | bloom over tokenized `body` + min/max ts/service |
| **Traces** | `TraceService` / `POST /v1/traces` | `spans_wal` | `(service.name, start_time)` | `data-spans/` | bloom over `name` + whole `trace_id` |
| **Metrics** | `MetricsService` / `POST /v1/metrics` | `metrics_wal` | `(metric_name, service.name, host.name, timestamp)` | `data-metrics/` | bloom over whole `metric_name` + `host.name` min/max range (binary v2; see [`subsystems/infra.md`](subsystems/infra.md)) |

The flow (identical per signal): **OTLP request → bearer-token check + per-signal semaphore →
`otlp_*_to_*` mapping → Arrow `RecordBatch` → `wal.append`** (the group-commit `fsync` completes =
**the ack / durability boundary**; data survives a crash from here) **+ `IngestCounters` bump →**
the signal's background compactor `run_once` drains each _closed_ WAL segment → concat + lexsort by
the sort key → stream one zstd Parquet + build the skip-index `.idx` sidecar → append a
`FileEntry(durable=false)` to that signal's manifest → enqueue an async hot→durable replicate → a
lower-frequency `merge_once` consolidates small files → `purge_before(cutoff)` enforces retention.

**Crash-consistency recipe** (all three compactors, `photon-compact/src/stream.rs`): stream one zstd
Parquet to a temp file → `fsync` → atomic rename → `fsync` parent dir → pin the just-saved manifest
→ **only then** remove the WAL segment. A crash mid-compaction just redoes that segment on restart —
safe because segment IDs make it idempotent.

**RUM write path (no new storage):** browser `POST /api/rum` beacon → `photon-core::rum` maps Web
Vitals → gauge `MetricPoint`s and JS errors → `LogRecord`s → the `RumSink` (implemented in
photon-server over the metrics + logs `BroadcastingWal`s). Vitals land in the metrics store, errors
in the logs store. **Uptime write path (separate engine):** scheduler → probe results/incidents →
embedded SQLite via `UptimeStore` (no WAL/Parquet). See [`subsystems/rum.md`](subsystems/rum.md) and
the per-feature docs in [`subsystems/`](subsystems/).

## Read / query path

`photon-query` prunes cheapest-first, then hands the survivors to DataFusion:

1. **Manifest** selects candidate files by time overlap and per-file promoted-column value ranges.
2. **Skip index** (`.idx` sidecar) prunes further by min/max ranges and token bloom membership.
3. **DataFusion** columnar-scans only the survivors, predicates pushed down. Free-text log search is
   bloom-pruned then confirmed with a `strpos(body, text) > 0` substring scan — **there is no
   inverted index.**

Per-signal engines:

- **Logs — `QueryEngine`:** `search`/`search_with_count` (two-pass late materialization),
  `count_matching`, `facet`, `fields` (manifest-only catalog: `fixed`|`promoted`|`attribute`),
  `histogram` (severity-stacked), `distinct_services`, `storage_stats`, raw `sql`, and RUM
  `rum_errors` (fingerprint-grouped error issues).
- **Traces — `SpanQueryEngine`:** `get_trace` (waterfall), `search_traces` (rolled-up summaries),
  `search_spans`, `count_matching_spans`, span `facet`/`fields`/`histogram`, `latency` (t-digest
  percentiles + log-scale histogram), `red_metrics` + `red_timeseries` (RED + Apdex bands),
  `dependencies` (DB/external downstream rollups).
- **Metrics — `MetricsQueryEngine`:** `query_series` (gauge avg/min/max/sum/count/last, reset-aware
  rate/increase), distribution quantiles (explicit/exponential Histogram + Summary), Prometheus
  classic-histogram query-time reassembly (`le`-bucket → quantile, reusing `hist_ranges`/
  `interpolate_quantile`/`reset_aware_series`), catalog/metadata/labels discovery, and RUM `rum_vitals`
  + `rum_breakdown` (Web-Vitals p75 + rating distribution).

## Storage & durability model

Two `object_store` backends with strict role separation:

- **Hot store = local filesystem (primary).** The WAL, compacted Parquet, `.idx` sidecars, and the
  manifests all live here. **All reads and all compaction writes go here — no network hop on the
  query or ack path.** The query engine reads straight off the local filesystem and does **not**
  register an object_store with DataFusion.
- **Durable store = S3-compatible (Garage recommended), asynchronous replica.** Optional
  (`[storage.durable]`; omit to run hot-tier-only). Every finalized Parquet + skip index is uploaded
  in the background. This is what survives **local-disk failure**. Never on the ack or hot-query path.

| Failure | Recovered by |
|---|---|
| Process crash / restart | WAL replay of segments not yet confirmed compacted (idempotent by segment ID). |
| Local disk failure | Re-fetch Parquet + index from the durable store per the manifest onto a fresh disk. |

**Ack boundary = the local WAL `fsync`, not the durable upload** — this keeps ingest fast and
independent of object-storage latency. The compactor uploads promptly to keep the
accepted-but-not-yet-replicated window small; replication lag is an exposed metric. The **cold-tier
read path (fetch evicted files from durable on query) is deliberately unimplemented** — at the 30-day
target the hot window fits on local NVMe.

A shared **SQLite control-plane DB** (`[storage].db_path`) holds UI user accounts, uptime monitors,
and the RUM app registry (`rum_apps` table) — always used, required for login and by the always-on
uptime subsystem. RUM apps are registered/edited/rotated/removed at runtime (UI or the
`/api/rum/apps*` management API — see [`subsystems/rum.md`](subsystems/rum.md)), not via config;
there is no `[[rum.apps]]` config surface anymore.

## API surface

Handlers live in `crates/photon-api/src/*.rs`; the aggregation logic they call lives in
`photon-query`. Every route except the open ones requires the signed `photon_session` cookie.

- **Ingest (gRPC + HTTP, bearer token):** gRPC `POST /v1/logs`, `/v1/traces`, `/v1/metrics` (OTLP); HTTP `POST /v1/logs`, `/v1/traces`, `/v1/metrics` (OTLP), `/api/v1/write` (Prometheus remote-write 1.0, snappy+protobuf). All share `[ingest].token`.
- **Open (no session):** `POST /api/login`, `POST /api/setup` (first-run), `GET /api/session` (boot).
- **Public browser beacon (per-app key + Origin/CORS, no cookie):** `POST /api/rum`.
- **Logs:** `GET /api/services`, `POST /api/search`, `GET /api/fields|facet|histogram`.
- **Live tail (SSE):** `GET /api/stream/logs`, `GET /api/stream/spans`.
- **Traces / spans:** `GET /api/traces/:trace_id`, `POST /api/traces/search`, `POST /api/spans/search`,
  `GET /api/traces/fields|facet|histogram|latency`.
- **Services (APM):** `GET /api/red`, `GET /api/services/:service/timeseries|dependencies|settings`.
- **Metrics:** `POST /api/metrics/query`, `GET /api/metrics/catalog|metadata/:name|labels`.
- **Infrastructure (host/GPU resource monitoring):** `GET /api/infra/hosts`,
  `GET /api/infra/hosts/:host`, `GET /api/infra/hosts/:host/timeseries` — see
  [`subsystems/infra.md`](subsystems/infra.md).
- **RUM (read):** `GET /api/rum/apps|vitals|vitals/breakdown|pages|pages/detail|errors|errors/facets|errors/:fingerprint`
  (`errors` and `errors/facets` accept an optional `q` log-grammar filter, same syntax as Logs search;
  `GET /api/rum/apps` now returns the full app registry records, including the public `key`).
- **RUM (app registry, session-authed):** `POST /api/rum/apps` (register; server mints the key),
  `PATCH /api/rum/apps/:name` (update origins/sampling/rate limit), `POST
  /api/rum/apps/:name/rotate-key` (mint a fresh key), `DELETE /api/rum/apps/:name` (unregister).
- **Uptime:** `GET/POST /api/monitors`, `GET/PATCH/DELETE /api/monitors/:id`,
  `POST /api/monitors/:id/pause|resume`, `GET /api/monitors/:id/heartbeats|incidents`.
- **Data / usage / retention:** `GET /api/storage`, `GET /api/usage/series`, `GET/PUT /api/retention`,
  `POST /api/data/purge`.
- **Auth / users:** `POST /api/logout`, `GET/POST /api/users`, `DELETE /api/users/:username`.

Everything else falls through to the embedded UI (SPA fallback to `index.html`).

**Response compression:** a `tower-http` `CompressionLayer` wraps the whole HTTP surface (JSON API +
embedded UI bundle), content-negotiating **gzip/br** from the client's `Accept-Encoding` (adds
`Vary: Accept-Encoding`; transparent — no frontend change). JSON logs compress ~15x (a 500-row
`/api/search` payload of ~115 KB → ~8 KB), so this is the primary transfer win, not a wire-format
change. The default predicate skips SSE (`/api/stream/*` live-tail), gRPC, images, and sub-32-byte
bodies, so streaming is never buffered.

## Load-bearing invariants — do not break

- **No inverted index.** Pruning is min/max stats + token bloom filters (kilobytes/file). Bloom
  filters may false-**positive** (extra scan) but **never false-negative** — pruning can only add
  work, never drop a real result. This is a property test; keep pruning conservative (a missing
  `.idx` or an unknown range means _keep the file_).
- **Local disk is the primary store**; the S3 durable store is an async replica, never on the ack or
  query path. Ingest acks after the local WAL `fsync`, not after durable upload.
- **Rows are sorted by the signal's sort key before Parquet encoding** (logs `(service.name,
  timestamp)`, spans `(service.name, start_time)`, metrics `(metric_name, service.name, host.name,
  timestamp)`) — this is what makes min/max pruning effective and compresses well. The compactor's
  lexsort order _is_ the query engine's pruning contract. `service.name` is promoted and is the logs
  primary sort key — it cannot be un-promoted. The metrics skip index additionally ranges over
  `host.name` (binary sidecar format v2; a missing host block in an older v1 sidecar decodes as
  `None`, which — per the no-false-negative rule below — keeps the file rather than pruning it).
- **`search` is two-pass (late materialization).** Pass 1 applies the predicate but projects only
  `timestamp`, sorts DESC, takes `limit`, finds the cutoff; pass 2 re-runs with `timestamp >= cutoff`
  and returns full rows — so the wide `attributes` map is decoded only for survivors. Don't collapse
  it to one pass (it regresses broad-window, low-selectivity searches ~5×). Spans use the same trick
  for `SpanSort::Recent`.
- **Per-signal separation is enforced, not incidental** — separate WALs, separate manifest objects
  (so signals never write-race), and top-bit merge-segment IDs.
- **`PhotonError` is never edited** (all variants pre-declared) to avoid parallel-dev merge races.
- **Dependency co-pinning** — see [`conventions.md`](conventions.md).

## The log query grammar

The search bar speaks a small query language defined **once** in `photon-core/src/query/`
(`parse` → AST → `FieldResolver` → `eval`; pure, no DataFusion) and compiled two ways: to an
in-memory predicate (`eval`) and to a DataFusion filter (in `photon-query`) — one source of truth for
filter semantics. Syntax: `field:value` (exact), `field:v1,v2` (OR-list), `field:*` (exists),
`-field:v` (negate), `field>=n` / `>` / `<` / `<=` (numeric compare), and `"quoted text"` or bare
words (case-sensitive body substring). Terms are AND-ed; parse errors carry a byte `offset` so the UI
can underline the bad token. The frontend has a display-only mirror (`lib/queryLang.js`) that never
validates.
