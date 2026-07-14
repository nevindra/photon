# Logs Query Engine + Skip Index — Perf & Correctness Audit
**Scope:** photon-query (logs/shared), photon-index  ·  **Date:** 2026-07-06
**Method:** static read of the source; no benchmarks exist in the tree (no `criterion`, no `#[bench]`),
so every speed number below is **HYPOTHESIS / NEEDS-BENCH**. One measured structural fact: the
consolidating `search_with_count` has **zero callers** (grep-verified).

---

## TL;DR — biggest 10x levers

1. **[P0 correctness] Bloom pruning can drop real matches for substring free-text search.**
   Free-text is a *substring* match (`strpos(body,text)>0`), but pruning tests *whole tokens* in the
   bloom. `body="timeout"` is pruned for the search `tim`/`timeou`/`timeouts`. Silent data loss;
   violates the load-bearing "never false-negative" invariant. This is the single most important
   finding — it is a correctness bug, not a perf one.

2. **[P1 speed] Each `/api/search` does the whole prune+open+scan pipeline ~twice.**
   The handler calls `search()` and then `count_matching()` independently — two prunes, two
   `read_parquet` opens, three DataFusion executions. The purpose-built `search_with_count`
   (one prune, shared `DataFrame`) is already written but **never called**. ~1 line to adopt.

3. **[P2 speed] Nothing is cached between queries at the DataFusion / SkipIndex layer.**
   A fresh `SessionContext` per call → Parquet footers re-read every query; every `.idx` is
   re-read + re-parsed on every query. Live-tail refetches (every few seconds over the same files)
   pay this in full each tick.

4. **[P2 memory] Facet/`GROUP BY` holds full distinct-value state in RAM before the limit.**
   Faceting a high-cardinality field (request_id, url, …) over huge data = O(distinct) group table,
   no cardinality guard → OOM risk on a single node.

---

## Findings (ranked)

### P0 · correctness · Bloom pruning drops rows that substring free-text would match (false-negative)
- **Where:** `photon-query/src/lib.rs:600` (`text_tokens`) → `:436-453` (`keep_candidate` bloom gate);
  the confirming predicate is a *substring* match at `lib.rs:639-642` (`base_predicate`, `strpos`) and
  `photon-query/src/predicate.rs:86-88` (`FreeText` → `strpos`), mirrored in-memory at
  `photon-core/src/query/eval.rs:42-44` (`body.contains(text)`). Tokenizer: `photon-index/src/tokenize.rs:19`.
- **What:** Free-text search semantics are "**`body` contains `text` as a substring**" (documented in
  CLAUDE.md and implemented via `strpos`/`contains`). But file pruning tokenizes the search text
  (`text_tokens` → `tokenize`) and keeps a file only if the bloom `might_contain_all` those **whole
  tokens**. Whole-token membership is **strictly narrower** than substring containment, so pruning can
  drop a file that actually contains a matching row.
- **Concrete repros (all silently return nothing / miss files):**
  - search `tim` → body `"timeout"` matches substring, but bloom holds token `timeout`, not `tim` → file pruned.
  - search `timeout` → body `"timeouts"` matches, bloom holds `timeouts` → pruned. (Even *full-word* searches are unsafe due to suffixes/plurals.)
  - phrase `"baz qux"` → body `"xbaz quxy"` matches substring, but both boundary tokens are partial → pruned.
- **Why it matters:** This is the platform's #1 invariant ("pruning can only add work, never drop a real
  result"). It is *silent* — the query returns fewer rows / a smaller `matched_count` with no error. It
  bites the most common interactive behavior (typing partial words into the search box). High volume makes
  it worse: more files pruned = more real matches lost.
- **Why the tests don't catch it:** `grammar_consistency.rs` only checks `eval` == DataFusion predicate
  (both substring — they agree) and never exercises the bloom. `skip_index.rs::bloom_never_reports_a_false_negative`
  only asserts *whole tokens that are present* return `true`; it never models the substring-vs-token gap.
