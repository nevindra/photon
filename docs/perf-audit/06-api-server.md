# API Layer & Server Binary — Perf & Correctness Audit
**Scope:** photon-api, photon-server  ·  **Date:** 2026-07-06
**Method:** static read of the source; no benchmarks exist in the tree, so speed numbers are
**HYPOTHESIS / NEEDS-BENCH** unless stated otherwise. Two facts are structurally verified (not
guessed): (1) `#[tokio::main]` + `tokio::try_join!(ingest.serve(...), api.serve(...))` in
`photon-server/src/main.rs:228` run OTLP ingest and the REST API on the **same** multi-thread
runtime/worker pool (`tokio` workspace feature is `rt-multi-thread`, no separate runtime is built);
(2) `cookie::Key::derive_from` (vendored source checked) panics below 32 bytes of key material.

---

## TL;DR — biggest 10x levers

1. **[P0 · security/availability] A handful of concurrent bad-password `POST /api/login` requests
   can stall the entire server, including log/trace ingest.** `auth.rs:161-168` runs argon2 verify
   inline on the async handler (no `spawn_blocking`), and that handler runs on the *same* tokio
   worker pool that serves OTLP gRPC/HTTP ingest (`main.rs:228`). Argon2 is deliberately
   CPU/memory-heavy; a burst of logins against a guessable username (the docs' own dev default is
   `admin`/`admin`) with wrong passwords saturates every worker thread hashing, starving ingest and
   query handling for the whole process. No rate limit/lockout exists on `/api/login` either.
   Contrast: `photon-query` already `spawn_blocking`s its own CPU-bound pruning
   (`photon-query/src/lib.rs:315`) — this is an established, just inconsistently-applied, pattern.

2. **[P0 · memory/crash] Several query params that size a `Vec` allocation are client-controlled
   with no upper bound.** `histogram.rs:19-25` (`buckets`, default 48) and `traces_agg.rs:103-115`
   (`buckets`, shared by histogram + latency) pass the raw `usize` straight to
   `QueryEngine::histogram`, which does `(0..buckets).map(...).collect()` — `buckets=2_000_000_000`
   attempts a multi-gigabyte-to-terabyte allocation, which the global allocator typically **aborts
   the whole process** on rather than raising a catchable error. `search.rs:38-44`'s `limit` (u64,
   default 500) and `facet.rs:18-24`/`traces_agg.rs:57-63`'s `limit` (usize, default 50) have the
   same gap. Contrast: `metrics.rs:23-24,58-67` already clamps to `MAX_BUCKETS = 3000`, and
   `query_params.rs:139` already clamps spans `limit` to `.min(1000)` — the fix pattern already
   exists in this same crate, just not applied to logs histogram/facet or traces histogram/latency.

3. **[P1 · speed+availability] Every authenticated request pays a synchronous SQLite call, and one
   panic anywhere poisons it for the rest of the process's life.** `require_auth`
   (`auth.rs:251-263`) calls `UserStore::get`, which is `SqliteUserStore::get`
   (`users.rs:123-135`) — `self.conn.lock().unwrap()` over a plain `std::sync::Mutex<Connection>`,
   run synchronously inside an `async fn` with no `spawn_blocking`. This executes on **every**
   protected-route request (search, traces, metrics, everything), on the same shared runtime as
   ingest. Every `UserStore`/`SettingsStore` method uses the same `.lock().unwrap()` — if any one
   of them ever panics while holding the lock, the `Mutex` is poisoned and *all* future
   `.lock().unwrap()` calls panic too, i.e. `require_auth` permanently 500s/resets every request
   until the process restarts.

4. **[P1 · availability] Background compactor/uptime tasks have no supervision.** `main.rs`'s
   `spawn_compactor` (268-325), `spawn_span_compactor` (330-375), `spawn_metric_compactor`
   (380-425), and `spawn_uptime`'s scheduler/prune tasks (452, 458-473) are bare `tokio::spawn`
   with the `JoinHandle` dropped. Ordinary `Err`s are already logged and looped past (good) — but
   an actual **panic** silently ends that task forever (no restart, no alert); for a compactor that
   means the signal's WAL is never drained again and disk fills, per the brief's own "ingest fills
   disk" scenario. `Cargo.toml` sets no `panic = "abort"`, so this fails *quietly*, not loudly.

