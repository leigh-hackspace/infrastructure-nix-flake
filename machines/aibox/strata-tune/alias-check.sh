#!/usr/bin/env bash
# Is a pinned host allocation's DEVICE ALIAS usable on this stack? (upstream
# tests/hip/mapped_alias.cpp, installed as `strata-alias-check` by strata-package.nix)
#
#   sudo ./alias-check.sh
#
# This is the gate for `--pcie-mode direct` (see ../strata-uma-2026-10-08.md): the PCIe share
# is delivered as `device_alias(layer, expert)`, i.e. the grouped kernel reading the pinned
# expert arena through its device address instead of through a staged copy. If ROCr refuses
# the alias, ArenaExpertSource::device_alias() returns null, pcie_layer() is false,
# --pcie-frac is inert and the direct mode cannot do anything at all.
#
# The unit sets HSA_USERPTR_FOR_PAGED_MEM=0 (the #750/#920 mitigation for the gfx1030 verify
# timeout), and that is exactly the knob that can stop ROCr mapping paged host memory - so
# run it BOTH ways and compare. The arena is registered with cudaHostRegister(Mapped), which
# is a different path from the hipHostMalloc the test itself uses, so this is a strong
# indicator rather than a proof; the engine's own "+N% of the routed experts over PCIe" line
# in the request log is the proof that the alias is live in a real run.
set -uo pipefail
UNIT=$(systemctl show strata -p FragmentPath --value)
S=$(grep -oE 'ExecStart=[^ ]+/bin/strata-server' "$UNIT" | head -1)
S=${S#ExecStart=}
S=${S%/bin/strata-server}
BIN="$S/bin/strata-alias-check"
[ -x "$BIN" ] || { echo "no $BIN - rebuild the strata package (it installs hip_mapped_alias)"; exit 1; }

export HSA_OVERRIDE_GFX_VERSION=10.3.0 HIP_VISIBLE_DEVICES=0
for E in 0 1; do
    echo "== HSA_USERPTR_FOR_PAGED_MEM=$E"
    HSA_USERPTR_FOR_PAGED_MEM=$E "$BIN" 2>&1 | sed 's/^/   /'
done

echo "== is the alias live in the running engine? (the proof)"
journalctl -u strata --no-pager -n 400 2>/dev/null |
    grep -oE '\+[0-9.]+% of the routed experts over PCIe' | tail -3 ||
    echo "   no PCIe-share line in the journal yet (no requests served)"