- **Fix (two sound options):**
  1. *Keep substring semantics, prune only provably-safe tokens.* A substring `S` present in a body guarantees
     that each **interior, fully-delimited** token of `S` is a whole token of that body — but the **first and
     last** tokens of `S` may be partial (prefix/suffix of a larger body token). So bloom-test only tokens that
     are bounded by a non-alphanumeric char on **both** sides *within `S`*. A single-word search (`tim`,
     `timeout`) has no interior token → cannot be bloom-pruned at all (keep all candidate files; DataFusion
     still confirms per row). This preserves the invariant with a small pruning-power loss on single-word text.
  2. *Change free-text to whole-word/token matching* (what most log tools do). Then bloom pruning is exactly
     sound. This is a product/semantics change (breaks partial-word search) and must be reconciled across
     `eval`, `predicate`, and CLAUDE.md.
- **Effort/Risk:** Option 1: **S/low** (a helper that filters `text_tokens` to interior tokens; empty ⇒
  skip bloom). Add a property test: random substrings of random bodies must never be pruned away.
- **Invariant check:** This finding *is* the invariant. Option 1 restores false-negative safety; the
  first/last-token exclusion is the crux — do not "fix" by making the tokenizer match (tokenizers already
  agree; the gap is substring vs. token, not build-vs-query divergence).

### P1 · speed · `/api/search` runs the prune+open+scan pipeline twice; `search_with_count` is dead code
- **Where:** `photon-api/src/search.rs:90` (`state.query.search(query.clone())`) then `:100-104`
  (`state.query.count_matching(query)`); the consolidating method sits unused at
  `photon-query/src/lib.rs:270-282` (`search_with_count`).
- **What:** `search()` → `survivors_df` (prune in `spawn_blocking` + `read_parquet`) then two `collect`s
  (pass 1 + pass 2). `count_matching()` → **another** `survivors_df` (second prune: re-reads every `.idx`,
  re-opens every Parquet footer) + a third `collect`. So one user search = **2 prunes + 2 `read_parquet`
  opens + 3 executions**. `search_with_count` was built to do **1 prune + 1 open**, sharing the cheap
  `DataFrame` handle between `search_over` and `count_over`, but nothing calls it.
- **Why it matters:** Every interactive search and every live-tail refetch pays the prune (all `.idx`
  reads + bloom checks) and the Parquet-footer opens twice. On a broad window with many surviving files,
  the duplicated prune + metadata open is pure waste.
- **Fix:** Have the handler call `search_with_count(query)` once and use both returns. Deletes a whole
  prune + open per request. (Still 3 `collect`s — pass1/pass2/count — but over one opened set.)
- **Effort/Risk:** **S/low** — the method, its tests, and the shared `survivors_df`/`count_over` already exist.
- **Invariant check:** Two-pass late-materialization preserved (it *is* `search_over`); no pruning change.

### P2 · bug/robustness · A corrupt or unreadable `.idx` fails the whole query instead of "keep the file"
- **Where:** `photon-query/src/lib.rs:441-453` (`keep_candidate`): a non-`NotFound` read error
  `return Err(...)`, and `SkipIndex::from_bytes(&bytes)?` on a decode error → both propagate out through
  `prune` → `survivors_df` → `search`.
- **What:** The invariant/comment says an unreadable `.idx` must be treated as "keep the file"
  (conservative). Only the `ErrorKind::NotFound` arm does that. A permission/IO error, a **truncated** blob,
  or a **garbage** blob returns `Err`, which aborts the entire search over that time window (the API then
  logs and returns empty results — user sees *nothing*, not "one file un-prunable").
