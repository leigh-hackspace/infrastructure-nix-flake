# 3d-blue — Klipper / Moonraker log review

**Date:** 2026-10-03
**Host reviewed:** `3d-blue` (10.3.14.62) — Raspberry Pi 3 Model B print server, Klipper + Moonraker + Mainsail
**Reviewed by:** infra log investigation (services1, 10.3.1.20)
**Related config:** `machines/services1/services/printer-monitoring.nix` (moonraker-exporter)

## Scope & method

There is SSH access to the printer using the shared **machine-hop-key** as the
`hackspace` user (**permanent access method** — noted in `AGENTS.md`):

```bash
ssh -i ~/.ssh/agent-hop-key hackspace@10.3.14.62
```

Klipper config/logs live under `/home/hackspace/printer_data/`. Everything was
also read through Moonraker's HTTP API on `:7125` (this review is a snapshot of
what the on-device files showed at the time):

- `GET /printer/info`, `/server/info` — live Klipper state
- `GET /server/files/logs/klippy.log` — current Klipper log
- `GET /server/files/logs/klippy.log.<date>` — archived daily logs
- `GET /server/files/logs/moonraker.log[.<date>]` — Moonraker logs
- `GET /server/files/logs/KlipperScreen.log`
- `GET /server/history/list?limit=50` — print job history

No credentials were used; the API is a trusted-client endpoint for the LAN
(`trusted_clients` includes `10.3.0.0/16`). Nothing was changed on the printer.

Software versions seen: Klipper `v0.13.0-563-gf1fb5756`, Moonraker
`v0.10.0-9-g3fbe6ee`, Debian 13 (trixie), kernel `6.12.47+rpt-rpi-v8`,
KlipperScreen `v0.4.6`.

---

## TL;DR

1. **3d-blue is currently down — not printing and not ready.** Klipper is in
   `error`: it cannot talk to the MCU board.
2. The MCU is an **Arduino Mega 2560 clone over a CH340 USB-serial adapter**
   (`/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0`). That device node
   disappeared after an unclean reboot and the MCU is not coming back reliably.
3. The same **USB-serial link has an established history of failing mid-print**.
   On 2026-09-30 a 4 h+ print was killed by `Timeout with MCU 'mcu'` →
   `Lost communication with MCU 'mcu'` after serial retransmissions exploded.
   The Klipper host then sat in a broken retransmit loop for ~23 hours.
4. The Pi has **repeated unclean power events** — Moonraker's
   `Unsafe Shutdown Count` went 48 → 49 → 50 in three days.
5. Print history is poor: of the last 50 jobs, **20 completed, 12 `klippy_shutdown`,
   15 cancelled, 3 interrupted**.

The serial/MCU link and the power/reboot behaviour are the prime suspects for
the failed prints.

---

## UPDATE — same day (2026-10-03): recovery and hardening applied

Both printers were power-cycled at the wall later the same afternoon. On
power-up the Mega boards reset themselves and the CH340s re-enumerated
(3d-blue `usb 1-1.5`, 3d-lime `usb 1-1.2`), **but Klipper did not re-attach**:
it had already exhausted its 90 s `connect_uart` window and parked in
`error`. `sudo systemctl restart klipper` brought both to `ready`. This
confirms the outage was unpowered hardware (not a dead board) and exposes the
true recurring gap — **Klipper never re-attaches to an MCU that appears
after it has given up.**

### Applied — both Pi print servers (out-of-band; not flake-managed)

