# Federation

One central Photon monitors many customer Photon installs (tenants) that push telemetry to it, egress-only via OTLP. **No new storage engine** — federated data flows through the existing logs/traces/metrics WAL and storage pipeline; this doc covers what's specific to federation: the config, token flow, server-side tenant stamping, the summary/full modes, and the curated management UI.

> Shared plumbing and invariants: [`../architecture.md`](../architecture.md).

## Two modes: summary (default) and full

- **Summary mode** (default): the tenant-side Photon periodically computes a lightweight health summary (ingest counters, incident count, disk usage) and POSTs it as OTLP metrics back to central. Central stamps these with a `tenant` resource attribute and stores them normally — no data duplication.
- **Full mode** (opt-in): the tenant-side Photon **also** forwards raw OTLP batches (logs/traces/metrics) to central in real time. This is a best-effort tee (non-blocking; a full queue drops the incoming batch — drop-newest) that rides alongside local ingest. Central stamps each forwarded batch with the same `tenant` attribute.

Both modes use per-tenant bearer tokens and server-side stamping — client-supplied `tenant` attributes are always overwritten. Summary mode is the default and runs alone; full mode enables the tee and ALSO runs the summary pusher (so central sees both the high-volume raw mirror and the low-volume summary).

## Tenant-side configuration

Optional `[federation]` config block (omit = disabled):

```toml
[federation]
endpoint = "https://central.example.com:8080"    # required: central's OTLP/HTTP endpoint
token = "tk_tenant_…"                            # required: per-tenant bearer token (minted by central, never generate locally)
mode = "summary"                                 # optional: "summary" (default) or "full"
signals = ["traces"]                             # optional, full mode only: subset of ["logs","traces","metrics"] to mirror
interval_secs = 30                               # optional: how often to push summary metrics (default 30, min 5)
queue_batches = 1024                             # optional: full-mode tee queue capacity (default 1024, min 16); dropped on overflow
```

**Env overrides** (create or override the block):
- `PHOTON_FEDERATION_ENDPOINT`
- `PHOTON_FEDERATION_TOKEN`
- `PHOTON_FEDERATION_MODE` (`"summary"` | `"full"`)
- `PHOTON_FEDERATION_INTERVAL_SECS`
- `PHOTON_FEDERATION_QUEUE_BATCHES`

## Token flow & tenant stamping

**Three independent auth systems, never conflated:**

1. **OTLP ingest (tenant-side only)**: shared service bearer token (`[ingest].token`), same as before — no change.
2. **Tenant registry (central)**: per-tenant minted bearer tokens (`tk_tenant_…`), rotateable, stored in SQLite control-plane DB (`tenants` table). UI-managed only, no config surface.
3. **Tenant identity stamping (central ingest, trust boundary)**: when central's OTLP receiver resolves the `Authorization` header against its tenant token map, it stamps the tenant's name as a **`tenant` resource attribute** on every resulting record (logs/spans/metrics). Client-supplied `tenant` attributes are overwritten — this is the security boundary. `tenant` is **not** in the default `promoted_attributes` — it lives in the long-tail attributes map, so `tenant:<id>` filters work but scan every file in the time window (no skip-index pruning on it). On a central with many tenants and real full-mode volume, add `"tenant"` to `[schema].promoted_attributes` to make it a first-class pruned column (the query engines handle either location).

### Stamping implementation

In `photon-ingest`:
- `resolve_bearer(auth_header, local_token, tenant_tokens) -> Auth { Local, Tenant(name), Denied }`
  - `Local`: matches the local ingest token (existing constant-time compare)
  - `Tenant(name)`: key found in the tenant token map
  - `Denied`: no match or missing header

- For each accepted OTLP batch (post-decode), `stamp_tenant(&mut attributes, tenant_name)` removes any existing `tenant` key and inserts the server-resolved name.

- **Prometheus remote-write limitation** (`/api/v1/write`): tenant tokens are rejected with 401 + body `"tenant tokens not accepted on remote-write"`. Remote-write is a v1 protocol without the rich resource attributes that OTLP carries; stamping is ambiguous there, so it's forbidden by design.

## Summary metrics (OTel semantic conventions)

Emitted by the tenant-side summary pusher, captured by central:

| Metric | Kind | Unit | Attributes | Notes |
|---|---|---|---|---|
| `photon.federation.up` | Gauge | `1` | `mode` = `summary` \| `full` | always 1, presence = liveness probe |
| `photon.federation.ingest.rows` | Sum (monotonic, cumulative) | `{rows}` | `signal` = `logs` \| `traces` \| `metrics` | snapshot of each signal's ingested-rows counter (central diffs them to compute rate) |
| `photon.federation.ingest.bytes` | Sum (monotonic, cumulative) | `By` | `signal` | snapshot of each signal's ingested-bytes counter |
| `photon.federation.incidents.open` | Gauge | `{incidents}` | — | current count of open alert incidents on the tenant |
| `photon.federation.disk.hot_bytes` | Gauge | `By` | `signal` | current disk usage per signal (hot tier only) |

All federation metrics carry a resource attribute `service.name = "photon"` + the current `mode` attribute. The tenant name is stamped by central at ingest, so central's queries filter per-tenant with `tenant:<name>`.

### Metric collection cadence

- **Summary push interval** (tenant-side): configured via `[federation].interval_secs` (default 30s). The pusher samples the three ingest counters + storage stats once per interval and POSTs them. If a push fails, it logs the error but continues — next interval will try again.
- **Central staleness** (central-side): `GET /api/tenants/summary` considers a tenant:
  - `up`: last `photon.federation.up` point within 120 seconds
  - `stale`: within 600 seconds
  - `down`: older than 600 seconds (or never seen)

## Full-mode tee & forwarder (best-effort)

`[federation].signals` (full mode only) narrows what the tee mirrors — e.g. `signals = ["traces"]` mirrors spans only, giving central the Services/APM + Traces views for the tenant without shipping logs (usually the heaviest signal). Omitted → all three. `signals` under `mode = "summary"` is a **config error** (fail-fast at startup): summary mirrors nothing, so a signal list there would be silently ignored — the config refuses the contradiction instead. The summary `mode` attribute reports the subset (`full:traces`), so the central card badge and its tooltip stay honest. Env override: `PHOTON_FEDERATION_SIGNALS=traces,metrics`.

When `[federation].mode = "full"`:

1. **Tenant-side tee** (in `photon-ingest`): after successful auth and decode, each OTLP batch is offered to a bounded MPSC channel (capacity = `[federation].queue_batches`). If the channel is full, the batch is dropped + a counter increments; the local ingest ack is NOT delayed (tee is non-blocking, never blocks ingest).

2. **Tenant-side forwarder** (in `photon-server`): a background task drains the tee channel and POSTs each batch to central's `/v1/{logs|traces|metrics}` endpoint with the tenant bearer token. On failure:
   - HTTP-status failures (central reachable but erroring): up to 3 attempts with backoff between them (250ms, 1s — `FORWARD_BACKOFF`), then drop + increment `dropped`
   - Connection-level failures (connect refused / timeout): drop after the **first** attempt — retrying an unreachable central would serialize ~30s of timeouts per payload and head-of-line-block the queue into overflow; the single attempt per payload doubles as the recovery probe
   - Never block or fail the loop — it continues forever

3. **Central ingest**: receives the forwarded batch (marked by bearer token → tenant name), stamps it, writes it normally.

**Key properties:**
- Dropped batches when the queue is full are **not a sync problem** — the bounded queue drops the incoming batch (`try_send` on full = drop-newest). A spike of ingest + active forwarding can lose full-mode visibility; this is documented and acceptable (no guaranteed delivery).
- The tee is a separate concern from the summary pusher — both features coexist on the tenant without conflict.
- `stats.queued` reflects current channel depth; `stats.dropped` increments on queue-full; `stats.pushed` increments on successful batch POST.

## Central-side: tenant registry & APIs

Central maintains a `tenants` SQLite table (one row per tenant, schema at `crates/photon-api/src/tenants.rs`):

| Column | Type | Notes |
|---|---|---|
| `name` | TEXT PRIMARY KEY | tenant identifier, `[a-z0-9-]{1,64}` |
| `token` | TEXT UNIQUE | server-minted bearer token `tk_tenant_…` |
| `ui_url` | TEXT NULL | optional link-back to tenant UI (shown on the Home board card whenever set) |
| `created_at` | INTEGER | unix milliseconds |

