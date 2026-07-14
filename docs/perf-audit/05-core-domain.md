# Core Domain (schema, manifest, grammar) — Perf & Correctness Audit
**Scope:** photon-core  ·  **Date:** 2026-07-06

## TL;DR — biggest 10x levers

1. **No dictionary encoding anywhere.** Every string column in all three signal schemas
   (`service.name` and other promoted attrs, `severity_text`, `scope_name`, span
   `kind_text`/`status_text`, metric `type_text`/`unit`) is plain `DataType::Utf8`. These are
   textbook low-cardinality columns. Dictionary-encoding them is the single biggest in-memory
   footprint + filter-speed lever at query and compaction time. **P1, NEEDS-BENCH.**
2. **The attributes `Map<Utf8,Utf8>` duplicates every key string on every row.** The map's
   `keys` StringArray physically re-stores `"http.method"`, `"http.route"`, … once per row.
   For attribute-heavy data this dominates the batch's RSS. Dictionary-encoding the map
   key/value fields is a large but high-payoff memory bet. **P1, NEEDS-BENCH.**
3. **The manifest is a linear `Vec<FileEntry>`; `candidates()` is an O(N) scan + fresh `Vec`
   allocation on every prune**, and prune runs once per aggregation per query across logs,
   spans, and metrics. Bounded by merge cadence today, but it is the first gate on the "HUGE
   data" path. **P2, NEEDS-BENCH on file count.**

Important scoping fact discovered during the audit: **the in-memory `ResolvedQuery::matches`
(`eval.rs`, `span_eval.rs`, `metric_eval.rs`) has no production caller** — every call site is a
`#[cfg(test)]` module or the two-backend consistency test. Production search runs entirely
through the DataFusion predicate (`photon-query`). So the eval per-row cost (per-row BTreeMap
lookups, per-row `str::parse::<f64>`) is **not** a production hot path; its real job is to be
the *reference-semantics oracle*. That reweights the findings: eval allocation is a non-issue,
but any eval-vs-DataFusion **semantic divergence** is dangerous precisely because eval is the
oracle the consistency test trusts.

---

## Findings (ranked)

### 1. [P1 · memory/speed] String columns are not dictionary-encoded
- **Where:** `schema.rs:50-58` (`LogSchema::new`), `span_schema.rs:57-72`, `metric_schema.rs:66-83`
- **What:** All identifier/low-cardinality string columns use `DataType::Utf8`:
  `severity_text`, `scope_name`, and every promoted attribute (incl. `service.name`) in logs;
  `kind_text`, `status_text`, `scope_name`, promoted attrs in spans; `type_text`, `unit`,
  `scope_name`, promoted attrs in metrics. None are `DataType::Dictionary(Int32, Utf8)`.
- **Why it matters:** MEASURED (from the schemas). Parquet applies dictionary encoding *on
  disk* automatically, so compression is partly covered — but the **in-memory** Arrow arrays
  are plain `StringArray` both while the compactor sorts `(service.name, timestamp)` and buffers
  the Parquet write, and while `photon-query` decodes columns to run `in_list`/`TryCast`
  filters. Low-cardinality Utf8 stores every repeated value's bytes and forces byte-wise string
  comparison per row; a dictionary column stores values once and compares `i32` keys. At the
  stated "HUGE data, LOW memory" target this is a per-column multiplier on RSS and on
  filter/sort CPU across `photon-compact` and `photon-query`.
- **Fix:** Change these fields to `DataType::Dictionary(Box::new(Int32), Box::new(Utf8))` and
  switch the corresponding builders (`StringBuilder` → `StringDictionaryBuilder<Int32Type>`).
  Start with the highest-leverage, lowest-cardinality ones: `service.name` (promoted),
  `severity_text`/`kind_text`/`status_text`/`type_text`/`unit`, `scope_name`.
- **Effort/Risk:** M/M — ripples into: `photon-compact` sort + Parquet write (must write the
  dictionary type and keep it on read), `photon-query` `predicate.rs`/`span_predicate.rs`/
  `metric_predicate.rs` (`in_list` and `TryCast` must accept a dictionary column — DataFusion
  supports this but verify), and the `read_parquet` schema handed to DataFusion (`lib.rs`).
- **Invariant check:** Neutral-to-positive for pruning (min/max stats still computable; sort
  key semantics unchanged). Does not touch grammar semantics. Does not bump arrow.

### 2. [P1 · memory] Attributes `Map<Utf8,Utf8>` re-stores every key (and value) per row
- **Where:** `schema.rs:69-82` (`attributes_map_type`), consumed by all three `*BatchBuilder`s
  (`record.rs:39/111-117`, `span_record.rs:56/148-154`, `metric_record.rs:57/117-123`)
