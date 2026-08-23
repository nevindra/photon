# Data — usage, storage & retention

Operational self-observability: see and bound what Photon is keeping. Covers data-usage/storage
accounting and retention management (including manual purge).

> Shared plumbing and invariants: [`../architecture.md`](../architecture.md).

## Backend

- **Engine:** `storage_stats` (`QueryEngine`) + the usage sampler (spawned in `photon-server`).
  Retention purge is routed to the three per-signal compactors over an mpsc channel; each runs
  `purge_before(cutoff)` → `PurgeReport` (`photon-core/src/retention.rs`).
- **Config:** `[retention].days` (default 30; must be `> 0` and `<= MAX_RETENTION_DAYS` — 36,500,
  i.e. 100 years — enforced in both `Config::validate` and `PUT /api/retention`, since the retention
  loops' cutoff arithmetic (`now - days * 86_400_000_000_000` ns) overflows `i64` well past that).
- **Trait seam:** `DataAdmin` (defined in `photon-api`, implemented in `photon-server`).

## API

Handlers: `crates/photon-api/src/{data,usage,usage_services}.rs`.

| Route | Purpose |
|---|---|
| `GET /api/storage` | storage stats |
| `GET /api/usage/series` | usage over time |
| `GET /api/usage/services` | per-service footprint over a window: `rows` (biggest first), the per-signal accounting behind the estimate, and the metrics note. `start`/`end` are epoch-ns strings; `limit` defaults to 50 |
| `GET/PUT /api/retention` | read / set retention |
| `POST /api/data/purge` | manual purge before a cutoff |

## UI

`/data` → `DataView.vue`, with five URL-synced (`?tab=`) tabs — **Overview**, **Storage**,
**Services**, **Retention**, **Delete** — rebuilt on the shared design system (`StatTile`, `ChartPanel`, `Meter`)
and the standard `px-5` spacing rhythm, matching Services/RUM. Each signal (`logs`/`traces`/
`metrics`/`uptime`) gets one visual identity — a colour + a Lucide icon — from
`frontend/src/lib/signalMeta.ts` (`signalColor()`/`signalIcon()`; the colour reuses
`seriesColor(key).stroke` so a signal's hue is identical across the composition bar, the per-signal
cards, and the charts).

- **Overview** (`DataOverview.vue`): a `StatTile` KPI row (On disk / Durable / Rows / Ingest) + a
  storage-composition bar (each signal's share of on-disk bytes) + the two usage-over-time charts
  (footprint, ingestion rate). No page-local window selector — the usage charts follow the ONE
  global time control (the `ContextBar`) via `usageWindow`, a computed in `dataQueries.js` derived
  from `context.windowMs` and mapped onto the API's `1h|24h|7d|30d` buckets.
- **Storage** (`DataStorage.vue`): a durable-replication status band, then one card per signal —
  signal icon+hue, size, rows/files, a 24h on-disk footprint trend chart (`charts/MiniAreaChart.vue`,
  same `usageWindow`/series as Overview), a signal-hued "share of disk" bar, and a "durable
  replication" `Meter`. Uptime keeps its own shape (monitors/heartbeats/incidents, no bytes/trend).
- **Services** (`DataServices.vue`): who is filling the disk, one row per service, **sorted by
  estimated footprint and never by row count** — a chatty service writing 40,000 tiny log lines can
  cost less disk than a quieter one writing 9,000 spans, and ordering by rows would put the cheap
  service at the top of a cost page. It lives here rather than as a services-index column because
  retention and cost are `/data`'s job and `/data` owns its own window; a cumulative footprint beside
  per-window RED numbers would invite exactly the wrong comparison.
- **Retention** (`DataRetention.vue`): card-framed per-signal rows, each with a "window used" gauge
  (oldest-surviving-data age vs. the configured retention window; warns when data is already older
  than the window).
- **Delete** (`DataDelete.vue`): a danger-zone card per signal — current counts, a `ui/date-picker`
  `DatePicker` for "delete older than", and a type-`DELETE`-to-confirm "delete all".

### Per-service footprint is an ESTIMATE, and the page says so twice

Nothing stores bytes per service, and nothing can cheaply: Parquet is columnar and sorted by
`(service.name, timestamp)`, so a service's rows are physically contiguous and compress *together*.
There is no per-service byte figure to read, and producing a measured one would mean re-encoding.

What is exact is the pair the estimate is built from — a signal's total rows and total on-disk bytes
(straight off the manifest, no data scan) and a service's row count in the window (the same facet the
search UI uses). So `bytes ≈ service_rows × (store_bytes / store_rows)`. The response carries
`estimated: true` and the mean it divided by, and the UI states both the word "estimate" and the
actual bytes-per-row it used.

**Metrics are deliberately not attributed**, and that is a fact about the data model rather than a
gap. Logs and spans are keyed by `service.name`, so a facet over them is a real answer. A metric
series is keyed by `metric_name` first — it leads both the sort key and the pruning bloom — and most
metric series belong to no service at all: host and GPU metrics belong to a host, RUM vitals to a
browser app, and the service rollup's own output is *about* a service rather than emitted by one.
Faceting every metric in the store would need a metric-name-free scan, defeating the pruning the
metrics engine is built on. The route reports metrics as `attributed: false` and carries the reason
as prose the UI renders verbatim; the page also shows a **"not attributed to a service"** total, so
nobody concludes the difference is unaccounted-for waste.

**Components** (`frontend/src/components/data/`): `DataOverview`, `DataStorage`, `DataRetention`,
`DataDelete`, `DataServices`, `UsageChart`. **Queries** (`frontend/src/lib/dataQueries.js`): `useStorage`,
`useRetention`, `usageWindow` + usage-series queries, `useServiceUsage` (which takes the RAW
nanosecond window rather than the coarse `usageWindow` bucket — it facets the real stores, so it can
answer any window, and rounding "the last 90 minutes" up to 24h would attribute six times the data
to everybody), and retention/purge mutations (these return
`{ ok, error }` and don't throw — match that contract when extending).
