#!/usr/bin/env bash
# End-to-end logs-ingest benchmark: drive photon-loadgen --saturate at a throwaway
# photon-server and report sustained records/sec + peak RSS. Linux-only (reads /proc).
#
# Env knobs (all optional):
#   PHOTON_BENCH_DIR   data dir. Default /dev/shm/photon-bench (tmpfs → CPU ceiling).
#                      Set to a path on the real disk for the deliverable number.
#   BENCH_DURATION     loadgen run seconds (default 30)
#   BENCH_CONCURRENCY  in-flight senders (default 32)
#   BENCH_SERVICES     service.name cardinality (default 10)
#   BENCH_BATCH        log records per request (default 500)
set -euo pipefail

BENCH_DIR="${PHOTON_BENCH_DIR:-/dev/shm/photon-bench}"
DURATION="${BENCH_DURATION:-30}"
CONCURRENCY="${BENCH_CONCURRENCY:-32}"
SERVICES="${BENCH_SERVICES:-10}"
BATCH="${BENCH_BATCH:-500}"
TOKEN="dev-ingest-token"
API_ADDR="127.0.0.1:18080"   # off the default :8080 so a running `make dev` doesn't clash
HTTP_ADDR="127.0.0.1:14318"
GRPC_ADDR="127.0.0.1:14317"

cd "$(dirname "$0")/.."

if [ ! -d frontend/dist ]; then
  echo "error: frontend/dist missing (photon-api embeds it at build time)." >&2
  echo "       run once:  cd frontend && bun install && bun run build" >&2
  exit 1
fi

echo ">> building release server + loadgen…"
cargo build --release -p photon-server -p photon-loadgen

rm -rf "$BENCH_DIR"
mkdir -p "$BENCH_DIR/hot"
CONFIG="$BENCH_DIR/photon.bench.toml"
cat > "$CONFIG" <<EOF
[ingest]
token = "$TOKEN"
http_addr = "$HTTP_ADDR"
grpc_addr = "$GRPC_ADDR"
[storage]
hot_dir = "$BENCH_DIR/hot"
db_path = "$BENCH_DIR/photon.db"
[retention]
days = 7
[schema]
promoted_attributes = ["service.name", "host.name"]
[wal]
segment_max_bytes = 134217728
segment_max_age_secs = 3600
group_commit_max_delay_ms = 5
[auth]
session_secret = "photon-bench-session-secret-not-for-production-0123456789"
EOF

echo ">> starting server (data dir: $BENCH_DIR)…"
PHOTON_API_ADDR="$API_ADDR" ./target/release/photon-server "$CONFIG" >"$BENCH_DIR/server.log" 2>&1 &
SRV=$!
trap 'kill "$SRV" 2>/dev/null || true' EXIT

# Wait for the OTLP/HTTP port to accept connections.
for _ in $(seq 1 100); do
  (exec 3<>"/dev/tcp/127.0.0.1/${HTTP_ADDR##*:}") 2>/dev/null && { exec 3>&- 3<&-; break; }
  sleep 0.1
done

echo ">> running loadgen --saturate for ${DURATION}s…"
SUMMARY=$(./target/release/photon-loadgen logs --saturate \
  --endpoint "http://$HTTP_ADDR/v1/logs" --token "$TOKEN" \
  --concurrency "$CONCURRENCY" --services "$SERVICES" --batch "$BATCH" \
  --duration "$DURATION" 2>&1 1>/dev/null)   # final summary is on stderr

# Peak RSS while the server is still alive (VmHWM = peak resident set size).
PEAK_RSS_KB=$(awk '/VmHWM/ {print $2}' "/proc/$SRV/status")

echo
echo "──── ingest benchmark result ────"
echo "  backing dir     $BENCH_DIR"
echo "  duration        ${DURATION}s  concurrency=${CONCURRENCY} services=${SERVICES} batch=${BATCH}"
echo "$SUMMARY" | grep -E "logs accepted|data sent|ack latency" | sed 's/^/  /'
echo "  peak RSS        $((PEAK_RSS_KB / 1024)) MiB"