| Change | Detail |
|---|---|
| **Serial baud 250000 → 115200** | `[mcu] baud: 115200` in `printer.cfg`; MCU reflashed with `CONFIG_SERIAL_BAUD=115200` (`make clean && make && make flash FLASH_DEVICE=/dev/serial/by-id/...`). Longer bit time ⇒ more noise/EMI margin. Verified live: `mcu_constants.SERIAL_BAUD=115200`, `bytes_retransmit=0`, `bytes_invalid=0`. |
| **USB LPM + autosuspend off** | `/boot/firmware/cmdline.txt` += `usbcore.autosuspend=-1 dwc_otg.lpm_enable=0`; `/etc/modprobe.d/usb-autosuspend.conf` (`options usbcore autosuspend=-1`); udev rule `/etc/udev/rules.d/60-klipper-usb-autosuspend.rules` forces the CH340 `power/control=on`. |
| **Reset method pinned** | `[mcu] restart_method: arduino` (explicit; CH340 DTR reset). Deliberately **not** `rpi_usb`: the Pi's SMSC hub power switching is ganged, so a port power-cycle would also drop the on-board Ethernet. |
| **Link watchdog** | `klipper-link-watchdog.timer` runs every 60 s. If Moonraker reports klippy in `error`/`shutdown` on two consecutive checks it restarts `klipper` (10-minute cooldown). Log: `~/printer_data/logs/klipper-link-watchdog.log`. |
| **Probe / mesh quality** | `[bltouch] samples: 3` (was 1); `[bed_mesh] fade_end: 10.0` (was 5.0). |
| **Backups** | `~/printer_config_backup/2026-10-03-usb-hardening/` holds the pre-change `printer.cfg`, `klipper.config.250000`, the 250000 `klipper.elf.hex` (reflashable revert) and `cmdline.txt.bak`. |

Both Pis were rebooted to apply the cmdline changes and to exercise the new
recovery stack; both came back `ready` at 115200 with zero retransmits.

> A `PathExists=` **path unit** was tried first, to restart Klipper the instant
the CH340 appears. It raced `klipper.service`'s own `Restart=always` during
boot and tripped both start limits, leaving Klipper failed. It was removed;
the watchdog covers the same case within ~2 minutes.

### Applied — monitoring (this repo, deployed to services1)

- `moonraker-exporter` now also queries Moonraker's `mcu` object and exports
  `moonraker_mcu_connected`, `moonraker_mcu_info{baud=...}`, cumulative
  `moonraker_mcu_bytes_{write,read,retransmit,invalid}_total` and
  `moonraker_mcu_{send,receive}_seq_total`, plus `moonraker_mcu_srtt_seconds`
  and `moonraker_mcu_rto_seconds`.
- New Prometheus rules (light up the Alerts tab; no alertmanager yet):
  `PrinterMcuDisconnected`, `PrinterMcuRetransmits` (rate > 1 B/s over 10 m —
  the 09-30 bad day was ~0.14 % of the wire rate), `PrinterMcuInvalidBytes`,
  and `KlipperShutdown` (critical).
- Grafana **3D Printers** dashboard gained *MCU link errors* and
  *MCU link latency* panels.

### Still outstanding

- **Power reliability** remains the top risk (Unsafe Shutdown Count 50 on
  blue / 34 on lime). This is not a software problem — it needs a UPS or a
  graceful-shutdown trigger on the hackspace printers.
- The baud change trades throughput for margin. 250000 is *exact* on the AVR;
  115200 carries a ~ +2.1 % AVR divisor error (UBRR=16, U2X on) but is the
  conventional Klipper AVR rate. If a fast print ever shows `print_stall` /
  buffer starvation, revisit.

### Follow-up review — evening of 2026-10-03, while printing (read-only)

A later read-only pass (Moonraker HTTP API plus the machine-hop-key SSH) was
made while 3d-blue was **mid-print**, to check the hardened link under load.
Nothing was changed, restarted or sent to the printer.

Job `00003C` `kevin/2 Hands V1 Blue_0.2mm_PLA_X1_8h32m.gcode` (PrusaSlicer,
~8 h 32 m), ~20 min in (~1.4 %) at the time of review.

| Check | Value |
|---|---|
| Klipper / Moonraker | `ready` / `ready`, MCU `connected` |
| MCU | `atmega2560`, `SERIAL_BAUD=115200`, `restart_method: arduino` |
| Temps | extruder 220.0/220, bed 62/62 |
| Link health | `bytes_invalid=0`, `rto=0.025` (floor), `ready_bytes` 7–55, `buffer_time` ~1.0–1.9 s, `print_stall=0` |
| Under-voltage | `throttled=0x0`, CPU 52.6 °C |
| Unsafe Shutdown Count | 50 (unchanged since the 16:33 boot) |

