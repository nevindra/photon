# Docker fleet management — what is done, what is left

Branch `feat/docker-fleet-ui`, as of 2026-09-07. Nothing is committed yet. The requirements are in
`docker-fleet-management.md`; that file now carries a per-requirement status, this one is the
narrative. The earlier `RINGKASAN-DOCKER-FLEET-UI.md` covers the frontend-only phase in detail.

## 1. Done

### The UI (phases 0–6, frontend)
Every screen in the requirements exists and was reviewed by an independent reviewer three times
(71 → 82 → 85/100): host enrolment (SSH or token, bulk), the Containers tab with drawer (logs, stats,
details, audited shell), one-shot lifecycle actions with confirmations sized to the blast radius,
Stacks (editor with lint, deploy options, drift desired-beside-observed, adopt), Templates (catalog,
versions, registries, Portainer import, four-step deploy wizard with preflight), Rollouts (staged,
gated, per-target status, rollback), the Audit page, roles with `GatedButton`, and the Attention
Board's Deploy world. A backend that lacks the subsystem gets a clean "unavailable" surface, never a
fake Critical.

### The backend (today)
- **`photon-deploy`** — the pure domain: the model every route serializes, the agent wire types,
  compose lint/render/drift/preflight/rollout planning, Portainer import, the `DeployStore` trait.
- **`photon-api/src/deploy/`** — every route the UI calls (agents, inventory, actions, stacks,
  templates, registries, rollouts, audit, exec), the agent control channel (`POST /api/agent/sync`,
  a long poll), the in-memory hub (wake-ups, stream sessions, event and stat rings, bootstrap
  jobs), and the supervisor tick that advances rollouts and expires stale work.
- **Roles** on users (`viewer` / `operator` / `admin`), a `CurrentUser` extractor and a 403 gate,
  `PATCH /api/users/:name`. The **agent token** (`ak_…`, sha256 stored, rotatable) is documented as
  the fourth independent auth system.
- **`photon-server/src/deploy/`** — the SQLite store (`deploy_*` document tables in the control-plane
  DB), AES-256-GCM secrets under a key file beside the DB, and the SSH bootstrapper (russh).
- **`photon-agent` 0.2.0** with the `docker` feature: bollard plus the `docker compose` CLI,
  capabilities opt-in per flag, state on disk, clock-skew rebasing, self-upgrade from the sync
  response, `install` / `uninstall` / `version` subcommands, metrics endpoint taken from the
  server's hint.
- `[deploy]` config with `PHOTON_DEPLOY_*` overrides; the container image now ships the agent
  binary beside the server so `GET /api/agent/download` works air-gapped.

### Verified
- Unit and integration tests across all four crates, including two end-to-end protocol tests that
  drive the server with a fake agent (`crates/photon-server/tests/deploy_e2e.rs`).
- A **live run** against this laptop's own Docker daemon with the real agent: enrol → first sync
  (host online, Docker facts, GPU metrics landing in `/infra`) → compose stack up → logs and stats
  → restart action → out-of-band `docker stop` → drift → reconcile → audited shell (exit codes,
  slow commands) → destroy. Every step in the audit log.
- That run found four real bugs, all fixed with regression tests: a lost update on the host row
  (two writers overwriting each other, which left a stack in `destroying` forever), lost wake-ups
  between agent polls, the shell running in a pty (busybox `ash` stalls on a cursor query), and
  shell output skipped between reads.
- `cargo clippy`, `cargo fmt`, `bun run type-check`, and 1451 frontend tests are green.

### The Lenz template (second live run, later on 2026-09-07)
The `dumpbench` compose and the `autocd-lenz-rc` container on 192.168.103.76 were modelled as a
**Lenz** template (13 typed variables, a generated DB password, an optional RUM key, a maps bind
mount that trips the dangerous-construct acknowledgement, and the Photon block) and deployed to
the laptop through the real agent: dry-run → deploy → three immutable upgrades (1.0.0 → 1.0.3)
→ Lenz serving on :4030 with its migrations applied on TimescaleDB-HA, its traces arriving in
Photon as `raisa-gateway-local` through the injected `PHOTON_OTLP_*` values, three release
markers in `deployments`, and every step in the audit log. That run found and fixed: the agent
applied stacks inline so a 17-minute pull showed the host offline (applies and actions now run on
background tasks); "recreate volumes" used `up -V`, which never touches named volumes (now
`down --volumes` + `up`); an optional secret was demanded anyway; a failed apply kept only
"exit status: 1" (now the transcript tail); the Photon block only spoke OTel names (now also
`PHOTON_OTLP_HTTP_ENDPOINT` / `PHOTON_OTLP_TOKEN`); the SSH bootstrap refused sudo-with-password
accounts (now `sudo -S`). Testing on 192.168.103.76 itself was stopped: the laptop's binaries need
glibc 2.39 and the server has 2.35, so a build on that machine (30 cores, gcc present, no cargo)
or a release build against an older glibc is the way there.

