# Traces (spans)

A distributed-trace explorer + single-trace waterfall, with log↔trace↔span correlation. Its own WAL
(`spans_wal`), Arrow schema, and compactor — it does **not** share the logs machinery.

> Shared plumbing and invariants: [`../architecture.md`](../architecture.md). APM/RED metrics derived
> from spans live in [`services-apm.md`](services-apm.md).

## Backend

- **Ingest:** `otlp_traces_to_spans` (`photon-ingest`) maps OTLP → `Span` records → `spans_wal`.
- **Schema** (`photon-core/src/span_schema.rs`): 15 fixed columns. Sorted `(service.name, start_time)`.
- **Compaction:** `SpanCompactor` → zstd Parquet under `data-spans/` + a spans skip index that blooms
  over `name` tokens **and whole `trace_id` values** (so a lookup by trace ID prunes hard).
- **Query** (`SpanQueryEngine`, `photon-query`):
  - `get_trace` — all spans of one trace, for the waterfall.
  - `search_traces` — rolled-up `TraceSummary` list.
  - `search_spans` / `search_spans_with_count` — raw span rows (cursor-paginated).
  - `count_matching_spans`, span `facet` / `fields` / `histogram`.
  - `latency` — t-digest percentiles + log-scale (geometric) histogram.
  - `SpanSort` controls ordering; `SpanSort::Recent` uses the two-pass late-materialization trick.

## API

| Route | Purpose |
|---|---|
| `GET /api/traces/:trace_id` | all spans of one trace (waterfall) |
| `POST /api/traces/search` | rolled-up trace summaries |
| `POST /api/spans/search` | raw span rows |
| `GET /api/traces/fields\|facet\|histogram\|latency` | span aggregations |
| `GET /api/stream/spans` | SSE live tail |

Handlers: `crates/photon-api/src/{traces,traces_search,traces_agg}.rs`.

## UI

- `/traces` → `TracesExplorer.vue`: a traces-vs-spans `Segmented` toggle, span-volume + latency
  histograms, a virtualized table, a peek drawer, and live tail via cursor-paginated
  `useInfiniteQuery`.
- `/traces/:traceId` → `TraceDetailView.vue`: the waterfall + a span detail panel. The route param is
  the source of truth.

**Components** (`frontend/src/components/traces/`): `TraceTable`, `SpanTable`, `TraceWaterfall`,
`TraceMinimap`, `SpanDetailPanel`, `TracePeekDrawer`, `TracesFilters`, `LatencyHistogram`,
`SpanVolumeHistogram`. **Queries**: `frontend/src/lib/tracesQueries.js`. **Waterfall geometry**:
`frontend/src/lib/traceTree.js` — pure BigInt-nanosecond trace assembly + geometry, defensive against
orphans / cycles / clock skew.

**Correlation:** log→trace, span/trace→logs all flow through `router.push` (see the router redirect
`/traces/metrics → /services`, kept for old links).
