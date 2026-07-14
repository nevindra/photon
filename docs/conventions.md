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
  incidental.
- **Prometheus remote-write type inference:** RW 1.0 samples carry no type tag, so
  `promrw_mapping::classify` infers it from the metric-name suffix — `_total`/`_bucket`/`_count`/
  `_sum` → cumulative monotonic `SUM`, else `GAUGE`. This mirrors the OTel Collector's Prometheus
  receiver. `__name__` → metric name, `job` → `service.name`.
- **Prometheus classic-histogram families:** folded at query time (not ingest). `le` is always
  consumed by the aggregation and never a group-by dimension. Supported percentiles are p50/p90/p99
  (no p95).
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
  and `lib/context.ts` — the sanctioned home for app-wide **time window** (`timeRange`/`customRange`,
  plus derived `startNs`/`endNs`) and **entity scope** (`scope`, e.g. a service or RUM app). Do not
  add a global store, and do not duplicate time/scope state locally in a view — read it from
  `context.ts`.
- **URL query-key ownership is split and merge-preserving.** `lib/context.ts` is the *sole* owner of
  the `range`/`from`/`to`/`scope` keys; `useUrlState.js` owns only `svc`/`sev`/`q` (per-view
  service/severity/text filters). Each module's URL sync (`history.replaceState`) deletes and
  rewrites *only* the keys it owns, leaving the rest of `location.search` untouched — so editing a
  view's text filter never clobbers the current time window, and changing the time range never drops
  a view's filters. `lib/useCorrelate.ts`'s `correlate()` follows the same discipline when building
  cross-signal links: it always injects the current time+scope alongside the destination's own query
  params. Give any new URL-backed state a single owning module and touch only that module's keys.
- **`ui/` primitives are `<script setup lang="ts">`; views and domain components are, by default,
  plain JS `<script setup>`.** The only plain-JS files under `ui/` are the Photon-authored composites
  (`facet/`, `nav-tabs/`, `peek-drawer/`, `select-menu/`, `sparkline/`).
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
