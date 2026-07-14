# Uptime Vertical + Loadgen — Perf & Correctness Audit
**Scope:** photon-uptime (primary), photon-loadgen (secondary)  ·  **Date:** 2026-07-06

Read-only audit. No source was modified. All findings cite `file:line`; MEASURED facts and
HYPOTHESES are tagged, and anything requiring a microbench is tagged `NEEDS-BENCH`. Severity is
calibrated against the vertical's own declared scale — `docs/superpowers/specs/2026-07-04-uptime-monitoring-design.md:31`
says "100 monitors @ 30s ≈ 288k [heartbeats/day]" — which at the default 30-day retention
(`crates/photon-core/src/config.rs:100-102`) implies a **steady-state `heartbeats` table around
8.6M rows**. That number is load-bearing for several findings below.

## TL;DR — biggest 10x levers

1. **No `spawn_blocking` anywhere in `photon-uptime` (grep confirms zero hits).** Every
   `SqliteStore` method (`crates/photon-uptime/src/store/sqlite.rs`) runs synchronous rusqlite
   calls directly inside an `async fn`, on the same multi-threaded tokio runtime that also drives
   OTLP ingest and query (`crates/photon-server/src/main.rs:46` `#[tokio::main]`,
   `tokio::try_join!(ingest.serve(...), api.serve(...))` at line 228). Combined with finding U4
   below (an hourly, **unindexed, unbatched, full-table-scan `DELETE`** over a ~8.6M-row table),
   this is the one architectural issue that can stall unrelated request-handling threads, not just
   uptime. (U3 + U4)
2. **`reqwest::Client` and `surge_ping::Client` are rebuilt from scratch on every single probe**
   (`crates/photon-uptime/src/probe.rs:71-79` for HTTP, `:148` for ICMP) — every check pays full
   TCP+TLS handshake cost (no keep-alive reuse across checks of the *same* monitor) and, for ICMP,
   opens a fresh raw socket per ping. `notify.rs` already does this correctly (builds one
   `reqwest::Client` in `WebhookNotifier::new` and reuses it) — `probe.rs` should follow the same
   pattern. Cheap, high-value fix. (U1 + U2)
3. **`SqliteStore::open` never sets `PRAGMA synchronous`** (`sqlite.rs:80-83` sets `journal_mode`,
   `foreign_keys`, `busy_timeout` only), so it stays at SQLite's default `FULL`, which under WAL
   fsyncs the WAL file on *every* commit instead of only at checkpoint. `synchronous=NORMAL` is
   the documented safe pairing for WAL (still crash-safe against a process crash) and is a
   one-line addition. Every `append_heartbeat` call currently pays the more expensive sync policy.
   (U11)
4. **No jitter on the initial/re-scheduled `next_due`** (`scheduler.rs:95`, `:135` both use
   `now_ms()`) — every monitor at the same interval fires in lockstep forever, producing
   correlated CPU/socket/SQLite-write bursts every interval boundary (and an immediate stampede of
   every enabled monitor on server restart) instead of smoothly spread load. (U6)

Loadgen is in good shape — client reuse, correct accounting, and a well-tested rate limiter are
already in place. Only one real efficiency question surfaced there (payload-generation CPU cost
vs. network cost at very high `--rate`), tagged `NEEDS-BENCH`, not a fix.

---

## Findings — Uptime (ranked)

### U3 — SQLite calls run synchronously on the shared tokio runtime, no `spawn_blocking`
- **Severity:** P1 · **Category:** speed
- **Where:** every method in `crates/photon-uptime/src/store/sqlite.rs` (e.g.
  `append_heartbeat:202-207`, `set_monitor_state:208-222`, `list_monitors:130-137`); the crate has
  zero occurrences of `spawn_blocking` (verified by search).
- **What:** `SqliteStore` holds a single `Mutex<Connection>` and every trait method does
  `let c = self.conn.lock().unwrap();` then issues a blocking rusqlite call inline inside the
  `async fn` body. `photon-server` runs `#[tokio::main]` (multi-threaded) and the uptime scheduler
  + prune tasks share this runtime with OTLP ingest and the query engine
  (`crates/photon-server/src/main.rs:228`, `:451-473`).
- **Why it matters:** Any rusqlite call that blocks on disk I/O (a `PRAGMA synchronous=FULL`
  fsync per commit — see U11 — or, worse, the prune scan in U4) occupies a tokio worker thread for
  its full duration. On a small worker-thread pool this can transiently starve unrelated ingest/
  query tasks scheduled on the same runtime. At the declared scale (~100 monitors, a few
  heartbeats/sec) routine writes are fast enough that this is mostly theoretical, but it compounds
  directly with U4's multi-million-row scan.
