# Logs Write-Path Baseline (Phase A)

**Commit:** 2257d54  ·  **Date:** 2026-07-06
**Host:** AMD Ryzen 7 9700X (8C/16T), 16 logical cores, Linux 7.0.0-27-generic, repo disk fs=ext4 (NVMe /dev/nvme0n1p2)
**How to reproduce:** `make bench-micro` · `cargo test -p photon-ingest --test alloc_guard -- --nocapture` · `make bench-ingest`

> Measurement note: micro-benches ran with `--measurement-time 5` (criterion mid-point estimates
> reported). The two WAL rows contrast tmpfs (`PHOTON_BENCH_DIR=/dev/shm/photon-wal`, fsync is a
> no-op → CPU/framing ceiling) against real disk (`PHOTON_BENCH_DIR=./.photon-bench-wal`, ext4/NVMe
> → true fsync cost). The e2e layer contrasts the same two backings for the whole ingest path.

## Layer 1 — criterion micro-benches (elements/sec, higher is better)
| bench | 500 rows | 10k rows |
|---|---|---|
| map (otlp_logs_to_records, F1) | 2.424 Melem/s | 2.113 Melem/s |
| build (RecordBatchBuilder, F2) | 4.766 Melem/s | 1.836 Melem/s |
| decode_map_build (full CPU) | 702.4 Kelem/s | 458.6 Kelem/s |
| wal_append — tmpfs (F6/F7) | 81.35 Kelem/s / 6.146 ms/round (500) | 789.0 Kelem/s / 6.337 ms/round (5000) |
| wal_append — real disk | 71.43 Kelem/s / 7.000 ms/round (500) | 689.0 Kelem/s / 7.257 ms/round (5000) |

Each `wal_append` iteration is one full-batch `append` awaited alone, so it always pays the
~5 ms `group_commit_max_delay_ms` window (a lone writer never gets an early flush) plus framing;
the fsync-attributable cost is the disk−tmpfs delta only: **+0.85 ms/round at 500 rows, +0.92 ms
at 5000** (≈ +12–14% latency). The near-flat curve across 10× the rows (6.15 → 6.34 ms tmpfs)
shows a large fixed per-round cost dominates and the marginal per-row cost is tiny — this
per-round shape is a single-writer micro artifact; the e2e path amortizes it via group commit.

## Layer 2 — allocation guard
- decode→map→build, 1000 rows × 8 attrs: **37181 allocations** (37.2 allocs/record), 4336635 bytes.
- Phase-A ceiling `MAX_ALLOCS = 500_000` (regression guard). Phase-B target after F1: ~10x fewer.

## Layer 3 — end-to-end (photon-loadgen --saturate, 30s, concurrency=32 services=10 batch=500)
| backing | logs/s (avg) | MB/s (avg) | peak RSS | ack p50/p95/p99 |
|---|---|---|---|---|
| tmpfs (/dev/shm) | 1,618,012 | 220.1 | 6365 MiB | 9.3 / 13.1 / 16.5 ms |
| real disk | 1,398,992 | 190.3 | 5942 MiB | 10.9 / 14.8 / 17.3 ms |

## Regime read (the key Phase-A finding)
**The write path is CPU/allocation-bound, not fsync-bound.** Removing durable-disk fsync entirely
(tmpfs) buys only ~13.5% end-to-end (1.618 M vs 1.399 M logs/s; 220.1 vs 190.3 MB/s) and adds just
~1.6 ms to ack p50 (9.3 → 10.9 ms) — the WAL micro-bench agrees, where real disk is only ~12% slower
than tmpfs and fsync costs under 1 ms per group-commit round. Group commit is already doing its job:
across 32 concurrent senders the per-round fsync is amortized, so disk ≈ tmpfs. That means the
remaining ~87% of the cost lives in the per-request CPU pipeline — prost decode + OTLP→LogRecord map
(F1) + RecordBatch build (F2) — corroborated by the allocation guard's **37.2 allocs/record** of
churn (dominated by per-record `BTreeMap` construction) and by `decode_map_build` collapsing to
459–702 Kelem/s versus 1.8–4.8 Melem/s for `build` or `map` alone. **Phase-B priority: F1 (eliminate
per-record BTreeMap/`String` churn in `otlp_logs_to_records`) and F2 (`RecordBatchBuilder::with_capacity`)
— these attack the dominant cost and should drive the ~10x alloc reduction the guard is set up to
prove.** F6/F7 (WAL writer / group-commit tuning) and disk/batching work are secondary here: even a
perfect fsync path can only recover the ~13% tmpfs headroom.