**Retransmits are elevated but not (yet) failing.** The session accumulated
`bytes_retransmit` in ~180-byte bursts:

```
Stats 1039.9: 17
Stats 1152.9: 37
Stats 1719.4: 223
Stats 2012.6: 348
Stats 2346.8: 532
Stats 2368.8: 712     # +180 in ~22 s
Stats 2472.9: 890     # +178 in ~104 s
Stats 2474.9: 1079    # +189 in 2 s
```

That is **1079 B / 1.27 MB written ≈ 0.085 %** — about 12× the 0.005–0.007 %
seen on the healthy 2026-09-26/27 logs, but well below the 0.14 % of the
2026-09-30 fatal day. The `PrinterMcuRetransmits` rule
(`rate(moonraker_mcu_bytes_retransmit_total[10m]) > 1`) was **pending for
`blue` at ~1.17 B/s** at review time (`lime` = 0). Crucially the link keeps
recovering: `rto` never left the 0.025 s floor and the backlog stayed tiny, so
this is an early warning, not the 09-30 failure shape. There is no safe
mid-print intervention — watch it.

**The cold-boot race is real and needed manual recovery.** At the 16:32 boot
the CH340 enumerated late and `klipper.service` **hit `start-limit-hit`**:

```
16:32:54  Started klipper.service
... (five quick restart cycles) ...
16:32:55  klipper.service: Start request repeated too quickly.
16:32:55  klipper.service: Failed with result 'start-limit-hit'.
```

It only came back at **16:34:53**, after the `hackspace` user manually ran
`systemctl stop/disable` and deleted leftover **`klipper-usb-present.path` /
`.service`** units, then `systemctl reset-failed` + `systemctl start klipper`.
(The UPDATE above says those units were removed; on blue they were evidently
still present and were cleaned up during this session.) The watchdog timer
(`OnBootSec=120`, then every 60 s) is healthy and, correctly, only restarts
Klipper on `error`/`shutdown` — it cannot interrupt a healthy print.

**3d-lime is back.** It was unreachable at the original review; it now pings
(0.6 ms) and its Moonraker reports `ready` with `mcu_connected=1` and zero
retransmits.

**Minor:** `dmesg` logs `mmc0: Problem switching card into high-speed mode!`
(old C10/A1-class SD card) and root is 67 % full (8.8 G / 14 G). No ext4/mmc
I/O errors this boot beyond the normal post-unclean-shutdown orphan cleanup.

---

## 1. Current live state (2026-10-03)

```
GET /printer/info
{"result":{"state":"error","state_message":
 "mcu 'mcu': Unable to connect
  Once the underlying issue is corrected, use the
  \"FIRMWARE_RESTART\" command to reset the firmware, reload the
  config, and restart the host software.
  Error configuring printer\n", ...}}
```

`GET /server/info` → `klippy_connected: true`, `klippy_state: "error"`,
`warnings: []`.

End of the current `klippy.log`:

```
mcu 'mcu': Unable to open serial port: [Errno 2] could not open port
  /dev/serial/by-id/usb-1a86_USB_Serial-if00-port0:
  No such file or directory: '/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0'
   ... (18 occurrences) ...
MCU error during connect
Traceback (most recent call last):
  File "/home/hackspace/klipper/klippy/mcu.py", line 841, in _attach
    self._serial.connect_uart(self._serialport, self._baud, rts)
  File "/home/hackspace/klipper/klippy/serialhdl.py", line 191, in connect_uart
    self._error("Unable to connect")
serialhdl.error: mcu 'mcu': Unable to connect
mcu.error: mcu 'mcu': Unable to connect
Error configuring printer
```

In the current Klipper session: 1 `Starting serial connect`, **18 × "Unable to
open serial port"**, then the node reappeared, then **4 × "Unable to connect"**
and `Error configuring printer`.

So the failure has two faces:

- the `/dev/serial/by-id/...` node is **missing** (USB device not enumerated), and
- once it does enumerate, the **MCU handshake fails** (`Unable to connect`).

KlipperScreen mirrors this: it transitions `disconnected` → `startup` →
`error`.

### Confirmed over SSH (2026-10-03 14:47)

