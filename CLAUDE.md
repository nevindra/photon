# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What Photon is

A fast, lightweight, OTEL-native observability platform — a self-hosted alternative to SigNoz,
Datadog, Sentry, and Grafana, shipped as a **single Rust binary** with an embedded Vue 3 UI. It
carries **five signals** — logs, traces (spans), metrics, uptime monitoring, and RUM (Real-User
Monitoring) — plus data usage/retention management. Single node, single implicit auth role (no RBAC
yet); RBAC/multi-tenancy/clustering are deliberately out of scope until there's demand.

**The `docs/` directory is the project knowledge base — read it to gather context fast:**
- [`docs/architecture.md`](docs/architecture.md) — crate graph, per-signal write/read data flow,
  storage/durability, the `/api/*` surface, and the load-bearing invariants.
- [`docs/subsystems/`](docs/subsystems/) — **one doc per feature**: `logs`, `traces`,
  `services-apm`, `metrics`, `rum` (incl. the `@photon/rum` SDK), `uptime`, `infra` (host/GPU
  resource monitoring), `data`, `auth` — each with its key files, endpoints, and UI.
- [`docs/frontend.md`](docs/frontend.md) — the Vue app: routes/views, the `ui/` primitives, `lib/`.
- [`docs/conventions.md`](docs/conventions.md) — conventions & gotchas.
- [`README.md`](README.md) — the public-facing overview and positioning.

> **Keep the docs in sync with the code.** Whenever you change the codebase (`crates/`,
> `frontend/src/`, `sdk/`) in a way that affects behavior, architecture, an API route, config, or a
> subsystem, update the matching doc **in the same change** — the relevant
> [`docs/subsystems/`](docs/subsystems/) file (or a cross-cutting doc) **and this CLAUDE.md** — then
> re-verify internal links. A `PostToolUse` hook (`.claude/settings.json`) reminds you after source
> edits. Docs that name a file/route/symbol must stay true to the code.

## Build & run

The frontend bundle is **embedded into the binary at build time** (`photon-api` uses `rust-embed` on
`frontend/dist`, via `crates/photon-api/src/assets.rs`). `frontend/dist` is gitignored and absent on
a fresh checkout, so **build the frontend first** — otherwise `photon-server` serves a 404 UI and
`photon-api`'s embed tests (`frontend_bundle_is_embedded`, `root_serves_index_html`) fail:

```bash
cd frontend && bun install && bun run build   # regenerates frontend/dist (package manager is bun, never npm)
cd .. && cargo build --release                 # or `cargo build` for debug
```

Run the server (config path resolves argv[1] → `$PHOTON_CONFIG` → `photon.toml`; REST/UI API binds
`0.0.0.0:8080`, override with `$PHOTON_API_ADDR`). On first start, open the UI and complete the
one-time "create your account" onboarding — UI users live in the SQLite control-plane DB, not config:

```bash
cp photon.example.toml photon.toml             # then edit secrets
cargo run -p photon-server -- photon.toml
cargo run -p photon-server -- hash-password '<pw>'   # print an argon2 password hash
```

For containerized deployment (single non-root distroless image + `docker compose`, with an opt-in
Garage durable-S3 profile), see `deploy/README.md`. Config is via `PHOTON_*` env vars; `make
docker-up` after `cp .env.example .env`.

Generate synthetic load against a running server (dev/bench only — posts OTLP/HTTP protobuf batches
via a `logs`/`traces`/`metrics` subcommand; `--saturate` or `--rate <n>` are mutually exclusive):

```bash
cargo run -p photon-loadgen -- logs    --rate 5000 --services 10   # --rate is logs/sec; $PHOTON_INGEST_TOKEN read
cargo run -p photon-loadgen -- traces  --rate 2000 --services 10 --spans-per-trace 4..20
cargo run -p photon-loadgen -- metrics --rate 2000 --services 10
```

Run the host/GPU resource-monitoring agent against a running server (its own standalone binary, not
part of `photon-server`; samples the host every 15s by default and POSTs OTLP metrics tagged
`host.name`) — see [`docs/subsystems/infra.md`](docs/subsystems/infra.md):