- **Related panic risk:** `idx_binary::decode` (`skip_index.rs:275-279`) reads `num_bits`/`bits_len` from
  the file and reconstructs the bloom (`bloom.rs:67`) with **no validation** that `bits.len()*8 >= num_bits`
  or `num_bits > 0`. A well-framed-but-corrupt `.idx` then panics at query time inside `Bloom::might_contain`
  → `index_for` (`bloom.rs:75-80`): `% num_bits` divides by zero, or `self.bits[idx/8]` indexes out of bounds.
  A panic in the `spawn_blocking` prune surfaces as `PhotonError::Query("prune task panicked")` — again
  taking down the query rather than keeping the file.
- **Why it matters:** One bad sidecar (torn write, disk corruption, partial replication) can black-hole all
  searches over its window — the opposite of the "conservative, keep the file" guarantee.
- **Fix:** In `keep_candidate`, on *any* read error other than success and on `from_bytes` `Err`, log once and
  `return Ok(true)` (keep the file). Add validation in `decode` (`num_bits > 0`, `bits.len() == num_bits.div_ceil(8)`)
  returning `PhotonError::Index` so the keep-the-file path triggers instead of a panic.
- **Effort/Risk:** **S/low.**
- **Invariant check:** Strengthens false-negative safety (keep-on-doubt) and removes a crash path.

### P2 · speed/memory · No cross-query caching of Parquet metadata or parsed skip indexes
- **Where:** `photon-query/src/lib.rs:669-682` (`session()` builds a fresh `SessionContext` per call);
  `:321-329` (`survivors_df` → `read_parquet` re-opens files each call); `:441-452` (`keep_candidate`
  `std::fs::read` + `SkipIndex::from_bytes` on every candidate, every query); `skip_index.rs:278`
  (`.to_vec()` copies the bloom bits on each decode).
- **What:** Each search/count/facet/histogram constructs a new context and re-opens every surviving Parquet
  file's footer/metadata; pruning re-reads and re-parses every `.idx` from scratch. Nothing is memoized
  between the many calls that make up one page load (search, count, facet, histogram, fields) or between
  successive live-tail refetches over an unchanged file set.
- **Why it matters:** For repeated queries over a stable hot set (the common live-tail case), footer reads
  and `.idx` parse cost are paid again every few seconds. `.idx` blooms are KB–tens-of-KB each; thousands of
  candidate files on a broad text search = thousands of sequential reads + allocations per query.
- **Fix:** (a) Cache parsed `SkipIndex` keyed by `(segment_id, mtime/len)` — segments are immutable once
  written, so this is a near-perfect cache; invalidate exactly like `ManifestCache`. (b) Read/parse candidate
  `.idx` files concurrently (rayon/`buffer_unordered`) rather than the current sequential loop. (c) Consider a
  longer-lived `SessionContext` or a `ListingTable`/`ParquetExec` with cached file statistics so footers
  aren't re-read across the calls of a single page load. **NEEDS-BENCH** to size the win.
- **Effort/Risk:** SkipIndex cache **S/M**; context/metadata reuse **M** (watch the `Utf8`-not-`Utf8View`
  and pushdown config must stay on any shared context).
- **Invariant check:** Cache is immutable-segment keyed → no pruning-power change. No two-pass change.

### P2 · memory · Facet aggregation is unbounded in group cardinality
- **Where:** `photon-query/src/facet.rs:55-80` (`facet_over`: `GROUP BY value_expr` → `ORDER BY count` →
  `limit+1`).
- **What:** The `limit` is applied *after* the full `GROUP BY`. DataFusion must hold one hash-table entry
  per distinct value before it can sort by count and cut to `limit+1`. Faceting a high-cardinality field
  (request id, URL, user id) over huge data materializes all distinct values in RAM.
- **Why it matters:** Single-node, low-memory target. There is no cardinality guard and no session memory
  limit set (`session()`), so a facet on the wrong field can OOM the whole binary.
