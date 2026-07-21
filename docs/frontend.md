# Frontend

The Vue 3 SPA in `frontend/`, embedded into the `photon-api` binary at build time. Deliberately lean.

## Stack

Vue 3 + Vite + Tailwind, **Reka UI** headless primitives (the shadcn-vue equivalent), Lucide icons.
**TanStack Query** (server-state cache) + **Table** (headless tables) + **Virtual** (row windowing),
**uPlot** (charts), **VueUse** (small composables), **Ky** (HTTP). Package manager is **bun** (never
npm). Bootstrapped in `src/main.js` (`createApp(App).use(router).use(VueQueryPlugin)`); `App.vue`
wraps `<router-view>` in one app-wide `TooltipProvider` + `Toaster` and calls `initTheme()`.

**No Pinia.** Server state lives in TanStack Query; within-view state lives in URL params. App-wide
auth/theme/context are a few ad-hoc reactive-ref modules (`lib/core/auth.ts`, `lib/core/theme.ts`,
`lib/core/context.ts` — the app-wide time window + entity scope).

## Design tokens & elevation

The token layer (`src/styles/tokens.css` + `tailwind.config.js`) is the single source of truth for
the look; primitives and chrome consume it rather than hardcoding color/shadow/radius. A near-neutral
base carries one **Photon Cyan** brand ramp (`--brand`/`--brand-strong`/`--brand-foreground`/
`--brand-soft`, ~190°) spent on a **Reserved** reach: active-nav (`NavRail`), the focus ring
(`--ring`), links, selection, key data highlights, single-series chart lines (`useChartTheme.js`), and
an explicit `variant="brand"` button. `--surface-1` (cards/panels) and `--surface-2`
(popovers/drawers/sheets) lift off `--background`; `--card`/`--popover` map onto them. Dark mode sits
on a faintly-cool near-black (`240 8% 7%`, not pure black) so elevation reads. Elevation is
`--shadow-1`/`--shadow-2` (the `--hi` top-highlight is baked in) plus `--sink` for recessed chrome
(inputs, `SearchBar`), exposed as Tailwind `shadow-1`/`shadow-2`/`shadow-sink`/`shadow-hi`. Radius is
Tight (`--radius: 0.5rem`). Tactile hover-lift/press-recess transforms live behind Tailwind's
`motion-safe:` variant, so `prefers-reduced-motion` disables them outright. A `success`/`success-soft`
token (theme-aware green) is the positive counterpart to severity — "healthy/up/good" — while `sev-*`
stays reserved for warn/error/fatal only.

**Physicality budget (density rule — load-bearing):** chrome — buttons, cards, tiles, panels,
overlays, nav — gets elevation plus hover-lift / press-recess / recessed inputs. Dense **data** rows
(tables, log/span rows, waterfall bars) stay **flat and instant**: no shadow, no transform, just
`hover:bg-muted` and a cyan `bg-brand-soft` selection with an inset left border. Don't add shadows or
transforms to a table/row component — it breaks the density contract and can regress virtualized-
scroll perf.

**Tactile "physical key" (`.pk`, `base.css`):** pressable chrome — the solid `button` variants and the
`select-menu` trigger — sits on a hard bottom "lip" that grows on hover and collapses as the control
presses **down** on `:active` (theme-aware; add `.pk-brand` alongside `.pk` for the cyan lip). `.pk`
owns its own shadow/transform/transition, so don't pair it with `shadow-*`/manual `translate`
utilities. Toggles read as lifted **keycaps**: `switch` (thumb taller than the track), `checkbox`
(recessed → lifted cyan keycap with check), and `toggle-group`/`tabs` active items (raised pill,
theme-correct in dark). `.pk` is for button-sized keys only — never data rows.

## Build & embed

`frontend/dist` is built by Vite (`bun run build`) and **embedded into `photon-api` at compile time**
via `rust-embed` (`crates/photon-api/src/assets.rs`, `#[folder = "../../frontend/dist"]`, served with
an SPA fallback to `index.html`). It is gitignored and absent on a fresh checkout — **build the
frontend before the backend** or the UI is a 404 and photon-api's embed tests fail.

## Data-flow pattern

```
Ky client (lib/core/api.ts, with per-method mock fallback)
   → TanStack Query composables (lib/*Queries.ts, keyed off URL/filter/time state via a computed queryKey)
      → views own the server state, pass plain props into pure domain components
```

