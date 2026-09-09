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
`lib/core/context.ts` — the app-wide time window, federation tenant (Site), and deployment
environment).

## Design tokens & elevation

The token layer (`src/styles/tokens.css` + `tailwind.config.js`) is the single source of truth for
the look; primitives and chrome consume it rather than hardcoding color/shadow/radius. A near-neutral
base carries one **Photon Cyan** brand ramp (`--brand`/`--brand-strong`/`--brand-foreground`/
`--brand-soft`, ~190°) spent on a **Reserved** reach: active-nav (`NavRail`), the focus ring
(`--ring`), links, selection, key data highlights, single-series chart lines (`useChartTheme.js`), and
an explicit `variant="brand"` button. Dark mode sits on a faintly-cool near-black (`240 8% 7%`, not
pure black). A `success`/`success-soft` token (theme-aware green) is the positive counterpart to
severity — "healthy/up/good" — while `sev-*` stays reserved for warn/error/fatal only.

**Separation comes from the canvas, not from shadow (load-bearing).** `--background` is a visibly
grey sheet (`235 16% 96.5%`) and `--surface-1` is pure white on top of it, so a card reads as a card
with nothing but a hairline `--border` and a 1px ambient shadow. Do **not** lighten `--background`
back toward white — the whole flat elevation ramp depends on that ~3.5% gap. `--card` maps to
`--surface-1`; `--popover` maps to `--surface-2` (popovers / drawers / dialogs), the only layer that
carries real depth. Elevation is exactly three steps, exposed as Tailwind `shadow-1`/`shadow-2`/
`shadow-sink`: `--shadow-1` is a hairline ambient that only stops white dissolving into the canvas,
`--shadow-2` is the one real float, `--sink` is the pressed-in state. There is no `--hi`
top-highlight and no `shadow-hi` utility any more.

**Radius is one family derived from `--radius` (0.75rem):** `rounded-sm` 6px (menu rows, checkboxes,
swatches), `rounded-md` 10px (controls — buttons, inputs, toolbar triggers), `rounded-lg` 12px
(panels, segmented/tab tracks), `rounded-xl` 14px (cards, tables-in-a-panel, alerts, toasts),
`rounded-2xl` 18px (dialogs, the login card). Pills — badges, status pills, entity chips — use
`rounded-full` and sit outside the scale. Pick by *what the thing is*, not by eye.

**`AppShell`'s `timePicker` prop:** every view keeps the one global `TimeRangePicker` in the
ContextBar, except `/home`, which passes `:time-picker="false"` and puts its own bounded Watch Range
picker in the `actions` slot. Two time controls in one header would be two sources of truth; see
[`adr/0001`](adr/0001-landing-page-is-an-attention-board.md).

**Canvas vs sheet (`AppShell`'s `surface` prop):** dashboards (`/home`, `/infra`, `/alerts`, `/data`,
`/rum`, `/services`, `/uptime`) stay on the default `surface="canvas"` — the grey canvas is what makes
their white cards read as cards, so **every table on a canvas view must own a panel**
(`overflow-hidden rounded-xl border border-border bg-card shadow-1`, as `ServicesTable`/
`RumBreakdownTable`/`DependencyTable` do). The dense explorers (`/logs`, `/traces`, `/traces/:id`,
`/metrics`) pass `surface="sheet"` for one edge-to-edge white working surface, because their tables
are full-bleed and own no panel.

**Physicality budget (density rule — load-bearing):** chrome — buttons, cards, tiles, panels,
overlays, nav — carries the hairline + ambient shadow. Dense **data** rows (tables, log/span rows,
waterfall bars) stay **flat and instant**: no shadow, no transform, just `hover:bg-muted/60` and a
cyan `bg-brand-soft` selection with an inset left border. Don't add shadows or transforms to a
table/row component — it breaks the density contract and can regress virtualized-scroll perf.

**Pressable chrome (`.pk`, `base.css`):** the solid `button` variants and the `select-menu` trigger
sit FLAT on the surface (`--shadow-1`, nothing more) and sink 1px on `:active` (`--sink`). No bevel,
no bottom lip, no hover lift — depth is spent on things that actually float. The transform lives
behind Tailwind's `motion-safe:`/`prefers-reduced-motion` guard. `.pk` owns its own
shadow/transform/transition, so don't pair it with `shadow-*`/manual `translate` utilities, and don't
put it on `outline`/`ghost` variants (flat by definition) or on data rows. Toggles are flat too:
`switch` is a pill track with a plain circular thumb inside it, `checkbox` is a 6px-radius box that
fills with brand, and `segmented`/`nav-tabs`/`tabs`/`toggle-group` active items are a white pill on a
`bg-muted` track — `bg-surface-1` + `shadow-1` + `ring-1 ring-inset ring-border` (and `bg-secondary`
in dark). All four switchers share that one look; what differs is what they drive.

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

### The environment dimension

`deployment.environment` is the **third global context dimension**, alongside the time Window and the
federation Site (`tenant`), and it applies to every signal — not just Backend. It is a promoted
attribute (`[schema].promoted_attributes` in `photon.example.toml`), so it filters through the ordinary
query grammar: `contextQueryTerm()` contributes `deployment.environment:<x>`.

Three choices in `ContextBar.vue` / `lib/core/context.ts` that look arbitrary and are not:

