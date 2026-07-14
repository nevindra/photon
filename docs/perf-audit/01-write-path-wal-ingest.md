# Write Path (WAL + Ingest) — Perf & Correctness Audit
**Scope:** photon-wal, photon-ingest  ·  **Date:** 2026-07-06

Read-only audit of the hot write path: OTLP request → bearer-token check → protobuf decode →
map to LogRecord/SpanRecord/MetricPoint → append to WAL → group-commit fsync (ack boundary).
No source was modified. All findings cite `file:line`; MEASURED facts and HYPOTHESES are tagged,
and anything requiring a microbench is tagged `NEEDS-BENCH`.

## TL;DR — biggest 10x levers

1. **Kill the per-record `BTreeMap` + intermediate `Vec<Record>` — stream OTLP → Arrow builder
   directly.** Every attribute is routed protobuf-String → a fresh `BTreeMap<String,String>` node
   (one heap alloc per entry, per record) → then copied again into the Arrow buffer. Resource
   attributes are additionally cloned once *per record*. This is the dominant allocator-pressure
   source on ingest and is an ~order-of-magnitude reduction in allocation *count* if removed.
   (F1)
2. **All 6 ingest handlers build the Arrow batch with `Builder::new()` (zero capacity) even though
   the exact row count is known.** A `with_capacity(schema, rows)` constructor already exists on
   all three builders and is documented as the way to avoid geometric reallocation — it is simply
   never called from ingest. One-line fix per handler. (F2)
3. **The WAL writer uses `tokio::fs::File`, paying a `spawn_blocking` round-trip for *each*
   `write_all` and *each* `sync_data`, and memcpy-concatenates coalesced frames into a throwaway
   buffer every round.** A dedicated OS writer thread over `std::fs::File` + vectored (`writev`)
   writes removes both. (F6, F7) `NEEDS-BENCH`

Also two real correctness/durability items: a mid-segment torn-frame data-loss window on a partial
write (F3), and the OTLP HTTP endpoints inheriting axum's default 2 MB body cap — too small for
real batches yet unbounded if raised (F4).

---

## Findings (ranked)

### F1 — Per-record `BTreeMap` + intermediate `Vec<Record>` dominate ingest allocation churn
- **Severity:** P1 · **Category:** memory + speed
- **Where:**
  `crates/photon-ingest/src/mapping.rs:20-88` (`otlp_logs_to_records`),
  `crates/photon-ingest/src/trace_mapping.rs:28-110` (`otlp_traces_to_spans`),
  `crates/photon-ingest/src/metrics_mapping.rs:19-134` (`otlp_metrics_to_points` / `merge_attrs`),
  consumed at `grpc.rs:35-40`, `http.rs:56-61`, `grpc_trace.rs:35-40`, `trace_http.rs:57-61`,
  `grpc_metrics.rs:36-41`, `metrics_http.rs:59-63`.
- **What:** Mapping builds an owned `Vec<LogRecord>` (each record owns a
  `BTreeMap<String,String>` of attributes), then the handler loops `for record in &records {
  builder.append(record) }`, which copies every key/value *again* into the Arrow buffers. So each
  attribute costs: (a) one `BTreeMap` node heap-allocation at map-build time, (b) a second byte
  copy into the Arrow `StringBuilder`. `BTreeMap` is the worst container choice here — every
  `insert` is a separate red-black-tree node allocation, and the downstream only needs
  `.get(promoted_name)` (a handful of keys) plus a full iteration. Peak memory holds the entire
  `Vec<Record>` *and* the Arrow buffers simultaneously (~2× the string payload) before the Vec is
  dropped at end of handler scope.
- **Why it matters:** At high ingest volume this is the single largest source of allocator traffic
  and cache misses on the write path — N records × M attributes × (node alloc + copy). It also
  doubles transient RSS per in-flight batch, working directly against the "low memory" goal.
- **Fix:** Two tiers.
  - *Cheap (mechanical):* replace `BTreeMap<String,String>` in the `*Record` structs with a sorted
    `Vec<(String,String)>` (or `Box<[(String,String)]>`); promoted lookup becomes a linear/binary
    scan over a handful of keys. Removes the per-entry node allocs. (Touches photon-core, just
    below this scope.)
  - *Best (structural):* make the mapper stream straight into the batch builder — feed
    resource-attr + record-attr *iterators* into `builder.append(...)` and never materialize an
    owned `Vec<Record>` or a per-record map at all. Eliminates the intermediate Vec, the per-record
    `BTreeMap`, and the resource-attr N-clone in one move. Keep a thin pure wrapper returning
    `Vec<Record>` for the existing unit tests.