```
$ ssh -i ~/.ssh/agent-hop-key hackspace@10.3.14.62
$ ls -l /dev/serial/by-id/
ls: cannot access '/dev/serial/by-id/': No such file or directory
$ lsusb
Bus 001 Device 001: ID 1d6b:0002 Linux Foundation 2.0 root hub
Bus 001 Device 002: ID 0424:9514 Microchip ... SMC9514 Hub
Bus 001 Device 003: ID 0424:ec00 Microchip ... SMSC9512/9514 Fast Ethernet Adapter
$ vcgencmd get_throttled
throttled=0x0                 # no undervoltage currently
$ uptime
... up 8 min
```

The board is **entirely absent from the USB bus** — not merely a missing
`by-id` symlink. `lsusb` shows only the root hub, the SMSC hub and the onboard
Ethernet adapter; there is no `1a86` (CH340) device and no `ttyUSB*`. `dmesg`
over the whole boot contains no CH340/USB-serial enumeration event at all, i.e.
the Mega never came up on USB after this boot. `throttled=0x0` says there is no
undervoltage right now, but sampling only happens on demand.

---

## 2. Timeline of the current outage

| Time (printer local) | Event |
|---|---|
| 2026-10-03 12:45:55 | Klipper log rollover |
| 2026-10-03 12:46:50 | Moonraker restarts; `Unsafe Shutdown Count` 49 → **50** |
| 2026-10-03 12:46:51 | Klipper `Start printer` — last healthy MCU connect was the session *before* this |
| 2026-10-03 12:46:51+ | `Unable to open serial port` repeatedly (MCU node absent) |
| 2026-10-03 14:39:30 | KlipperScreen reconnects, sees `klippy_state: startup` |
| 2026-10-03 14:40:43 | KlipperScreen state flips `startup` → `error` |
| now | Klipper `error`: `mcu 'mcu': Unable to connect` |

Before the reboot the MCU was healthy — the previous session logged
`Loaded MCU 'mcu' 135 commands` and normal `Stats` lines (bed ~22–23 °C,
extruder ~22 °C) up to `Stats 94.5`.

### Unsafe shutdown counter

| When | Count |
|---|---|
| `moonraker.log.2026-10-01` (log start) | 48 |
| 2026-10-01 17:50:45 | 49 |
| 2026-10-03 12:46:55 | 50 |

Three unclean shutdowns in ~48 h. This is a power/reboot reliability problem,
independent of but correlated with the MCU dropouts.

---

## 3. Evidence — serial/MCU link drops out mid-print (the failed-print cause)

The single archived shutdown that lines up with the job history is
**2026-09-30, `95mm disk_0.2mm_PLA_Blue_X1_4h49m.gcode`** (history job
`00003B`, `status: klippy_shutdown`, 253 min of a scheduled ~4 h 49 m print).

Just before the failure (`klippy.log.2026-09-30`):

```
Stats 17423.5: ... bytes_retransmit=4485 ... send_seq=662162 receive_seq=662162 ... rto=0.025
Stats 17424.5: ... bytes_retransmit=5209 ... send_seq=662194 receive_seq=662191
                  retransmit_seq=662194 ... rto=0.400 ready_bytes=1406
Stats 17425.5: ... bytes_retransmit=5390 ... rto=0.800 ready_bytes=4122
Stats 17426.5: ... bytes_retransmit=5571 ... rto=1.600 ready_bytes=7646
Stats 17427.5: ... bytes_retransmit=5752 ... rto=3.200 ready_bytes=11109
Timeout with MCU 'mcu' (eventtime=17428.499324)
Transition to shutdown state: Lost communication with MCU 'mcu'
...
MCU 'mcu' shutdown:
```

Read: the host stopped getting ACKs (`send_seq` 662194 vs `receive_seq` 662191),
the retransmit timer doubled (`rto` 0.025 → 0.4 → 0.8 → 1.6 → 3.2 s), the
backlog (`ready_bytes`) grew, and Klipper gave up → shutdown → print dead.

### The link then stayed broken for ~23 hours

`klippy.log.2026-10-01` continues the *same* session past midnight. The
`print_time` is frozen at `17398.775` and retransmits climb without bound:

