# Memory Profile — Where the ~6–7 GiB Lives

**Commit:** 2257d54 (+ uncommitted Phase-A/B1 benchmark harness in the working tree)
**Date:** 2026-07-06  ·  **Host:** AMD Ryzen 7 9700X, 16 logical cores, tmpfs (`/dev/shm`) backing
**Reproduce:** `SEGMENT_MAX_BYTES=… CONCURRENCY=… BATCH=… RATE=… LABEL=… ./scripts/mem-profile.sh`
**RSS note:** tmpfs data files live in shared memory, **not** in the process's `VmRSS`/`VmHWM`, so
these numbers are the server's real heap/anon working set — not the WAL/Parquet bytes on `/dev/shm`.
**Caveat:** glibc malloc may retain freed pages, so "post-idle stays near peak" is ambiguous
(retained OR allocator-held); "post-idle drops a lot" is unambiguously transient.

## Segment-size sweep — the dominant lever (conc=32, batch=500, saturate, 40s)

| segment_max_bytes | peak MiB | steady MiB | post-idle MiB |
|---|---|---|---|
| 16 MiB  | **1,287** | 1,181 | 1,253 |
| 64 MiB  | **4,149** | 3,959 | 4,031 |
| 128 MiB | **7,236** | 5,429 | 7,015 |

**Peak RSS is ~linear in segment size** — slope ≈ (7236 − 1287) / (128 − 16) ≈ **53 MiB of RSS per
MiB of `segment_max_bytes`**; extrapolating to segment→0 leaves a fixed floor of only **~0.4–0.9 GiB**.
This confirms doc-04's hypothesis directly: the bulk of the footprint is **compaction / WAL-segment
buffering** (the whole closed segment decoded into an Arrow `RecordBatch`, the ~3× copy through
sort→Parquet, and the whole-file Parquet write buffer — multiplied by however many segments/compactions
are in flight). **Shrinking the segment 128 → 16 MiB cut peak RSS 82% (7.2 → 1.3 GiB)** with a config
knob and no code change.

## In-flight & load attribution (segment=128 MiB default)

| run | peak MiB | steady MiB | post-idle MiB |
|---|---|---|---|
| conc=8  batch=500  saturate 40s | 4,754 | 3,980 | 4,661 |
| conc=32 batch=500  saturate 40s | 7,236 | 5,429 | 7,015 |
| conc=32 batch=2000 saturate 40s | 5,100 | 3,993 | 4,888 |
| conc=128 batch=500 saturate 40s | **OOM — server died** (4,330 & still climbing at 15s) | 2,763 (15s) | — |
| rate=100k (unsaturated) 40s | 1,444 | 1,267 | 1,335 |

- **Concurrency is the second lever:** conc=8 → 4.75 GiB vs conc=32 → 7.24 GiB at the same segment
  size (+2.5 GiB from more in-flight batches + more parallel compaction).
- **conc=128 saturate has no memory bound — it OOM-kills the server.** At 40s the process
  (RSS + tmpfs) exceeded physical RAM and died mid-run; a 15s window survived at 4.3 GiB **and still
  climbing** (steady 2.76 → peak 4.33 in 15s). There is **no ingest/compaction backpressure**: offered
  unbounded load, memory grows without limit. This is a correctness risk, not just a footprint number.
- **Batch size is not a driver of peak** (batch 500 → 7.24 GiB vs batch 2000 → 5.10 GiB at conc=32):
  larger batches mean fewer requests and a slightly smaller resident footprint, not larger.
- **The footprint is load-dependent, not fixed:** at a realistic **100k logs/s** (≈4% of the ceiling)
  peak RSS is only **1.44 GiB** even with the 128 MiB segment — the 6–7 GiB only appears under full
  saturation, i.e. it is compaction/ingest **backlog**, not a fixed baseline.
- **Transient vs retained:** post-idle sits within ~3% of peak in every run (e.g. seg128: 7,015 vs
  7,236). Since the working memory is provably transient (segment-scaled, load-scaled, released when
  the compaction finishes), the fact that RSS does **not** fall during 30s idle means glibc is
  **holding freed pages in its arenas rather than returning them to the OS** — not a true leak. An
  allocator change (jemalloc/mimalloc) or `MALLOC_ARENA_MAX` / periodic `malloc_trim` would reclaim it.

## Breakdown & verdict

Best estimate of the **7.24 GiB peak** at the baseline-like operating point (conc=32, seg=128 MiB,
saturated):

| component | share | evidence |
|---|---|---|
| Compaction / WAL-segment buffering (segment-scaled) | **~5.5–6 GiB** | segment sweep: 53 MiB RSS per MiB segment; 16→128 MiB adds 5.95 GiB |
| In-flight ingest + parallel-compaction (concurrency-scaled) | **~1–2.5 GiB** | conc 8→32 adds 2.5 GiB; present even at conc=8 |
| Fixed baseline (DataFusion, tokio, runtime, mmap) | **~0.4–1.0 GiB** | segment→0 extrapolation; low-rate floor |

- The ~6–7 GiB is **overwhelmingly transient compaction/WAL buffering that scales with
  `segment_max_bytes`**, plus a concurrency-scaled in-flight share — **not** a fixed baseline.
- It is **retained by the glibc allocator** after use (RSS doesn't drop when idle), so a real
  deployment sees the peak as its effective resident size until the process restarts.
- **Steady-state a real deployment would see:** at a sane sustained rate (~100k logs/s) with the
  default 128 MiB segment, ~**1.4 GiB**; the multi-GiB figures require saturation. But because there is
  **no backpressure**, a burst to saturation at high concurrency can OOM the node — so the footprint is
  effectively unbounded under adversarial load until Phase B2 adds a cap.

See `b2-targets.md` for the ordered fixes these findings imply.