- **Fix:** Set a `datafusion.execution` memory pool limit on the session so it errors instead of OOMing;
  and/or offer an approximate/heavy-hitters path (e.g., cap distinct groups, or `approx_distinct` gating)
  for known high-card fields. Same shape applies to `count`/`histogram` (bounded) and `red.rs`
  (bounded post-hoc by `MAX_RED_GROUPS` *after* full aggregation). **NEEDS-BENCH.**
- **Effort/Risk:** Memory-limit guard **S**; approximate facet **M/L**.
- **Invariant check:** N/A (aggregation, not pruning).

### P2 · bug · Histogram bucket arithmetic can overflow i64 on wide windows
- **Where:** `photon-query/src/histogram.rs:82-93` (`raw = (ts_nanos - start) * buckets / span`).
- **What:** The multiply happens in Int64 *before* the divide. For a wide window (e.g., an "all time"
  view: `start≈0`, data at `ts≈1.77e18` nanos, `buckets=100`), `(ts-start)*buckets ≈ 1.77e20` overflows
  i64 (`9.2e18`). Arrow's default multiply kernel wraps (or errors), producing wrong bucket assignments —
  a silently corrupt histogram. `span=(end-start).max(1)` at `:82` can also overflow if `end-start`
  overflows (e.g., `start<0`, `end=i64::MAX`).
- **Why it matters:** Reachable via a very broad/"all-time" range selection; the chart silently mis-buckets.
- **Fix:** Divide before multiply — compute `bucket_width = (span / buckets).max(1)` then
  `bucket = (ts - start) / bucket_width` (the existing `raw >= buckets` clamp still guards the top edge),
  or cast to `Int64`→`Decimal128`/i128 for the intermediate. Guard `span` against overflow.
- **Effort/Risk:** **S/low** (small rounding change at bucket edges, already clamped).
- **Invariant check:** N/A.

### P3 · speed · `distinct_services` full-column scan per manifest change; clones on cache hit
- **Where:** `photon-query/src/lib.rs:467-524`.
- **What:** On a services-cache miss (every compactor tick that rewrites the manifest) it `read_parquet`s
  **all** files and runs `DISTINCT`+`SORT` over `service.name`. Projection pushdown means only that column is
  read (good), and results are cached by `Arc` identity (good), but each cache hit still `(*services).clone()`s
  the whole `Vec<String>` (`:472-476`, `:523`).
- **Why it matters:** Service cardinality is naturally low, so the scan is cheap-ish and amortized; the
  per-call `Vec` clone is pure overhead on a hot endpoint.
- **Fix:** Return `Arc<Vec<String>>` (or `Arc<[String]>`) to callers instead of cloning. Optional:
  incremental distinct maintenance keyed off the manifest delta.
- **Effort/Risk:** **S/low.**
- **Invariant check:** N/A.

### P3 · speed · Manifest `candidates()` is an O(segments) linear scan allocating a `Vec` per query
- **Where:** `photon-core/src/manifest.rs:59-64`; called from `lib.rs:291` (prune), `fields.rs:34`, etc.
- **What:** Every query linearly filters all entries by time overlap and collects `Vec<&FileEntry>`.
  Entries are in insertion order (not time-sorted), so no binary search is possible.
- **Why it matters:** Negligible next to Parquet I/O until segment counts get very large; merge compaction
  keeps counts bounded, so this is low priority.
- **Fix (only if it shows up in a profile):** Keep entries sorted by `min_ts` (or a coarse time index) for a
  range scan; or return an iterator to avoid the per-query `Vec`.
