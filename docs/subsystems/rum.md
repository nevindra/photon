# RUM — Real-User Monitoring

> Frontend performance (Core Web Vitals) + JavaScript error tracking, collected from the browser by
> a tiny SDK and surfaced in a purpose-built `/rum` UI section. The newest and most self-contained
> Photon subsystem; this doc is the single reference for it.

## The one guiding decision

**Reuse is a backend concern, not a frontend mandate.** RUM adds **no new storage signal**: Web
Vitals become **gauge metrics**, JS errors become **ERROR logs**. The write/store/query spine is
Photon's existing machinery; only the ingest mapping, a thin query facade, the SDK, and the UI are
new. App identity = `service.name`, so a frontend app is "just another service" and cross-signal
correlation (error → trace → log) comes for free.

## Data model — riding existing signals

### Web Vitals → metrics (`MetricPoint`, gauge)

Each reported vital is one gauge point through the existing `MetricSchema`, sorted with every other
metric by the compactor's `(metric_name, service.name, host.name, timestamp)` sort key (`host.name`
is simply absent/null on RUM points — see [`infra.md`](infra.md)). **Frozen metric names** (other
code depends on these exact strings):

| Vital | Metric name        | Unit          |
|-------|--------------------|---------------|
| LCP   | `web_vitals.lcp`   | `ms`          |
| INP   | `web_vitals.inp`   | `ms`          |
| CLS   | `web_vitals.cls`   | `1` (unitless)|
| FCP   | `web_vitals.fcp`   | `ms`          |
| TTFB  | `web_vitals.ttfb`  | `ms`          |
| Route change (soft nav) | `web_vitals.route_change` | `ms` |
| View duration (time-on-view) | `web_vitals.view_duration` | `ms` |

Attributes on every vital point: `service.name`, `browser.route` (route pattern, e.g. `/product/:id`),
`url.path`, `device.type`, `browser.name`, `network.connection`, `session.id`, `view.id`, `nav`
(`hard` | `soft`), `view.seq` (ordinal within the session), `view.previous_route` (absent on the
landing view) (plus optional `geo.country`). LCP attribution sub-parts are stored as their own gauge
metrics (`web_vitals.lcp.ttfb`, `.resource_load_delay`, `.resource_load_time`,
`.element_render_delay`) so they aggregate cleanly for the attribution bar; the LCP element selector
/ URL ride as string attributes on the vital's point. INP and CLS carry **no** attribution sub-parts
— they are measured per view by the SDK's `spa.ts` (one source of truth across hard + soft
navigations), so web-vitals' page-lifetime INP/CLS attribution is deliberately not collected.

`web_vitals.route_change` and `web_vitals.view_duration` are additive (older SDKs simply never emit
them; `photon-core::rum::beacon_to_metric_points` skips whatever a beacon omits). **Honesty
decision:** `web_vitals.lcp` / `.fcp` / `.ttfb` are never faked for a soft navigation — native LCP
doesn't fire without a hard document load, and reporting a heuristic under the real LCP name would
pollute the p75 scorecard. Soft-nav transition time is instead its own honest metric,
`web_vitals.route_change` (a DOM-settle heuristic: mutation + long-task + resource quiet-window,
measured from the view's `navStart`); `.fcp`/`.ttfb` are simply omitted for soft-navigated views.
`web_vitals.view_duration` is the view's time-on-view (its own `navStart` → the next rotation or
`pagehide`), emitted for every view, hard or soft.

### JS errors → logs (`LogRecord`, severity ERROR)

Each error is one log row: `severity_number = 17` / `severity_text = "ERROR"`, `body = <message>`,
attributes `service.name`, `exception.type`, `exception.message`, `exception.stacktrace`,
`error.kind` (`exception` | `unhandledrejection`), `browser.route`, `url.path`, `browser.name`,
`device.type`, `session.id`, `view.id`, `nav` (`hard` | `soft`), `view.seq`, `view.previous_route`
(absent on the landing view), and a server-computed **`rum.error.fingerprint`**. `nav`/`view.seq`/
`view.previous_route` are stamped by the same shared `photon_core::rum::common_attrs` helper that
builds the vitals' base attributes above, so errors and vitals from the same view carry identical
SPA context.

**Fingerprint** = stable FNV-1a hash of `exception.type` + normalized message (digit runs masked, so
"chunk 12" / "chunk 99" collapse) + top in-app stack frame. Computed **server-side at ingest** and
stored as an attribute, so "group into issues" is a `facet`/`count` by fingerprint at query time —
no bespoke error store.

