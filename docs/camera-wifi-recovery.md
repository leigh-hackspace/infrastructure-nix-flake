# Recovery with an SD card (no console, no WiFi)

**Status: built and run once, on `cam8`, 2026-10-29.** The camera was taken offline
deliberately, recovered from the card, and put back — the receipts are at the end. It covers
exactly one failure class: **Linux boots, the radio does not** (credentials changed, or the
WiFi module is missing). It is not a substitute for the console; a camera that will not boot
still needs `docs/camera-sd-card-firmware-flash.md`.

## 1. Where the credentials actually are

Measured on all four cameras on 2026-10-28. Every one of them reads the SSID and PSK out of
the **U-Boot environment**, not out of a file:

```
$ cat /etc/network/interfaces.d/wlan0        # cam9, and cam8 after 2026-10-29
iface wlan0 inet dhcp
    pre-up wlan_conf "$(fw_printenv -n wlanssid)" "$(fw_printenv -n wlanpass)" > /tmp/wpa_supplicant.conf
    pre-up wpa_supplicant -B -i wlan0 -D nl80211,wext -c /tmp/wpa_supplicant.conf
    post-down killall -q wpa_supplicant
```

`/usr/sbin/wlan_conf` hex-encodes the SSID and emits the network block; `/tmp` is tmpfs, so
the file is regenerated on every `ifup`. `cam5` and `cam7` carry an **overlay copy** of this
stanza that uses `wpa_passphrase` and `post-up` instead — same effect, same env source.

**`/etc/wpa_supplicant.conf` is a stale open-network stub and is not read:**

```
ctrl_interface=/var/run/wpa_supplicant
ap_scan=1

network={
  key_mgmt=NONE
}
```

`cameras/README.md` said the PSK lived in that file (and earlier notes said "in the rootfs").
Both were wrong on every board: it lives in `wlanssid`/`wlanpass` in the env, which is also why
`just cam-env` masks those two keys and why every env dump is a secret. Rotating the WiFi key
therefore means **changing the env** — and that is the whole difficulty, because the env is
also the partition that holds `bootcmd` and `ethaddr`.

## 2. Why the card is the only way in

A C110 has no ethernet port, this build has one rootfs (no second bank), OpenIPC's `failsafe`
rescue is gk7205-only, and the radio is the only network interface. So a camera that boots
without `wlan0` cannot be reached at all.

One thing runs from the card at boot, and it is the whole in-band recovery surface:

```sh
# /etc/init.d/S99sdconf.sh
if [ -f /mnt/mmcblk0p1/sdconf.sh ]; then
    echo "======== SD CONFIG DETECTED ========"
    /bin/sh /mnt/mmcblk0p1/sdconf.sh
else
    echo "======== SD CONFIG NOT DETECTED ========"
fi
```

Nothing else on the box reads the card (`grep -rn mmcblk /usr/share/openipc /etc/init.d` finds
only this), and **the hook is not fleet-wide**:

| board | `S99sdconf.sh` | card recovery available |
| --- | --- | --- |
| `cam5` (10.3.2.10) | yes | yes |
| `cam7` (10.3.2.12) | **no** | **no** — console only, until the hook is added |
| `cam8` (10.3.2.112) | yes | yes |
| `cam9` (10.3.2.17) | yes | yes |

## 3. The card

The payload script is [`../cameras/sd-recovery/sdconf.sh`](../cameras/sd-recovery/sdconf.sh).
Card layout, FAT32, at the root:

```
/sdconf.sh                      the script (chmod +x not needed; it is `sh`-ed)
/sdrecover/wifi.txt             line 1 = SSID, line 2 = PSK, no other lines
/sdrecover/files/…              optional: extra files, paths relative to /
                                (only /etc/** and /lib/modules/** are accepted)
```

It runs at **S99**, i.e. after `S40network` has already failed to associate, so it applies the
files and **reboots**; the second boot is the one that comes up on the new credentials. To
avoid a boot loop it renames `sdrecover/` to `sdrecover.done/` on the card, so a card left in
the slot does nothing on later boots. To retry, rename it back on a computer and edit
`wifi.txt`.

What it writes, and why it is not the env:

- `/etc/wpa_supplicant.conf` — generated with the stock helper and then `scan_ssid=1` in place
  of the cleartext `#psk=` comment (`wlan_conf` on 2026, `wpa_passphrase` on the older overlays
  — the script uses `wpa_passphrase`, which exists on all four).
- `/etc/network/interfaces.d/wlan0` — three lines that read **that file** instead of the env.
- Both land in the jffs2 overlay, so they survive reboots and are removed by deleting the
  overlay copies.

