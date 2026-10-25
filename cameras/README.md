# The cameras — OpenIPC on the WiFi VLAN

`cam5` `cam7` `cam9` (10.3.2.10 / 10.3.2.12 / 10.3.2.17) are **Buildroot/OpenIPC
surveillance cameras**, and `cam1` (10.3.2.6) is a device on the same VLAN that
is **not** OpenIPC.  None of them runs NixOS, none has systemd, none is a module
in the flake: their entire state is files under `/etc` sitting on a tmpfs root
(`overlay` on `/`, 704 KB, **~452 KB free**) that busybox init re-reads on boot
and that a **firmware reflash overwrites**.  They are the reason a fault can be
invisible where it matters: services1's frigate container pulls their RTSP
through its own go2rtc, and the table that maps a feed name to a camera lives
**inside that container** (`/dev/shm/go2rtc.yaml`, generated at start from
`/srv/frigate/config/config.yaml` — a runtime bind mount the flake only mounts,
`machines/services1/services/frigate.nix`).  Nothing in this repo could see
either end of that pipe until this directory existed; now the table, the reach,
the report and the record of what each device runs all live here.

Read the sibling doc this directory owns before touching a camera:
**[`./c110.md`](./c110.md)**, "TP-Link Tapo C110 v2.0 — Complete Hacking Guide"
(610 lines / 50 KB: the SPI-NOR dump, the flash layout, the boot chain, what
stock firmware contains, the app-less WiFi options, §8 the reflash route —
U-Boot plus an SD-card or TFTP `sf erase`/`sf write` of an OpenIPC image — and
§9 restoring stock).  It came with the fleet rather than from this pass, and it
is the device-side story `just cam-push`/`just cam-init` assume; the one
operation this repo will not do (reflash) is written there because someone will
be asked for it someday.  **These are its hardware or its close neighbour, on
the devices' own words**: cam7 reports `ssc333 (infinity6b0 family), sc3338,
8 MB nor flash` with the board string `INFINITY6B0 SSC009A-S01A QFN88`, which is
the doc's C110 v2 line for line, while cam5 and cam9 report `ssc337` on the same
board string — the doc's "other C110 versions" branch.  No model label was read
by this pass (see "not proven": the reflash image is chosen per SoC, so the
difference matters).  **Its `mmcblk` is not a running device here**: `mmcblk0`
is the micro-SD slot (§1) and SD is the *flashing medium* (§8.3), while the
running boxes have no `/dev/mmc*` node at all and `/proc/partitions` lists
`mtdblock0…mtdblock4` (`flash_mmc_note`, `sdconf_mount=/mnt/mmcblk0p1`,
`sdconf_mount_present=absent`), so `S99sdconf.sh`'s hook can never fire and that
doc's device names are the vendor's, not ours.

## The table

Identity was read **off the devices** on 2026-10-25 (their own
`/tmp/webui/signature.txt`, `/etc/os-release`, `/proc/cpuinfo`) and cross-checked
against `common/dns-records.nix`; the MAC each device prints in its signature
line is **the same MAC the DNS row pins**, so those records are pinned by
identity, not by guesswork (verified below).  VLAN 226's `dhcp-mode` is `gateway`
(`common/vlans.nix`), which is why the devices' own `/etc/resolv.conf` names
`10.3.2.1` as their `nameserver` — VLAN 226's gateway is the router itself.

| device | address (reserved by MAC) | VLAN | MAC (device == DNS row) | firmware / majestic | board (self-reported) | signature line | frigate camera | feed (go2rtc) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `cam5` | 10.3.2.10 | 226 | `3c:52:a1:48:fa:b2` ✓ | 2.4.08.22 / build 2024.02.1 / `master+9371591, 2024-08-21` | `INFINITY6B0 SSC009A-S01A QFN88` | `ssc337 (infinity6b0 family), sc3338, 8 MB nor flash, 2.4.08.22-lite, cam5, …` | `pi_room` | `rtsp://cam5.int.leighhack.org:554/stream=0` |
| `cam7` | 10.3.2.12 | 226 | `3c:52:a1:49:24:0a` ✓ | **2.3.11.22** / build 2023.02.7 (no sysinfo at all) | `INFINITY6B0 SSC009A-S01A QFN88` | `ssc333 (infinity6b0 family), sc3338, …, 2.3.11.22-lite, cam7, …` | `cnc` | `rtsp://cam7.int.leighhack.org:554/stream=0` |
| `cam9` | 10.3.2.17 | 226 | `3c:52:a1:49:1b:f7` ✓ | 2.4.08.22 / build 2024.02.1 / `master+9371591, 2024-08-21` | (sysinfo present, **no board line**) | `ssc337 (infinity6b0 family), sc3338, …, openipc-ssc337, …` | `social_space` | `rtsp://cam9.int.leighhack.org:554/stream=0` |
| `cam1` | 10.3.2.6 | 226 | `10:a4:be:03:0d:36` | **not OpenIPC** — family not established | unknown | unknown | `rack_corridor` | `rtsp://cam1.int.leighhack.org:554` with `ffmpeg:` transcode (its go2rtc line asks `:554/0/av0`, the Novastorm path — commented out, see faults) |

