# Traces & Metrics Query — Perf & Correctness Audit
**Scope:** photon-query (span_* + metric_* + trace_list)  ·  **Date:** 2026-07-06

Method: static read of all in-scope files plus their API call sites
(`traces_search.rs`, `traces_agg.rs`, `metrics.rs`). No benchmarks run — quantitative
claims are HYPOTHESIS unless tagged MEASURED. Nothing below violates the load-bearing
invariants (pruning stays conservative, local-fs-only, two-pass logs untouched).

---

## TL;DR — biggest 10x levers

1. **`search_traces` step-3 re-scans the *entire* time window** to fetch the ≤2000 capped
   traces' spans — no trace-id-bloom prune, no window narrowing — even though step 2 already
   knows the exact start-times of the 2000 traces it kept. On a broad window this is the
   dominant cost of the trace-list view. (F1, P1 speed)
2. **Cumulative-metric `pointwise` and all distribution (`collect_dist_series`) paths
   materialize *every* raw point / JSON payload of *every* matching series into RAM** before
   the `MAX_SERIES` cap is applied. A high-cardinality counter/histogram over a wide window is
   an unbounded-memory / OOM query. (F2, P1 memory)
3. **`buckets` is unbounded on `/api/traces/histogram` and `/api/traces/latency`** →
   `vec![0u64; buckets]` with an attacker-chosen `?buckets=2000000000` aborts the process.
   (F3, P1 memory/robustness)
4. **`search_traces` step 2 collects *all* distinct `trace_id`s into a Rust `Vec`** to compute
   `matched_count`, then throws all but 2000 away. Millions of traces in the window = a
   multi-MB Rust alloc on top of DataFusion's group-by hash table. (F4, P2 memory)
5. **`bucket_of` / `bucket_index_expr` do the `(ts-start)*buckets` multiply in i64** and
   overflow for wide windows (≈>35 days at 3000 buckets) — while the sibling `bucket_start`
   correctly uses i128. Wide metric charts either error or (if arrow wraps) silently corrupt.
   (F5, P2 correctness, NEEDS-BENCH)

---

## Findings (ranked)

### F1 — `search_traces` fetches whole-trace spans by rescanning the full window · P1 · speed
- **Where:** `trace_list.rs:170-231` (`search_traces`, step 3 + `whole_req`)
- **What:** After step 2 picks the newest ≤`MAX_CANDIDATE_TRACES` (2000) trace ids, step 3
  re-opens survivors with `query:None` (`whole_req`, l.176-188 — time-prune only, **same
  `[start,end]` as the request**) and filters `trace_id IN (<2000 lits>)` (l.195, l.224-225).
  Because `whole_req.query` is `None`, `span_prune` emits no name tokens and keeps *every*
  file overlapping the window; the `trace_id IN (...)` predicate cannot prune Parquet
  (random-hex ids ⇒ useless min/max stats) and is **not** checked against the per-file
  `trace_id` bloom. Net: step 3 scans all span rows in the window to recover 2000 traces.
- **Why it matters:** Trace-list is the default traces view and runs on every window change.
  On a wide window (hours/days) this is a full-dataset scan for a page of 100 traces. The two
  spawn_blocking prunes + two `read_parquet` opens (step 2 `df`, step 3 `whole_df`) compound it.
- **Fix (two cheap, composable levers):**
  (a) **Narrow the step-3 window to the capped set.** Step 2 already has each kept trace's
  `min(start)` (the `matched` vec, l.129/148); after truncation you know `min`/`max` start of
  the 2000. Build `whole_req` with `start = min_capped - PAD`, `end = max_capped + PAD` (reuse
  the `get_trace` ±1h `TRACE_TIME_HINT_PADDING_NANOS`). The 2000 newest traces are usually a
  thin recent slice, so most files drop out at the manifest.
  (b) **Bloom-prune step 3 by the capped ids.** Add a `span_prune` variant that keeps a file
  iff its `.idx` `trace_id` bloom `might_contain_token` for *any* capped id (≤2000 bloom probes
  per file, all in-memory). Missing `.idx` ⇒ keep (invariant preserved).
