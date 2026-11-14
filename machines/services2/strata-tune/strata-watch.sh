#!/usr/bin/env bash
# 1 Hz samples of what the GPU and CPU are actually doing during a bench, so
# "100% utilization" can be told apart from "the card is saturated".
#
#   ./strata-watch.sh 90 &   # then run a bench; ^C or kill it afterwards
#
# Why this exists: on 2026-11-14 services2 read 100% `utilization.gpu` across a whole
# bench pass while drawing 157-164 W through the prefill arms and only 60-110 W through
# the decode ones.  nvidia-smi's utilization means "at least one kernel resident", not
# "SMs busy", so power is the second opinion - a decode that is stalled waiting for
# experts looks identical to a compute-bound one in the util column and completely
# different in the watts.  Pair it with the engine's own lines: the PCIe bytes/token it
# reports (`more read by the GPU over PCIe`) against the `PCIe probe: N GB/s` it probed.
set -u

DUR="${1:-60}"
echo "t util% power_W sm_MHz cpu0_MHz free_VRAM_MiB"
i=0
while [ "$i" -lt "$DUR" ]; do
    # One nvidia-smi call per sample, not three: each call is a GPU round trip and
    # this script runs next to the thing it is measuring.
    read -r u p c v < <(nvidia-smi --query-gpu=utilization.gpu,power.draw,clocks.sm,memory.free \
        --format=csv,noheader,nounits | awk '{print $1, $2, $3, $4}')
    f=$(awk '{printf "%.0f", $1/1000}' /sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq 2>/dev/null || echo -)
    echo "$i $u $p $c $f $v"
    i=$((i + 1))
    sleep 1
done
