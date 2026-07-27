# Traces (spans)

A distributed-trace explorer + single-trace waterfall, with log↔trace↔span correlation. Its own WAL
(`spans_wal`), Arrow schema, and compactor — it does **not** share the logs machinery.

> Shared plumbing and invariants: [`../architecture.md`](../architecture.md). APM/RED metrics derived
> from spans live in [`services-apm.md`](services-apm.md).

## Backend

- **Ingest:** `otlp_traces_to_spans` (`photon-ingest`) maps OTLP → `Span` records → `spans_wal`.
- **Schema** (`photon-core/src/span_schema.rs`): 15 fixed columns. Sorted `(service.name, start_time)`.
- **Compaction:** `SpanCompactor` → zstd Parquet under `data-spans/` + a spans skip index that blooms
  over `name` tokens **and whole `trace_id` values** (so a lookup by trace ID prunes hard). Free-text
  over `name` is *substring*, so `span_text_tokens` only bloom-prunes on the **interior** tokens of the
  search string (`photon_index::interior_tokens`, shared with the logs path) — never on an edge token,
  which could be a fragment of a longer name word (see [`logs.md`](logs.md)).
- **Query** (`SpanQueryEngine`, `photon-query`):
  - `get_trace` — all spans of one trace, for the waterfall.
  - `search_traces` — rolled-up `TraceSummary` list. To avoid a full-window rescan, step 3 hydrates
    the picked trace ids over a window narrowed to each kept trace's `min(start) ± 1h`
    (`TRACE_TIME_HINT_PADDING_NANOS`), the same padding `get_trace` uses. Known limitation: a trace
    whose spans span **> 1h** from its earliest span can be undercounted in the rollup (its late
    spans fall outside the padded window) — consistent with what `get_trace` can render; widen the
    pad if sub-hour traces stop being the norm.
  - `search_spans` / `search_spans_with_count` — raw span rows (cursor-paginated).
  - `count_matching_spans`, span `facet` / `fields` / `histogram`.
  - `latency` — t-digest percentiles + log-scale (geometric) histogram.
  - `SpanSort` controls ordering; all three sorts (`Recent`, `Slowest`, `Errors`) use the two-pass
    late-materialization trick (`span_search.rs`) — pass 1 projects only the sort key(s) to find a
    cutoff, pass 2 re-filters on it before decoding the wide `attributes` map. `Errors` sorts on
    the COMPOSITE `(status_code, start_time)` key, so its cutoff is a `(status, start)` pair. All
    three append a deterministic `(span_id ASC, trace_id ASC)` tiebreaker as the final ORDER BY
    key in both passes — `span_id` is only unique *within* a trace per OTLP, but the pair is
    unique per row, giving a true total order so pagination at exact ties is stable across pages
    (not plan-dependent).

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

**Filter persistence.** The explorer's whole view state lives in the query string: `q` (the grammar
query every facet/quick-filter writes into — see `useUrlState`) plus `sort` and `mode`, layered on
by TracesExplorer's own `flush: 'post'` watcher. That watcher is `immediate`, so the entry carries
the full state from mount, not only after the first change; unknown `sort`/`mode` values are
rejected at seed time rather than forwarded to the API. The detail view's back button therefore
**pops** back to the explorer's own history entry (`lib/core/useBackTo.ts` — `router.back()` when
the previous entry is `/traces`, `router.push` only for deep links), which is what makes the
filters survive a drill-in → back round trip. `ServiceDetailView` uses the same helper.

**Waterfall layout.** The label column (service · operation, indented 14px per depth) is
**user-resizable** via the splitter between it and the bar track — drag, or focus it and use ←/→
(Shift for a bigger step), Home/double-click to reset. The width is clamped to 160–720px and
persisted in `localStorage` under `photon:waterfall-label-width`. The axis, the gridline overlay and
the rows all read one `GRID_TEMPLATE_COLUMNS` computed and share identical horizontal insets, so
ticks/gridlines stay locked to the bars. Bar geometry floors every span at 0.5% width and slides
`left` back to fit (never trims width against `100 - left`, which used to erase spans sitting at the
very end of a skewed trace); the duration caption flips to the other side of its anchor when the bar
ends near the right edge, so it can't be clipped by the rows' scroll container.

**Components** (`frontend/src/components/traces/`): `TraceTable`, `SpanTable`, `TraceWaterfall`,
`TraceMinimap`, `SpanDetailPanel`, `TracePeekDrawer`, `TracesFilters`, `LatencyHistogram`,
`SpanVolumeHistogram`. **Queries**: `frontend/src/lib/tracesQueries.js`. **Waterfall geometry**:
`frontend/src/lib/traceTree.js` — pure BigInt-nanosecond trace assembly + geometry, defensive against
orphans / cycles / clock skew.

**Correlation:** log→trace, span/trace→logs all flow through `router.push` (see the router redirect
`/traces/metrics → /services`, kept for old links).