- **Effort/Risk:** M / M (touches the hot trace-list path; needs the existing correctness
  tests in `tests/trace_search.rs` to stay green).
- **Invariant check:** OK. Bloom keep-on-miss and keep-on-maybe preserve "never false-negative";
  matching is still done on the free-text-pruned `df` in step 2, unchanged.

### F2 — pointwise & distribution paths materialize all rows before capping series · P1 · memory
- **Where:** `metric_query.rs:382-391` + walk `417-477` (`query_series_pointwise`);
  `metric_dist.rs:128-217` (`collect_dist_series`, feeds histogram/exp-histogram/summary)
- **What:** Both do `filter → select → sort → collect()` and then group/cap in Rust. The
  `MAX_SERIES` (1000) cap is applied **while walking the already-collected batches**
  (`metric_query.rs:451`, `metric_dist.rs:148`), so peak memory = *all* rows matching the
  metric+filter in the window, not 1000 series' worth. `collect_dist_series` additionally
  projects the raw JSON payload column `__j` (`metric_dist.rs:121`) — a `String` per row —
  so a cumulative-histogram metric with fine scrape interval × many series holds every
  serialized bucket array in RAM at once.
- **Why it matters:** These are the counter-`rate`/`increase`, gauge-`last`, and *all*
  histogram/exp-histogram/summary aggregations — i.e. most non-gauge metric charts. Cardinality
  × window is user-controlled; there is no row ceiling. HYPOTHESIS: a 100k-series counter over
  30 days is tens of millions of rows collected → OOM.
- **Fix:** (1) Enforce the series cap *before* full materialization — e.g. a
  `COUNT(DISTINCT fingerprint)` guard (you already compute `series_fingerprint` in
  `metric_catalog.rs`) that rejects/《caps》 the query up front; or (2) stream per-series by
  pushing an `ORDER BY group, ts` + windowed read and flushing each series as its key changes
  (the walk already detects key boundaries — it just needs the *input* bounded). At minimum add
  a hard scanned-row ceiling that errors loudly rather than OOMs.
- **Effort/Risk:** L / M (architectural; DataFusion has no per-group limit operator, so a true
  fix means chunked reads or a pre-count guard).
- **Invariant check:** OK (no pruning change).

### F3 — unbounded `buckets` on span histogram/latency → allocation DoS · P1 · memory/robustness
- **Where:** API `traces_agg.rs:104-111` (`buckets: usize`, default 48, **no clamp**), passed
  to `span_histogram.rs:35`/`span_latency.rs:45` which only do `buckets.max(1)` (lower bound
  only). Alloc sites: `span_histogram.rs:50-60`/`157` (`vec![...; buckets]`),
  `span_latency.rs:157`/`175`.
- **What:** `GET /api/traces/latency?...&buckets=2000000000` makes the engine allocate
  `vec![0u64; 2e9]` (16 GB) / a 2e9-element `Vec<LatencyBucket>` → OOM/abort. `usize::MAX`
  panics. (Same shape exists in the logs `histogram.rs` — systemic, not spans-only — but the
  spans endpoints are in scope.)
- **Why it matters:** Single unauthenticated-shaped request pattern (post-login) can crash the
  single-node process. Metrics are *not* affected — `buckets_for` clamps to `MAX_BUCKETS=3000`
  (`metrics.rs:58-67`); the traces endpoints simply skipped that clamp.
- **Fix:** Clamp in the engine: `let buckets = buckets.clamp(1, MAX_BUCKETS)` in
  `SpanQueryEngine::histogram`/`latency` (defensive, one line each), and/or reuse a shared
  `MAX_BUCKETS` const in `traces_agg.rs`.
- **Effort/Risk:** S / S.
- **Invariant check:** N/A.

### F4 — step 2 collects every distinct trace_id for `matched_count` · P2 · memory
- **Where:** `trace_list.rs:116-152` (aggregate `GROUP BY trace_id`, collect into `matched`),
  `161-169` (sort in Rust, then `truncate(2000)`)
