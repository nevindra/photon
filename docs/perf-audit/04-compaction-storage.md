# Compaction & Storage — Perf & Correctness Audit
**Scope:** photon-compact (`compactor.rs`, `span_compactor.rs`, `metrics_compactor.rs`, `lib.rs`), photon-storage (`storage.rs`, `replicator.rs`, `lib.rs`)  ·  **Date:** 2026-07-06
**Method:** static read of the crates + the wiring in `photon-server/src/main.rs`, cross-checked against `photon-core::manifest`, `photon-query` (consumer of the manifest), `photon-wal`, and the vendored `object_store 0.11.2` / `parquet 53.4.1` sources. Runtime numbers are tagged `NEEDS-BENCH`.

> The three compactors (`Compactor` / `SpanCompactor` / `MetricsCompactor`) are byte-for-byte structural clones. **Every finding below applies to all three** unless noted; I cite the logs `compactor.rs` line and the identical construct exists in the other two.

---

## TL;DR — biggest 10x levers

1. **Peak RAM is ~3× the WAL segment, ×3 signals, and it's tunable only via `wal.segment_max_bytes` (128 MiB default).** `run_once` holds `batches` + `concatenated` + `sorted` simultaneously and then materialises the *entire* Parquet file in a `Vec<u8>` before writing. Two early `drop`s + streaming the writer to a temp file roughly halves the peak for a few lines of change. **[P1 memory]**
2. **All compaction CPU (concat + lexsort + take + zstd encode) runs inline on the tokio runtime, not `spawn_blocking`** — the exact opposite of the query path, which *does* `spawn_blocking` its pruning. Under sustained ingest this starves the shared executor that also serves ingest and queries. **[P1 speed]**
3. **The manifest handoff is not crash-atomic and `Manifest::add` is not idempotent** → a crash *or a single transient `remove_segment` error* between `save_manifest` and `remove_segment` produces a **duplicate manifest entry**, which `prune` turns into the **same Parquet file read twice** = duplicate rows / inflated counts, permanently. **[P0 crash-inconsistency]**
4. **No fsync barrier before the WAL segment is dropped.** `object_store`'s local `put` is staged-write-then-`rename` with *no* `sync_all` and *no* parent-dir fsync (verified in 0.11.2); `remove_segment` is a bare `remove_file`. Compaction moves durability from WAL→Parquet without a durability barrier, so a power loss can lose an already-acked segment — contradicting the documented "survives a crash" guarantee. **[P1 durability]**
5. **No size-tiered compaction:** the single 10k-row merge threshold freezes every file the moment it crosses 10k rows, so under sustained ingest the manifest bloats to thousands of small Parquet files that queries must prune/`.idx`-read every time. **[P2 read-amplification]**

---

## Findings (ranked)

### 1. Non-atomic {save_manifest → remove_segment} + non-idempotent `Manifest::add` → duplicate rows/counts  ·  **P0** · correctness / crash-inconsistency
- **Where:** `photon-compact/src/compactor.rs:89-96` (`run_once`); `photon-core/src/manifest.rs:45-47` (`add` just `push`es, no dedup); consumed at `photon-query/src/lib.rs:291-297` (`prune` pushes one path *per candidate entry*).
- **What:** `run_once` does, in order: `save_manifest` (91) → `enqueue` replication (93-94) → `remove_segment` (96). If the process crashes, **or if `remove_segment` returns any error** (EIO on unlink, etc. — it propagates as `Err` and the loop breaks), the closed segment is *still listed*. Next tick re-runs `run_once`: it rewrites the same Parquet path (fine, overwrite), reloads the manifest **that already contains the entry**, and calls `manifest.add(entry)` again → a **second `FileEntry` with the same `path`/`segment_id`**. `prune` then emits that path twice and `ctx.read_parquet(surviving, …)` scans the file twice → **duplicate rows in `search`, over-count in `count`/`histogram`/`facet`**. If both duplicates later fall under the merge threshold, `merge_once` reads the same file twice (`compactor.rs:131-133`) and **bakes the duplication into the merged Parquet**.
- **Why it matters:** Silent wrong answers, not a transient — the dup persists in the manifest. Reachable without a crash (any `remove_segment` error). Under sustained ingest, unlink errors and restarts are not exotic.
- **Fix:** Make `Manifest::add` an **upsert keyed by `segment_id`** (replace an existing entry with the same id) — `segment_id` is already the natural idempotency key and `merge_once` already reuses ids. ~5 lines in `manifest.rs`, closes the window for all three signals and both `run_once`/`merge_once`. (Optionally also fsync-then-remove per Finding 4 so the retry path is rarer.)
- **Effort/Risk:** S / low.
- **Invariant check:** Restores crash-idempotency of the WAL→Parquet handoff. Does not touch sort order, pruning conservativeness, or the async-replica boundary.