- **Fix:** Wrap each `Connection`-touching block in `tokio::task::spawn_blocking`, or — simpler
  given SQLite only supports one writer anyway — move the `Connection` onto a dedicated OS thread
  behind an mpsc/oneshot request channel, so async callers never touch it directly. The dedicated
  thread also gives a single natural serialization point instead of a `Mutex` sprinkled with
  `spawn_blocking` at every call site.
- **Effort/Risk:** M / S (mechanical, well-isolated behind the `UptimeStore` trait; `MemStore`
  test double is unaffected).
- **Invariant check:** OK — doesn't touch `PhotonError`, doesn't move rusqlite version.

### U4 — Hourly retention prune is a full-table-scan, unbatched `DELETE`
- **Severity:** P1 · **Category:** speed + memory
- **Where:** `crates/photon-uptime/src/store/sqlite.rs:295-301` (`prune_heartbeats`, `DELETE FROM
  heartbeats WHERE ts < ?1`) and `:302-311` (`prune_incidents`); scheduled hourly at
  `crates/photon-server/src/main.rs:457-473`. Schema index is `idx_hb_monitor_ts ON
  heartbeats(monitor_id, ts DESC)` (`sqlite.rs:32`) — a composite index that **cannot** be used to
  filter on `ts` alone, so this delete does a sequential scan of the whole table.
- **What:** At the design's own declared steady-state (~8.6M rows in `heartbeats` at 100 monitors
  / 30s / 30-day retention), every hour this does a full sequential scan of the entire table to
  find the ~1/720th of rows past the cutoff, as one single unbatched transaction, while holding
  the one shared connection `Mutex` — blocking every other uptime store operation (heartbeat
  writes, monitor reads, incident open/close) for the scan's duration — and (per U3) blocking a
  tokio worker thread directly rather than yielding it.
- **Why it matters:** This is the most scale-sensitive item in the vertical: it gets worse every
  month of retained history, is on a fixed hourly timer regardless of load, and single-handedly
  stalls the store for all monitors at once, not just the one being pruned.
- **Fix:**
  - Add `CREATE INDEX IF NOT EXISTS idx_hb_ts ON heartbeats(ts);` so the delete becomes an index
    range scan instead of a full scan.
  - Batch the delete so no single transaction holds the lock for long. `rusqlite`'s `bundled`
    feature does not compile in `SQLITE_ENABLE_UPDATE_DELETE_LIMIT`, so `DELETE ... LIMIT` isn't
    available directly — chunk via `DELETE FROM heartbeats WHERE id IN (SELECT id FROM heartbeats
    WHERE ts < ?1 LIMIT 5000)` in a loop until 0 rows affected (same pattern for `prune_incidents`,
    which is far smaller volume but should stay consistent).
  - Combine with U3's `spawn_blocking`/dedicated-thread fix so the scan doesn't tie up a shared
    async worker.
- **Effort/Risk:** S (index) + M (chunking loop) / S.
- **Invariant check:** OK.

### U1 — `reqwest::Client` rebuilt on every HTTP probe (no connection/keep-alive reuse)
- **Severity:** P1 · **Category:** speed
- **Where:** `crates/photon-uptime/src/probe.rs:70-83` (`probe_http`) — `reqwest::Client::builder()
  ... .build()` runs fresh inside every call.
- **What:** Each HTTP check builds a brand-new `Client` (fresh connection pool, fresh TLS config)
  and issues one request on it, so successive checks of the *same* monitor never reuse a TCP or
  TLS session. Contrast with `crates/photon-uptime/src/notify.rs:26-31`, which builds one
  `reqwest::Client` in `WebhookNotifier::new` and reuses it via `.clone()` per delivery — the
  correct pattern, just not applied here.
- **Why it matters:** Every HTTP check pays a full TCP handshake + TLS handshake (when the target
  is HTTPS) on top of the actual request/response, inflating both measured latency (the recorded
  `latency_ms` bakes in connection setup every time) and CPU/socket churn. This scales with
  monitor count × check frequency and is pure waste since `ignore_tls`/`follow_redirects` are
  static per-monitor config, not per-call state.