### Sessions & pageviews

`session.id` is generated **client-side** (in-memory; rotates after 30 min idle or 4 h max).
"Sessions affected" is a `COUNT(DISTINCT session.id)` at query time.

`view.id` is a **logical pageview**, not a document load, and is owned by the SDK's `view.ts`
module (no longer `session.ts`). It **rotates on every real client-side route change** —
`history.pushState`/`replaceState`/`popstate`, auto-detected by `spa.ts`, on by
default, no config flag. A route change means `location.pathname` (or `routeOf(path)`, if
configured) actually changed; pure query/hash-only changes do **not** rotate the view. MPAs are
unaffected — with no History mutations the detector never fires, so `view.id` behaves exactly as
it did before this feature. Each view carries `seq` (an incrementing ordinal within the session;
the landing view is `0`), `prevRoute` (the route navigated from; absent on the landing view), `nav`
(`hard` for the initial document load, `soft` for every subsequent route), and its own `navStart`
for time-on-view. Vitals and errors are attributed to the view that was active **when they were
collected** — by construction, via a per-view beacon buffer flushed on each rotation, not by
flush-time timing.

**Every view records a pageview.** The first flush of a view (rotation or tab-hide) is its
*finalizing* beacon and is sent **even when nothing accrued** — a clean soft view with no layout
shift, no slow interaction, and an unsettled `route_change` still ships `view.dur`, whose
`web_vitals.view_duration` point is the pageview marker the pages breakdown counts. `dur` is
emitted exactly once per view id (repeat flushes — e.g. `visibilitychange` then `pagehide` — send
only newly-buffered items, without `dur`).

## Transport, ingest & browser auth

### The beacon

Compact JSON, sent with **`Content-Type: text/plain`** so it counts as a CORS "simple request" and
skips the preflight `OPTIONS` (more reliable via `navigator.sendBeacon`). One beacon batches a
pageview's vitals + errors:

```json
{
  "app": "web-storefront", "key": "pk_live_…",
  "session": "018f…",
  "view": { "id": "018f…", "route": "/checkout", "path": "/checkout", "seq": 2, "prev": "/cart", "nav": "soft", "dur": 4210 },
  "ctx": { "ua": "<raw user-agent>", "conn": "4g" },
  "vitals": [ { "n": "CLS", "v": 0.06 }, { "n": "route_change", "v": 180 } ],
  "errors": [ { "kind": "exception", "type": "TypeError", "msg": "…", "stack": "…", "src": "checkout.js", "line": 214 } ],
  "trace": "4bf92f3577b34da6a3ce929d0e0e4736"
}
```

The `view` object's `seq`/`prev`/`nav`/`dur` fields are all wire-optional (`#[serde(default)]`
server-side, so older SDKs simply omit them and behave exactly as before this feature): `seq` is
the view's ordinal within the session (landing = `0`), `prev` is the route navigated from (omitted
on the landing view), `nav` is `hard` (initial document load) or `soft` (a client-side route
change), and `dur` is the view's time-on-view in ms. The server copies `nav`/`seq`/`prev` onto
every vital point *and* every error log from this beacon as the `nav` / `view.seq` /
`view.previous_route` attributes (`photon_core::rum::common_attrs`); `dur` becomes its own gauge
point, `web_vitals.view_duration`.

`trace` is **optional** (`#[serde(default)]` — absent from beacons sent by SDKs without the opt-in
`tracing` module, or by older SDK versions) and, when present, is the current view's W3C trace id —
a fresh id is minted per view, including per soft-navigated view, so each route's backend requests
correlate to their own trace. The server validates/normalizes it
(`photon-core::rum::normalize_trace_id`) — exactly 32 hex digits, lowercased, rejecting the
all-zero id — before stamping it onto every error row's **native** `LogRecord.trace_id` column (no
schema change; malformed values are silently dropped, never partial-written). Web Vitals points
don't carry a trace id. **This means `trace_id` is only populated for errors ingested after the
SDK's `tracing` module shipped** — rows ingested by older SDKs simply have no trace id.

The server **derives `service.name` from the registered `app`** (never trusts a client-set service
name), parses the **raw UA server-side** into `device.type`/`browser.*` (keeps the SDK tiny and the
value trustworthy), maps vitals→`MetricPoint`s and errors→`LogRecord`s, and appends through the
existing metrics + logs WALs. Ack = the same group-commit `fsync` boundary as all other ingest.

### Endpoint & auth