- **The picker hides below two values.** It is populated from `GET /api/facet?field=deployment.environment`
  **unioned with** `GET /api/traces/facet?field=deployment.environment`, both over the active window — the
  dimension is global, and a trace-only install (APM first, logs later) would never see the picker off the
  logs facet alone. An install whose producers declare no environment, or all declare the same one, has
  nothing to choose between — a picker over a constant is a control that cannot change anything. Same rule
  that hides the tenant picker on an empty registry.
- **`prod` is the default when it exists.** Prod is the environment worth waking someone for, and averaging
  staging's numbers into it is how a real regression gets hidden. Everything else stays one click away
  rather than selected for the reader. The default applies **once** per session (`environmentDefaulted`),
  so clearing the picker sticks instead of snapping back on the next facet refetch; an explicit `?env=` in
  the URL wins over it.
- **Nothing selected means no term at all** — never `deployment.environment:*`. Records whose producer
  declares no environment belong to no environment (Photon never infers one), and `:*` would drop them.

The environment facet itself is the one query that does **not** apply the environment term
(`lib/core/environments.ts`): picking `prod` would otherwise collapse the facet to a single value and hide
the picker that was just used. It does apply the Site term — environments are per-Site.

## Routes → views

| Path | View | Section |
|---|---|---|
| `/` → `/home` | `HomeView.vue` | The **Attention Board** (landing) — see [`adr/0001`](adr/0001-landing-page-is-an-attention-board.md) |
| `/logs` | `LogsView.vue` | Logs explorer |
| `/traces` · `/traces/:traceId` | `TracesExplorer.vue` · `TraceDetailView.vue` | Traces + waterfall |
| `/services` · `/services/errors` · `/services/map` | `ServicesListView.vue` · `BackendIssuesView.vue` · `ServiceMapView.vue` | Backend: the fleet board, its Issues, its topology |
| `/services/:service` (+ `/operations[/:operation]` · `/errors[/:fingerprint]` · `/dependencies` · `/deployments`) | `ServiceDetailView.vue` + the sub-route views | APM detail — each tab owns its own queries |
| `/metrics` · `/metrics/catalog` | `MetricsExplorer.vue` (reused) | Metrics explorer |
| `/rum` · `/rum/:appId` · `/rum/:appId/pages[/:route]` · `/rum/:appId/errors` | `Rum*View.vue` | RUM (apps · vitals · pages/detail · errors) |
| `/uptime` | `UptimeDashboard.vue` | Uptime |
| `/infra` · `/infra/:host` · `/infra/:host/containers` · `/infra/:host/agent` | `InfraHostsView.vue` · `InfraHostDetailView.vue` · `InfraHostContainersView.vue` · `InfraHostAgentView.vue` | Infrastructure (host/GPU resource monitoring; the Agent tab is Docker fleet management, see [`subsystems/deploy.md`](subsystems/deploy.md)) |
| `/data` | `DataView.vue` | Usage / storage / retention |
| `/alerts` | `AlertsView.vue` | Alerts (webhook rules, incidents, channels) |
| `/tenants` | `TenantsView.vue` | Federation: live fleet-health board + tenant registry (central-side) |
| `/deploy/templates` · `/deploy/templates/:id` | `TemplatesView.vue` · `TemplateDetailView.vue` | The Deploy world: the template catalog and the deploy wizard (see [`subsystems/deploy.md`](subsystems/deploy.md)) |
| `/deploy/rollouts` · `/deploy/rollouts/:id` | `RolloutsView.vue` · `RolloutDetailView.vue` | The Deploy world: staged rollouts of a template version (see [`subsystems/deploy.md`](subsystems/deploy.md)) |
| `/deploy/stacks` · `/deploy/stacks/:id` | `StacksView.vue` · `StackDetailView.vue` | The Deploy world: compose stacks under desired state (see [`subsystems/deploy.md`](subsystems/deploy.md)) |
| `/audit` | `AuditView.vue` | The audit log — every mutation Photon made to a host (see [`subsystems/deploy.md`](subsystems/deploy.md)) |
| `/settings/users` (`/settings` redirects here) | `UsersView.vue` | Who can sign in and what each may change: the paged users table (in-place role picker for an admin, a key that sets a password — your own with the current one, anyone's as admin — and a Remove that explains a refusal), the "Add a user" modal with inline validation, a password confirmation and role cards, and the anti-lockout note (see [`subsystems/auth.md`](subsystems/auth.md)). Reached from the rail's gear; replaced the Settings modal |
| `/login` · `/onboarding` | `LoginView.vue` · `OnboardingView.vue` | Auth (public) |

`router/index.js` has a global `beforeEach` guard: cached `hydrate()` auth probe, onboarding-first
gating, then login gating. Static sub-paths are declared **before** dynamic catch-alls (e.g.
`/traces/metrics` before `/traces/:traceId`; `/rum/:appId/pages` before `/rum/:appId`).

NavRail (`components/common/NavRail.vue`) groups routes by **what the operator is trying to do**
rather than as a flat per-signal list: an ungrouped **Home** entry, then four groups, each headed by a
verb —

- **Monitor** — the health of what you run, ordered outside-in: **Frontend** (→ `/rum`), **Backend**
  (→ `/services`), **Hosts** (→ `/infra`), **Uptime** (→ `/uptime`). These are the ownership *worlds*
  the Attention Board rolls up by; the item labelled "Ops" is now "Uptime" (the old word named nothing
  a first-time user could guess, and `/uptime` is what it opens).
- **Explore** — the raw signals underneath those judgments: Logs · Traces · Metrics.
- **Deploy** — changing what runs: Templates · Stacks · Rollouts.
- **Manage** — Photon itself: Alerts (the cross-signal alert engine), Data (storage + retention),
  Tenants (the federation registry), Audit (what Photon changed on your hosts).

The previous layout was seven groups for sixteen items — two one-item worlds whose headings were
suppressed, and an "Infra" heading over Hosts + Ops — which read as a list with random gaps. Each
heading now sits on a hairline so the four groups read as four groups in a 74px column. A heading
has room for ~10 characters at its 9px uppercase/`tracking-wider` size ("Infrastructure" once
overflowed both edges and was clipped by the rail border; "Monitor" fits) and `truncate`s so an
over-long one clips cleanly instead of bleeding. The group column scrolls with a hidden scrollbar
(and a bottom fade while it actually overflows) rather than pushing the account cluster off a short
viewport.

**Every item has its own two-tone icon** (`components/common/nav-icons/`, one SFC per item wrapping
`NavIconBase.vue`). Two colours only: the NEUTRAL parts inherit `currentColor` (muted grey idle, the
foreground ink on the active item) and the ACCENT parts read the `--ni-accent` custom property, which
NavRail sets to Photon Cyan — softened to 70% while idle so fifteen accents stay a rail and not a
rainbow, full strength on hover and on the active item. Each accent is the part that *does* the
item's job: the heartbeat inside Home's house, the user's cursor in Frontend's browser window, the
needle on Backend's gauge, the lit LEDs on Hosts' rack, the pulse through Uptime's probe ring, the
highlighted line in Logs, the selected span in Traces' waterfall, the trend over Metrics' bars, the
header block of Templates' layout, the top layer of Stacks, Rollouts' rocket window + flame, Alerts'
badge, the stored band of Data's cylinder, the Central hub among Tenants' sites, and Audit's check.
The classes are `ni-accent` (accent stroke), `ni-accent-fill` (accent fill + stroke) and `ni-fill`
(neutral fill); outside the rail an icon falls back to the plain brand token. `AppShell.vue` derives
which item to highlight from the route via a `ROUTE_GROUP` map (e.g. `/rum` → `frontend`, `/uptime`
→ `uptime`) and the inverse `LANDING` map picks the route a NavRail click pushes to.

## The Attention Board (`/home`)

The landing page answers ONE question: **is anything wrong, and where?** It shows judgments, not
readings. The decisions behind it — and the alternatives that were rejected — are in
[`adr/0001-landing-page-is-an-attention-board.md`](adr/0001-landing-page-is-an-attention-board.md);
the vocabulary (Subject, Attention Level, Silence, Watch Range, Spread, Unattributed Incident, Site,
Central) is in [`../CONTEXT.md`](../CONTEXT.md). What matters for the code:

- **One axis, and it is defined once.** Every watched thing becomes a `Subject` at one of five
  levels — **Down** (not answering) / **Critical** / **Warning** / **OK**, plus **Unknown**, which
  is the *absence* of a judgment and never a colour. The axis itself (`AttentionLevel`,
  `LEVEL_RANK`, the `STATUS_META` display table, `INCIDENT_LEVEL`) lives in
  `lib/services/serviceHealth.ts` and `attention.ts` re-exports it as `LEVEL_META` &co. — the
  Services pages and `/home` speak the same five words, so nothing can read "Critical" on one page
  and "Degraded" on the other in the same second. `attention.ts` adds no fourth opinion about what
  "bad" means: it maps `serviceHealth.ts`'s `serviceLevel()`, `hostStats.ts`, the API's own
  Web-Vital `rating` and the tenants API's own up/stale/down onto that axis, and keeps each kind's
  native word verbatim in the reason line.
- **The Watch Range is not the Window.** `watchRange.ts` owns a bounded 5m/15m/1h/6h lookback and
  the `watch` URL key. Drilling out calls `applyWatchRangeToWindow()` *before* `correlate()`, so the
  destination shows the same slice of time.
- **Silence is Down.** A Subject that reported and then stopped is Down; one that never reported is
  Unknown. Detected client-side against the previous equal-length range (`InfraHost.lastSeenNs`
  gives hosts a finer signal). This is what stops a service falling to zero traffic from reading as
  a calm neutral nothing and disappearing — the old `idle` level covered both facts at once and is
  retired.
- **The Deploy world joins the board once something is under management** (`DEPLOY_WORLD`,
  `stackSubject`, `rolloutSubject`): a failed apply or a rollout paused on failures is Critical,
  drift or a rollout waiting at its gate is Warning — the same five words as everything else.
- **The roll-up cards are the filter.** No tabs. Each card always shows its World's full tallies and
  toggles `?world=<key>` in place.
- **Merging across Sites is Services-only.** `RedRow.tenant` is populated for `group=service`, so
  same-named service rows merge into one Subject carrying a **Spread** (`2 of 3 sites`). Hosts and
  monitors never merge. RUM apps could not merge without one query per Site, so they do not.
- **A tenant filter is a lens, and the board narrows honestly.** Services, RUM apps and hosts carry
  the tenant dimension; uptime and the alert engine do not, so under a lens the **Uptime** world (key `ops`) is
  dropped rather than mislabelled as the Site's, and incidents that resolve to another Site's
  Subject stay out of the Unattributed banner. Per-row Site labelling (the Site badge, the Spread
  chip, the `central` marker) also switches off — the board spans one place and the breadcrumb
  already names it. `useAttentionBoard` exposes this as `showSites`, which is `hasSites && !lensed`,
  NOT `hasSites`: a row the backend never stamped with a `tenant` would otherwise read `central`
  while sitting under a `divtik` filter.

## The Services index (`/services`)

A **fleet board**, not a table with a Site column — see
[`adr/0004`](adr/0004-the-services-index-is-a-fleet-board.md). Three reading layers, in the order a
person needs them, all fed by ONE request (`GET /api/services/overview`) plus a second copy of it for
the previous window:

1. **The Site band** (`SiteBand` → `SiteCard`) — which Sites are even reporting. It *is* the Site
   filter: clicking a card sets the global tenant, clicking the active one clears it. There is no
   separate picker, because a picker that showed no health would make you choose blind. A stale Site
   is tinted, badged `stale`, and renders `—` rather than `0/s`. Renders nothing on a Site install
   (the tenant registry is empty), which is what makes one page work in both deployments.
2. **KPIs + the attention strip** (`MetricTiles`, `AttentionStrip` → `AttentionCard`) — Requests /
   Error rate / p99 / Apdex / Open incidents, then a card per subject that is not OK, worst first,
   capped at 8 with a `+N more` chip that scrolls to the table. The chip is load-bearing: capping
   silently makes a fleet with thirty problems read as having eight.
3. **The table** (`ServicesTable`) — one dense row per `(service, environment)`. Seven columns, all
   on the active Window: `Health · Service · <window> sparkline · Traffic · Errors · Latency p99 ·
   Apdex`. No 30-day error budget, no cumulative data usage — a row that mixes three clocks with none
   of them labelled is how a table starts lying quietly.

Two behaviours in the table are deliberate. Clicking a **row** expands it (`ServiceRowExpansion`:
per-Site child rows plus a top-operations / slowest-operations / worst-dependencies mini-dashboard,
fetched only while open); only the service **name** navigates. Making the whole row navigate is what
made the old table useless for comparison, and the expansion is what replaces a separate compare
mode. One row is open at a time.

The previous-window request is fetched with `buckets=1`: it exists solely so `serviceLevel` can tell
Silence ("it reported, then stopped" → Down) from never-reported (→ Unknown), and 30 unused series
per service is a real cost on a fleet.

**Services · Issues · Map** are three sibling tabs on one bar, not modes of one view. Issues groups
errored spans and Map groups call edges; neither is a filter of the services table, and presenting
them as a toggle would imply they are. See
[`subsystems/services-apm.md`](subsystems/services-apm.md) for what each answers.

The **fleet graph** on `/services/map` is the app's only route that pulls a layout library, and it
pulls it with a dynamic `import()` so `@dagrejs/dagre` lands in its own ~48 kB chunk instead of the
initial bundle. Its node dots read the Attention Level out of `lib/services/serviceHealth.ts` rather
than deriving a second judgment from span counts — `GET /api/services/map` deliberately carries no
level, because a level depends on Apdex and on the previous window and neither is a topology fact.

### Two chart engines, and the measurement that decided the boundary

Photon draws its charts with **uPlot**, except one: the service Overview's **Apdex band chart**,
which renders through **TanStack Charts** (`components/charts/tanstack/`). That is a deliberate,
measured boundary, not a migration in progress.

The evaluation ported the one shape that actually qualified. Of the four candidates, three did not:

| Shape | Ported? | Why |
|---|---|---|
| Apdex band chart | **yes** | A plain stacked bar over evenly-spaced buckets. No brush, no drag-zoom, no drag-to-filter — the interactions TanStack Charts has no documented equivalent for. |
| Latency heatmap | no | Already a plain CSS grid, on purpose. It is a grid of rectangles with a colour scale and backend-supplied axis labels; a chart engine would cost a dependency and lose text selection and hover targets. |
| Row sparkline | no | Already a hand-authored SVG path, 22px tall. There is nothing an engine would add at that size. |
| Deploy markers | no | An annotation on other charts, not a shape of its own. Porting them means porting the charts they sit on, which this boundary excludes. |

`SpanVolumeHistogram` and `LatencyHistogram` stay on uPlot for the reason the plan named: their
drag-to-zoom and drag-to-filter-duration interactions have no documented equivalent.

**The bundle delta, measured on the real build:**

| | Raw | Gzipped |
|---|---|---|
| uPlot — every other chart in the app | 52.35 kB | 23.30 kB |
| TanStack Charts — this one chart | **96.95 kB** | **33.85 kB** |

TanStack Charts costs roughly **1.5x uPlot's entire footprint to draw one chart**. It is therefore
loaded through a dynamic `import()` (`components/charts/tanstack/lazy.ts`), exactly like the fleet
graph's dagre, so it lands in its own chunk and nobody opening `/logs` pays for it: the initial
bundle moved 327.98 kB to 328.95 kB gzipped, a ~1 kB cost for the async wrapper.

Two more things the port surfaced. The Vue adapter is **SVG only** — the canvas surface has React
and Octane entry points, not a Vue one — which is acceptable at Photon's 48-bucket default
(`crates/photon-api/src/histogram.rs`) and is a second reason the uPlot explorers stay put. And the
engine's default colours are `currentColor`, so `chartTheme.ts` resolves `tokens.css` into concrete
`hsl()` values and hands them over explicitly, mirroring what `useChartTheme.js` already does for
uPlot.

`ServiceVolumeChart` (uPlot) and `ApdexBandChart` (TanStack) sit on the same page over the same
buckets on purpose: the two engines can be compared on something real. **A full migration is a
separate decision, and the numbers above are the evidence it should be taken on.**

**Deploy markers** are one helper (`lib/services/deployMarkers.ts`) feeding four chart kinds, so a
release line looks and reads the same on a bar chart and on the latency lines beside it. Charts take
them as `[{ xMs, label, color }]` — wall-clock moments, not points on a series — and `BaseChart`
draws each as a dashed vertical with the version as its label. The helper caps at the **12 newest**:
a service that ships forty times in the window would otherwise turn the plot into a picket fence,
hiding the very thing the markers exist to reveal, and when you are chasing a step in a chart the
release you want is almost always a recent one.

> Note the unit split this exposed: `charts/LineChart.vue` takes `points[].t` in **ms**, but
> `metrics/MetricChart.vue` — shared with the metrics explorer, whose points are ns — divides by 1e6
> on the way in. Both service views were handing it ms, so their error and latency charts plotted at
> epoch 1970 and drew an empty window. Fixed at the call sites (`b.ts * MS_TO_NS`) rather than by
> changing `MetricChart`'s contract, which the metrics explorer depends on.

## Component groups (`src/components/`)

- **`charts/`** — the shared uPlot layer: `BaseChart`/`LineChart`/`BarChart`, `MiniAreaChart` (a
  compact, interactive inline trend chart — `LineChart`'s `compact` mode: axes/legend hidden, tight
  padding, a `height` prop, still the real uPlot engine underneath with crosshair + hover tooltip),
  `ChartPanel`, `ChartTooltipCard`, plus pure option builders (`chartOptions.js`), the lifecycle
  composable (`useUplot.js`), and theme-aware colors (`useChartTheme.js`).
- **`common/`** — app chrome & toolbar widgets: `AppShell`, `NavRail`, `ContextBar` (the single
  consolidated header row, mounted once by `AppShell`: breadcrumb + tenant picker + environment
  picker on the left, a
  middle `search` slot, and the global time picker + `LiveControl` on the right — searchable views
  forward their `SearchBar` into that slot via `AppShell`'s `toolbar` slot; non-searchable views
  leave it empty and the breadcrumb labels the page), `PhotonMark`, `SearchBar` (query-language
  input + autocomplete), `RelatedMenu` ("Related ▾" cross-signal destination dropdown),
  `TimeRangePicker`, `ColumnPicker`, `LiveControl` (refresh mode), `ThemeToggle`. The rail's gear
  navigates to `/settings/users` (nav key `settings`); the Settings modal it used to open is gone.
- **`settings/`** (the `/settings/users` page) — `UsersTable.vue` (rows with `UserAvatar`, a
  `RoleBadge` or an in-place `SelectMenu` for an admin, an age from `addedLabel`, a key whose
  disabled reason comes from `passwordBlockedReason`, and a Remove whose disabled reason comes from
  `removeBlockedReason`; the view pages its rows through `lib/core/users.ts`'s `paginate` and the
  `ui/pagination` strip), `AddUserDialog.vue` (the modal off the header's `GatedButton`: server rules
  restated by `validateCredentials` before the round trip, three radio cards for the role, and the
  form reset on every open), `PasswordFields.vue` (password + confirmation with one reveal toggle and
  live "Long enough." / "Matches." checks, shared by both dialogs), `ChangePasswordDialog.vue` (two
  shapes in one: your own asks for the current password, an admin's reset of someone else does not),
  `RolesCard.vue` (the role explainer, shown beside the table to a non-admin), `RemoveUserDialog.vue`
  (one plain confirmation), `RoleBadge.vue`, `UserAvatar.vue`.
- **`logs/` · `traces/` · `services/` · `metrics/` · `uptime/` · `data/` · `rum/` · `infra/` ·
  `alerts/`** — the per-signal domain components (see the per-feature docs in
  [`subsystems/`](subsystems/) for the file lists). `infra/` (host/GPU resource monitoring):
  `HostFleetSummary.vue` (the `/infra` fleet in one line — host / critical / warning / waiting
  counts that double as the `?status=` filter, avg CPU, GPU hosts, containers) +
  `HostFleetTable.vue` (the fleet as one table, metrics hosts and managed agents unioned by
  `lib/infra/fleet.ts`, ranked worst first, a "why" column in words beside the CPU/MEM/DSK/GPU
  meters),
  `HostStatTiles.vue` (the
  `/infra/:host` glance stat-tile row — last-point derivation off a shared series bundle, 80%/90%
  warn/error tint), `HostResourcePanels.vue` (presentational per-resource trend cards — each chart in
  a titled `charts/ChartPanel`: CPU total/per-core toggle + load average, memory/network, disk
  meters, a 4-card GPU section — reads the same series bundle rather than owning its own queries),
  `HostProcessesTable.vue` (the `/infra/:host` **Processes** table — one row per supervised process
  (`service.name`) on the host, built on `ui/table` + `EmptyState`, click-to-sort columns defaulting
  to CPU descending with nulls last; see
  [`subsystems/infra.md`](subsystems/infra.md)). `tenants/` (federation central registry):
  `TenantCard.vue` (the `/tenants` fleet-health card — health status, sparkline, mode badge, open-UI link
  for summary-mode; it lived on `/home` until that page became the Attention Board, which judges Sites
  into the same ranked list as every other Subject), `TenantManageDialog.vue` (add/edit dialog with rotate + minted-token panel; delete is a row action in `data/DataTenants.vue`), see
  [`subsystems/federation.md`](subsystems/federation.md). `alerts/` (the webhook alert engine, cross-signal):
  `AlertStatBand`, `AlertRulesTable`/`AlertRuleRow` (its "Browse templates" button + empty-state link
  open the quick-setup picker), `AlertRuleDialog`/`ConditionBuilder` (the plain-English condition
  builder — `AlertRuleDialog` also accepts an optional `:seed` prop, a partial `RuleInput` that
  pre-fills *create* mode, honored only when `:rule` is null), `TemplatePickerDialog`/`TemplateRow`
  (the target-first template quick-setup picker: pick Service/App/Host/Global → Apply directly or
  Customize into a `:seed`-ed `AlertRuleDialog`; templates come from `lib/alertTemplates.ts`),
  `IncidentsTable`, `ChannelsGrid`/`ChannelCard`/`ChannelDialog` — see
  [`subsystems/alerts.md`](subsystems/alerts.md).
- **`docker/`** (the read-only Docker inventory on `/infra/:host/containers`,
  [`subsystems/deploy.md`](subsystems/deploy.md)) — `HostContainersPanel` (owns the inventory
  query + `?view=`/`?q=`/`?c=`), `ContainerKpis`, `ContainerGroup` (one compose project as a panel
  of flat rows), `ImagesTable`/`VolumesTable`/`NetworksTable`, `DockerEventsFeed`, and the
  `ContainerDrawer` with `ContainerLogView` (tail + 1s follow poll, pinned-to-bottom with a "new
  lines" chip), `ContainerStatsView` (six `MiniAreaChart`s), `ContainerDetails`. Lifecycle (phase 2):
  `ContainerActionMenu` (primary + more, disabled-with-reason), `ActionConfirmDialog` (risk-sized
  confirmation, offline → queued), `PruneDialog` (preview first, volumes opt-in + typed),
  `HostActionsStrip` (queued / running / done rows, beside the events feed).
- **`deploy/`** (Docker fleet management, [`subsystems/deploy.md`](subsystems/deploy.md)) —
  `GatedButton.vue` (a Button that knows the role it needs: disabled + a tooltip naming the role
  when the signed-in user falls short — never hidden), `AuditTable.vue` (the `/audit` rows with a
  click-to-expand detail line); the Stacks pieces: `StacksTable`/`StackStatusPill`,
  `NewStackDialog` (write compose or adopt a running project), `ComposeEditor` (file tabs + a
  live Checks panel) with `ComposeWarnings` (one acknowledgement per dangerous construct) and
  `DeployOptionsForm`, `StackActionDialog` (apply / restart / redeploy / down / destroy /
  reconcile, sized confirmations), `StackServicesGrid`, `StackDriftPanel`; the Templates pieces:
  `TemplateCard`/`TemplateIcon`, `VariableField`/`VariablesForm` (typed controls off the manifest),
  `DeployTemplateDialog` (Target → Variables → Preflight → Deploy; also the upgrade path),
  `PreflightList`, `DiffView` (collapsed line diff), `RegistriesDialog` (+ Portainer JSON import),
  `PublishVersionDialog` (structured manifest, immutable versions); the Rollouts pieces:
  `RolloutStatePill`/`TargetStatusPill`, `RolloutProgressBar`, `RolloutStageBoard` (stages with the
  gate drawn between them), `RolloutActionDialog`, `NewRolloutDialog` (Template → Targets →
  Variables → Strategy → Review with per-target preflight and acknowledgements); the tie-ins
  `DeployImpactPanel` ("did it work?" — before/after RED with a verdict), `CanaryVerdictPanel`,
  `TemplateVersionMatrix`, `VersionDiff`; `DeployUnavailable` (what a Deploy surface shows on a
  server without the subsystem). Under `infra/`: `HostScaffold.vue` + `HostSubNav.vue` (the shared
  chrome of every `/infra/:host/*` tab, like `ServiceScaffold`), `HostAgentPanel.vue` (the Agent
  tab), `AddHostDialog.vue` (SSH bootstrap with a per-host stepper, or a minted token shown once),
  `CapabilityPicker.vue`, `AgentTokenPanel.vue` (the shown-once token as three numbered steps:
  token · editable server URL with loopback/http/no-binary warnings · the command as a systemd
  service or a foreground run, token prompted for unless "embed" is ticked), `DockerDiskBar.vue`.
- **`home/`** (the Attention Board) — `WatchRangePicker.vue` (the bounded 5m/15m/1h/6h Segmented that
  replaces the global time picker on `/home`), `WorldRollupCard.vue` (one World's tallies, and the
  page's filter control — the card IS the filter), `SubjectRow.vue` (one Subject that needs
  attention: severity rail, Attention Level pill, name + kind, Site badge / Spread chip, incident
  badge, reason line, age), `VitalSigns.vue`, `UnattributedBanner.vue`, `AttentionLegend.vue`.
- **`ui/`** — the shared primitive library (below).

## The `ui/` primitive library

Each folder has an `index.js` barrel. **Reka-UI/shadcn ports use `<script setup lang="ts">`**; the
**Photon-authored composites use plain JS** (the only plain-JS files under `ui/`).

**shadcn-vue ports (TS):** `alert`, `badge`, `button`, `card`, `checkbox`, `dialog`, `dropdown-menu`,
`form-field`, `input`, `label`, `popover`, `scroll-area`, `select`, `separator`, `sheet`, `skeleton`,
`spinner`, `switch`, `table`, `tabs`, `toast` (+ `Toaster.vue`, `useToast.ts`), `toggle-group`,
`tooltip`. Several carry Photon-specific additions: `button` adds a `variant="brand"` CTA (spent
sparingly) and its `ghost` variant defaults to muted text; `badge` adds the soft tonal pills
(`success`/`warning`/`error`/`brand`) plus `outline` as the neutral entity chip; `card` adds an
additive `interactive` prop (hairline darkens + `shadow-2` on hover) for clickable cards; `input`,
`select` and `SearchBar` share one flat field look (hairline + `focus:ring-2 ring-ring/20`, no inset
shadow); `dialog`/`popover`/`sheet` sit on `--surface-2` + `shadow-2`; `table` rows follow the
flat/instant density rule above.

**Photon-authored primitives:**

| Primitive | Purpose | Lang |
|---|---|---|
| `segmented` | segmented control on Reka ToggleGroup (traces/spans, table/cards); active item gets the raised pill | TS |
| `nav-tabs` | sub-navigation tab bar (route sub-nav) | JS |
| `stat-tile` | KPI card: a header row (`#icon` in a soft rounded tile, label, `#action`), the headline value with its `secondary`/`delta` reading, an optional `sub` caption, a `#spark` slot, and an optional hairline-separated `#footer`. Static — no hover lift. `data-slot="label"`/`"value"` are the stable test hooks | TS |
| `sparkline` | inline-SVG polyline sparkline with last-point dot | JS |
| `status-dot` / `status-pill` | small tone-colored dot / pill label. `StatusPill` also takes `solid` — the full-strength tone with reversed text, spent on the ONE state per screen that must not be mistaken for a milder neighbour (`/home` gives it to **Down**); two solid pills side by side cancel each other out | TS |
| `peek-drawer` | Sheet-based side drawer with prev/next stepping through a list | JS |
| `meter` | horizontal 0–1 proportion bar, tone-colored | TS |
| `select-menu` | dropdown-select on Popover (jsdom-testable, unlike Reka Select). `size="sm"` (default) is the compact toolbar trigger; `size="md"` is a form field sized exactly like `Input` (h-9, text-sm, full width) so it sits level beside inputs in a dialog form. `id`/`class` are forwarded to the trigger — the root is renderless — and a caller's `id` is seeded into Reka's popover context so a `FormField`'s `for=` and the content's `aria-labelledby` both resolve to it | JS |
| `empty-state` | no-data placeholder | TS |
| `kbd` | keyboard-key chip | TS |
| `number-field` | numeric form control; recessed field | TS |
| `date-picker` | single-date popover picker modeled on `TimeRangePicker` (preset "older than" cutoffs + a specific calendar date); emits an ISO `YYYY-MM-DD` string | TS |
| `facet` | the Fields facet catalog (search + promoted/attribute grouping + `useQueries` fan-out); data-source-agnostic via an injected page adapter | JS |
| `textarea` | multi-line sibling of `input`, same flat field look; `mono` for code (compose files, keys, address lists) | TS |
| `pagination` | the strip under a client-paged table: `from–to of total`, a per-page `select-menu`, Previous/Next; every number comes from the parent's `paginate()` result so it never disagrees with the rows | TS |

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
- *App-wide reactive state & navigation:* `auth.ts`, `context.ts` (app-wide time range + federation
  tenant (Site) + deployment environment — the same module-singleton pattern as
  `auth.ts`/`theme.ts`; sole owner of the `range`/`from`/`to`/`tenant`/`env` URL keys.
  `tenantQueryTerm()` / `envQueryTerm()` are the two grammar terms the dimensions contribute;
  `contextQueryTerm()` joins them into the ONE value every composable puts in its cache key, and
  `withContext(q)` is what appends them to the `q`/`filter` string sent to the backend — every
  query composable in `lib/*Queries.ts` routes through it, so a page cannot silently ignore a
  picked Site or environment. Tenant is picked via the ContextBar dropdown or a Home tenant card;
  environment via the ContextBar dropdown fed by `lib/core/environments.ts`'s `useEnvironments()`,
  a `deployment.environment` facet — see "The environment dimension" below),
  `useUrlState.ts` (per-view `svc`/`sev`/`q`; merge-preserves
  every other key, including the context ones — plus `useUrlParam(key, ref)`, its one-key counterpart
  for views owning a single param, e.g. `/home`'s `?world=`), `historyUrl.ts` (`replaceSearch()` — the **only**
  sanctioned way to rewrite the query string without navigating; preserves vue-router's entry state
  and keeps its `current` in step, see [conventions](conventions.md)), `useCorrelate.ts` (`correlate()` builds a same-app
  link that always carries the current time+tenant+env; `relatedFor()` is the per-entity-kind
  related-destination graph behind `RelatedMenu`; **every destination must send a key its target
  view actually reads** — `q` for logs/traces/metrics/uptime, `app` for `/rum` — a param nothing
  consumes silently lands the pivot unfiltered), `useBackTo.ts` (`backTo(router, listPath)` —
  a detail view's back button POPS to the list's own history entry when that's where it came from,
  so the list's filters survive the round trip, and only `push`es as the deep-link fallback).
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
- **`services/`** — `servicesQueries.ts`, `servicesOverview.ts`, `serviceColor.ts` (stable hash →
  palette), and
  `serviceHealth.ts` — **the app's single Attention Level model** (pure, table-tested): the
  `AttentionLevel` type + `LEVEL_RANK` + `STATUS_META` + `INCIDENT_LEVEL`, and `serviceLevel(row,
  { prev, siteStale, incidents })`, which judges one RED row in this order — a stale Site is
  Unknown whatever the numbers say, traffic in `prev` and none now is Down (Silence), nothing at
  either end is Unknown, a firing incident may only *raise* the level, and otherwise error rate or
  Apdex (whichever is worse) picks the band off the exported `ERROR_*`/`APDEX_*` thresholds.
  `/home`'s `attention.ts` imports it rather than re-deriving a second opinion.
  `servicesOverview.ts` is the `/services` index's data layer: `useServicesOverview` (one request,
  `GET /api/services/overview`) plus the pure **fleet merge**. Rows arrive one per
  `(service, Site, environment)` because that is what the backend can honestly produce; `mergeFleet`
  folds them into one row per `(service, environment)` carrying **Spread** — how many Sites this
  service is unhealthy in, out of how many are actually *reporting*. A stale Site is excluded from
  Spread's denominator and from every numeric total, because "unhealthy in 1 of 3" is a lie when one
  of the three has not been heard from, and averaging a blind Site's zeros in halves the fleet's real
  traffic. p50/p90/p99 are the **worst** across Sites, never the mean (a mean of two p99s is not a
  percentile of anything), and Apdex is recomputed from the bands rather than averaged.
  `siteStats()` derives the Site band from those same merged rows, so the band and the table under it
  cannot disagree inside one response — the backend sends only the registry facts (liveness, mode, UI
  url) that no row could carry. `useIncidentsByService()` attributes triggered incidents through
  `attention.ts`'s `resolveIncident`, not a second copy of the rules.
- **`uptime/`** — `uptimeQueries.ts`.
- **`data/`** — `dataQueries.ts`.
- **`infra/`** — `infraQueries.ts` (`useInfraHosts`/`useInfraHost`/`useInfraHostSeries`, plus
  `useHostResourceSeries` — the nine-resource query bundle for one host-detail view) and
  `hostStats.ts` (pure last-point/worst-series/threshold helpers for the glance tiles).
- **`tenants/`** (federation central only) — `tenantsQueries.ts` (`useTenants`/`useTenantsSummary` +
  mutations for create/update/rotate/delete, all with mutation side-effects: invalidate + toast).
- **`home/`** (the Attention Board, cross-signal) — `attention.ts` (the pure, table-tested domain
  model: Subject builders per kind, Silence for hosts/RUM apps, incident resolution, roll-up and
  ordering; the Attention Level axis itself is imported from `services/serviceHealth.ts` and
  re-exported here, `LEVEL_META` being `STATUS_META` under the board's name), `watchRange.ts` (the bounded lookback `/home` judges over — its own module singleton,
  sole owner of the `watch` URL key, NOT the global Window), `useAttentionBoard.ts` (the fan-out that
  wires every signal's existing composable into Subjects + the vital-signs strip).

**Cross-signal:**
- `alertsQueries.ts` (root `lib/`, not a per-signal folder — the alert engine spans metrics/logs/
  traces/RUM) — `useRules`/`useChannels`/`useIncidents`/`usePreview` (all poll ~15s except the
  on-demand preview) + create/update/delete/toggle/test mutations for rules and channels.
- `alertTemplates.ts` (root `lib/`) — the static, read-only 23-template quick-setup catalog (Service/
  App/Host/Global) + `build(target)` target substitution, consumed by `TemplatePickerDialog`; frontend-
  only, no backend counterpart.

**`deploy/` — Docker fleet management** ([`subsystems/deploy.md`](subsystems/deploy.md)):
`roles.ts` (the viewer/operator/admin model + `roleAllows`/`permissionHint`; `lib/core/auth.ts`
exposes the signed-in user's `role` and `can()` over it), `agentStatus.ts` (agent status/capability
vocabulary + the install snippet), `audit.ts` (action phrases, result tones, the *Since* picker), `containers.ts` (container state/health tones, compose-project grouping, port formatting, event labels), `actions.ts` (the one-shot Action vocabulary and its confirmation policy), `stacks.ts` (stack status vocabulary, generation labels, drift summaries, list filters), `compose.ts` (the dangerous-construct lint, service/variable scans, `.env` parsing, path and size rules), `templates.ts` (variable types, defaults, validation, `.env` render, semver, manifest YAML), `diff.ts` (LCS line diff + collapse), `portainer.ts` (Portainer v2/v3 JSON → templates), `rollouts.ts` (rollout/target vocabulary, selector resolution, canary staging), `impact.ts` (the before/after RED judge), `availability.ts` (the demo switch and `DeployUnavailableError`),
`deployQueries.ts` (agents, enrolment, bootstrap-job polling, audit). The mock corpus is
`core/mockDeploy.ts`, kept apart from the telemetry corpus in `core/mock.ts`.

## Conventions

See [`conventions.md`](conventions.md#frontend-vue). The load-bearing ones: bun not npm; no Pinia;
`ui/` primitives in TS and views/domain components in JS; **ns strings** at the query boundary /
**ms Numbers** at the chart boundary / **BigInt ns** for trace geometry; query composables normalize
refs-or-getters with `toValue` into a `computed` queryKey; the `api.ts` mock fallback must keep working.