- **What:** The whole distinct-trace-id set is pulled to Rust (`(String,i64)` per trace) purely
  to (a) report `matched_count = matched.len()` and (b) pick the newest 2000. Everything past
  2000 is dropped at l.167.
- **Why it matters:** High-cardinality windows (millions of traces) allocate a large Rust vec +
  a full DataFusion group-by hash table, then discard 99.x%. Adds to F1's cost on the same path.
- **Fix:** Push the ranking into SQL: `… .aggregate([trace_id],[min(start)]).sort(min DESC)
  .limit(0, 2000)` to bring back only 2000 rows, and get `matched_count` from a separate
  `COUNT(DISTINCT trace_id)` aggregate (or `approx_distinct` if an estimate is acceptable for
  the toolbar total). Trades a second aggregate for bounded Rust memory — pairs naturally with
  F1(a) since it yields the capped ids *and* their min/max start.
- **Effort/Risk:** M / M.
- **Invariant check:** OK.

### F5 — i64 overflow in bucket-index arithmetic on wide windows · P2 · correctness · NEEDS-BENCH
- **Where:** `metric_query.rs:79-89` (`bucket_index_expr`, DataFusion `(ts-start)*buckets/span`),
  `metric_query.rs:502-505` (`bucket_of`, Rust `(ts-start)*buckets/span`),
  `span_histogram.rs:84` (`(start_time-start)*buckets/span`)
- **What:** The index math multiplies in i64. Overflow when `(ts-start)*buckets > i64::MAX`
  (≈9.2e18). With the metrics cap `buckets=3000` that is `span > ~35.6 days`; with the default
  `buckets=200`, `span > ~533 days`. Note the sibling `bucket_start` (l.72-75) already uses
  i128 — the index path was left in i64, so the two disagree exactly where it matters.
- **Why it matters:** Retention-scale metric queries (30–90 days) with a fine `step` reach
  `buckets≈3000`. NEEDS-BENCH: if arrow 53 / DF 43 integer multiply **errors**, wide charts
  fail with a confusing message (P2); if it **wraps**, buckets are misassigned → silently wrong
  quantiles/rates (would be P1). Determine which before ranking finally.
- **Fix:** Compute the index in i128 in `bucket_of`; for the DataFusion exprs avoid the multiply
  by dividing first (`(ts-start) / step` with `step = (span/buckets).max(1)`) or cast the
  operands to a wider/`Float64` domain. Keep it consistent with `bucket_start`.
- **Effort/Risk:** S / M (edge-alignment vs `bucket_start` needs a unit test at the boundary).
- **Invariant check:** N/A.

### F6 — Slowest/Errors span search skips two-pass late materialization · P2 · speed
- **Where:** `span_search.rs:138-162` (`search_single_pass`)
- **What:** `Recent` uses the correct two-pass cutoff trick (`search_recent`, l.88-134;
  mirrors logs). `Slowest`/`Errors` do `filter → sort → limit(offset,limit)` with **no
  projection before the sort**, so DataFusion decodes the full row (incl. the wide `attributes`
  map) for every matching row, sorts them all, then keeps a page. This is the exact ~5x
  regression the two-pass design avoids for `Recent`. Documented as a v1 deferral in the module
  header, flagged here as the concrete lever.
- **Why it matters:** "Slowest traces/spans" is a primary triage sort; on a broad, low-selectivity
  window it sorts the entire match set with heavy columns materialized.
- **Fix:** Apply the same cutoff per sort key: pass 1 projects only `duration_nanos` (Slowest)
  or `status_code,start_time` (Errors), sort DESC, take `offset+limit`, read the cutoff; pass 2
  re-filter `key >= cutoff` (or `> cutoff` with tie handling) and materialize full rows.
- **Effort/Risk:** M / M.
- **Invariant check:** OK (same semantics, narrower pass-2 set).

### F7 — API re-prunes/re-opens for spans search + count instead of the combined method · P2 · speed
- **Where:** API `traces_search.rs:181-192` calls `search_spans` **then** `count_matching_spans`
  separately; each calls `span_survivors_df` (prune via spawn_blocking + `read_parquet`).
  Meanwhile `span_search.rs:67-84` `search_spans_with_count` shares one open — and has **zero
  callers** (verified: only its own definition references the name).
