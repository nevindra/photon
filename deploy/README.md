# Deploying Photon with Docker

Photon ships as a single ~30 MB non-root container: the Vue SPA, the REST/UI API, and both
OTLP receivers are all served by one `photon-server` process. Configuration is entirely via
`PHOTON_*` environment variables — no config file is required.

## Quick start

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

**RUM apps (`[[rum.apps]]`) have no `PHOTON_*` env var equivalent** — it's a repeatable table
(name/key/allowed_origins/sample_rate/rate_limit per frontend app), which doesn't map onto flat
env vars. To enable the `POST /api/rum` beacon, mount a `photon.toml` with one or more
`[[rum.apps]]` entries (see `photon.example.toml`) instead of using `.env` alone; other
`PHOTON_*` vars still override individual fields on top of it.

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