- **Effort/Risk:** Cheap tier S/low-risk; structural tier M/medium (reshapes the pure mapping API
  that tests depend on). `NEEDS-BENCH` to quantify, but the alloc-count reduction is structural,
  not speculative.
- **Invariant check:** Pure mapping semantics unchanged (same flattening, same `(service.name,
  timestamp)` sortability downstream). No inverted index, no durable-path change. ✔

### F2 — Ingest builds Arrow batches at zero capacity despite knowing the row count
- **Severity:** P1 · **Category:** speed (memory secondary)
- **Where:** `grpc.rs:36`, `http.rs:57`, `grpc_trace.rs:36`, `trace_http.rs:58`,
  `grpc_metrics.rs:37`, `metrics_http.rs:60` — all call `RecordBatchBuilder::new(&schema)` /
  `SpanBatchBuilder::new(...)` / `MetricBatchBuilder::new(...)`.
- **What:** Each builder starts every Arrow column at capacity 0 and grows by geometric doubling as
  rows are appended. Yet the mappers return a `Vec` whose `.len()` is exactly the row count, and a
  `with_capacity(schema, rows)` constructor already exists and is *documented* for exactly this
  purpose (`photon-core/src/record.rs:64-93`, `span_record.rs:93`, `metric_record.rs:67`). It is
  simply never called from the ingest path. (MEASURED: the constructor exists and pre-sizes every
  column incl. the attributes map; ingest calls the zero-cap `new`.)
- **Why it matters:** For large batches (the whole point of high-volume ingest), the doubling path
  reallocates+memcpies each column O(log rows) times — wasted CPU and transient 2× buffer spikes
  during each grow. Free to avoid.
- **Fix:** `let n = records.len();` then `RecordBatchBuilder::with_capacity(&state.schema, n)` (and
  equivalents) in all six handlers.
- **Effort/Risk:** S / negligible.
- **Invariant check:** Pure sizing hint, no semantic change. ✔

### F3 — Partial write leaves a mid-segment torn frame → later *acked-durable* frames lost on recovery
- **Severity:** P2 · **Category:** correctness / durability
- **Where:** `crates/photon-wal/src/disk.rs:351-393` (`Writer::commit`) + recovery at
  `disk.rs:296-310` / `frame.rs:67-106` (`scan_segment` stops at the first torn frame).
- **What:** On a `write_all`/`sync_data` error, every ack in the round correctly gets `Err` (no
  false ack). But the writer task does **not** shut down and does **not** roll the file back: any
  partially-written bytes remain at the current file offset, `self.size` is not advanced, and the
  next successful round `write_all`s *after* those bytes. If a subsequent round then succeeds and is
  acked `Ok`, a later crash + recovery runs `scan_segment`, which stops at the earlier torn/partial
  frame and truncates everything after it — **discarding the acked-durable round**. Requires a write
  that partially succeeds then errors (e.g. transient `ENOSPC`/`EIO` mid-frame) followed by a
  recovering write. (HYPOTHESIS — narrow, I/O-error-triggered, not reproduced.)
- **Why it matters:** Violates the core WAL contract (ack ⇒ survives a crash). Rare, but it is a
  silent data-loss path exactly when the disk is already misbehaving.
- **Fix:** On any commit error, before accepting the next round, restore the last known-good state:
  `self.file.set_len(self.size)` + seek to `self.size` (drop the partial tail), or treat the error
  as fatal and rotate to a fresh segment. Simplest: truncate back to `self.size` so the file never
  carries un-acked partial bytes ahead of good ones.
- **Effort/Risk:** S / low (touches only the error arm).
- **Invariant check:** Strengthens the ack=durable boundary; local-disk-primary unchanged. ✔

### F4 — OTLP HTTP endpoints inherit axum's default 2 MB body cap (too small; unbounded if raised)
- **Severity:** P2 · **Category:** correctness / throughput (memory if misconfigured)
- **Where:** router construction in `http.rs:33-37`, `trace_http.rs:34-38`, `metrics_http.rs:36-40`
  and `lib.rs:124-130` — no `DefaultBodyLimit` / `RequestBodyLimitLayer` anywhere (grep: zero hits).
  Body is taken as `Bytes` (`http.rs:43`, etc.), which buffers the whole request before decode.