Within-view state (query/time/facets) lives in **URL query params** (`lib/core/useUrlState.ts`, `?tab=`,
route params) — no store. `lib/core/api.ts` tries `/api` and falls back per-method to the in-browser mock
corpus (`lib/core/mock.ts`) on a **network** failure, while still surfacing real 400/404s; the reactive
`api.mock` flag drives the shell's "demo data" banner.

## Routes → views

| Path | View | Section |
|---|---|---|
| `/` → `/home` | `HomeView.vue` | Home dashboard (landing) |
| `/logs` | `LogsView.vue` | Logs explorer |
| `/traces` · `/traces/:traceId` | `TracesExplorer.vue` · `TraceDetailView.vue` | Traces + waterfall |
| `/services` · `/services/:service` | `ServicesListView.vue` · `ServiceDetailView.vue` | APM |
| `/metrics` · `/metrics/catalog` | `MetricsExplorer.vue` (reused) | Metrics explorer |
| `/rum` · `/rum/:appId` · `/rum/:appId/pages[/:route]` · `/rum/:appId/errors` | `Rum*View.vue` | RUM (apps · vitals · pages/detail · errors) |
| `/uptime` | `UptimeDashboard.vue` | Uptime |
| `/infra` · `/infra/:host` | `InfraHostsView.vue` · `InfraHostDetailView.vue` | Infrastructure (host/GPU resource monitoring) |
| `/data` | `DataView.vue` | Usage / storage / retention |
| `/alerts` | `AlertsView.vue` | Alerts (webhook rules, incidents, channels) |
| `/login` · `/onboarding` | `LoginView.vue` · `OnboardingView.vue` | Auth (public) |

`router/index.js` has a global `beforeEach` guard: cached `hydrate()` auth probe, onboarding-first
gating, then login gating. Static sub-paths are declared **before** dynamic catch-alls (e.g.
`/traces/metrics` before `/traces/:traceId`; `/rum/:appId/pages` before `/rum/:appId`).

NavRail (`components/common/NavRail.vue`) groups routes into ownership **worlds** rather than a flat
per-signal list: an ungrouped **Home** entry, then three worlds — **Frontend** (→ `/rum`), **Backend**
(→ `/services`), **Infrastructure** (two items: **Hosts** → `/infra`, **Ops** → `/uptime`) — Frontend
and Backend are each today a single landing item into an existing route (room to grow their own
sub-nav later), then **Explore** (Logs · Traces · Metrics, the raw per-signal browsers) and **Manage**
(Data, then Alerts — the cross-signal webhook alert engine). `AppShell.vue` derives which group to
highlight from the route via a `ROUTE_GROUP` map (e.g.
`/rum` → `frontend`, `/infra` → `infra`) and the inverse `LANDING` map picks the route a NavRail click
pushes to.

## Component groups (`src/components/`)

- **`charts/`** — the shared uPlot layer: `BaseChart`/`LineChart`/`BarChart`, `MiniAreaChart` (a
  compact, interactive inline trend chart — `LineChart`'s `compact` mode: axes/legend hidden, tight
  padding, a `height` prop, still the real uPlot engine underneath with crosshair + hover tooltip),
  `ChartPanel`, `ChartTooltipCard`, plus pure option builders (`chartOptions.js`), the lifecycle
  composable (`useUplot.js`), and theme-aware colors (`useChartTheme.js`).
- **`common/`** — app chrome & toolbar widgets: `AppShell`, `NavRail`, `ContextBar` (the single
  consolidated header row, mounted once by `AppShell`: breadcrumb + scope chip on the left, a
  middle `search` slot, and the global time picker + `LiveControl` on the right — searchable views
  forward their `SearchBar` into that slot via `AppShell`'s `toolbar` slot; non-searchable views
  leave it empty and the breadcrumb labels the page), `PhotonMark`, `SearchBar` (query-language
  input + autocomplete), `RelatedMenu` ("Related ▾" cross-signal destination dropdown),
  `TimeRangePicker`, `ColumnPicker`, `LiveControl` (refresh mode), `ThemeToggle`, `SettingsDialog`,
  `SettingsUsers`.
