# Which radio a camera is on: cam5's 1.3 fps is the channel, not the camera

**2026-10-31.** Asked: *"cam8 works great but cam5 is terrible — why?"* Both units were on
thingino by then, both in frigate, both nominally identical, and `cam5` was delivering
`camera_fps 1.31` while `cam8` delivered 5.0.

**The answer, in one line: it is not the camera, and on 2026-10-31 it was never the firmware
either — it is which access point each camera happens to be sitting on, and what channel that
radio chose.** `cam5` is a client of **AP2 – Pi Room (U7LT) on channel 6**, in a cell shared
with two other clients (one at −79 dBm) inside a 26-BSS RF environment; `cam8` is the **only**
client of **AP1 – Main Space (U7PG2), which was replaced that morning and auto-selected
channel 11**. Measured capacity: **cam5 61 KB/s up / 76 KB/s down, cam8 1.38 MB/s / 1.3 MB/s**
— a 22× gap between two cameras at the same signal level with byte-identical encoder config.

This file owns: the measurement, the test that separates *the cell* from *the camera's radio*,
the AP/BSSID map and how to re-derive it, what to do about the channel plan (**DECIDE** — the
knobs are in the UniFi controller, not this flake), the failure I caused while proving it, and
the two older camera docs it corrects.

## 1. What was identical, i.e. what the camera was not

Read off both devices over ssh (the full command set is §5):

| thing compared | cam5 (10.3.2.10) | cam8 (10.3.2.11) |
| --- | --- | --- |
| OS | `Thingino 2 (Figata)`, kernel `4.9.84` | *identical* |
| wifi driver | `ssw101b_wifi_usb`, 641 167 B, `Tainted: P` | *identical* |
| sensor module | `sensor_sc3336_mipi` | **also `sensor_sc3336_mipi`** — see §6 |
| encoder config | `/etc/raptor.conf`: stream0 `1920×1080@20`, `bitrate = 2000000`, `gop 40`; stream1 `640×360@5`, `512000` | *identical file* |
| RSSI at the camera | −52 dBm | −55 dBm |
| signal at its AP (controller) | −47.4 dBm | −46.0 dBm |
| negotiated link | `Bit Rate=65 Mb/s`, HT, `Power Management:on`, `Tx-Power=30 dBm` | *identical* |
| BSS | AP2, bssid `78:8a:20:f1:db:24`, **ch 6** (2437) | AP1, bssid `fc:ec:da:35:5a:14`, **ch 11** (2462) |

Everything the fleet normally blames itself for — driver, sensor pad, bitrate, geometry, power
save — is the same on both. So the difference is not in the box.

## 2. What was different: the cell, from the controller's side, not the camera's

Five-minute aggregates from the Network Application's own stats DB (mongo on 27117 inside the
`unifi-os-server` container; query recipe §5), 20:00–20:40 GMT:

| | cam5 on AP2 (ch 6) | cam8 on AP1 (ch 11) |
| --- | --- | --- |
| AP's RX rate *from* the camera | 23.6 Mbit avg (max 39) | **39 Mbit, sitting on the max** |
| AP's TX retries / attempts | 4 605 / 9 174 = **50 %** | 15 107 / 64 975 = 23 % |
| UniFi client satisfaction | 93, 5-min low **82** | **100** |
| 2.4 GHz clients on that BSS | **3**: cam5, `led-pi-room` (−62), `big-button` (**−79**, sat 65) | **1**: cam8 |
| radio channel utilisation | 48–56 % (`cu_self_rx` 37–44 %) | 39–43 % (`cu_self_rx` 16 %) |
| weakest client on the radio | **−79 dBm** | −46 dBm (= cam8 itself) |

And the end-to-end consequence, measured from services1 and from the cameras themselves
(the `dd` pair is the procedure `docs/camera-wifi-tx-2026-10-29.md` invented — it is a good
test and it is what settled this):

| | cam5 | cam8 |
| --- | --- | --- |
| 64 B ping → its own gateway, 40 × | **7 % loss, avg 331 ms, max 1048 ms** | 0 %, 4.3 ms |
| 1400 B ping → services1, 20 × | **5–10 % loss, avg 580 ms, max 1275 ms** | 0 %, 4.8 ms |
| **TX 2 MB, camera → services1** | **33.7 s = 61 KB/s ≈ 0.49 Mbit/s** | 1.49 s = **1.38 MB/s ≈ 11 Mbit/s** |
| **RX 2 MB, services1 → camera** | 26.8 s = **76 KB/s** | 1.59 s = 1.3 MB/s |
| AP2/AP1's own counter for the camera's uplink | ~12.0 MB / 5 min ≈ 40 KB/s | — (matches the `dd` figure) |
| frigate | `camera_fps 1.31`, **138 watchdog events in 20 min** | 5.0 fps, stable |

