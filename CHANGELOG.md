# Changelog

All notable changes to Photon are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.3.0] - 2026-07-20

A feature release adding **system-wide alerting & notifications** — a cross-signal
webhook alert engine with provider-native channel presets — on top of correctness fixes
to the RUM pages breakdown and the WAL. Fully backward compatible: alerting is always-on
with sensible defaults and no required config, and every change is additive.

### Added

- **System-wide webhook alert engine (`photon-alerts`).** Rules watch **metrics, logs,
  traces, and RUM** and fire a webhook when a condition holds, moving each `(rule, series)`
  through a pure **OK · Pending · Triggered · Resolved** state machine. Incidents,
  notification channels, and per-rule severity / `for`-duration / evaluation interval are
  all UI/SQLite-managed (no config surface); the engine is a read-path consumer of the
  three query engines and is always on (optional `[alerts]` tunes only defaults). Uptime
  up/down transitions **bridge onto the same incident history + channels**, so there is one
  notification system, not two. New `/alerts` UI (rules · incidents · channels) and the
  `/api/alerts/*` surface.
- **Alert rule templates.** A target-first **"Browse templates"** quick-setup on the Rules
  tab: pick a target (Service · RUM app · Host · Global) and a concrete instance from live
  data, then **Apply** or **Customize** from a 23-template catalog — a frontend-only on-ramp
  that flows straight through the existing rule-create path.
- **Provider channel presets — Discord & Telegram.** Notification channels are now typed
  presets: the original **Generic webhook** (+HMAC) plus **Discord** (native embed) and
  **Telegram** (Bot API, HTML), each rendered by a pure `format.rs`. Pick a preset and fill
  in only its fields (Discord webhook URL; Telegram bot token + chat id). Channel input is
  validated (Discord host-locked to Discord's own hosts; Telegram bot-token shape), and a
  channel **Test** now performs one real, awaited delivery and reports the actual outcome —
  including for an **unsaved draft**, straight from the create/edit dialog. Discord
  (host-locked) and Telegram (server-constructed `api.telegram.org` endpoint) are SSRF-free;
  only the Generic webhook can target an arbitrary host.

### Fixed

- **Soft-navigated routes missing from the RUM pages list.** Two compounding drops hid
  clean soft views (no layout shift, no slow interaction) from `/rum/:app/pages`:
  - *SDK*: `beacon.flush()` skipped views whose buffers were empty, so such a view never
    reached the server at all. The first flush of a view is now its **finalizing beacon**,
    sent even with empty buffers so its `view.dur` → `web_vitals.view_duration` pageview
    marker always lands — and `dur` is now emitted **exactly once per view id** (repeat
    flushes, e.g. `visibilitychange` then `pagehide`, previously double-counted
    `view_duration` when new vitals accrued in between).
  - *Query*: the pages breakdown (`rum_breakdown`) counted pageviews only from
    LCP/INP/CLS sample counts — but soft views never emit LCP and emit CLS/INP only when
    nonzero, so a route reached exclusively by clean soft navigations stored
    `route_change`/`view_duration` points the pages list never looked at.
    `web_vitals.view_duration` (one point per finalized view — the true pageview count)
    now joins the merge.
- **Idle WAL segments never became queryable.** Age-based segment rotation ran only
  after a commit, so on a low-traffic instance the data in the active segment stayed
  invisible to the compactor — and to every query — until the *next* write happened to
  arrive, no matter how long you waited. The WAL writer's idle wait now wakes at the
  active segment's age deadline and seals it, so ingested data always becomes queryable
  within ~`segment_max_age_secs` even with zero follow-up traffic. Applies to all three
  WALs (logs, spans, metrics); no config change.

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