- **What:** With no explicit limit, axum 0.7's `Bytes` extractor applies its **default 2 MB** cap,
  so a legitimate high-volume OTLP batch >2 MB is rejected with 413 — silent ingest loss under the
  exact load this system targets. Conversely, raising the limit without a tuned ceiling makes the
  body (plus the prost-decoded copy) an unbounded per-request memory spike / OOM lever. The gRPC
  side is separately capped at tonic's 4 MB default `max_decoding_message_size` (also unset), so the
  two front ends disagree. (HYPOTHESIS on the exact 2 MB default — `NEEDS-VERIFY` against axum 0.7,
  but the "no explicit limit" fact is MEASURED.)
- **Why it matters:** Either large batches fail (throughput/data loss) or memory is unbounded —
  both bad for a "huge volume, low memory" ingester, and the HTTP/gRPC ceilings are inconsistent.
- **Fix:** Set an explicit, config-driven `DefaultBodyLimit::max(N)` on the ingest router and match
  it to tonic's `.max_decoding_message_size(N)` on each gRPC service, sized for expected OTLP
  batches (e.g. 8–32 MB).
- **Effort/Risk:** S / low.
- **Invariant check:** No durability/index impact; purely a front-door bound. ✔

### F5 — Bearer-token comparison is not constant-time
- **Severity:** P2 · **Category:** security
- **Where:** `crates/photon-ingest/src/auth.rs:8-13` (`check_bearer_token`) — `t == token`.
- **What:** `str == str` short-circuits on the first differing byte (and on length), leaking a
  timing signal correlated with the shared ingest secret. Called on every gRPC (`grpc*.rs:25-33`)
  and HTTP (`*_http.rs:44-52`) request.
- **Why it matters:** A network timing side channel can, in principle, recover the ingest bearer
  token byte-by-byte. The token is the sole ingest authN secret; the fix is trivial.
- **Fix:** Compare with a constant-time primitive (`subtle::ConstantTimeEq`, or
  `ring::constant_time::verify_slices_are_equal`) after stripping the `Bearer ` prefix. Compare full
  fixed-length buffers so length isn't leaked either.
- **Effort/Risk:** S / low (adds one small dep or reuses one already in the tree).
- **Invariant check:** Auth-only; ingest-token vs session-cookie systems stay separate. ✔

### F6 — WAL writer uses `tokio::fs::File`: two `spawn_blocking` hops per commit round
- **Severity:** P1 · **Category:** speed · `NEEDS-BENCH`
- **Where:** `crates/photon-wal/src/disk.rs:351-374` (`commit`: `self.file.write_all(...).await`
  then `self.file.sync_data().await`), file type `tokio::fs::File` (`disk.rs:339`), single writer
  task spawned at `disk.rs:158`.
- **What:** `tokio::fs::File` wraps a `std::fs::File` behind an internal mutex and dispatches every
  operation to the blocking thread pool. So each commit round pays a `spawn_blocking` round-trip for
  the `write_all` **and** another for the `sync_data` — two thread hand-offs and two synchronization
  points per fsync, plus `tokio::fs`'s read/write buffering state machine that a pure append+fsync
  WAL doesn't need. (Correct that it does *not* stall the runtime — the fsync is off-thread — but
  the per-round overhead is real.)
- **Why it matters:** The writer is the serialization point for all ingest durability; every µs of
  per-round overhead throttles max sustained fsync/commit rate. Under group commit this is per-round
  (amortized over the batch), so impact scales with how small/frequent the batches are.
- **Fix:** Run the writer on a dedicated OS thread (`std::thread`) owning a plain `std::fs::File`,
  fed by a `std::sync::mpsc` (or `tokio::sync::mpsc` drained in a loop). Do synchronous
  `write`+`sync_data` there — no `spawn_blocking`, no async-fs buffering. Keep the ack via `oneshot`.
- **Effort/Risk:** M / medium (rewrites the writer plumbing; keep the group-commit batching logic).
- **Invariant check:** Ack-after-fsync boundary preserved; local disk primary. ✔

### F7 — Coalesced frames are memcpy-concatenated into a fresh throwaway buffer each round
- **Severity:** P2 · **Category:** speed + memory
- **Where:** `crates/photon-wal/src/disk.rs:356-369` (`commit`, the `frames => { Vec::with_capacity;
  extend_from_slice }` arm).
- **What:** When ≥2 appends coalesce (the common case under load — the whole point of group commit),
  every frame is copied into a freshly-allocated `buf`, written once, then `buf` is dropped. So a
  busy round does a full extra memcpy of all its bytes plus an allocation, every round. The single-
  frame fast path (`[only]`) correctly avoids this.
