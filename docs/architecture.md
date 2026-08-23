# Architecture

The authoritative map of Photon's backend. Read this before changing a crate's public surface, the
write/read path, or anything touching storage and durability.

## The shape in one breath

Photon is a **single Rust binary** = a Cargo **workspace of small crates**. The crate graph _is_ the
architecture: dependencies point one way, toward `photon-core`, and the compiler forbids cycles and
cross-boundary reach. One `photon-server` process runs the OTLP receivers, three per-signal compactor
loops, the query engine, the uptime scheduler, the alert-engine evaluation loop, the background
replicator, and the axum API that also serves the embedded Vue UI.

It carries **five signals** — logs, traces (spans), metrics, uptime, and RUM — plus data
retention/purge, usage/storage accounting, live-tail SSE, hot→durable replication, and a
cross-signal webhook **alert engine**.

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
  ├─────────── photon-uptime   → core        self-contained SQLite monitoring vertical
  └─────────── photon-alerts   → core        system-wide webhook alert engine (SQLite rules/channels)

photon-compact → core, wal, index, storage   WAL segment → sorted Parquet + skip index → manifest
photon-query   → core, index, storage        manifest + skip-index pruning, then DataFusion
photon-ingest  → core, wal                    OTLP gRPC + HTTP receivers, OTLP→record mapping
photon-api     → core, query, uptime, alerts  axum REST + session auth + embedded Vue UI
photon-loadgen → ingest                        dev-only OTLP load generator
photon-agent   → (standalone, no internal deps) host/GPU sampler, OTLP/HTTP client
photon-server  → all of the above             the binary: config, wiring, task supervision
```

**Notable edges:** `photon-api` does **not** depend on `photon-wal` — so it defines **trait seams**
(`RumSink`, `UsageStore`, `ReplicationStatus`, `DataAdmin`, `UserStore`) that `photon-server`
implements over the real WALs. `photon-query` does **not** depend on `photon-compact`/`photon-wal` at
runtime (dev-dep only). `photon-loadgen` depends only on `photon-ingest` (reuses its OTLP mapping).
`photon-alerts` does **not** depend on `photon-query` — it defines a `ConditionSource` trait seam
that `photon-server` implements (`EngineConditionSource`, `crates/photon-server/src/alerts_source.rs`)
over the three query engines, the same seam pattern as `Wal`/`UptimeStore`/`RumSink`.
`photon-server` is the sole crate that touches every layer.

## Crate reference

| Crate | Purpose | Key public types |
|---|---|---|
| **photon-core** | Shared domain types, no I/O. Owns `PhotonError` (one enum, a variant per crate). Modules per signal: logs (`schema.rs`, `record.rs`), spans (`span_schema.rs`, `span_record.rs`), metrics (`metric_schema.rs`, `metric_record.rs`, `metric_agg.rs`), RUM (`rum.rs`), plus `manifest.rs`, `segment.rs`, `config.rs`, `retention.rs`, `ingest_counters.rs`. | `PhotonError`, `LogRecord`, `Span*`, `MetricPoint`, `Manifest`, `SegmentId`, `Config` |
| **photon-wal** | Durable WAL with group commit; `append` resolves only after the coalesced `fsync` covering its bytes (the ack boundary). Signal-agnostic — one type backs the logs, spans, and metrics WALs. | trait **`Wal`** (append/sync/list_closed_segments/read_segment/remove_segment); `DiskWal`; `BroadcastingWal` (fans appends to live-tail SSE) |
| **photon-index** | Pure/sync **skip index — a bloom filter + min/max ranges, NOT an inverted index.** Per-file variants for logs (bloom over tokenized `body`), spans (bloom over `name` + whole `trace_id`), metrics (bloom over whole `metric_name`). Binary `.idx` sidecar. | struct **`SkipIndex`** (not a trait), `tokenize` |
| **photon-storage** | Hot (always local) + optional durable S3-compatible object store; owns the per-segment object-path scheme; a single long-lived background `Replicator` drain loop that copies finalized objects hot→durable AND applies retention deletes to durable (one FIFO queue of upload/delete ops, bounded in-flight, retry + re-enqueue on failure); also a startup `*.tmp` sweep. | **`Storage`**, **`Replicator`** (concrete types — there is no `BlobStore` trait) |
| **photon-compact** | Drains closed WAL segments → sorted zstd Parquet (`[storage] zstd_level`) + skip-index sidecar → manifest (`FileEntry.bytes` recorded at write time) → enqueue replication; `merge_once` consolidates small files (bounded per pass); `purge_before` enforces retention; both enqueue durable deletes for unlinked objects. **Three parallel compactors, one per signal.** | `Compactor` (logs), `SpanCompactor`, `MetricsCompactor` |
| **photon-query** | Prune (manifest time overlap + skip-index bloom/min-max — our code) then read only surviving **local** Parquet with DataFusion 43. **Three engines, one per signal.** | `QueryEngine` (logs), `SpanQueryEngine` (traces), `MetricsQueryEngine` (metrics + RUM vitals) |
| **photon-ingest** | OTLP receivers for all three write signals: gRPC (`LogsService`/`TraceService`/`MetricsService`) + HTTP (`/v1/logs`, `/v1/traces`, `/v1/metrics`); plus a Prometheus remote-write 1.0 receiver (`POST /api/v1/write`, snappy+protobuf → the metrics WAL). Holds three WALs + three schemas sharing one bearer token; per-signal `max_in_flight` semaphore. Both front doors accept **gzipped** requests (HTTP `RequestDecompressionLayer`; gRPC `accept_compressed(Gzip)` — a stock OTel Collector gzips by default) and enforce one shared `max_body_bytes` cap on the *decompressed* size (HTTP `DefaultBodyLimit`, gRPC `max_decoding_message_size`, snappy remote-write decompress cap). | `IngestServer`; pure cores `otlp_logs_to_records`, `otlp_traces_to_spans`, `otlp_metrics_to_points` |
| **photon-api** | axum REST + embedded Vue UI + argon2 signed-cookie sessions. `ApiServer` holds the three query engines + optional builder-attached subsystems (`with_uptime`, `with_data_admin`, `with_live_hub`, `with_usage`, `with_rum`, `with_alerts`) — each `None` ⇒ its routes 404. | `ApiServer`; trait seams `RumSink`, `RumAppStore`, `UserStore`, `UsageStore`, `ReplicationStatus`, `DataAdmin` |
| **photon-server** | The single binary. Wires everything; spawns ingest, **three** compactor loops, the usage sampler, the uptime scheduler + retention, the alert-engine scheduler (`spawn_alerts`, before `spawn_uptime` so both share one `AlertStore`), the replicator flush, and builds `ApiServer`. Also `hash-password <pw>` and `healthcheck` subcommands. | `main` |
| **photon-loadgen** | Standalone OTLP/HTTP load generator (`logs`/`traces`/`metrics` subcommands): steady-rate soak or max-concurrency ceiling, reporting throughput/ack-latency. | `main` |
| **photon-agent** | Standalone host/GPU resource-metrics agent (its own binary, like `photon-loadgen`): samples the host (`sysinfo`) + NVIDIA GPU (`nvml-wrapper`, `gpu` feature default-on, graceful fallback when no driver is present), maps to OTLP `system.*`/`system.gpu.*` metrics tagged `host.name`/`host.id`/`os.type`, and POSTs them to `/v1/metrics`. Not compiled by `cargo build -p photon-server`. | `main` |
| **photon-uptime** | Self-contained active-monitoring vertical: schedules HTTP(S)/TCP/ICMP probes, records up/down + latency to embedded SQLite, tracks incidents, fires webhook alerts (and, via `photon-server`'s `UptimeAlertBridge`, bridges each transition onto the shared alerts incident store + channels). | trait **`UptimeStore`**; `probe`/`scheduler`/`state`/`notify` |
| **photon-alerts** | System-wide webhook alert engine: per-signal rules (metrics/logs/traces/RUM) evaluated on a near-real-time loop against a `ConditionSource` seam, a pure `Ok→Pending→Triggered` state machine per (rule, series), SQLite-backed rules/channels/incidents, and generic-JSON webhook delivery (optional HMAC). See [`subsystems/alerts.md`](subsystems/alerts.md). | trait **`ConditionSource`**, **`AlertStore`**; `state::apply`, `scheduler::run`, `notify::WebhookNotifier` |

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

The flow (identical per signal): **OTLP request → (gzip-decompress if `Content-Encoding: gzip`) +
`max_body_bytes` cap on the decompressed body → bearer-token check (local or tenant) + tenant stamping +
per-signal semaphore → `otlp_*_to_*` mapping → Arrow `RecordBatch` → optional federation tee
(non-blocking, full-mode only) → `wal.append`** (the group-commit `fsync` completes =
**the ack / durability boundary**; data survives a crash from here) **+ `IngestCounters` bump →**
the signal's background compactor `run_once` drains each _closed_ WAL segment → concat + lexsort by
the sort key → stream one zstd Parquet (level from `[storage] zstd_level`) + build the skip-index
`.idx` sidecar → append a `FileEntry` (carrying the Parquet's on-disk `bytes`, so `storage_stats` is
manifest arithmetic rather than one `stat()` per file per sampler tick) to that signal's manifest →
enqueue an async hot→durable replicate → a lower-frequency `merge_once` consolidates small files
**toward `MERGE_TARGET_ROWS` (150k rows)**, oldest-first so outputs stay time-adjacent and prune
well, taking files until either that target or `MERGE_MAX_FILES_PER_PASS` is reached (**bounded per
pass**, so a merge can't hold the whole small-file union in RAM). The target is an _output_ size,
not an input filter — the earlier 10k-row input threshold made merging self-limiting (a pass's own
output immediately counted as "large" and froze), which let the file population grow without bound
under steady ingest →
`purge_before(cutoff)` enforces retention. Both `merge_once` and `purge_before` also **enqueue async
durable deletes** for the objects they unlink from hot, so the durable replica honors retention
instead of accumulating every file ever written. (An empty/torn-tail segment compacts to zero rows
and is dropped without a manifest entry; orphaned `*.tmp` files from a crash mid-write are swept at
startup.)

**Crash-consistency recipe** (all three compactors, `photon-compact/src/stream.rs`): stream one zstd
Parquet to a temp file → `fsync` → atomic rename → `fsync` parent dir → pin the just-saved manifest
→ **only then** remove the WAL segment. A crash mid-compaction just redoes that segment on restart —
safe because segment IDs make it idempotent.

**RUM write path (no new storage):** browser `POST /api/rum` beacon → `photon-core::rum` maps Web
Vitals → gauge `MetricPoint`s and JS errors → `LogRecord`s → the `RumSink` (implemented in
photon-server over the metrics + logs `BroadcastingWal`s). Vitals land in the metrics store, errors
in the logs store. **Uptime write path (separate engine):** scheduler → probe results/incidents →
embedded SQLite via `UptimeStore` (no WAL/Parquet).

**Service-rollup write path (read engine → write path, no new storage):** every `[rollup].interval_secs`,
`photon-server/src/rollup/` runs `SpanQueryEngine::red_rollup` over the spans in one settled 60s
bucket and appends the result to the **metrics** WAL as ordinary `photon.service.*` (per service)
and `photon.operation.*` (per operation) `MetricPoint`s — the only place a query engine feeds a
write path. Two invariants ride on it: each bucket is published **exactly once**, guarded by a
watermark in the control-plane SQLite DB (the metrics reader SUMS the points in a bucket, it does
not de-duplicate, so a re-emitted bucket multiplies every counter); and the operation level is
bounded by folding all but the busiest `top_operations` per `(service, environment)` into one
`__other__` series **inside the GROUP BY**, so the aggregate can never build more groups than that
cap allows. See [`subsystems/rollup.md`](subsystems/rollup.md).

See [`subsystems/rum.md`](subsystems/rum.md) and the per-feature docs in
[`subsystems/`](subsystems/).

## Read / query path

`photon-query` prunes cheapest-first, then hands the survivors to DataFusion:

1. **Manifest** selects candidate files by time overlap and per-file promoted-column value ranges.
2. **Skip index** (`.idx` sidecar) prunes further by min/max ranges and token bloom membership.
3. **DataFusion** columnar-scans only the survivors. Free-text log search is
   bloom-pruned then confirmed with a `strpos(body, text) > 0` substring scan — **there is no
   inverted index.**

**Row-level Parquet filter pushdown is OFF** (`datafusion.execution.parquet.pushdown_filters`,
`photon-query/src/lib.rs`), and it must stay off. Every signal is read with an *explicit* schema
(`ParquetReadOptions::schema`) because it has to be: changing `[schema].promoted_attributes` changes
the columns new segments are written with, so for one retention period a query legitimately spans
two layouts. In DataFusion 43, pushdown over a file set whose physical schemas do not all match the
declared schema makes any plan carrying a `fetch` (a `LIMIT`) return **zero rows** — not an error,
not a partial answer, an empty result. Measured on the 120M-span replica across four surviving
metrics files, two written before a promotion and two after: the same filtered frame reported 29,819
rows to `count()` and 0 rows to `LIMIT 1`.

An aggregate or a sort between the filter and the limit hides it, because the fetch then never
reaches the scan — which is why only three call sites in `photon-query` ever showed it
(`metric_meta_probe`, `service_map`, `service_dependencies`) and why the symptom was "this metric
does not exist" rather than a failure. The counters on a Backend page stayed right while its
percentiles vanished. `a_fetch_plan_over_mixed_schema_files_still_returns_rows` (`lib.rs`) is the
regression test; the flag can go back on when the bug is fixed upstream.

Every engine's `SessionContext` (the shared `session()` factory in `photon-query/src/lib.rs`) draws on
**one process-wide, bounded DataFusion memory pool** (`FairSpillPool`) — a single `Arc<RuntimeEnv>`
built lazily via `OnceLock` and shared by every session (only
each session's `SessionConfig` / Parquet tuning is freshly built). So the ceiling is an **aggregate cap
across ALL concurrent queries**, not a per-query allowance: opening the UI dashboard fans out ~10
queries at once, and a fresh pool per session would make that budget additive and effectively
unbounded (N × pool). Its job is to keep the remaining intrinsically-unbounded query path — a
facet `GROUP BY value` on a high-cardinality field (holds every distinct value in a hash table before
the `LIMIT`) — from OOM-killing a low-memory single node. As a `FairSpillPool` it does this two ways:
a heavy **spillable** operator (sort, grouped aggregation) that exceeds its fair share **degrades to
disk-spill** (temp files in the OS temp dir — the runtime keeps DataFusion's default `NewOs`
`DiskManager`) rather than erroring, while a truly-**unspillable** operator that overflows the
remaining budget still **fails loud** (`ResourcesExhausted`) — the pool stays the final OOM guard.
The metrics **pointwise** path (`query_series_pointwise` in `metric_query.rs` —
cumulative reset-aware rate/increase, gauge `last`) and the metrics **distribution** scans
(`metric_dist.rs`'s `collect_dist_series`, shared by the histogram/exp-histogram/summary series
queries) are both narrower: each applies `.limit(0, Some(MAX_COLLECT_ROWS + 1))` after the sort (so a
truncation keeps a deterministic sort-order-leading prefix, not an arbitrary one), capping the row
collect itself at 200,000 rows and flagging the existing `capped` response field on overflow — so
neither depends on this pool as its only backstop. One honesty note on `capped`: the single series
straddling the row-cap boundary keeps only its time-ascending prefix, so *its own value* (a stale
`last`, an under-counted `rate`/histogram) can be inaccurate — `capped` covers that too, not just
"fewer series shown". Sharing one pool bounds the process; making it a **fair-spill** pool keeps that
bound from turning into cross-query starvation: the budget is divided evenly among concurrent
spillable consumers (`(pool_size - unspillable) / num_spillers`), so one expensive query can't
monopolize it and push unrelated ones into `ResourcesExhausted` — it hits its own fair-share ceiling
and spills to disk instead.

**Sizing it.** `photon-server` sets the pool once at startup, before any engine is built, from
`[query].memory_pool_bytes` (`photon_core::config::QueryConfig`) — omit it and the size is a quarter
of total RAM read from `/proc/meminfo`, clamped to 256 MiB..=4 GiB, falling back to the floor where
RAM cannot be read. It used to be a hard-coded 512 MiB regardless of the host, which is the wrong
shape for a number covering a whole machine's read path: generous on a 2 GB VM, absurd on a 30 GB
server, where a 7-day histogram query failed with "Resources exhausted" while the box had 21 GB
free. `photon_query::set_memory_pool_bytes` is a setter rather than a `session()` parameter on
purpose — a per-call parameter would let each caller mint its own budget, which is exactly what one
shared pool exists to prevent. Nothing calls it in tests, so they keep the old 512 MiB constant.

Pool exhaustion is a **500, not a 400.** `engine_query_failure` (`photon-api/src/search.rs`) splits
engine errors: a rejected request (unsupported aggregation, `rate` on a gauge, an unknown group-by
column) is the caller's fault and stays 400, while an I/O failure or `Resources exhausted` is ours.
`/api/red` and `/api/metrics/query` used to answer both with 400, which is not a cosmetic mislabel —
a 400 says "your query is malformed", so a server out of query memory looked exactly like a user
typo, and nothing in an access log or an uptime check told the two apart.

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
  in the background by a single long-lived drain loop (bounded in-flight; failed items are
  re-enqueued and retried, never silently dropped). Superseded (merge input) and expired (purge)
  objects are **deleted from durable** through that same queue (NotFound-tolerant), so durable-tier
  retention is enforced. This is what survives **local-disk failure**. Never on the ack or
  hot-query path.
- **`[storage] zstd_level`** (optional, default `1`, validated range `1..=19`) tunes the zstd
  compression level all three compactors pass to `write_parquet_streamed` (`photon-compact/src/
  stream.rs`) when streaming a compacted/merged Parquet file to disk. **Higher is not reliably
  smaller** — measured on a production corpus, level 3 came out ~3% *larger* than level 1; the
  next level that actually pays is 6 (~−9 pp on spans, ~2.7× the compaction CPU), and 19 costs
  ~20× the CPU. Raise it only with a measurement on your own data.
- **Per-column Parquet encodings** are chosen in `writer_properties` (`photon-compact/src/
  stream.rs`) from the Arrow type, not configured. parquet-rs defaults every column to
  dictionary-first, which is right for the low-cardinality string/enum columns and wrong for two
  shapes that dominate these files: the `Timestamp`/`Int64` time columns (all-distinct, so the
  dictionary spills to a flat 8 bytes/row that zstd barely dents) and the random hex ids
  (`trace_id`/`span_id`/`parent_span_id`, where the dictionary is as large as the data it
  indexes). Those get `DELTA_BINARY_PACKED` / `DELTA_BYTE_ARRAY` with the dictionary off — on a
  production corpus, −18% on logs and −11.5% on spans at equal or lower encode CPU. The time-column
  half is **signal-scoped** via `TimeEncoding`: logs and spans sort time last within a service run
  so it climbs monotonically, but metrics sorts it last of *four* keys and one scrape stamps many
  rows with the identical instant — a repetitive column where the dictionary is already optimal and
  forcing delta measured 2.4% *larger*. `Int32` (the `severity_number`/`kind`/`status_code`/
  `metric_type`/`temporality` enums) stays on the dictionary path deliberately. This is a
  write-side choice recorded per column in each file's own metadata, so old and new files read back
  identically and no migration is needed — existing files keep their old encoding until retention
  ages them out or a merge pass rewrites them.

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
the RUM app registry (`rum_apps` table), the detected-release table (`deployments`) and service SLOs (`service_slos`) — always used, required for login and by the always-on
uptime subsystem. RUM apps are registered/edited/rotated/removed at runtime (UI or the
`/api/rum/apps*` management API — see [`subsystems/rum.md`](subsystems/rum.md)), not via config;
there is no `[[rum.apps]]` config surface anymore.

## API surface

Handlers live in `crates/photon-api/src/*.rs`; the aggregation logic they call lives in
`photon-query`. Every route except the open ones requires the signed `photon_session` cookie.

**Search-handler failure contract.** `POST /api/search`, `/api/traces/search`, and
`/api/spans/search` return an empty page for a genuinely empty result (a fresh server prunes down
to nothing and the engines return that on their `Ok` path), but a hard engine failure is a **500**
carrying `{"error": ...}` — see `search::engine_failure`. These handlers used to log the error and
return an empty-but-200 page, which rendered a `ResourcesExhausted` from the shared query pool as
"your search matched nothing". The frontend mirrors this: `api.ts`'s mock fallback triggers only
when there is no HTTP response at all (real dev-without-backend), never on a 4xx/5xx, so a failing
backend can't be silently replaced with demo data.

- **Ingest (gRPC + HTTP, bearer token):** gRPC `POST /v1/logs`, `/v1/traces`, `/v1/metrics` (OTLP); HTTP `POST /v1/logs`, `/v1/traces`, `/v1/metrics` (OTLP), `/api/v1/write` (Prometheus remote-write 1.0, snappy+protobuf). All share `[ingest].token`.
- **Open (no session):** `POST /api/login`, `POST /api/setup` (first-run), `GET /api/session` (boot).
- **Public browser beacon (per-app key + Origin/CORS, no cookie):** `POST /api/rum`.
- **Logs:** `GET /api/services`, `POST /api/search`, `GET /api/fields|facet|histogram`.
- **Live tail (SSE):** `GET /api/stream/logs`, `GET /api/stream/spans`.
- **Traces / spans:** `GET /api/traces/:trace_id`, `POST /api/traces/search`, `POST /api/spans/search`,
  `GET /api/traces/fields|facet|histogram|latency`.
- **Services (APM):** `GET /api/services/overview` (the whole index in one request — the Site band plus one row per `(service, Site, environment)` with sparklines; `source` says `rollup` or `raw`), `GET /api/red`, `GET /api/services/:service/timeseries|dependencies|settings` (`timeseries`/`dependencies` take an optional `q` grammar param AND-ed with the service term — the UI's tenant filter), `GET /api/services/:service/operations` (one row per operation with per-row sparklines, `source`-aware), `GET /api/services/:service/operations/:operation/heatmap` (the latency distribution over time, on the rollup's fixed ladder), `GET /api/services/errors` (fleet-wide Backend Issues — errored spans grouped by `error.fingerprint`; every row names its own service), `GET /api/services/:service/errors|errors/facets|errors/:fingerprint` (the per-service list, its six facet fields, and one Issue's detail; identity is `(service, fingerprint)`, never the fingerprint alone), `GET /api/services/map` (the topology — every service-to-service edge from one scan; optional `service` narrows to edges touching it in both directions, applied AFTER the edges exist because scoping the scan would delete the parent spans the join needs; the `MAX_MAP_SPANS` scan cap is reported as `capped`, never silent), `GET/POST /api/deployments` (releases — `GET` filters by `service`/`environment`/`tenant` and an ns-string `start`/`end` window; `POST` is CI enrichment, upserting on `(service, environment, tenant, version)` while leaving `detected_at_ms` and `source` write-once). `GET/POST /api/services/:service/slos`, `PATCH/DELETE /api/services/:service/slos/:id`, `GET /api/services/:service/slos/:id/budget` (error budget + multi-window burn rate; rollup-only, so a window the rollup does not cover is reported `partial`).
- **Metrics:** `POST /api/metrics/query`, `GET /api/metrics/catalog|metadata/:name|labels`.
- **Infrastructure (host/GPU resource monitoring):** `GET /api/infra/hosts`,
  `GET /api/infra/hosts/:host`, `GET /api/infra/hosts/:host/processes`,
  `GET /api/infra/hosts/:host/timeseries?resource=cpu\|memory\|disk\|network\|gpu\|gpu_memory\|gpu_temp\|gpu_power\|load`
  — see [`subsystems/infra.md`](subsystems/infra.md).
- **RUM (read):** `GET /api/rum/apps|vitals|vitals/breakdown|pages|pages/detail|errors|errors/facets|errors/:fingerprint`
  (all read endpoints accept an optional `q` grammar filter — log grammar on the error routes,
  metrics grammar on the vitals/breakdown/pages routes; the UI's federation tenant filter.
  `GET /api/rum/apps` returns the full app registry records, including the public `key`; with
  `?tenant=<name>` it instead derives a names-only app list from mirrored `web_vitals.*` data —
  central's read-only tenant lens, no registry sync).
- **RUM (app registry, session-authed):** `POST /api/rum/apps` (register; server mints the key),
  `PATCH /api/rum/apps/:name` (update origins/sampling/rate limit), `POST
  /api/rum/apps/:name/rotate-key` (mint a fresh key), `DELETE /api/rum/apps/:name` (unregister).
- **Uptime:** `GET/POST /api/monitors`, `GET/PATCH/DELETE /api/monitors/:id`,
  `POST /api/monitors/:id/pause|resume`, `GET /api/monitors/:id/heartbeats|incidents`.
- **Alerts:** `GET/POST /api/alerts/rules`, `GET/PATCH/DELETE /api/alerts/rules/:id`,
  `POST /api/alerts/rules/:id/test` (evaluate a saved rule now), `POST /api/alerts/preview`
  (dry-run a draft condition), `GET/POST /api/alerts/channels`,
  `GET/PATCH/DELETE /api/alerts/channels/:id`, `POST /api/alerts/channels/:id/test` (send a sample
  webhook), `GET /api/alerts/incidents` (`?status=triggered|resolved&rule_id=&limit=`) — see
  [`subsystems/alerts.md`](subsystems/alerts.md).
- **Data / usage / retention:** `GET /api/storage`, `GET /api/usage/series`, `GET /api/usage/services` (per-service footprint — an ESTIMATE, `service rows × the signal's mean bytes/row`, because Parquet sorts by `service.name` and compresses a service's rows together; metrics are reported `attributed: false` with the reason, since a metric series is keyed by name and most belong to a host, a browser app or the rollup rather than a service), `GET/PUT /api/retention`,
  `POST /api/data/purge`.
- **Federation (central + tenant):** `GET /api/tenants` (central list all), `POST /api/tenants`
  (central create), `PATCH/DELETE /api/tenants/:name` (central update/remove),
  `POST /api/tenants/:name/rotate-token` (central rotate), `GET /api/tenants/summary` (central curated
  per-tenant health + rate/incidents/disk), `GET /api/federation/status` (tenant see local federation
  config + push telemetry) — see [`subsystems/federation.md`](subsystems/federation.md).
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
  work, never drop a real result. This is a property test; keep pruning conservative (a missing,
  unreadable, or corrupt `.idx`, or an unknown range, means _keep the file_ — a torn sidecar can
  never drop a real result, abort the query, or panic; `idx_binary::decode` rejects the frames
  that used to divide-by-zero in the bloom, and every `keep_candidate` keeps on any read/decode
  error, not just `NotFound`).
- **Local disk is the primary store**; the S3 durable store is an async replica, never on the ack or
  query path. Ingest acks after the local WAL `fsync`, not after durable upload.
- **Rows are sorted by the signal's sort key before Parquet encoding** (logs `(service.name,
  timestamp)`, spans `(service.name, start_time)`, metrics `(metric_name, service.name, host.name,
  timestamp)`) — this is what makes min/max pruning effective and compresses well. The compactor's
  lexsort order _is_ the query engine's pruning contract. `service.name` is promoted and is the logs
  primary sort key — it cannot be un-promoted. `deployment.environment` is promoted by default too
  (it is a global UI filter, read on every page — see [frontend.md](frontend.md#the-environment-dimension));
  Parquet written before that default simply lacks the column, which is fine in both directions: the
  grammar resolves the key to the long-tail map when it isn't a promoted column, and a missing
  skip-index range degrades to "keep the file". There is no migration. The metrics skip index additionally ranges over
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
filter semantics. Syntax: `field:value` (exact), `field:"a b"` (quoted value — the only way to express a value containing
a space or a comma; the quotes are grouping and are stripped), `field:v1,v2` (OR-list, split on
commas OUTSIDE quotes), `field:*` (exists),
`-field:v` (negate), `field>=n` / `>` / `<` / `<=` (numeric compare), and `"quoted text"` or bare
words (case-sensitive body substring). Terms are AND-ed; parse errors carry a byte `offset` so the UI
can underline the bad token. There is **no escape** for a `"` inside a value, so a caller building a
term from an arbitrary name uses `photon-api`'s `grammar_term` (or the frontend's `grammarTerm`),
which quotes when needed and returns `None` rather than emitting a term that would parse cleanly and
match nothing. The frontend has a display-only mirror (`lib/core/queryLang.ts`).