- **What:** The map entries struct is `{keys: Utf8, values: Utf8}`. The `keys` array physically
  duplicates the same attribute-name strings on every row; low-cardinality values duplicate too.
- **Why it matters:** HYPOTHESIS (structural, high confidence). For workloads with many
  long-tail attributes, the keys buffer alone can rival the size of all the value data — pure
  redundancy, since a given segment has a small fixed key vocabulary. This inflates the batch in
  memory during compaction and every scanned batch at query time.
- **Fix:** Make the map's `keys` (and ideally `values`) field
  `DataType::Dictionary(Int32, Utf8)`, and build via a map builder whose key/value sub-builders
  are dictionary builders. This is the "bigger bet" version of finding #1 for the long-tail
  column.
- **Effort/Risk:** L/M — `MapBuilder<StringBuilder, StringBuilder>` becomes a dictionary-keyed
  map builder in three record builders; `get_field(attributes, name)` in the predicates must
  still resolve (works on dictionary-valued maps in DataFusion, but verify). Manifest
  `attribute_keys` unaffected.
- **Invariant check:** No grammar change; pruning unaffected (bloom is built from tokens, not
  the map layout). Keep the map declared keys-unsorted (`false`) as today.

### 3. [P2 · memory/speed] Record structs hold `BTreeMap<String,String>` + owned `String`s — per-record allocation churn at ingest
- **Where:** `record.rs:6-18` (`LogRecord`), `span_record.rs:5-26` (`SpanRecord`),
  `metric_record.rs:12-34` (`MetricPoint`)
- **What:** Each record owns a `BTreeMap<String,String>` of *all* attributes plus several
  owned `Option<String>` fields. On the OTLP→record→builder→drop path, every record allocates
  one `String` per key and per value plus BTree node allocations, and the keys are re-allocated
  identically for every record in a batch.
- **Why it matters:** HYPOTHESIS. These structs are transient (consumed by `append`, then
  dropped), so this is allocation *churn*, not resident growth — but at the ingest rates Photon
  targets it is a real allocator/CPU tax, and `append` also pays `promoted.len()` `BTreeMap::get`
  (O(log n)) lookups + a full attribute iteration with `HashSet` probes per row
  (`record.rs:108-117`).
- **Fix (incremental):** (a) intern attribute keys as `Arc<str>` (a per-batch or per-connection
  key dictionary) so keys aren't re-allocated per record; or (b) skip the intermediate map
  entirely and map OTLP KV pairs straight into the column builders in `photon-ingest`. Option
  (b) is the larger structural win and removes the double iteration in `append`.
- **Effort/Risk:** M/M (touches `photon-ingest` mapping). Type change is confined to photon-core
  but callers ripple.
- **Invariant check:** Routing (promoted vs map) and `(service.name, timestamp)` sort unchanged.
  BTreeMap's sorted iteration currently yields sorted map keys "for free"; if you switch
  containers, keep the map declared unsorted (already the case) so nothing depends on order.

### 4. [P2 · speed] Manifest prune is an O(N) linear scan that allocates a `Vec` per call
- **Where:** `manifest.rs:59-64` (`Manifest::candidates`)
- **What:** `candidates(start,end)` filters the whole `entries: Vec<FileEntry>` linearly and
  collects `Vec<&FileEntry>`. Callers: `photon-query` `prune` (logs), `metric_engine::prune`,
  `span_engine::span_prune`/`trace_candidates` — i.e. once per aggregation per query, per signal
  (see `lib.rs:291`, `metric_engine.rs:170`, `span_engine.rs:143/246`). The manifest itself is
  cached by `(len, mtime)` (`photon-query/lib.rs:144`), so JSON parse is not per-query — good —
  but the scan is.
- **Why it matters:** MEASURED (call graph). Bounded by the merge pass today, but on the "HUGE
  data" path (long retention × many segments before consolidation) N grows and every query pays
  O(N) before the (dominant) per-candidate `.idx` reads even begin. The extra `Vec<&FileEntry>`
  allocation per call is minor but avoidable.
- **Fix:** Keep `entries` sorted by `max_ts_nanos` (they are appended in roughly time order
  already) and binary-search the lower bound — files with `max_ts < start` can be skipped
  wholesale; then linear-scan only the tail, early-exiting once `min_ts > end` isn't guaranteed
  so keep the upper filter. Alternatively return `impl Iterator<Item=&FileEntry>` to drop the
  Vec allocation. A full interval tree is overkill until N is large.
- **Effort/Risk:** S/M — `set_entries` (merge pass) must maintain the sort invariant; `add`
  (single append) must insert in order or the sort must be re-established. Contained to
  photon-core + the compactor's manifest updates.
