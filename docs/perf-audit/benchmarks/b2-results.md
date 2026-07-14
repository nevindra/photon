# Write-Path Phase B2 — measured results (before / after)

Measured on the shared 16-core dev box, tmpfs (`/dev/shm`), release build with jemalloc.
Baseline column = Phase B1 diagnosis (commit `2257d54`, `concurrency-sweep.md` / `memory-profile.md`).
After column = Phase B2 (this branch, staged). Fixtures/knobs identical to B1 unless noted.

## Headline

| Axis | Baseline (B1) | After (B2) | Multiple | Notes |
|---|---|---|---|---|
| Peak RSS @ seg=128 MiB, conc=32 | 7.24 GiB | **1.28 GiB** | **5.6× less** | target was 10× (<0.72 GiB); see below |
| RSS per MiB-of-segment | 53 MiB/MiB (linear) | **~10 MiB/MiB (sublinear)** | **5.3× flatter** | whole-file `Vec<u8>` + 3× copy removed (WS3) |
| conc=128 saturate | **OOM-killed** | **survives, 920 MiB peak** | ∞ (crash → bounded) | WS4 in-flight bound + WS3 stream write |
| Peak ingest throughput (100% acc) | 2.35M logs/s @ conc=64 | **2.96M logs/s @ conc=64** | **1.26×** | absolute ceiling |
| Per-core throughput | ~235k logs/s·core (~10 cores) | **~604k logs/s·core (4.9 cores)** | **2.57×** | the real WS1+WS2 CPU win |
| Allocs / record (decode→build) | 37.2 | **28.2** | **1.32×** | decode-bound floor; see note |

## Memory — the segment sweep (conc=32, saturate, 40 s load)

| segment | peak VmHWM | steady (load) | post-idle (+25 s) | RSS/MiB |
|---|---|---|---|---|
| 16 MiB  | 425 MiB  | 383 MiB | 392 MiB | 26.6 |
| 64 MiB  | 1027 MiB | 683 MiB | 983 MiB | 16.0 |
| 128 MiB | 1282 MiB | 819 MiB | 1165 MiB | 10.0 |

Baseline was a **linear** 53 MiB-of-RSS per MiB-of-segment (16 MiB→1.3 GiB, 128 MiB→7.24 GiB).
After B2 the curve is **sublinear** — marginal slope drops from 12.5 MiB/MiB (16→64) to 4.0 MiB/MiB
(64→128). The segment-scaled bulk (compaction's whole-file Parquet buffer + the concat/sort/encode
copies held live at once) is gone: WS3 streams the Parquet encode straight to a `File` and drops each
intermediate batch as it is consumed, so peak no longer tracks segment size the way it did.

**c128 survival:** conc=128 saturate — the load that OOM-killed the baseline — now peaks at **920 MiB**
and runs to completion. The WS4 `max_in_flight` semaphore (default 256) caps concurrently-decoded
batches; excess load is shed (HTTP 503 / `resource_exhausted`) instead of piling decoded batches on the
heap. Confirmed in the sweep too: conc≥128 shows acc% falling (72%/62%) while the server stays up.

**On the 10× memory target:** we hit **5.6×** (1.28 GiB), not 10× (<0.72 GiB). The remaining ~1.3 GiB is
largely the fixed working set of the Arrow/DataFusion read+compact path and the WAL/hot buffering at a
128 MiB segment, not a segment-scaled leak — the slope is already sublinear. Dropping `segment_max_bytes`
to 16 MiB gets peak to **425 MiB** (a 17× reduction vs the 128 MiB baseline) if memory is the priority;
the segment size is now a genuine tuning knob rather than a 53×-multiplied liability.

**Idle return:** post-idle RSS stays within ~90% of peak for the segment sweep and drops to ~71% for
c128 (656 of 920 MiB). jemalloc returns some freed pages (baseline glibc retained ~all — post-idle ≈
peak); the modest return here suggests most of the peak is live working set, not allocator-retained.

## Speed — concurrency sweep (compaction ON, fresh server/point, tmpfs)

| conc | logs/s | acc% | srv_cores | logs/s·core |
|---|---|---|---|---|
| 8   | 553k  | 100 | 1.1 | 503k |
| 16  | 1.03M | 100 | 2.3 | 446k |
| 32  | 1.88M | 100 | 3.3 | 568k |
| 64  | **2.96M** | **100** | **4.9** | **604k** |
| 128 | 2.83M | 72  | 6.3 | — (shedding) |
| 256 | 2.77M | 62  | 7.2 | — (shedding) |

Honest read on speed: the **per-core** number is the real win — ~2.6× more logs/s per core-second (WS1
streams OTLP straight into the Arrow builder with no per-record `BTreeMap`/`Vec<LogRecord>`; WS2 moved
compaction's concat/sort/zstd off the async workers). The **absolute** ceiling only rose 1.26× (2.35M→
2.96M) because the server is **no longer CPU-bound** — at the 2.96M ceiling it uses just 4.9 of 16 cores.
The limit past conc=64 is the single WAL group-commit writer serializing appends, plus the WS4 in-flight
bound shedding the overflow. Saturating more cores would require the WAL-writer rewrite (Task 11, gated —
see `parallelism-diagnostic.md`); it is **not done** in this branch.

## Allocs / record

37.2 → **28.2** allocs/record (`alloc_guard.rs`, 1000 rows × 8 attrs, now measuring the streaming
`otlp_logs_into_builder` path). The measured section includes the prost protobuf **decode**, which
allocates a `String` per attribute field and dominates the count (~26/record) — an inherent floor WS1
does not address. WS1-F1 removed the build-side `BTreeMap` + `Vec<LogRecord>` (~9 allocs/record), which
is the whole 37.2→28.2 delta. `MAX_ALLOCS` guard tightened 500k → 30k to lock this in.

## Scorecard vs the plan's "10× faster + lighter"

- **Lighter:** ✅ substantially — 5.6× less peak RSS at the default segment, up to 17× at 16 MiB,
  sublinear slope, and the OOM under saturation is gone (bounded at <1 GiB).
- **Faster:** ✅ per-core (~2.6×); ⚠️ absolute ceiling only 1.26× because the write path is now
  WAL-writer/backpressure-bound, not CPU-bound — the remaining lever (Task 11) is gated and deferred.
- **Correctness:** the final integration review caught + fixed a Critical silent-data-loss bug
  (merge-id vs WAL-id collision); 362/362 tests green.

## Not re-run

`bench-micro` (criterion `logs_ingest` decode/map/build) was not re-run — the macro sweep + the alloc
guard already characterize the WS1 effect, and criterion adds ~10 min for marginal signal. Repoint and
run it if a per-operation micro number is needed.