### The test that separates "the camera's radio" from "the medium"

This is the whole finding, and it is cheap:

* **Both directions are capped, and by about the same factor** (61 KB/s up, 76 KB/s down). A
  weak transmit chain — the diagnosis `docs/camera-wifi-tx-2026-10-29.md` reached for `cam8`
  — is *asymmetric*: the AP transmits at 20 dBm with better antennas, so a camera that cannot
  get its frames out still downloads well. cam5 downloads terribly too.
* **The AP's own retries toward the camera are 50 %.** The AP's frames to a −47 dBm client
  should not be failing half the time unless something else is on the channel.
* **The camera's *physical* rate is fine (23–39 Mbit) while its *throughput* is 0.49 Mbit/s.**
  Rate adaptation is not what is losing the data; frames are being corrupted and retransmitted,
  which is what co-channel interference does and what a failing PA does not.
* **The environment is right there in the client's own scan.** cam5 sees **26 BSSs** in 2.4 GHz;
  cam8 sees **4**. At cam5's position the loudest BSS in the band is `leighmodelsandhobbies` on
  **ch 5 at −45 dBm — 7 dB louder than the AP cam5 is associated to** (adjacent-channel, so the
  front end eats it), with three more BSSs on its own channel at −52, ch 3 at −57, ch 1 at −56,
  ch 9 at −51, and — damning for the placement question — **AP1's ch-11 `Leighhack` BSS at
  −40 dBm, 12 dB louder than its own AP**. cam8's only neighbour is one foreign AP on ch 1.
* **The cell has a −79 dBm client in it** (`big-button`, 10.3.2.21, satisfaction 65, 80 % retries
  on its own trickle). A client that weak drags the BSS's usable airtime for everyone on it, and
  AP2's radio is already broadcasting **three BSSIDs** (`Leighhack`, `Leigh IOT`, one hidden — all
  `xx:8a:20:f1:db:24`, i.e. the same radio, so three lots of beacon/probe overhead at the lowest
  mandatory rate).

Corollary, and the reason this matters beyond one camera: **`AP2`, `AP3` and `AP4` were all at
or beside channel 6** (measured below), i.e. the four cameras' cameras were stacked on one
channel while the building's newest radio sat alone on 11.

## 3. Why cam8 "got better": its AP was replaced that morning, not its firmware

`docs/camera-thingino-migration-2026-10-31.md` §5 records cam8's OpenIPC-era TX fault and says
it *"needs a fresh measurement under thingino … the doc's driver-vs-hardware question may just
have been answered."* Measured history from the controller (hourly buckets for cam8's MAC):

```
Wed Oct 28  ap=44:d9:e7:90:69:11 (AP1, the original unit)      sig -55  retries 14 988/50 245
Thu Oct 29  ap=44:d9:e7:90:69:11 (AP1, the original unit)      sig -52  retries  5 811/22 974
Sat Oct 31  ap=fc:ec:da:34:5a:14 (AP1, the REPLACEMENT, ch 11) sig -42  sat 99-100
```

AP1's original unit failed on 2026-10-31 and was replaced by a unit that kept its name and
address (`common/dns-records.nix` has that story and its MAC). The replacement auto-selected
channel 11, and cam8 became its only 2.4 GHz client. **That, not thingino, is when cam8
started working** — and the 2026-10-29 write-up's failing camera (≤45 KB/s, "Tx excessive
retries" barely moving) was on `44:d9:e7:91:69:11` = AP1's old unit **on channel 6** — a channel three of
this network's four APs were still on when this was measured two days later (AP2 at 6, AP3 at 6,
AP4 configured to 6 and heard on 8), carrying cam5, cam7 and cam1 beside it. (Channel state is
`auto`-chosen and not recorded in this repo, so the 2026-10-29 channels are inferred from the
2026-10-31 measurement plus that doc's own `2437 MHz` line, not remembered.) So:

* the **driver-vs-hardware A/B in that doc must not be run** — it tests a variable that is not
  in the fault, and one side of it risks a board-level oops for nothing;
* cam5 now carries the same ceiling, and it stayed on channel 6 when cam8 left it;
* "the thingino migration fixed cam8's WiFi" is not supported by the evidence and should not be
  repeated. (Nothing here says the migration was *not* an improvement — only that it is not what
  changed the WiFi.)

**But do not over-read that into "channel 6 breaks cameras".** This same file's predecessor
measured `cam5` at **1.38× real time on 2026-10-29 — on the AP and channel it is still on**, so the
channel is not a fixed sentence; a cell's *contents and load* are what vary (AP2 has a -79 dBm
client on it now, ch 6 has three APs' BSSs beside it, and the neighbouring ch 5 network is louder
than the serving AP). What the 2026-10-31 measurements do establish, and all they establish, is
that the fault sits at the cell rather than in the camera, and that it **moved with the AP/channel
each camera was on, not with the firmware either of them was running**.