```
Stats 18814.5: ... send_seq=662194 receive_seq=662191
                  bytes_retransmit=55889   rto=5.000 ready_bytes=19260
Stats 24819.1: ... bytes_retransmit=273089  rto=5.000 ready_bytes=25359
Stats 37329.4: ... bytes_retransmit=725770  rto=5.000 ready_bytes=38063
Stats 48338.2: ... bytes_retransmit=1123970 rto=5.000 ready_bytes=49243
Stats 82644.8: ... bytes_retransmit=2365268 rto=5.000 ready_bytes=84084
webhooks client ...: Disconnected
Restarting printer
Start printer at Thu Oct 1 17:43:54 2026
```

That is ~1 hour of log at ~4000 retransmit-bytes/s, from the 09-30 shutdown
until 2026-10-01 17:43:54 — the Klipper process never exited, it just
retransmitted to a dead MCU (~1.4 CPU-seconds of `cputime` burned). After the
17:43 restart it *still* failed once:

```
Loaded MCU 'mcu' 135 commands ...
MCU error during connect
Can not update MCU 'mcu' config as it is shutdown
Error configuring printer
```

Then at 17:45:41 a restart finally configured the MCU
(`Configured MCU 'mcu' (592 moves)`) and at 17:50:41 it restarted again and
reached `Klippy ready`.

### Retransmit rate is anomalously high on the bad day

| Daily log | bytes_write | max bytes_retransmit | rate |
|---|---|---|---|
| 2026-09-26 | 68.4 MB | 4,914 | 0.0072 % |
| 2026-09-27 (1-day print completed) | 214 MB | 10,707 | 0.0050 % |
| 2026-09-30 (print died) | 39.7 MB | 55,889 | **0.14 %** |

The 09-30 link retransmitted ~20–28× more per byte than the previous days. A
healthy Klipper serial link should retransmit essentially zero (a handful per
long print). This is the strongest single signal pointing at the CH340/USB-serial
path as the source of failed prints.

No `Timer too close`, `Rescheduled timer in the past`, `Move out of range`,
`ADC out of range` or `Heater ... not heating at expected rate` were found in any
of the archived logs — the failures are **communication** failures, not
kinematics/temperature faults.

---

## 4. Print history — 50 most recent jobs

| Status | Count |
|---|---|
| completed | 20 |
| `klippy_shutdown` | 12 |
| cancelled | 15 |
| interrupted | 3 |

The 12 `klippy_shutdown` jobs (Klipper aborted them):

| Job | Started | Duration | File |
|---|---|---|---|
| 00003B | 2026-09-30 | 253 m | `95mm disk_0.2mm_PLA_Blue_X1_4h49m.gcode` |
| 000039 | 2026-08-27 | 57 m | `Pelvis V3 blue split_0.2mm_PLA_X1_6h18m.gcode` |
| 000038 | 2026-08-27 | 37 m | `pelvis and ball V3 blue_0.2mm_PLA_X1_10h42m.gcode` |
| 000025 | 2026-07-06 | 55 m | `Alistair HY310x Ceiling Mount_0.2mm_PLA_X1_3h52m.gcode` |
| 000024 | 2026-07-06 | 27 m | `Alistair HY310x Ceiling Mount_0.2mm_PLA_X1_3h52m.gcode` |
| 000023 | 2026-07-04 | 0 m | `3DBenchy_0.2mm_PLA_X1_1h38m.gcode` |
| 000017 | 2026-04-11 | 50 m | `3DBenchy_0.2mm_PLA_X1_1h7m.gcode` |
| 000010 | 2026-03-21 | 2 m | `3DBenchy_0.4mm_0.8n_PLA_X1_50m.gcode` |
| 00000E | 2026-03-21 | 2 m | `3DBenchy_0.4mm_0.8n_PLA_X1_50m.gcode` |
| 00000D | 2026-03-21 | 0 m | `3DBenchy_0.4mm_0.8n_PLA_X1_50m.gcode` |
| 00000B | 2026-03-21 | 0 m | `3DBenchy_0.4mm_0.8n_PLA_X1_50m.gcode` |
| 00000A | 2026-03-21 | 0 m | `3DBenchy_0.4mm_0.8n_PLA_X1_50m.gcode` |

