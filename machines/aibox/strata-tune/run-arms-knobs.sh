#!/usr/bin/env bash
# The cheap A/B knobs the deployed engine already ships (see ../STRATA.md, "Where a decode
# token actually goes"): the cache admission policy, the MTP draft trade, the CPU-pool task
# batching, and the overlap arms upstream documents.
#
#   sudo ./run-arms-knobs.sh            # all arms
#   sudo ./run-arms-knobs.sh spec       # only the arms whose name contains "spec"
#
# Why these: the decode window is a CHAIN, not a pipeline - the profile's GPU time and host
# time ADD (frac 0: 107+135, frac 0.55: 119+69, frac 1.0: 177+11), so moving experts between
# the two cannot win. What can win is doing the same work in fewer ms: a better-resident
# cache (--expert-cache-per-layer), more accepted tokens per window (--spec/--spec-min-p), a
# better-fed CPU pool (--pool-tasks, --no-host-worker) and the overlap pokes
# (--no-hit-poke, --shared-late).
#
# Every arm runs with the profile on and inherits the deployed engine args (including
# --pcie-mode direct): mkexp.py appends these args AFTER the installed ones, so a later
# --spec overrides the deployed one.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SHORT="$HERE/strata-bench-short.py"
ONLY="${1:-}"
PROF='{"STRATA_DECODE_TIMING":"1","STRATA_VERIFY_PROFILE":"1"}'

arm() {
    local name="$1" args="$2" env="${3:-}"
    [ -z "$env" ] && env="$PROF"
    if [ -n "$ONLY" ]; then
        case "$name" in *"$ONLY"*) : ;; *) return 0 ;; esac
    fi
    echo "== $name: $args $env"
    "$HERE/strata-exp.sh" 4096 8192 "$env" "$args" "$SHORT" > "/tmp/kn-$name.log" 2>&1
    # the profile lines go to the ENGINE's log (mkexp.py points it at /tmp/exp-strata.log,
    # which the engine appends to), so copy it out per arm before reading it
    cp -f /tmp/exp-strata.log "/tmp/kn-$name-engine.log" 2>/dev/null
    grep -h -E 'short-decode|gtt_used|over PCIe' "/tmp/kn-$name.log" | tail -3
    grep -h -E 'strata decode timing' "/tmp/kn-$name-engine.log" | tail -1 | cut -c1-190
    grep -h -E 'decode GPU stages' "/tmp/kn-$name-engine.log" | tail -1 |
        grep -oE '(waitA|VRAM hits|waitB|PCIe grp|waitCPU|head) [0-9.]+' | tr '\n' ' '
    echo; echo
}

# Baseline: the deployed config (4096 slots, --pcie-mode direct, --spec 4 --spec-min-p 0.5).
arm control '[]'

# R4.2g: each layer gets its OWN slots instead of one shared arrival-order counter.
# Upstream's numbers are for a cache with no profile (256 slots: 2.97 % hits global vs 21.4 %
# at 8/layer); we fill from --expert-profile, which is frequency-ranked GLOBALLY, so this
# could go either way at 4292 slots / 48 layers = 89 per layer. Measure, don't assume.
arm perlayer '["--expert-cache-per-layer"]'

# The MTP draft costs ~25 ms/window and accepts 2.14 of 2.54 offered. Accepted tokens per
# window is a direct multiplier on decode, so this is the cheapest decode lever left.
arm spec6      '["--spec","6"]'
arm specminp35 '["--spec-min-p","0.35"]'
arm specminp65 '["--spec-min-p","0.65"]'

# The CPU pool: --pool-workers 0 (the default) = every physical core except the one the host
# loop spins on (6 on the 6600H), with 3 batched tasks per thread per GU/Down phase.
arm pooltasks6   '["--pool-tasks","6"]'
arm pooltasks12  '["--pool-tasks","12"]'
arm nohostworker '["--no-host-worker"]'

# The overlap arms: the hit path pokes the driver right after its launch "so the GPU starts
# while the CPU pool runs; without it the work waits for the next driver entry and does not
# overlap at all" - so --no-hit-poke should be clearly WORSE if that overlap is real here.
arm nohitpoke  '["--no-hit-poke"]'
arm sharedlate '["--shared-late"]'

echo "logs: /tmp/kn-*.log"
