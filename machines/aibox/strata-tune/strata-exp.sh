#!/usr/bin/env bash
# A/B harness for aibox's Strata engine: run the engine with overridden args,
# benchmark it, then hand port 8080 back to strata.service.
#
#   sudo ./strata-exp.sh <expert-cache> <prefill> [extra-env-json] [extra-args-json] [bench-script]
#   sudo ./strata-exp.sh 4096 8192 '{}' '["--pool-workers","12"]' ./strata-bench-short.py
#
# Everything is derived from the installed unit, so it follows a rebuild.
# It stops strata.service, runs the experiment on 127.0.0.1:8123 (8090 is
# frigate-monitor on this box), benchmarks, kills the engine and restarts the
# unit.  Watch RAM: the IQ2_XS arena alone is 33 GiB.
set -uo pipefail

EC="${1:-4096}"
PF="${2:-8192}"
EXTRA_ENV="${3:-}"
[ -z "$EXTRA_ENV" ] && EXTRA_ENV='{}'
EXTRA_ARGS="${4:-}"
HERE="$(cd "$(dirname "$0")" && pwd)"
BENCH="${5:-$HERE/strata-bench.py}"
PORT=8123
UNIT=$(systemctl show strata -p FragmentPath --value)   # a symlink into the store

[ "$(id -u)" = 0 ] || { echo "run as root (it stops/starts strata.service)"; exit 1; }
CFG_STORE=$(grep -o '/nix/store/[a-z0-9]*-strata-iq2xs.json' "$UNIT" | head -1)
S=$(grep -oE 'ExecStart=[^ ]+/bin/strata-server' "$UNIT" | head -1)
S=${S#ExecStart=}
S=${S%/bin/strata-server}
[ -n "$CFG_STORE" ] && [ -x "$S/bin/strata-server" ] || {
    echo "no strata unit found - is services.strata.enable set and switched on?"; exit 1;
}

echo "=== arm: expert-cache=$EC prefill=$PF env=$EXTRA_ENV args=$EXTRA_ARGS ==="
rm -f /tmp/exp.json
STRATA_CONFIG="$CFG_STORE" "$S/bin/strata-prep" "$HERE/mkexp.py" "$EC" "$PF" "$EXTRA_ENV" "$EXTRA_ARGS"
[ -f /tmp/exp.json ] || { echo "mkexp.py failed"; exit 1; }

systemctl stop strata
rm -f /tmp/exp-server.log
nohup "$S/bin/strata-server" --engine strata --config /tmp/exp.json \
    --host 127.0.0.1 --port "$PORT" > /tmp/exp-server.log 2>&1 &
SPID=$!

READY=0
for _ in $(seq 1 100); do
    kill -0 "$SPID" 2>/dev/null || break
    if grep -q "ready:" /tmp/exp-server.log 2>/dev/null; then READY=1; break; fi
    sleep 3
done
if [ "$READY" != 1 ]; then
    echo "ARM FAILED TO START; server log:"; tail -12 /tmp/exp-server.log
    kill "$SPID" 2>/dev/null
    systemctl start strata
    exit 1
fi
grep -E "experts loaded|filling the GPU|no AVX-512|ready:" /tmp/exp-server.log | tail -6

"$S/bin/strata-prep" "$BENCH" "http://127.0.0.1:$PORT"

echo "gtt_used=$(cat /sys/class/drm/card0/device/mem_info_gtt_used)"
free -h | head -2
grep -E "expert cache .* hit|done: |decode timing|decode GPU stages" /tmp/exp-server.log | tail -8

kill "$SPID" 2>/dev/null
for _ in $(seq 1 20); do
    pgrep -u root -f "bin/strata --serve" > /dev/null || break
    sleep 2
done
if pgrep -u root -f "bin/strata --serve" > /dev/null; then
    echo "engine still running, killing it"
    pkill -u root -f "bin/strata --serve"
    sleep 3
fi
systemctl start strata
echo "=== arm done ==="
