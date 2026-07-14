#!/usr/bin/env bash
# Concurrency sweep: is ingest throughput CPU-saturated or concurrency-limited?
# A FRESH throwaway server on tmpfs (fsync ~ free, so this isolates the CPU/latency ceiling) is
# started PER concurrency point on an empty data dir, then loadgen --saturate drives it. Records
# throughput, request-acceptance %, ack latency, and the SERVER's own CPU-cores-used (from
# /proc/<pid>/stat utime+stime) so we can see whether the server is saturating cores or just waiting.
#
# Why fresh-server-per-point: a single long-lived server accumulates WAL + hot files on the
# size-limited tmpfs across the whole sweep, so the later (higher-concurrency) points fail on
# ENOSPC ("Disk quota exceeded") rather than on any real server limit — a confound. Restarting
# on an empty dir measures each concurrency in isolation; the acc% column still flags any single
# point that fills mid-run (acc < 100 with no other cause).
#
# Env: CONC_LIST (default "8 16 32 64 128 256"), BATCH (500), SERVICES (10), PER_SECS (20),
#      PHOTON_BENCH_DIR (/dev/shm/photon-sweep).
set -euo pipefail
CONC_LIST="${CONC_LIST:-8 16 32 64 128 256}"
BATCH="${BATCH:-500}"
SERVICES="${SERVICES:-10}"
PER_SECS="${PER_SECS:-20}"
BENCH_DIR="${PHOTON_BENCH_DIR:-/dev/shm/photon-sweep}"
TOKEN="dev-ingest-token"
API_ADDR="127.0.0.1:18080"
HTTP_ADDR="127.0.0.1:14318"
GRPC_ADDR="127.0.0.1:14317"
CLK="$(getconf CLK_TCK)"
CONFIG="$BENCH_DIR/photon.sweep.toml"

cd "$(dirname "$0")/.."
[ -d frontend/dist ] || { echo "error: build frontend/dist first (cd frontend && bun run build)" >&2; exit 1; }

echo ">> building release server + loadgen…"
cargo build --release -p photon-server -p photon-loadgen

# (Re)create an empty data dir + config. Called before every concurrency point so each runs on a
# clean disk (see header). Config matches scripts/bench-ingest.sh.
write_config() {
  rm -rf "$BENCH_DIR"; mkdir -p "$BENCH_DIR/hot"
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
session_secret = "photon-sweep-session-secret-not-for-production-0123456789"
EOF
}

# Sum of utime+stime (clock ticks) for the server. /proc/<pid>/stat field 2 (comm) may contain
# spaces/parens, so strip everything up to ") " first; then utime is field 14 -> index 12 of the
# remainder (1-based) -> a[11] (0-based), stime -> a[12].
read_cpu() { local s; s=$(cat "/proc/$1/stat"); s=${s#*) }; local a=($s); echo $(( ${a[11]} + ${a[12]} )); }

# Block until the OTLP/HTTP port accepts a connection (server ready), or fail after ~10s.
wait_port() {
  for _ in $(seq 1 100); do
    (exec 3<>"/dev/tcp/127.0.0.1/${HTTP_ADDR##*:}") 2>/dev/null && { exec 3>&- 3<&-; return 0; }
    sleep 0.1
  done
  return 1
}

SRV=""
trap '[ -n "$SRV" ] && kill "$SRV" 2>/dev/null; true' EXIT

printf '%-6s %-12s %-8s %-6s %-7s %-7s %-7s %-9s\n' conc logs/s MB/s acc% p50ms p95ms p99ms srv_cores
for C in $CONC_LIST; do
  write_config
  PHOTON_API_ADDR="$API_ADDR" ./target/release/photon-server "$CONFIG" >"$BENCH_DIR/server.log" 2>&1 &
  SRV=$!
  if ! wait_port; then
    echo "server failed to start for conc=$C (see $BENCH_DIR/server.log)" >&2
    kill "$SRV" 2>/dev/null || true; wait "$SRV" 2>/dev/null || true; SRV=""
    printf '%-6s %-12s %-8s %-6s %-7s %-7s %-7s %-9s\n' "$C" START_FAIL NA NA NA NA NA NA
    continue
  fi

  c0=$(read_cpu "$SRV"); t0=$(date +%s.%N)
  SUMMARY=$(./target/release/photon-loadgen logs --saturate \
    --endpoint "http://$HTTP_ADDR/v1/logs" --token "$TOKEN" \
    --concurrency "$C" --services "$SERVICES" --batch "$BATCH" --duration "$PER_SECS" 2>&1 1>/dev/null)
  t1=$(date +%s.%N); c1=$(read_cpu "$SRV")

  logs_s=$(echo "$SUMMARY" | grep 'logs accepted' | grep -oE '\([0-9]+/s' | tr -dc '0-9')
  mbps=$(echo "$SUMMARY"   | grep 'data sent'     | grep -oE '[0-9.]+ MB/s' | grep -oE '[0-9.]+')
  # Latency: anchor the number to end-of-token ([0-9.]+$) so the "50" in the "p50" LABEL is not
  # also captured — without the $, `p50 7.4` yields both "50" and "7.4" and the column wraps.
  p50=$(echo "$SUMMARY"    | grep 'ack latency'   | grep -oE 'p50 [0-9.]+' | grep -oE '[0-9.]+$')
  p95=$(echo "$SUMMARY"    | grep 'ack latency'   | grep -oE 'p95 [0-9.]+' | grep -oE '[0-9.]+$')
  p99=$(echo "$SUMMARY"    | grep 'ack latency'   | grep -oE 'p99 [0-9.]+' | grep -oE '[0-9.]+$')
  # Request acceptance %. `data sent` counts bytes for EVERY request but `logs accepted` counts
  # only 2xx, so a throughput drop is ambiguous without this: acc≈100 while throughput plateaus
  # and cores climb => CPU-bound; acc falling => the server is SHEDDING load (rejecting or the
  # disk filled), a different lever. Line: "ok / http-err / net-err   OK / HE / NE".
  errline=$(echo "$SUMMARY" | grep 'http-err' | grep -oE '[0-9]+ / [0-9]+ / [0-9]+')
  ok=$(echo "$errline" | awk '{print $1}'); he=$(echo "$errline" | awk '{print $3}'); ne=$(echo "$errline" | awk '{print $5}')
  acc=$(awk "BEGIN{t=${ok:-0}+${he:-0}+${ne:-0}; if(t>0) printf \"%.0f\", 100*${ok:-0}/t; else printf \"NA\"}")
  cores=$(awk "BEGIN{printf \"%.1f\", ($c1-$c0)/$CLK/($t1-$t0)}")

  printf '%-6s %-12s %-8s %-6s %-7s %-7s %-7s %-9s\n' "$C" "${logs_s:-NA}" "${mbps:-NA}" "${acc:-NA}" "${p50:-NA}" "${p95:-NA}" "${p99:-NA}" "$cores"

  kill "$SRV" 2>/dev/null || true; wait "$SRV" 2>/dev/null || true; SRV=""
done