Notes:

- The 2026-03-21 cluster (five `klippy_shutdown` jobs within ~5 h) looks like a
  bad day for the same link — several died at 0–2 minutes.
- 15 `cancelled` + 3 `interrupted` are operator- or Mainsail-initiated aborts.
  These can't be attributed from the logs (Moonraker stores no reason), but the
  repeated re-runs of `Pelvis V3 Sidewinder)_0.2mm_PLA_X1_8h14m.gcode`
  (cancelled 15:13, 17:22, 12:41, 13:44; interrupted twice) suggests a print
  that repeatedly failed to get going.
- Only the 2026-09-30 shutdown has a surviving archived log; the older ones
  predate the retained daily logs (`klippy.log.2026-09-26` … `2026-10-01`).

---

## 5. Hardware / config observations

From the config dump in `klippy.log`:

- MCU: `atmega2560`, `SERIAL_BAUD=250000`, `RECEIVE_WINDOW=192`, connected via
  **CH340** (`usb-1a86_USB_Serial-if00-port0`, a chip with a well-known
  reputation for flaky 250 kbaud USB-serial and unreliable DTR resets).
- `[mcu]` has **no `restart_method`**. On AVR the default is the DTR/`arduino`
  reset; with a CH340 that reset can fail — consistent with the observed
  `MCU error during connect` after a restart.
- Single MCU only — no CAN/second board involved.
- `[bltouch] samples = 1`, `probe_with_touch_mode = true`,
  `stow_on_each_sample = false` — single-sample probing (no averaging) makes
  first-layer Z less repeatable than it could be (a quality risk, not an
  observed failure).
- `[bed_mesh] algorithm = bicubic`, `fade_end = 5.0` — mesh compensation is
  fully faded out by 5 mm; typical is 10 mm. Not fatal but worth revisiting.
- `[printer] max_velocity = 200`, `max_accel = 3000`, `square_corner_velocity = 5`.
- Extruder: `max_extrude_only_distance = 500`, `pressure_advance = 0.04`,
  PID tuned. Heater bed PID, `smooth_time = 3.0`.
- SD card: `MSSD0`, 14.6 GiB (C10/A1-class card).
- CPU temp at the 09-30 shutdown: 51.5 °C; `Throttled Flags:` empty (no
  undervoltage recorded **at that sample** — but this is only snapshotted on
  shutdown, so it does not rule out brown-outs while printing).

---

## 6. What is most likely responsible for failed prints

Ranked by evidence:

1. **Unreliable USB-serial link to the Mega (CH340) → `Lost communication with
   MCU 'mcu'`.** Proven on 2026-09-30, with a retransmit rate ~20–28× normal.
   This directly aborts prints partway through and is the best explanation for
   the `klippy_shutdown` history entries. Contributing factors could be the
   CH340 adapter itself, a poor/loose USB cable, EMI from steppers, or a
   brown-out of the Mega while the bed/extruder switch.
2. **Repeated unclean power loss / reboots (Unsafe Shutdown Count 50 and
   climbing).** Each power event risks the MCU not re-enumerating (exactly the
   current state) and can leave the firmware in a shutdown state that needs a
   hard reset (`FIRMWARE_RESTART` would not even complete on 2026-10-01).
3. **MCU/serial enumeration not surviving reboot** — the current outage: the
   `/dev/serial/by-id/...` node is absent, and when it appears the handshake
   fails.
4. **Secondary/quality:** single-sample BLTouch probing and a short bed-mesh
   fade may cause first-layer inconsistencies that lead to operator
   cancellations, but there is no log evidence tying them to the
   `klippy_shutdown` jobs.

---

## 7. Recommended actions

**On the printer (over the machine-hop-key SSH, plus physical for the board):**

1. Power-cycle the Mega 2560 and reseat its USB cable; then check, on the Pi:
   ```bash
   ssh -i ~/.ssh/agent-hop-key hackspace@10.3.14.62
   ls /dev/serial/by-id/
   lsusb
   dmesg -w | grep -iE 'usb|ch34|tty'   # watch for disconnect/reconnect during a print
   sudo systemctl restart klipper       # or FIRMWARE_RESTART from Mainsail
   ```