- **Fix:** Cache a `reqwest::Client` per distinct `(ignore_tls, follow_redirects, timeout)`
  config — simplest: store one `Client` per `Slot` in the scheduler (rebuilt only when the monitor
  is edited), or a small keyed cache inside `NetworkProber` keyed by the tuple. `timeout` can stay
  a per-request override (`RequestBuilder::timeout`) rather than forcing a client rebuild.
- **Effort/Risk:** S / S.
- **Invariant check:** OK — `reqwest`/`rustls-tls` version unchanged.

### U2 — `surge_ping::Client` (raw socket) rebuilt on every ICMP probe
- **Severity:** P1 · **Category:** speed
- **Where:** `crates/photon-uptime/src/probe.rs:148-156` (`probe_icmp`).
- **What:** `surge_ping::Client::new(&surge_ping::Config::default())` — which opens a raw (or
  DGRAM) ICMP socket — runs fresh on every single ping. `surge-ping`'s `Client` is designed to be
  constructed once per process and shared across many `Pinger`s (it's the socket-owning handle,
  analogous to `reqwest::Client`).
- **Why it matters:** Raw-socket creation is a syscall plus (per the existing error message at
  `probe.rs:152`) a capability check (`CAP_NET_RAW` / `ping_group_range`) — repeating it on every
  check of every ICMP monitor is avoidable syscall + setup overhead, and turns a privilege problem
  that should fail once at startup into one that's silently retried (and logged as a per-probe
  "down") on every check.
- **Fix:** Construct one `surge_ping::Client` when `NetworkProber` is built (or lazily on first use
  and cached), reuse it for every ICMP probe via `client.pinger(ip, ...)` per call (that part is
  already cheap/correct — only the `Client` itself needs to be shared).
- **Effort/Risk:** S / S (surfacing the socket-open failure at startup instead of per-probe is a
  minor behavior change worth calling out, not a risk).
- **Invariant check:** OK — `surge-ping` version unchanged.

### U11 — Missing `PRAGMA synchronous=NORMAL` (stuck on the more expensive WAL default)
- **Severity:** P2 · **Category:** speed
- **Where:** `crates/photon-uptime/src/store/sqlite.rs:80-83` (`SqliteStore::open`).
- **What:** The pragma batch sets `journal_mode=WAL`, `foreign_keys=ON`, `busy_timeout=5000` but
  never touches `synchronous`, so it stays at SQLite's default `FULL`. In WAL mode, `FULL` fsyncs
  the WAL file on every transaction commit; `NORMAL` only fsyncs at checkpoint time and is the
  documented-safe pairing with WAL (durable against a process/app crash; the only added risk is
  the much rarer "OS crash / power loss at the exact wrong instant," which `FULL` also doesn't
  fully protect a WAL database from beyond what `NORMAL` does).
- **Why it matters:** `append_heartbeat` and `set_monitor_state` each commit once per completed
  probe — i.e., this is a straight multiplier on the sync cost of the hottest write path in the
  vertical.
- **Fix:** Add `PRAGMA synchronous=NORMAL;` to the same `execute_batch` call at `sqlite.rs:80-83`.
- **Effort/Risk:** S / S.
- **Invariant check:** OK.

### U6 — No jitter on initial/rescheduled `next_due` → correlated bursts every interval
- **Severity:** P2 · **Category:** speed + correctness (thundering herd)
- **Where:** `crates/photon-uptime/src/scheduler.rs:95` (initial seed from the store) and `:135`
  (`SchedulerCommand::Upsert` handling) both set `next_due: now_ms()`.
- **What:** Every monitor becomes "due" at the same wall-clock instant it's loaded/upserted, and
  since `next_due` is recomputed as `now + interval*1000` off that same shared reference point
  every cycle (`scheduler.rs:114`), monitors sharing an interval stay in lockstep indefinitely —
  the 250ms scheduler tick (`scheduler.rs:106`) is far finer than any realistic interval, so
  nothing naturally desynchronizes them.
- **Why it matters:** Produces periodic correlated bursts of probes (bounded by the
  `worker_concurrency` semaphore, default 32 — `config.rs:109-111` — so not literally simultaneous
  sockets, but a queued burst) and correlated SQLite write bursts every interval boundary, plus an
  immediate stampede of every enabled monitor on every server restart. Even at the declared ~100
  monitor scale this is an observable, avoidable pattern.
- **Fix:** Jitter the initial `next_due` by a random offset in `[0, interval_secs*1000)` on both
  the seed path and `Upsert`, so monitors spread evenly across their interval window instead of
  syncing to process-start time.