### 2. Whole-segment 3× copy + full in-memory Parquet buffer = peak RSS bomb  ·  **P1** · memory
- **Where:** `photon-compact/src/compactor.rs:83-87` (`run_once`), `:164-169` (`concat`), `:172-196` (`sort`), `:209` + `:344-360` (`encode_parquet` → `Vec<u8>`), `:281-288` (`put_object` takes the whole `Vec`). Peak is bounded *only* by `wal.segment_max_bytes` = 128 MiB default (`photon.example.toml:56`).
- **What:** In one `run_once` these are all live at once:
  1. `batches: Vec<RecordBatch>` — the full segment decoded from the WAL.
  2. `concatenated` — a second full contiguous copy (`concat_batches`).
  3. `sorted` — a third full copy (`take_record_batch` on the sort indices; `take` on the wide `attributes` MapArray allocates).
  4. `encode_parquet` then builds the **entire** compressed file in a `Vec<u8>` while `sorted` is still alive, plus the ArrowWriter's per-row-group column buffers (up to 1,048,576 rows, `DEFAULT_MAX_ROW_GROUP_SIZE`).
  `batches` and `concatenated` are never dropped early — they live to the end of `run_once`. So peak ≈ **3× uncompressed segment + compressed output buffer + row-group buffers**. The three signal loops each sleep(2s) independently (`main.rs:281/343/393`) and can hit their CPU peak concurrently → multiply by up to 3.
- **Why it matters:** Directly against the "10x lighter / low memory" single-node goal. A 128 MiB segment can transiently cost ~400 MiB; three signals ~1 GiB of churn. `NEEDS-BENCH` for exact multiplier, but the ownership graph is `MEASURED`.
- **Fix (cheap first):** `drop(batches)` immediately after `concat`, `drop(concatenated)` immediately after `sort` → cuts peak from ~3× to ~2×. **(bigger)** Since the hot store is *always* local disk, stream the `ArrowWriter` to a `std::fs::File` (or `object_store::put_multipart`) instead of a `Vec<u8>`, so the compressed file never sits fully in RAM. **(biggest)** Feed batches to the writer in sorted order via a k-way merge instead of `concat`+`take`, avoiding copies 2 and 3.
- **Effort/Risk:** S for the drops / M for streaming write / L for merge-sort. Low risk for the drops.
- **Invariant check:** Streaming to a `File` bypasses `object_store` for the write — keep the atomic-rename + fsync semantics (Finding 4). Sort order unchanged.

### 3. CPU-heavy encode/sort run inline on the async runtime (no `spawn_blocking`)  ·  **P1** · speed
- **Where:** `photon-compact/src/compactor.rs:84-87` — `concat`, `sort_by_service_and_timestamp` (lexsort+take), and `encode_parquet` all execute synchronously inside the `async fn run_once`, which runs inside `tokio::spawn` (`main.rs:276`). Contrast `photon-query/src/lib.rs:309-317`, where pruning is deliberately `spawn_blocking`.
- **What:** zstd Parquet encoding and Arrow lexsort/take are heavily CPU-bound and hold the tokio worker thread between `.await` points (the only yields are the `put_object` disk writes, which `object_store` *does* offload internally). Ingest receivers (OTLP gRPC/HTTP) and query handlers share this runtime.
- **Why it matters:** Under sustained ingest, a large encode blocks a worker for its whole duration, adding tail latency to ingest acks and queries. `NEEDS-BENCH` to quantify; the asymmetry with the query path is `MEASURED`.
- **Fix:** Wrap the concat→sort→encode section in `tokio::task::spawn_blocking` (or a dedicated rayon/thread pool). The batches must be `Send` (they are). Keep the `object_store` `put`s async.
- **Effort/Risk:** S–M / low.
- **Invariant check:** Pure relocation of work; no semantic change.

