#!/usr/bin/env bash
# The UMA arms (see ../strata-uma-2026-10-08.md): does the engine stop copying when the
# "PCIe" path is a direct read through the arena's device alias?
#
#   sudo ./run-arms-uma.sh            # all arms
#   sudo ./run-arms-uma.sh direct     # only the arms whose name contains "direct"
#
# Control first, then the mode, then the share, then the pool, then prefill. Each arm
# reloads the model (~35 s); the short bench is ~25 s. Run as root.
#
# What to read on each arm (measured 2026-10-08, so these are the lines that actually move):
#   - `waitB`: THIS is the copy stage. verify.cpp stamps grouped(VRAM) -> 20, then
#     wait_flag_ge(m_flagB_) plus, only in mode 2, fetch_blobs + rebase_ptrs -> 21. So the
#     staging copy is inside waitB: 6.95 ms/window in `auto`, 0.14 in `direct`/`dma`.
#   - `PCIe grp`: NOT the copy - the grouped kernel computing the PCIe share, which runs in
#     every mode. It goes UP ~15% in `direct` (10.86 -> 12.49) because reading the arena
#     through the alias is slower per byte than reading a staged GTT copy.
#   - `waitCPU`: how much the CPU pool is the critical path (--pcie-frac 0: 14 -> 73 ms).
#   - decode tok/s, and gtt_used (--no-prefill-borrow costs +4.8 GiB here).
#
# The alias question itself (is a pinned host mapping readable by a kernel?) is answered
# without loading a model by ./alias-check.sh; it runs first below.
#
# Every arm runs with STRATA_DECODE_TIMING=1 STRATA_VERIFY_PROFILE=1 so the summary printed
# under each arm header is the same breakdown STRATA.md quotes; without it there is no
# `PCIe grp` line to read. Those lines go to the ENGINE's log (the run config's "log", which
# mkexp.py points at /tmp/exp-strata.log), not to the server log, so each arm copies it out
# to /tmp/uma-<name>-engine.log before reading it.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SHORT="$HERE/strata-bench-short.py"
ONLY="${1:-}"
PROF='{"STRATA_DECODE_TIMING":"1","STRATA_VERIFY_PROFILE":"1"}'
CPUSHARE='{"STRATA_DECODE_TIMING":"1","STRATA_VERIFY_PROFILE":"1","STRATA_PREFILL_CPU_SHARE":"auto","STRATA_PREFILL_CPU_SHARE_MAX":"3072"}'

# Cheap, and it gates everything below: if the alias were refused, device_alias() returns
# null, pcie_layer() is false, --pcie-frac is inert and `direct` cannot do anything.
"$HERE/alias-check.sh" 2>&1 | sed 's/^/   /'
echo

# arm <log-name> <extra-args-json> [extra-env-json]
arm() {
    local name="$1" args="$2" env="${3:-}"
    [ -z "$env" ] && env="$PROF"
    if [ -n "$ONLY" ]; then
        case "$name" in *"$ONLY"*) : ;; *) return 0 ;; esac
    fi
    echo "== $name: $args $env"
    "$HERE/strata-exp.sh" 4096 8192 "$env" "$args" "$SHORT" > "/tmp/uma-$name.log" 2>&1
    cp -f /tmp/exp-strata.log "/tmp/uma-$name-engine.log" 2>/dev/null
    grep -h -E 'short-decode|gtt_used|over PCIe' "/tmp/uma-$name.log" | tail -3
    grep -h -E 'strata decode timing|decode GPU stages' "/tmp/uma-$name-engine.log" | tail -4
    echo
}

D='["--pcie-mode","direct"]'

arm control  '[]'
arm direct   "$D"
# The share, swept. On UMA the probe's 0.55 is not a link measurement, so the optimum is a
# compute-balance question, not a bandwidth one.
arm direct-025 '["--pcie-mode","direct","--pcie-frac","0.25"]'
arm direct-100 '["--pcie-mode","direct","--pcie-frac","1.0"]'
arm direct-000 '["--pcie-mode","direct","--pcie-frac","0.0"]'
# The other delivery modes, for the record: `dma` stages with the copy engine (mode 0),
# `kernel` is what `auto` resolves to here (mode 2: a copy kernel inside the graph).
arm dma      '["--pcie-mode","dma"]'
# CPU pool: 6 physical cores, 12 SMT threads, AVX2 kernels. The pool is the biggest line in
# the profile, and on UMA it reads the same bytes the GPU would.
arm direct-w12 '["--pcie-mode","direct","--pool-workers","12"]'
# Prefill: no cache loan (the loan is what streams blobs through host staging), and the CPU
# share, which reads the arena as it is. Off by default on HIP builds.
arm noborrow   '["--pcie-mode","direct","--no-prefill-borrow"]'
arm cpushare   "$D" "$CPUSHARE"

# Needs PR #1548's build (open, unmerged as of 2026-10-08): the per-layer cost fit that
# minimises max(GPU, CPU) from measured costs instead of from the link probe.
# arm balance "$D" "$PROF"'{"STRATA_PCIE_BALANCE":"1"}'

echo "logs: /tmp/uma-*.log"