**`POST /api/rum`** is the only public, CORS-enabled, unauthenticated RUM route (browsers can't hold
a session cookie for a beacon). Its auth model, appropriate for first-party multi-app:

- **Public app-key** (`pk_…`): identifies the app; safe to embed in client JS — it only *names* the
  app, it can't read data.
- **Origin allowlist**: the handler checks the `Origin` header against the app's configured origins.

RUM is **always-enabled** — there is no `[rum]` config section anymore. Apps live in the `rum_apps`
table of the shared control-plane SQLite DB (`[storage].db_path`), managed entirely at runtime
through `RumAppStore`/`SqliteRumAppStore` (`crates/photon-api/src/rum_apps.rs`, mirrors the
`users.rs` store pattern: a `Mutex<Connection>`, WAL mode, `CREATE TABLE IF NOT EXISTS`). `RumApi`
(`crates/photon-api/src/rum.rs`) wraps the store with a live in-memory cache (keyed by app `key`,
rebuilt after every mutation) that the beacon handler and the CORS layer both read on the hot path —
no DB hit per request. Register/edit/rotate/remove apps from the UI (`/rum` → "Manage apps") or the
session-authed management API:

| Route | Purpose |
|---|---|
| `GET /api/rum/apps` | list full app records, including the public `key` |
| `POST /api/rum/apps` | register an app; the server mints the `pk_live_<uuid>` key (201; 400 invalid fields; 409 duplicate name) |
| `PATCH /api/rum/apps/:name` | update `allowed_origins`/`sample_rate`/`rate_limit` (name + key immutable) (200; 404 unknown; 400 invalid) |
| `POST /api/rum/apps/:name/rotate-key` | mint a fresh key, invalidating the old one (200 `{key}`; 404 unknown) |
| `DELETE /api/rum/apps/:name` | unregister an app (204; 404 unknown) |

`name` is the immutable identity (`service.name`); there is no rename endpoint. The public `key` is a
client identifier, not a secret — the actual browser auth boundary is the Origin allowlist.

CORS for the beacon is `tower-http`'s `AllowOrigin::predicate`, reading the live `RumApi` cache on
every preflight/request — **scoped to `/api/rum` only** (the rest of `/api` stays session-gated), and
a newly-registered app's origin works immediately with **no server restart**. An unregistered app (a
beacon `key` not in the cache, or an `Origin` outside its allowlist) → **403**, not the old
404-when-disabled — since RUM has no "off" state anymore, only an empty registry.

Rejection matrix: bad/absent key or Origin mismatch → **403**; malformed body → **400** (never
partial write).

### Enrichment

- **UA → device/browser**: small dependency-free server-side parser at ingest (required).
- **IP → geo (`geo.country`)**: optional/opt-in, same pattern as optional `[storage.durable]` —
  behind config with a bundled MaxMind GeoLite2 DB the operator supplies. Omit it and geo facets
  are simply absent.

## Query layer (`photon-query`)

Thin RUM helpers over the existing engine — no new storage reads:

- **`rum_vitals`** — per app + window + filters: **p75** of each vital (via
  `approx_percentile_cont(0.75)` — the same t-digest aggregate used for span latency), the rating
  distribution (% good / needs-improvement / poor by the fixed Google thresholds), and a p75 time
  series. *Note:* `Agg` exposes P50/P90/P99 but not P75, so RUM computes 0.75 directly rather than
  extending the metrics `Agg` enum.
- **`rum_breakdown`** — the same vitals grouped by one dimension (route / device / browser / country
  / connection). A group's `pageviews` is the max per-metric sample count across LCP/INP/CLS **and
  `web_vitals.view_duration`** — the latter is one point per finalized view, so routes reached only
  by clean soft navigations (which emit no LCP/INP/CLS) still rank in the pages list.
- **`rum_page_detail`** — vitals + attribution sub-part aggregates + per-segment breakdown + top
  error issues, scoped to one route.