All four rows above are also rows in `common/dns-records.nix`; the router
reserves the address by MAC (`dhcp-host=3c:52:a1:49:24:0a,set:vlan226,10.3.2.12,cam7`).
There is **no second copy of the addresses** in this directory: `just cam-list`
and `just cam-addr` read that table, so there is nothing to drift.

The other three feeds in frigate's table — `main_space`, `woodwork`, `workshop`
— belong to **other devices**: `common/dns-records.nix` reserves 10.3.14.12 /
.13 / .11 on VLAN 227 (the workshop's own block) under those names.  They are
not OpenIPC devices we have looked at, and they are out of scope here; the fact
recorded is that **the feed names and the device names are different tables**
and both are needed to answer "is this camera's picture alive?".

## How to reach one

```
just cam-probe cam5          # no credentials, no login: alive? gated? DNS?
just cam-check   cam5        # one report, one ssh pass: versions, uptime, feed, md5s
just cam-pull    cam5        # its /etc files into cameras/config/cam5/
just cam-diff    cam5        # repo copy vs device, per file
just cam-push    cam5 cameras/config/cam5/majestic.yaml
just cam-init    cam5 S95majestic restart
just cam-key     cam5        # the bootstrap: hop key -> /root/.ssh (asks once)
just cam-keytest cam5        # is key-only ssh working
```

Every recipe is key-only (`BatchMode=yes`, so an unkeyed device fails in
seconds rather than hanging a call — rule 11).  **None of the three was keyed
as of 2026-10-25**: `authorized_keys_note=none-only-login-is-the-vendor-password`
on all three, so `just cam-key <cam>` is the first thing to run, and it is the
one recipe expected to prompt for the vendor password, typed by the operator on
their own tty — the justfile never holds, prints, or learns that password
(rule 13; the password is not named anywhere in this repo, and `cameras/inventory.sh`
reports its length, 12 bytes, not its value).  These boxes run **Dropbear 2022.82
buildroot** (their `dropbear -v` banner), not OpenSSH: same `authorized_keys`, and
`/root/.ssh` is a **symlink to `/etc/dropbear`** on these devices (`sd_dir_link`
/`ssh_dir_link` fields in the report), which is why the hop key goes to
`/root/.ssh/authorized_keys` while `/etc/dropbear` itself holds only
`dropbear_ed25519_host_key`.  Keying them is the same shape as the printer Pis
(`docs/printer-servers.md`).

The pipe is three hops and each one can be probed alone:

```
camera /etc/majestic.yaml (video0, rtsp port 554, `stream=0`)
      -> go2rtc restream inside the frigate container (:8554, name = the stream key)
      -> frigate (camera name = the go2rtc stream key, retry_interval: 120)
      -> gatus (`common/monitoring-endpoints.nix` watches cam5.int / cam9.int)
```

## What is proven on these devices (2026-10-25), and what is still a guess

**Proven, read off the devices:**

* **All three answer their own interfaces and gate both of them.**  RTSP on
  `:554` answers **`RTSP 401 Authentication required`, realm `WallyWorld`**
  (curl speaks RTSP: `curl -m 6 -o /dev/null -w '%{http_code}'
  rtsp://<addr>:554/stream=0` → `401`; with credentials, `--config` +
  `--data-urlencode`, the `DESCRIBE` answers `200`), and HTTP on `:80`
  (`:85` too) answers **`401` / `Authentication Required`, realm `Authentication`**
  with `www-authenticate: Basic realm="Authentication"`.  **A 200 from
  `:554` is a finding, not a win**: an ungated OpenIPC feed is one any LAN
  client can pull.  There is **no separate web-server process** on these boxes
  (`httpd_process_note=none-no-separate-web-server`): the `:80`/`:85` gate is
  majestic's own, reading `/etc/httpd.conf`, and busybox `httpd` is not what
  answers (an earlier probe of mine inferred `httpd` from a self-matched `ps`
  line — the self-match, not the device, was the error, and that is why the
  report greps `ps w` and reports the note rather than a count).
* **Feed geometry differs per device and the difference is not cosmetic.**  From
  the pulled `/etc/majestic.yaml`s: cam5's `video0` has `size: 1280x720`,
  `fps: 5`, `rcMode: vbr`, `gopSize: 1.0`, `bitrate: 4096`, `profile: main`;
  cam7's has `size: 1280x720`, `fps: 5`, `rcMode: cbr`, `gopSize: 5` and
  **no `bitrate` and no `profile`**; **cam9's `video0:` carries only
  `enabled: true`** — its size/fps/bitrate are vendor defaults.  Anyone who
  changes one of these changes it alone.
* **Only cam5 pins its sensor pad**: `isp:` at line 74 with
  `sensorConfig: /etc/sensors/sc3338.bin` at 76.  cam7 and cam9 have **no
  `isp:` section at all** (yet `/etc/sensors/` on them holds more than one pad).
  All three signature lines say **`sc3338`**, so the default choice is currently
  right on all three — the exposure is that a firmware bump that changes the
  loader's default pad list changes cam7's and cam9's image **with no config
  change to review**.
* `nightMode.enabled`: **true on cam5 and cam7, false on cam9**.  `nightAPI: true`
  on cam5 and cam7 (lines 20 / 17), absent on cam9.  `/etc/init.d/S96autonight`
  exists **only on cam9** (md5 `9860da91…`) — so the device with night mode
  **disabled** is the one carrying the autonight package, and cam5/cam7 use
  majestic's builtin hook.  Three devices, three day/night mechanisms, one
  observable effect (IR cut filter + sensor-led behaviour).
* `watchdog:` is present in all three `majestic.yaml`s (lines 71 / 57 / 55) and
  the report reads `watchdog_on=1`, `watchdog_timeout=10` on all three:
  majestic restarts itself when it stops answering, so **a hung stream server is
  not the same as a down one** and the log ring is what tells them apart.
* **The sensor does not survive every restart**: the log says `Sensor panic!`,
  `Camera is about to reboot...`, then `Connect scs_base init driver`
  (`log_reinits_total` counts those).  A camera whose sensor dies **reboots
  itself**, so an unexplained boot on one of these is a sensor fault, not a
  power cut — and `/var/log` is a symlink to `../tmp`, so **a reboot wipes the
  log ring that recorded why it rebooted**.  A reboot's cause is recoverable
  only while the box is up.  This is the reason `just cam-check` exists and the
  reason it is worth running often.
* `/etc/resolv.conf` is **45 bytes, `nameserver 10.3.2.1`, md5
  `3a2c6424…`, identical on all three** — VLAN 226's gateway is 10.3.2.1, i.e.
  the router's own address (`common/vlans.nix`).  `/etc/httpd.conf` is **74
  bytes, md5 `6f9c8c6e…`, identical on all three** (`H:/var/www`, `A:127.0.0.1`,
  `A:*`, `D:8.8.8.8/32`).  `/etc/init.d/S95majestic` is **md5
  `25664714…` on cam5 and cam9 (both 2.4.08.22) and `a868ec56…` on cam7 (2.3.11.22)**
  — i.e. the vendor's own init script tracks the firmware version.
* `/etc/init.d/S99sdconf.sh` is a **9-line hook** that runs
  `/bin/sh /mnt/mmcblk0p1/sdconf.sh` if that file exists.  On these devices
  `/proc/partitions` holds `mtdblock0…mtdblock4` and **there is no `/dev/mmc*`
  node at all** (`flash_mmc_note`, `sdconf_mount=/mnt/mmcblk0p1`,
  `sdconf_mount_present=absent`), so the hook can never fire: the `mmcblk`/SD
  story in `c110.md` is the vendor's, not ours.  cam7 does not even carry the
  hook (`sd_hook=0`).
* **The pulled `/etc` files match the devices byte for byte, except two known
  cases** (md5s from the report's `etc_md5s` line against
  `cameras/config/<cam>/`): `majestic.yaml` cam5 `5e936364…` ✓ cam7 `36b961be…`
  ✓ cam9 `9774f13b…` ✓; `httpd.conf` and `resolv.conf` ✓ on all three; `crontabs/root`
  cam5 `a1356469…` ✓ cam9 `a1356469…` ✓ (the same file on both), cam7 ✗ — the
  repo copy is **the redacted one** (we replaced the vendor's Telegram example, so
  the device's `1fdb757f…` can never match it: expect `cam-diff cam7` to report
  that one file as drift for ever, and read the note inside the file instead of
  the md5);
  `S95majestic` ✓ all three; `S96autonight` (cam9 only) ✓;
  **and `S99sdconf.sh` on cam5 ✗: the device says `ae29e67d…`, our copy says
  `1b4ac291…` (cam9's copy).**  That is the one file the pulled set does not
  prove — `just cam-diff cam5` is the command that closes it, and it is an open
  item (docs/follow-ups.md), not something to hand-edit from memory.
* **crond has nothing to run**: `/etc/crontabs/root` is **0 active lines** on
  all three (cam5/cam9 23 comment lines; cam7 30 lines, 0 active, incl. the
  redacted vendor example).  Nothing on a camera runs by itself — no cron, no
  timer, no watchdog besides majestic's own.  And cam5/cam9 also carry
  `/etc/crontabs/root.r` (1311 bytes, md5 `999679e0…`, **byte-identical on both**)
  — a crond-compatible name busybox crond does not read, i.e. dead config.
* **No camera appears in any podman/registry/nginx/service-unit path.**  The
  only services1-side thing that knows them is that container's config, plus
  the two gatus rows in `common/monitoring-endpoints.nix`.

**Not proven, and do not turn into a fact:**

* **Why cam5 rebooted mid-session on 2026-10-24.**  It booted at `00:00:01`
  (no NTP yet, ~23:15 UTC on the wall) and has been up since (`uptime_s` ~21.9 k
  s at report time).  Everything it logged from boot to `23:51` is there, but
  nothing on the box can say what caused the boot, because that log ring lived
  in `/tmp` and the boot replaced it.
* **`/tmp/` and `/overlay/` are the same mount** — `df` reports 34–36 % and the
  same free figure for both, but `mount` prints them as separate lines
  (`root_used_pct`, `overlay_free_kb`, `tmp_free_kb`).  Where the 704 KB is
  consumed matters (`/etc` config, `/etc/sensors/*.bin` ~115 KB, `/tmp/webui`
  ~27 KB, `/tmp/log` up to 64 KB) and has to be measured, not inferred.
* **What model each of these is.**  Their self-reported SoC/board puts cam7 on
  the `c110.md` C110 v2 line and cam5/cam9 on the doc's `ssc337` branch, but
  nobody read a label or a stock-version string, so "cam5 is a Tapo C110 vX" is
  not a fact here — and the reflash image is chosen per SoC, which is exactly
  when the difference stops being academic.
* **What firmware `cam1` is**, and what its interfaces answer — its feed is
  enabled and alive, its address and MAC are reserved, but its family is
  different and none of this directory's verbs have been run against it.
* **How a `*.int` device name gets to the device rather than the front door.**
  `curl http://cam5.int.leighhack.org/` answers **from cam5's own web
  interface** (401, realm `Authentication`), yet `cam5`/`cam7`/`cam9`/`cam1`
  are **not rows in `common/int-dns.nix`** — the table rule 14 says every
  `*.int` name belongs to — and both guards (`machines/gw/int-dns.nix`,
  `machines/services1/int-dns.nix`) pass with them absent.  So these four
  names are `*.int` names that answer with a LAN device, on a path no
  evaluation-time guard describes.  Open item, not a fact to "fix" by adding
  rows on a guess.

## Faults found on 2026-10-25

What the report in `./inventory.sh` exists to make repeatable.  None was fixed
by this pass; acting on 1–5 means changing a camera's config, and changing a
camera's config before the repo can hold it (`cam-pull` → `cam-diff` →
`cam-push`) is how the fleet gets quietly worse.

1. **A Telegram bot API key sits in a comment in cam7's `/etc/crontabs/root`**
   (the vendor's own example block; the command it comments out would post a
   snapshot of the camera to that bot).  **The value is deliberately not in
   this repo**: `cameras/config/cam7/root` has that line replaced by a redaction
   note, and the report gives that file's md5 and line counts rather than cat'ing
   it, because `cam-pull` puts these files into a store path and into a future
   transcript (rule 13).  **Blocks:** nothing (a commented line; crond reads
   `/etc/crontabs/root` and it has 0 active lines). **Needs:** whoever owns that
   bot to rotate the token, and the line deleted **on the device** — it is the
   vendor's, not ours.  Do not "just delete it from the repo copy".
2. **All three crontabs have 0 active lines** — nothing on a camera runs by
   itself.  A camera is not doing anything it was ever asked to do besides
   stream; and if a cron line is ever added, note **cam5 and cam9 also have
   `/etc/crontabs/root.r`** (md5 `999679e0…`, identical on both): busybox crond
   reads `crontabs/root`, so the `.r` file is dead config — what makes "why
   isn't my cron line running" a two-minute trap on these boxes.
3. **Only cam5 pins its sensor pad** (`isp:` → `sensorConfig: sensors/sc3338.bin`);
   cam7 and cam9 have no `isp:` section while their `/etc/sensors/` hold more
   than one pad, and all three signature lines say `sc3338`.  The picture is
   right today because the loader's default happens to be right; it is a
   finding because a firmware bump can change cam7's and cam9's image with no
   config diff to review.
4. **cam9 is the odd one out in three independent ways**: firmware 2.4.08.22
   like cam5 (yet cam9's `S95majestic` md5 matches cam5's while cam7's, on the
   older 2.3.11.22, does not); `nightMode` **disabled** but the only device
   carrying `S96autonight`; `video0` with **no geometry at all**; no `isp:`
   section; `records` enabled-space measured **464 KB** vs "Available" on cam5/cam7;
   `/etc/custom.ok` present.  A flash of cam9 would be the first camera to
   change what "flash it" means on this fleet.
5. **cam7's log ring has rotated its first 74 minutes out** (2 KB ring at boot,
   64,797 bytes at report time, `BusyBox v1.36.1 …` gone) while cam5's is
   **67 KB with boot intact** (51,297 bytes) and cam9's has rotated 4× (64,690).
   Any camera fault older than an hour is already gone from the device.  This
   is why `check` writes its findings where a reboot cannot eat them.
