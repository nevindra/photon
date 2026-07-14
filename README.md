<div align="center">

# Photon

**Fast, lightweight, OTEL-native observability — logs, traces, metrics, APM, uptime, and RUM in a single Rust binary.**

A self-hosted alternative to SigNoz, Datadog, Sentry, and Grafana that aims to be _better, faster, and cheaper_:
one process, one binary, your disk, your data.

</div>

---

## Why Photon

Modern observability makes you choose between two bad options: a **sprawling self-hosted stack**
(SigNoz needs ClickHouse + collectors; Grafana's "LGTM" is Loki + Tempo + Mimir + Grafana, each a
separate service) or a **metered SaaS bill** (Datadog and Sentry price per host, per seat, per GB,
per million spans — and it compounds). Photon takes a third path.

- **One binary, one process.** The OTLP receivers, the storage/query engine, the REST API, and the
  Vue UI are all compiled into a single ~30 MB Rust binary (the UI is embedded at build time). No
  ClickHouse, no Zookeeper, no Kafka, no sidecars. `docker compose up` — or just run the binary.
- **10× lighter storage.** No full-text inverted index (the thing that makes log stores as big as
  the logs themselves). Photon prunes with **skip indexes** — per-row-group min/max stats + token
  bloom filters, kilobytes per file — over zstd-compressed Parquet sorted by `(service, timestamp)`.
- **10× faster queries by layout, not luck.** Arrow-native end to end (Arrow → Parquet → DataFusion),
  local NVMe is the primary store (no network hop on the query path), and search is two-pass
  late-materialized so wide attribute maps are only decoded for rows that survive.
