# Service rollup

A background pass that reads spans and publishes per-service and per-operation RED as **ordinary
metrics**, so the Backend pages can read a wide window without re-scanning raw spans.

It adds no storage engine. There is no rollup table, no fourth Parquet tier, no new WAL — the pass
writes `MetricPoint`s into the existing metrics WAL and everything downstream (compaction,
retention, the `/metrics` explorer, the alert engine, federation) treats them as metrics, because
they are. See [`docs/adr/0002`](../adr/0002-red-rollups-are-published-as-metrics.md).

**Key files**

| File | What it is |
|---|---|
| `crates/photon-query/src/red_rollup.rs` | The query: `SpanQueryEngine::red_rollup` → `RollupPass` |
| `crates/photon-server/src/rollup/mod.rs` | The pass loop, window arithmetic, cardinality logging |
| `crates/photon-server/src/rollup/points.rs` | `RollupRow` → `MetricPoint`s (the wire contract) |
| `crates/photon-server/src/rollup/watermark.rs` | The published-through marker + the latency-ladder row, in control-plane SQLite |
| `crates/photon-server/src/rollup/otlp.rs` | `MetricPoint` → OTLP, for shipping to a federation central |
| `crates/photon-api/src/services_rollup.rs` | The **read** side: when to trust the rollup, and how to read it |
| `crates/photon-server/tests/rollup.rs` | Spans in, metrics out, read back through the metrics engine |

## What it publishes

Two metric families, one per level. Every metric carries `service.name` and (when the service sets
it) `deployment.environment`; the operation family additionally carries `operation`.

| Metric | Kind | Unit | Meaning |
|---|---|---|---|
| `photon.service.requests` | Sum, delta | `{requests}` | Entry spans — requests served |
| `photon.service.spans` | Sum, delta | `{spans}` | Every span, all kinds — work recorded |
| `photon.service.errors` | Sum, delta | `{errors}` | Entry spans with `status_code == 2` |
| `photon.service.http_4xx` / `.http_5xx` | Sum, delta | `{responses}` | By HTTP status class |
| `photon.service.apdex.satisfied` / `.tolerating` / `.frustrated` | Sum, delta | `{requests}` | Apdex bands |
| `photon.service.latency` | Histogram, delta | `ns` | Explicit-bucket latency distribution |
| `photon.service.basis` | Gauge | `1` | Present only when the count is **not** entry spans |

`photon.operation.*` mirrors all of the above except `basis`, which is a service-level fact.

### The two families are deliberately not one

`metric_query`'s bucket fold is `sum(value)` over every series matching the metric name. If both
levels shared `photon.service.requests`, a bare query for it would add the per-operation series to
the per-service one and report a service serving roughly twice its real traffic. Separate names make
`photon.service.requests` mean exactly one thing, with no filter required to get it right.

### `requests` is entry spans, and `spans` rides beside it

Per [`ADR-0003`](../adr/0003-a-services-request-rate-is-its-entry-spans.md), a service's request rate
is its **entry** spans (`SERVER`/`CONSUMER`), with a per-series cascade down to trace roots and then
to all spans when a service emits neither. `photon.service.basis` fires when a service landed on one
of the fallbacks, so the UI can badge it without a second query.

**An operation inherits its service's basis** rather than resolving its own. Left to its own
cascade, a `CLIENT`-only operation like `SELECT users` has no entry spans and no roots, so it would
fall through to "all spans" and report every outbound database call as a request the service served.
Inheriting means it reports `0` requests and its real `spans` count — and it means the operation
series sum exactly to their service, which is what makes an operation table and `__other__`
trustworthy.

### Only local spans

Spans carrying a `tenant` attribute arrived from a Site, and that Site rolls up its own services and
ships the result. Rolling them up again on Central would double-count every federated service, so
the pass filters them out.

## Each bucket is published exactly once

Buckets are 60 seconds wide, on whole-minute boundaries. Bucket `[T, T+60)` is published on the
first pass where `now >= T + 60 + lag_secs`, and **never revisited**.