### Tenant management API

Session-cookie-authed (like all central UI routes):

| Route | Method | Purpose |
|---|---|---|
| `/api/tenants` | `GET` | list all tenants (token redacted to last 4 chars) |
| `/api/tenants` | `POST` | create tenant `{name, ui_url?}` → 201 with full minted token (shown ONCE) |
| `/api/tenants/:name` | `PATCH` | update `ui_url` → 200 |
| `/api/tenants/:name/rotate-token` | `POST` | mint a new token, return the full new token, old token removed from ingest token map → 200 |
| `/api/tenants/:name` | `DELETE` | remove tenant → 204; token removed from ingest token map |

Mutations automatically reload the live tenant-token map used by central's ingest receivers (zero ingest downtime, no restart required).

### Curated summary endpoint

`GET /api/tenants/summary` → `Json<Vec<TenantSummary>>`

Returns one card per tenant with aggregated health:

```rust
{
  "name": "acme",
  "mode": "full",                  // from photon.federation.up "mode" attribute, None if never reported
  "status": "up" | "stale" | "down",
  "last_seen_ms": 1723917600000,  // unix ms; 0 = never
  "ingest_rows_per_sec": 1250.5,  // differenced from photon.federation.ingest.rows over 15-min window
  "open_incidents": 3,             // latest photon.federation.incidents.open
  "hot_bytes": 52428800,          // sum of latest photon.federation.disk.hot_bytes across signals
  "ui_url": "https://acme.example.com",
  "spark": [[1723916700000, 1200.0], [1723916760000, 1350.0], ...]  // (ms, rows/sec) sparkline points
}
```

Implementation: for each tenant, query the four `photon.federation.*` metrics over the last 15 minutes with filter `tenant="{name}"` (the stamped attribute), bucketed into ~30 points. Staleness determination uses hardcoded thresholds: `up` ≤ 120s, `stale` ≤ 600s, else `down`.

## Federation status (tenant-side UI visibility)

`GET /api/federation/status` (tenant-side) → 

```rust
{
  "enabled": bool,
  "status": Option<{
    "mode": "summary" | "full",
    "endpoint": "...",
    "last_push_ms": 0,            // unix ms; 0 = never
    "last_error": null | "...",   // latest error string if any
    "pushed": 123,                // successful summary pushes (full mode: OTLP batches)
    "dropped": 0,                 // full-mode tee drops
    "queued": 5,                  // full-mode queue depth
  }>
}
```

Only present (non-`None`) when `[federation]` is configured; `enabled: false` when absent.

## Central Home board