- **Effort/Risk:** S / S.
- **Invariant check:** OK.

### U5 — `heartbeats()` has no LIMIT/downsampling; a 30-day window can return tens of thousands of rows
- **Severity:** P2 · **Category:** memory
- **Where:** trait method `crates/photon-uptime/src/store/mod.rs:30`; SQLite impl
  `crates/photon-uptime/src/store/sqlite.rs:223-239`; consumed by `GET
  /api/uptime/monitors/:id/heartbeats` at `crates/photon-api/src/uptime.rs:146-155` (`window_since`
  at `:137-144` allows up to a 30-day window).
- **What:** The query is efficient (uses `idx_hb_monitor_ts`), but there's no row cap or
  server-side downsampling — a monitor at the default 60s interval (`config.rs:103-105`) over a
  30-day window returns up to ~43,200 rows, fully materialized and JSON-serialized in one response,
  to feed what's ultimately a small heartbeat-bar / response-time chart in the UI
  (`frontend/src/components/uptime/HeartbeatBar.vue`-style consumers).
- **Why it matters:** Working against "low memory" — every chart render at a wide window pays for
  full-resolution history it doesn't display pixel-for-pixel.
- **Fix:** Add an optional `limit`/bucket parameter to the store method (e.g., bucket into ~200-500
  points server-side for windows beyond a threshold), or at minimum cap `heartbeats()` with a
  `LIMIT` and have the API request only what the chart can render.
- **Effort/Risk:** M / S (touches the trait + both impls + the API handler).
- **Invariant check:** OK.

### U7 — Statements re-prepared on every call instead of using rusqlite's statement cache
- **Severity:** P2 · **Category:** speed
- **Where:** every `conn.prepare(...)` call in `crates/photon-uptime/src/store/sqlite.rs` (e.g.
  `list_monitors:132-134`, `get_monitor:140-141`, `heartbeats:225`, `incidents:281`) and every
  `conn.execute(...)` convenience call (`append_heartbeat:204`, `set_monitor_state:216-219`, etc.,
  which prepare+step+finalize internally on each call).
- **What:** `rusqlite` ships a built-in LRU statement cache via `Connection::prepare_cached`, not
  used anywhere in this file — every call re-parses and re-plans the same fixed SQL text.
- **Why it matters:** Cheap freebie; SQL re-planning cost is small per call but is pure overhead on
  the hottest path (`append_heartbeat`, once per completed probe).
- **Fix:** Swap `conn.prepare(sql)` → `conn.prepare_cached(sql)` throughout; for the `.execute()`
  convenience calls, switch to `conn.prepare_cached(sql)?.execute(params)`.
- **Effort/Risk:** S / S. `NEEDS-BENCH` — at the declared scale this is dominated by U1-U4;
  worth doing opportunistically, not urgently.
- **Invariant check:** OK.

### U9 — Scheduler tick is an O(n) `HashMap` scan every 250ms regardless of how many are due
- **Severity:** P3 · **Category:** speed
- **Where:** `crates/photon-uptime/src/scheduler.rs:106` (`tokio::time::interval(250ms)`) and
  `:112` (`for slot in slots.values_mut()`).
- **What:** Every tick walks every monitor's slot to check `next_due`, even though only a handful
  are ever due at once. Fine at hundreds of monitors (cheap comparisons); would need a
  `BinaryHeap`/min-heap keyed by `next_due` to scale to a much larger fleet without the scan cost
  growing linearly with total monitor count.
- **Why it matters:** Not a bottleneck at the declared scale; flagged for awareness if the vertical
  is ever asked to support thousands of monitors.
- **Fix:** Not urgent. If scale grows, replace the `HashMap` iteration with a min-heap ordered by
  `next_due`, popping only entries that are actually due.
- **Effort/Risk:** N/A (no action recommended now) — would be M/S if undertaken.
- **Invariant check:** OK.

### U8 — Keyword-match body read has no size cap
- **Severity:** P3 · **Category:** memory
- **Where:** `crates/photon-uptime/src/probe.rs:101-103` (`resp.text().await.unwrap_or_default()`,
  gated on `m.keyword.is_some()` — the read is already correctly conditional, just uncapped).
- **What:** When a monitor configures a `keyword` check, the full response body is buffered into
  memory with no maximum-size guard.
- **Why it matters:** Low likelihood (targets are admin-supplied health-check URLs), but a
  misbehaving/misconfigured target returning a very large body would balloon memory for that probe
  task.