6. **cam5's pulled `S99sdconf.sh` is not cam5's** (device `ae29e67d…`, repo
   `1b4ac291…` = cam9's copy) — the only pulled file whose md5 disagrees with
   its device.  The set is otherwise byte-clean (see above), so this is a pull
   artefact to close with `just cam-diff cam5`, not a device fault.
7. **`/etc/crontabs/root` on cam5 and cam9 was rewritten 2026-10-24 22:47**
   (both byte-identical, md5 `999679e0…`… i.e. the `.r` file's content,
   while `crontabs/root` itself is md5 `a1356469…`) whereas `majestic.yaml` on
   cam5 is dated `2025-05-03`: something touched both cameras at the same minute
   and left them the same.  Recorded so nobody re-asks "why is this file newer
   than the config".
8. **frigate's camera config is not in the repo and is deciding to do nothing.**
   `/srv/frigate/config/config.yaml` is a runtime bind mount and its
   in-container generated `/dev/shm/go2rtc.yaml` is the thing that maps a feed
   name to a camera.  Today it **disables** `3d_1`/`3d_2`/`3d_3`/`voron` and
   comments out `rack_corridor`'s `cam1.int.leighhack.org:554/0/av0` line while
   leaving an `ffmpeg:` transcode line in play. **Blocks:** a camera fault that
   looks like "frigate is fine but the picture is old" cannot be told from
   "the config stopped meaning what the flake says". **Needs:** that file
   tracked, and the three-name mapping (DNS name → go2rtc stream key → frigate
   camera name) in one table both `just cam-probe` and the gatus rows can read.
9. **`/etc/custom.ok` exists and is empty** on cam5, cam7 and cam9: an install
   hook ran and completed on each — and on cam9 it is **absent**, which is
   another way cam9 got a different install path despite reporting 2.4.08.22.

## What belongs where

* **`cameras/README.md`** — this file: the table, the reach, the proven/guess
  split, the findings, the rules.  Update it in place, dated, saying what was
  read off which device.
* **`cameras/c110.md`** — the OpenIPC device story (firmware layers, MTD layout,
  how to flash, the traps).  Here rather than in `docs/` because it is the
  device counterpart of this file and this directory is the only place that owns
  these devices.
* **`cameras/inventory.sh`** — the busybox-ash report one camera runs
  (`just cam-check` pipes it to `sh -s` over ssh).  **POSIX sh + busybox applets
  only, no `awk`, no bash** (busybox on these boxes has no awk, so a first
  attempt at the report died on `awk: not found` seconds into every probe and
  looked like a dead camera), and no `rev` (absent on all three, which once made
  a `free_kb` computation print empty **on a box with 452 KB of it** — a broken
  check reads as a healthy device).  It never cats a credential and never cats
  a binary (the WiFi PSK lives in `/etc/wlanpass` + `/etc/wpa_supplicant.conf`,
  the Basic-auth user in `/etc/httpd-htpasswd.user`, the sensor pads in
  `/etc/sensors/` at ~115 KB, all four reported by md5/size only).
* **`cameras/config/<cam>/`** — what the device is running, as it was pulled
  (`majestic.yaml`, `httpd.conf`, `resolv.conf`, `crontabs/root` (named `root`),
  the `S9x*` init scripts).  Pulled, **not deployed**: nothing deploys them; a
  reflash overwrites them; this is the record that lets `cam-push` put them back.
  Credentials are held as md5s in the report, and one file (cam7's crontab) is
  redacted where the vendor example leaked a token.

## Rules for these devices, in the house's terms

1. **No reflash without someone asking**, exactly as with the switches (AGENTS.md
   rule 16): it overwrites `/etc`, and `/etc` **is** the configuration.  Everything
   here is a config-file operation instead.  And a reflash is not a free operation
   on the network either: the image goes on over U-Boot from SD or TFTP
   (`c110.md` §8.3, which also points at a `jenkins.int.leighhack.org` OpenIPC
   Tapo-C110 artifact — that URL is the doc's claim, **not verified by this**
   **pass**), and the first boot afterwards has the published default serial
   console login, so a freshly flashed camera answers on both interfaces with no
   credential of ours until someone sets it.
2. **`cam-diff` before `cam-push`.**  A camera's `/etc/majestic.yaml` is not a
   file this repo generates, so the honest operation is: read the diff, replace
   the whole file, prove it landed by md5, then restart the daemon.  `majestic.yaml`
   is read at **start**, not on SIGHUP: `just cam-init <cam> S95majestic restart`
   is what makes a change live, and that restart remounts the overlay, reinits
   the sensor and leaves the feed down for ~30 s (frigate's
   `retry_interval: 120` covers it).  `cam-push` refuses to print the restart
   hint if the md5 does not match — a write that did not land must not be
   followed by a restart.
3. **One config per device, visibly different.**  A fleet where cam9's `video0`
   has no geometry and cam5 alone pins its sensor is a finding, not a style
   choice.  Unifying means picking the sensor pad per device and re-`cam-diff`ing
   all three, not assuming vendor defaults match.
4. **Never print a camera's secrets into a transcript.**  The WiFi PSK
   (`/etc/wlanpass`, `/etc/wpa_supplicant.conf`), the Basic-auth user
   (`/etc/httpd-htpasswd.user`), the vendor crontab's Telegram token and the
   device login password are referenced by path/md5/length, same as every other
   secret in this repo (rule 13).  The realms (`Authentication`, `WallyWorld`)
   and user names are fine; the values are not.
5. **The addresses are the router's to hand out.**  A camera moves only by a row
   in `common/dns-records.nix` (MAC → VLAN 226 → address); nothing here pins an
   address, and the recipes read that table.
6. **The frigate end of the pipe is not in the flake.**  `/srv/frigate/config` is
   a runtime bind mount (`machines/services1/services/frigate.nix` mounts it,
   nothing generates it), so what the cameras feed is decided on services1,
   outside version control (fault 8).
7. **A camera is not a machine in the flake**: `just check`, `just switch`,
   `just boot`, the GOLDEN RULE and the auto-rollback timer mean nothing here.
   What replaces them is `cam-probe`/`cam-check` (alive, gated, feed geometry)
   before and after any change, and `just cam-init <cam> S95majestic restart` as
   the only restart primitive.  They also never enter `nixbld`, never take the
   hop key's *user* side, and must not be added to any deploy set.

Open items for the fleet go to **docs/follow-ups.md** (filed from this pass: the
Telegram token, frigate's config into the repo, the `*.int` names that answer with
LAN devices with no guard describing the path, the cam5 `S99sdconf.sh` md5
disagreement, and the Rust tool that would replace these shell recipes —
`crates/camera-monitor/` was started half-written and **deleted untracked the same
day**; the shell recipes above are the shipped thing and the crate is written from
a clean slate against this doc, see docs/follow-ups.md).

## How this was verified (2026-10-25)

* **`ssh root@<addr>` password-only worked; key-only did not** (host keys unknown,
  no `BatchMode` success, `authorized_keys_note=none`).  The vendor password is
  12 bytes and is named nowhere in this repo or in these notes.
* The report was run on all three via a pty runner (password typed into the prompt
  by the tool, never printed) — `cameras/inventory.sh` piped through `sh -s`.
  One tooling caveat worth recording for the next agent: **the ad-hoc runner sent
  the script as ssh's remote-command argument, and a ~10 KB script hit that path's
  length limit** (the first passes carried an 8 KB report and worked; the 10 KB
  version produced an empty reply and a `closed by remote host` line that looks
  like a dead camera).  **`just cam-check` does not have that limit** — it sends
  the script on ssh's *stdin* (`< cameras/inventory.sh`), no pty, no argv — so
  when a probe over an ad-hoc runner comes back empty, suspect the runner before
  believing the device, and split the script rather than chasing a phantom fault.
  Two other tooling artefacts found this pass and fixed in the report itself:
  a self-matched `ps` line invented a `httpd` process that does not exist
  (`ps w` + a note instead of a count), and `du -kb /overlay/root` reads 0 on the
  union mount so that line was deleted rather than left reading 0.