- **`logs/` · `traces/` · `services/` · `metrics/` · `uptime/` · `data/` · `rum/` · `infra/` ·
  `alerts/`** — the per-signal domain components (see the per-feature docs in
  [`subsystems/`](subsystems/) for the file lists). `infra/` (host/GPU resource monitoring):
  `HostFleetKpis.vue` (the `/infra` fleet KPI band — host/warning/critical counts, avg CPU, GPU
  hosts, derived client-side from the host list) + `HostCard.vue` (the host card grid replacing the
  old `HostTable.vue` — CPU/MEM/DSK/GPU meters, a worst-resource degraded flag, last-seen),
  `HostStatTiles.vue` (the
  `/infra/:host` glance stat-tile row — last-point derivation off a shared series bundle, 80%/90%
  warn/error tint), `HostResourcePanels.vue` (presentational per-resource trend cards — each chart in
  a titled `charts/ChartPanel`: CPU total/per-core toggle + load average, memory/network, disk
  meters, a 4-card GPU section — reads the same series bundle rather than owning its own queries; see
  [`subsystems/infra.md`](subsystems/infra.md)). `alerts/` (the webhook alert engine, cross-signal):
  `AlertStatBand`, `AlertRulesTable`/`AlertRuleRow` (its "Browse templates" button + empty-state link
  open the quick-setup picker), `AlertRuleDialog`/`ConditionBuilder` (the plain-English condition
  builder — `AlertRuleDialog` also accepts an optional `:seed` prop, a partial `RuleInput` that
  pre-fills *create* mode, honored only when `:rule` is null), `TemplatePickerDialog`/`TemplateRow`
  (the target-first template quick-setup picker: pick Service/App/Host/Global → Apply directly or
  Customize into a `:seed`-ed `AlertRuleDialog`; templates come from `lib/alertTemplates.ts`),
  `IncidentsTable`, `ChannelsGrid`/`ChannelCard`/`ChannelDialog` — see
  [`subsystems/alerts.md`](subsystems/alerts.md).
- **`ui/`** — the shared primitive library (below).

## The `ui/` primitive library

Each folder has an `index.js` barrel. **Reka-UI/shadcn ports use `<script setup lang="ts">`**; the
**Photon-authored composites use plain JS** (the only plain-JS files under `ui/`).

**shadcn-vue ports (TS):** `alert`, `badge`, `button`, `card`, `checkbox`, `dialog`, `dropdown-menu`,
`form-field`, `input`, `label`, `popover`, `scroll-area`, `select`, `separator`, `sheet`, `skeleton`,
`spinner`, `switch`, `table`, `tabs`, `toast` (+ `Toaster.vue`, `useToast.ts`), `toggle-group`,
`tooltip`. Several picked up the elevation treatment above: `button` adds a `variant="brand"` tactile
CTA (elevation, hover-lift, press-recess — spent sparingly); `card` adds an additive `interactive`
prop that adds hover-lift for clickable cards; `input` is recessed (`shadow-sink`); `dialog`/
`popover`/`sheet` sit on `--surface-2` + `shadow-2`; `table` rows follow the flat/instant density
rule above.

**Photon-authored primitives:**

| Primitive | Purpose | Lang |
|---|---|---|
| `segmented` | segmented control on Reka ToggleGroup (traces/spans, table/cards); active item gets the raised pill | TS |
| `nav-tabs` | sub-navigation tab bar (route sub-nav) | JS |
| `stat-tile` | KPI tile: label + value + delta-arrow trend, optional `sub` caption + a `#spark` slot for an inline sparkline; raised, hover-lifts | TS |
| `sparkline` | inline-SVG polyline sparkline with last-point dot | JS |
| `status-dot` / `status-pill` | small tone-colored dot / pill label | TS |
| `peek-drawer` | Sheet-based side drawer with prev/next stepping through a list | JS |
| `meter` | horizontal 0–1 proportion bar, tone-colored | TS |
| `select-menu` | compact toolbar dropdown-select on Popover (jsdom-testable, unlike Reka Select); recessed field | JS |
| `empty-state` | no-data placeholder | TS |
| `kbd` | keyboard-key chip | TS |
| `number-field` | numeric form control; recessed field | TS |
| `date-picker` | single-date popover picker modeled on `TimeRangePicker` (preset "older than" cutoffs + a specific calendar date); emits an ISO `YYYY-MM-DD` string | TS |
| `facet` | the Fields facet catalog (search + promoted/attribute grouping + `useQueries` fan-out); data-source-agnostic via an injected page adapter | JS |

> The **switcher rule**: `Segmented` = in-place toggle between mutually-exclusive views; Reka `Tabs` =
> in-page panels; `nav-tabs` = route-level sub-navigation. Don't conflate them.
>
> The RUM `WebVitalScorecard` is a **domain** component (`components/rum/`), not a `ui/` primitive —
> there is no generic `scorecard` in `ui/`.

