# Logs

The original signal and the template every other signal follows. An OTLP log explorer with a small
query grammar, facets, a severity histogram, and live tail.

> Shared plumbing (WAL, compaction, pruning, invariants) is in
> [`../architecture.md`](../architecture.md); frontend conventions in [`../frontend.md`](../frontend.md).

## Backend

- **Ingest:** `otlp_logs_to_records` (`photon-ingest`) maps OTLP → `LogRecord` → the logs `wal`.
- **Schema** (`photon-core/src/schema.rs`): fixed typed columns — `timestamp` (ns),
  `observed_timestamp`, `severity_number`, `severity_text`, `body`, `trace_id`, `span_id`,
  `scope_name` — plus **promoted** dictionary columns (configurable via `[schema].promoted_attributes`,
  default `service.name` + `host.name`) and a single long-tail `attributes` `Map<Utf8,Utf8>`. Sorted
  `(service.name, timestamp)`; `service.name` is the required primary sort key.
- **Compaction:** `Compactor` (`photon-compact/src/compactor.rs`) → zstd Parquet under `data/` + a
  logs skip index (bloom over tokenized `body` + min/max on `timestamp`/`service.name`).
- **Query** (`QueryEngine`, `photon-query/src/lib.rs`):
  - `search` / `search_with_count` — two-pass late-materialized row search.
  - `count_matching` — true matched count independent of `limit`.
  - `facet` — top values + counts for a field.
  - `fields` — field catalog from the **manifest only, no data scan** (kinds:
    `fixed` | `promoted` | `attribute`).
  - `histogram` — severity-stacked volume buckets over the full match set.
  - `distinct_services`, `storage_stats`, raw `sql`.

## API

Every route requires the signed `photon_session` cookie. Errors are `{ error, offset? }` JSON.

| Route | Purpose | Handler |
|---|---|---|
| `GET /api/services` | distinct `service.name` | `services`* |
| `POST /api/search` | row search | `search.rs` |
| `GET /api/fields` | field catalog | `fields.rs` |
| `GET /api/facet` | top values by count | `facet.rs` |
| `GET /api/histogram` | severity-stacked volume | `histogram.rs` |
| `GET /api/stream/logs` | SSE live tail | `stream.rs` |

The aggregation logic the handlers call lives in `crates/photon-query/src/{facet,count,fields,histogram}.rs`.

## The log query grammar

Defined **once** in `photon-core/src/query/` (`parse` → AST → `FieldResolver` → `eval`; pure, no
DataFusion) and compiled two ways — to an in-memory predicate (`eval`) and to a DataFusion filter (in
`photon-query`) — so filter semantics have one source of truth.

| Syntax | Meaning |
|---|---|
| `field:value` | exact match |
| `field:v1,v2` | OR-list |
| `field:*` | field exists |
| `-field:v` | negate |
| `field>=n` `>` `<` `<=` | numeric compare |
| `"quoted text"` / bare words | case-sensitive **body** substring |

Terms are **AND**-ed. Parse errors carry a byte `offset` so the UI can underline the bad token. Free-
text is bloom-pruned then confirmed with a `strpos(body, text) > 0` substring scan — **no inverted
index**. The frontend has a display-only mirror in `frontend/src/lib/queryLang.js` (never validates).

## UI

`/logs` → `LogsView.vue`: filter rail + volume histogram + virtualized log table + detail drawer,
with live tail.

- **Components** (`frontend/src/components/logs/`): `LogTable`, `LogDetailDrawer`, `LogsFilters`,
  `VolumeHistogram`, `SeverityTag`.
- **Queries** (`frontend/src/lib/logsQueries.js`): `useServices`, `useFields`, `useSearchLogs`,
  `useFacet`.
- Within-view filter state (query/time/facets) lives in URL params (`lib/useUrlState.js`).
  Cross-view correlation (log→trace) flows through `router.push`.
