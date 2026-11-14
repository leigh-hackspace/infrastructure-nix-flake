#!/bin/sh
# Read-only power / temperature / clock probe for aibox's Ryzen 5 6600H, 1 Hz, one CSV line.
#
#   sudo ./power-probe.sh /tmp/ap.csv          # start (Ctrl-C stops; it writes as it goes)
#   python3 - <<'PY'  ...aggregate by phase...  # or eyeball:  column 5 is package watts
#
# Why this exists (2026-11-14, answering "can we overclock aibox?").  `machines/aibox/STRATA.md`
# had established that decode is chain-bound and that BOTH halves are compute-bound, which is
# precisely the case where clock work is worth considering - so the question became which of
# three ceilings actually binds: the GPU's own clock table, the DRAM, or the package's power
# window.  Those three are answered by three different columns of this file, under the same
# load the bench produces, and the answer decides whether any of them is worth a unit.
#
# What each column is, because two of them are easy to read wrong:
#   tctl_c        k10temp Tctl, the CPU's own hotspot.  The part's limit is 95 C.
#   igpu_c        the amdgpu die sensor.  On this box it runs a few degrees HOTTER than Tctl,
#                 so "how much thermal margin do we have" must be asked of THIS one.
#   package_w     amdgpu hwmon power1_input / 1e6.  This is the APU's package power and it is
#                 the only working power meter on the box: ryzenadj 0.19 cannot read the SMU
#                 table here (no ryzen_smu module, /dev/mem refused), so this sensor is what
#                 any power-limit experiment has to be judged by.
#   sclk_active_mhz  the DPM state marked `*` in pp_dpm_sclk.  There is no freq1_input on this
#                 APU (only freq1_label), so the mask is the only way to ask; it reports the
#                 state, not a measured clock, which is exactly what "is it at the top state"
#                 needs and not what "what is it really running at" would.
#   cpuN_mhz      scaling_cur_freq for three cores, chosen to span CCD/SMT topology.  A single
#                 core is not enough: the pool's workers and the host thread land on different
#                 ones, and a boost-clamped box shows up here as a low MAX, not a low mean.
#
# Nothing is written by this script except its own CSV.  Writes to `pp_od_clk_voltage`,
# `power_dpm_force_performance_level` or the SMU are deliberately not part of it.

PATH=/run/current-system/sw/bin:/run/current-system/sw/sbin; export PATH
OUT=${1:-/tmp/aibox-power.csv}
: > "$OUT"
echo "epoch,utc,tctl_c,igpu_c,package_w,sclk_active_mhz,gpu_busy,cpu0_mhz,cpu3_mhz,cpu6_mhz,epp,pstate" > "$OUT"
K10=$(for h in /sys/class/hwmon/hwmon*/; do [ "$(cat $h/name 2>/dev/null)" = k10temp ] && echo $h; done)
AMD=$(for h in /sys/class/hwmon/hwmon*/; do [ "$(cat $h/name 2>/dev/null)" = amdgpu ] && echo $h; done)
DEV=/sys/class/drm/card0/device
[ -n "$K10" ] && [ -n "$AMD" ] || { echo "no k10temp/amdgpu hwmon here - this is not aibox" >&2; exit 1; }
while :; do
    printf '%s,%s,' "$(date +%s)" "$(date -u +%H:%M:%S)" >> "$OUT"
    awk '{printf "%.1f,", $1/1000}' $K10/temp1_input >> "$OUT" 2>/dev/null || printf '?,' >> "$OUT"
    awk '{printf "%.1f,", $1/1000}' $AMD/temp1_input >> "$OUT" 2>/dev/null || printf '?,' >> "$OUT"
    awk '{printf "%.1f,", $1/1000000}' $AMD/power1_input >> "$OUT" 2>/dev/null || printf '?,' >> "$OUT"
    printf '%s,' "$(grep -o '[0-9]*Mhz \*' $DEV/pp_dpm_sclk 2>/dev/null | tr -dc '0-9')" >> "$OUT"
    printf '%s,' "$(cat $DEV/gpu_busy_percent 2>/dev/null)" >> "$OUT"
    for c in 0 3 6; do
        awk -v c=$c '{printf "%.0f,", $1/1000}' /sys/devices/system/cpu/cpu$c/cpufreq/scaling_cur_freq >> "$OUT" 2>/dev/null || printf '?,' >> "$OUT"
    done
    printf '%s,%s\n' "$(cat /sys/devices/system/cpu/cpufreq/policy0/energy_performance_preference 2>/dev/null)" \
                     "$(cat /sys/devices/system/cpu/amd_pstate/status 2>/dev/null)" >> "$OUT"
    sleep 1
done
