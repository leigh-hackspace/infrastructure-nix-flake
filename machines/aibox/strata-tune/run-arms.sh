#!/usr/bin/env bash
# The A/B arms measured on aibox (results in ../STRATA.md).  Each arm reloads the
# model (~35 s) and runs the bench (~6 min for the full bench, ~25 s for the
# short one).  Run as root: sudo ./run-arms.sh
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"

"$HERE/strata-exp.sh" 2048 8192 > /tmp/arm-ec2048.log 2>&1
"$HERE/strata-exp.sh" 8192 8192 > /tmp/arm-ec8192.log 2>&1
"$HERE/strata-exp.sh" 0 8192 > /tmp/arm-ec0.log 2>&1

# Where the 260 ms decode window goes (the numbers quoted in STRATA.md):
"$HERE/strata-exp.sh" 2048 8192 '{"STRATA_DECODE_TIMING":"1","STRATA_VERIFY_PROFILE":"1"}' '[]' "$HERE/strata-bench-short.py" > /tmp/arm-profile.log 2>&1

# NOT YET MEASURED: the CPU expert pool is ~95 ms of the 260 ms window, and the
# 6600H has 12 SMT threads while the pool defaults to 5 (physical cores minus the
# host thread).  These arms are the cheapest remaining win.
"$HERE/strata-exp.sh" 4096 8192 '{}' '["--pool-workers","12"]' "$HERE/strata-bench-short.py" > /tmp/arm-w12.log 2>&1
"$HERE/strata-exp.sh" 4096 8192 '{}' '["--pool-workers","10"]' "$HERE/strata-bench-short.py" > /tmp/arm-w10.log 2>&1
"$HERE/strata-exp.sh" 4096 8192 '{}' '["--pool-workers","3"]' "$HERE/strata-bench-short.py" > /tmp/arm-w3.log 2>&1

echo "logs: /tmp/arm-*.log"
