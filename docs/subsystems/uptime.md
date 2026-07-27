# Uptime

Always-on synthetic monitoring: scheduled HTTP(S)/TCP/ICMP probes with per-monitor intervals,
timeouts, and webhook alerts. A **self-contained SQLite vertical** (`photon-uptime`) — independent of
the Arrow/DataFusion write path used by the other signals.

> Shared conventions: [`../conventions.md`](../conventions.md). Frontend patterns:
> [`../frontend.md`](../frontend.md).

## Backend (`photon-uptime`)

- **Engine:** schedules probes (`probe.rs`, `scheduler.rs`), records up/down + latency to embedded
  SQLite (`store/`, exposed as the **`UptimeStore`** trait), tracks incidents (`state.rs`), and fires
  webhook alerts (`notify.rs`). Domain types in `model.rs`.
- **Storage:** the shared control-plane SQLite DB (`[storage].db_path`) — the same DB that holds UI
  users. No WAL/Parquet.
- **Config** (`[uptime]`, all optional tuning — omit to accept defaults): `retention_days` (30),
  `default_interval` (`60s`), `default_timeout` (`10s`), `worker_concurrency` (32), and an optional
  global `webhook_url` (per-monitor overrides supported).
- The subsystem is **always on**; `photon-server` spawns the scheduler + hourly retention.
- **Alerts bridge:** a monitor also carries `channel_ids: Vec<String>` (a `Monitor`/`MonitorInput`
  field, persisted as a JSON `TEXT` column added by an idempotent additive migration in
  `store/sqlite.rs`; NULL/legacy rows → `[]`). On each up/down **transition** the scheduler's notifier
  is `photon-server`'s `UptimeAlertBridge` (`crates/photon-server/src/uptime_bridge.rs`), which (1)
  still fires the legacy per-monitor / global `webhook_url` and (2) opens/closes a shared **alerts**
  incident (`photon_alerts` `AlertStore`, synthetic `rule_id = "uptime:<monitor.id>"`, empty series
  key) and delivers an alerts-shaped payload (`status` `triggered`/`resolved`) to each channel in
  `channel_ids`. `spawn_alerts` runs before `spawn_uptime` in `main.rs` so both share one `AlertStore`.

## API

Attached via `ApiServer::with_uptime`; routes 404 unless attached. Handler: `crates/photon-api/src/uptime.rs`.

| Route | Purpose |
|---|---|
| `GET/POST /api/monitors` | list / create monitors |
| `GET/PATCH/DELETE /api/monitors/:id` | read / update / delete |
| `POST /api/monitors/:id/pause\|resume` | pause / resume |
| `GET /api/monitors/:id/heartbeats\|incidents` | history |

## UI

`/uptime` → `UptimeDashboard.vue`: a table/cards toggle (persisted via `useStorage`), a monitor
filter, a stat band, and create/detail dialogs.

The filter is a case-insensitive substring match over a monitor's **`name` and `target`**, mirrored
to `?q=` so a filtered view is shareable. It's also where a service's "Related ▾ → Uptime" pivot
lands (`/uptime?q=<service>`, see `lib/core/useCorrelate.ts`). Note what that is and isn't: a
`Monitor` has **no service field** — only `name` and `target` (`crates/photon-uptime/src/model.rs`) —
so the pivot is an honest best-effort text match, not a modeled relationship. Associating monitors
with services properly would mean a schema change; until then the empty state distinguishes "no
matching monitors" from "no monitors yet" so a miss reads as a miss.

**Components** (`frontend/src/components/uptime/`): `MonitorTable`, `MonitorRow`, `MonitorCard`,
`MonitorForm`, `MonitorDetailDialog`, `HeartbeatBar`, `ResponseTimeChart`, `StatePill`,
`UptimeStatBand`. **Queries** (`frontend/src/lib/uptimeQueries.js`): `useMonitors` (polls 15s) +
heartbeats/incidents queries + create/update/delete mutations (toast-wired).