- **Invariant check:** Pruning stays conservative (a time-overlap test, unchanged); this only
  changes *how fast* the same candidate set is produced. No false-negative risk.

### 5. [P2 · correctness] `parse_compare_value` accepts non-finite compare literals (NaN/Inf) and can overflow to Inf on unit scaling
- **Where:** `parser.rs:175-185` (`parse_compare_value`)
- **What:** The value is parsed with unchecked `rest.parse::<f64>()`. Rust's `f64` parser
  accepts `"nan"`, `"inf"`, `"infinity"` (case-insensitive), so `duration>=inf`, `x>=nan`,
  `foo<NaN` all parse *successfully* into a `Compare { value: NAN|INF }`. Unit scaling can also
  overflow: `duration>=1e308ms` → `1e308 * 1e6` = `+inf`.
- **Why it matters:** MEASURED (Rust parse semantics) + HYPOTHESIS (divergence). Two problems:
  (1) a query the user expects to be rejected instead "succeeds" and silently matches nothing
  (or, negated, everything); (2) **backend divergence** — the eval oracle uses Rust `PartialOrd`
  where every `x <op> NaN` is `false`, while the DataFusion side (`predicate.rs:76-85`,
  `casted.gt/gt_eq/...` against `lit(NaN)`) goes through Arrow's comparison kernels whose
  NaN/total-order handling need not match Rust. Since eval is the consistency oracle and no test
  exercises non-finite literals, a real divergence here would pass CI.
- **Fix:** After parsing/scaling, reject non-finite: `.filter(|n| n.is_finite())` (return `None`
  → the existing `expected a number …` `ParseError` with offset). One line per return path.
- **Effort/Risk:** S/S. No language change — `nan`/`inf` were never intended values.
- **Invariant check:** Keeps grammar semantics (still `field<op>number`), just closes a hole;
  parse errors still carry the byte offset (invariant #2).

### 6. [P2/P3 · correctness] Compare coerces the field *value* two different ways across backends
- **Where:** `eval.rs:39-41` / `span_eval.rs:43-45` / `metric_eval.rs:24-26`
  (`v.parse::<f64>().ok()`) vs `photon-query/predicate.rs:76-85` (`TryCast` Utf8→Float64)
- **What:** For a Compare on a string-typed field, the oracle uses Rust `str::parse::<f64>`
  while DataFusion uses Arrow's `TryCast`. These parsers agree on clean integers/decimals (all
  the consistency test covers) but can disagree on edge strings: leading/trailing whitespace,
  `"+5"`, scientific notation, `"inf"`/`"nan"` stored as an attribute value, hex, etc.
- **Why it matters:** HYPOTHESIS, NEEDS-BENCH. Where they disagree, in-memory semantics
  (documented/tested) and production results diverge for numeric-looking attribute values.
  Likelihood is low (real numeric attributes are usually clean integers), hence P2/P3.
- **Fix:** Don't hand-write a fix from guesswork — **fuzz it**: property-test the two backends
  over a corpus of messy numeric strings (whitespace, signs, exponents, non-finite, unicode
  digits) and pin the intended coercion, then align whichever side is "wrong". Note the span
  `Duration` path already avoids this by comparing the real `i64` (`span_eval.rs:40-42`) — good.
- **Effort/Risk:** S to characterize, S–M to align.
- **Invariant check:** This *is* invariant #2 (single source of truth for filter semantics);
  the finding is that the two compiled forms can drift on untested inputs.

### 7. [P3 · correctness/config] Missing lower-bound validation on WAL knobs and session secret
- **Where:** `config.rs:121-173` (`Config::validate`)
- **What:** `WalConfig.segment_max_bytes`, `segment_max_age_secs`, and
  `group_commit_max_delay_ms` are unvalidated — `0` is accepted. `group_commit_max_delay_ms = 0`
  defeats group-commit batching (approaching an fsync per append → throughput collapse);
  `segment_max_bytes = 0` could close a segment per record. Also `session_secret`'s doc comment
  (`config.rs:79-80`) claims it must be "long enough to be secure" but `validate` only checks
  non-empty (`:167`).
- **Why it matters:** MEASURED (code). A silent misconfig can quietly wreck ingest throughput —
  the opposite of the project's goal — with no error at startup.
- **Fix:** Add guards: `segment_max_bytes >= <sane floor>`, `group_commit_max_delay_ms` within a
  sane range (or explicitly allow 0 with a documented meaning), and enforce a minimum
  `session_secret` length to match the doc.
- **Effort/Risk:** S/S. Contained to `validate`.
- **Invariant check:** None affected.

---

