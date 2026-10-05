# 3d-lime — Klipper MCU link stress harness

**Date:** 2026-10-03
**Host:** `3d-lime` (10.3.14.61) — Raspberry Pi 3 Model B print server, Klipper + Moonraker + Mainsail
**Purpose:** reproduce the CH340 USB-serial `Lost communication with MCU 'mcu'`
failures diagnosed on `3d-blue` (see
[`3d-blue-klipper-log-review-2026-10-03.md`](3d-blue-klipper-log-review-2026-10-03.md))
under sustained, controlled MCU traffic, and leave capture/logging running for
a follow-up review.

This harness is **out-of-band** — it is not flake-managed. Both Pi print
servers live in the hackspace LAN and are configured by hand.

## What was set up

A systemd service on `3d-lime`, `klipper-stress.service`, runs
`/home/hackspace/klipper-stress/klipper_stress.py` (Python 3 stdlib only).

It was **enabled** on 2026-10-03 ~17:36 BST, so it restarts on boot and runs
until explicitly stopped.

### Behaviour

* Streams a rapid back-and-forth X/Y oscillation (no extrusion, Z parked at
  20 mm) through Moonraker `POST /printer/gcode/script`. The oscillation centre
  slowly drifts across the bed so the whole travel envelope is exercised.
  Measured ~2.0–2.3 kB/s `bytes_write` — comparable to a real print on blue
  (~2.7 kB/s) and far above the idle ~5 B/s baseline.
* Homes once per Klipper session, then keeps moving. If Klipper shuts down or
  errors it stops, preserves the evidence, waits for the
  `klipper-link-watchdog` to restart Klipper, then re-homes and resumes. It
  never restarts Klipper itself.
* Samples MCU/link counters at 2 Hz into `logs/telemetry.csv`: klipper state,
  `bytes_write/read/retransmit/invalid`, `send_seq`/`receive_seq`,
  `retransmit_seq`, `srtt`/`rttvar`/`rto`, `ready_bytes`/`upcoming_bytes`,
  `mcu_awake`, `mcu_task_avg/stddev`, `freq`, toolhead position/`homed_axes`,
  `print_time`/`stalls`, `print_stats`, `virtual_sdcard`, system load/cputime/
  memavail, Pi CPU temp and `vcgencmd get_throttled`.
* Records state transitions and heartbeats in `logs/events.log`, filtered
  `dmesg` USB/mmc lines in `logs/usb-events.log`, and serial-node
  present/absent transitions.
* On any fault (klipper `shutdown`/`error`, or MCU counters vanishing while
  `ready`) writes a bundle to `crashes/<timestamp>/` containing `klippy.log`,
  `moonraker.log`, the last 400 telemetry rows, `dmesg`, `/printer/info`,
  `/server/info` and a snapshot. Keeps the newest 25 bundles.

### Safety interlocks

* Refuses to jog unless Klipper is `ready` and `print_stats.state` is not
  `printing`/`paused` and the virtual SD is not active — so starting a real job
  from Mainsail stops the stress motion.
* Only emits `G1` moves with X/Y/Z, never E. X/Y clamped to [40, 270] mm;
  Z raised to 20 mm before any X/Y motion.

## Controlling it

```bash
ssh -i ~/.ssh/agent-hop-key hackspace@10.3.14.61
systemctl status  klipper-stress
systemctl stop    klipper-stress          # stop now
systemctl disable klipper-stress          # do not come back after reboot
journalctl -u klipper-stress -f
~/klipper-stress/status.sh                # one-shot summary
```

## Logs / where to look for the crash

```
~/klipper-stress/logs/telemetry.csv       # 2 Hz link/MCU/host telemetry (rotates at 100 MB)
~/klipper-stress/logs/events.log          # state changes + heartbeats
~/klipper-stress/logs/usb-events.log      # filtered dmesg USB/mmc lines
~/klipper-stress/crashes/<ts>/            # crash bundles (newest 25)
~/klipper-stress/README.md                # on-device copy of this info
```

Suggested first look at a crash bundle:

```bash
d=$(ls -1d ~/klipper-stress/crashes/* | tail -1); echo "$d"
grep -an "Timeout with MCU\|Lost communication\|Unable to connect\|bytes_retransmit" "$d/klippy.log" | tail
python3 - <<PY
import csv
r=list(csv.DictReader(open("$d/telemetry_tail.csv")))
for row in r[-40:]:
    print(row['ts'], row['state'], 'w=',row['mcu_bytes_write'],
          'rtx=',row['mcu_bytes_retransmit'], 'rto=',row['mcu_rto'],
          'ready=',row['mcu_ready_bytes'], 'srtt=',row['mcu_srtt'])
PY
```

## Tunables

Set as systemd environment (or edit the unit) and restart. Defaults in
parentheses:

| Var | Meaning |
|---|---|
| `STRESS_SEG` (2.0) | mm per move |
| `STRESS_OSC_HALF` (2.0) | X oscillation half-amplitude |
| `STRESS_CX_DRIFT` (0.30) | X centre drift per reversal |
| `STRESS_CY_DRIFT` (0.05) | Y drift per move |
| `STRESS_FEED` (12000) | feedrate mm/min |
| `STRESS_X_MIN/X_MAX`, `STRESS_Y_MIN/Y_MAX` (40/270) | travel envelope |
| `STRESS_Z_HOP` (20) | Z clearance |
| `STRESS_BATCH` (180) | G-code lines per Moonraker request |

## Notes / status

* First run confirmed the harness works: homed, raised Z to 20, kept
  `print_stats=standby`, sustained ~2.0–2.3 kB/s.
* The existing `klipper-link-watchdog.service` (restarts Klipper after two
  consecutive `error`/`shutdown` checks, 10-minute cooldown) is left enabled;
  it interacts with the harness as intended (harness waits, watchdog recovers).
* `moonraker-exporter` on services1 continues to export
  `moonraker_mcu_bytes_retransmit_total` etc., so the same event is visible in
  Prometheus/Grafana while the harness runs.
