# Deploying Photon with Docker

Photon ships as a single ~30 MB non-root container: the web UI, the REST/UI API, and both
OTLP receivers are all served by one `photon-server` process. Configuration is entirely via
`PHOTON_*` environment variables — no config file is required.

**The pieces, in plain terms:**

| Piece | What it is | Where it runs |
|---|---|---|
| **Photon server** | The observability platform itself — stores your logs/traces/metrics and serves the dashboard you open in a browser | One machine, in Docker (this guide) |
| **photon-agent** | A small program that reports a machine's CPU, memory, disk, network, and GPU to the server | On **every machine you want to monitor** ([see below](#monitoring-your-machines-photon-agent)) |
| **Your applications** | Send their own logs/traces/metrics via OpenTelemetry, and browser data via the `@photon/rum` SDK | Wherever they already run |

## Quick start (the server)

```bash
cp .env.example .env
# Fill in secrets. Generate strong values:
#   openssl rand -hex 32   # for PHOTON_INGEST_TOKEN
#   openssl rand -hex 32   # for PHOTON_SESSION_SECRET
$EDITOR .env

make docker-up          # docker compose up -d --build
```

Open http://localhost:8080, complete the one-time "create your account" onboarding, and log in.

Ports: **8080** UI+API · **4317** OTLP gRPC · **4318** OTLP HTTP.

Send data (OTLP/HTTP protobuf) to `http://<host>:4318/v1/logs` (and `/v1/traces`, `/v1/metrics`)
with header `Authorization: Bearer $PHOTON_INGEST_TOKEN`.

## Monitoring your machines (photon-agent)

The server you just started only *receives* data — it doesn't know anything about your
machines yet. To see a machine's CPU, memory, disk, network, and GPU in the
**Infrastructure → Hosts** page, run `photon-agent` on that machine.

Two things to know up front:

- The agent runs **directly on the machine** (not inside Docker) — it has to see the real
  hardware, not a container's view of it.
- It needs exactly **two settings**: where your Photon server is, and the ingest token from
  your `.env`. There is no registration step — the moment the agent starts sending, the host
  appears in the UI (within ~15 seconds).

### 1. Get the agent binary

