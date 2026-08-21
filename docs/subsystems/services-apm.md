# Services (APM)

Per-service RED metrics (Rate / Errors / Duration) + Apdex, and per-service dashboards. **Derived
from trace spans** — there is no separate storage; the computation is in `SpanQueryEngine`.

> Depends on the traces signal: [`traces.md`](traces.md). Shared plumbing:
> [`../architecture.md`](../architecture.md).

## What counts as a request

**A service's Requests are its entry spans, not every span it emits**
([ADR-0003](../adr/0003-a-services-request-rate-is-its-entry-spans.md)). `count`, `error_count`, the
duration percentiles and the Apdex bands are all computed over the **entry set**; the raw span total
survives beside them as `span_count`. So a service that wraps each stage of its own work in an
INTERNAL span is no longer several times "busier" than one doing the same work with a single span,
and a CLIENT span that failed (a database call the code retried successfully) no longer counts as a
service error.

The entry set is a cascade, **first non-empty wins, decided per group** — a worker that only emits
INTERNAL spans never vanishes from the table exactly when it breaks:

| # | Rule | `basis` |
|---|---|---|
| 1 | `kind IN (2 SERVER, 5 CONSUMER)` | `entry` |
| 2 | trace roots — `parent_span_id` null or empty | `trace_root` |
| 3 | every span | `all_spans` (the UI badges the row `span-based`) |

All three candidates are aggregated **side by side in one pass** and the winner is picked in Rust
while decoding (`red.rs`'s `candidate_aggs` + `entry_mask`/`root_mask`, reused by
`red_timeseries.rs`). Three separate queries would put three DataFusion passes on the read path of
every Backend page. `red_metrics` decides per RED group; `red_timeseries` decides **once for the
whole series** (it is already scoped to one service, and a per-bucket decision would let a quiet
bucket holding only INTERNAL spans fall back to rule 3 and spike the chart).

Consequence on upgrade: every rate and error rate on these pages changes, in most cases downward,
and Apdex bands now sum to at most `count` rather than `span_count`. Alert rules written against the
old inflated rate need their thresholds revisited.

## Error taxonomy: `error_count` vs. `http_4xx`/`http_5xx`

`error_count` (`status_code == 2`, OTEL ERROR) is the only error signal a span carries if the SDK
never sets HTTP status attributes — and an SDK that never sets span status hides real HTTP 5xx from
it entirely, while a spike of 401s or 429s (not OTEL errors at all) is invisible on every Backend
page. `RedRow`/`RedBucket` add `http_4xx`/`http_5xx`, read from the span's HTTP status attribute,
**beside** `error_count` — never instead of it. `error_count` keeps meaning exactly `status_code ==
2` so "error rate" continues to mean one thing; the two new counters give the Overview a second,
HTTP-native read over the same entry set.

Attribute lookup: `http.response.status_code` (current OTel semantic convention), falling back to
the legacy `http.status_code` only when the current key is absent — each key looked up by its own
promoted/map location (`red.rs`'s `http_status_expr`, mirroring the `tenant` lookup: a promoted key
is routed OUT of the `attributes` map by `span_record`, so a map lookup on it reads NULL). The stored
value is classified by its leading digit (`'4'`/`'5'`) rather than parsed numerically. A span with
neither key set contributes to neither counter, and both counters are computed over the same winning
entry set as `count` — a failing CLIENT call to a downstream 500 does not count as this service
returning a 500.

## Backend

- **Query** (`SpanQueryEngine`, `photon-query`):
  - `red_metrics` — RED per service and per operation. `RedRow` carries `count` (entry spans),
    `span_count` (all kinds), `basis: RedBasis { Entry, TraceRoot, AllSpans }`, `error_count`, and
    `http_4xx`/`http_5xx` (see above).
  - `red_timeseries` — bucketed RED + Apdex bands, for the detail page. `RedBucket` carries the same
    `span_count` + `basis` + `http_4xx`/`http_5xx`.
  - `red_rollup` — the THIRD RED shape, for the background rollup pass only. Groups by
    `(bucket, service, deployment.environment[, operation])` in one query, which neither of the
    other two can do (`red_metrics` does not bucket by time; `red_timeseries` does not group by
    service). It emits no t-digests: it counts an explicit-bucket latency histogram instead, so a
    wide-window read can re-quantile it honestly. It also reads LOCAL spans only. See
    [`rollup.md`](rollup.md).
  - `dependencies` — DB + external downstream rollups with p50/p95/p99.
