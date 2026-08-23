# Conventions & gotchas

House rules that aren't obvious from any single file. Read the relevant entry before changing a
public API, adding a dependency, or touching the query/chart data path.

## Backend (Rust)

- **`PhotonError` is one enum with a variant per crate** (`photon-core/src/lib.rs`). Downstream
  crates **never edit the enum** (that would race under parallel development) — they use the variant
  that fits. Every crate's domain error is pre-declared.
- **Dependency versions are co-pinned** in the root `Cargo.toml`. `arrow 53` / `object_store 0.11` /
  `parquet 53` / `datafusion 43` **must move together** (DataFusion 43 re-exports the others), and
  `opentelemetry-proto 0.27` ↔ `tonic 0.12` / `prost 0.13`. Do **not** bump any of these
  independently. Enabling a *feature* on an existing dep (e.g. `arrow`'s `ipc`) is allowed — a
  feature flag is not a version bump; copy the exact version string, never edit the number. `snap`
  (snappy block codec for Prometheus remote-write) is standalone — not co-pinned.
- **`arrow` is `default-features = false`** (drops unused csv/ipc/json + ~15 transitive crates — part
  of "10× lighter"). Crates opt into the features they need (`photon-wal` adds `ipc`, etc.).
- **DataFusion column access for dotted names:** use `col_ref(name)`
  (`Column::new_unqualified`) for names like `service.name`. `col("service.name")` is **WRONG** — it
  splits on the `.`. Non-promoted attributes are read via `get_field(col_ref(schema::ATTRIBUTES), key)`.
- **Boundaries are traits** (`Wal`, `SkipIndex`, `BlobStore`, `RumSink`, …) so crates test against
  in-memory fakes; real disk/object-store impls are wired only in `photon-server`. Keep pure logic
  (`photon-index`, most of `photon-core`, mapping/enrichment in `photon-core::rum`) I/O-free and
  table-testable.
- **Debug builds compile dependencies at `opt-level = 3`** (`[profile.dev.package."*"]` in the root
  `Cargo.toml`) while our crates stay at `opt-level = 0` for fast incremental rebuilds. This is why a
  debug `/api/search` decodes Parquet at near-release speed (~13× faster than a fully-unoptimized
  debug build). Don't "simplify" this profile away.
- **jemalloc** (`tikv-jemallocator`) is the global allocator in the server — it returns freed pages
  to the OS (glibc retained them) and speeds the allocation-heavy ingest path. Measured, not
  incidental. It is **tuned, not stock**: `main.rs` exports a `_rjem_malloc_conf` static setting
  `background_thread:true` plus 1 s dirty/muzzy decay. Stock jemalloc drives decay from allocation
  activity *within each arena* and leaves `background_thread` off, so an idle process never purges
  what it freed — a production node sat at **734 MB RSS at 2% CPU**, 698 MB of it freed-but-unpurged
  arena pages. Two traps if you touch this:
  - The symbol MUST be `_rjem_malloc_conf`. `tikv-jemallocator` builds jemalloc with the `_rjem_`
    prefix (it only drops it under the `unprefixed_malloc_on_supported_platforms` feature, which we
    do not enable), so a plain `malloc_conf` is silently ignored and every setting is lost. The env
    var is likewise `_RJEM_MALLOC_CONF`, not `MALLOC_CONF` — which makes it the way to test a
    setting against a *running* deployment with no rebuild.
  - It is `#[cfg(target_os = "linux")]` on purpose. macOS does not support `background_thread` and
    prints `option background_thread currently supports pthread only` on every run; the deployment
    target is distroless Linux, and dev builds keep jemalloc's defaults.
- **Parquet per-column encodings are chosen, not defaulted** (`photon-compact/src/stream.rs`,
  `writer_properties`). parquet-rs defaults every column to dictionary-first; Photon overrides two
  shapes where that is actively wrong — the `Timestamp`/`Int64` time columns (all-distinct, so the
  dictionary spills to a flat 8 bytes/row) and the random hex ids (`trace_id`/`span_id`/
  `parent_span_id`, where the dictionary is as big as the data it indexes). Three rules that look
  like they could be "simplified", and each is load-bearing:
  - **`Int32` stays on the dictionary path.** In all three schemas it is only ever a small enum
    (`severity_number`, `kind`, `status_code`, `metric_type`, `temporality`). Widening the delta
    rule to "all integers" regresses exactly these.
  - **The time-column rule is signal-scoped** (`TimeEncoding`), because it is a property of the
    sort key. Logs/spans sort time last within a service run so it climbs; metrics sorts it last of
    *four* keys and one scrape stamps many rows with the identical instant, so the column is
    repetitive — the dictionary's best case. Forcing delta on metrics measured **2.4% larger**.
  - **The hex-id rule is not signal-scoped**, because it is about content being random, which no
    sort key changes.
  Measured on a production corpus: −18.3% (logs), −11.5% (spans), 0% (metrics), at equal or lower
  encode CPU. Write-side only — the encoding is recorded per column in each file's own metadata, so
  old files stay readable and need no migration. Related trap: **`zstd_level` is not monotonic.**
  Level 3 measured ~3% *larger* than level 1 on this data; the next level that pays is 6.
- **Prometheus remote-write type inference:** RW 1.0 samples carry no type tag, so
  `promrw_mapping::classify` infers it from the metric-name suffix — `_total`/`_bucket`/`_count`/
  `_sum` → cumulative monotonic `SUM`, else `GAUGE`. This mirrors the OTel Collector's Prometheus
  receiver. `__name__` → metric name, `job` → `service.name`.
- **Prometheus classic-histogram families:** folded at query time (not ingest). `le` is always
  consumed by the aggregation and never a group-by dimension. Supported percentiles are p50/p90/p99
  (no p95).
- **`promoted_attributes` is a read-path lever, not cosmetics.** A span Parquet's long-tail
  `attributes` map is 63% of the file's bytes and **96% of its decoded bytes**, so referencing it at
  all costs roughly the same however many keys are probed. Promotion is what gets a query off it;
  `span_engine::attr_lookup` picks the cheapest correct form per key and drops the map column
  entirely when the manifest proves no surviving file carries it. Changing the list only affects
  segments written from then on — a query spanning the change reads both layouts correctly, and the
  speed arrives once the older segments age out.
- **Parquet filter pushdown stays OFF.** Photon reads with an explicit schema, and in DataFusion 43
  pushdown over files whose schemas differ from it empties any `LIMIT` plan — silently. The failure
  hides behind any sort or aggregate, so it shows up as "this metric does not exist" on exactly the
  three call sites that filter straight into a limit. See
  [`architecture.md`](architecture.md) and the regression test in `photon-query/src/lib.rs`.
- **Two independent auth systems, never conflated:** OTLP **ingest** uses a shared service **bearer
  token** (`[ingest].token`); human **UI** users use **argon2 password + signed session cookie**.
  RUM's public beacon (`POST /api/rum`) uses a **third** model — per-app public key + Origin
  allowlist — and is the only CORS-enabled, unauthenticated route.
- **Tests:** inline `#[cfg(test)]` modules for units; integration tests in `crates/*/tests/`
  (e.g. `photon-wal/tests/wal.rs`, `photon-query/tests/search.rs`, `photon-server/tests/e2e.rs`,
  `photon-server/tests/rum_e2e.rs`). Follow TDD where the plans do (write the failing test first).
- **Crate doc comments cite the design intent.** When changing a crate's public surface, read its
  `//!` header first — the interface contracts are deliberate.

## Frontend (Vue)

- **Package manager is `bun`, never npm.** `bun.lock` is the lockfile. Add deps with `bun add`.
- **No Pinia.** Server state lives in **TanStack Query** (a request cache); within-view state lives
  in **URL params** (`useUrlState.js`, `?tab=`, route params). App-wide state is a few ad-hoc
  reactive-ref modules — the module-singleton pattern, not a store: `lib/auth.js`, `lib/theme.js`,
  and `lib/core/context.ts` — the sanctioned home for the three app-wide context dimensions: the
  **time window** (`timeRange`/`customRange`, plus derived `startNs`/`endNs`), the **Site**
  (`tenant`), and the **environment** (`environment`, i.e. `deployment.environment`). Do not add a
  global store, and do not duplicate any of it locally in a view — read it from `context.ts`.
- **URL query-key ownership is split and merge-preserving.** `lib/core/context.ts` is the *sole*
  owner of the `range`/`from`/`to`/`tenant`/`env` keys; `useUrlState.ts` owns only `svc`/`sev`/`q` (per-view
  service/severity/text filters). Each module's URL sync (`history.replaceState`) deletes and
  rewrites *only* the keys it owns, leaving the rest of `location.search` untouched — so editing a
  view's text filter never clobbers the current time window, and changing the time range never drops
  a view's filters. `lib/useCorrelate.ts`'s `correlate()` follows the same discipline when building
  cross-signal links: it always injects the current time+tenant+env alongside the destination's own
  query params. Give any new URL-backed state a single owning module and touch only that module's keys.
- **Every query composable appends the global context through `withContext()`.** The Site and
  environment dimensions filter by contributing grammar terms (`tenant:x`,
  `deployment.environment:y`) to the `q`/`filter` string a composable sends, and `contextQueryTerm()`
  is the single cache-key slot they re-key on — one slot, not one per dimension, so adding a fourth
  dimension later doesn't widen every key. Never hand-roll the join at a call site: a composable that
  appends one dimension but not the other renders a page that silently ignores a picker, which is
  worse than not having the picker. The one sanctioned exception is `lib/core/environments.ts`, which
  facets `deployment.environment` and must not filter by it — see
  [frontend.md](frontend.md#the-environment-dimension).
- **`ui/` primitives are `<script setup lang="ts">`; views and domain components are, by default,
  plain JS `<script setup>`.** The only plain-JS files under `ui/` are the Photon-authored composites
  (`facet/`, `nav-tabs/`, `peek-drawer/`, `select-menu/`, `sparkline/`).
- **Style comes from the tokens, never from eyeballed utilities.** Colour, elevation and radius live
  in `styles/tokens.css` + `tailwind.config.js`; components consume them. Elevation is three steps
  only — `shadow-1` (hairline ambient on chrome), `shadow-2` (things that actually float: popovers,
  drawers, dialogs, tooltips), `shadow-sink` (pressed) — so don't reach for stock Tailwind
  `shadow-sm`/`md`/`lg`. Radius is one family off `--radius`: `rounded-sm` 6 (menu rows, checkboxes),
  `rounded-md` 10 (controls), `rounded-lg` 12 (panels, switcher tracks), `rounded-xl` 14 (cards),
  `rounded-2xl` 18 (dialogs); pills use `rounded-full`. Pick by what the thing *is*. Separation comes
  from white paper on a grey canvas, so a table on a `surface="canvas"` view must own a panel
  (`rounded-xl border border-border bg-card shadow-1`) — see `docs/frontend.md`.
- **TypeScript adoption is new-files-only.** Every *new* frontend file — new `lib/` modules, new
  components, new test files — is authored in TypeScript: `.ts` for plain modules
  (`lib/context.ts`, `lib/useCorrelate.ts`), `<script setup lang="ts">` for new components
  (`ContextBar.vue`, `RelatedMenu.vue`, `HomeView.vue`), and `.ts` for new test files
  (`context.test.ts`, `useCorrelate.test.ts`). Existing `.js` files/`<script setup>` components stay
  JS when modified — they are not converted as a side effect. Import new TS modules
  **extensionlessly** (`@/lib/context`, not `@/lib/context.js`) — Vite resolves `.ts` via
  `resolve.extensions`, but a literal `.js` specifier will not find a `.ts` file at runtime. `bun run
  type-check` (`vue-tsc --noEmit`, config in `frontend/tsconfig.json`) is the enforcement gate: its
  `include` covers every `.ts`/`.d.ts` file and every `lang="ts"` SFC under `src/`; `allowJs: true` +
  `checkJs: false` keep legacy `.js` files and plain-JS `.vue` scripts importable but unchecked.
- **Timestamp units — two boundaries:**
  - Time **bounds** at the query/data layer are **nanosecond strings** (via an `ns()` helper);
    query composables stringify them into the `queryKey`.
  - Timestamps handed to **charts (uPlot)** are **ms Numbers** (`chartOptions.js` converts ms→sec
    for uPlot's UNIX-second x scale). Trace-tree/histogram geometry uses **BigInt nanoseconds**.
- **Query composables follow one contract:** reactive inputs (refs **or** getter functions)
  normalized with `toValue`, feeding a **`computed` `queryKey`**, with the `AbortSignal` threaded
  through. No business logic in composables.
- **Mutation error contract differs by backend:** `servicesQueries.js` throws the real Ky
  `HTTPError` (branch in `onSuccess`/`onError`); `dataQueries.js` / `usersQueries.js` mutations
  return `{ ok, error }` and never throw. Match the neighbouring file.
- **The `api.js` mock fallback must keep working.** The single Ky client tries `/api` and falls back
  per-method to the in-browser mock corpus (`mock.js`) on a **network** failure, while still
  surfacing real 400/404s. `api.mock` is the reactive degraded-mode flag the shell shows.
- **`cn()`** (`lib/utils.js`, clsx + tailwind-merge) is used by every `ui/` primitive for class
  composition.
- **Never call `history.replaceState` directly — use `replaceSearch()` (`lib/core/historyUrl.ts`).**
  Several layers merge their own keys into `location.search` outside vue-router (context's
  `range`/`from`/`to`/`scope`, `useUrlState`'s `svc`/`sev`/`q`, per-view `sort`/`mode`/`viz`/…), and
  a raw call breaks vue-router two ways: (1) `replaceState(null, …)` **wipes** the
  `{ back, current, forward, position, scroll }` object vue-router keeps on the entry — and since
  `router.afterEach` re-syncs context on every navigation, that state was destroyed the moment you
  landed anywhere, which is what stopped `useBackTo` from ever finding the entry to go back to
  (vue-router warns about this in dev); (2) even preserving the state isn't enough — vue-router's
  `push()` re-asserts `state.current` onto the outgoing entry
  (`changeLocation(currentState.current, …, true)`), so a **stale `current` reverts the URL you just
  merged**, silently dropping the filters from the entry you'd later go back to. `replaceSearch()`
  preserves the state and moves `current` with the URL.
- **Tailwind's `content` globs must cover `.ts`, not just `.vue`/`.js`.** Class-name literals live in
  plain TypeScript modules too — `services/serviceColor.ts` (`SERVICE_PALETTE`),
  `core/seriesColor.ts` (chart swatches), `services/serviceHealth.ts` (status dots),
  `core/format.ts` (severity tones). Drop `ts` from the glob and Tailwind purges exactly those
  utilities: the classes still land in the DOM, so nothing throws and no component test fails — the
  trace waterfall's span bars, the "time by service" band and every series swatch just render with
  **no background at all**. `tailwind.config.test.js` guards the glob. Corollary: keep those class
  strings **literal** (never built by concatenation), or the scanner can't see them either.
- **Design tokens (`styles/tokens.css` + `tailwind.config.js`) are the single source of truth for
  the look:** a near-neutral base + one reserved Photon Cyan brand accent (`--brand`), layered
  `surface-1`/`surface-2` chrome, `shadow-1`/`shadow-2`/`shadow-sink` elevation, Tight radius, and
  `motion-safe:`-gated tactility. **Physicality budget:** chrome (buttons, cards, panels, overlays,
  nav) gets elevation + hover-lift/press-recess; dense data rows (tables, log/span rows) stay flat
  and instant — never add a shadow or transform to a row component, it breaks the density contract.
  `success`/`success-soft` is the healthy/up/good green, kept separate from `sev-*` (warn/error/fatal
  only).

## Build order

`frontend/dist` is **embedded into `photon-api` at compile time** via `rust-embed`
(`crates/photon-api/src/assets.rs`, folder `../../frontend/dist`). It is gitignored and absent on a
fresh checkout, so **build the frontend before the backend** — otherwise `photon-server` serves a
404 UI and `photon-api`'s embed tests (`frontend_bundle_is_embedded`, `root_serves_index_html`) fail.

```bash
cd frontend && bun install && bun run build   # regenerate frontend/dist
cd .. && cargo build --release
```

## Git workflow

**Do not `git commit` between tasks.** Stage changes as you go (`git add`); the human reviews batched
changes and controls commit granularity. This applies to subagents dispatched from a plan too — strip
any "git commit" step and leave the working tree dirty.
