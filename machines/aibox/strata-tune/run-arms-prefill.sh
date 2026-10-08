#!/usr/bin/env bash
# The PREFILL arms of the UMA set (see ../strata-uma-2026-10-08.md): the short bench cannot
# see prefill (its prompt is 70 tokens), and prefill is aibox's real limit (~45 tok/s, so a
# 20k-token prompt costs ~7 min). These arms use the full bench - decode plus two cold ~6k
# prompts - so each one is ~5 min plus the model reload.
#
#   sudo ./run-arms-prefill.sh            # all arms
#   sudo ./run-arms-prefill.sh cpushare   # only the arms whose name contains "cpushare"
#
# What to read: the `long-prefill` / `long-prefill-2` prompt/s numbers, and the gtt_used
# line (--no-prefill-borrow reserves the prompt buffers instead of lending cache slots, and
# measured +4.8 GiB of GTT here for nothing on the short bench).
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
FULL="$HERE/strata-bench.py"
ONLY="${1:-}"
PROF='{"STRATA_DECODE_TIMING":"1","STRATA_VERIFY_PROFILE":"1"}'
CPUSHARE='{"STRATA_DECODE_TIMING":"1","STRATA_VERIFY_PROFILE":"1","STRATA_PREFILL_CPU_SHARE":"auto","STRATA_PREFILL_CPU_SHARE_MAX":"3072"}'

arm() {
    local name="$1" args="$2" env="${3:-}"
    [ -z "$env" ] && env="$PROF"
    if [ -n "$ONLY" ]; then
        case "$name" in *"$ONLY"*) : ;; *) return 0 ;; esac
    fi
    echo "== $name: $args $env"
    "$HERE/strata-exp.sh" 4096 8192 "$env" "$args" "$FULL" > "/tmp/pf-$name.log" 2>&1
    grep -h -E 'long-prefill|short-decode|gtt_used' "/tmp/pf-$name.log"
    echo
}

D='["--pcie-mode","direct"]'

arm control  '[]'
arm direct   "$D"
# The other zero-copy path: the CPU takes experts it reads from RAM as they are, no staging.
# On by default on CUDA builds since 0.1.41, off on HIP builds unless the variable is set.
# It changes bits (first-token KL mean 0.006, max 0.026), so it is an A/B, not a parity, arm.
arm cpushare "$D" "$CPUSHARE"
# The cache loan the prompt path takes (the thing that streams blobs through host staging).
arm noborrow '["--pcie-mode","direct","--no-prefill-borrow"]'

echo "logs: /tmp/pf-*.log"