## `lib/` reference

`lib/` is organized **by signal/domain**: thin per-signal folders plus one `core/` for everything
cross-cutting. Imports use the extensionless alias form `@/lib/<folder>/<name>`.

**`core/` — shared foundations used across signals:**
- *Transport:* `api.ts` (Ky + mock fallback), `mock.ts` (in-browser mock corpus), `liveStream.ts`
  (`EventSource` wrapper for live-tail SSE).
- *App-wide reactive state & navigation:* `auth.ts`, `context.ts` (app-wide time range + entity scope
  — the same module-singleton pattern as `auth.ts`/`theme.ts`; sole owner of the
  `range`/`from`/`to`/`scope` URL keys), `useUrlState.ts` (per-view `svc`/`sev`/`q`; merge-preserves
  every other key, including the context ones), `useCorrelate.ts` (`correlate()` builds a same-app
  link that always carries the current time+scope; `relatedFor()` is the per-entity-kind
  related-destination graph behind `RelatedMenu`).
- *Search DSL & generic composables:* `queryLang.ts` (display-only lexer mirroring the Rust parser —
  never validates), `useTableColumns.ts` (persisted column visibility), `useLiveTail.ts` (refresh-mode
  → stream-or-poll + ring buffer with pause-on-scroll), `listNav.ts` (pure j/k stepping), `useCopy.ts`.
- *Formatting / color / geometry:* `format.ts` (severity model + number/duration/timestamp
  formatters), `color.ts`, `seriesColor.ts` (stable hash → palette), `signalMeta.ts` (per-signal color
  + Lucide icon identity for logs/traces/metrics/uptime — `signalColor()`/`signalIcon()`, keyed on
  `seriesColor()` so a signal's hue matches its chart series), `histogram.ts`, `theme.ts`, `utils.ts`
  (`cn()` = clsx + tailwind-merge, used by every primitive).
- *Account:* `usersQueries.ts` (TanStack Query composable for UI users).

**Per-signal folders** — each holds that signal's TanStack Query composable (one per signal, all
follow the same contract) plus its signal-specific helpers:
- **`logs/`** — `logsQueries.ts`, `fields.ts` (autocomplete catalog).
- **`traces/`** — `tracesQueries.ts`, `traceTree.ts` (BigInt-ns waterfall assembly), `spanFields.ts`.
- **`metrics/`** — `metricsQueries.ts`, `metricFields.ts`, plus the pure metrics utilities (no Vue):
  `metricNamespaces.ts` (namespace grouping + search ranking), `metricFavorites.ts` (favorites/recent
  with `localStorage` backing), `quickStarts.ts` (curated + type-relative quick-start definitions),
  `metricViz.ts` (viz registry, stat summary, bucket building, viz URL codec).
- **`rum/`** — `rumQueries.ts`, `rumSummary.ts` (fleet executive-summary derivation).
- **`services/`** — `servicesQueries.ts`, `serviceHealth.ts`, `serviceColor.ts` (stable hash → palette).
- **`uptime/`** — `uptimeQueries.ts`.
- **`data/`** — `dataQueries.ts`.
- **`infra/`** — `infraQueries.ts` (`useInfraHosts`/`useInfraHost`/`useInfraHostSeries`, plus
  `useHostResourceSeries` — the nine-resource query bundle for one host-detail view) and
  `hostStats.ts` (pure last-point/worst-series/threshold helpers for the glance tiles).

**Cross-signal:**
- `alertsQueries.ts` (root `lib/`, not a per-signal folder — the alert engine spans metrics/logs/
  traces/RUM) — `useRules`/`useChannels`/`useIncidents`/`usePreview` (all poll ~15s except the
  on-demand preview) + create/update/delete/toggle/test mutations for rules and channels.
- `alertTemplates.ts` (root `lib/`) — the static, read-only 23-template quick-setup catalog (Service/
  App/Host/Global) + `build(target)` target substitution, consumed by `TemplatePickerDialog`; frontend-
  only, no backend counterpart.

## Conventions

See [`conventions.md`](conventions.md#frontend-vue). The load-bearing ones: bun not npm; no Pinia;
`ui/` primitives in TS and views/domain components in JS; **ns strings** at the query boundary /
**ms Numbers** at the chart boundary / **BigInt ns** for trace geometry; query composables normalize
refs-or-getters with `toValue` into a `computed` queryKey; the `api.ts` mock fallback must keep working.