This is the part most likely to be "fixed" by the next reader, so: the pass cannot simply re-roll
its recent window every minute. That approach only works if the reader de-duplicates by
`(series, timestamp)`, and Photon's does not — `metric_query` sums every point that lands in a
bucket. A bucket re-emitted on ten successive passes would report ten times its real traffic.

So the pass keeps a **watermark**, one row in the control-plane SQLite DB, holding the newest bucket
start already written. It never moves backwards, and it survives a restart — without it, a restart
would republish what the previous process had already written.

**The one inaccuracy:** a span that arrives later than `lag_secs` after its bucket closed never
reaches the rollup. `lag_secs` therefore buys exactly what it costs. The default of 600s covers a
collector retry storm; raising it delays every Backend page's rollup-backed view by the same amount.

**Delivery is at-least-once.** A crash between the WAL append and the watermark update republishes
one bucket. The alternative — marking first — would drop a bucket that was actually written, and a
silent hole is harder to notice than a single doubled minute.

## It backfills its own history on first run

Without a backfill the rollup can only describe the future, and that is not the state most installs
start in. A fresh control-plane DB over an existing hot tier, an upgrade from a build without the
rollup, a node re-seeded from a replica — each has months of spans on disk and reports no coverage
for any of it, so every Backend page over a window wider than "since we started" falls back to
scanning raw spans. That fallback is correct and it is slow in exactly the way the rollup exists to
prevent: measured on 300M spans, `/api/services/overview` took 23.1 s over 7 days and 3.4 s over
24 hours from spans, against 63 ms off the rollup.

So a first run reaches `backfill_days` back (default 30, matching the default `[retention].days`)
and walks forward, one `CATCH_UP_SLICE_SECS` (1 hour) slice at a time, writing the watermark after
each. It is resumable — a restart mid-catch-up continues where it stopped — and bounded by what is
on disk, since slices whose spans have aged out prune to nothing and cost a manifest lookup.

The default is deliberately **not** "the widest window the UI offers". Coverage starts at
`now - backfill_days` and the widest built-in window *ends* at now, so sizing the two the same puts
every 7-day query exactly on the boundary: a query starting a minute earlier than coverage is
ineligible and silently falls back to a raw scan. Over-reaching is cheap; under-reaching is not.

The slice cap is the other half. A pass reads raw spans, so an uncapped catch-up after a long
outage would put a single multi-day span scan on the read path and write one enormous metrics
batch.

## The latency ladder, and repairing a rename

`LATENCY_BOUNDS_NS` is a wire contract (see below), so renumbering it requires renaming
`photon_query::LATENCY_METRIC` in the same change. That is how the old 10s ladder was retired: it
topped out at 10s, so every span slower than that landed in the `+Inf` overflow bucket and the
quantile read interpolated at the top bound — a service whose real p50 was two minutes reported
**exactly 10.000 s**, silently, with nothing marking the number as a floor. The ladder now runs to
30 minutes in 19 buckets, and the metric is `photon.service.latency` / `photon.operation.latency`
(it was `…duration`).

The rename does its job and leaves a second problem behind: every bucket already published still
carries its histogram under the retired name, so a Backend page over that range shows no
percentiles at all. `RollupTask::repair_latency` closes that gap. A row in
`rollup_latency_ladder` records which name the histogram is published under and how much of the
covered range still has to be republished; when the recorded name does not match the current one
(including when there is no row at all, which is how an install upgrading from before this
bookkeeping looks), the whole covered range is queued and republished slice by slice, ahead of the
forward pass on each tick.

**Only the histogram is republished.** The counters under those buckets are still correct, and
re-emitting them would double every one of them. The histogram is exempt from the publish-once rule
for one specific reason: a quantile is scale-invariant, so a bucket whose histogram somehow landed
twice reports the same p50, p90 and p99 as one that landed once. What a double landing does change
is the absolute sample count — which is why the repair records its own progress watermark
(`repair_from`, forward-only) instead of relying on that safety net.

