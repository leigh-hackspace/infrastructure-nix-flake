# thingino on the SigmaStar fleet: the port exists, and `cam5` + `cam8` now run it

**2026-10-31.** `cameras/README.md` and `docs/camera-thingino-evaluation-2026-10-25.md`
both say, with receipts, that thingino is not an option for these cameras because upstream
builds only Ingenic SoCs. **As of today that is no longer true**: a build tree porting
thingino to the SigmaStar SSC333 / Infinity6B0 family exists on the operator's bench
(`~/Projects/tapo/thingino-c110`, branch `ssc333-infinity6b0`, board
`tplink_tapo_c110_ssc333_sc3338_ssw101b`), and two fleet cameras were migrated onto it
today — **`cam5` (10.3.2.10)** and **`cam8` (10.3.2.112, back in production; its lease is
still dynamic, pinning filed)**. `cam7` and `cam9` remain OpenIPC. The 2026-10-25
evaluation is superseded at its headline and stands as history at its receipts; re-read the
four re-check commands there before repeating the old sentence to anyone.

Everything below was measured today on cam5 and cam8 hardware; the full bench narrative
with hashes lives in the operator tree (`~/Projects/tapo/docs/thingino-c110-execution.md`
§7d–§7e, `~/Projects/tapo/docs/openipc-migration.md`). This file is the network-facing
summary: what changed, what the endpoints are, what broke and how to not break it again.

## 1. What a migrated camera is now

Partition map is **`256k(boot), 64k(env), 1984k(kernel), 5376k(rootfs), 512k(data)`** —
the OpenIPC `2048k/5120k/704k` slots are gone, and the new thingino rootfs
(5,300,224 B) does not fit the old 5,242,880 B partition: **the geometry change is part of
the firmware**, so a rollback is not "flash the old rootfs back", it is the u-boot
procedure of §5.

| piece | value (measured on both units) |
| --- | --- |
| boot0 container | md5 `7a8643b8…`, sha256 `c10ea860…` — **byte-identical across the fleet** (cam5's and cam8's OpenIPC boot0 both hashed `ca9c15c3…` pre-migration), so the same image works unit to unit |
| u-boot | banner `I61286efbd`, hush parser; bootcmd is an SD **rescue**: `if fatload mmc 0 … RESCUE.KRN` → `sf erase`/`sf write` kernel+rootfs → boot → `S98sd-rescue-clear` eats the markers only after a successful boot, so an interrupted rescue **retries on next power-cycle** |
| kernel | uImage md5 `cb14c981…` (4.9.84, + MIIC/I2C driver fixes: the sensor bus never worked on this SoC without them — see the execution log §5q) |
| rootfs | squashfs md5 `ce3b7ef9…`; `NAME=Thingino`; dropbear **2026.94**; `rvd` + `rsd` (the raptor stack, not majestic) |
| env | `bootdelay 3` on the migrated units; OpenIPC's `wlanssid`/`wlanpass`/`sensor` vars **deleted** |

The thingino image **bakes Wi-Fi** — two `wpa_supplicant.conf` networks, `Leighhack` and
`The Lab 2.4GHz`, both `scan_ssid=1` — because the env-var credential channel this fleet
relied on does not exist in thingino. Serial console: `root` + the fleet camera password
(that convention: `cameras/README.md` "How to reach one"), and a first-login gate forces a
password change on **interactive** login until `shadow-` exists; scripted `ssh host cmd`
never sees the gate.

### Endpoints (this is what the frigate pipe must use now)

| | OpenIPC (cam7, cam9) | thingino (cam5, cam8) |
| --- | --- | --- |
| RTSP | `:554` `/stream=0`, **401-gated** | `:554` **`/streamN`** (`/stream0` main, `/stream1` sub; `/stream=0` answers **404 Unknown stream endpoint**), **no auth by default** |
| second media socket | — | `:8554` also open (webui-side); raw ffprobe against it resets — treat `:554` as the only ingestion port |
| web/admin | `:80`/`:85` majestic | `:80` webui + ONVIF (ws-discovery), `:443` |
| ssh | dropbear 2022.82 | dropbear 2026.94 |

