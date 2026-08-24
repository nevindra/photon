# RED rollups are published as metrics, not stored in a table of their own

Status: accepted

`red.rs` has always said it: *"v1 is breadth-first / correctness-over-speed... no precomputed rollups — the
flagged optimization target."* Four things came due at once and made it payable. The Services index became a
cross-Site board polling every 15 seconds, so every poll re-aggregated every span in the window. Per-row
sparklines need RED **per bucket per service**, which is a shape the point-in-time aggregate cannot produce.
A 30-day error budget over spans means scanning essentially the whole dataset, since default retention is 30
days. And a before/after deploy diff needs two windows that may sit far back in time.

We build the rollup as a background pass in `photon-server` that summarises spans into **`photon.service.*`
metrics on the existing metrics pipeline** — not into a new SQLite table, and not into a fourth Parquet tier.

## Why metrics

**It obeys the rule the codebase already keeps.** RUM adds no storage engine: Web Vitals become gauge
metrics, JS errors become logs. `photon-alerts` adds no storage engine: it reads the three query engines and
keeps three small control-plane tables. A fourth Parquet tier would be the first subsystem to break a
principle two others already hold.

**Retention, compaction, and durable replication come free.** A bespoke SQLite rollup table is precisely the
kind of derived data everyone forgets about until a disk fills. On the metrics path, `[retention]`,
`MetricsCompactor`, and the replicator already apply.

**Alert rules point straight at it.** "`payments-api` error rate above 2% for five minutes" becomes an
ordinary metrics rule. In a SQLite table it would need a new `ConditionSource` implementation and a new rule
kind in `photon-alerts`.

**Federation comes free, and this is the decision-maker.** The Services index is cross-Site by default
(ADR-0004), but a Site in `summary` mode tells Central almost nothing about its services today — cross-Site
service health would have worked only for Sites able to afford `full` mode and its raw span mirror. Because
the rollup is metrics, a summary-mode Site adds it to the payload it already pushes, and Central stamps
`tenant` exactly as it does for everything else. Neither alternative gives this at any price.

## What it costs, honestly

**Cardinality is the real risk.** `metric_query.rs` caps a query at `MAX_SERIES = 1000`, and
`site × environment × service × operation` can pass that. So: the service level is always rolled up in full,
and the operation level is capped at the top 50 per `(service, environment)` per pass, with everything else
summed into a single `operation = "__other__"` series that is rendered explicitly in the UI. An operation
name carrying an id (`GET /v1/charges/ch_1a2b3c`) would otherwise take the metrics store down, and that is
the least pleasant class of bug to diagnose.

**`MetricsQueryEngine` has no "top N by ratio" primitive.** It returns series over time, not a ranked table.
So the `/api/red` handler assembles the table from several series queries and divides. The HTTP response
shape does not change, so the frontend is unaffected.

**Late data has a stated boundary.** Each pass recomputes the last ten minutes and then seals those buckets.
A span arriving later than that never reaches the rollup. This is the rollup's one real inaccuracy, and it
belongs in `docs/subsystems/rollup.md` rather than in a comment nobody reads.

**Derived data is never the only copy.** Every endpoint that reads the rollup keeps a raw-span fallback for
narrow windows and sub-minute buckets, responses carry `"source": "rollup" | "raw"`, and a test asserts the
two paths agree on the same fixture. A silent divergence between them would be worse than no rollup at all.
