#!/usr/bin/env bash
# Memory profile: sample the server's RSS during a load run, then keep sampling through a
# post-load idle tail ("does it come back down?"). Vary ONE knob per run to attribute the
# ~6 GiB footprint:
#   - SEGMENT_MAX_BYTES  smaller vs larger  -> compaction-buffer share (doc 04's 3x-copy hypothesis)
#   - CONCURRENCY / BATCH                    -> in-flight-batch share
#   - RATE (vs saturate)                     -> load-dependent vs fixed baseline
# Caveat: glibc malloc may not return freed pages to the OS, so "post-idle stays high" is
# ambiguous (retained OR allocator-held); "post-idle drops a lot" is unambiguously transient.
# The segment-size sweep is the sharper attribution probe.
#
# Env: SEGMENT_MAX_BYTES (134217728), CONCURRENCY (32), BATCH (500), SERVICES (10),
#      RATE ("" = saturate; else logs/s), LOAD_SECS (40), IDLE_SECS (30), SAMPLE_MS (500),
#      LABEL ("run"), PHOTON_BENCH_DIR (/dev/shm/photon-mem).
set -euo pipefail
SEGMENT_MAX_BYTES="${SEGMENT_MAX_BYTES:-134217728}"
CONCURRENCY="${CONCURRENCY:-32}"
BATCH="${BATCH:-500}"
SERVICES="${SERVICES:-10}"
RATE="${RATE:-}"
LOAD_SECS="${LOAD_SECS:-40}"
IDLE_SECS="${IDLE_SECS:-30}"
SAMPLE_MS="${SAMPLE_MS:-500}"
LABEL="${LABEL:-run}"
BENCH_DIR="${PHOTON_BENCH_DIR:-/dev/shm/photon-mem}"
TOKEN="dev-ingest-token"
API_ADDR="127.0.0.1:18080"
HTTP_ADDR="127.0.0.1:14318"
GRPC_ADDR="127.0.0.1:14317"

cd "$(dirname "$0")/.."
[ -d frontend/dist ] || { echo "error: build frontend/dist first (cd frontend && bun run build)" >&2; exit 1; }
cargo build --release -p photon-server -p photon-loadgen >/dev/null

rm -rf "$BENCH_DIR"; mkdir -p "$BENCH_DIR/hot"
CONFIG="$BENCH_DIR/photon.mem.toml"
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
segment_max_bytes = $SEGMENT_MAX_BYTES
segment_max_age_secs = 3600
group_commit_max_delay_ms = 5
[auth]
session_secret = "photon-mem-session-secret-not-for-production-0123456789"
EOF

PHOTON_API_ADDR="$API_ADDR" ./target/release/photon-server "$CONFIG" >"$BENCH_DIR/server.log" 2>&1 &
SRV=$!
trap 'kill "$SRV" 2>/dev/null || true' EXIT
for _ in $(seq 1 100); do
  (exec 3<>"/dev/tcp/127.0.0.1/${HTTP_ADDR##*:}") 2>/dev/null && { exec 3>&- 3<&-; break; }
  sleep 0.1
done

SERIES="$BENCH_DIR/rss-${LABEL}.tsv"
: > "$SERIES"
START=$(date +%s.%N)
SLEEP_S=$(awk "BEGIN{print $SAMPLE_MS/1000}")
(
  while kill -0 "$SRV" 2>/dev/null; do
    now=$(date +%s.%N)
    r=$(awk '/VmRSS/{print $2}' "/proc/$SRV/status" 2>/dev/null) || break
    [ -n "$r" ] && awk "BEGIN{printf \"%.1f\t%d\n\", $now-$START, $r/1024}" >> "$SERIES"
    sleep "$SLEEP_S"
  done
) &
SAMP=$!

if [ -n "$RATE" ]; then MODE=(--rate "$RATE"); else MODE=(--saturate); fi
echo ">> loading [$LABEL] mode=${RATE:-saturate} seg=$((SEGMENT_MAX_BYTES/1048576))MiB conc=$CONCURRENCY batch=$BATCH for ${LOAD_SECS}s…"
./target/release/photon-loadgen logs "${MODE[@]}" \
  --endpoint "http://$HTTP_ADDR/v1/logs" --token "$TOKEN" \
  --concurrency "$CONCURRENCY" --services "$SERVICES" --batch "$BATCH" --duration "$LOAD_SECS" >/dev/null 2>&1 || true

echo ">> idle tail ${IDLE_SECS}s (watching if RSS comes back down)…"
sleep "$IDLE_SECS"
kill "$SAMP" 2>/dev/null || true

# The server can die mid-run (e.g. OOM under an unbounded saturate load, or tmpfs ENOSPC). Read
# VmHWM defensively: if /proc/<pid>/status is gone, fall back to the sampled-series peak and flag
# the death instead of crashing on an unbound variable.
HWM_KB=$(awk '/VmHWM/{print $2}' "/proc/$SRV/status" 2>/dev/null || true)
if kill -0 "$SRV" 2>/dev/null && [ -n "$HWM_KB" ]; then
  PEAK_HWM="$(( HWM_KB / 1024 )) MiB"; ALIVE=yes
else
  PEAK_HWM="n/a (server died mid-run)"; ALIVE=no
fi
PEAK_SERIES=$(awk 'BEGIN{m=0}{if($2>m)m=$2}END{print (m>0)?m:"NA"}' "$SERIES" 2>/dev/null || echo NA)
POST=$(tail -1 "$SERIES" 2>/dev/null | awk '{print $2}'); POST=${POST:-NA}
STEADY=$(awk -v L="$LOAD_SECS" '$1<=L{print $2}' "$SERIES" | sort -n | awk '{a[NR]=$1} END{if(NR==0)print "NA"; else print a[int(NR/2)+1]}')

echo "──── mem profile [$LABEL] seg=$((SEGMENT_MAX_BYTES/1048576))MiB conc=$CONCURRENCY batch=$BATCH mode=${RATE:-saturate} ────"
[ "$ALIVE" = no ] && echo "  !! server DIED before final read (likely OOM / tmpfs-full under unbounded load) — 'peak sampled' is the last RSS seen before death, a floor"
echo "  peak VmHWM        ${PEAK_HWM}"
echo "  peak sampled      ${PEAK_SERIES} MiB"
echo "  steady (load)     ${STEADY} MiB"
echo "  post-idle (+${IDLE_SECS}s) ${POST} MiB   # << peak => transient; ~ peak => retained/allocator-held"
echo "  series            $SERIES"