**Consequences, both live right now:** (1) frigate's `pi_room` feed still points at
`rtsp://…cam5…:554/stream=0` and is being **404'd** until `/srv/frigate/config/config.yaml`
on services1 changes it to `/stream0` (fault 8 file, action item filed); (2) thingino's
stream answers **without credentials** — a `200` from `:554` was this directory's definition
of a finding, and on thingino it is the default, so either enable auth on the camera or
keep the VLAN rule that fences VLAN 226 in. `rsd` serves **2 clients max** and stale
probes hold slots (`/etc/init.d/S31raptor restart` clears them; never `kill -9` rvd — its
watchdog bites).

## 2. The migration procedure (and the rule it teaches the hard way)

Two flows are proven. Both **back up every mtd, sha-verified, before writing** (five
partitions over ssh, a few minutes).

* **Network two-phase** (`~/Projects/tapo/docs/openipc-migration.sh`, proven on cam8
  2026-10-31, then re-run twice): phase 1 from OpenIPC Linux — env rewrite (thingino
  geometry, **simple-parser bootcmd**, because the old u-boot has no hush and would execute
  an `if` unconditionally), then a RAM-resident writer (busybox here is *dynamically* linked:
  the writer's loader + libc + applets must be staged to tmpfs and exec'd through
  `loader --library-path`, or erasing the rootfs yanks the writer's own page cache — that
  killed the first attempt) flashes kernel+rootfs, **erases the data partition**, reboots.
  Phase 2 from the booted thingino takes over mtd0/mtd1 (u-boot + rescue env), read-back
  hash-verified **at source length** (the boot0 file is 244,668 B, *not* 244,832 — hashing a
  `head -c` at the wrong length looks like corrupt flash).
* **SD-first bench variant** (what cam5 got today): write u-boot + rescue env from the
  running Linux (`flashcp mtd0`, `flashcp mtd1` — safe from the page trap, nothing executes
  from those partitions), stage `RESCUE.KRN`/`RESCUE.RFS` on a FAT card (md5-verified
  through the copy chain), power-cycle, u-boot reflashes the payload and eats the markers.
  No serial needed — the rescue bootcmd runs automatically on power-up.

**The rule: always wipe the data partition (`0x780000 0x80000`) during a changeover.** The
network flow does it inside the writer; the SD-first variant does not, and skipping it on
cam5 is what made today take two hours. See §3.

From u-boot the wipe is `sf probe 0; sf erase 0x780000 0x80000; reset`. From a Linux
console: `umount -l /overlay; flash_eraseall /dev/mtd4; sync; reboot -f`. A unit with a
broken payload but a card (or a marker) present just retries; a unit whose boot region is
verified good at source length is recoverable from card or clip, and the boot0 is
fleet-identical.

Break-in, if you want the u-boot console: **cold** plug-cycle, then alternate `Ctrl-C`/CR —
prompt ~4.8 s in. Flooding from the first millisecond never works (the IPL swallows keys
into serial-update sniffing) and warm reboot loops never show it.

## 3. What "bricked the WiFi" actually was (the cam5 incident, in full)

Symptom after an SD-first flash that left the data partition: LED normal, camera
**associated but no IP**, invisible everywhere; `wpa_cli status` says
`COMPLETED`/`ssid=Leighhack` — the radio and credentials were never the problem.

Cause: thingino mounts the surviving OpenIPC jffs2 as the **upper layer of `/`**, and
`cameras/README.md` fault 10 already catalogued exactly what OpenIPC leaves there —
including **cam5's overlay copy of `interfaces.d/wlan0`** with
`wpa_passphrase "$(fw_printenv -n wlanssid)"` post-ups. The migration *deletes* `wlanssid`,
so `ifup wlan0` aborted before `udhcpc` ever started. Also in that upper: the
`sstar_mmc_upgrade`-era `S99sdconf.sh` hook (thingino's rcS runs it — it executed every
boot, harmlessly only because the OpenIPC-era card payload was inert), OpenIPC `passwd`/
`shadow`, `majestic.yaml`, `httpd.conf`, and an `authorized_keys` situation worth
re-checking on any unit this is done to.