### 4. No durability barrier before dropping the WAL segment  ·  **P1** · durability / crash-consistency
- **Where:** write path `compactor.rs:201-230` (`write_file` → `put_object`), then `remove_segment` at `:96`. `object_store 0.11.2` local `put_opts` = staged write + `std::fs::rename` with **no `sync_all` and no parent-dir fsync** (verified: `object_store-0.11.2/src/local.rs:379-397`). `remove_segment` = bare `std::fs::remove_file`, **no dir fsync** (`photon-wal/src/disk.rs:221`).
- **What:** After `run_once` returns, the Parquet + `.idx` + `manifest.json` bytes may still be in the page cache while the WAL segment's `unlink` metadata reaches disk. The WAL itself uses an explicit `sync_data` to survive power loss (`disk.rs` group-commit); compaction discards that guarantee for the compacted data. A power failure / kernel panic in the writeback window (seconds) can leave: WAL segment gone, Parquet/manifest not durable → **acked data lost**.
- **Why it matters:** Contradicts the documented invariant "data survives a crash from [the WAL fsync]." Narrow (power-loss only; a clean process crash keeps the page cache), but it's a silent data-loss window on exactly the hardware event the WAL was built to survive.
- **Fix:** Before `remove_segment`, fsync the newly written objects **and** their parent directory (`data/`, `manifest/`). Simplest with the local-only hot store: write Parquet/idx via `File` + `sync_all`, then `File::open(dir)?.sync_all()` on the containing dir, then save+fsync the manifest, *then* remove the segment. (Pairs naturally with the streaming write in Finding 2.)
- **Effort/Risk:** M / medium (touches the write path of all three signals).
- **Invariant check:** Strengthens the crash-consistency invariant; keeps local-primary/async-replica boundary intact.

### 5. `merge_once` overwrites an input file in place before committing the manifest  ·  **P1** · correctness / crash-consistency
- **Where:** `compactor.rs:124-156` — `merged_seg = max(small ids)` (124-128); `write_file(merged_seg, …)` **overwrites the existing Parquet at that path** with the merged content (137) *before* `save_manifest` (144); the other inputs are deleted only afterward (149-156).
- **What:** Between the in-place overwrite (137) and the manifest commit (144) the on-disk file for `merged_seg` already holds the *union* of all merged rows, but the manifest still describes it with the **old, narrower** `min/max`/`row_count`. A crash/error in that window leaves: (a) the other small inputs still present *and* their rows now also inside the overwritten file → **duplicate rows on read**; (b) a manifest entry whose `min/max` is narrower than the file's true content. Because the other inputs still exist, this manifests as over-count rather than loss — but it's an inconsistency baked by reusing an input id as the output.
- **Why it matters:** Same non-atomic class as Finding 1, plus a stale-min/max hazard. Rare (merge is every 10s and the window is short), but avoidable by construction.
- **Fix:** Write the merged file under a **new** segment id / new path, `save_manifest` (single commit point), then delete **all** inputs (including the old one). Combine with the idempotent `add` (Finding 1) and fsync barrier (Finding 4).
- **Effort/Risk:** S–M / low.
- **Invariant check:** Makes the manifest swap the atomic commit; no in-place data mutation; sort order unchanged.

### 6. Replicator silently drops objects after ~150 ms of retries; `durable` flag never set  ·  **P2** · correctness (durable tier)
- **Where:** `photon-storage/src/replicator.rs:88-104` (`replicate_with_retry` returns `false` and the item is dropped, no re-enqueue, no metric), `MAX_ATTEMPTS=5` (`:15`), `BASE_BACKOFF=10ms` (`:17`) → total backoff ≈ 10+20+40+80 = 150 ms. Also `main.rs:315` wires `on_durable` to `println!` only, so the manifest's `durable` flag (set `false` at `compactor.rs:227`) is **never flipped to `true`** despite the doc claim in `photon-storage/src/lib.rs:9-11`.
- **What:** Any durable outage longer than ~150 ms permanently drops that object from the queue — the durable replica silently diverges from hot, with no record to reconcile from. And nothing ever records which objects *are* durable.
- **Why it matters:** Primary/query path is unaffected (async replica), so not P0 — but "durable backup" silently becoming incomplete defeats its purpose, and there's no signal that it happened.
- **Fix:** On exhausted retries, re-enqueue at the tail (or persist a pending-replication set); increment a failure counter. Wire `on_durable` to actually mark the manifest entry `durable = true` (needs the manifest write to stay single-writer — route through the compactor loop, not the detached replicator task).
- **Effort/Risk:** M / medium (manifest write ownership).
- **Invariant check:** Must keep the compactor the sole manifest writer; don't let the replicator task race manifest writes.

