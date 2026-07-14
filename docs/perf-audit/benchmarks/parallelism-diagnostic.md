# WS2 diagnostic — is inline compaction the whole serialization story?

**Question (from the plan's Task 10):** after WS2 moved compaction's CPU onto `spawn_blocking`
(Task 5), does the ingest ceiling still plateau below full CPU? If disabling compaction entirely
lifts `srv_cores` toward ~16 and raises the ceiling, inline compaction was the story and the WAL-writer
rewrite (Task 11) is low value. If the plateau persists with compaction off, the WAL writer (or another
serialization point) is the cause → Task 11 is indicated.

Method: `bench-sweep.sh` (fresh server per concurrency point, tmpfs, 20 s saturate), run twice — once
normally, once with `PHOTON_DISABLE_COMPACTION=1` exported into the server env (the gate at
`main.rs` skips only the logs compactor spawn; server log confirms
`"PHOTON_DISABLE_COMPACTION set — logs compactor disabled"`).

## Sweep — compaction ON (normal)

| conc | logs/s | acc% | srv_cores |
|---|---|---|---|
| 8   | 553k  | 100 | 1.1 |
| 16  | 1.03M | 100 | 2.3 |
| 32  | 1.88M | 100 | 3.3 |
| 64  | 2.96M | 100 | 4.9 |
| 128 | 2.83M | 72  | 6.3 |
| 256 | 2.77M | 62  | 7.2 |

## Sweep — compaction DISABLED

| conc | logs/s | acc% | srv_cores |
|---|---|---|---|
| 8   | 548k  | 100 | 0.6 |
| 16  | 1.04M | 100 | 1.3 |
| 32  | 1.89M | 100 | 2.3 |
| 64  | 2.46M | 78  | 4.1 |
| 128 | 2.45M | 57  | 6.0 |
| 256 | 2.45M | 51  | 6.8 |

## Verdict

**Disabling compaction did NOT raise the ceiling or push `srv_cores` toward 16.** With compaction OFF
the ceiling is if anything *lower* (2.46M vs 2.96M) and cores stay in the same 4–7/16 band. So inline
compaction is **not** the serialization bottleneck — Task 5's `spawn_blocking` offload already removed
compaction as a CPU thief (the server never approaches the old ~11.4/16-core plateau; at its 2.96M
ceiling it uses just 4.9 cores).

**Important confound:** with compaction disabled, closed WAL segments are never drained, so at high
concurrency the undrained WAL fills tmpfs — the compaction-OFF run left **13 GiB** in `/dev/shm` and its
falling acc% (78→57→51) is partly ENOSPC-driven shedding, not a pure CPU signal. So the OFF sweep is a
weak *positive* probe (it can't cleanly show "faster without compaction"), but it is a clean *negative*
one: it definitively does **not** lift cores toward saturation.

**What actually limits the ceiling:** the server is **not CPU-bound** — 2.96M logs/s at only 4.9/16
cores, with acc% falling past conc=64 as the WS4 `max_in_flight` semaphore sheds the overflow. The
serialization point is the **single WAL group-commit writer** (all appends funnel through one
`tokio::fs` writer + fsync loop), plus the intentional in-flight bound. Neither is compaction.

## Task 11 (dedicated `std::fs` WAL writer thread + vectored writes) — GATED

Per the plan's gate ("plateau persists with compaction off → do Task 11"), the diagnostic **indicates**
Task 11: the remaining ceiling is the WAL writer. BUT it is left **DEFERRED as a decision**, not done,
because:

1. Task 11 rewrites the crash-consistency-critical WAL **append/ack** path (the durability boundary) —
   high-risk relative to the headroom it buys.
2. Phase B2 already delivers the primary goals: **5.6× less peak RSS**, **~2.6× per-core throughput**,
   and **OOM under saturation eliminated**. The absolute-throughput headroom Task 11 might unlock is the
   least-critical axis for a single-node self-hosted target.
3. The OFF-sweep confound above means the diagnostic proves "not compaction" more strongly than it
   proves "it's the WAL writer, and a thread will fix it."

Recommendation: implement Task 11 only if raising the absolute ingest ceiling above ~3M logs/s on this
box becomes a priority; measure a `commit_rounds`-style WAL micro-benchmark first to confirm the writer
is the bound before touching the ack path.