**Diagnosis in four commands (no serial needed if anything reaches the unit):**
`wpa_cli status | grep wpa_state`, `ps | grep udhcpc`, `cat /etc/network/interfaces.d/wlan0`
— is that the OpenIPC stanza? — and the decisive one:
`ls /overlay/root/etc` (thingino tip 16: *list the upper before trusting that a unit runs
what it was flashed with*). Recovery is the data wipe of §2; the unit then boots pristine
(rootfs defaults incl. baked Wi-Fi), formats a fresh jffs2, and behaves like cam8 did
(Leighhack `COMPLETED` in seconds, same room, same image).

## 4. Rollback, if it is ever wanted

Rolling a thingino unit back to OpenIPC is **a u-boot procedure, not a Linux one**: from
Linux, erasing thingino's rootfs takes `rvd` down and the **hardware watchdog resets the
board mid-write** (measured — died at 92% of mtd3). From u-boot: `fatload` the nightly's
`uImage.ssc337` + `rootfs.squashfs.ssc337` (the original conversion's release dir is kept
on the rescue card), `sf erase 0x50000 0x200000` / write, `sf erase 0x250000 0x500000` /
write, replay the saved `fw_printenv` dump verbatim (`env set` ×N + `env save`), reset.
First boot lands in `openipc-claim`: the EULA acceptance must be typed by a human and the
root password reset — plan a console for it.

## 5. Per-unit state at end of day 2026-10-31

* **cam5** — thingino, `10.3.2.10` (the router held its old lease), sensor **sc3336**
  autodetected from silicon (`ipcinfo -s`) despite the OpenIPC `sensor=sc3338` hint;
  overlay wiped, password gate closed, `rvd` streaming 2304×1296@20. **RF at its position
  is marginal: −63 dBm, TCP cam→host 73 KB/s, the 2 Mbit/s main arrives at ~17% real time**
  — the known bad-corner physics (`docs/camera-wifi-tx-2026-10-29.md`), placement or
  bitrate action needed before it is a useful feed. Backups: all five pre-mtds + env dump
  + serial transcripts on the operator bench.
* **cam8** — thingino, **10.3.2.112, back in production**; joined Leighhack seconds after
  boot; its OpenIPC-era TX fault (`docs/camera-wifi-tx-2026-10-29.md`, <45 KB/s) needs a
  fresh measurement under thingino now that the MIIC fixes and a matched driver ship —
  the doc's driver-vs-hardware question may just have been answered. Lease **not yet
  pinned**; filed: a `cam8` row in `common/dns-records.nix` (`3c:52:a1:49:01:05`, vlan226,
  10.3.2.112) — router change, deploy through `machines/gw` normally.
* **cam7, cam9** — untouched OpenIPC. The bench tree's docs carry a stale habit of calling
  cam9 "the sealed unit at 10.3.2.17"; **this repo's table is the authority: cam9 is an
  ordinary fleet camera**, no special sealing or protection. Anyone touching a camera
  should confirm the target's **MAC** against the `cameras/README.md` fleet table before
  any write — today's
  near-miss (flashing cam5 while someone meant cam9's address space) was a labelling
  error, not a technical one, and only MACs close labelling errors.

## 6. What is still true from the old docs

The `uknor`/`urnor`-refuses-erase finding, the `sf update` + read-back discipline, the
"no size check exists but our own" rule, the recovery-dumps-first habit, rule 13 on
credentials, and cam1's open feed — all unaffected. `cameras/inventory.sh` and the
`cam-pull`/`cam-diff` verbs assume the OpenIPC `/etc` and **will mislead on thingino
units** (no `majestic.yaml`, no `/tmp/webui/signature.txt`) until they grow a thingino
mode — filed. The thingino-evaluation doc's one enduring asset turned out to be its
purchase note: the v3 C110 line is the Ingenic one; ours are, and remain, the SigmaStar
ones — they just aren't unbuildable for thingino any more.