- **Fix:** Cap the read (e.g., stream with `resp.bytes_stream()` and bail out past N MB), or at
  least document the expectation that keyword-check targets return small bodies.
- **Effort/Risk:** S / S — low priority.
- **Invariant check:** OK.

---

## Findings — Loadgen (ranked, dev-only)

### L1 — Per-request payload generation allocates heavily; could self-bottleneck at very high rates
- **Severity:** P3 · **Category:** speed · **NEEDS-BENCH**
- **Where:** `crates/photon-loadgen/src/logs.rs` (`build_record`/`build_request`, every attribute
  is a fresh `String`), `crates/photon-loadgen/src/traces.rs` (`build_one_trace` builds a `Vec<Node>`
  tree with `String` names/attrs per span, then re-walks it into OTLP `Span`s), each ultimately
  encoding into a freshly `Vec::with_capacity`-allocated buffer per request
  (`logs.rs` `build_batch`, `traces.rs` `build_request_bytes` tail).
- **What:** No buffer/string reuse across requests — expected, since content must vary per
  request by design, but it means generation is real per-request CPU + allocator work happening on
  the same worker task that also awaits the network round-trip
  (`crates/photon-loadgen/src/worker.rs:28-58`).
- **Why it matters:** If generation CPU cost ever exceeds the network/ack cost at high
  `--rate`/`--saturate` settings, the loadgen becomes the bottleneck instead of the server —
  silently understating Photon's real ceiling and misleading whoever reads the benchmark numbers.
  No evidence this currently happens; flagging so it's checked rather than assumed.
- **Fix (if confirmed):** Profile `photon-loadgen`'s own CPU utilization at the target rate/
  concurrency; if generation dominates, consider trace-tree/string-pool reuse or pre-generating a
  small rotating pool of request bodies.
- **Effort/Risk:** N/A until measured.

### L2 — Single shared token-bucket `Mutex` could cap achievable rate at extreme rate+small-batch combos
- **Severity:** P3 · **Category:** speed · **NEEDS-BENCH**
- **Where:** `crates/photon-loadgen/src/ratelimit.rs:49-51` (`RateLimiter.inner: Option<Mutex<...>>`),
  `:72-89` (`acquire`).
- **What:** All workers serialize through one `tokio::sync::Mutex` per `acquire` call. In
  `--saturate` mode this is bypassed entirely (`inner: None` short-circuits at `:73-75`) — no
  concern there. In `--rate` mode, the number of `acquire` calls per second scales with
  `rate / batch_size`, so only very high `--rate` combined with a very small `--batch`/
  `--traces-per-request` would drive meaningful lock contention.
- **Why it matters:** Realistic invocations use batch sizes of hundreds (`--batch` defaults to 500,
  `logs.rs:22`), keeping acquire-call frequency low; this is only a concern at unusual CLI flag
  combinations.
- **Fix (if confirmed):** Shard the bucket per worker (each takes `rate/concurrency` tokens/sec
  locally) if this is ever observed to matter.
- **Effort/Risk:** N/A until measured.

---

## Quick wins

- **U1 / U2** — cache the `reqwest::Client` and `surge_ping::Client` instead of rebuilding per
  probe. Highest ratio of impact to effort in the whole audit.
- **U11** — add `PRAGMA synchronous=NORMAL;` next to the existing pragma batch
  (`sqlite.rs:80-83`). One line.
- **U6** — jitter `next_due` on initial schedule/upsert (`scheduler.rs:95`, `:135`).
- **U4 (index only)** — add `CREATE INDEX IF NOT EXISTS idx_hb_ts ON heartbeats(ts);` even before
  tackling the batching/spawn_blocking work; turns the prune's full scan into an index range scan
  immediately.
- **U7** — swap `prepare` → `prepare_cached` throughout `sqlite.rs`. Mechanical, no behavior
  change.

## Bigger bets (architectural)

- **Move `SqliteStore` off the shared async runtime entirely.** Instead of `Mutex<Connection>` +
  scattered `spawn_blocking`, run the connection on one dedicated OS thread behind an mpsc request/
  response channel. Gives a single serialization point that matches SQLite's single-writer model,
  and makes "never block the shared runtime" structurally true instead of something every new
  method has to remember. Resolves U3 and makes U4's fix safer by construction.
