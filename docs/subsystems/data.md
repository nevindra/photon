# Data — usage, storage & retention

Operational self-observability: see and bound what Photon is keeping. Covers data-usage/storage
accounting and retention management (including manual purge).

> Shared plumbing and invariants: [`../architecture.md`](../architecture.md).

## Backend

- **Engine:** `storage_stats` (`QueryEngine`) + the usage sampler (spawned in `photon-server`).
  Retention purge is routed to the three per-signal compactors over an mpsc channel; each runs
  `purge_before(cutoff)` → `PurgeReport` (`photon-core/src/retention.rs`).
- **Config:** `[retention].days` (default 30, must be > 0).
- **Trait seam:** `DataAdmin` (defined in `photon-api`, implemented in `photon-server`).

## API

Handlers: `crates/photon-api/src/{data,usage}.rs`.

| Route | Purpose |
|---|---|
| `GET /api/storage` | storage stats |
| `GET /api/usage/series` | usage over time |
| `GET/PUT /api/retention` | read / set retention |
| `POST /api/data/purge` | manual purge before a cutoff |

## UI

`/data` → `DataView.vue`, with four URL-synced (`?tab=`) tabs — **Overview**, **Storage**,
**Retention**, **Delete** — rebuilt on the shared design system (`StatTile`, `ChartPanel`, `Meter`)
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
- **Retention** (`DataRetention.vue`): card-framed per-signal rows, each with a "window used" gauge
  (oldest-surviving-data age vs. the configured retention window; warns when data is already older
  than the window).
- **Delete** (`DataDelete.vue`): a danger-zone card per signal — current counts, a `ui/date-picker`
  `DatePicker` for "delete older than", and a type-`DELETE`-to-confirm "delete all".

**Components** (`frontend/src/components/data/`): `DataOverview`, `DataStorage`, `DataRetention`,
`DataDelete`, `UsageChart`. **Queries** (`frontend/src/lib/dataQueries.js`): `useStorage`,
`useRetention`, `usageWindow` + usage-series queries, and retention/purge mutations (these return
`{ ok, error }` and don't throw — match that contract when extending).
