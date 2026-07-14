# Photon docs

This directory is the project's knowledge base — written to get both humans and AI agents to the
right context fast. Start here, then jump to the doc that matches your task.

## Cross-cutting docs

- **[`architecture.md`](architecture.md)** — the backend. Crate graph, per-signal write/read data
  flow, storage & durability model, the `/api/*` surface, and the **load-bearing invariants you must
  not break**. Read this before touching a crate's public API or the query/storage path.
- **[`frontend.md`](frontend.md)** — the Vue 3 SPA. Routes → views, the `ui/` primitive library, the
  `lib/` layer, and the Ky → TanStack Query → components data flow.
- **[`conventions.md`](conventions.md)** — coding conventions and gotchas that aren't obvious from any
  single file (the `PhotonError` enum, DataFusion column access, dependency co-pinning, frontend
  timestamp units, the no-commit-between-tasks git rule).

## Per-feature docs — [`subsystems/`](subsystems/)

**One doc per feature** so an agent working on a feature goes straight to its file:

- **[`subsystems/logs.md`](subsystems/logs.md)** — log explorer, the query grammar, facets, histogram, live tail.
- **[`subsystems/traces.md`](subsystems/traces.md)** — trace explorer + waterfall + log↔trace↔span correlation.
- **[`subsystems/services-apm.md`](subsystems/services-apm.md)** — RED metrics + Apdex, derived from spans.
- **[`subsystems/metrics.md`](subsystems/metrics.md)** — OTLP metrics explorer + query builder + catalog.
- **[`subsystems/rum.md`](subsystems/rum.md)** — Real-User Monitoring end to end: beacon, `@photon/rum` SDK, ingest, query, UI.
- **[`subsystems/uptime.md`](subsystems/uptime.md)** — synthetic HTTP/TCP/ICMP monitors (self-contained SQLite vertical).
- **[`subsystems/data.md`](subsystems/data.md)** — data usage/storage accounting + retention/purge.
- **[`subsystems/auth.md`](subsystems/auth.md)** — the three auth systems + onboarding.

## Find it fast — task → doc

| I'm about to… | Read |
|---|---|
| change a crate's public API, the WAL, compaction, or query engine | [`architecture.md`](architecture.md) |
| add/modify an OTLP signal or an `/api/*` route | [`architecture.md`](architecture.md) + the signal's doc in [`subsystems/`](subsystems/) |
| work on a specific UI section (logs, traces, APM, metrics, uptime, data) | that feature's [`subsystems/`](subsystems/) doc → [`frontend.md`](frontend.md) |
| build a shared UI primitive, a query composable, or a chart | [`frontend.md`](frontend.md) |
| touch anything RUM (SDK, beacon, vitals, error grouping) | [`subsystems/rum.md`](subsystems/rum.md) |
| add a dependency, or hit a DataFusion/Arrow/timestamp gotcha | [`conventions.md`](conventions.md) |

## Ground truth vs. these docs

The **code is the ground truth**; these docs summarize intent and orient you. If a doc names a file,
symbol, or route, verify it still exists before relying on it — and if you find a doc is stale, fix
the doc as part of your change. The top-level [`../README.md`](../README.md) is the public-facing
overview; [`../CLAUDE.md`](../CLAUDE.md) is the concise always-loaded index that points here.