- **Downsample heartbeat history instead of keeping full resolution for the whole retention
  window.** Roll heartbeats older than e.g. 24h up to one point per minute (or per bucket). This
  shrinks the steady-state table (currently ~8.6M rows at the design's own declared scale),
  shrinks the hourly prune's scan cost, and directly shrinks U5's worst-case response payload —
  one change addressing both U4 and U5's root cause.
- **If the vertical is ever expected to scale past ~100s of monitors:** replace the `HashMap`
  scheduler scan (U9) with a `next_due`-ordered min-heap, and revisit whether per-probe
  `tokio::spawn` (rather than a fixed worker-pool with a queue) still makes sense at that scale.

## Already good / no action

- **SQL is fully parameterized** — every query in `sqlite.rs` uses `params![...]`; no
  string-interpolated SQL anywhere. No injection risk.
- **Timeout handling is correct across all three probers**: TCP uses `tokio::time::timeout`
  (`probe.rs:122`), HTTP uses `reqwest`'s per-request `.timeout()` covering connect+response
  (`probe.rs:72`), ICMP uses `pinger.timeout()` (`probe.rs:158`). No hung-connect leak found.
  `probe_icmp`'s "needs CAP_NET_RAW" error is surfaced, not swallowed (`probe.rs:150-155`).
- **State machine (`state.rs`) is pure, exhaustively table-tested**, and correctly handles the
  `retries` 0/1 edge case, flap-reset-on-recovery, and "already down → no repeat transition."
  Nothing to fix.
- **`WebhookNotifier` reuses one `reqwest::Client`** (`notify.rs:26-31`) and detaches delivery into
  its own task with bounded retries (3 attempts, capped backoff) so a slow/unreachable webhook
  never stalls the scheduler's `select!` loop (`notify.rs:58-73`). This is the pattern `probe.rs`
  should copy for U1.
- **Single long-lived `Connection`, no per-call connection churn** — `SqliteStore` opens one
  connection and guards it with a `Mutex` (`sqlite.rs:10-12`), rather than reopening per
  operation. `busy_timeout=5000` is already set specifically because the DB file is shared with
  the separate `SqliteUserStore` writer (`sqlite.rs:77-83` comment) — the two-writer contention
  case is already handled.
- **Scheduler survives a panic in any one probe**: each probe runs in its own detached
  `tokio::spawn` (`scheduler.rs:116-120`), so one bad prober can't kill the scheduling loop; a
  `process_result` error is logged, not propagated, and doesn't stop the loop either
  (`scheduler.rs:127`).
- **Loadgen client reuse, concurrency model, and stats accounting are all correct**: one pooled
  `reqwest::Client` shared via `Arc` across all workers with `pool_max_idle_per_host` sized to
  concurrency (`driver.rs:31-35`); each worker awaits its response before sending the next, so
  `--concurrency` is a true in-flight cap and server backpressure naturally throttles the client
  (`worker.rs` doc comment + body); `Stats::record` only counts `units`/`spans` on a 2xx response,
  never double-counts, and separately tracks transport errors vs HTTP errors
  (`stats.rs:64-83`, unit-tested at `stats.rs` bottom). The token-bucket rate limiter is
  unit-tested to within 5% drift over a simulated window (`ratelimit.rs` `simulated_window_tracks_target_rate`).

## Open questions & NEEDS-BENCH

- **What's the real target scale for photon-uptime?** The design doc frames ~100 monitors as "low
  volume." If that's a hard M1 ceiling, U1-U4 are still worth fixing (they're cheap and strictly
  better) but not urgent; if the vertical is expected to grow to 1,000+ monitors, U3/U4/U9 move
  from "good hygiene" to "will actually page someone."
- **`NEEDS-BENCH`**: measured cost of `reqwest::Client::builder().build()` and
  `surge_ping::Client::new()` per call in this environment — this audit asserts they're
  non-trivial (rustls `ClientConfig` construction, raw-socket syscall + capability check) as a
  reasoned HYPOTHESIS, not a measurement.
- **`NEEDS-BENCH`**: actual wall-clock duration of the hourly prune `DELETE` against a
  representative ~8-9M row `heartbeats` table on the target deployment disk, to confirm how long
  U3+U4 together block the store (and, if not `spawn_blocking`'d, a runtime worker thread).
- **`NEEDS-BENCH`** (loadgen, L1/L2): whether payload-generation CPU cost or rate-limiter lock
  contention ever becomes the limiting factor before the server does, at realistic high
  `--rate`/`--saturate` settings — no profiling was done as part of this audit.