**The repair ignores tenant coverage, for the same reason.** A forward pass skips any tenant already
pushing its own rollup (below), because rolling its mirrored spans up beside its own points would
double every counter. A repair emits no counter, so `Publish::LatencyOnly` rolls up every tenant.
Without that a federation **central** could not repair at all: it holds a mirrored tenant's spans
and deliberately never rolls them up, so the repair would find nothing to publish and every Backend
page for that tenant would show no percentiles until the Site upgraded, repaired and re-pushed.
Central heals its own history instead. It does **not** heal the present: the forward pass still
skips covered tenants, so a central whose Site is on the old build has percentiles for the repaired
range and none after it. Upgrading the Site is the fix; the Site's own repair tees the republished
histogram to central.

## Cardinality

Service level is always rolled up in full: services are few and bounded by how many things you run.

Operation level is not. An operation name carrying an id (`GET /v1/charges/ch_1a2b3c`) makes distinct
operations scale with distinct **requests**, and `metric_query` caps a query at `MAX_SERIES = 1000`.
So the pass keeps the busiest `top_operations` per `(service, environment)` and sums everything else
into a single `operation = "__other__"` series.

Three properties make that safe rather than merely small:

- **Top-N is per service, not global.** A single high-cardinality service cannot starve every other
  service out of the operation level.
- **`__other__` is aggregated, not subtracted.** The fold happens in the `GROUP BY`, so the
  remainder is summed over its own rows with its own counters — it is exact even when discovery was
  truncated, and the operation series still sum to their service.
- **Nothing is dropped silently.** A pass that folds anything logs how many; a pass whose discovery
  hit its own cap says so separately.

`__other__` is a remainder, not an endpoint. The UI must render it as such — it is not something a
person can click into.

## Reading it back

`photon-api/src/services_rollup.rs` is the only reader. It is a shortcut with a raw-span fallback,
never a source of truth — see [`services-apm.md`](services-apm.md) for the conditions a request must
meet, the **hybrid split** that reads the covered prefix off the rollup and the unsettled tail off
raw spans, and the `source` field every response carries.

## Config

`[rollup]`, all optional. Env: `PHOTON_ROLLUP_ENABLED`, `_INTERVAL_SECS`, `_LAG_SECS`,
`_TOP_OPERATIONS`, `_BACKFILL_DAYS`.

```toml
[rollup]
enabled        = true   # false = never publish; Backend pages then always scan raw spans
interval_secs  = 60     # floored at 15 — a pass per bucket; faster buys nothing
lag_secs       = 600    # floored at one bucket
top_operations = 50     # per (service, environment); 0 = service level only
backfill_days  = 30     # how far a first run reaches back; 0 = future only
```

The bucket width (60s) is **not** configurable. It is baked into every emitted timestamp, and two
nodes disagreeing about it would make a federated series unreadable.

## Two things that are wire contracts

Change either and you change the meaning of data already on disk.

1. **`LATENCY_BOUNDS_NS`** (`red_rollup.rs`) — the histogram's `explicit_bounds`, 19 buckets from
   1 ms to 30 minutes. A query spanning a change would interpolate across two different ladders,
   because `metric_dist::accumulate` takes the canonical bounds from the first row it parses and
   sums positionally from there. Renumbering it means renaming `LATENCY_METRIC` in the same change
   and letting the repair pass republish the covered range.
2. **The histogram JSON shape** — the writer (`points.rs`) and the reader (`photon-query`'s
   `HistPayload`) pin it independently; neither imports the other. A renamed field produces an empty
   histogram and a `null` p50 while every unit test on both sides still passes. That is exactly what
   `tests/rollup.rs` exists to catch.

## Timestamps

Both `timestamp` and `start_timestamp` are the bucket **start**. OTLP would put the interval end in
`timestamp`, but Photon's metric read path buckets by `timestamp`, so an end-labelled point lands in
the following bucket — every rollup series would sit one minute to the right of the raw spans it
summarizes, and the two would never line up on a chart.

## Related

- [`docs/adr/0002`](../adr/0002-red-rollups-are-published-as-metrics.md) — why metrics, not a table
- [`docs/adr/0003`](../adr/0003-a-services-request-rate-is-its-entry-spans.md) — why entry spans
- [`docs/subsystems/services-apm.md`](services-apm.md) — the pages that read this
- [`docs/subsystems/metrics.md`](metrics.md) — the store these land in
- [`docs/subsystems/federation.md`](federation.md) — how a Site ships its rollup to Central