2. If the `by-id` path changes, update `[mcu] serial` in
   `~/printer_data/config/printer.cfg` (a serial-numberless CH340 can
   re-enumerate differently). Prefer a stable path.
3. **Replace the USB cable** with a short, shielded one (ferrite if possible)
   and route it away from stepper/power wiring. If the CH340 is built into the
   Mega clone, consider an **FTDI/CP2102** based board or a **powered USB hub**
   so the Mega's 5 V doesn't sag with the heaters.
4. Add an explicit reset method under `[mcu]` (e.g. `restart_method: arduino`)
   and verify `FIRMWARE_RESTART` actually resets the Mega, since a failed reset
   is what produced the 2026-10-01 `MCU error during connect` loop.
5. **Fix the power reliability** (UPS / clean shutdown). This is a shared
   hackspace printer, so at minimum set up a graceful-shutdown trigger and stop
   the repeated `Unsafe Shutdown` events.
6. Consider `[bltouch] samples = 2` or `3` and reviewing `bed_mesh fade_end`
   once the printer is stable again.

**Monitoring (this repo):**

- `moonraker-exporter` (`machines/services1/services/printer-monitoring.nix`)
  already exports Moonraker metrics. Make sure there is an alert when Klipper
  is not `ready` / is in `error`/`shutdown`, and when the exporter can't reach
  Moonraker — the current outage would then be noticed from services1.

**Separate issue spotted in passing:**

- **3d-lime (10.3.14.61) is unreachable** from services1: ping fails and
  `http://10.3.14.61:7125/server/info` gets no connection. Its Klipper/Moonraker
  isn't answering either and needs its own look. *(Resolved later the same day —
  see the follow-up review above: lime is back, `ready`, zero retransmits.)*

---

## 8. Making a comms loss survivable (tolerance / workarounds)

Follow-on analysis (2026-10-03, while blue was printing) of what can be done to
stop retransmit bursts from turning into a dead 8 h print. Current behaviour is
benign — the bursts are clustered (e.g. `bytes_retransmit` flat at 1253 for
minutes at a time) and each one drains on the next ACK — but the goal is to make
the *unrecoverable* case rare.

### 8.1 Where the tolerance actually lives — it is ~5 s and hardcoded

Klipper's retransmit layer already tolerates loss indefinitely; what aborts a
print is the **clock-sync liveness check**:

- `klippy/clocksync.py` sends `get_clock` every `0.9839 s` and resets
  `queries_pending` on any reply;
- `is_active()` is literally `return self.queries_pending <= 4`;
- `klippy/mcu.py::check_timeout()` invokes
  `shutdown("Lost communication with MCU")` when `queries_pending > 4`.

So **~5 consecutive unanswered clock queries ≈ 4–5 s of total MCU silence** is
the cutoff. It lines up with `serialqueue.c`'s `MAX_RTO 5.000` and with the
2026-09-30 log (`rto` 0.025 → 0.4 → 0.8 → 1.6 → 3.2 → `Timeout`). There is **no
`printer.cfg` knob** for this.

### 8.2 Reduce the number of dropouts (root cause) — highest value

Physical, to be done when **not** printing. The existing hardening (115200,
LPM/autosuspend off, `restart_method`) already lowered severity.

| Change | Why / notes |
|---|---|
| **USB isolator (ADuM3160/ADuM4160)** between Pi and Mega | Breaks the ground loop that makes CH340 links drop on stepper/heater transients. Low cost, no firmware; often the biggest single win. |
| **Short shielded USB cable + ferrite**, routed away from stepper/heater wiring | The ~0.05 % per-byte loss is classic EMI/crosstalk. |
| **Powered USB hub / stable 5 V for the Mega** | CH340+Mega draw from the Pi's 5 V rail; bed switching can sag it. |
| **Replace the CH340 path**: FTDI/CP2102-based Mega, or a native-USB MCU (RP2040/STM32) / CAN toolboard | Definitive fix; native USB stacks are far more robust than CH340. |
| Lower `max_accel` / `max_velocity` in `printer.cfg` | Less host→MCU traffic ⇒ fewer events and smaller in-flight bursts. Easy, reversible, changes print time. |
| Pi CPU governor `ondemand` → `performance` | Removes frequency-transition latency as a possible RTO trigger. Low risk, marginal. |
| Drive the link over the **Pi hardware UART (GPIO14/15)** instead of USB | Bypasses USB scheduling and the CH340 entirely; needs wiring, bypassing the shared UART0 CH340, and disabling the serial console. Medium effort, high payoff. |