### 7. Replicator re-spawned every tick → unbounded concurrent drains, each buffering a whole file  ·  **P2** · robustness / memory
- **Where:** `main.rs:311-315` re-spawns `replicator.clone().spawn(…)` **every 2 s tick**; `spawn` drains-then-exits (`replicator.rs:60-83`); `replicate_once` loads the whole object into RAM (`hot.get(&p).await?.bytes()` then `durable.put(PutPayload::from(bytes))`, `:106-115`).
- **What:** If durable is slow, a drain task can still be running when the next tick spawns another. They share the queue (mutex) but run concurrently, so you get N concurrent `get→put`, each holding a full-file `Bytes` buffer → upload concurrency and memory both scale with how far durable lags. The path-`String` queue itself is unbounded but cheap; the real cost is the concurrent in-flight file buffers.
- **Why it matters:** A durable slowdown turns into a memory/concurrency amplifier on the primary node.
- **Fix:** Spawn **one** long-lived drain task at startup that loops on an interval / `Notify` instead of re-spawning per tick; cap in-flight uploads with a `Semaphore`.
- **Effort/Risk:** M / low.
- **Invariant check:** Replication stays off the ack/query path.

### 8. Purge/merge delete hot objects but never the durable copies  ·  **P2** · space (durable tier)
- **Where:** `merge_once` deletes only from `storage.hot` (`compactor.rs:151-153`); `purge_before` deletes only from hot (`:324-327`). No durable delete is ever enqueued.
- **What:** Retention and merge reclaim *local* disk but the durable store accumulates every Parquet/idx ever written (including superseded merge inputs) forever. Durable-tier retention is unenforced.
- **Why it matters:** Under sustained ingest + a configured durable tier, S3 cost/space grows without bound.
- **Fix:** Enqueue durable deletes alongside hot deletes (respecting async-replica: deletes must not block), or run a periodic durable GC that reconciles against the manifest.
- **Effort/Risk:** S–M / low.
- **Invariant check:** Deletes must stay async / off the ingest+query path.

### 9. Single 10k-row merge threshold → manifest bloat / read amplification at scale  ·  **P2** · read-amplification / scalability
- **Where:** `MERGE_ROW_THRESHOLD = 10_000` (`compactor.rs:46`); `merge_once` only ever consolidates files *below* the threshold (`:114-116`), merging into one file that is then "large" and never touched again.
- **What:** There's one flat merge tier. Every file freezes the moment it crosses 10k rows (a tiny Parquet file). Under steady ingest you accumulate an ever-growing population of ~10k–1M-row files; the manifest grows to thousands of entries and every query prunes + reads an `.idx` for each time-overlapping candidate (`photon-query prune`).
- **Why it matters:** For the "HUGE data" target this is the dominant long-run query cost — pruning is O(candidate files) and file count is unbounded.
- **Fix:** Size-tiered / leveled compaction (merge into progressively larger tiers), or raise the threshold substantially and add a periodic "compact everything older than X into day-files" pass. Cadence knobs live in `main.rs:40-44`.
- **Effort/Risk:** L / medium (new compaction policy).
- **Invariant check:** Keep `(service, timestamp)` sort within each output file; keep min/max from the same `SkipIndex::build`.

### 10. Detached, unsupervised compactor tasks → a panic silently stops compaction forever  ·  **P2** · robustness
- **Where:** `spawn_compactor`/`spawn_span_compactor`/`spawn_metric_compactor` (`main.rs:276/338/388`) `tokio::spawn` and drop the `JoinHandle`. `run_once` errors are logged and the loop continues, but an unexpected **panic** (e.g. a poisoned replicator mutex — `replicator.rs:43/50/72` `.expect("…poisoned")`, or an Arrow kernel panic on malformed data) kills the task with no restart.
- **What:** If a compactor task panics, its loop is gone; closed WAL segments then accumulate indefinitely → hot disk fills → ingest eventually blocks. CLAUDE.md advertises "background-task supervision" but none is present here.
- **Why it matters:** A single poison event silently disables durability compaction for that signal until the process is restarted.
- **Fix:** Supervise: watch the `JoinHandle` and restart on panic, or wrap the loop body so a panic is caught/logged and the loop continues. Avoid `Mutex` poisoning by not panicking while holding the replicator lock.
- **Effort/Risk:** S–M / low.

### 11. Replicator buffers each whole object in RAM (no streaming multipart)  ·  **P3** · memory
- **Where:** `replicator.rs:106-115` — `hot.get(&p).await?.bytes()` then `durable.put(PutPayload::from(bytes))`.
- **What:** Whole-file buffer per upload. Bounded by file size (small today, larger after merge/leveled compaction).
- **Fix:** `object_store::put_multipart` streaming for files above a threshold.
- **Effort/Risk:** M / low. Lower priority until files get large.