A new **Tenants** section (conditional, visible only when `enabled: true` in `GET /api/federation/status`... wait, that's tenant-side. Let me re-read the task.)

Actually, the task says "Home gains a conditional tenant board" — that's on central, not tenant-side. Central's Home should show a grid of `TenantCard`s populated from `GET /api/tenants/summary`. Each card:
- Name + mode Badge (`summary` | `full`)
- Status dot (success/warning/error by up/stale/down)
- Ingest rate, open incidents, hot bytes rows
- Sparkline of rows/sec over the last 15 min
- Footer: "last seen X ago"
- Down-tenant: destructive tint + "Unreachable — no heartbeat"
- "Open UI ↗" link to `ui_url` whenever set (any mode)

Clicking a tenant card:
- Full-mode: sets the tenant context dimension and navigates to Logs with a `tenant:` filter applied
- Summary-mode: open `ui_url` in a new tab


With a tenant filter active, the board narrows to the active tenant's card (a health header for the tenant being inspected) and notes how many cards the filter hides; clearing the filter restores the full fleet board.

## Tenant context dimension

Central's UI gains `tenant` as its own global context dimension in `lib/core/context.ts`, alongside the time window. It is set via the Home board (full-mode card) or the ContextBar's tenant picker (a dropdown that only renders when the tenant registry is non-empty), synced to the `tenant` URL key, and carried by `correlate()`. The query composables append `tenantQueryTerm()` (`tenant:<name>`) to the existing `q` grammar filter, scoping Logs/Traces/Metrics, the Services (APM) vertical (`/api/red`, `/api/services/:service/{timeseries,dependencies}` take an optional `q` grammar param), and the Infrastructure vertical (`/api/infra/*` likewise) to that tenant's stamped records. `GET /api/red?group=service` additionally groups by the `tenant` attribute and returns it per row (null for local spans), so the Home/Services tables label federated rows with a tenant badge.

The field catalogs (`logs/fields.ts`, `traces/spanFields.ts`, `metrics/metricFields.ts`) each gain a `tenant` entry for autocomplete.

## Files & modules

**Tenant-side (photon-server):**
- `crates/photon-server/src/federation/mod.rs` — summary pusher spawn + `FederationStats` shared telemetry
- `crates/photon-server/src/federation/otlp.rs` — summary metric builder (`build_summary`, ~70 lines copied from `photon-agent/src/otlp.rs`)

**Central (photon-server):**
- `crates/photon-server/src/federation/mod.rs` — full-mode forwarder spawn (same module as tenant-side summary)

**Ingest (both):**
- `crates/photon-ingest/src/auth.rs` — `resolve_bearer`, `Auth` enum, `stamp_tenant` helper
- `crates/photon-ingest/src/lib.rs` — `IngestServer` gains `tenant_tokens: TenantTokenMap` + full-mode `federation_tee: Option<FederationTee>`

**Central API:**
- `crates/photon-api/src/tenants.rs` — `Tenant`, `TenantStore` trait, `SqliteTenantStore`, `TenantApi` handlers
- `crates/photon-api/src/federation.rs` — `FederationStatus` trait seam, `federation_status` handler

**Frontend (central):**
- `frontend/src/lib/tenants/tenantsQueries.ts` — API client + TanStack Query composables
- `frontend/src/components/tenants/TenantCard.vue` — card for Home board
- `frontend/src/components/tenants/TenantManageDialog.vue` — add/edit dialog (rotate token inside edit; minted/rotated token snippet shown once)
- `frontend/src/views/TenantsView.vue` — the `/tenants` page (NavRail Manage group)
- `frontend/src/components/data/DataTenants.vue` — the registry table (per-row edit/delete actions, "Add tenant" button) rendered by TenantsView
- `frontend/src/components/data/DataOverview.vue` — federation status strip (tenant-side only, shown when `enabled: true`)
- `frontend/src/views/HomeView.vue` — tenant board section
- `frontend/src/lib/core/context.ts` — `tenant` context dimension (`tenant` ref, `setTenant`/`clearTenant`, `tenantQueryTerm()`, `tenant` URL key)
- `frontend/src/components/common/ContextBar.vue` — tenant picker dropdown (renders only when the registry is non-empty)

**Config:**
- `crates/photon-core/src/config.rs` — `Config.federation: Option<FederationConfig>`, env overrides
- `photon.example.toml` — commented `[federation]` example

## Design notes

- **No per-tenant retention** — central retention applies uniformly to all data, regardless of source or tenant. If different tenants need different retention, that's a future enhancement (separate retention groups per tenant).
- **No central-controlled mode** — mode is set by the tenant in their config; central doesn't dictate it. Tenants can unilaterally upgrade from summary to full or downgrade.
- **RUM and uptime are NOT federated (known v1 gap, deliberate)** — the full-mode tee sits in `photon-ingest`'s OTLP receivers, but RUM enters through a different front door entirely: the browser beacon (`POST /api/rum`, `photon-api`'s `RumSink`) writes straight to the WALs and never passes the tee, so a full-mode tenant's vitals/errors are never mirrored to central. Uptime is likewise a local SQLite vertical (only its open-incident count rides the summary metrics). Central's `/rum` would stay empty for tenants even if the data were mirrored — the app registry (`rum_apps`) is per-install SQLite, so central doesn't know tenant apps exist. The chosen posture: tenant RUM/uptime is monitored on the tenant's own UI (the Home tenant card links out via `ui_url`). Closing the gap would need both a `RumSink`-level tee (synthesizing OTLP from vitals/errors) and tenant app-registry sync — a follow-up if demand appears.
- **Tenant filter coverage** — the tenant context dimension filters Logs, Traces, Metrics, Services (APM), and Infrastructure. Home's cross-signal tiles (RUM vitals, uptime) are not tenant-filtered — see the gap above.
- **No Prometheus remote-write federation** — tenant tokens are rejected on `/api/v1/write` to avoid ambiguity in stamping (no rich resource attributes in the protocol).
