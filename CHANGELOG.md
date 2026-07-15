# Changelog

All notable changes to Photon are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.2.0] - 2026-07-15

A feature release adding first-class Single-Page-App (SPA) support to the RUM SDK,
plus the metrics and attributes to store and query it. Fully backward compatible —
older SDKs and existing data are unaffected (every new beacon field and metric is
additive; unknown fields are ignored).

### Added

- **SPA / soft-navigation RUM tracking.** The `@photon/rum` SDK now models a **view**
  as a logical pageview instead of a document load: `view.id` rotates on every real
  client-side route change (History API — `pushState`/`replaceState`/`popstate`,
  auto-detected, on by default; query/hash-only changes don't rotate, and MPAs are
  unaffected). Each route becomes its own pageview with correctly-attributed Web
  Vitals, JS errors, and — with `tracing: true` — its own backend trace. Attribution
  is by construction (a per-view beacon buffer flushed on each rotation), not
  flush-time timing. Fixes SPA routers (e.g. TanStack Router) reporting no data on
  in-app navigation.
- **Honest per-route Web Vitals.** Soft-navigated routes report per-view **CLS**
  (web-vitals' session-window rule) and **INP**, plus a new **`web_vitals.route_change`**
  metric — a DOM-settle transition-time heuristic (good ≤ 1 s / poor > 3 s). LCP/FCP/
  TTFB stay real web-vitals for the landing load and are **never** synthesized for soft
  navigations. A new **`web_vitals.view_duration`** metric captures time-on-view.
- **New RUM attributes** on every vital point and error log — `nav` (`hard` | `soft`),
  `view.seq` (ordinal within the session), and `view.previous_route` — enabling
  navigation-path and engagement analysis.
- **`trackView(route?)` SDK export** — a manual escape hatch for routers that prefer to
  drive soft-navigation boundaries themselves (e.g. TanStack Router's `router.subscribe`).

## [1.1.0] - 2026-07-15

A hardening release on top of 1.0.0: correctness, durability, and DoS fixes across
the read and write paths, meaningful transfer/allocation wins, and a few new
operator-facing knobs. Fully backward compatible — no config or API breaking changes.

### Added

- **Response compression.** The whole HTTP surface (JSON API + embedded UI) now
  content-negotiates gzip/br per the client's `Accept-Encoding`. A ~115 KB / 500-row
  `/api/search` payload compresses ~15× to ~8 KB. Live-tail SSE (`/api/stream/*`),
  gRPC, images, and sub-32-byte bodies are skipped, so streaming is never buffered.
- **Gzipped OTLP ingest.** Both the HTTP and gRPC receivers now accept gzip-compressed
  OTLP, so a stock OpenTelemetry Collector (which gzips by default) works out of the box.
- **Configurable Parquet compression.** New `[storage].zstd_level` (1–19, default 1)
  tunes the compactor's zstd level. The default is byte-identical to the previous
  hardcoded level.
- **Ingest body-size cap.** `[ingest].max_body_bytes` (16 MiB default, override with
  `PHOTON_INGEST_MAX_BODY_BYTES`) is enforced on the *decompressed* request body across
  all receivers, bounding gzip bombs and pre-allocation blowups.
- **`PHOTON_DISABLE_COMPACTION`** environment variable to gate the three background
  compactors (dev/ops).
- **Retention ceiling.** A `MAX_RETENTION_DAYS` cap is now validated in config, in the
  retention API route, and in the server retention loop.

### Changed

- **The durable replica now honors retention.** Retention deletes replicate to the
  durable (S3-compatible) store through a unified Upload/Delete replication queue
  (NotFound-tolerant), so the replica no longer grows forever.
- **Storage/usage stats come from the manifest.** Each file's on-disk size is captured
  at compaction time (`FileEntry.bytes`); footprint stats are now manifest arithmetic
  with a `stat()` fallback for files written before this release.
- **Deterministic span pagination.** All span sorts gain a stable `(span_id, trace_id)`
  final tiebreaker, so paginating across exact-key ties is deterministic (this changes
  the tie-order of the "Recent" sort).
- **Bounded query memory.** A 512 MiB DataFusion memory pool makes the facet/metrics
  paths fail loud (`ResourcesExhausted`) instead of OOMing the node.

### Fixed

- **Substring search no longer over-prunes.** Bloom pruning could drop files that
  contained partial-word matches (e.g. `tim` inside `timeout`); interior tokens are now
  bloom-tested on both sides, so a matching file is never pruned away.
- **Corrupt skip-index sidecars are tolerated.** A corrupt/unreadable `.idx` now *keeps*
  the file (conservative pruning) instead of aborting the query or panicking.
- **Wide time windows bucket correctly.** Fixed i64 overflow in time-bucket math so
  30–90 day windows bucket consistently.
- **Histogram reset detection.** Classic-histogram resets now compare bucket bounds by
  value, so a redefined histogram no longer corrupts the reset-aware delta.
- **Crash durability on the write path.** On a failed WAL commit, the torn tail is rolled
  back (or the segment rotated) and poisoned/recovered before the next write — restoring
  the "ack ⇒ survives crash" guarantee even under compound disk failure.
- **No overflow panics on untrusted OTLP.** Span timestamps and durations saturate /
  `checked_sub` instead of panicking on debug builds or producing negative durations.
- **Compaction durability & robustness.** The manifest is fsynced before any file is
  unlinked; empty WAL segments no longer emit a zero-row Parquet file; the replicator
  backs off and re-enqueues instead of silently dropping on retry exhaustion; compactor
  tasks are supervised; and stale `*.tmp` files are swept on startup.
- **Frontend request churn (logs view).** The volume histogram and facet requests are now
  debounced (one request per settle instead of one per keystroke), and the field catalog
  is fetched once against a shared key (no more `ColumnPicker` empty-list flash).

### Security

- **Constant-time ingest-token comparison** (`subtle::ConstantTimeEq`).
- **Token is checked before the in-flight permit** is acquired in every receiver, so an
  unauthenticated flood can't starve backpressure permits.
- **Query DoS closed.** Shared bucket/limit clamps (`MAX_BUCKETS=3000`, `MAX_LIMIT=1000`)
  are applied at every API handler and inside every engine method, closing the
  `?buckets=2e9` ~16 GB OOM vector.

### Performance

- **Single-pass search + count.** `/api/search` and `/api/spans/search` no longer prune
  the manifest/skip-indexes and re-open every surviving Parquet file twice — the row
  fetch and the match count share one query. Output-identical envelope.
- **Late materialization for all span sorts.** Slowest/Errors join Recent in decoding the
  wide `attributes` map only for surviving rows.
- **Streaming OTLP mappers.** Traces and metrics now stream straight into their Arrow
  builders, dropping the intermediate `Vec`/`BTreeMap` allocations (~2× transient memory),
  proven byte-identical to the reference path.
- **Bounded, off-runtime compaction.** Merge passes are bounded per pass (with a carry set
  so no entry is ever dropped) and Parquet decode runs on the blocking pool.
- **Narrowed trace-search hydration.** `search_traces` hydrates each kept trace's
  `min(start) ± 1h` and ranks in DataFusion instead of a full-window rescan (also fixes a
  window-straddle undercount).
- **Cached metric probe metadata** per engine (manifest-pointer invalidated), dropping a
  redundant prune + Parquet open per chart panel.

[1.2.0]: https://github.com/nevindra/photon/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/nevindra/photon/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/nevindra/photon/releases/tag/v1.0.0