### 12. Parquet writer left fully on defaults  ·  **P3** · tuning (disk size)
- **Where:** `encode_parquet` sets only `Compression::ZSTD(Default)` (`compactor.rs:346-348`). Verified defaults (parquet 53.4.1): **ZSTD level = 1** (`ZstdLevel::default()` == `ZstdLevel(1)`), dictionary **on**, `EnabledStatistics::Page` **on**, `max_row_group_size = 1,048,576`.
- **What:** Note the common "zstd default is slow" worry is **false here** — level 1 is the *fast, low-ratio* end. Dictionary-on is good for `service.name`/`severity`; Page stats help DataFusion's in-file row-group/page skipping. The only real lever is the compression/disk-size tradeoff: level 3–6 would shrink files ~10–30% (helping the "10x lighter" disk goal) at CPU cost.
- **Fix:** Make the zstd level a config knob; leave the default at 1 or bump to 3. Consider `set_column_dictionary_enabled` explicitly for the high-cardinality body column (dictionary there can hurt). `NEEDS-BENCH`.
- **Effort/Risk:** S / low.

---

## Quick wins
- **Idempotent `Manifest::add` (upsert by `segment_id`)** — closes Finding 1 (P0) for all three signals in ~5 lines. *Do this first.*
- **`drop(batches)` after concat, `drop(concatenated)` after sort** — Finding 2, ~2 lines, ~1× less peak RAM.
- **Wrap concat→sort→encode in `spawn_blocking`** — Finding 3, keeps the runtime responsive under ingest.
- **Spawn the replicator drain loop once (not per tick) + re-enqueue on retry-exhaustion** — Findings 6/7.
- **Make zstd level configurable** — Finding 12.

## Bigger bets (architectural)
- **Streaming, fsync'd write path:** ArrowWriter → temp `File` → `sync_all` → dir fsync → atomic rename → manifest fsync → *then* `remove_segment` (Findings 2, 4, 5). Removes both the RAM peak and the durability window, and makes `merge_once` write to a fresh id.
- **Size-tiered / leveled compaction** to bound manifest/file count under sustained ingest (Finding 9).
- **Durable-tier lifecycle:** wire `on_durable` to flip `durable=true` (single-writer via the compactor loop), and enqueue durable deletes on purge/merge (Findings 6, 8).

## Already good / no action
- **Sort order is correct and stable.** Arrow `lexsort_to_indices` + `take_record_batch` on `(service.name, timestamp)` / `(service, start_time)` / `(metric_name, service, timestamp)` produces exactly the physical order pruning relies on (`compactor.rs:172-196`, `span_compactor.rs:132-160`, `metrics_compactor.rs:139-155`). A missing sort column errors loudly rather than silently skipping the sort.
- **Manifest min/max can't disagree with the `.idx`.** Both come from the *same* `SkipIndex::build` over the *same* sorted batch (`compactor.rs:212-216`), so there's no independent max computation to get wrong. (False-negative safety ultimately rests on `photon-index`, out of scope here.)
- **`purge_before` is conservative** — keeps straddling files (`max_ts < cutoff` only), never drops newer data (`compactor.rs:299-333`).
- **Handoff ordering is in the right direction** — the WAL segment is removed only *after* the manifest is saved; the gap is the missing fsync barrier + non-idempotent retry, not the ordering.
- **Per-signal separate manifests** (`manifest/manifest.json`, `spans-manifest.json`, `metrics-manifest.json`) with a single writer each → no write-write races.
- **`enqueue` is a true no-op when `durable` is `None`** (`replicator.rs:37-45`) — hot-only mode pays nothing.
- **`run_once` errors are logged and the loop survives** to the next tick (`main.rs:285-292`) — only *panics* are unhandled (Finding 10).

## Open questions & NEEDS-BENCH
- **Peak-RSS multiplier per compaction** at 128 MiB segments, and combined across the 3 concurrent signal loops — `NEEDS-BENCH` (loadgen `--saturate`, watch RSS). Hypothesis: ~3× uncompressed segment before the Finding-2 fixes.
- **Executor-starvation impact of inline encode** (Finding 3): measure ingest-ack p99 and query latency with vs. without `spawn_blocking` under saturating ingest — `NEEDS-BENCH`.
- **zstd level 1 → 3 tradeoff** (Finding 12): file-size reduction vs. compaction CPU/time — `NEEDS-BENCH`.
- Does any consumer actually read the manifest `durable` flag? (Grep says no — it's currently cosmetic; confirm before investing in Finding 6's flag-flip.)
- Confirm the exact writeback-window durability behavior on the target FS (ext4 `data=ordered` vs. others) to size the Finding-4 risk precisely.