- **Why it matters:** Under high load (many coalesced frames/round) this extra copy + alloc runs on
  the busiest path, on the single writer, sized to the whole round's bytes.
- **Fix:** Use vectored I/O — collect `IoSlice`s over the frames and `write_all_vectored` (natural
  once F6 moves to `std::fs`), eliminating the concat. If staying on `tokio::fs`, at least reuse a
  persistent scratch buffer on `Writer` instead of allocating per round.
- **Effort/Risk:** S–M / low (pairs naturally with F6).
- **Invariant check:** Same bytes, same order, same single fsync. ✔

### F8 — `otlp_metrics_to_points` doesn't pre-size its output Vec and clones resource attrs per point
- **Severity:** P2 · **Category:** memory + speed
- **Where:** `crates/photon-ingest/src/metrics_mapping.rs:20` (`Vec::new()`), `:122-134`
  (`merge_attrs` does `base.clone()` for *every* data point).
- **What:** Unlike logs/traces — which pre-size the output `Vec` (`mapping.rs:21-27`,
  `trace_mapping.rs:29-35`) and move the resource map into the *last* record — metrics starts from
  `Vec::new()` (geometric regrow) and `merge_attrs` unconditionally clones the full resource
  attribute `BTreeMap` for every point. High-cardinality metric batches (many points sharing
  resource attrs) pay N full `BTreeMap` clones + regrows.
- **Why it matters:** Metrics is the worst of the three mappers for allocation churn precisely where
  data-point fan-out is highest.
- **Fix:** Pre-count total points into `Vec::with_capacity`, and apply the logs/traces "last group
  member takes the map by `mem::take`, earlier ones clone" pattern (or fold into the F1 streaming
  refactor).
- **Effort/Risk:** S / low.
- **Invariant check:** Same mapping output. ✔

### F9 — `HistogramJson` clones `bucket_counts` / `explicit_bounds` although the data point is owned
- **Severity:** P3 · **Category:** memory
- **Where:** `crates/photon-ingest/src/metrics_mapping.rs:183-184`
  (`bucket_counts: dp.bucket_counts.clone()`, `explicit_bounds: dp.explicit_bounds.clone()`).
- **What:** `dp` is owned (moved in), so these `Vec<u64>`/`Vec<f64>` can be moved into the JSON
  payload instead of cloned-then-dropped. The other fields read are `Copy`.
- **Why it matters:** Two extra Vec allocations+copies per histogram point, then immediately freed.
- **Fix:** Move the Vecs (reorder field reads so the moves come last), or destructure `dp`.
- **Effort/Risk:** S / negligible.
- **Invariant check:** None affected. ✔

### F10 — Redundant per-record `*_text` String allocations derived from the numeric enum
- **Severity:** P3 · **Category:** memory
- **Where:** `trace_mapping.rs:93` (`kind_text`), `:98` (`status_text`);
  `metrics_mapping.rs:158,191,231,265` (`type_text(...).to_string()`); helper fns return
  `Some("CLIENT".into())` etc.
- **What:** For every span/point a fresh `String` is allocated from a tiny fixed set of `&'static
  str`, and it duplicates information already stored in the adjacent numeric `kind`/`status_code`/
  `metric_type` column. The text is a pure function of the code.
- **Why it matters:** One small heap alloc per record for data that is redundant, and it inflates
  WAL frame + downstream Parquet size (the text column compresses but still costs bytes and a decode
  column).
- **Fix:** Prefer deriving the display text at query/UI time from the numeric code and dropping the
  `*_text` columns; or, if kept, have the builder map code→`&'static str` without a per-row `String`
  (requires a downstream schema tweak). Lower priority — verify UI dependence first.
- **Effort/Risk:** M / low-value unless combined with a schema pass.
- **Invariant check:** No durability/index impact. ✔

### F11 — Untrusted OTLP timestamps: `as i64` + subtraction can wrap (debug-panic / wrong duration)
- **Severity:** P3 · **Category:** correctness (robustness against hostile input)
- **Where:** `crates/photon-ingest/src/trace_mapping.rs:70-76` (`start`/`end` = `u64 as i64`,
  `duration = end.map(|e| (e - start).max(0))`); same `u64 as i64` casts in `mapping.rs:73` and
  the metrics mapper timestamps.