- **10× cheaper to run and to own.** Self-hosted on one well-resourced node. No per-seat, per-host,
  or per-GB metering. The durable tier is optional and any S3-compatible bucket
  ([Garage](https://garagehq.deuxfleurs.fr/) recommended — also a single Rust binary).
- **OTEL-native.** Ingest is standard OTLP over gRPC (`:4317`) and HTTP (`:4318`). Point your
  existing OpenTelemetry Collector or SDKs at it; nothing proprietary on the wire.
- **A RUM SDK that doesn't slow your site down.** The browser agent is **< 5 KB gzipped** — versus
  Sentry's ~20 KB+ and Datadog RUM's ~25–40 KB.

## What's inside

Photon is a **full-signal** platform, not just a log store. Every section below is live in the UI:

| Section | What it does |
|---|---|
| **Logs** | OTLP log explorer with a small query grammar (`field:value`, OR-lists, negation, numeric compare, free-text), facets, severity histogram, and live tail. |
| **Traces** | Distributed-trace explorer + waterfall detail view, with log↔trace↔span correlation. |
| **Services (APM)** | Per-service RED metrics (Rate/Errors/Duration), Apdex, health-first service list, and per-service dashboards derived from trace spans. |
| **Metrics** | OTLP metrics explorer (gauges, sums, histograms/summaries) with a query builder and a metric catalog. |
| **RUM** | Real-User Monitoring: Core Web Vitals (LCP/INP/CLS/FCP/TTFB) + JS error tracking from a tiny browser SDK, sliceable per app / route / device / browser. |
| **Uptime** | Always-on synthetic HTTP/ping monitors with per-monitor intervals, timeouts, and webhook alerts. |
| **Data** | Data-usage and storage monitoring + retention management, so you can see and bound what you're keeping. |

All signals share one storage/query spine, so cross-signal correlation (a log line → its trace → the
service's RED metrics → the errors on that page) is a click, not an integration.

## How Photon compares

|  | **Photon** | SigNoz | Grafana (LGTM) | Datadog | Sentry |
|---|---|---|---|---|---|
| Deployment | 1 binary / 1 container | ClickHouse + collector + UI | Loki + Tempo + Mimir + Grafana | SaaS agent | SaaS SDK |
| Storage engine | Embedded (Parquet + DataFusion) | ClickHouse server | 3 separate TSDB/stores | Proprietary cloud | Proprietary cloud |
| Signals | Logs, traces, metrics, APM, uptime, RUM | Logs, traces, metrics | Logs, traces, metrics | All + more | Errors, some tracing/RUM |
| Log index | Skip index (min/max + bloom) | Inverted / ClickHouse | Loki labels | Proprietary | — |
| RUM SDK size | **< 5 KB gzip** | n/a | Faro (~larger) | ~25–40 KB | ~20 KB+ |
| Cost model | Self-host, flat | Self-host | Self-host | Per-host/GB/seat | Per-event/seat |
| Your data | On your disk | On your disk | On your disk | Their cloud | Their cloud |

> Photon targets **medium scale** — dozens of services, tens of GB/day of logs, meaningful trace and
> metric volume — on a single well-resourced node. Clustering/horizontal scale-out is deliberately
> not built; the crate boundaries are kept clean so pieces _could_ split into separate processes
> later without a rewrite.

## Quick start

Photon's frontend is **embedded into the binary at build time**, so build the UI first (the package
manager is **bun**, never npm):

```bash
# 1. Build the Vue UI into frontend/dist (embedded by photon-api)
cd frontend && bun install && bun run build && cd ..

# 2. Build & run the server
cp photon.example.toml photon.toml            # then edit the secrets
cargo run -p photon-server -- photon.toml
```

Open <http://localhost:8080>, complete the one-time **"create your account"** onboarding, and log in.

- **UI + REST API:** `:8080`
- **OTLP gRPC:** `:4317`  ·  **OTLP HTTP:** `:4318`

Send data with your OTEL Collector/SDK pointed at `http://<host>:4318/v1/{logs,traces,metrics}`
(header `Authorization: Bearer <your ingest token>`).

### Drop-in Prometheus remote-write sink

Keep your existing Prometheus (or Grafana Agent / Alloy / OTel Collector) and re-point its
`remote_write` at Photon — no changes to your exporters or scrape config. Photon stores and
visualizes the data, so you can switch off Grafana:

```yaml
# prometheus.yml
remote_write:
  - url: http://<photon-host>:<ingest-http-port>/api/v1/write
    authorization:
      credentials: <ingest token>   # matches [ingest].token
```

### Local development (both processes, hot reload)

```bash
make install-tools      # one-time: installs process-compose (dev TUI)
make dev                # backend + Vite dev server together; UI at :5173, API at :8080
```

On first run this generates a throwaway `photon.dev.toml` (local data dir, dev secrets,
`admin`/`admin` login) — it works out of the box. `make help` lists every target.

### Docker

```bash
cp .env.example .env            # fill in PHOTON_INGEST_TOKEN + PHOTON_SESSION_SECRET
make docker-up                  # single non-root distroless container
```

Add the opt-in Garage durable-S3 tier with `make docker-up-durable`. See
[`deploy/README.md`](deploy/README.md) for the full env-var reference and durability model.

### Generate load (dev/bench only)

```bash
cargo run -p photon-loadgen -- logs   --rate 5000 --services 10
cargo run -p photon-loadgen -- traces --rate 2000 --services 10 --spans-per-trace 4..20
```

## The RUM browser SDK — `@photon/rum`

A **< 5 KB gzipped** agent (CI-enforced) that captures Core Web Vitals and JS errors and beacons
them to Photon. One call, never throws into your app, no cookies:

```js
import { initPhoton } from "@photon/rum";

initPhoton({
  app: "web-storefront",              // becomes service.name in Photon
  endpoint: "https://photon.example.com",
  key: "pk_live_…",                   // public app-key (safe to ship in client JS)
  attribution: true,                   // opt-in per-page "why is LCP slow" breakdown
});
```

Ships as **ESM** (npm, tree-shakeable) and a drop-in **IIFE `<script>`** build for no-bundler /
HTTP-only apps. Vitals become gauge **metrics** (`web_vitals.*`); errors become ERROR **logs**
grouped into issues by a server-computed fingerprint — no new storage signal. Source in
[`sdk/rum/`](sdk/rum/); design in [`docs/subsystems/rum.md`](docs/subsystems/rum.md).

## Architecture at a glance

Photon is a Cargo **workspace of small crates** — the crate graph _is_ the architecture, with
dependencies pointing one way toward `photon-core` (the compiler forbids cycles).

```
photon-core     domain types only, no I/O: Arrow schemas, config, query grammar, PhotonError
photon-wal      group-commit write-ahead log (the durability boundary)
photon-index    skip-index format: token bloom + min/max stats (pure)
photon-storage  hot + durable object_store wrappers + background replicator
photon-compact  closed WAL segment → sort → Parquet + skip index → replicate → manifest
photon-query    manifest + skip-index pruning, then DataFusion (search, facets, RED, RUM, …)
photon-ingest   OTLP gRPC + HTTP receivers, OTLP→record mapping, token auth
photon-uptime   always-on synthetic HTTP/ping monitors
photon-api      axum REST API + session auth + embedded Vue UI
photon-server   the binary: config, wiring, background-task supervision
photon-loadgen  dev-only OTLP load generator
```

**Write path (one signal):** OTLP request → token check → map to Arrow record → **WAL append**
(group-commit `fsync` = the ack/durability boundary) → background compactor drains closed segments
into sorted Parquet + `.idx` skip index → async replicate to the durable store → manifest update.

**Read path:** manifest prunes candidate files by time overlap → skip index prunes by min/max +
token bloom → DataFusion scans only the survivors. Bloom filters may false-_positive_ (extra scan)
but **never false-negative** — pruning can only add work, never drop a real result.

**Full detail lives in [`docs/`](docs/)** ↓

## Documentation

The [`docs/`](docs/) directory is written to get both humans and AI agents to the right context fast:

- **[`docs/architecture.md`](docs/architecture.md)** — the crate graph, per-signal write/read data
  flow, storage/durability model, and the load-bearing invariants you must not break.
- **[`docs/frontend.md`](docs/frontend.md)** — the Vue 3 app: routes, views, the `ui/` primitive
  library, and the Ky → TanStack Query → components data flow.
- **[`docs/subsystems/`](docs/subsystems/)** — **one doc per feature**:
  [logs](docs/subsystems/logs.md), [traces](docs/subsystems/traces.md),
  [services/APM](docs/subsystems/services-apm.md), [metrics](docs/subsystems/metrics.md),
  [rum](docs/subsystems/rum.md) (incl. the `@photon/rum` SDK), [uptime](docs/subsystems/uptime.md),
  [data](docs/subsystems/data.md), [auth](docs/subsystems/auth.md) — each with its key files,
  endpoints, and UI.
- **[`docs/conventions.md`](docs/conventions.md)** — coding conventions and gotchas (the
  `PhotonError` enum, DataFusion column access, frontend timestamp units, dependency co-pinning).

`CLAUDE.md` is the concise, always-loaded entry point that indexes into these docs.

## Repository layout

```
crates/          the Rust workspace (see the crate graph above)
frontend/        Vue 3 + Vite + Tailwind + Reka UI SPA (embedded into the binary)
sdk/rum/         the @photon/rum browser SDK (TypeScript, ESM + IIFE)
deploy/          Dockerfile, docker-compose, Garage durable-tier profile
docs/            architecture & subsystem reference (this project's knowledge base)
photon.example.toml   annotated example config
Makefile         dev / build / docker / bench targets (make help)
```

## Testing & tooling

```bash
cargo test                    # whole Rust workspace
cargo test -p photon-query    # one crate
cargo fmt && cargo clippy --all-targets
cd frontend && bun run test   # frontend unit tests (vitest)
cd sdk/rum   && bun run test  # SDK tests + bun run size (bundle-size gate)
```

## Status & license

Photon began as an internal SigNoz replacement and is under active development. It is a single-node,
single-role (all authenticated users share one role) platform today — RBAC, multi-tenancy, and
clustering are explicitly out of scope until there's demand.

_License: TBD._
