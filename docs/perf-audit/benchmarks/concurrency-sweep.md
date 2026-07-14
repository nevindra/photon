# Concurrency Sweep — True Ingest Ceiling

**Commit:** 2257d54 (+ uncommitted Phase-A/B1 benchmark harness in the working tree)
**Date:** 2026-07-06  ·  **Host:** AMD Ryzen 7 9700X, 16 logical cores, tmpfs (`/dev/shm`) backing (fsync ~free → CPU/latency ceiling)
**Reproduce:** `make bench-sweep`

| concurrency | logs/s | MB/s | acc% | p50 ms | p95 ms | p99 ms | server cores |
|---|---|---|---|---|---|---|---|
| 8   | 519,243   | 70.6  | 100 | 7.4  | 8.8  | 9.5   | 1.9 |
| 16  | 959,225   | 130.5 | 100 | 7.9  | 9.8  | 10.9  | 3.9 |
| 32  | 1,659,243 | 225.7 | 100 | 9.2  | 12.5 | 14.4  | 6.5 |
| 64  | **2,348,832** | 319.5 | 100 | 12.9 | 19.9 | 23.0  | 9.9 |
| 128 | 2,269,421 | 308.7 | 100 | 27.2 | 39.6 | 46.8  | 11.4 |
| 256 | 2,010,415 | 273.5 | 100 | 61.2 | 97.9 | 116.0 | 11.3 |

`acc%` = fraction of requests the server accepted (2xx). It is the discriminator that makes the
verdict unambiguous: `data sent`/MB-s counts bytes for **every** request, but `logs/s` counts only
2xx, so without `acc%` a throughput dip could be either CPU saturation or the server shedding load.
Here acc% is **100 at every point** — the numbers reflect real accepted work, not rejection.

## Verdict — CPU-bound, clean-scaling; true ceiling ≈ 2.35M logs/s

**The write path is CPU-bound and scales near-linearly with cores up to the box's useful limit.**
Throughput rises almost proportionally with concurrency while server-cores rise in lockstep — from
conc=8 (1.9 cores) to conc=64 (9.9 cores), efficiency holds at **~240–255k logs/s per core**:

| region | logs/s per core |
|---|---|
| conc=16 → 3.9 cores | 246k |
| conc=32 → 6.5 cores | 255k |
| conc=64 → 9.9 cores | 237k |

- **True ceiling ≈ 2.35M logs/s at conc=64** (100% accepted, ~9.9 server-cores, ~320 MB/s). This is
  **45% above** the Phase-A baseline's 1.62M logs/s — that baseline only measured conc=32 and so
  undershot the ceiling. Q1's "are we concurrency-limited or CPU-saturated at 1.4–1.6M?" is answered:
  **the baseline was concurrency-limited**; the real ceiling is ~2.35M.
- **Past conc=64 throughput declines** (2.35M → 2.27M → 2.01M) while cores plateau at **~11.4 of 16**
  and latency inflates (p99 23ms → 116ms). The server does **not** saturate all 16 cores. Two causes,
  both consistent with the data: (1) **shared-box contention** — loadgen runs on the same 16 cores and
  gets heavier as concurrency climbs, so the server is starved; (2) a **serialization point** inside
  ingest (the WAL group-commit path is the prime suspect) that caps useful parallelism. Because
  throughput stays above 2M even at conc=256, this is a soft roll-off, not a hard lock ceiling.

**Shared-box caveat:** loadgen competes for the same 16 cores, so `server cores` is a **floor**, and
the 2.35M ceiling is a **lower bound** on a dedicated single node (loadgen elsewhere would let the
server claim more cores and push higher).

**Implication for Phase B2:** since throughput is set by per-core CPU efficiency (~250k logs/s/core),
the per-record CPU levers **F1** (kill per-record `BTreeMap`/`String` churn — 37.2 allocs/record from
the baseline) and **F2** (`RecordBatchBuilder::with_capacity`) translate directly into a higher
ceiling: roughly, halving per-record CPU ≈ doubling logs/s per core ≈ doubling the ceiling.
A secondary, lower-priority lever is the serialization point that keeps the server under ~11.4 cores.

## Methodology note (deviation from the plan, for reproducibility)

The plan's original `bench-sweep.sh` ran all six concurrency points against **one long-lived server**
on the size-limited tmpfs. That is a confound: cumulative WAL + hot files from the earlier points fill
`/dev/shm` (~15 GiB), and the later (higher-concurrency) points then fail on `ENOSPC`
("Disk quota exceeded") — not on any server limit. The first run showed exactly this: acc% collapsed
to **1%** at conc≥128 with `compactor: run_once failed: … Disk quota exceeded` in the server log.
The script was corrected to start a **fresh server on a wiped data dir per concurrency point**, so each
concurrency is measured in isolation on an empty disk; the acc% column remains as a guard (it would
drop below 100 if any single point still filled mid-run — none did). The latency parse was also fixed
(the original captured the `50` in the `p50` label alongside the value).