The Docker image does not include the agent — build it once from this repository (needs the
[Rust toolchain](https://rustup.rs)):

```bash
cargo build --release -p photon-agent
# → binary at target/release/photon-agent
```

The binary is self-contained. To monitor other machines of the **same OS/architecture**, just
copy it over — no Rust needed on the target:

```bash
scp target/release/photon-agent user@machine:/usr/local/bin/photon-agent
```

(For a different OS/architecture, build on a matching machine or cross-compile.)

### 2. Try it

On the machine you want to monitor:

```bash
PHOTON_INGEST_TOKEN=<PHOTON_INGEST_TOKEN from your .env> \
photon-agent --endpoint http://<photon-server-address>:4318/v1/metrics
```

Replace `<photon-server-address>` with the address of the machine running the Docker server
(on that same machine, `127.0.0.1` works). Then open the UI → **Infrastructure → Hosts**:
the machine appears within ~15 seconds, and clicking it opens per-resource charts.

The agent is silent while everything works and prints a line only on errors — no output is
good news.

### 3. Keep it running (systemd service)

So the agent survives reboots, install it as a service. Create
`/etc/systemd/system/photon-agent.service`:

```ini
[Unit]
Description=Photon host metrics agent
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/photon-agent
Environment=PHOTON_AGENT_ENDPOINT=http://<photon-server-address>:4318/v1/metrics
Environment=PHOTON_INGEST_TOKEN=<PHOTON_INGEST_TOKEN from your .env>
Restart=always
RestartSec=5
DynamicUser=yes

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now photon-agent
systemctl status photon-agent     # should show "active (running)"
```

### Agent options

Every flag has an environment-variable equivalent (flags win):

| Flag | Env var | Default | Purpose |
|---|---|---|---|
| `--endpoint` | `PHOTON_AGENT_ENDPOINT` | `http://127.0.0.1:4318/v1/metrics` | Where to send metrics |
| `--token` | `PHOTON_INGEST_TOKEN` | `dev-ingest-token` | Must match the server's ingest token |
| `--host-name` | `PHOTON_AGENT_HOST` | the OS hostname | The name shown in the UI |
| `--interval-secs` | `PHOTON_AGENT_INTERVAL` | `15` | Seconds between samples |
| `--no-gpu` | `PHOTON_AGENT_NO_GPU` | off | Skip GPU sampling |

**GPU metrics** (NVIDIA) are automatic: if the machine has an NVIDIA driver, utilization,
GPU memory, temperature, and power appear on the host page; if not, the agent simply runs
without them. No configuration either way.

### If the host doesn't show up

- **Wrong token** — must be exactly the server's `PHOTON_INGEST_TOKEN`. The agent prints
  `ingest returned 401` if it isn't.
- **Server unreachable** — the machine must reach port **4318** on the server (check
  firewalls). The agent prints `send failed: …` if it can't connect.
- **Wrong path** — the endpoint ends in `/v1/metrics`, not just the host and port.
- **Looking too far back** — hosts only appear when they reported inside the selected time
  range; keep it at "Last 15m" while testing.
- **GPU missing but expected** — the agent reads NVIDIA's driver via NVML; make sure the
  user it runs as can read `/dev/nvidia*` (worst case, add it to the `video` group).

## Sending application telemetry

- **Backend services**: point any OpenTelemetry SDK/collector at
  `http://<host>:4318` (HTTP) or `:4317` (gRPC) with the
  `Authorization: Bearer $PHOTON_INGEST_TOKEN` header. A stock OTel Collector works as-is.
- **Browser (RUM)**: RUM apps are managed **in the UI** — create an app under RUM, copy the
  generated public key (`pk_live_…`) into the `@photon/rum` snippet, and set the allowed
  origins there. No server configuration is involved, and the beacon endpoint
  (`POST /api/rum`) is always on; beacons from unregistered apps are rejected with a 403.

## Configuration (environment variables)

Secrets and settings are read from the environment (compose loads `.env` automatically). A
fully-mounted `photon.toml` still works too (`-v ./photon.toml:/etc/photon/photon.toml:ro`
with `PHOTON_CONFIG=/etc/photon/photon.toml`); env vars override individual fields.

| Variable | Required | Default | Purpose |
|---|---|---|---|
| `PHOTON_INGEST_TOKEN` | ✅ | — | OTLP ingest bearer token |
| `PHOTON_SESSION_SECRET` | ✅ | — | UI session cookie signing secret (≥ 32 bytes) |
| `PHOTON_API_ADDR` | | `0.0.0.0:8080` | UI/REST bind |
| `PHOTON_INGEST_HTTP_ADDR` | | `0.0.0.0:4318` | OTLP/HTTP bind |
| `PHOTON_INGEST_GRPC_ADDR` | | `0.0.0.0:4317` | OTLP/gRPC bind |
| `PHOTON_INGEST_MAX_IN_FLIGHT` | | `256` | Max in-flight ingest requests |
| `PHOTON_STORAGE_HOT_DIR` | | `/var/lib/photon/hot` | Hot tier (WAL + Parquet + index) |
| `PHOTON_STORAGE_DB_PATH` | | `/var/lib/photon/photon.db` | Control-plane SQLite db |
| `PHOTON_RETENTION_DAYS` | | `30` | Log retention (days, > 0) |
| `PHOTON_PROMOTED_ATTRIBUTES` | | `service.name,host.name` | Comma-separated promoted columns (must include `service.name`) |
| `PHOTON_UPTIME_ENABLED` | | `false` | Enable the uptime subsystem with defaults |
| `PHOTON_APM_DEFAULT_APDEX_THRESHOLD_MS` | | `500` | Default Apdex T (ms) |
| `PHOTON_DURABLE_ENDPOINT` | | — | S3 endpoint; **presence enables durable replication** |
| `PHOTON_DURABLE_BUCKET` | when durable | — | S3 bucket |
| `PHOTON_DURABLE_REGION` | when durable | — | S3 region |
| `PHOTON_DURABLE_ACCESS_KEY_ID` | | — | S3 access key (optional; IAM/env otherwise) |
| `PHOTON_DURABLE_SECRET_ACCESS_KEY` | | — | S3 secret key |

An unconfigured server (no token/secret) **refuses to start** — this is intentional.

## Data & persistence

All state lives under `/var/lib/photon` (hot dir + SQLite db), mounted as the `photon-data`
named volume. `make docker-down` keeps it; `docker compose down -v` deletes it.

**Permissions:** the container runs as uid **65532**. Fresh *named* volumes inherit the image's
ownership automatically. If you instead **bind-mount** a host directory, pre-create it owned by
65532:

```bash
mkdir -p ./photon-data && sudo chown -R 65532:65532 ./photon-data
```

## Durable S3 tier (optional, Garage)

An opt-in profile adds [Garage](https://garagehq.deuxfleurs.fr/) as the S3-compatible durable
replica. Photon always acks after the local WAL fsync; replication is asynchronous and never on
the query path.

```bash
# In .env, uncomment the durable block from .env.example (it ships working dev defaults):
#   PHOTON_DURABLE_ENDPOINT=http://garage:3900
#   PHOTON_DURABLE_BUCKET=photon
#   PHOTON_DURABLE_REGION=garage
#   PHOTON_DURABLE_ACCESS_KEY_ID=GK43ba46c38ae6886c283896c1
#   PHOTON_DURABLE_SECRET_ACCESS_KEY=<64 hex chars>
# The key id/secret MUST be valid Garage formats — id = "GK" + 24 hex chars, secret = 64 hex
# chars — or `garage key import` rejects them. Regenerate for production:
#   id:     printf 'GK%s' "$(openssl rand -hex 12)"
#   secret: openssl rand -hex 32

make docker-up-durable   # docker compose --profile durable up -d --build
```

`garage-init` runs once to assign a cluster layout, import the key (so the `PHOTON_DURABLE_*`
credentials match), and create+grant the `photon` bucket. It is idempotent. Note that
replication only fires once a WAL segment closes and compacts (age `segment_max_age_secs`,
default 60s, or size), so a brief ingest burst may not produce objects immediately — drive a
sustained load to see them. Verify:

```bash
docker exec photon-garage /garage bucket info photon
```

For production Garage, regenerate `rpc_secret` and `admin_token` in `deploy/garage.toml` and use
strong key material.

## HTTPS

Photon serves plain HTTP. For TLS, front it with a reverse proxy (Caddy, nginx, Traefik)
terminating HTTPS and proxying to `:8080` (and, if exposing ingest over TLS, `:4318`/`:4317`).

## Health

`photon-server healthcheck` (used by the container `HEALTHCHECK`) exits 0 when the API port is
accepting connections. `docker inspect -f '{{.State.Health.Status}}' photon` reports status.