- **`rum_errors`** — group ERROR rows by `rum.error.fingerprint` into `ErrorIssue { fingerprint,
  exception_type, message, count, sessions, trace_id }`: total occurrence `count`, an exact
  `sessions` (`COUNT(DISTINCT session.id)`), a representative (lexicographic-min) sample
  `exception_type`/`message`, and a representative `trace_id` (lexicographic-min over the group,
  `None` if no error in the group carried one) that lights the list row's "Related ▾" trace jump.
  Takes an optional `route` scope (page detail) and an optional resolved log-grammar `query` (the
  UI's `q` search param on `/api/rum/errors`), ANDed on top via the same `base_predicate` fold Logs
  search / `/api/facet` use. **`trace_id` is populated only for errors ingested after the SDK's
  `tracing` module shipped** — see "The beacon" above.
- **`rum_error_detail`** — full read-only detail for one fingerprint: header stats (first/last seen,
  occurrences, sessions, a representative exception type/message/kind), an occurrence-count time
  series (reuses `histogram_over`), top-value tag breakdowns for `browser.name`/`device.type`/
  `browser.route`/`network.connection` (reuses `facet_over`), a representative raw stack trace, and
  the 20 most-recent individual events — each with its own `trace_id`/session/route, so one
  occurrence can jump straight to its trace waterfall or session logs. Returns an all-empty
  `ErrorDetail` (**200**, not 404) when the fingerprint has no rows in the window.

**Google Core Web Vitals thresholds** (rating boundaries), encoded once in `photon-core::rum`:

| Vital | Good ≤ | Poor > |
|-------|--------|--------|
| LCP   | 2.5 s  | 4.0 s  |
| INP   | 200 ms | 500 ms |
| CLS   | 0.10   | 0.25   |
| FCP   | 1.8 s  | 3.0 s  |
| TTFB  | 800 ms | 1.8 s  |

The same `thresholds()` function also carries a Photon-defined (non-Google) pair for the SPA
soft-nav metric: `web_vitals.route_change` good ≤ 1 s / poor > 3 s — there's no official CWV
guidance for in-app transition time. `web_vitals.view_duration` has no threshold entry: it's an
unscored engagement measure, not a rated vital.

## API (`photon-api`, session-authed like the rest of `/api`)

| Route | Purpose |
|---|---|
| `GET /api/rum/apps` | full app registry records (name/key/allowed_origins/sample_rate/rate_limit/created_at) |
| `POST /api/rum/apps` | register a new app (see "Endpoint & auth" above) |
| `PATCH /api/rum/apps/:name` | update an app's origins/sampling/rate limit |
| `POST /api/rum/apps/:name/rotate-key` | rotate an app's public key |
| `DELETE /api/rum/apps/:name` | unregister an app |
| `GET /api/rum/vitals` | the vital scorecards — the 5 Core Web Vitals plus `route_change` whenever the window has soft-nav samples (p75 + distribution + trend) |
| `GET /api/rum/vitals/breakdown` | breakdown table by `dimension` |
| `GET /api/rum/pages` | pages list |
| `GET /api/rum/pages/detail` | page detail (vitals + attribution + breakdown + errors) |
| `GET /api/rum/errors` | error issues (grouped by fingerprint); optional `q` log-grammar filter (same syntax as Logs search) — a malformed `q` yields a 400 with a byte `offset` |
| `GET /api/rum/errors/facets` | top values + counts for each of the 6 fixed facet fields (`exception.type`, `error.kind`, `browser.route`, `browser.name`, `device.type`, `network.connection`), scoped to ERROR rows and optionally the same `q` filter; registered ahead of `:fingerprint` below so axum matches the static segment first |
| `GET /api/rum/errors/:fingerprint` | issue detail (`rum_error_detail`): header stats + occurrence series + tag breakdowns + sample stack + recent sample events |
| `POST /api/rum` | **public** ingest beacon (the only unauthenticated, CORS-enabled RUM route) |

## The SDK — `@photon/rum` (`sdk/rum/`)

A thin wrapper, not a re-implementation: it vendors Google's `web-vitals` library (correct INP/CLS
windowing, LCP finalization, bfcache handling) and adds error capture, context, batching, and the
beacon. One integration call, `initPhoton(opts)` (see `sdk/rum/src/index.ts`):

```ts
initPhoton({
  app: "web-storefront",     // → service.name
  endpoint: "https://photon.example.com",
  key: "pk_live_…",          // public app-key
  sampleRate: 1.0,            // client-side session sampling
  routeOf: (path) => …,       // optional path → route-pattern mapper
  attribution: true,          // opt-in per-page attribution (dynamic import — keeps base tiny)
  tracing: true,               // opt-in W3C traceparent propagation (dynamic import — keeps base tiny)
  tracePropagationTargets: ["same-origin"], // default; strings are exact origins, RegExp tests the full URL
});
```