```bash
cargo run -p photon-agent -- --endpoint http://127.0.0.1:4318/v1/metrics   # $PHOTON_INGEST_TOKEN / PHOTON_AGENT_* env
```

### Local dev (both processes together)

`make dev` runs the backend and the Vite dev server together under **process-compose** (a TUI process
manager; install once with `make install-tools`). On first run it generates a throwaway
`photon.dev.toml` (local `./.photon-dev/hot` data dir, dev secrets, `admin`/`admin` login) — so it
works out of the box. The Vite dev server (`http://localhost:5173`) proxies `/api` → the backend on
`:8080`; edit Rust and press **F5** on the `server` pane to rebuild (no auto hot-reload for the
backend; Vue hot-reloads via Vite). `make help` lists all targets.

To run just the frontend dev server manually: `cd frontend && bun run dev` (needs the backend running
on `:8080` for `/api`).

The frontend (`frontend/`) is **Vue 3 + Vite + Tailwind** with **Reka UI** headless components (the
shadcn-vue equivalent) and Lucide icons; package manager is **bun** (never npm — `bun.lock` is the
lockfile). It is deliberately lean — **Vue Router for top-level routes, but no Pinia** (server state
lives in **TanStack Query**, a request cache). Time range + entity scope are **global**,
module-singleton state in `lib/core/context.ts`, surfaced by one `ContextBar` mounted once in `AppShell`
(not per-view, and no longer on `TopBar`); within-view filters (`svc`/`sev`/`q`) live in URL params
via `lib/core/useUrlState.ts`, which merge-preserves the context keys. Routes: `/` → `/home` (`HomeView`,
the cross-signal landing dashboard), `/logs`, `/traces` (+ `/traces/:traceId` waterfall), `/services`
(+ `/services/:service` APM detail), `/metrics` (+ `/metrics/catalog`), `/rum` (+ app-scoped
sub-routes: `/rum/:appId` vitals, `/pages[/:route]`, `/errors` (search bar + fixed facet panel), and
`/errors/:fingerprint` issue detail), `/uptime`, `/infra` (+ `/infra/:host` host detail — host/GPU
resource monitoring), `/data`, plus `/login` and `/onboarding`, behind a `beforeEach` auth guard
(`router/index.js`) gated on reactive flags (`lib/core/auth.ts`). `NavRail` groups these into ownership
**worlds** (Home; Frontend → `/rum`, Backend → `/services`, Infrastructure → **Hosts** `/infra` +
**Ops** `/uptime`), an **Explore** section (Logs/Traces/Metrics), and **Manage** (Data), with `AppShell`
deriving the highlighted group from the route. Cross-view correlation (log→trace, span/trace→logs,
and a "Related ▾" menu) flows through `router.push` via `lib/core/useCorrelate.ts`'s `correlate()`, which
always carries the active time+scope. Components are grouped by signal under `src/components/`
(`logs/`, `traces/`, `services/`, `metrics/`, `rum/`, `uptime/`, `data/`, `charts/`, `common/`,
`ui/`). Data flows through one **Ky** client (`lib/core/api.ts` — keeps a mock-fallback: tries `/api`,
falls back to in-browser mocks on a network failure while surfacing real 400/404s) wrapped by
**TanStack Query** composables (`lib/*Queries.ts`) keyed off URL/filter state. Tables/waterfalls are
row-windowed with **TanStack Virtual** on headless **TanStack Table**; charts use **uPlot**
(`components/charts/`); **VueUse** supplies small composables. Visual design is token-driven
(`styles/tokens.css`): a near-neutral base, a single reserved Photon Cyan brand accent, and layered
`surface-1`/`surface-2` chrome — see [`docs/frontend.md`](docs/frontend.md).

## Test, lint, format

```bash
cargo test                              # whole workspace
cargo test -p photon-wal                # one crate
cargo test -p photon-query --test search    # one integration-test file (in crates/*/tests/)
cargo test -p photon-wal group_commit   # one test by name substring
cargo fmt
cargo clippy --all-targets
cd frontend && bun run test             # frontend unit tests (vitest)
cd frontend && bun run type-check       # vue-tsc --noEmit; gates new *.ts / lang="ts" frontend files
cd sdk/rum   && bun run test && bun run size   # SDK tests + the <5 KB bundle-size gate
```