- **What:** OTLP nanos are `u64`; values ≥ 2^63 cast to negative `i64`, and `e - start` can overflow
  `i64` for crafted inputs (e.g. `start=2^63`, `end=2^63-1`). Release builds wrap silently (wrong
  duration); debug builds (our crates run with debug-assertions on) would panic. No crash in the
  shipped release binary, but it's a latent data-integrity/robustness gap on adversarial input.
- **Why it matters:** A malicious/buggy exporter can inject nonsense durations; the debug-panic also
  bites developers/tests. Cheap to harden.
- **Fix:** Use `i64::try_from(...).unwrap_or(0)` or clamp, and `end.saturating_sub(start).max(0)` /
  `checked_sub`.
- **Effort/Risk:** S / low.
- **Invariant check:** None affected. ✔

---

## Quick wins (low-effort, high-impact)
- **F2** — pass `records.len()` to `with_capacity` in all six handlers (biggest effort:impact ratio).
- **F5** — constant-time token compare.
- **F8** — pre-size the metrics Vec + stop cloning resource attrs per point.
- **F9** — move (don't clone) histogram Vecs.
- **F3** — truncate back to `self.size` on commit error (small, closes a durability hole).
- **F7 (partial)** — reuse a scratch buffer on `Writer` if not doing full F6.

## Bigger bets (architectural)
- **F1** — stream OTLP → Arrow builder, deleting the intermediate `Vec<Record>` and per-record
  `BTreeMap`. The single largest allocator + memory win on the write path.
- **F6 + F7** — dedicated `std::fs` writer thread with vectored `writev` in place of `tokio::fs`.
  `NEEDS-BENCH` to size the win, but removes two `spawn_blocking` hops and a per-round memcpy.
- **F4** — unify and make configurable the HTTP body limit and gRPC max-message size.

## Already good / no action
- **Group commit is genuinely correct.** One writer task, coalesce-window (`delay_ms`, default 5 ms)
  or non-blocking `try_recv` drain when 0, one `write_all` + one `sync_data` per round, acks resolve
  only after the fsync — the ack-before-durability boundary holds (`disk.rs:351-393`, `424-459`).
  Concurrent appenders are batched, not serialized, and the channel (cap 1024, `disk.rs:53`) gives
  real backpressure instead of unbounded heap growth.
- **Frame format is robust against torn/corrupt tails:** length + crc32 header, self-contained IPC
  stream per frame, bounds checks + `checked_add` before every slice, crc verified before decode,
  scan stops at the first bad frame (`frame.rs:67-106`). CRC is computed once on write.
- **No lock held across an `.await`** in the WAL: `Shared::inner` (std `Mutex`) is taken only for
  tiny `BTreeSet` ops and always dropped before any await (`disk.rs:396-420`, `199-227`).
- **fsync is off the runtime**, not blocking a worker (`tokio::fs` → blocking pool). Recovery reads
  and closed-segment reads correctly use `tokio::fs::read` for the same reason (`disk.rs:204-213`).
- **Logs/traces mappers are already lean:** output Vec pre-sized, protobuf strings *moved* (not
  cloned) via `any_value_into_string`, the resource-attr map `mem::take`-moved into the last record
  of a group, and a hand-rolled hex encoder avoids a `format!` per id byte (`otlp_value.rs:34-45`).
- **Malformed protobuf is rejected before the WAL is touched**, on both HTTP (`http.rs:51-54`) and
  via prost decode limits on gRPC; decode errors return 400, not a panic. No `unwrap`/indexing on
  untrusted request fields in the mappers.
- **Schema-drift guard** rejects a mismatched batch before it can enter the log
  (`disk.rs:170-176`).

## Open questions & NEEDS-BENCH
- **F1 / F6+F7:** microbench end-to-end ingest (loadgen `--saturate`) before/after — measure
  allocations (heaptrack/dhat), sustained records/s, and RSS. Expect the largest movement from F1
  (alloc count) and F6 (writer overhead). `NEEDS-BENCH`
- **F4:** confirm axum 0.7's exact default body limit (believed 2 MB) and whether any large real
  OTLP batch already trips it in practice. `NEEDS-VERIFY`
- **F3:** can `write_all` on `tokio::fs::File` actually leave a *partial frame* (vs. all-or-nothing)
  on a mid-stream error? Confirm the torn-tail-in-middle scenario is reachable on the target FS.
- **Group-commit window default (5 ms):** adds up to 5 ms latency per request at low load in
  exchange for batch size at high load — confirm this matches the intended ingest SLO, or make it
  adaptive (commit immediately when the queue is short).