### 8.3 Increase tolerance (source changes — with caveats)

- **Patch `clocksync.py` `queries_pending <= 4`** (e.g. to 10–20) to ride out a
  10–20 s silence. Mechanically works, but the AVR move buffer only holds
  ~1–2 s (`buffer_time`), so the toolhead starves anyway and you resume with a
  seam/defect; the clock sync must also re-lock across the gap. Mostly converts
  a clean abort into a lumpy resume — **not recommended on its own**.
- Raising `serialqueue.c MAX_RTO` only changes retransmit frequency, not the
  ~5 s liveness cutoff.
- Both are source patches that will conflict on Klipper updates.

### 8.4 Avoid losing the whole print (recovery)

Klipper has **no built-in resume** (Mainsail/Fluidd have no power-loss
recovery), and shutdown invalidates the stepper position, so a clean automatic
resume is genuinely hard.

- **Cheapest:** keep the Mainsail `file_position` / layer for the job so a
  person can slice from a layer and restart. Low tech, no risk.
- `on_error_gcode` is currently `CANCEL_PRINT`. It could be replaced by a macro
  that records `file_position` + `gcode_move` position to `save_variables`,
  enabling a manual/scripted resume. Resuming a bed-slinger after a shutdown
  needs re-homing + `SET_KINEMATIC_POSITION` — treat as experimental.
- **Pre-emptive pause:** extend `klipper-link-watchdog` to watch the
  `bytes_retransmit` rate and issue a Moonraker `PAUSE` *while the link is
  still alive*, converting a hard shutdown into a resumable pause. Caveat: on
  09-30 the degradation to death took only ~5 s; today's clustered bursts with
  quiet gaps would give more warning. Repo-side and testable.
- **UPS / graceful shutdown** removes the separate `Unsafe Shutdown Count 50`
  failure class entirely.

### 8.5 Monitoring (this repo)

`PrinterMcuRetransmits` is `rate(...[10m]) > 1` for `10m` — deliberately
conservative, but it means the alert may fire only as the link is already
trending dead. A shorter-window / lower-threshold "link distress" rule routed
to a human would give time to decide whether to pause. Pure flake change, no
printer risk.

### 8.6 Recommended order

1. Physical first: USB isolator + shielded/ferrite cable + independent 5 V for
   the Mega (cheap, no firmware, largest reduction in dropout frequency).
2. If it persists: replace the CH340 board with a native-USB controller, or
   move to the Pi hardware UART.
3. Treat resume as a fallback, not the strategy — the ~5 s tolerance plus a
   robust physical link is what actually prevents failed prints.
4. Avoid the source-patch "more tolerance" route unless deliberately
   experimenting; and do nothing on the printer while a print is running.

---

## Appendix — reproduction commands

All read-only, run from services1:

```bash
# live state
curl -s http://10.3.14.62:7125/printer/info
curl -s http://10.3.14.62:7125/server/info

# current and archived logs
curl -s http://10.3.14.62:7125/server/files/logs/klippy.log -o /tmp/klippy.log
curl -s http://10.3.14.62:7125/server/files/logs/klippy.log.2026-09-30 -o /tmp/klippy-0930.log
curl -s http://10.3.14.62:7125/server/files/logs/moonraker.log -o /tmp/moonraker.log

# print history
curl -s 'http://10.3.14.62:7125/server/history/list?limit=50' -o /tmp/hist.json

# the fatal sequence
grep -an "Timeout with MCU\|Lost communication with MCU\|Unable to open serial\|Unable to connect" /tmp/klippy-0930.log
```

Raw log files were only read into `/tmp` on services1; nothing on the printer
was modified.