Unit tests are inline `#[cfg(test)]` modules; integration tests live in `crates/*/tests/`
(`photon-wal/tests/wal.rs`, `photon-query/tests/search.rs`, `photon-server/tests/e2e.rs`,
`photon-server/tests/rum_e2e.rs`).

## Architecture

Cargo workspace of small crates. **The crate graph is the architecture** — dependencies point one
way, toward `photon-core`; the compiler forbids cycles and cross-boundary reach. Boundaries are kept
clean so a piece could later split into its own process (via a mode flag) without a rewrite.

```
photon-core     domain types only, no I/O: Arrow schemas (logs/spans/metrics), config, segment IDs,
                manifest, the pure query grammar (parse/AST/resolver/eval), RUM mapping, the shared
                PhotonError enum. Everything depends on it.
photon-wal      group-commit write-ahead log. Trait: Wal. DiskWal + BroadcastingWal (live-tail SSE).
photon-index    skip-index format (bloom + min/max stats). Struct SkipIndex (NOT a trait). Pure.
photon-storage  hot + durable object_store wrappers + background Replicator. Concrete Storage +
                Replicator (there is NO BlobStore trait).
photon-compact  three per-signal compactors (Compactor/SpanCompactor/MetricsCompactor):
                closed WAL segment → sort → Parquet + skip index → replicate → manifest; + merge_once.
photon-query    three engines: QueryEngine (logs), SpanQueryEngine (traces/APM/RED), MetricsQueryEngine
                (metrics + RUM vitals). Reassembles Prometheus classic histograms (`le`-bucket →
                quantile) at query time. Manifest + skip-index pruning, then DataFusion over survivors.
photon-ingest   OTLP gRPC (tonic) + HTTP (axum) receivers for logs/traces/metrics, mapping, token auth.
                + Prometheus remote-write 1.0 (/api/v1/write, snappy+protobuf → metrics WAL). Both
                front doors accept gzipped requests (stock OTel Collector gzips by default) and share
                one `[ingest].max_body_bytes` cap (~16 MiB) enforced on the *decompressed* size.
photon-uptime   always-on synthetic HTTP/TCP/ICMP monitors → embedded SQLite. Trait: UptimeStore.
photon-api      axum REST + session auth (argon2, signed cookies) + embedded Vue UI. Defines trait
                seams RumSink/RumAppStore/UserStore/UsageStore/ReplicationStatus/DataAdmin (photon-api
                can't dep photon-wal).
photon-server   the binary: config load, wiring, background-task supervision (3 compactor loops, etc.).
photon-loadgen  dev-only OTLP/HTTP logs+traces+metrics load generator (its own binary).
photon-agent    standalone host/GPU resource-metrics agent (its own binary, like photon-loadgen):
                samples host (sysinfo) + NVIDIA GPU (nvml-wrapper, default-on `gpu` feature), POSTs
                OTLP system.*/system.gpu.* metrics tagged host.name/host.id/os.type to /v1/metrics.
```

**Signal isolation by duplication:** logs, spans, and metrics each get their **own** WAL, Arrow
schema, compactor, and manifest object — a deliberate cost so adding a signal never destabilizes the
logs path. RUM adds **no** storage engine (Web Vitals → gauge metrics, JS errors → logs); uptime is a
self-contained SQLite vertical. RUM's own app registry (`rum_apps` — name/key/allowed_origins/
sample_rate/rate_limit, UI-managed, no config surface) lives in the same shared control-plane SQLite
DB as UI users and uptime monitors. Full per-signal data flow in
[`docs/architecture.md`](docs/architecture.md).

**Data flow (one signal):** OTLP request → `photon-ingest` checks the bearer token + per-signal
semaphore, maps OTLP → an Arrow record, appends to that signal's WAL → the group-commit `fsync`
completes = **the ack / durability boundary** (data survives a crash from here) → the signal's
background compactor in `photon-server` drains each *closed* WAL segment into a sort-key-ordered
Parquet file + `.idx` skip-index sidecar in the hot dir, enqueues an async replicate to the durable
store, and updates the manifest → a lower-frequency `merge_once` consolidates small files (bounded
per pass) →
`photon-query` prunes candidate files via the manifest (time overlap) then the skip index (min/max +
token bloom), and only then reads the surviving **local** Parquet with DataFusion.

