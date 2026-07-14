# Phase B2 — Ordered Optimization Targets (data-driven)

Derived from: `logs-baseline.md`, `concurrency-sweep.md`, `memory-profile.md`.
Host: AMD Ryzen 7 9700X, 16 cores, tmpfs backing. Commit 2257d54 (+ uncommitted harness).

## What the diagnosis established

- **Speed regime = CPU-bound.** Throughput scales ~linearly with cores (~250k logs/s/core) to a
  **true ceiling ≈ 2.35M logs/s at conc=64** — 45% above the concurrency-limited baseline. Per-record
  CPU is the lever; fsync is not (Phase A: tmpfs only ~13.5% faster).
- **Memory = transient compaction/WAL buffering, ~linear in `segment_max_bytes`** (53 MiB RSS per MiB
  of segment; 128 MiB → 7.2 GiB peak, 16 MiB → 1.3 GiB), plus a concurrency-scaled in-flight share,
  over a small (~0.5–1 GiB) fixed floor. Retained by glibc after use. **No ingest backpressure** —
  conc=128 saturate OOM-kills the server.

## Speed (raise the ~2.35M logs/s ceiling)

The ceiling is per-core CPU efficiency, so the two Phase-A allocation levers convert ~directly into
throughput. Ranked by expected share of the per-record CPU cost:

1. **F1 — eliminate per-record `BTreeMap` + `String` churn in `otlp_logs_to_records`.** The alloc guard
   measured **37.2 allocations/record**, dominated by per-record attribute-map construction. This is
   the dominant CPU cost in `decode_map_build` (459–702 Kelem/s vs 1.8–4.8 Melem/s for map or build
   alone). Expected: the largest single throughput gain; targets the ~10x alloc reduction the guard is
   set up to prove.
2. **F2 — `RecordBatchBuilder::with_capacity`.** Pre-size the Arrow builders to the batch row count so
   they stop reallocating/growing. Cheaper and more isolated than F1; do it first as the warm-up.
3. *(secondary)* **Investigate the serialization point** that caps the server at ~11.4/16 cores and makes
   throughput roll off past conc=64 (WAL group-commit is the prime suspect). Lower priority — we are
   still CPU-scaling in the useful range and there is no hard lock ceiling. Revisit after F1/F2, once
   per-record CPU no longer dominates.

## Memory (recover the 6–7 GiB peak; make it bounded)

1. **Bound in-flight ingest + concurrent compactions (backpressure).** *Correctness first:* conc=128
   saturate OOMs the server today (memory grows without limit). A cap on concurrent compactions and an
   in-flight-bytes limit on ingest turns an unbounded footprint into a bounded one. Evidence: c128 OOM;
   conc 8→32 adds 2.5 GiB.
2. **Stream compaction / drop-early (doc-04's fix).** The segment-scaled bulk (~5.5–6 GiB at 128 MiB)
   is the whole segment decoded + ~3× copy + whole-file Parquet buffer. Stream-encode the Parquet and
   free the input as it is consumed instead of holding the whole segment + copies. Recovers the largest
   share **without** shrinking segments (so it keeps compaction/merge efficiency). Evidence: 53 MiB
   RSS/MiB-segment slope.
3. **Config quick-win: drop `segment_max_bytes` 128 → 16–32 MiB.** Immediate 82% peak cut (7.2 → 1.3 GiB
   at 16 MiB) with zero code — usable *today* as a stopgap while (2) lands. Cost: more, smaller Parquet
   files → more `merge_once` work; measure the merge/query impact before making it the default. Evidence:
   segment sweep.
4. **Return freed memory to the OS.** RSS stays within ~3% of peak through 30s idle → glibc arenas hold
   the pages. Switch the global allocator to **jemalloc/mimalloc**, or set `MALLOC_ARENA_MAX` / call
   `malloc_trim` after compaction. Cheap; recovers the idle/steady footprint a deployment actually holds.
   Evidence: post-idle ≈ peak in every run.

## Ordered target list (highest impact first)

1. **Bound in-flight + concurrent compactions (backpressure)** — evidence: c128 OOM (`memory-profile.md`);
   effect: turns unbounded RSS into bounded; removes the OOM-under-burst correctness risk.
2. **F1: kill per-record BTreeMap/String churn** — evidence: 37.2 allocs/record + `decode_map_build`
   collapse (`logs-baseline.md`); effect: largest throughput gain, raises the 2.35M ceiling; ~10x fewer allocs.
3. **F2: `RecordBatchBuilder::with_capacity`** — evidence: `build` bench (`logs-baseline.md`); effect:
   removes builder-regrowth allocs; cheap warm-up to F1.
4. **Stream compaction / drop-early** — evidence: 53 MiB RSS/MiB-segment (`memory-profile.md`); effect:
   recovers ~5–6 GiB of segment-scaled peak without shrinking segments.
5. **Drop `segment_max_bytes` to 16–32 MiB (config stopgap)** — evidence: segment sweep; effect: 82% peak
   cut today; verify merge/query cost before defaulting.
6. **jemalloc/mimalloc or `malloc_trim`** — evidence: post-idle ≈ peak; effect: returns retained pages to
   the OS, lowering the resident footprint a deployment holds at steady state.