- **Effort/Risk:** **M** (touches the compactor's manifest maintenance).
- **Invariant check:** Must remain conservative (keep on any overlap); a sorted index must not exclude
  overlapping-but-out-of-order segments.

---

## Quick wins
- Adopt `search_with_count` in the search handler (P1) — deletes a full prune+open per request.
- Keep-the-file on `.idx` read/decode errors + validate bloom dims in `decode` (P2 robustness) — removes a
  query-killing crash/abort path and honors the stated invariant.
- Divide-before-multiply in histogram bucketing (P2) — one-line overflow fix.
- Return `Arc<[String]>` from `distinct_services` (P3) — drop the per-hit clone.
- Set a DataFusion memory-pool limit on `session()` (P2 memory) — fail loud instead of OOM.

## Bigger bets (architectural)
- **Resolve the substring-vs-token free-text tension (P0).** Either restrict bloom pruning to interior
  delimited tokens (keeps substring semantics, sound) or move free-text to whole-word matching (fully
  sound, changes product behavior). Add a property test that random substrings are never pruned away.
- **Parsed-SkipIndex + Parquet-metadata caching keyed on immutable segments (P2).** The hot set changes only
  when the compactor writes; almost everything is cacheable. Biggest sustained win for live-tail.
- **Concurrent `.idx` pruning** for broad text searches over many candidates (P2).

## Already good / no action
- **Two-pass late materialization is correct and preserved** (`lib.rs:531-576`): pass 1 projects only
  `timestamp`; the cutoff is the `limit`-th newest; pass 2 re-applies `ts >= cutoff` + re-sort + re-limit, so
  ties at the cutoff are trimmed exactly like a single `ORDER BY ts DESC LIMIT` — no drop, no dup.
- **DataFusion scan is tuned well** (`session()`, `lib.rs:669-682`): `pushdown_filters` + `reorder_filters`
  (predicate pushdown / late materialization in the Parquet decode), `metadata_size_hint=512KiB` (one footer
  read), and `Utf8` (not `Utf8View`) for stable output arrays. Pass-1's `select(timestamp)` gives projection
  pushdown so the wide `attributes` map isn't decoded for the probe.
- **Pruning uses the manifest for min/max and only reads `.idx` when there's text** (`keep_candidate`,
  `lib.rs:410-453`) — no redundant re-derivation of ranges the manifest already carries.
- **Predicate compilation matches the in-memory evaluator** for the *grammar* (`predicate.rs`): the
  `IS [NOT] TRUE` collapse (with the documented reason it's not `CASE`, which DF-43 mis-simplifies) is proven
  equivalent by `tests/grammar_consistency.rs`. (The one gap is bloom pruning of substring free-text — P0 —
  which is a *pruning* mismatch, not a predicate mismatch.)
- **Binary `.idx` format** replaces per-number JSON parsing of the bloom bit vector (`skip_index.rs`
  `idx_binary`), with a bounds-checked forward cursor and a legacy-JSON fallback.
- **Tokenizer is shared and dedup-into** (`tokenize.rs`) so build-side and query-side can't diverge and
  allocation is O(distinct) not O(total).
- **Bloom** uses Kirsch–Mitzenmacher double hashing with a forced-odd step (`bloom.rs:75-80`) and standard
  `m`/`k` sizing; property tests assert no false-negatives for present whole tokens.
- **Aggregations return only small grouped results** (count/histogram/facet fold ≤ buckets×severities /
  ≤ limit+1 rows into Rust) — no full-result materialization on those paths.

## Open questions & NEEDS-BENCH
- **Substring free-text is the intended product semantics?** If yes, P0 fix option 1 (interior tokens); if
  whole-word search is acceptable, option 2 is simpler and fully sound. Needs a product call.
- **NEEDS-BENCH:** magnitude of the duplicate-prune cost (P1) and of parsed-SkipIndex / metadata caching
  (P2) on a realistic broad-window text search with thousands of candidate files. No harness exists yet;
  a `loadgen`-populated hot dir + a criterion bench over `QueryEngine::search` / `search_with_count` would
  quantify all three.
- **NEEDS-VERIFY:** does DataFusion 43 *error* or *wrap* on the histogram Int64 multiply overflow? Either
  way the fix stands, but the symptom (500 vs. silently-wrong chart) determines urgency.
- **Facet cardinality in practice:** which fields does the UI let users facet on? If any high-card field is
  reachable, the memory guard (P2) is urgent rather than defensive.
</content>
</invoke>
