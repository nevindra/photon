# Services (APM)

Per-service RED metrics (Rate / Errors / Duration) + Apdex, and per-service dashboards. **Derived
from trace spans** — there is no separate storage; the computation is in `SpanQueryEngine`.

> Depends on the traces signal: [`traces.md`](traces.md). Shared plumbing:
> [`../architecture.md`](../architecture.md).

## Backend

- **Query** (`SpanQueryEngine`, `photon-query`):
  - `red_metrics` — RED per service and per operation.
  - `red_timeseries` — bucketed RED + Apdex bands, for the detail page.
  - `dependencies` — DB + external downstream rollups with p50/p95/p99.
- **Config:** `[apm].default_apdex_threshold_ms` (default 500). Apdex bands: **satisfied** ≤ T,
  **tolerating** T..4T, **frustrated** > 4T. A per-service override can be set from the UI (stored via
  the `settings` route).

## API

| Route | Purpose |
|---|---|
| `GET /api/red` | RED table (per service / operation) |
| `GET /api/services/:service/timeseries` | per-service RED time series + Apdex bands |
| `GET /api/services/:service/dependencies` | downstream DB/external rollups |
| `GET/PUT/DELETE /api/services/:service/settings` | per-service Apdex threshold override |

Handlers: `crates/photon-api/src/{red,services}.rs`.

## UI

- `/services` → `ServicesListView.vue`: a health-first RED table + Apdex, fleet health counts, an
  attention strip, and volume/latency charts.
- `/services/:service` → `ServiceDetailView.vue`: a KPI row, four time-series charts, a key-operations
  RED table, DB/external dependency tables, and an Apdex threshold control.

**Components** (`frontend/src/components/services/`): `ServicesTable`, `ServiceHealthCounts`,
`ServiceVolumeChart`, `HealthBanner`, `HealthPill`, `ApdexBadge`, `ApdexBandChart`,
`ApdexThresholdControl`, `AttentionStrip`, `AttentionCard`, `DependencyTable` — plus the reused
`components/metrics/RedTable.vue`. **Queries** (`frontend/src/lib/servicesQueries.js`):
`useServicesList` (polls 15s, adds `apdex`), `useServiceTimeseries`, `useServiceDependencies`, and
settings mutations that throw the real Ky `HTTPError` and invalidate `['service', service]` + the list
by prefix. **Health classifier**: `frontend/src/lib/serviceHealth.js` (pure RED-row → status +
exported thresholds).

> History: the RED view used to live under `/traces/metrics`; that path now **redirects** to
> `/services` (kept for old links/bookmarks).