## 4. The AP map, and how to tell a BSSID's owner

`common/dns-records.nix` pins the **management MAC** of each AP; a client sees a **BSSID**, and
on this fleet the 2.4 GHz BSSID is the management MAC **plus 0x100 in the second-from-last
group** (measured on three APs, and it is what identifies the old AP1 in the 2026-10-29 doc):
`fc:ec:da:34:5a:14` → `fc:ec:da:35:5a:14`, `78:8a:20:f0:db:24` → `78:8a:20:f1:db:24`,
`44:d9:e7:90:69:11` → `44:d9:e7:91:69:11`. Add a row to `cameras/README.md`'s table rather than
memorising offsets.

| AP | model | mgmt MAC (dns-records) | 2.4 ch — controller | 2.4 ch — measured | 2.4 clients (20:25) | weakest client |
| --- | --- | --- | --- | --- | --- | --- |
| AP1 – Main Space | U7PG2 (replaced 2026-10-31) | `fc:ec:da:34:5a:14` | `auto` | **11** | cam8 | −46 (cam8) |
| AP2 – Pi Room | U7LT | `78:8a:20:f0:db:24` | `auto` | **6** | cam5, led-pi-room, big-button | **−79** |
| AP3 – Fabrication | U7LT | `fc:ec:da:a6:4b:53` | `auto` | **6** (cam7's bucket reads `6:20`) | cam7, cam1, `00:33:7a:…` (−80), `78:e3:6d:…` | −80 |
| AP4 – Social | U7LT | `fc:ec:da:a6:26:43` | **`6` (pinned)** | **8** — `fc:ec:da:a7:26:43` is heard on 2447 | cam9, `54:32:04:…` | −65 |

Two things to read carefully there: **AP4's stored channel and its measured channel disagree**,
so before anyone "fixes the plan" from either source, get it from the UI (or from a client
scan, which cannot lie about frequency); and AP2/AP3 are *coincidentally* on the same channel,
which `auto` chose and which nobody decided.

## 5. How to re-measure this (all read-only)

The Network Application inside the appliance answers mongo on **27117**, and its stats live in
**`ace_stat`**, not `ace` (`ace` has `device`/`wlanconf` and *empty* `client`/`stat`
collections, which is how you lose twenty minutes). `stat_minute` is **empty** on this install —
stats retention is `stat_5minutes` / `stat_hourly`. Client docs key the MAC as **`oid`**, not
`mac`, and radio fields are prefixed `ng-` / `na-`.

```console
# which AP exists, what model, what it thinks its radios are
cat > /tmp/q.js <<'JS'
db.getCollection("device").find({type:"uap"},{name:1,mac:1,model:1,radio_table:1}).forEach(function(d){
  print(d.name+" "+d.mac+" "+d.model);
  (d.radio_table||[]).forEach(function(r){ print("  radio="+r.radio+" channel="+r.channel); });
});
JS
timeout 60 sudo podman cp /tmp/q.js unifi-os-server:/tmp/q.js \
  && timeout 120 sudo podman exec unifi-os-server sh -c 'mongo --port 27117 ace --quiet /tmp/q.js'
```

Per-client and per-radio numbers, then, from `db.getCollection("stat_5minutes")` with
`{o:"user",oid:"<client mac>",time:{$gte:…}}` and `{o:"ap",oid:"<ap mac>",…}`. Four traps, all
hit tonight: the mongo shell in that container is **legacy** (`countDocuments` does not exist,
use `count`; `datetime` is not always a string, so `String(d.datetime).substr(…)`); **project
the fields you want** — an unprojected `ap` doc is ~20 KB of `x-total-*` noise; a `--eval`
inside `"…"` lets **bash eat `$in`/`$gte`** (write the JS to a file and `podman cp` it); and a
5-minute bucket means a 4-minute query window legitimately returns nothing.

From the camera (works on thingino; `iw` is **not** installed, so `wpa_cli` is the only
RF oracle):

```sh
wpa_cli status | grep -E 'bssid|freq|ssid|wpa_state'
wpa_cli signal_poll                      # RSSI and LINKSPEED as the supplicant reports them
wpa_cli scan; sleep 6; wpa_cli scan_results   # the whole band; THIS is the interference answer
iwconfig wlan0                           # negotiated Bit Rate, Link Quality, retry/misc counters
grep -inE 'bitrate|width|height|fps' /etc/raptor.conf   # what the camera is trying to push
```

And the ceiling test, both directions, from services1 — the only two numbers that decide
"camera vs medium":

```bash
time timeout 75 ssh camera 'dd if=/dev/zero bs=64k count=32 2>/dev/null' > /dev/null   # TX
time dd if=/dev/zero bs=64k count=32 | timeout 75 ssh camera 'cat > /dev/null'          # RX
```

`/usr/bin/time` is not on services1 — use the shell's `time`. `rsd` answers **2 clients max**,
so an RTSP `DESCRIBE` from a third viewer *times out* on a busy camera; that is the slot limit,
not a new fault, and it is why the frigate log's own lines are the better witness.

## 6. Two claims this pass falsified

* **"`cam5`'s RF at its position is marginal: −63 dBm … the known bad-corner physics."** The AP
  sees **cam5 at −47 dBm** and the camera hears its AP at −52. **−63 dBm is AP2's
  `ng-client_signal_avg` — a per-cell average dominated by `big-button` at −79** — and 73 KB/s
  of throughput is real but is the cell's, not the position's. A per-cell average was read as a
  per-camera fact and it sent the diagnosis to placement. When a doc says "the camera is at N
  dBm", it must name *whose* measurement of *which client*.
* **"cam8 is genuinely `sc3338`" / "cam5 … sensor sc3336 … `rvd` streaming 2304×1296@20".** Both
  units now load `sensor_sc3336_mipi` and report `ipcinfo -s` → `sc3336`, and both
  `raptor.conf` files say **1920×1080@20** at 2 Mbit/s. (The sc3338 claim is OpenIPC-era; the
  sensor pad that mattered to the *flashing* rules is a separate question — do not re-open the
  "sc3338 was pruned from the build" reasoning from these two lines.)

## 7. What to do — the knobs are in the controller, so this is a decision, not a patch

* **DECIDE — give AP2's 2.4 GHz a channel, or turn that radio off.** AP2's 2.4 is `auto` and
  landed on 6 with two neighbours already on/near it. Options, cheapest first: pin AP2 to
  **channel 1** (at cam5: three foreign BSSs at −56/−56/−70, and 1 is far from the ch 3/5/6/8
  pile); or **switch AP2's 2.4 radio off and let its three clients go to AP1**, which cam5 hears
  at −40 dBm, 12 dB over its current AP. The second is better for cam5 and worse for whoever
  `big-button` and `led-pi-room` end up on — read their placement before doing it. Either way it
  is a UniFi UI change with no flake diff, which is exactly why it is still open.
* **DECIDE — un-pin AP4's channel** (`6` in config, heard on 8) so the stored plan and the
  radio agree; until then, neither source can be trusted to describe the network.
* **DECIDE — the Pi Room camera wants a wire or 5 GHz.** 2 Mbit/s upstream on 2.4 GHz, in a
  26-BSS neighbourhood, on a camera whose whole job is detection, is not a medium problem to
  solve with tuning. Until then `pi_room` will sit near 1 fps whatever the channel.
* **DON'T cut cam5 to `/stream1`** (640×360@5, 512 kbit/s) **as the fix**: on the 0.49 Mbit/s
  this cell delivers it is *still* above the ceiling, and it silently drops detection
  resolution. It is a legitimate stopgap for the watchdog churn only if labelled that way.
* **DON'T run the 2024-vs-2026 `ssw101b_wifi_usb` module swap** proposed by
  `docs/camera-wifi-tx-2026-10-29.md` §"The two candidates" — see §3 above.
* **Housekeeping, not the cause:** `Power Management:on` on both cameras; identical, so it
  cannot explain a difference between them, and it is still worth `iwconfig wlan0 power off`
  (runtime only, does not survive a reboot — the same one-liner the 2026-10-29 doc left off).

## 8. The failure I caused proving this, which is the rule that came out of it

To confirm "cam5 would be fine on a better BSS" I appended a **BSSID pin** to the `Leighhack`
network block in cam5's `/etc/wpa_supplicant.conf` and sent `SIGHUP` to wpa_supplicant:

```
        ssid="Leighhack"
        bssid=fc:ec:da:35:5a:14        # AP1's ch-11 BSS, −40 dBm at the camera
```

**cam5 stopped existing on the network within a minute and never came back.** The ssw101b
client never joins the pinned BSS (a BSSID-restricted scan on that vendor driver appears to
return nothing), and `bssid=` in a network block means *only this BSS*: with the pin in place
wpa_supplicant will not fall back to the AP it was happily using, and the second baked network
(`The Lab 2.4GHz`) is not audible in the Pi Room. cam5 sat disassociated and unreachable.

The second half — the part that turned a 30-second test into an outage: I had armed a
self-revert on the device,

```sh
( sleep 150; cp /tmp/wpabak.conf /etc/wpa_supplicant.conf; kill -HUP $(pidof wpa_supplicant) ) &
```

and **it died with the ssh session.** dropbear tears down the session's process group when the
channel closes; a backgrounded `sleep` is in that group, so the revert never ran and I had no
way back in, because the thing I broke was the network path. `nohup` would not have fixed the
class of problem either: the config lives in the persistent overlay, so the bad file would have
outlived any amount of waiting.

**The rule: on these cameras, an over-ssh change to the association is not revertible, because
the only channel back to the device is the association you are changing.** Three consequences:
(i) anything that can lose the Wi-Fi association gets done **at a console** (or not at all),
not over the Wi-Fi you are about to break; (ii) never trust a detached timer over ssh as a
safety net — dropbear reaps it, and a safety net that dies with the session is worse than
knowing you have none; (iii) before changing *how* a device associates, know where its pristine
config is — for thingino that is the read-only squashfs, see the runbook below.

### Recovery runbook (a camera pinned to a BSSID it cannot join)

Symptoms: the camera answers nothing — no ping, no `:554`, no `:80`, ARP `INCOMPLETE` on the
router, and **no client stats for its MAC against any AP** in the controller (that last check is
how you know it is off the air rather than merely broken or on a foreign VLAN). The lease row
survives (it is a reservation, not evidence of life). No remote path exists after that.

On the console (serial, `root` + the fleet camera password):

```sh
cat /etc/wpa_supplicant.conf            # confirm what the bad line actually is
rm -f /overlay/root/etc/wpa_supplicant.conf   # delete the OVERLAY copy; /rom has the default
kill -HUP $(pidof wpa_supplicant)       # re-reads config; rejoin is seconds
wpa_cli status | grep -E 'bssid|freq|wpa_state'
```

Why `rm` of the overlay copy is the whole fix, and why a power cycle is not: `/` is `overlayfs`
with `upperdir=/overlay/root` over the squashfs (`/dev/root /rom squashfs ro`), so an edited `/etc`
file persists across reboots in the 512 KB jffs2 data partition, while the pristine default stays
at `/rom/etc/wpa_supplicant.conf` (662 B, two networks, verified on cam8). **A reboot leaves the bad
file exactly where it was.** There is no self-heal, and that is now read off the script rather than
inferred: `S38wpa_supplicant`'s `start_client()` does nothing but start wpa_supplicant, and the only
timeout path in the file is the captive-portal mode's 600 s sound.

**Do not reach for `wlan reset` / `wlanreset` — it does not work on this build.**
`/usr/sbin/wlan`'s `reset()` is `rm /overlay/etc/wpa_supplicant.conf`, but on this port the overlay's
upper directory is `/overlay/root`, so `/overlay/etc` does not exist: the command deletes nothing,
reports nothing, exits 0, and never restarts wpa_supplicant. Measured on cam8 2026-10-31:
`ls -d /overlay/etc` → *No such file or directory* while `/overlay/root/etc` is there. The lesson is
wider than this command: a vendor recovery tool ported to a different overlay layout fails quietly,
so on these boxes the *path* is the fact and the exit status is not.

### Recovery without a console: the SD credential card

`S38wpa_supplicant`'s `start()` calls `credentials_from_card()` before it decides anything: if a FAT
card at `/mnt/mmcblk0p1` holds `uenv.txt` with `wlan_ssid=` and `wlan_pass=`, it runs
`wlan configure "$ssid" "$password"`, and `configure` **truncates and rewrites**
`/etc/wpa_supplicant.conf` with one clean network block. A card carrying `wlan_ssid=Leighhack` /
`wlan_pass=<the fleet camera password>` plus a power cycle therefore clears a bad config with no
console at all, and that is the option to reach for first. Three caveats, all of them this file's
own kind: `configure` stores `psk=` as the **hex PSK** rather than the passphrase (the file is still
a secret - rule 13); the rewrite drops the second baked network (`The Lab 2.4GHz`), so re-add it
afterwards if that network is wanted; and there is no `custom.ok`-style marker, so **the card
re-writes the credentials on every boot it is present for**. Read `/usr/sbin/wlan` before trusting
it - and notice that `wlan configure <ssid> <pass> [<bssid>|ap]` *accepts a BSSID argument*, which is
exactly why the pin in this section looked like a supported operation. It is supported by the tool
and not by the driver.

If no card either, the heavier path is the **reflash** card, and there is now a verb for it:
`just cam-rescue-card /dev/sdX` (with `CAM_CARD_YES=1` for the unattended form) writes the one-shot,
self-retrying `RESCUE.KRN`/`RESCUE.RFS` payload documented in
`docs/camera-thingino-build-deploy-2026-10-31.md`, whose §5-§7 carry the pins, the container-build
traps and the hashes; `cameras/thingino-manifest.sh` is what says the bytes on the card are the bytes
the fleet runs. That is a changeover-sized tool being used for a credentials-sized problem - reach for
the `uenv.txt` card first - but it is the path that survives a genuinely bad payload, since u-boot
reflashes kernel and rootfs on power-up and `S98sd-rescue-clear` eats the markers only after a Linux
boots. Worth knowing as a last resort: if `/etc/wpa_supplicant.conf` ever has **no `ssid=`**
line the box boots as a captive-portal AP (`THINGINO-<last two MAC octets>`, `172.16.0.1`, `udhcpd` +
`dnsd`), which needs a person with a laptop beside the camera - the same cost as the console, and the
reason neither is a substitute for the card.

**Status of cam5 at the time of writing: off the air, awaiting that console.** Its frigate feed
was delivering 1.3 fps of a wedged reader loop before the change, so the loss is small, but it
is a loss and someone has to walk to the Pi Room.

## 9. What this pass left changed

* `/root/.ssh/authorized_keys` on **cam5 and cam8**: the fleet hop key appended and deduped —
  the documented `just cam-key` bootstrap, done non-interactively because the password prompt
  is not scriptable. **Both thingino units had lost the hop key** at the changeover, so every
  `cam-*` recipe was failing on them; `cameras/README.md`'s "all three are keyed as of
  2026-10-25" is corrected there. cam5's file still holds three operator keys
  (`gemini-pda-root`, two for `cjdell`) — untouched, and the migration doc's "worth re-checking
  the authorized_keys situation" note is now fact rather than suspicion.
* `~/.ssh/known_hosts` (services1, operator account): **not** edited. Its `10.3.2.10` line is
  the pre-reflash OpenIPC host key, so `just cam-*` on cam5 fails on a host-key mismatch until
  someone runs `ssh-keygen -R 10.3.2.10` and re-tofus it — recorded in follow-ups rather than
  done here, because pinning a new key on a reflashed device is a human TOFU decision.
* Nothing in the flake, in frigate's config, or in the controller. No camera's flash, `/etc`
  (other than `authorized_keys` and the pin in §8) or stream config was touched.
* **Credential note, deliberately without the value:** the pre-migration fleet root password still
  authenticates as root on both thingino units over ssh, *and* it sits in plaintext in a
  *commented-out* RTSP URL in `/srv/frigate/config/config.yaml`. Because cam8's data partition was
  wiped by the migration flow and cam8 still takes that password, the credential is **baked into the
  image** - so it lives in the SSOT repo's history beside the two Wi-Fi PMKs and is in scope of the
  leaked-pending-rotation item in `docs/camera-thingino-build-deploy-2026-10-31.md` §7. Rotating it is
  a re-bake, not a `passwd`, which is why it belongs to that item and not to this file. Add thingino's
  default of **no RTSP auth** (migration doc §1) and the state on VLAN 226 is open video plus a live
  shared root password; both are the item to close, and neither is a tidying task.