- **What:** Every `/api/spans/search` does 2 prunes + 2 Parquet opens where 1 suffices; a Recent
  search then runs its 2 internal passes off one of them and the count off the other.
- **Why it matters:** Doubles pruning I/O (manifest read + per-file `.idx` reads) and Parquet
  footer/open work on a paged, interactive endpoint.
- **Fix:** Switch the handler to `search_spans_with_count(query)` (returns `(rows, matched)` from
  one `span_survivors_df`). The method already exists and is tested-by-construction.
- **Effort/Risk:** S / S.
- **Invariant check:** OK.

### F8 — `metric_meta_probe` adds a prune+open per metric per query · P2 · speed
- **Where:** `metric_query.rs:94-122` (`metric_meta_probe`) then the chosen agg path
  (`series_sql_agg` l.228 / `query_series_pointwise` l.353 / `collect_dist_series` l.109) each
  call `survivors_df` again.
- **What:** Discovering type/temporality/monotonicity requires a full prune + `read_parquet` +
  `LIMIT 1` scan *before* the real aggregate re-prunes and re-opens. The metrics dashboard loops
  over `req.queries` (`metrics.rs:158`), so it's ×2 opens per panel query.
- **Why it matters:** Extra manifest+`.idx`+footer I/O per chart; scales with dashboard panel count.
- **Fix:** Cache per-metric metadata (type/temporality/monotonic/unit rarely change) keyed by
  metric name with a short TTL, or fold the probe columns into the first aggregate's output and
  branch afterward. The manifest doesn't carry metric type, so a small metadata cache is the
  pragmatic win.
- **Effort/Risk:** M / M.
- **Invariant check:** OK.

### F9 — `latency` scans the survivor set twice · P2 · speed
- **Where:** `span_latency.rs:106-120` (global `max`+percentiles aggregate) then `145-155`
  (bucket-count aggregate) — two `collect()`s over the same `filtered` DataFrame ⇒ two Parquet
  scans per latency request.
- **What:** The first pass exists only to learn `max(duration)` to size the linear bins before
  the second pass can bucket.
- **Why it matters:** Doubles the scan for the latency panel.
- **Fix:** Adopt fixed log-scale duration bins (the "log-scale follow-up" the code TODO already
  anticipates) so no `max` probe is needed → single pass; or read `max(duration)` from the
  skip-index/manifest if it's tracked there (it isn't today). Percentiles already come from the
  t-digest in the same pass, so only the bin-sizing probe is the waste.
- **Effort/Risk:** M / M.
- **Invariant check:** OK.

