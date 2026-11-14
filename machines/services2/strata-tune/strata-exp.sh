#!/usr/bin/env bash
# A/B harness for services2's Strata engine (CUDA, sm_86): run the engine with
# overridden args, benchmark it, then hand 10.3.1.21:8080 back to strata.service.
# services2's copy of machines/aibox/strata-tune/strata-exp.sh.
#
#   sudo ./strata-exp.sh <expert-cache> <prefill> [extra-env-json] [extra-args-json] [bench-script]
#   sudo ./strata-exp.sh auto auto '{}' '["--spec","6"]'
#   sudo ./strata-exp.sh 2000 auto '{"STRATA_EMB_REUSE_ACCOUNT":"1"}'
#
# Everything is derived from the installed unit, so it follows a rebuild.  It stops
# strata.service, runs the experiment on 127.0.0.1:8123 (8080 is the real endpoint,
# 8088 is the node exporters), benchmarks it, kills the engine and restarts the unit.
#
# The experiment server has "api_key": "none" (mkexp.py) and binds loopback, so the
# deployed key is never copied anywhere and nothing secret is printed.  Cost per arm:
# strata.service is DOWN for the whole run - about 60 s to load 22.85 GiB of experts
# plus the bench.  Read the arms table in machines/services2/STRATA-TUNE-2026-11-14.md first:
#
# Read the noise floor before believing an arm (measured 2026-11-14, 23 arms): control
# repeats give 31.9 / 32.0 / 32.6 / 31.2 decode tok/s and 1,048-1,056 tok/s on the 8k
# prompt, and one control pass came in at 807 tok/s - so **under about 10 % is noise
# unless it replicates**, and an effect worth shipping has to be seen twice.  Two traps
# this script used to have, both fixed here: /tmp/exp-strata.log was appended across
# arms (so an arm's tail could quote its predecessor's numbers), and a bench path that
# did not resolve made the arm exit 0 having benchmarked nothing.
set -uo pipefail

EC="${1:-auto}"
PF="${2:-auto}"
EXTRA_ENV="${3:-}"
[ -z "$EXTRA_ENV" ] && EXTRA_ENV='{}'
EXTRA_ARGS="${4:-}"
HERE="$(cd "$(dirname "$0")" && pwd)"
BENCH="${5:-$HERE/strata-bench-fast.py}"
shift $(( $# >= 5 ? 5 : $# ))
EXTRA_BENCHES=("$@")        # $6, $7 ... run after the primary bench (e.g. strata-answer-check.py)
PORT=8123
# A bench path is absolute/relative to the caller or bare, and then it lives beside this
# script.  Resolve it HERE: a bench that silently does not exist is an arm that reports
# success and measures nothing (it happened to arms 18 and 21 on 2026-11-14).
resolve() {
    [ -n "$1" ] || return 1
    [ -x "$1" ] && { echo "$1"; return 0; }
    [ -x "$HERE/$1" ] && { echo "$HERE/$1"; return 0; }
    return 1
}
BENCH=$(resolve "$BENCH") || { echo "no such bench script: ${5:-}"; exit 1; }
for i in "${!EXTRA_BENCHES[@]}"; do
    [ -n "${EXTRA_BENCHES[$i]}" ] || continue
    EXTRA_BENCHES[$i]=$(resolve "${EXTRA_BENCHES[$i]}") || {
        echo "no such extra bench script: ${EXTRA_BENCHES[$i]}"; exit 1; }
done
UNIT=$(systemctl show strata -p FragmentPath --value)   # a symlink into the store

[ "$(id -u)" = 0 ] || { echo "run as root (it stops/starts strata.service)"; exit 1; }
CFG_STORE=$(grep -o '/nix/store/[a-z0-9]*-strata-coder-iq1m.json' "$UNIT" | head -1)
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
# The engine's own log (the config's "log" key, written by mkexp.py) is per-arm: left
# appended, the excerpt at the end of an arm quotes whichever earlier arm last printed a
# `PCIe share:` or a hit-rate line, which is how one arm nearly got another's numbers
# into the write-up.
rm -f /tmp/exp-strata.log
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
grep -E "experts loaded|filling the GPU|expert cache|VRAM free|prompt chunk auto|PCIe share|ready:" \
    /tmp/exp-server.log | tail -8

"$S/bin/strata-prep" "$BENCH" "http://127.0.0.1:$PORT"
for b in "${EXTRA_BENCHES[@]:-}"; do
    [ -n "$b" ] || continue
    [ -x "$b" ] || b="$HERE/$b"
    echo "--- extra bench: $b"
    "$S/bin/strata-prep" "$b" "http://127.0.0.1:$PORT"
done

nvidia-smi --query-gpu=memory.used,memory.free,power.draw,utilization.gpu --format=csv,noheader
free -h | head -2
grep -E "decode expert cache hit rate|prompt [0-9]+ tokens =|PCIe share: the CPU pool|expert cache [0-9]+ slots" \
    /tmp/exp-strata.log /tmp/exp-server.log 2>/dev/null | tail -8

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