## Quick wins
- **#5** reject non-finite compare literals — one-line-per-branch, closes a real correctness +
  divergence hole. Do this first.
- **#4** return an iterator from `candidates()` (drop the per-call `Vec` alloc) even before the
  sorted/binary-search version.
- **#7** add the three WAL validation guards + session-secret length.
- Dictionary-encode just `service.name` and the `severity_text`/`kind_text`/`status_text`
  columns (subset of #1) — highest cardinality-payoff, smallest surface.

## Bigger bets (architectural)
- **#1 full dictionary-encoding pass** across all low-cardinality Utf8 columns (schema + builders
  + compaction write/read + predicates).
- **#2 dictionary-encoded attributes map** — largest single memory reduction for
  attribute-heavy tenants.
- **#3 kill the intermediate `BTreeMap<String,String>`** by mapping OTLP straight into column
  builders (interned keys), removing per-record allocation churn and the double-iteration in
  `append`.
- **#4 manifest as a time-indexed structure** (sorted-by-`max_ts` + binary search, escalating to
  an interval index only if measured file counts justify it).

## Already good / no action
- **Two-pass late materialization** is real and documented (`photon-query/lib.rs:233-282`); the
  wide attributes map is decoded only for surviving rows. Don't collapse it.
- **Parser robustness:** fully **iterative** — no recursion, so no stack-overflow risk on
  pathological/deeply-nested input; per-token work is linear (`tokenize` O(n), `classify`
  O(token) with a bounded number of `find`/`split_once` scans) — **no quadratic blowup**; and
  it never panics on malformed input (no `unwrap`/`expect` on the parse path — every failure is
  a `ParseError`). Parse runs once per query, not per row, so its allocations are irrelevant to
  throughput.
- **Byte offsets are UTF-8-correct:** `tokenize` uses `char_indices()` and all `ParseError`
  offsets are those byte indices at char boundaries (invariant #2 satisfied). Offset points at
  the token start, which is the intended "underline the bad token" granularity.
- **Three-valued-logic parity trick:** `predicate.rs` uses `IS [NOT] TRUE` (not a `CASE`) to map
  SQL NULL to the in-memory `base ^ negated`, with a well-documented reason (DataFusion 43's
  `SimplifyExpressions` mis-rewrites the `CASE` form). This is the crux of invariant #2 and it's
  handled carefully.
- **Bloom-safety of free-text pruning:** `positive_freetext` (`eval.rs:18-27`,
  `span_eval.rs:18-27`) deliberately excludes negated terms, so a `-word` term can never skip a
  file — preserves the never-false-negative invariant (#1).
- **Manifest is cached by `(len, mtime)`** so JSON parse is not per-query; torn concurrent
  writes self-heal (`photon-query/lib.rs:144-185`).
- **Attribute routing** uses a `HashSet` (`promoted_set`) for O(1) promoted lookup, and both
  builders offer `with_capacity` presizing to avoid geometric reallocation
  (`record.rs:64-93`).
- **SegmentId** is a `u64` with hex-zero-padded filenames (`seg-{:016x}.wal`) so lexicographic
  directory order equals numeric order; `parse_filename` round-trips and rejects junk. Monotonic
  `next()`. (Nit: `self.0 + 1` would debug-panic at `u64::MAX` — astronomically unreachable.)
- **`default_agg`** is a total, well-tested pure function with a safe fallback for unknown types.
- **PhotonError** shape is fine as-is (one variant per crate domain) — do not split (invariant #3).

## Open questions & NEEDS-BENCH
- **NEEDS-BENCH (#1/#2):** memory + query-latency delta from dictionary-encoding the string
  columns and the map, on a high-cardinality-attribute + low-cardinality-service dataset. This
  is the number that decides whether these are P1-worth-doing-now or P2.
- **NEEDS-BENCH (#4):** actual `FileEntry` count in a steady-state high-volume deployment
  (post-merge). If it stays in the low thousands, the linear scan is fine and the sorted/binary
  version is premature; if it reaches tens of thousands, do it.
- **NEEDS-BENCH (#6):** fuzz `str::parse::<f64>` vs Arrow `TryCast(Utf8→Float64)` over messy
  numeric strings to find (and then pin) any real divergence.
- **Open (#3):** is there ever a case where OTLP delivers a batch large enough that transient
  `LogRecord`/`SpanRecord` allocation churn shows up in ingest-side flamegraphs? Confirm before
  investing in the OTLP-straight-to-builder rewrite.
- **Minor:** `Manifest.entries` `FileEntry.attribute_keys: Vec<String>` grows the in-memory
  manifest with the per-segment long-tail key vocabulary — bounded in practice, but worth an eye
  on manifest size for tenants with exploding attribute-key cardinality.