### F10 — window-straddling traces get partial rollups · P3 · correctness
- **Where:** `trace_list.rs:176-184` (`whole_req` keeps the request's `[start,end]`) + step-3
  filter applies no per-row time predicate.
- **What:** "Whole-trace" rollups actually mean "whole-trace within files overlapping the
  window." A long trace whose spans live partly in files outside `[start,end]` yields
  undercounted `span_count`/`services` and a possibly-wrong representative/duration. Unlike
  `get_trace`, there is no ±padding here.
- **Why it matters:** Minor skew at window edges / for long traces; not a crash or a pruning
  violation. Largely subsumed by F1(a), which would add padding anyway.
- **Fix:** Pad the step-3 window (F1a) so a trace's neighbouring-file spans are included.
- **Effort/Risk:** S / S.
- **Invariant check:** OK.

### F11 — histogram cumulative bounds taken from first row only · P3 · correctness
- **Where:** `metric_dist.rs:301-347` (`histogram_series`): canonical `bounds` = first row with
  non-empty `explicit_bounds`; cumulative delta resets only on *length* change (`cur.len() !=
  pc.len()`, l.325), not on same-length differing bound *values*.
- **What:** If a series changes its explicit bounds to a different vector of the same length
  mid-window, deltas mix incompatible layouts silently. Rare in practice (bounds are usually
  stable per series).
- **Fix:** Also reset when the bound vector value changes, not just its length.
- **Effort/Risk:** S / S.
- **Invariant check:** N/A.

---

## Quick wins
- **F3** clamp `buckets` (one line each in `histogram`/`latency`) — kills an OOM vector.
- **F7** point the handler at `search_spans_with_count` — halves prune/open on spans search.
- **F5** i128 `bucket_of` (+ divide-first exprs) — remove the wide-window overflow.
- **F1(a)** narrow step-3 window using the capped traces' known min/max start — big trace-list
  win for little code.

## Bigger bets (architectural)
- **F2** bound distribution/pointwise memory: pre-count series guard or chunked per-series
  streaming; today these are the only unbounded-RAM query paths in scope.
- **F1(b)+F4** a precomputed per-trace rollup (the design's own deferred item) collapses
  `search_traces` from "2 prunes + 2 full scans + Rust rollup" to a manifest/rollup lookup and
  makes the 2000-trace cap unnecessary.
- **F8** per-metric metadata cache to drop the probe open on dashboards.

## Already good / no action
- **Pruning is correct and conservative everywhere in scope** — missing/erroring `.idx` ⇒ keep
  the file (`span_engine.rs:165,325`; `metric_engine.rs:195`), single-token blooms for
  `trace_id`/`metric_name`, timestamp overlap belt-and-suspenders. No false-negative risk found.
- **`search_spans` `Recent` two-pass late materialization** faithfully mirrors the logs engine
  (`span_search.rs:88-134`) — projects only `start_time`, finds the cutoff, then materializes
  full rows `>= cutoff`.
- **Aggregations push heavy work to DataFusion, fold only the tiny grouped result in Rust**
  (`span_facet`, `span_histogram`, `metric_catalog` — the catalog reads type/unit/last-seen/
  series-count in one grouped aggregate over time-pruned files, no per-metric re-scan).
- **`session()` enables Parquet `pushdown_filters` + `reorder_filters` + projection pushdown**
  (`lib.rs:669-682`), so predicates apply during decode and unrequested columns (the wide
  `attributes` map) aren't materialized — trace_list step 3 correctly omits `ATTRIBUTES` unless
  `projected_attributes` is set (`trace_list.rs:218-220`).
- **Sync fs pruning runs in `spawn_blocking`** everywhere (`get_trace`, `span_survivors_df`,
  `survivors_df`, `time_survivors_df`) — never blocks a tokio worker.
- **Quantile / counter math is careful and table-tested:** `interpolate_quantile` handles
  empty/total-zero, the +Inf overflow bucket, and boundary ranks (`metric_dist.rs:62-88`);
  reset-aware cumulative deltas (value-decrease OR start_ts-advance OR shape change ⇒ reset;
  first sample contributes 0) are consistent across the scalar, histogram, and exp-histogram
  paths and each has a hand-computed unit test. Div-by-zero is guarded in `latency`
  (`max_duration<=0`), avg (`count>0`), and every `bucket_start`/`step` via `.max(1)`.
- **Exp-histogram scale alignment** (`ExpAccum::add`/`exp_downscale`/`merge_dense`) uses
  arithmetic shift toward −∞ for negative indices — correct for signed bucket downscaling.

## Open questions & NEEDS-BENCH
- **F5**: does arrow 53 / DataFusion 43 integer `*` **error** or **wrap** on i64 overflow? That
  decides whether F5 is "wide charts fail" (P2) or "wide charts silently wrong" (P1). Quick
  bench: metric `query` over a 60-day window with `step=1s` (→ buckets 3000).
- **F1**: MEASURE trace-list latency for a 24h window on a loadgen-populated store before/after
  the window-narrow + bloom-prune — confirm the full-window scan is in fact the dominant cost.
- **F2**: MEASURE peak RSS of a cumulative-histogram `query` at ~10k–100k series over a wide
  window to size the row ceiling / confirm OOM risk.
- **In-list scaling**: `trace_id IN (2000 lits)` (`trace_list.rs:195`) — DF 43 builds a static
  hash-set for large literal in-lists, so per-row eval should be O(1); the cost in F1 is scan
  *volume*, not the in-list. Worth a spot-check that DF isn't doing a linear scan here.