**SPA / soft-navigation tracking (on by default)** — `view.id` is a logical pageview, not a document
load: `spa.ts` patches `history.pushState`/`replaceState` and listens for `popstate` to
detect a real client-side route change and rotate the view (query/hash-only changes don't rotate;
MPAs are unaffected). Each soft-navigated view gets its own `CLS`/`INP` plus a `route_change`
DOM-settle heuristic, its own trace id (see below), and the beacon fields `seq`/`prev`/`nav`/`dur` —
see "Sessions & pageviews" and "The beacon" above for the full model. Routers that want to drive the
boundary themselves (instead of relying on auto-detection) can call the exported
`trackView(route?)` escape hatch.

**Trace propagation (opt-in, `tracing: true`)** — mints one W3C trace id per view (32 lowercase
hex via `crypto.getRandomValues`; a no-op if `crypto` is unavailable) and patches `window.fetch` +
`XMLHttpRequest` to inject a fresh `traceparent: 00-<trace-id>-<span-id>-01` header on each outgoing
request matching `tracePropagationTargets` — default `["same-origin"]`; other strings are
exact-origin matches, `RegExp`s test the full resolved URL, and off-list origins are never touched
(so an unexpected header can never trip CORS). A fresh trace id is minted again on every soft
navigation, so each route gets its own trace. Once initialized, every beacon from the current view
(vitals *and* errors) also carries its trace id as `trace` (see "The beacon" above), which is what
lets an error's "Related ▾" menu jump straight to its backend trace waterfall. Same never-throw
contract as the rest of the SDK: any internal failure leaves `fetch`/`XHR` un-instrumented.

**Layout:** `src/index.ts` (`initPhoton` + vitals/error wiring + the `trackView` re-export),
`view.ts` (the view lifecycle owner: descriptor `{ id, route, path, seq, prevRoute, nav, navStart }`
+ `initView`/`rotateView`/`currentView`/`onViewChange` — the single source of truth for the current
view), `spa.ts` (the SPA soft-navigation detector: History-API patching, per-view CLS/INP
observers, the `route_change` DOM-settle heuristic, and the `trackView` export), `session.ts`
(in-memory session id — the view id now lives in `view.ts`), `context.ts` (UA + connection),
`errors.ts` (capture + dedup + rate-limit), `beacon.ts` (per-view buffer + flush — a fresh buffer
per view, flushed with the outgoing view's descriptor on each rotation, carrying the view's own trace
id), `attribution.ts` (tree-shakeable, dynamically imported), `trace.ts` / `tracing.ts` (opt-in W3C
trace propagation — id-gen (per view, cached on the view descriptor) + target matching, and the
fetch/XHR patching; tree-shakeable, dynamically imported only when `tracing: true`).

**Distribution — two builds, one codebase** (`tsup.config.ts`):

- **ESM** via npm (`@photon/rum`) for bundler apps (tree-shakeable).
- **IIFE `<script>`** drop-in (global `PhotonRUM`) for HTTP-only / no-bundler / legacy apps.

**Performance budget — CI-enforced:**

- **< 5 KB gzipped** for the full core; `scripts/size-check.mjs` (run via `bun run size`) hard-fails
  the build over budget. Attribution is a tree-shakeable / dynamically-imported module.
- **Zero critical-path blocking:** load `async`/`defer`; `PerformanceObserver` is passive; work is
  deferred; no synchronous layout reads.
- **Beacon at the right moments:** buffer, then flush via `sendBeacon` on `visibilitychange`→hidden
  and `pagehide` (with `fetch(keepalive:true)` fallback) — not one request per metric.
- **Resilient & cheap:** small ring buffer, capped payload, client-side error dedup + rate-limit,
  feature-detect every API, and **never throw into the host app** (all capture is wrapped;
  SDK-internal errors are silently dropped).

## UI — `/rum`

A dedicated top-level nav section (**RUM**), not folded into Services. Frontend apps still exist as
services underneath so correlation works, but the Services list stays backend-focused.

| Route | View | Purpose |
|---|---|---|
| `/rum` | `RumAppsView` | **executive summary across all apps**: fleet KPI strip, fleet-wide CWV band, a ranked-by-health apps table, a cross-app "live issues" feed + slowest routes |
| `/rum/:appId` | `RumVitalsView` | the hero — five vital scorecards + breakdown table |
| `/rum/:appId/pages` | `RumPagesView` | routes ranked by traffic |
| `/rum/:appId/pages/:route` | `RumPageDetailView` | route-scoped vitals + errors on that page |
| `/rum/:appId/errors` | `RumErrorsView` | JS errors grouped into issues (Sentry-style), with a search bar + fixed facet panel |
| `/rum/:appId/errors/:fingerprint` | `RumErrorDetailView` | one issue: hero summary, occurrence-series chart, tag breakdowns, sample stack, recent sample events (each with its own trace/logs jump) |