### Docs
`docs/subsystems/deploy.md` (with a Backend section), `docs/subsystems/auth.md`,
`docs/architecture.md`, `CLAUDE.md`, `deploy/README.md` (a fleet-management part plus the new agent
flags and env vars), `photon.example.toml`, `.env.example`, `Dockerfile`.

### The machine
Builds were killing the session: systemd-oomd kills the whole desktop session when a cargo build
pushes memory pressure past 50% for 20 s. Cargo is now capped at 4 jobs in `~/.cargo/config.toml`.
The permanent fix needs sudo (a `ManagedOOMMemoryPressure=no` drop-in for `user@.service`). The
debug tree also filled the disk twice; `target/debug` reaches 44 GB.

## 2. Left

### Requirements not met (see the status marks in `docker-fleet-management.md`)
| Req | Gap | Size |
|---|---|---|
| R2.3, R2.4, R7.2 (tasks 8, 9) | Container stats and Docker events reach the UI through the sync channel only. They are **not** emitted as OTLP metrics/logs, so they are not retained, alertable, or on the Attention Board (an OOM kill is invisible there). | medium — an OTLP emitter in the agent, reusing the metrics sender |
| R7.4 (task 33) | No post-deploy health gate over uptime / alert rules / SLO burn; a rollout target turns healthy after a grace timer with its containers running. | medium — a `HealthSource` seam in `photon-api` over the existing engines |
| R1.5 | The agent trusts the system CA roots; no hostname + certificate/public-key pinning of the server. | small |
| R9.2 | Image-registry credentials per Site (for private/mirror registries) do not exist; "registries" today are template sources. | medium |
| R9.1 | Air-gapped path: no per-Site mirror registry, no resumable pulls, no offline bundle (`docker save`) flow, not even documented. | large |
| R8.6 | No two-person approval for rollouts to production Sites. | small–medium |
| R4.7 | Git-backed stacks. | large, optional |
| R7.3 | A stack links to the APM services it produced; the reverse link (service → stack) is not there. | small |
| R1.8 | Desired documents are authenticated by the channel and schema-validated, not signed. | small if wanted |
| R1.9.2 | The SSH bootstrap runs from whichever Photon the operator is logged into; nothing enforces "Site, not Central". | small (policy + copy) |
| R9.4 | The protocol is versioned (v1) and a newer-than-server agent is refused, but there is only one version, so "support one older version" is untested by construction. | none now |

### Built but not exercised for real
- **SSH bootstrap** (R1.9) and **agent self-upgrade** (R1.10): unit-tested and protocol-tested, never
  run against a real remote host. Worth one afternoon with a VM.
- **Rollouts** (R6): end-to-end tested with a fake agent only; never with several real hosts.
- **The UI against the real backend**: the live run drove the API; a browser walkthrough of every
  screen with the backend behind it (not the mock) is still owed.

### Engineering leftovers
- Commit the work (86 changed files, none committed). Suggest one commit per layer.
- The agent and server binaries are glibc-dynamic and the laptop builds them against glibc 2.39;
  server 76 (Ubuntu 22.04) has 2.35 and refused them. Build on the target (it has gcc and 30 cores;
  install rustup there) or build the release in a `rust:1-bullseye` container so the served agent
  runs on any host with glibc ≥ 2.31. A musl static agent would lose GPU metrics (nvml dlopen).
- Federation decision 5 (does a Site relay for its Hosts) is unresolved; agents dial the server
  they were enrolled on.
- The first shell command in a session takes ~4 s (the agent learns of the session on its next
  sync, then starts the exec); the rest answer at once.
- One federation e2e test is flaky under a full parallel test run (passes alone).
- Run the sudo oomd fix and keep an eye on `df -h /`.