**It does not write the U-Boot env**, on purpose. `/etc/fw_env.config` is
`/dev/mtd1 0 0x10000 0x10000 0x1` — one 64 KiB sector, **no redundant copy** — and the same
partition holds `bootcmd` and `ethaddr`. A `fw_setenv` interrupted mid-write costs the console
at best and the MAC at worst, and `docs/camera-sd-card-firmware-flash.md` §12 forbids it. The
file override gets the camera back without touching mtd1.

## 4. The procedure

1. **Check the hook exists** — if `S99sdconf.sh` is absent this cannot work:
   `ssh root@<cam> 'ls /etc/init.d/S99sdconf.sh'`.
2. Put `sdconf.sh` and `sdrecover/wifi.txt` on a FAT32 card at the root, with the new SSID and
   PSK. If you are editing the card from the camera itself, build `wifi.txt` from the env so the
   secret never reaches a terminal:
   `{ fw_printenv -n wlanssid; fw_printenv -n wlanpass; } > /mnt/mmcblk0p1/sdrecover/wifi.txt`.
3. Card in, power cycle (or `reboot` if you still have a shell).
4. Watch the console if you have one: `SD CONFIG DETECTED` → `sdrecover: payload found` →
   `sdrecover: credentials written` → `sdrecover: applied; rebooting`.
5. The camera comes back on the new credentials. **Delete `sdrecover.done/wifi.txt`** — it
   holds the PSK in cleartext.

## 5. Cleanup once you have a shell

The override means the env is ignored, which is fine as a recovery but wrong as a resting
state: the next `fw_setenv wlanssid` would appear to do nothing. Either

- put the credentials where the fleet expects them (`fw_setenv wlanssid …; fw_setenv wlanpass …`
  — the §12 caveat still applies, so do it deliberately, with power you trust), then
  `rm /overlay/root/etc/network/interfaces.d/wlan0 /overlay/root/etc/wpa_supplicant.conf` and
  reboot; or
- just leave the override and note it, knowing the env is stale.

Removing the two overlay files restores the stock stanza — verified on `cam8`, which then came
up with `wpa_supplicant … -c /tmp/wpa_supplicant.conf` and DHCP `10.3.2.112`.

## 6. What this does not cover

- **The WiFi module is missing from the rootfs** (the outage that started
  `docs/camera-sd-card-firmware-flash.md` §9a). Same card, same hook: put the module under
  `sdrecover/files/lib/modules/4.9.84/…` and it is installed into the overlay. It must be
  built for the kernel that is running — a driver from a different release oopses, which §9a
  documents.
- **The rootfs does not boot.** That is §8/§10 of the flash guide: U-Boot, a card, `sf update`.
- **`cam7`** — no hook. Adding `S99sdconf.sh` to its overlay (`cam-push`) is a one-file change
  and would bring it level with the other three.
- **A wiped overlay.** The hook lives in the overlay, not in the rootfs; losing the settings
  partition loses the recovery path too.

## 7. Receipts (2026-10-29, `cam8`)

```
======== SD CONFIG DETECTED ========
sdrecover: payload found
sdrecover: credentials written
sdrecover: applied; rebooting
...
======== SD CONFIG DETECTED ========      # second boot: payload consumed, nothing done
```

```
$ tr '\0' ' ' < /proc/$(pidof wpa_supplicant)/cmdline
wpa_supplicant -B -i wlan0 -D nl80211,wext -c /etc/wpa_supplicant.conf    # the override
$ wc -l < /etc/wpa_supplicant.conf   -> 6 lines, scan_ssid=1, psk=<hex>, no #psk cleartext
$ ls /mnt/mmcblk0p1/                 -> sdconf.sh  sdrecover.done/
```

After removing the two overlay files and rebooting, `wpa_supplicant … -c
/tmp/wpa_supplicant.conf` and `inet 10.3.2.112` — the stock path, intact.

**One thing learned by doing it:** `cam8` had a *pre-existing* overlay copy of
`/etc/network/interfaces.d/wlan0` (the 2024-era `wpa_passphrase`/`post-up` variant). The test
overwrote it and the rollback deleted it, so the board now runs the stock 2026 stanza
(`wlan_conf`, `pre-up`). Both work; the stock one is the better state, but a "rollback" is only
a rollback if you checked what was there first.

## 8. Security

`sdconf.sh` is **root code executed from removable media on every boot**. Physical access to
the card is root on the camera. Do not leave a card with a payload in a camera you are not
holding, and treat `sdrecover/wifi.txt` as the PSK it is — the repo's rotation item for that key
(`docs/follow-ups.md`) is still open.

## 9. Related

- `docs/camera-sd-card-firmware-flash.md` — the flashing procedure (§8/§9a), the address map,
  and why `fw_setenv` is forbidden (§7, §12).
- `cameras/README.md` — the fleet table and how to reach a camera.
- `cameras/sd-recovery/sdconf.sh` — the payload.