The visual **LCP attribution panel** (`LcpAttributionBar` — the segmented "why is LCP slow here?"
sub-part bar) is live in `RumPageDetailView`: `GET /api/rum/pages/detail` returns an
`attribution.lcp` object (avg of the LCP sub-part gauge metrics + the top LCP element), and the
component renders the dominant sub-part with an actionable insight line.

Domain components in `frontend/src/components/rum/`: `WebVitalScorecard`, `VitalsDistributionBar`,
`RumBreakdownTable`, `ErrorIssueList`, `RumErrorFilters` (the errors search facet panel), and the
executive-summary set `RumFleetKpis` / `RumAppsTable` / `RumIssuesFeed` / `RumSlowestRoutes` (each
with a co-located test). The `/rum` overview aggregates with **`frontend/src/lib/rumSummary.ts`** —
a pure, table-tested module (fleet KPIs, fleet-wide vital band, ranked apps, top issues, slowest
routes; ratings are always re-derived from the API-supplied `good_max`/`poor_min`, never hardcoded).
Data via `frontend/src/lib/rum/rumQueries.ts` (TanStack Query composables keyed off app + time +
filters: `useRumApps`, `useRumVitals`, breakdown/pages/errors, `useRumErrorDetail`,
`useRumErrorFacets`; the overview fans out one query **per app** with `useRumAppsVitals` /
`useRumAppsErrors` / `useRumAppsPages` via TanStack `useQueries`). Routes are behind the existing
auth guard in `router/index.js`; static sub-paths (`/pages`, `/errors`, `/errors/:fingerprint`) are
declared before the dynamic `:appId` catch-alls so they aren't swallowed. The views reuse the
Services time-range plumbing (RANGE_MS presets, `ns()` converter, `useUrlState({ timeRange })`);
most RUM views have no query grammar, so they add no `SearchBar` to the `ContextBar` — the
"Frontend"/"Frontend › {app}" breadcrumb labels each page instead. **`RumErrorsView` is the
exception:** it reuses the Logs query grammar (`lib/core/queryLang.ts`) over the fixed six
facet fields `/api/rum/errors/facets` returns — a `SearchBar` + `RumErrorFilters` share one `text`
ref (URL-persisted as `q` via `useUrlState`), and facet clicks (`toggle`/`only`/`clear`) rewrite
`text` through the same `queryLang` helpers LogsView uses, so the search bar's pills, the facet
panel's checked state, and the result list can never desync. A malformed `q` surfaces the same
400 + byte-`offset` contract as Logs search.

## Code placement (modules over a new crate — the data rides existing signals)

- `photon-core/src/rum.rs` — beacon types, metric/attr constants, CWV thresholds, UA parser,
  fingerprint, beacon→record mappers (pure, no I/O). No RUM config type — the app registry lives in
  SQLite, not `Config`.
- `photon-api/src/rum_apps.rs` — `RumApp`, `RumAppStore` trait + `SqliteRumAppStore` (the `rum_apps`
  table).
- `photon-api/src/rum.rs` — `RumApi` (store-backed registry + live cache + CRUD), `RumSink` trait,
  `POST /api/rum` beacon handler + CORS predicate, `GET/POST/PATCH/DELETE /api/rum/apps*` management
  handlers, and the other `GET /api/rum/*` read routes.
- `photon-query/src/rum_vitals.rs` + `rum_errors.rs` — the query helpers (`rum_errors`,
  `rum_error_detail`).
- `photon-server/src/main.rs` — `RumWalSink` (writes to the existing metrics + logs WALs) + wiring.
- `sdk/rum/` — the SDK (the `view.ts`/`spa.ts` view-lifecycle + soft-navigation duo, incl. the
  opt-in `trace.ts`/`tracing.ts` trace-propagation pair).
  `frontend/src/{views,components/rum,lib/rum/rumQueries.ts}` — the UI.

## Non-goals (deferred)

Session replay; resource-timing waterfalls / long-tasks / custom marks; an error-issue workflow
(assign/mute/resolve); source-map de-minification of stack traces; searching/filtering by Web Vitals
value; multi-tenant public DSNs / abuse hardening; alerting on RUM signals. (Frontend↔backend trace
correlation via W3C `traceparent` — previously listed here as a deferred "natural M2" — has now
shipped as the SDK's opt-in `tracing` module; see "The SDK" above.)