**Load-bearing invariants — do not break:**
- **No inverted index.** Pruning is min/max stats + token bloom filters (kilobytes/file). Bloom
  filters may false-*positive* (extra scan) but **never false-negative** — pruning can only add work,
  never drop a real result. This is a property test; keep pruning conservative (a missing `.idx`, or
  an unknown range, means *keep the file*).
- **Local disk is the primary store**; the S3-compatible durable store is an async replica, never on
  the ack or query path. Ingest acks after the local WAL `fsync`, not after durable upload. The
  cold/durable **read** path is deliberately unimplemented (no `object_store` registered with
  DataFusion). `[storage.durable]` is optional; omit it to run hot-tier-only. The replicator is one
  long-lived drain loop (bounded in-flight, retry + re-enqueue — never silently drops) carrying BOTH
  uploads and **retention deletes**: `merge_once`/`purge_before` enqueue durable deletes for the
  objects they unlink from hot (NotFound-tolerant), so the durable replica honors retention instead
  of growing forever.
- **Rows are sorted by the signal's sort key before Parquet encoding** (logs `(service.name,
  timestamp)`, spans `(service.name, start_time)`, metrics `(metric_name, service.name, host.name,
  timestamp)`) — this is what makes min/max pruning effective and compresses well. The compactor's
  lexsort order *is* the query engine's pruning contract. The metrics skip index additionally ranges
  over `host.name` (binary sidecar format v2; older v1 sidecars decode with `host_range = None`,
  which — per the conservative-pruning rule above — keeps the file). See
  [`docs/subsystems/infra.md`](docs/subsystems/infra.md).
- **`search` is two-pass (late materialization).** Pass 1 applies the predicate but projects *only*
  `timestamp`, sorts DESC, takes `limit`, finds the cutoff; pass 2 re-runs with `timestamp >= cutoff`
  and returns full rows — so the wide `attributes` map is decoded only for surviving rows. Don't
  collapse it to one pass: it regresses broad-window, low-selectivity searches ~5x. Spans use the
  same trick for all three `SpanSort`s (`span_search.rs`) — `Recent` on `start_time_nanos`,
  `Slowest` on nullable `duration_nanos`, `Errors` on the COMPOSITE `(status_code, start_time)`
  (cutoff `(cs, cts)`, pass 2 filter `(status_code > cs) OR (status_code = cs AND start_time >=
  cts)`, not a single-column `>=`). All three span sorts also append `(span_id ASC, trace_id ASC)`
  as the final ORDER BY key in both passes — `span_id` is unique only *within* a trace per OTLP,
  but the pair is unique per row, giving a genuine total order so exact ties paginate
  deterministically instead of depending on which physical plan DataFusion picks.

### The query grammar & UI API

The log search bar speaks a small query language defined **once** in `photon-core/src/query/`
(`parse` → AST → `FieldResolver` → `eval`; pure, no DataFusion) and compiled two ways: to an
in-memory predicate (`eval`) and to a DataFusion filter (in `photon-query`) — one source of truth for
filter semantics. Syntax: `field:value` (exact), `field:v1,v2` (OR-list), `field:*` (exists),
`-field:v` (negate), `field>=n` / `>` / `<` / `<=` (numeric compare), and `"quoted text"` or bare
words (case-sensitive body substring). Terms are AND-ed; parse errors carry a byte `offset` so the UI
can underline the bad token. The frontend has a display-only mirror (`lib/core/queryLang.ts`).

The full `/api/*` surface (every route except the open/beacon ones requires the signed
`photon_session` cookie) is enumerated in [`docs/architecture.md`](docs/architecture.md#api-surface) —
including RUM's `GET /api/rum/errors` (now with an optional `q` log-grammar filter),
`GET /api/rum/errors/facets` (the fixed six-field facet panel), `GET /api/rum/errors/:fingerprint`
(issue detail), and the RUM app-registry management routes `GET/POST /api/rum/apps`,
`PATCH/DELETE /api/rum/apps/:name`, `POST /api/rum/apps/:name/rotate-key` (apps are UI-managed —
there is no `[[rum.apps]]` config anymore) — plus the curated Infrastructure vertical
`GET /api/infra/hosts`, `GET /api/infra/hosts/:host`, `GET /api/infra/hosts/:host/timeseries` (see
[`docs/subsystems/infra.md`](docs/subsystems/infra.md)). Handlers live in
`crates/photon-api/src/*.rs`; the aggregation logic they call lives in
`crates/photon-query/src/*.rs`. A `tower-http` `CompressionLayer` wraps the whole surface
(gzip/br, content-negotiated per `Accept-Encoding`; JSON logs compress ~15x) — its default predicate
skips SSE (`/api/stream/*`), so live-tail is never buffered.

## Conventions

See [`docs/conventions.md`](docs/conventions.md) for the full list. The load-bearing ones:

- **`PhotonError`** (`photon-core/src/lib.rs`) is one enum with a variant pre-declared for *every*
  crate's domain. Downstream crates never edit it (that would race under parallel development) — they
  use the variant that fits.
- **Three independent auth systems, never conflated:** OTLP **ingest** = shared service **bearer
  token** (`[ingest].token`); human **UI** users = **argon2 password + signed session cookie**
  (`[auth]`); the RUM public **beacon** (`POST /api/rum`) = per-app public key + Origin allowlist (the
  only CORS-enabled, unauthenticated route). The beacon is always-mounted now — apps are UI-managed
  in the `rum_apps` SQLite table (server-minted `pk_live_…` key, rotatable), not `[[rum.apps]]`
  config, and a beacon for an unregistered app now gets a **403** (there's no "RUM disabled" state
  left to 404 from).
- **Dependency versions are co-pinned** in the root `Cargo.toml` — `arrow 53` / `object_store 0.11` /
  `parquet 53` / `datafusion 43` must move together (DataFusion 43 re-exports the others), and
  `opentelemetry-proto 0.27` ↔ `tonic 0.12` / `prost 0.13`. Do **not** bump any of these
  independently (enabling a *feature* like `arrow`'s `ipc` is fine — not a version bump). `arrow` is
  `default-features = false`; crates opt into the features they need.
- **DataFusion column access:** use `col_ref(name)` (`Column::new_unqualified`) for dotted names like
  `service.name` — `col("service.name")` is WRONG (splits on `.`). Non-promoted attributes:
  `get_field(col_ref(schema::ATTRIBUTES), key)`.
- Boundaries are **traits** (`Wal`, `UptimeStore`, and the photon-api seams
  `RumSink`/`RumAppStore`/`UserStore`/`UsageStore`/`ReplicationStatus`/`DataAdmin`) so crates test
  against in-memory fakes; real disk/object-store impls are wired in only in `photon-server`. Note
  `SkipIndex` is a
  **struct** and storage exposes concrete `Storage`/`Replicator` — there is **no** `BlobStore` trait.
  Keep pure logic (`photon-index`, most of `photon-core`) I/O-free and table-testable.
- **Debug builds compile dependencies at `opt-level = 3`** (`[profile.dev.package."*"]` in the root
  `Cargo.toml`) while our own crates stay at `opt-level = 0` for fast incremental rebuilds. This is
  why a debug `/api/search` decodes Parquet at near-release speed (~13x faster than a fully-
  unoptimized debug build). Don't "simplify" this profile away.
- **Frontend:** bun not npm; no Pinia; `ui/` primitives are `<script setup lang="ts">`; views + domain
  components were historically plain JS, but that's legacy state, not a rule going forward — **new
  `.vue` files may now be `<script setup lang="ts">`** too (e.g. `RumErrorDetailView.vue`,
  `RumErrorFilters.vue`), gated by `bun run type-check` (`vue-tsc --noEmit`). Time **bounds** are
  nanosecond strings, chart timestamps are ms Numbers, trace geometry is BigInt ns; query composables
  normalize refs-or-getters with `toValue` into a `computed` queryKey.
