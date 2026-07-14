# Metrics

An OTLP metrics explorer with a query builder and a metric catalog. Its own WAL (`metrics_wal`), Arrow
schema, and compactor — it does **not** share the logs machinery.

> Shared plumbing and invariants: [`../architecture.md`](../architecture.md). RUM Web Vitals also ride
> the metrics store and are queried through `MetricsQueryEngine` — see [`rum.md`](rum.md).

## Backend

- **Ingest:** `otlp_metrics_to_points` (`photon-ingest`) maps OTLP → `MetricPoint`s → `metrics_wal`.
  Supported types: gauge, sum, histogram, exponential histogram, summary.
- **Prometheus remote-write:** `POST /api/v1/write` (`promrw_http.rs`, on the ingest HTTP
  listener) accepts Prometheus remote-write 1.0 (snappy + protobuf). `promrw_to_points`
  (`promrw_mapping.rs`) maps each series → `MetricPoint` → the **same** `metrics_wal`: `__name__`
  → metric name, `job` → `service.name`, other labels → attributes. Type is classified by name
  suffix — `_total`/`_bucket`/`_count`/`_sum` → cumulative monotonic `SUM` (so reset-aware
  `rate()`/`increase()` applies), else `GAUGE`. The receiver is stateless; histogram bucket
  series are stored flat (`le` as an attribute) — percentile queries over them are a separate
  concern (see the remote-write Plan 2 spec).
- **Schema** (`photon-core/src/metric_schema.rs`): typed `value` column + attributes; distribution
  payloads and exemplars are stored as JSON string columns. Sorted `(metric_name, service.name,
  timestamp)`.
- **Compaction:** `MetricsCompactor` → zstd Parquet under `data-metrics/` + a metrics skip index that
  blooms over whole `metric_name` values.
- **Query** (`MetricsQueryEngine`, `photon-query`):
  - `query_series` (`metric_query.rs`) — gauge avg/min/max/sum/count/last; delta & cumulative
    **reset-aware** rate/increase.
  - distribution quantiles (`metric_dist.rs`) — explicit + exponential Histogram + Summary.
  - Prometheus classic histograms (`metric_classic_hist.rs`) — `<base>_bucket{le}`/`_sum`/`_count`
    series folded at query time into a single `HISTOGRAM` metric, supporting p50/p90/p99/count/sum/avg
    by reassembling `le` buckets (reset-aware increase → difference → `interpolate_quantile`); the
    catalog/metadata surface reports `<base>` as histogram-typed.
  - catalog / metadata / labels discovery (`metric_catalog.rs`) for autocomplete.
  - `rum_vitals` / `rum_breakdown` (`rum_vitals.rs`) — Web-Vitals p75 + rating distribution (see
    [`rum.md`](rum.md)).

## API

| Route | Purpose |
|---|---|
| `POST /api/metrics/query` | run a series query |
| `GET /api/metrics/catalog` | list available metrics |
| `GET /api/metrics/metadata/:name` | one metric's metadata |
| `GET /api/metrics/labels` | label discovery |
| `POST /api/v1/write` | Prometheus remote-write 1.0 ingest (ingest HTTP listener) |

Handler: `crates/photon-api/src/metrics.rs` (the `/api/metrics/*` query routes). `POST /api/v1/write`
is different — it's served by `crates/photon-ingest/src/promrw_http.rs` on the **ingest** HTTP
listener (a separate crate and port from the REST API), not by photon-api.

## UI

`/metrics` (and `/metrics/catalog` — the same reused component, mode derived from the path) →
`MetricsExplorer.vue`: a query-builder row → a `[chart + legend | metadata]` grid, plus a browse-all
Catalog tab. The view owns **all** server state; the `components/metrics/` children stay pure.

**Components** (`frontend/src/components/metrics/`): `MetricChart`, `MetricLegendTable`,
`MetricMetaPanel`, `MetricCatalog`, `MetricPicker`, `MetricQueryRow`, `MetricTiles`, `MetricPresets`,
`MetricVizSwitcher`, `MetricStat`, `MetricQuickStarts`, `RedTable` (the last reused by Services/APM).
`MetricPicker` now groups by namespace with favorites/recent and type badges; `MetricChart` routes viz
types (line/area/stacked/bar/stacked-bar/stat/table, reusing `MetricLegendTable` for the table viz);
the explorer supports brush-to-zoom, click-a-point-to-correlate-to-traces, and a y-axis log toggle.
**Client state:** favorites/recent in `localStorage` (`photon.metrics.favorites`/`.recent`); selected
viz in the `viz` URL param. **Queries** (`frontend/src/lib/metricsQueries.js`): `useMetricCatalog`,
`useMetricMetadata`, `useMetricSeries`. **Autocomplete fields**: `frontend/src/lib/metricFields.js`.
**Series colors**: `frontend/src/lib/seriesColor.js` (stable hash → palette, consistent across legend
and tooltip).