5. **[P2 · speed, quick win] No response compression, no static-asset caching.** `into_router()`
   (`lib.rs:139-207`) adds no `tower_http` layer at all (not a dependency of this crate). Every
   `/api/search`/`/api/traces/search`/`/api/spans/search` JSON payload and every embedded JS/CSS
   asset ships uncompressed; `assets.rs:41,54` additionally does `content.data.into_owned()` —
   copying the full asset bytes out of the embedded `&'static` binary on **every single request** —
   and sets no `Cache-Control`/`ETag`, so browsers can't skip re-fetching either. Cheap to fix, high
   value for "10x lighter" over a WAN link or a busy dashboard doing live-tail polling.

---

## Findings (ranked)

### P0 · security/availability · `/api/login` argon2 verify blocks the shared ingest/query runtime
- **Where:** `auth.rs:156-170` (`login`), `:265-273` (`verify_password`, calls `Argon2::default().verify_password` directly); same pattern in `setup` (`:131-153`, `hash_password_prod` at `:145`) and `create_user` (`:193-215`, `:207`). Runtime sharing: `photon-server/src/main.rs:46` (`#[tokio::main]`, default `rt-multi-thread` worker count = num CPUs) and `:228` (`tokio::try_join!(ingest.serve(...), api.serve(...))` — both front ends share one worker pool, not two runtimes).
- **What:** Argon2id hashing/verification (the default profile in the `argon2` crate is intentionally slow — tens of milliseconds of CPU + several MiB of memory per call) executes synchronously inside an `async fn` handler. Nothing hands it to `spawn_blocking` or a bounded blocking pool.
- **Why it matters:** A tokio multi-thread runtime has `N` (≈ CPU core count) worker threads shared by *every* task, including the OTLP gRPC/HTTP ingest handlers and every query handler. A CPU-bound synchronous call inside an async fn doesn't yield — it occupies a worker thread for its whole duration. `N` concurrent `POST /api/login` attempts (even with the *wrong* password, as long as the *username* exists — `login`'s `Ok(None) => false` path skips hashing, but a real username like the documented dev default `admin` does not) can occupy every worker thread simultaneously, so ingest requests queue behind login attempts. `/api/login` is intentionally open (pre-auth), so this is reachable by anyone who can hit the API port, with no valid credentials required — a trivial DoS against the whole platform, not just auth.
- **Fix:** Wrap the `Argon2::...hash_password`/`verify_password` calls in `tokio::task::spawn_blocking` (mirrors `photon-query`'s existing pattern at `photon-query/src/lib.rs:306-317`, which has a doc comment explicitly justifying this for the same reason). Optionally also cap concurrent login attempts (a `tokio::sync::Semaphore` sized to leave headroom for ingest, or a per-IP rate limit) since even off the main pool, unbounded concurrent argon2 work still burns all CPU.
- **Effort/Risk:** S — the four call sites are already small, isolated functions; wrapping is mechanical.
- **Invariant check:** Doesn't touch either auth system's semantics (bearer token vs. session), just where the CPU work runs.

### P0 · memory/crash · Unbounded `buckets`/`limit` query params size unchecked allocations
- **Where:** `histogram.rs:19-25` (`HistogramParams.buckets`, default 48, no max) → `photon-query`'s `empty_buckets`/`histogram_over` allocate `Vec<HistogramBucket>` sized exactly `buckets`. `traces_agg.rs:103-115` (`TracesHistogramParams.buckets`, shared by `traces_histogram:119-149` and `traces_latency:163-187`) — same gap, same downstream allocation pattern (`span_histogram.rs`/`span_latency.rs`, not read here but same shape). `search.rs:27-44` (`SearchRequest.limit: u64`, default 500, no max — passed straight through at `search.rs:176`/`:90`). `facet.rs:11-24` and `traces_agg.rs:50-63` (`limit: usize`, default 50, no max — passed straight through at `facet.rs:36`/`traces_agg.rs:83`, and `photon-query/src/facet.rs`'s `.limit(0, Some(limit + 1))` pulls that many rows into a `Vec<FacetValue>` regardless).
- **What:** None of these five params are clamped at the API boundary before reaching the query engine. Two other places in this exact crate already solve this correctly: `metrics.rs:23-24` defines `MAX_BUCKETS: usize = 3000` and `buckets_for` (`:58-67`) clamps to it; `query_params.rs:123-143`'s `build_span_query_request` does `limit.min(1000)` at `:139`. Neither pattern is applied to the logs histogram/facet/search handlers or the traces histogram/latency handlers.
- **Why it matters:** `buckets=2_000_000_000` (a `usize`, so any value up to `u64::MAX` on a 64-bit host deserializes fine — `serde`/axum's `Query` extractor has no schema-level bound) tries to allocate a `Vec` of that many `HistogramBucket` structs. A failed allocation of that size is not a `Result` a handler can catch — Rust's default global allocator calls `handle_alloc_error`, which aborts the **entire process** (not just the request), taking down ingest with it. `limit` similarly controls how many full rows (each with a `serde_json::Map` of attributes — see the next finding) get pulled into RAM and then serialized into one response body.
- **Fix:** Add the same `MAX_BUCKETS`/`.min(N)` clamp used in `metrics.rs`/`query_params.rs` to `histogram.rs`, `traces_agg.rs` (both the facet limit and the histogram/latency buckets), `facet.rs`, and `search.rs`'s `SearchRequest.limit`. A single shared helper (e.g. `clamp_buckets`/`clamp_limit` in `query_params.rs`, since that module is already the shared home for exactly this kind of cross-cutting param validation) avoids repeating five ad hoc constants.
- **Effort/Risk:** S — pure input validation, no behavior change for well-behaved clients already under the informal defaults.
- **Invariant check:** None of the load-bearing invariants are touched; this is purely a missing-validation gap at the HTTP boundary the invariants don't cover.

### P1 · speed+availability · Sync SQLite-over-Mutex on every request; poisoning turns one panic into a permanent outage
- **Where:** `auth.rs:251-263` (`require_auth`, calls `state.users.get` on **every** protected-route request) → `users.rs:115-162` (`count`/`get`/`list`/`create`/`delete`, each `self.conn.lock().unwrap()` over `Mutex<rusqlite::Connection>`); `settings.rs:63-87` (`get_retention`/`set_retention`, same pattern).
- **What:** Two separate problems compound: (a) synchronous file-backed SQLite I/O runs inline in an `async fn`, never `spawn_blocking`'d, on the same shared runtime flagged above (this fires on every single UI request via the auth gate, not just login — much more frequent than the argon2 path); (b) every method does `.lock().unwrap()`, so a poisoned `std::sync::Mutex` (from *any* panic anywhere while the lock is held — including a future contributor's bug, not necessarily one that exists today) makes every subsequent call panic too, and since `require_auth` gates literally every protected route, that's a full, permanent, silent 401/reset storm until someone restarts the process.
- **Why it matters:** SQLite point lookups are normally sub-millisecond, so (a) alone is a minor tax — but it's a *tax paid on every request*, on the runtime ingest also depends on, and it's disk I/O (can stall on a slow/contended disk, e.g. the shared control-plane DB file this store shares with the uptime store per `users.rs`'s own doc comment). (b) is the sharper edge: a `Mutex` poisoning failure mode here has an outsized blast radius specifically *because* `require_auth` is a single chokepoint for the whole API.
- **Fix:** (a) Wrap the rusqlite calls in `spawn_blocking`, or — since these are tiny, frequent, latency-sensitive calls — run them on a single dedicated blocking thread behind an mpsc request/reply channel (the same shape `docs/perf-audit/07-uptime-loadgen.md:298` already recommends for the sibling uptime store; `users`/`settings` share the exact same SQLite-behind-a-Mutex shape and should get the same treatment, or literally the same worker thread since they can share the DB file/connection). (b) Replace `.lock().unwrap()` with a small helper that recovers from poisoning (`.unwrap_or_else(|e| e.into_inner())`) — SQLite's on-disk state isn't corrupted by an unrelated Rust panic elsewhere, so recovering the guard is safe and prevents one bug from cascading into a permanent auth outage.
- **Effort/Risk:** (a) M — touches every method of two stores; (b) S — a one-line helper, mechanical find/replace.
- **Invariant check:** No change to the two-auth-systems boundary; this is purely how the session-cookie store is accessed.

### P1 · availability · Background compactor/uptime tasks are unsupervised `tokio::spawn`s
- **Where:** `main.rs:268-325` (`spawn_compactor`), `:330-375` (`spawn_span_compactor`), `:380-425` (`spawn_metric_compactor`), `:429-480` (`spawn_uptime` — the scheduler task at `:451-454` and the hourly retention-prune task at `:457-473`). All five are `tokio::spawn(async move { loop { ... } })` with the returned `JoinHandle` immediately dropped.
- **What:** Ordinary errors inside the loops are already handled well — `Err(e) => eprintln!(...)` then the loop continues (this is exactly right for e.g. a transient I/O error). But there is no `catch_unwind`/supervisor around the task, and the `JoinHandle` is never awaited or checked. If the task body ever *panics* (an `unwrap`/`expect`/index-out-of-bounds several layers down in Arrow/DataFusion/Parquet code, say), the task simply ends; nothing restarts it, nothing pages an operator, and (per `Cargo.toml`, no `panic = "abort"` anywhere) the *process itself keeps running* looking healthy while one compactor is now permanently dead.
- **Why it matters:** This is precisely the scenario the design brief calls out: "compactor stops → ingest fills disk." The WAL keeps accepting writes (ingest is a separate component from the compactor), but nothing ever drains a dead compactor's closed segments into Parquet again — unbounded local-disk growth on a system whose core pitch is single-node efficiency, and it fails *silently* (a stderr line from the panic hook, not a health-check-visible state).
- **Fix:** Wrap each loop body in `std::panic::AssertUnwindSafe(...)` + `futures::FutureExt::catch_unwind` (or simpler: a small `spawn_supervised(name, f)` helper that loops `tokio::spawn(f()).await`, logs+re-spawns on `Err` from the `JoinHandle`, with a backoff) so a panic becomes "log loudly and restart the loop" instead of "vanish." Cheap because the existing loop bodies don't need to change — only the top-level `tokio::spawn` wrapper.
- **Effort/Risk:** S/M — one shared helper, applied at 5 call sites; no change to compaction logic itself.
- **Invariant check:** None of the compaction/replication invariants change; this only affects what happens after an already-abnormal panic.

### P1 · speed (cross-ref) · `/api/search` and `/api/spans/search` each run prune+scan twice
- **Where:** `search.rs:90` (`state.query.search(query.clone())`) then `:100-104` (`state.query.count_matching(query)`); `traces_search.rs:181` (`state.span_query.search_spans(query.clone())`) then `:188-192` (`state.span_query.count_matching_spans(&query)`).
- **What:** Both handlers call the row-fetch and the count separately, each re-running the full prune (manifest + skip-index) and re-opening every surviving Parquet file. `docs/perf-audit/02-logs-query-and-index.md` already documents this in depth for the logs path (`search_with_count` exists in `photon-query` and does the equivalent work in one prune, but has zero callers) — the root fix lives in `photon-query`, out of this audit's crate scope, but **the call sites causing the duplicate work are in this crate** (`search.rs`, and the same shape in `traces_search.rs`'s `spans_search`, which 02 doesn't cover since it's the spans sibling).
- **Why it matters:** Doubles prune I/O and DataFusion planning/execution per user search — see 02 for the detailed cost breakdown.
- **Fix:** Call `search_with_count` (logs) and its spans equivalent (add one if it doesn't exist yet for spans) from `search.rs`/`traces_search.rs` instead of the two independent calls.
- **Effort/Risk:** S — near drop-in once the engine-side method is confirmed for both signals.
- **Invariant check:** N/A, output-preserving.

### P2 · speed · No compression; static assets re-copied every request with no cache headers
- **Where:** `lib.rs:139-207` (`into_router`, no `tower_http` layer of any kind — `tower_http` isn't even a dependency of `photon-api`). `assets.rs:36-46` (`static_handler`, `content.data.into_owned()`) and `:49-59` (`index_html`, same).
- **What:** `rust_embed::EmbeddedFile::data` is `Cow<'static, [u8]>`; in a release build the embedded bundle is a `Cow::Borrowed(&'static [u8])`, so `.into_owned()` unconditionally heap-allocates a fresh copy of the *entire file's bytes* on every request, discarding the zero-copy option (`Bytes::from_static` for the `Borrowed` case). No `Cache-Control`/`ETag`/`Last-Modified` header is set, so the browser can't skip re-fetching either. Separately, no response-compression layer exists, so JSON search/traces/spans payloads and the JS/CSS bundle both ship uncompressed.
- **Why it matters:** For "10x lighter," a several-hundred-KB-to-few-MB JS/CSS bundle being fully re-copied and fully re-transmitted uncompressed on every page load (and on every asset request within a load) is pure waste; large `/api/search`/`/api/traces/search` JSON bodies (highly compressible text) pay full bandwidth + serialization-to-wire time uncompressed too. This is also one of the cheapest fixes in the whole audit.
- **Fix:** Add `tower-http` (`compression-gzip`/`compression-br`, `catch-panic` — see next finding — as needed) as a dependency; `.layer(CompressionLayer::new())` on the router in `into_router`. For assets, match on `content.data` and use `Bytes::from_static` for the `Cow::Borrowed` case (avoiding the copy), and add `Cache-Control: public, max-age=31536000, immutable` for content-hashed asset paths (Vite fingerprints filenames) plus `Cache-Control: no-cache` for `index.html` (so SPA updates are picked up).
- **Effort/Risk:** S — one dependency + a few lines; well-trodden.
- **Invariant check:** None.

### P2 · memory · Full `Vec<serde_json::Value>` tree + full in-memory response buffering for row endpoints
- **Where:** `search.rs:182-270` (`batches_to_rows`/`row_to_json`), `traces.rs:78-204` (`spans_to_json`/`span_row_to_json`), `traces_search.rs:80-90` (`span_batches_to_rows`) — all three build a `Vec<serde_json::Value>` (each row a fresh `serde_json::Map`, i.e. a `BTreeMap<String, Value>` since this workspace doesn't enable `serde_json`'s `preserve_order`, plus a `String` allocation per attribute key *and* value) and hand the whole thing to axum's `Json(...)`, which calls `serde_json::to_vec` on the entire structure at once before the first response byte is written.
- **What:** This is exactly the pattern the audit brief asks about: `RecordBatch` → per-row `Value`/`Map` intermediate (heavy small-allocation churn) → one fully-materialized response body, rather than serializing Arrow arrays directly into the output writer.
- **Why it matters:** For the currently-effective row caps (500 logs / 100 traces / 200 spans, still informal defaults, not enforced maxima — see the P0 finding above) this is a moderate, bounded cost. It becomes severe exactly when combined with the missing `limit` clamp: an uncapped `limit` turns a "some extra allocations" issue into "the whole response, plus its `Value`-tree scaffolding, sits in RAM at once, with no bound." Peak memory during a large search is at minimum `decoded RecordBatches + Value tree + serialized Vec<u8>`, all alive simultaneously.
- **Fix (once the limit cap above is in place, this becomes lower priority):** Consider a `Serialize` impl that walks the `RecordBatch` columns directly into a `serde_json::Serializer` (writing straight into the response body writer) instead of building `Vec<Value>` first — this is the standard fix for "Arrow batch → JSON HTTP response" and would apply uniformly to `search.rs`/`traces.rs`/`traces_search.rs` (three near-identical `row_to_json` implementations already invite a shared abstraction). Streaming the body (chunked/NDJSON) is a further step but a bigger API contract change (the current envelope carries `matched_count`/`elapsed_ms` alongside `rows`, which wants the total count known before the rows are framed — doable with NDJSON if the count line comes first, but changes the wire format the frontend consumes).
- **Effort/Risk:** M — the direct-serialize refactor touches three files' conversion functions but not their call sites; true streaming is L and a frontend-coordinated change.
- **Invariant check:** None of photon's load-bearing invariants apply to the API's serialization strategy; this is purely about not amplifying the (already fixed, see above) `limit`/`buckets` caps into worse-than-necessary memory use.

### P2 · security · Login timing side-channel enables username enumeration
- **Where:** `auth.rs:161-165` (`login`): `Ok(None) => false` returns immediately for an unknown username; `Ok(Some(u)) => verify_password(...)` (an ~tens-of-ms argon2 call) only runs for a *known* username.
- **What:** Response latency for "unknown username" is near-zero; for "known username, wrong password" it's however long argon2 verify takes. An attacker can distinguish the two by timing, enumerating valid usernames without ever needing a correct password.
- **Why it matters:** Low severity for a small internal-tool user base, but it's a textbook-avoidable leak and the fix is nearly free.
- **Fix:** When the username isn't found, still run `verify_password` against a fixed dummy PHC hash (computed once, e.g. via `once_cell`/`OnceLock`) so the constant-ish argon2 cost is paid on both branches.
- **Effort/Risk:** S.
- **Invariant check:** None.

### P2 · security · Session cookie has no `Secure` flag
- **Where:** `auth.rs:63-71` (`set_session`): sets `HttpOnly` + `SameSite=Lax` + a 7-day `Max-Age`, but never calls `.set_secure(true)`.
- **What:** Without `Secure`, the browser will send `photon_session` over a plaintext `http://` connection too, if one exists (e.g. someone reaches the API port directly instead of through a TLS-terminating reverse proxy).
- **Why it matters:** Photon is a self-hosted single binary; nothing in `photon.example.toml`/CLAUDE.md mandates a TLS-terminating proxy in front of it, so a deployment that binds `0.0.0.0:8080` directly to an untrusted network segment ships session cookies in the clear.
- **Fix:** Set `.set_secure(true)` unconditionally (the common case is a reverse proxy or direct HTTPS; for a pure-HTTP LAN deployment this is a defense-in-depth cost worth paying), or gate it on a config flag if plain-HTTP-only deployments are an intentional supported mode.
- **Effort/Risk:** S — one line, but worth confirming against the plain-HTTP dev workflow (`make dev`) so local development doesn't break (browsers still send `Secure` cookies over `http://localhost` in most current browsers' special-casing of localhost, but double-check).
- **Invariant check:** None.

### P3 · correctness · `Key::derive_from` can panic the whole server at startup on a short secret
- **Where:** `lib.rs:100` (`ApiServer::new`: `Key::derive_from(session_secret.as_bytes())`), fed from `main.rs:220` (`&cfg.auth.session_secret`).
- **What:** `cookie::Key::derive_from` (vendored source confirmed) panics if given fewer than 32 bytes. `photon-core/src/config.rs:167-169` only validates that `session_secret` is non-empty, not that it meets this length floor — already flagged from the config side in `docs/perf-audit/05-core-domain.md:165-172`; noting it here because the actual panic site is in this crate's constructor, not config validation, and it currently has zero defense at the call site that would turn this into the same clean `PhotonError::Config` path the empty-string case already uses.
- **Why it matters:** A plausible operator mistake (a short placeholder secret, e.g. copy-pasting a partial value) doesn't fail with "auth.session_secret must be set" — it crashes the entire binary with a `cookie` crate panic message unrelated to config validation, at a point after the WAL/compactor are already constructed.
- **Fix:** Extend the existing `Config` validation (05's recommendation) to check `session_secret.len() >= 32`; that alone closes this call site too since it never sees a too-short value.
- **Effort/Risk:** S (in `photon-core`, out of this crate's edit surface, but the call site here would then never trip).
- **Invariant check:** None.

### P3 · availability · No graceful shutdown wiring
- **Where:** `main.rs:228` (`tokio::try_join!(ingest.serve(grpc_addr, http_addr), api.serve(api_addr))`); `lib.rs:127-135` (`ApiServer::serve`, plain `axum::serve(listener, app).await`, no `.with_graceful_shutdown(...)`).
- **What:** Neither server installs a `tokio::signal` handler or graceful-shutdown hook. A `SIGTERM` (container/orchestrator stop, `systemctl stop`, etc.) hard-kills the process via the OS default action, mid-flight requests included.
- **Why it matters:** In-flight WAL writes are still safe (the ack boundary is the fsync, already durable), but in-flight *HTTP responses* (a search mid-serialization, a still-connecting ingest client) are simply dropped rather than allowed to finish — rough edges on every restart/redeploy, and no bounded shutdown window for the background compactor loops to finish a `run_once` cleanly either.
- **Fix:** Add `axum::serve(...).with_graceful_shutdown(shutdown_signal())` (a `tokio::signal::ctrl_c()`/SIGTERM future) to both servers, and have the compactor loops select on the same shutdown signal to finish their current tick before exiting.
- **Effort/Risk:** M — touches both serve paths and the compactor loop's `select!`.
- **Invariant check:** None; purely additive.

### P3 · speed, minor · `/api/metrics/query` has no cap on the number of sub-queries per request
- **Where:** `metrics.rs:111-129` (`MetricsQueryRequest.queries: Vec<QuerySpec>`), looped sequentially at `:158-205`.
- **What:** No maximum on `queries.len()`; a single POST can specify an arbitrarily large batch of independent series queries, each triggering its own `query_series` DataFusion execution, sequentially, within one request/response cycle.
- **Why it matters:** Minor compared to the P0/P1 items above (bounded by request body size, and DataFusion work per query is itself bounded by the already-clamped `MAX_BUCKETS`), but it's an easy amplification vector and has no legitimate UI use case beyond a handful of overlaid series.
- **Fix:** Clamp `queries.len()` to a small constant (e.g. 20) with a 400 on overflow, mirroring the buckets/limit clamps recommended above.
- **Effort/Risk:** S.
- **Invariant check:** None.

---

## Quick wins (low-effort, high-impact)

- Clamp `histogram.rs`'s and `traces_agg.rs`'s `buckets`, and `search.rs`'s/`facet.rs`'s/
  `traces_agg.rs`'s `limit`, to the same style of constant `metrics.rs`/`query_params.rs` already
  use. (P0 finding #2 — this alone closes the crash vector.)
- `spawn_blocking` the four argon2 call sites in `auth.rs` (login/setup/create_user/verify_password).
  (P0/P1 finding #1.)
- Add `tower-http` with `compression-*` (and `catch-panic` as a cheap defense-in-depth net for any
  future handler panic) and wire `CompressionLayer`/`CatchPanicLayer` into `into_router`. (P2 #5.)
- Replace `.lock().unwrap()` in `users.rs`/`settings.rs` with a poison-recovering helper. (P1 #3,
  the "b" half — the `spawn_blocking` half is more involved.)
- Fix the dummy-hash timing side-channel in `login`. (P2, security hygiene, near-free.)
- Set `.set_secure(true)` on the session cookie. (P2, one line — verify against `make dev`'s
  plain-HTTP flow first.)

## Bigger bets (architectural)

- Move `UserStore`/`SettingsStore` SQLite access off the shared async runtime entirely — either
  `spawn_blocking` per call, or (better, since these are small/frequent/latency-sensitive) a
  dedicated OS thread behind an mpsc request/reply channel, exactly as `07-uptime-loadgen.md`
  already recommends for the sibling uptime store; all three stores share the same
  Mutex-over-rusqlite shape and the same control-plane DB file, so one fix likely serves all three.
- Add a small task-supervision helper (`catch_unwind` + restart-with-backoff around each
  `tokio::spawn` loop) and apply it to the 3 compactor loops + the 2 uptime tasks, so a panic
  degrades to "logged and restarted" instead of "silently dead forever."
- Replace the `Vec<Value>` intermediate in `search.rs`/`traces.rs`/`traces_search.rs`'s row
  conversions with direct `RecordBatch` → JSON-writer serialization (no `serde_json::Value` tree);
  consider true response streaming (NDJSON/chunked) for the row endpoints once the hard limits above
  bound worst-case size, so peak memory scales with what's actually being sent rather than requiring
  the whole body to be built first.
- Add graceful shutdown across both front ends and the compactor loops.

## Already good / no action

- `metrics.rs`'s `MAX_BUCKETS = 3000` clamp and `query_params.rs`'s spans `limit.min(1000)` clamp
  are exactly the right pattern — they just need to be copied to the other four endpoints flagged
  above, not invented.
- axum's `Json`/`Bytes` extractors already enforce a **built-in 2 MB default body-size limit**
  (verified against axum's docs) — no additional request-body cap is needed for `/api/login`,
  `/api/search`, etc.; large-payload risk here is all on the *response* side, not the request side.
- `require_auth` correctly revokes sessions the instant a user is deleted (checks the live store,
  not just cookie signature validity) — well covered by `require_auth_rejects_cookie_for_deleted_user`.
- All persistence access goes through bound rusqlite `params!`/DataFusion expression trees, never
  string-built SQL or string-built DataFusion predicates — no injection surface anywhere in this
  crate, in either the grammar path or the SQLite path.
- `data.rs`'s retention/purge endpoints validate their small enum-like inputs (`mode`, per-signal
  `days > 0`, the uptime-enabled gate) tightly and route mutations through the single owning
  compactor per signal via an mpsc + oneshot reply — a clean design, no notes.
- `QueryEngine`'s manifest/services caching (pointer-identity invalidation, referenced from
  `search.rs`'s `services()`) avoids a manifest re-read on every `/api/services` call — this lives
  in `photon-query` (out of this crate's scope) but the call site here benefits from it correctly.
- Route ordering between `/api/traces/:trace_id` and the more specific `/api/traces/fields`,
  `/api/traces/search`, etc. is deliberately tested (`traces_agg.rs`'s
  `traces_fields_route_does_not_collide_with_trace_id_route`) — a real footgun in path-param
  routers, already guarded.

## Open questions & NEEDS-BENCH

- **NEEDS-BENCH:** actual wall-clock cost of one argon2 verify under this crate's `Argon2::default()`
  params, and how many concurrent logins it takes in practice to visibly stall ingest throughput on
  a representative core count — the P0 #1 finding is architecturally sound but the exact blast
  radius (how many bad logins, how much ingest latency) is unmeasured.
- **NEEDS-VERIFY:** whether a handler panic today is actually isolated to one connection (rather
  than crashing the process) — reasoned from tokio/axum's per-connection-task architecture (a panic
  during polling unwinds only that spawned task), but not exercised with a live fault-injection test
  in this codebase. `CatchPanicLayer` is recommended regardless as a defense-in-depth measure that
  also gets a clean 500 + log line instead of a bare connection reset.
- **NEEDS-BENCH:** peak RSS for a single `/api/search` response at a large `limit` (e.g. 100k rows)
  once the P0 clamp is fixed and raised to a deliberately generous cap — to decide whether the P2
  `Value`-tree-removal refactor is worth doing now vs. deferring until a concrete OOM/latency
  complaint surfaces.
