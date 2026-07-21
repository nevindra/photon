# Deploying Photon

This guide takes you from an empty server to a working observability stack: the **Photon
server** running in Docker, **photon-agent** reporting your machines' health, and your
applications sending telemetry. Each step says *what* to run and *why*, so it works whether
you deploy software daily or this is your first `docker compose`.

**The pieces, in plain terms:**

| Piece | What it is | Where it runs |
|---|---|---|
| **Photon server** | The observability platform itself — stores your logs/traces/metrics and serves the dashboard you open in a browser | One machine, in Docker ([Part 1](#part-1--run-the-photon-server)) |
| **photon-agent** | A small program that reports a machine's CPU, memory, disk, network, and GPU to the server | On **every machine you want to monitor** ([Part 2](#part-2--monitor-your-machines-photon-agent)) |
| **Your applications** | Send their own logs/traces/metrics via OpenTelemetry, and browser data via the `@photon/rum` SDK | Wherever they already run ([Part 3](#part-3--send-application-telemetry)) |

**What you need:**

- One Linux server (or your laptop) with [Docker + Docker Compose](https://docs.docker.com/engine/install/) installed.
- Photon is a **single container** — no external database, no cluster. All its data lives in
  one Docker volume. A small VM handles a surprising amount of telemetry.
- Three ports reachable by the things that talk to it: **8080** (the web UI — you), **4318**
  and **4317** (telemetry ingest — your agents/apps).

---

## Part 1 — Run the Photon server

Photon needs two things before it will start: a Docker image, and a `.env` file holding two
secrets. First, the secrets — they're the same for both options below.

**Your two secrets.** Create an empty folder anywhere (say `~/photon`) and generate them:

```bash
mkdir -p ~/photon && cd ~/photon
cat > .env <<EOF
PHOTON_INGEST_TOKEN=$(openssl rand -hex 32)
PHOTON_SESSION_SECRET=$(openssl rand -hex 32)
EOF
```

| Secret | What it protects |
|---|---|
| `PHOTON_INGEST_TOKEN` | The "password" agents and apps must present to *send data in*. Anyone with it can write telemetry into your Photon. |
| `PHOTON_SESSION_SECRET` | Signs the browser login cookie for the *dashboard*. Never shared with anyone. |

These are deliberately two different credentials: machines that send data never hold your
dashboard secret. A server without both **refuses to start** — that's intentional, so an
unsecured Photon can't exist by accident.

Then pick ONE of the two options:

### Option A — you already have the image (no clone, no build)

This is the path if you pulled the release image from GHCR — or someone handed you an image
tarball (`docker load -i photon.tar`; then use whatever tag `docker load` printed instead of
the GHCR one below). Images are pinned by version, there is no `latest` tag — you always know
exactly what you run.

In the same folder as your `.env`, either run the container directly:

```bash
docker run -d --name photon --restart unless-stopped \
  --env-file .env \
  -p 8080:8080 -p 4317:4317 -p 4318:4318 \
  -v photon-data:/var/lib/photon \
  ghcr.io/nevindra/photon:1.3.0
```

…or, nicer to keep around, save this as `docker-compose.yml` next to `.env` and start it
with `docker compose up -d`:

```yaml
services:
  photon:
    image: ghcr.io/nevindra/photon:1.3.0
    container_name: photon
    restart: unless-stopped
    env_file: [.env]
    ports:
      - "8080:8080"   # web UI + API
      - "4317:4317"   # OTLP gRPC (telemetry in)
      - "4318:4318"   # OTLP HTTP (telemetry in)
    volumes:
      - photon-data:/var/lib/photon
volumes:
  photon-data:
```

### Option B — build from source (clone)

If you want to build the image yourself (or run a commit that isn't released yet):

```bash
git clone https://github.com/nevindra/photon.git
cd photon
cp .env.example .env    # then paste your two generated secrets into it
make docker-up          # = docker compose up -d --build; first build takes a few minutes
```

### Check it's up (both options)

```bash
docker ps               # "photon" should show status "healthy" after ~15s
```

### Create your account

Open **http://your-server:8080**. The first visit shows a one-time **"create your account"**
page — this creates the dashboard login (stored inside Photon's own database; it has nothing
to do with the `.env` secrets). After that, it's a normal login page.

That's the server done. The dashboard is live but mostly empty — Parts 2 and 3 fill it.

### Updating to a new version

Your data lives in a Docker **volume**, not in the container — so updating is just replacing
the container; nothing is lost.

- **Option A (image):** change the version in the image tag (compose file or `docker run`
  command), then `docker compose up -d` — or `docker rm -f photon` and `docker run` again.
- **Option B (source):** `git pull && make docker-up` (rebuilds and restarts).

### Backing up

Everything is in the `photon-data` volume (telemetry + settings + your login). To snapshot it:

```bash
docker run --rm -v photon-data:/data -v "$PWD":/backup alpine \
  tar czf /backup/photon-backup.tar.gz -C /data .
```

Restore by extracting into a fresh volume the same way (with `tar xzf`, before first start).

---

## Part 2 — Monitor your machines (photon-agent)

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

Every release ships a prebuilt Linux x86_64 binary on the
[GitHub Releases page](https://github.com/nevindra/photon/releases). On the machine you want
to monitor:

```bash
curl -fsSLO https://github.com/nevindra/photon/releases/latest/download/photon-agent-linux-x86_64.tar.gz
curl -fsSLO https://github.com/nevindra/photon/releases/latest/download/photon-agent-linux-x86_64.tar.gz.sha256
sha256sum -c photon-agent-linux-x86_64.tar.gz.sha256    # verifies the download; must say "OK"
tar xzf photon-agent-linux-x86_64.tar.gz
sudo install -m 755 photon-agent /usr/local/bin/photon-agent
```

<details>
<summary>Building from source instead (other architectures, macOS, or a 404 on the asset)</summary>

Releases older than the binary pipeline don't carry the asset; and the prebuilt binary is
Linux x86_64 only. For anything else, build with the [Rust toolchain](https://rustup.rs):

```bash
cargo build --release -p photon-agent
# → binary at target/release/photon-agent; copy it to same-OS/arch machines freely
```
</details>

### 2. Try it

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

Repeat Part 2 on every machine you want on the Hosts page.

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
without them. No configuration either way. (This is also why the prebuilt binary is
dynamically linked: the NVIDIA library is loaded at runtime.)

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

---

## Part 3 — Send application telemetry

- **Backend services**: point any OpenTelemetry SDK/collector at
  `http://<host>:4318` (HTTP) or `:4317` (gRPC) with the
  `Authorization: Bearer $PHOTON_INGEST_TOKEN` header. A stock OTel Collector works as-is.
- **Browser (RUM)**: RUM apps are managed **in the UI** — create an app under RUM, copy the
  generated public key (`pk_live_…`) into the `@photon/rum` snippet, and set the allowed
  origins there. No server configuration is involved, and the beacon endpoint
  (`POST /api/rum`) is always on; beacons from unregistered apps are rejected with a 403.

---

## Reference: configuration (environment variables)

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

## Data & persistence

All of Photon's state — telemetry, settings, your login — lives in **one directory inside the
container**: `/var/lib/photon`. Persistence is simply about mapping that directory somewhere
durable. Two ways:

**Way 1 — named volume (the default; both options already do this).** The `-v
photon-data:/var/lib/photon` line (docker run) / `photon-data` volume (compose) makes Docker
manage the storage. It survives container restarts, updates, and `docker compose down`; it is
deleted only by an explicit `docker compose down -v` or `docker volume rm photon-data`. Fresh
named volumes inherit the right ownership automatically — zero setup, recommended unless you
have a reason not to.

**Way 2 — bind mount (data in a folder you can see).** If you'd rather have the data at a
real path you control (say `/srv/photon-data`, on a disk you back up), pre-create the folder
owned by uid **65532** (the non-root user the container runs as — this step is why Way 1 is
the default):

```bash
sudo mkdir -p /srv/photon-data && sudo chown -R 65532:65532 /srv/photon-data
```

then swap the volume mapping to `-v /srv/photon-data:/var/lib/photon` (docker run) or

```yaml
    volumes:
      - /srv/photon-data:/var/lib/photon
```

in your compose file, and start it again. Backing up becomes a plain `tar` of that folder
(stop the container first, or accept a crash-consistent copy — Photon's WAL recovers it).
The volume-based backup command in Part 1 is only needed for Way 1.

## Durable S3 tier (optional, Garage)

By default Photon keeps data on local disk only. Optionally, it can **replicate every stored
file to an S3-compatible object store** as a second copy — Photon always acks after the local
WAL fsync; replication is asynchronous and never on the query or ingest path. Any S3 works
(set the `PHOTON_DURABLE_*` variables and you're done); this section covers self-hosting
[Garage](https://garagehq.deuxfleurs.fr/), the bundled choice, next to Photon. Skip it
entirely if local disk (+ your own backups) is enough.

> The durable tier assumes Photon runs under **compose** (the containers reach each other by
> service name — `http://garage:3900`). If you started with a bare `docker run`, switch to
> the compose file from Part 1 Option A first.

### Option A — you already have the image (no clone)

**1.** Next to your `docker-compose.yml`, save this as `garage.toml` (single-node Garage;
regenerate `rpc_secret`/`admin_token` for production — `openssl rand -hex 32`):

```toml
metadata_dir = "/var/lib/garage/meta"
data_dir = "/var/lib/garage/data"
db_engine = "sqlite"

replication_factor = 1

rpc_secret = "<openssl rand -hex 32>"
rpc_bind_addr = "0.0.0.0:3901"
rpc_public_addr = "127.0.0.1:3901"

[s3_api]
s3_region = "garage"
api_bind_addr = "0.0.0.0:3900"
root_domain = ".s3.garage"

[admin]
api_bind_addr = "0.0.0.0:3903"
admin_token = "<openssl rand -hex 32>"
```

**2.** Add a `garage` service to your `docker-compose.yml` (alongside the `photon` service),
and the two volumes:

```yaml
  garage:
    image: dxflrs/garage:v1.0.1
    container_name: photon-garage
    restart: unless-stopped
    volumes:
      - ./garage.toml:/etc/garage.toml:ro
      - garage-meta:/var/lib/garage/meta
      - garage-data:/var/lib/garage/data
```

```yaml
volumes:
  photon-data:
  garage-meta:
  garage-data:
```

**3.** Start Garage and initialize it once (layout → credentials → bucket). These run *inside*
the Garage container, so no extra tooling is needed:

```bash
docker compose up -d garage

# Generate S3 credentials in Garage's required formats:
KEY_ID="GK$(openssl rand -hex 12)"          # "GK" + 24 hex chars
KEY_SECRET="$(openssl rand -hex 32)"        # 64 hex chars
echo "$KEY_ID $KEY_SECRET"                  # save these — they go into .env below

# (if this errors, Garage is still booting — wait a few seconds and re-run)
ID=$(docker exec photon-garage /garage node id -q | cut -d@ -f1)
docker exec photon-garage /garage layout assign -z dc1 -c 1G "$ID"
docker exec photon-garage /garage layout apply --version 1
docker exec photon-garage /garage key import -n photon-key "$KEY_ID" "$KEY_SECRET" --yes
docker exec photon-garage /garage bucket create photon
docker exec photon-garage /garage bucket allow --read --write --owner photon --key photon-key
```

**4.** Tell Photon about it — append to `.env`:

```bash
PHOTON_DURABLE_ENDPOINT=http://garage:3900
PHOTON_DURABLE_BUCKET=photon
PHOTON_DURABLE_REGION=garage
PHOTON_DURABLE_ACCESS_KEY_ID=<the KEY_ID from step 3>
PHOTON_DURABLE_SECRET_ACCESS_KEY=<the KEY_SECRET from step 3>
```

then `docker compose up -d` (recreates `photon` with the new env; presence of
`PHOTON_DURABLE_ENDPOINT` is what switches replication on).

### Option B — from the clone

The repo's compose file ships all of the above as an opt-in `durable` profile with an
idempotent one-shot `garage-init` container (it assigns the layout, imports the key from your
`.env`, and creates+grants the bucket automatically):

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

For production, regenerate `rpc_secret` and `admin_token` in `deploy/garage.toml` and use
strong key material.

### Verifying (both options)

Replication only fires once a WAL segment closes and compacts (age `segment_max_age_secs`,
default 60s, or size), so a brief ingest burst may not produce objects immediately — drive a
sustained load, then:

```bash
docker exec photon-garage /garage bucket info photon
```

## HTTPS

Photon serves plain HTTP. For TLS, front it with a reverse proxy (Caddy, nginx, Traefik)
terminating HTTPS and proxying to `:8080` (and, if exposing ingest over TLS, `:4318`/`:4317`).

## Health & troubleshooting (server)

- `photon-server healthcheck` (used by the container `HEALTHCHECK`) exits 0 when the API port
  is accepting connections. `docker inspect -f '{{.State.Health.Status}}' photon` reports it.
- **Container exits immediately on first start** → almost always a missing/empty
  `PHOTON_INGEST_TOKEN` or `PHOTON_SESSION_SECRET`; `docker logs photon` says which.
- **UI unreachable from another machine** → port 8080 blocked by a firewall/security group,
  or you're browsing `http://localhost` from the wrong machine.
- **Agents/apps get connection errors but the UI works** → 4318/4317 aren't open; they're
  separate ports from the UI's 8080.