- **Rollup:** a background pass publishes these numbers as `photon.service.*` /
  `photon.operation.*` metrics so a wide window does not re-scan raw spans —
  [`rollup.md`](rollup.md), [`ADR-0002`](../adr/0002-red-rollups-are-published-as-metrics.md).
  Operations there inherit their service's `basis`, so a `CLIENT`-only operation reports 0 requests
  rather than falling through to `AllSpans` and counting outbound calls as traffic served.
- **Read-path switch** (`photon-api/src/services_rollup.rs`): `/api/red` and
  `/api/services/:service/timeseries` prefer the rollup and fall back to raw spans. Every row and
  bucket carries `"source": "rollup" | "raw"` so a wrong number is attributable. The rollup is used
  only when all of these hold, and it is deliberately conservative — falling back costs latency,
  answering wrongly costs correctness:
  - the window sits entirely inside what the rollup has published (`RollupStatus::coverage`, backed
    by the pass's SQLite watermark). Reading past the watermark would show traffic falling off a
    cliff at `now - lag`; reading before coverage begins would answer with zeros, which looks
    exactly like an outage.
  - the requested resolution is at least one 60s bucket. Finer than that, the rollup does not hold it.
  - the query only touches keys a rollup series carries (`service.name`, `deployment.environment`,
    `operation`, `tenant`), un-negated. `status_code:2`, a body substring or `duration>1s` did not
    survive aggregation, so those always read raw.

  A rollup read that errors is logged and falls back rather than 500ing: a Backend page that works
  slowly beats one that breaks because a shortcut did.
- **Config:** `[apm].default_apdex_threshold_ms` (default 500), `[rollup]` (see above). Apdex bands: **satisfied** ≤ T,
  **tolerating** T..4T, **frustrated** > 4T, all over the entry set (a null-duration span lands in no
  band). A per-service override can be set from the UI (stored via the `settings` route).

## API

| Route | Purpose |
|---|---|
| `GET /api/services/overview` | One request for the whole index (ADR-0004): `sites` (tenant-registry **liveness facts only** — `name`/`status`/`last_seen_ms`/`mode`/`ui_url`; per-Site totals are derived on the frontend from `rows`, so a band and the table under it cannot disagree inside one response), `rows` (one per `(service, tenant, environment)` in `/api/red`'s exact row shape, plus `spark`/`error_spark`), `window`, `source`. `sites` is `[]` on a Site install. Sparklines cover the busiest 40 rows; the rest return `[]` — an empty sparkline, never a flat one |
| `GET /api/red` | RED table (per service / operation). Each row carries `source` (`"rollup" \| "raw"`) and, for `group=service`, `environment`. Each row carries `span_count` (number) and `basis` (`"entry" \| "trace_root" \| "all_spans"`) beside `count`; `rate` is `count / window_seconds`, i.e. over the entry count; `http_4xx`/`http_5xx` (numbers) sit beside `error_count`/`error_rate`, never instead of them |
| `GET /api/services/:service/timeseries` | per-service RED time series + Apdex bands — each bucket's `ts` is an epoch-**ms** number (the UI plots it raw); percentiles stay ns strings; every bucket carries `span_count` + the series' `basis` + `http_4xx`/`http_5xx` + `source` |
| `GET /api/services/:service/dependencies` | downstream DB/external rollups |
| `GET /api/services/:service/operations` | one row per operation, `/api/red`'s row shape plus `spark`/`error_spark` (busiest 40), `source`-aware. Exists rather than `/api/red?group=operation` for exactly those two things the shared route cannot give it |
| `GET /api/services/:service/operations/:operation/heatmap` | the latency distribution **over time**: `columns` (bucket starts, epoch ms), `bounds` (the ladder's inclusive upper edges as ns strings), sparse `cells` (`[t, band, count]`), and `open_spans`. Raw spans only — see below |
| `GET /api/services/map` | the service topology: `nodes`, `edges`, `scanned`, `capped`. Optional `service` narrows to edges touching it in BOTH directions — as its own param, never folded into `q` |
| `GET /api/services/errors` | fleet-wide **Backend Issues**, ordered by occurrences. Every row names its own service. No `service` key on the envelope — the fleet list is not "some service's" list |
| `GET /api/services/:service/errors` | the same list, scoped to one service. Optional `q` (spans grammar) |
| `GET /api/services/:service/errors/facets` | top values + counts for the six Issue facet fields, over that service's **errored spans only** |
| `GET /api/services/:service/errors/:fingerprint` | one Issue's detail: header stats, occurrence series (`[epoch ms, count]`), tag breakdowns, a sample stack, and the 20 most recent occurrences. **200 with empty sections**, never a 404, when the Issue did not fire in the window |
| `GET /api/deployments` | releases in a window. Optional `service`/`environment`/`tenant` filters and `start`/`end` (epoch **ns** strings like every other window param, converted to the ms the table stores) |
| `POST /api/deployments` | CI enrichment: `service` + `version` (required) plus optional `environment`/`tenant`/`commit_sha`/`author`/`message`/`url`. Upserts on `(service, environment, tenant, version)`; `detected_at_ms` and `source` are **write-once** |
| `GET/PUT/DELETE /api/services/:service/settings` | per-service Apdex threshold override |
| `GET/POST /api/services/:service/slos` | this service's objectives; `POST` creates one (`name`, `kind`, `target`, `window_days`, optional `environment`/`operation`/`threshold_ms`) |
| `PATCH/DELETE /api/services/:service/slos/:id` | edit an objective in place (everything except its identity) or remove it |
| `GET /api/services/:service/slos/:id/budget` | error budget + multi-window burn rate. `partial` is `true` when the rollup does not cover the whole window |

Handlers: `crates/photon-api/src/{red,services,services_overview,service_operations,service_errors,service_map,deployments,slo}.rs`.

### The service map

`photon_query::service_map`. An edge exists when a span in service B has a parent span in service A.
That is the only definition of "A calls B" the trace data actually contains; a peer hostname or a
`db.system` describes something that is not a Service, which is what `service_dependencies` is for
and why it stays.

**`service` is not a filter on the scan.** Scoping the scan to one service deletes exactly the parent
spans the join needs, leaving that service's spans with nobody to attribute them to — the map comes
back empty and reads as "nothing calls this". Narrowing happens after the edges exist. The API
handler enforces it by keeping `service` out of the grammar query, and the engine's doc comment says
so at the one place someone would be tempted to "optimize" it.

**Cost, stated plainly.** The parent's service is unknown until you have read the parent, so the skip
index prunes nothing: this reads every span file overlapping the window on a six-column projection
(`trace_id`, `span_id`, `parent_span_id`, `service.name`, `duration_nanos`, `status_code`).
`MAX_MAP_SPANS` (300,000) caps it; a hit is logged **and** returned as `capped`, and the UI states it
on the page. A truncated map that looks complete is worse than no map. If this proves too slow in
practice the escape hatch is to emit edges from the rollup — deliberately not built speculatively.

It is a self-join executed as a hash join in Rust, not in DataFusion: same operation, one read of the
spans instead of two, and the `(trace_id, span_id)` index borrows straight out of the Arrow buffers so
the join side copies nothing. The join key is the **pair** — `span_id` is unique only within a trace
per OTLP, so keying on `span_id` alone would cross-wire two traces that happened to reuse an id.

Three more decisions worth knowing:

- **Edge latency is the CALLEE's own span duration**, not the caller's CLIENT-span duration. The
  caller waits longer by however much network and its own queueing add; those are two different
  numbers and an edge that mixed them per-span would be neither. The UI says this under the table.
- **A service calling itself is not an edge.** It is real (an internal stage, an async handoff) but
  drawing every service with a self-loop answers nothing about who depends on whom.
- **A node's counts are over ALL its spans**, not just the ones on edges — a node's size should mean
  "how busy is this", not "how connected is this". `roots` (spans with no parent) marks where work
  enters the system, which is what a left-to-right layout wants on the left.

### Backend Issues

An **Issue** is every errored span sharing one `error.fingerprint`: one bug, however many times it
fired. `photon_query::span_issues` is the read side; the fingerprint itself is written at ingest by
`photon_core::span_error` (see [`traces.md`](traces.md)).

**Identity is the pair `(service, fingerprint)`, never the fingerprint alone.** The fingerprint hashes
the exception type, its message and its top frame — nothing about where it ran. That is deliberate: it
is what lets you see that one bug is present in three services. But three services is three Issues,
with three owners, three deploys and three fixes, so every grouping carries `service.name` in the key,
including on the per-service routes where it is degenerate. One grouping rule, not one per caller.

Counts, and what they mean:

- `count` — errored spans. A retry loop inside one request counts several times, which is correct:
  that is how much work the fault caused.
- `traces` — distinct traces. How many **requests** went wrong. This is the backend's answer to RUM's
  distinct-sessions, and it is the number to rank by impact on.
- `operations` — distinct span names the Issue reaches. `> 1` means one fault, several call sites.

**The stack trace is not an attribute.** Ingest leaves `exception.stacktrace` inside the span's
`events` JSON rather than copying kilobytes onto a column every read pays for. The detail view parses
`events` for its ≤20 sample rows only. A service that sets `status = ERROR` without recording an
`exception` event therefore has **no stack**, and the UI says so rather than showing an empty box.

The six facet fields (`ISSUE_FACET_FIELDS`) are backend questions, not frontend ones: `error.type`,
`name` (operation), `kind_text`, `deployment.environment`, `host.name`, `http.status_code`. The facets
route scopes to `error.fingerprint:*` first — a facet over *all* spans would report that 99% of traffic
is `kind_text:SERVER`, which is true and useless.

**There is no backfill.** Spans ingested before fingerprinting shipped carry none and never will, and
there is no query-time fallback: two grouping rules for one concept is worse than one honest gap in
the history. The UI states this on every Issues list (`IssueBackfillNote.vue`) so a user who widens
the window and watches the list thin out is not left concluding Photon lost their errors.

### Deployments

Nobody sends Photon a deployment. But every service that follows OTel semconv already stamps
`service.version` on its **resource**, and the first time a version appears that has never appeared
before, something was deployed. That inference is the whole feature — there is nothing to install
and nothing to configure, and a service that does not set `service.version` simply has no releases.

- **Detection** (`photon-ingest/src/deployments.rs`): `resource_versions(&req)` reads the RESOURCE
  attributes of a trace batch — `service.name`, `service.version`, `deployment.environment`, and the
  `tenant` stamp federation adds. A 5,000-span batch is therefore four string lookups per resource
  group, not 5,000. Both front doors (`trace_http.rs`, `grpc_trace.rs`) call it after tenant stamping
  and hand each sighting to the `DeploymentWatcher` seam. The trait is **sync on purpose**: the
  implementation answers an already-seen triple from an in-memory `HashSet` with no I/O, so a service
  that never redeploys pays one hash lookup per batch and nothing else.
- **Storage** (`photon-api/src/deployments.rs`): a `deployments` table in the shared control-plane
  SQLite DB, `UNIQUE (service, environment, tenant, version)`. `environment` and `tenant` are
  `NOT NULL DEFAULT ''` rather than nullable, because SQLite treats NULL as distinct from NULL and a
  nullable column would let the same unversioned deployment insert forever. `deployment_id` is an
  FNV-1a hash of the four parts, so an id is stable across restarts.
- **`detected_at_ms` and `source` are write-once.** A CI POST that arrives after the traffic did
  fills in the commit, author and message but must not move the timestamp: the moment a deploy marker
  sits on is the moment the graph actually changed, and the pipeline's clock is a different fact.
  `source` therefore records who saw it FIRST, which is why an enriched row still reads
  `auto-detected` in the UI.
- **Wiring** lives in `photon-server/src/main.rs` (`DeploymentDetector`), because `photon-ingest`
  cannot depend on `photon-api` — the same shape as every other cross-crate seam here. An insert
  failure is logged and dropped: a deploy row is worth having, never worth failing an ingest for.

### SLOs, error budgets and burn rate

`photon-api/src/slo.rs`. An SLO turns a reading into a decision: "error rate 0.4%" needs context to
act on, "12% of the month's budget left and burning it 16× faster than you can afford" does not.

- **It reads the rollup, and only the rollup.** An SLO window is days or weeks wide, which is exactly
  the shape [`rollup.md`](rollup.md) exists for. Answering it from raw spans would put a multi-minute
  scan behind a page load, so when the rollup does not cover the whole window the budget comes back
  **`partial: true`** with the window it actually measured — never a part-window number presented as
  a full one, and never a silent fallback to a scan.
- **Two kinds.** *Availability*: good = requests that did not error, straight off
  `photon.{service,operation}.requests` and `.errors`. *Latency*: good = requests at or below a
  threshold, read off the rollup's explicit-bucket `duration` histogram.
- **A latency threshold snaps DOWN to a ladder edge**, and the response says which
  (`measured_threshold_ms`). A histogram knows nothing finer than its own `LATENCY_BOUNDS_NS` edges,
  so a 300ms objective is measured at 200ms. Snapping down keeps it conservative — an objective
  reported as met was met, never rounded into being met — and reporting the edge stops anyone
  believing a line was held that was never tested. The read is
  `MetricsQueryEngine::latency_sli` (`photon-query/src/metric_dist.rs`), which shares its
  accumulation with `histogram_series` so the two readers of one metric cannot drift apart on the
  cumulative-reset rule. It is deliberately **not** a new `Agg` variant: `Agg` is a closed vocabulary
  shared with the metrics explorer and the alert engine, and "count at or below X" carries a
  parameter none of the others do.
- **Scope.** An SLO is `(service, environment, operation, name)`. A named `operation` reads the
  `photon.operation.*` family, so the other operations of the same service contribute to neither the
  numerator nor the denominator — a checkout call can be held to a tighter line than an admin page
  in the same service.
- **Burn rate is multi-window on purpose.** A single window either cries wolf (short) or notices an
  outage an hour late (long). Both pairs need BOTH windows over the threshold to fire: fast = 1h
  confirmed by 5m at **14.4×**, slow = 6h confirmed by 30m at **6×** (the SRE-workbook numbers —
  14.4× spends a 30-day budget in two days, 6× in five). The confirming window is what stops a burst
  that ended forty minutes ago from paging someone now.
- **No new alert machinery.** Because the rollup is metrics, a burn-rate alert is an ordinary
  metrics rule in `photon-alerts` over `photon.service.errors` / `.requests`. This module adds no
  rule kind and no `ConditionSource`.
- **Storage** is a `service_slos` table in the shared control-plane SQLite DB, unique on
  `(service, environment, operation, name)`; `environment`/`operation` are `NOT NULL DEFAULT ''` for
  the same reason the deployments table does it. Validation rejects a target of 0 or 1 (a target of
  1 has a zero-sized budget, so every burn rate is infinite) and a latency SLO with no threshold.

### The latency heatmap

`photon_query::span_heatmap` — the one shape three percentile lines genuinely cannot draw. When a
distribution splits in two (half the requests served from cache in 2ms, half falling through to a
cold path at 800ms) p50/p90/p99 all sit in the empty gap between the modes, describing a latency no
request actually experienced.

Two decisions are load-bearing:

- **The latency ladder is fixed** (`LATENCY_BOUNDS_NS`, borrowed verbatim from the rollup), not
  derived from the matched set the way `span_latency`'s geometric bins are. Data-derived bins would
  re-scale the y-axis the moment one slow outlier entered the window, and a cell would mean something
  different at 10:00 than at 10:05. Reusing the rollup's ladder also means a wide-window heatmap can
  later be answered from `photon.service.duration` without changing what a row means.
- **Spans that never closed are counted apart from every band** (`open_spans`). "Under 1ms" and
  "never finished" are opposite facts, and folding the second into the first would make an operation
  whose spans stop closing look like it got faster.

Cells are **sparse** — an empty cell is absent, not zero. The UI scales colour **per column**, not
globally: scaled globally, one busy minute leaves every other column near-black and hides exactly the
shape shifts the chart exists to show. The Apdex **T** and **4T** lines are drawn on the same axes,
because the Apdex score and the distribution otherwise describe the same requests in two vocabularies
that never meet.

## UI

`/services` is a **fleet board** and the detail page is a set of **sub-routes** — each tab owns its
own queries, so Overview stays fast when Operations is expensive.

| Route | View | What it answers |
|---|---|---|
| `/services` | `ServicesListView.vue` | Which Sites are reporting, what is wrong right now, and everything else in one dense row per `(service, environment)`. See [`../frontend.md`](../frontend.md#the-services-index-services) |
| `/services/:service` | `ServiceDetailView.vue` | Is this service healthy, what changed, where is the problem |
| `/services/:service/operations` | `ServiceOperationsView.vue` | Where inside it the traffic, errors and time go |
| `/services/:service/operations/:operation` | `ServiceOperationView.vue` | One operation: latency heatmap + response codes + slowest traces |
| `/services/:service/errors` | `ServiceIssuesView.vue` | What is broken in this service, grouped into Issues |
| `/services/:service/errors/:fingerprint` | `ServiceIssueDetailView.vue` | One Issue: when it fires, where, and a sample stack |
| `/services/errors` | `BackendIssuesView.vue` | What is broken **anywhere** — the same list with the service term removed |
| `/services/map` | `ServiceMapView.vue` | What talks to what — the fleet graph, plus the edge table |
| `/services/:service/dependencies` | `ServiceDependenciesView.vue` | What it waits on, and who waits on it — the one-hop focused map plus the two tables |
| `/services/:service/deployments` | `ServiceDeploymentsView.vue` | What shipped, and what it did — releases with a before/after RED diff |

Shared chrome lives in `ServiceScaffold.vue` (AppShell + back button + `ServiceSubNav` + Related menu
+ Apdex control), so a new sub-route is one view file rather than one view file plus forty lines of
copied header.

**Overview** carries a health banner, a KPI row, four charts — request volume, **errors by class**
(5xx is ours, 4xx is the caller's, span status is neither), latency percentiles, Apdex bands — then
the busiest entry points beside a **Logs panel**. The logs panel closes a real hole: a service can be
on fire in its logs while its spans look normal (a failing background job, a config parse error, a
connection-pool warning every 200ms produce no errored SERVER span), so a health page with four
latency charts and no log volume can be green during an outage. It is a summary — a severity-stacked
volume strip plus the most frequent error messages — and `correlate()` hands off to `/logs` for the
lines themselves.

**Operations** has two modes, and the distinction is not cosmetic. *Entry points* (the default) lists
what the service SERVED; *All operations* additionally lists the work it DID — the outbound database
call, the internal stage. A footer states how many rows the default is hiding and why, because a
filter that silently drops rows is a filter you cannot trust. The rollup's `__other__` remainder shows
in both modes (it sums over both kinds; dropping it would make the visible rows stop adding up to the
service) and is rendered as a sentence, never as something clickable.

**Issues** is deliberately the same page as `/rum/:app/errors`, one runtime over: search bar, fixed
six-field facet panel, list ordered by occurrences. A Frontend Issue and a Backend Issue are the same
idea about two runtimes, and an engineer who has read one page should not have to relearn the other.
What differs is what a sample row is — there a browser session, here a span you can open in a
waterfall. The fleet-wide list at `/services/errors` sits beside `/services` as a sibling tab rather
than a mode toggle, because it groups errored spans, not services: it is a different list, not a
filter of the same one. It has no facet panel — facets are computed per service by the API, and a
fleet-wide facet would need its own endpoint to be honest about it.

**Objectives** sit on the service Overview directly under the KPI row and above the charts, because
a budget is the only thing on that page that is already a decision and everything below it is the
evidence. One `SloCard` per objective: budget left as the headline (the bar is clamped to `[0, 100]`,
the number beside it is not — "−240% of budget" is a real and useful thing to be told), both burn
windows labelled by their long window with the short one in the title, and time to exhaustion in a
unit somebody can act on. A card goes critical on a firing fast burn even with budget left, and warns
below a quarter of budget even with nothing burning: the level and the rate are separate signals. A
partial window and a snapped latency threshold are both stated on the card, never hidden. A service
with no objectives renders no section at all rather than a permanent call to action. **Objectives
never appear as a column on the services index** — an error budget is measured over its own window,
and putting it beside per-window RED numbers would sit two different time spans in one row.

**Deployments** answers "did we do this to ourselves". Each release carries a before/after diff of
the four numbers that matter — error rate, p99, Apdex, throughput — measured over a window either side
of it (`±5/15/30/60m`, a control rather than a constant: a short window catches a crash on rollout, a
long one catches a leak that takes an hour to show). Two rules keep it honest. A comparison needs
**both** sides: when the release is younger than one window, or the window before it falls outside the
time range, the row says so instead of diffing half a window against a full one and inventing a
regression. And direction is **per metric**: error rate up is bad, Apdex up is good, throughput up is
neither — a single "bigger is greener" rule would paint a traffic spike as an improvement. Moves under
5% are left untinted as noise. The diff is computed frontend-side from the same
`/api/services/:service/timeseries` buckets the charts draw, so the row and the chart above it cannot
disagree. The same releases are drawn as dashed vertical markers on the volume, error, latency and
Apdex charts across the service pages (`lib/services/deployMarkers.ts`, capped at the 12 newest so a
busy service does not turn its charts into a picket fence).

**Topology in the UI** is two readings of one endpoint. The **focused map**
(`FocusedDependencyMap.vue`, on the Dependencies tab) is always one hop — callers, this service,
callees — drawn as hand-authored SVG in three fixed columns, so it stays readable at any fleet size;
it merges service edges with database and external calls into one picture, because to someone
debugging "what is this waiting on" has one answer, and only the Service boxes are clickable, because
only they have a page. It states how many rows it did not draw; the tables below carry every one. The
**fleet graph** (`FleetGraph.vue`, on `/services/map`) is a node-link graph laid out with
**`@dagrejs/dagre`** — chosen over `elkjs` because a service map is a layered DAG with occasional
cycles, which is the one thing dagre is for, and elkjs ships roughly 1.5 MB against dagre's ~48 kB
(16.9 kB gzipped, its own chunk via a dynamic `import()`, so nobody pays for it to look at their
logs). Node dot = Attention Level (read from `serviceHealth.ts`, not re-derived from span counts),
edge weight = throughput, edge hue = error rate — three facts on three channels, so a busy healthy
edge and a quiet broken one never look alike.

**Components** (`frontend/src/components/services/`): `ServicesTable`, `OperationsTable`,
`ServiceRowExpansion`, `SiteBand`/`SiteCard`, `AttentionStrip`/`AttentionCard`, `RowSparkline`,
`ApdexBandBar`, `ServiceLogPanel`, `IssueList`, `IssueFilters`, `IssueBackfillNote`,
`FocusedDependencyMap`, `FleetGraph`, `DeploymentTimeline`, `SloPanel`/`SloBudgetCard`/`SloCard`,
`ServiceScaffold`, `ServiceSubNav`, `ServiceVolumeChart`,
`HealthBanner`, `HealthPill`, `ApdexBadge`, `ApdexBandChart`, `ApdexThresholdControl`,
`DependencyTable`, plus `components/charts/LatencyHeatmap.vue`. **Queries**
(`frontend/src/lib/services/`): `servicesOverview.ts` (the index's one request + the fleet merge) and
`servicesQueries.ts` (`useServicesList`, `useServiceTimeseries`, `useServiceOperations`,
`useServiceDependencies`, `useOperationHeatmap`, `useBackendIssues` (one composable for both the
per-service and the fleet list — pass `null` for the fleet, so the two can never disagree about
ordering or limits), `useBackendIssueFacets`, `useBackendIssueDetail`, `useServiceMap` (again one composable for both
readings — `null` for the fleet graph, a name for the focused map), `useDeployments`,
`useSlos`/`useSloBudget` (the budget is deliberately NOT keyed on the app's time window — a 30-day
budget is 30 days whatever the picker says — and polls every 60s instead, because a burn rate is a
now-fact) and the SLO mutations, and settings mutations that
throw the real Ky `HTTPError` and invalidate `['service', service]` + the list by prefix).

**Health classifier**: `frontend/src/lib/services/serviceHealth.ts` — the app's **single Attention
Level model**, not just this page's. `serviceLevel(row, { prev, siteStale, incidents })` returns one
of `down` / `critical` / `warning` / `ok` / `unknown` plus a reason line, in this order: a stale Site
is Unknown whatever the numbers say; traffic in the previous window and none now is **Down**
(Silence); nothing at either end is Unknown; a firing incident may only *raise* the level; otherwise
error rate or Apdex, whichever is worse, picks the band off the exported `ERROR_CRITICAL` /
`ERROR_DEGRADED` / `APDEX_CRITICAL` / `APDEX_DEGRADED` thresholds. `/home`'s Attention Board
(`lib/home/attention.ts`) imports this model instead of deriving a second one — see
[`../adr/0004-the-services-index-is-a-fleet-board.md`](../adr/0004-the-services-index-is-a-fleet-board.md).
The old `idle` status is **retired**: it read as calm neutral grey for a service that had fallen from
5,000 req/s to nothing, which is the one moment it mattered most.

> History: the RED view used to live under `/traces/metrics`; that path now **redirects** to
> `/services` (kept for old links/bookmarks).
