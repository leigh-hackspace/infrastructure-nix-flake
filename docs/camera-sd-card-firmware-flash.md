# Flashing a camera's firmware from an SD card (procedure, 2026-10-25)

**Status: nothing has been flashed.** This is the procedure, assembled on 2026-10-25 from
measurement on the three devices (`cam5`/`cam7`/`cam9`), and it has not been executed on any
of them. Every number below names the command that produced it; anything that has not been
measured is in §11 as an unknown with the test that closes it, and **nothing in §1–§9 should
be run on a camera before §11's first two items are closed on the bench.**

**Settled first, so nobody spends a day on it: the firmware to flash is OpenIPC, not thingino.**
thingino builds for **Ingenic** SoCs and this fleet is SigmaStar (`ssc337` ×2, `ssc333` ×1) — its
252 device configs and 176 release images contain no SigmaStar board, and its one SigmaStar
Kconfig model (`ssc30kq`) is a different chip on a different family. Receipts, the four commands
that re-answer it, the purchase case where thingino *is* the right answer (C110 **v3** = Ingenic
T23N), and what a firmware swap would cost the frigate pipe:
**[`./camera-thingino-evaluation-2026-10-25.md`](./camera-thingino-evaluation-2026-10-25.md)**.
Everything below assumes an OpenIPC image (`uImage.<soc>` + `rootfs.squashfs.<soc>`), because
thingino's images are a single assembled `.bin` with its own partition table — a different write
entirely, not a different filename.

**The question was "can we rig these to flash a whole image from SD on next boot, maybe by
modifying U-Boot?" — and the answer is: no modification is needed, and one is not
advisable.** All three cameras' U-Boot environments *already* contain the SD flashing
commands, because OpenIPC ships them:

```
setsdcard = setenv updatetool fatload mmc 0        # switch the loader from TFTP to the card
uknor     = mw.b ${baseaddr} 0xFF 0x1000000; ${updatetool} ${baseaddr} uImage.${soc};
            sf probe 0; sf erase ${kernaddr} ${kernsize}; sf write ${baseaddr} ${kernaddr} ${filesize}
urnor     = mw.b ${baseaddr} 0xFF 0x1000000; ${updatetool} ${baseaddr} rootfs.squashfs.${soc};
            sf probe 0; sf erase ${rootaddr} ${rootsize}; sf write ${baseaddr} ${rootaddr} ${filesize}
ubnor     = sf probe 0; sf erase 0x0 ${kernaddr}; sf write ${baseaddr} 0x0 ${kernaddr}
```

(`just cam-env <cam>` prints the env with the WiFi PSK masked. Read-only: this recipe never
runs `fw_setenv`, and §12 says why nothing else should either.)

So the only thing "rigging it for next boot" would add is *not needing a serial console*,
and buying that means editing `bootcmd` in the environment — the one partition a kernel or
rootfs flash does not restore. That trade is analysed in §7 and the recommendation is no.

---

## 1. What these boards are, as measured

| Fact | Value | How |
| --- | --- | --- |
| Layout, all three identical | `256k(boot),64k(env),2048k(kernel),5120k(rootfs),-(rootfs_data)` | `cat /proc/cmdline` |
| Sum of partitions | **8,388,608 B** — the whole chip, no unallocated space | `/sys/class/mtd/*/size`, `just cam-flashdump` → `mtd-layout.txt` |
| Settings live on **jffs2 on `/dev/mtdblock4`**, mounted at `/overlay`, with `/` an overlay whose upper half is `/overlay/root` | 704 KB, **244 KB used / 460 KB free** | `mount` (see §2 — this corrects this directory's older claim) |
| U-Boot | `U-Boot 2015.01 (Nov 05 2023 - 19:22:35)`, `ver` in env | `just cam-env <cam>` |
| IPL in the boot partition | `IPL_` header, **`I6b0 g05c44be`**, `IPL_CUST`, `UBOOT_ENV`, `runUBOOT()`; U-Boot itself is **not plaintext** in the dump (no `U-Boot 20`/`mmc`/`fat` strings → compressed) | `just cam-flashdump` then `tr -c '[:print:]' '\n' < mtd0-boot.bin` |
| SoC name the env uses for image filenames | cam7 `soc=ssc333`; cam5 and cam9 `soc=ssc337` | `just cam-env <cam>` |
| SD host | enabled and probing: `mmc0`, major 179, `[sdmmc] ms_sdmmc_probe`, DTB `soc/sdmmc` `status="ok"` with **`slot-removable`**, `slot-cdzs-gpios`, `slot-pwr-gpios`, `slot-fakecdzs=0`, `slot-max-clks=48 MHz` | `just cam-card <cam>`, `ls /proc/device-tree/soc/sdmmc/` |
| RAM given to Linux | `MemTotal 27,696 kB` (cam5/cam9), `27,628 kB` (cam7); `MemAvailable` 11,912 / 12,160 / **9,748 kB** | `/proc/meminfo` |
| `/tmp` | tmpfs, 13.8 MB — an image fits there | `mount`, `df -k /tmp` |
| In-place updater | **installed on all three**: `/usr/sbin/sysupgrade` v**1.0.41** (cam5, cam9) / v**1.0.21** (cam7); `flashcp` present; `fw` absent | `sysupgrade -h`, `command -v` |
| Console | `console=ttyS0,115200`, `bootdelay=0`, env `stdin/stdout/stderr=serial` | `/proc/cmdline`, env |
| Wired ethernet in U-Boot | `ethact=sstar_emac`, `ipaddr=192.168.1.10`, `serverip=192.168.1.1`, default `updatetool=tftpboot` | `just cam-env <cam>` |
| Camera cannot fetch its own image | `curl https://…` → exit **60** (untrusted cert); `http://10.3.1.20/` → `000` (VLAN 226 is not allowed at the front door); `ping 1.1.1.1` works | `just cam-preflight <cam>` gate 10, direct `curl` |

**Consequence of the last row:** the image gets *pushed* to the camera (scp with the hop
key) or carried on a card. Nothing in this procedure depends on the camera reaching the
internet.

## 2. The two beliefs this measurement replaced

Both were written into this repo on 2026-10-25 by earlier passes and both were wrong; they
are recorded here because the corrections change the risk model, not just the trivia.

- **`/etc` is not on a tmpfs root and is not re-read from anything.** It is on **jffs2 on
  the settings partition** (`/dev/mtdblock4 on /overlay type jffs2`), and `/` is an overlay
  whose upper dir lives there. So the config **survives reboot, survives a kernel or rootfs
  reflash, and is written to by the running system** — `mount | grep ' / '` says `rw`, not
  the read-only `nomount` this directory used to promise. Two operational consequences: a
  stray write to a camera is **permanent** (so `cam-init`'s read-only dance is a
  belt-and-braces measure, not a necessity, and nothing about `/etc` "silently disappears"
  — which `cam-diff`'s reason-for-existing paragraph assumed), and OpenIPC's own updater
  only needs to relocate that jffs2 volume, not resurrect a config from nowhere.
- **These are not the split/`mtdsplit` layout.** The kernel is handed an explicit
  `mtdparts=` on the command line and the partition table is fixed, so *`rootfs_data`
  starts at 0x750000 whatever the squashfs size is*. The upstream updater's split-NOR code
  (one `firmware` partition, divided at boot, settings relocated to follow the new
  squashfs) describes a **different** layout from the one these cameras run. The settings
  therefore do not move during a flash, and the whole class of "the update relocated the
  settings and lost them" failures applies to the other layout, not to this one. (Read
  `cameras/README.md` before believing any older sentence about tmpfs or about settings
  migration; both are corrected as found.)

What still stands from that scare, and matters more now: `/etc/sensors/*.bin` and
`/etc/majestic.yaml` are inside that overlay, so **the sensor selection and every pad
survive an update of this kind** — the pads are not what an update risks. It risks the
picture only if a newer `majestic` cannot parse the older `majestic.yaml`, which is a
software-compatibility question with a `cam-diff`-visible symptom and a
restore-the-rootfs-from-the-dump cure.

## 3. The address map, and what each command destroys

`kernaddr=0x50000 kernsize=0x200000`, `rootaddr=0x250000 rootsize=0x500000`, from the env;
the offsets are `mtd-layout.txt` from the dump. They agree exactly.

| Region | Flash range | Size | Written by | Destroyed by |
| --- | --- | --- | --- | --- |
| `boot` (IPL + IPL_CUST + U-Boot) | `0x000000–0x040000` | 262,144 | nothing in §8 | **`ubnor`** |
| `env` (U-Boot environment, incl. the PSK) | `0x040000–0x050000` | 65,536 | nothing in §8 | **`ubnor`** — it erases `0x0`–`0x50000`, i.e. boot **and** env |
| `kernel` = mtd2 | `0x050000–0x250000` | 2,097,152 | `uknor` | `uknor` |
| `rootfs` = mtd3 | `0x250000–0x750000` | 5,242,880 | `urnor` | `urnor` |
| `rootfs_data` = mtd4 (the settings, jffs2) | `0x750000–0x800000` | 720,896 | — | **untouched by `uknor`/`urnor`** |

Three things follow, and they are the reason this procedure is survivable:

1. `uknor` and `urnor` erase exactly mtd2 and mtd3 and nothing else — **WiFi credentials,
   host keys, `authorized_keys`, majestic config and sensor pads all sit outside what they
   touch.**
2. `ubnor` is a different animal: it erases the boot region *and the environment*. It is not
   a kernel/rootfs command and must never be run to "re-flash the boot loader" without an
   image that also contains a valid env (§10).
3. The write is `sf write ${baseaddr} ${rootaddr} ${filesize}` — it writes `filesize` bytes
   and does **not** consult `rootsize`. An image whose rootfs is larger than 0x500000 would
   be written straight across the start of `rootfs_data` and destroy the settings. The gate
   exists only in this document: **check the size before running `urnor`** (§6). The current
   builds fit with 106,496 B to spare.

## 4. Preconditions, in order

All four are read-only on the device and all four are already done for all three cameras
(dated 2026-10-25, in `$HOME/camera-backups`, mode 700 — never in the repo, per `AGENTS.md`
rule 13):

```bash
just cam-preflight cam7      # 10 gates; on 2026-10-25: 9 PASS, 1 UNKNOWN, 0 FAIL
just cam-flashdump cam7      # 5 partitions, each md5-checked against the device
just cam-backup cam7         # /etc + /root tar + env + dmesg: the WiFi/SSH recovery set
just cam-image ssc333_lite   # stages + verifies the build, prints the sizes to re-run
```

`cam-preflight`'s gates are not ceremony: gate 5 is "is the settings partition
flash-backed", gate 6 "are the WiFi credentials actually on the box" (`/etc/wpa_supplicant
.conf` — the runtime file; `/etc/wlanpass` is the *SD customizer's* filename and does not
exist in a running config, whatever the older notes here say), gate 7 "will ssh come back",
gate 2 "does the staged image fit the partitions". A FAIL means stop and measure, not
argue.

Before flashing anything, also:

- **Check that the build still ships this board's WiFi driver.** A C110's WiFi is a USB
  module loaded *from the rootfs* — `/lib/modules/4.9.84/extra/hal_apollo/ssw101b_wifi_usb.ko`,
  selected by the env's `wlandev=ssw101b-ssc333-tapo-c110` — so a rootfs that no longer
  carries it boots fine and joins nothing: no DHCP, no ARP, no ssh, and from the LAN that
  is indistinguishable from a dead camera. Measured 2026-10-28 on `cam8` (10.3.2.112,
  C110 **v1**, `ssc337`): `nightly-20261028-9a84dce` `ssc337_lite` **and**
  `ssc337_ultimate` both ship `/etc/wireless/usb` naming `ssw101b` and **no `ssw101b`
  module anywhere**, while the 2024 rootfs in that camera's own dump does. Flashing the
  generic build removed the only thing putting it on the air, and the rollback then needs
  a console. One command closes it first:
  `unsquashfs -l rootfs.squashfs.<soc> | grep -c ssw101b` (and `atbm6032i` for the
  ATBM603x C110 v2 revs) — **0 is a stop, not a detail.**
- **`just cam-check cam7` and fix its web admin.** On 2026-10-25 cam7 answers `200`
  unauthenticated on `:80`/`:85` while cam5/cam9 answer `401`. An open admin plane on the
  device about to be flashed is an open upgrade button in a room you are not watching.
- **Decide the device order: cam7 first.** It is the odd firmware out (2.3.11.22, the only
  one on `ssc333`, and the one whose updater is a version older than the other two), it has
  the least load-bearing feed, and everything learned about its env applies to cam5/cam9
  except the two filenames. **cam9 last** — its sensor is disputed (`env sensor=sc3338`,
  `docs/follow-ups.md` records `sc401ai`, its sysinfo line says `sc3338`) and it has no
  `isp:` pin.
- **Expect a feed gap.** frigate's `retry_interval` is 120 s and the container restarts
  forever; the camera reboots once per image. Nothing needs doing, but `cam-probe` will
  fail for a minute and that is not an incident.

## 5. Getting a console (the honest part)

Everything in §8 needs the U-Boot prompt.

- **Pins:** UART0, **3.3 V logic, 115200 8N1**, TX/RX on the main PCB near `TP501`, boot
  banner `IPL` then `U-Boot 2015.01` — all from `cameras/c110.md` §8.3, which did this on a
  donor board of this family. **Never 5 V TTL.** The `console=ttyS0,115200 panic=20` on the
  command line and `stdin/stdout/stderr=serial` in the env confirm the kernel and U-Boot
  both talk to it.
- **Unproven for these three units:** that the pads are reachable without removing a shield
  (`c110.md` §2 records the donor's being solder-jumpered to a header, and §"What is not
  proven" says to assume hot air may be needed), and that the pads on *this* PCB are where
  the donor's were. This is unknown §11-1 and it is the first thing the bench decides.
- **Interrupting autoboot with `bootdelay=0`:** the OpenIPC build for this family
  (`configs/infinity6b0_defconfig` + `include/configs/infinity6b0.h`, read 2026-10-25) has
  `CONFIG_BOOTDELAY 0`, **`CONFIG_ZERO_BOOTDELAY_CHECK`** and
  `CONFIG_AUTOBOOT_KEYED` with `CONFIG_AUTOBOOT_DELAY_STR "\x0d"` — i.e. it polls the
  console even at zero delay and interrupts on **Enter**. Our U-Boot identifies itself as
  `U-Boot 2015.01 (Nov 05 2023)` and is plausibly that build but **is not provably it**, so
  treat "hold Enter through power-on, or send `\r` at ~9600–115200 as soon as power is
  applied" as an attempt, not a certainty. Failing to interrupt is harmless: the camera
  boots normally and you try again.
- **If the console cannot be interrupted** the fallback is not this document: it is §10's
  restore-from-Linux, which needs no console at all and works whenever Linux still boots.

## 6. Making the card

Tooling verified present on services1 (`sfdisk`, `mkfs.vfat`, `fatlabel`, `dosfsck`,
`mtools`); no internal MMC slot, so a USB reader. **Verify the device node before every
write — this destroys the card's contents, and `c110.md` §8.1 warns some cards never work,
asking for a blank FAT32 card of ≤ 32 GB.**

```bash
DEV=/dev/sdX; PART=${DEV}1            # from lsblk -o NAME,SIZE,TRAN; re-read it twice
sudo umount ${PART} 2>/dev/null || true

# MBR + one FAT32 partition starting at LBA 8192.  `fatload` on this generation needs a
# real MBR partition table and a FAT32 BPB — a superfloppy card or exFAT reads as garbage.
sudo sfdisk ${DEV} <<'EOF'
label: dos
unit: sectors
start=8192, size=3071936, type=c, bootable
EOF
sudo partprobe ${DEV} 2>/dev/null || sudo partx -u ${DEV} 2>/dev/null || true
sudo mkfs.vfat -F 32 -S 512 -h 32 -s 64 -n OIPC ${PART}

# The filenames are dictated by the env: uImage.${soc} and rootfs.squashfs.${soc}.
d=$(ls -dt ~/camera-images/nightly-*-ssc333_lite | head -1)
read K R <<<"$(ls $d/uImage.ssc333 $d/rootfs.squashfs.ssc333)"
echo "kernel  $K  $(wc -c <$K) bytes  (partition 0x200000 = 2097152)"
echo "rootfs  $R  $(wc -c <$R) bytes  (partition 0x500000 = 5242880)"
# §3 note: sf write does not check this. If rootfs > 5242880, STOP: it would land on the
# settings partition. Same check for the kernel against 2097152.
[ "$(wc -c <$R)" -le 5242880 ] && [ "$(wc -c <$K)" -le 2097152 ] || { echo TOO BIG; exit 1; }

sudo mkdir -p /mnt/oipc && sudo mount ${PART} /mnt/oipc
sudo cp "$K" "$R" /mnt/oipc/ && sync && sudo umount /mnt/oipc

# Prove what the CPU will actually see, rather than trusting the filesystem calls.
sudo dd if=${DEV} bs=1 skip=$((8192*512)) count=8 status=none | head -c 8; echo   # FAT32
dd if=${DEV} bs=512 skip=$((8192 + $(sudo od -An -tu1 -j28 -N1 ${DEV} | tr -d ' ' \
   | head -1))) count=1 status=none 2>/dev/null | head -c 1 | od -c | head -1     # 0x0F
```

The last two lines are the paranoia `c110.md` §8.1 says the card demands: the FAT32
signature at the partition start, and a boot indicator of `0x0F`/`0x00` in the table. A card
that `mount` accepts and `fatload` refuses is the classic waste of an afternoon on a ladder.

## 7. Why not `fw_setenv bootcmd` (the "rig it for next boot" idea)

To flash from a card with nobody near the camera, the env's `bootcmd` would have to try the
card before the flash — e.g. something in the shape of

```
fw_setenv bootcmd 'if fatls mmc 0; then run setsdcard; run uknor; run urnor; reboot; fi; sf probe 0; …'
```

Do not do this, and here is the arithmetic of the risk rather than a vibe:

- The gain is one console session. The commands already exist; only the *keying* is missing.
- The cost is total. `bootcmd` lives in `env` (0x040000–0x050000). A kernel or rootfs flash
  does not restore it, and the command that would restore it — `ubnor` — **also erases the
  env** (§3), so a botched `bootcmd` cannot be undone by the tool you would reach for. You
  would be at the bench, with a shield to remove, holding a 320 KiB boot+env image you hope
  you made.
- The failure is silent and permanent-on-power-cycle: the bad `bootcmd` runs on every boot.
  A camera that reboots into a broken `bootcmd` never comes back to the state where you
  could `fw_setenv` it again.
- It also has to be right about the card *every* boot: an `if fatls mmc 0` that succeeds
  against a leftover card re-flashes a camera that was working fine.

If someone insists, the only defensible version is a `bootcmd` that requires an explicit
sentinel file and a `sleep`-free, `run`-free single line — and the original `bootcmd` must
be written down from `just cam-env <cam>` **and** present in a dump taken that day, so it can
be restored from the env image. Better: take the console once. That session is worth having
regardless, because it is also how §11-2 gets closed.

## 8. The flashing sequence

At the U-Boot prompt, with the card from §6 in the slot. **Read the env before typing
anything**: `${soc}` selects the filename, and `${updatetool}` must become `fatload mmc 0`.

```
sf probe 0
mmc info                      # does the card answer at all?  (see §11-1)
run setsdcard                 # updatetool := fatload mmc 0
fatls mmc 0                   # expect exactly uImage.<soc> and rootfs.squashfs.<soc>

run uknor                     # erase 0x050000+0x200000, write the kernel
run urnor                     # erase 0x250000+0x500000, write the rootfs
reset
```

What you should see for each: `mw.b` fills RAM, `fatload …` reports `** File not found …`
or `XXXXX bytes read`, `sf probe` reports the flash chip, `sf erase` reports the range,
`sf write` reports the byte count. **The byte counts are the whole check**: `uknor` must
report ~2,003,8xx and `urnor` ~5,136,384 for the 2026-10-24 builds. A `fatload` that reads
0 bytes followed by a successful-looking `sf write` of 0 bytes leaves an erased partition
and an unbootable camera, which is the mistake this table exists to prevent — `sf write`
does not care how empty what it wrote was.

Do **one at a time, verify the byte count, then the next**. Erasing both and then failing to
load is how a camera that was up becomes a paperweight; one at a time means the worst case
is one bad partition, and §10 can put one partition back from Linux if the other still
boots.

Then, from Linux after it comes up:

```bash
just cam-probe cam7 && just cam-check cam7 && just cam-diff cam7   # picture? config drift?
```

## 9. Doing it in Linux instead (and why this document does not lead with it)

`sysupgrade` is installed on all three and supports `--kernel=[FILE]`/`--rootfs=[FILE]`, so
the same two images can be scp'd to `/tmp` (13.8 MB of tmpfs, and the gate in §1 proves the
RAM) and applied without any console:

```bash
scp -i ~/.ssh/agent-hop-key uImage.ssc333 rootfs.squashfs.ssc333 root@10.3.2.12:/tmp/
ssh -i ~/.ssh/agent-hop-key root@10.3.2.12 'sysupgrade -h'      # THIS device's flags only
```

Two behaviours found on 2026-10-28 that this section did not know about, both harmless once
recognised: it **re-fetches itself** from the internet first (`A new version is available,
 trying to activate updated script` — v1.0.41 → v1.0.79 on a camera whose curl cannot fetch
the *manifest*, so gate 10 measures a different path than this one), and its "Stop services"
stage curls majestic's `http://127.0.0.1:80/api/v1/upgrade/prepare`, which a 2024 `majestic`
**accepts and never answers**. Normally `-m 60` bounds that; if the clock steps backwards
during the update (it did — `ntpd` set 2026, something then set 2024) the timeout never
fires and the updater hangs there forever with the camera still perfectly alive. The fix is
to kill that one child and let the parent continue.

It is not the lead option for three specific reasons. (a) The installed versions are
**1.0.41 and 1.0.21**, and upstream's current 1.0.77 — whose behaviour this document's
earlier version analysed at length — differs in ways that matter, so the flags must be read
off the device (`-h` above), never assumed from upstream. (b) It fetches from an online
repository by default, and these cameras **cannot validate a TLS certificate** (§1), so the
online path is a dead end and only `--kernel=`/`--rootfs=`/`--url=` are live. (c) Its own
per-partition size pre-check is documented upstream as **failing open** ("a size it cannot
read … must never block a flash that would have worked"), and `-f/--force_all` is
"do not validate anything"; the `sf write` arithmetic of §3 has the same property. **The
size check is ours either way.**

Both routes write the same two partitions, so both are equally safe once the sizes are
checked — the SD/U-Boot route is the one that also works when Linux no longer boots, which
is the only property that makes it worth the console.

## 10. Recovery, three levels

**L1 — Linux still boots (this works today, no console, no card).** Restore the partition
straight from the dump. This is why the dump is taken first, and it is the reason "restoring
the firmware needs soldering" is not true for this fleet:

```bash
d=$(ls -dt ~/camera-backups/cam7-flash-*)
ssh -i ~/.ssh/agent-hop-key root@10.3.2.12 'grep root /proc/mounts'   # confirm before writing
scp -i ~/.ssh/agent-hop-key "$d"/mtd[0-9]-*.bin root@10.3.2.12:/tmp/
ssh -i ~/.ssh/agent-hop-key root@10.3.2.12 'cd /tmp && \
  flashcp -v mtd2-kernel.bin /dev/mtd2 && flashcp -v mtd3-rootfs.bin /dev/mtd3'
```

`flashcp` writes kernel/rootfs. To restore the boot region or the env from Linux, write
`mtd0-boot.bin` → `/dev/mtd0` and `mtd1-env.bin` → `/dev/mtd1` the same way — and if you do,
`sync` and reboot, because you have just replaced the thing that boots the thing you
rebooted into.

**L2 — Linux does not boot, U-Boot does.** `run uknor`/`run urnor` from the card (§8) to
put the kernel and rootfs back; the env and settings were never touched. For the boot
region, note the §3 trap: `ubnor` erases `0x0`–`0x50000`, which is **boot *and* env**, so
the image must be both partitions concatenated, loaded at `${baseaddr}`:

```bash
cat "$d"/mtd0-boot.bin "$d"/mtd1-env.bin > bootenv-320k.bin     # 0x40000 + 0x10000 = 0x50000
# fatload mmc 0 0x21000000 bootenv-320k.bin ; run ubnor
```

…which is what `c110.md` §8.2 was doing with `openipc-ssc333-lite-8mb.bin` at offset `0x0`:
writing a **whole-chip** image, not a kernel.

**L3 — neither.** SPI clip (`c110.md` §8.2: CH341A + SOP8, with the 3.3 V VCC note, plus
OpenIPC's `snander`) or serial with the IPL's `sload` protocol (§8.3). The dumps restore the
whole chip byte-for-byte here; `RECONSTRUCT.txt` in each dump directory holds the `dd
conv=notrunc bs=1 seek=<offset>` lines that rebuild `chip.bin` from the five partitions, and
`SHA256SUMS` holds what each partition must hash to.

## 11. What is not known, and the test that closes it

1. **Whether these three PCBs have a card socket.** The board file describes a removable
   slot with power and card-detect GPIOs (`slot-removable`, `slot-cdzs-gpios`,
   `slot-pwr-gpios`) and the driver probes — but a DTB is a claim about a board, not
   evidence of a populated connector. **Test: `just cam-card cam7` with a FAT32 card in.**
   It triggers an MMC bus rescan (a sysfs write, no flash, no config) and reports whether a
   card answers. Two minutes, no risk, and it decides whether this document is a plan or an
   essay.
2. **Whether Enter interrupts `bootdelay=0` on *this* U-Boot.** §5. Test: console attached,
   power-cycle, spam Enter. No risk — failing means a normal boot.
3. **Whether `fatload mmc 0` on this U-Boot reads this card format.** Test: `mmc info`,
   `fatls mmc 0` at the prompt — read-only, and it is already step 1 of §8.
4. **What the installed `sysupgrade` does to the settings** (it should do nothing, per §2,
   but the version on the device is 1.0.21/1.0.41 and its split-layout code has lost
   settings on other layouts before). Test: read `/usr/sbin/sysupgrade` on cam7 (9,332
   bytes on that build — small enough to read) and diff its behaviour against §3's map.
5. **Whether the 2026 rootfs runs on a 2.3.x/2.4.x kernel+bootloader** — a 2026 rootfs with
   a 2023 kernel is a supported-looking combination upstream does not test. Test: `uknor`
   and `urnor` are separate, so flash the **rootfs only** first; the old kernel stays and
   the failure mode is a userspace that does not come up, which §10 L1 undoes in two
   minutes.
6. **cam9's sensor.** Unresolved three ways (§4). Not a blocker for cam7; a blocker for
   cam9.

## 12. Do not

- **Never `-n` / `--wipe_overlay`**, on any version: that flag's whole job is erasing the
  jffs2 settings partition — the WiFi credentials, the host keys, the pads, the config.
- **Never `--force_all` / `--force_md5` / `--force_soc` / `--force_ver`** except while
  deliberately bricking a board on purpose with a console attached and a dump in the tree.
  They are documented as "do not validate anything".
- **Never `fw_setenv`** (§7). Not to add a bootcmd, not to "fix" `wlandev`, not to change
  `rootmtd` to make an image fit. `rootmtd`/`rootsize` are how this layout says where the
  rootfs ends; a camera whose env disagrees with its kernel's `mtdparts=` is unrecoverable
  without the bench, and `docs/camera-firmware-audit-2026-10-25.md` §3 records upstream
  issue #2238 being exactly that state.
- **Never `ubnor` without a boot+env image** (§10 L2) — it erases the env.
- **Never `sf write` something you have not measured against the partition it lands in**
  (§3), and never trust a `fatload` that did not print a plausible byte count.
- **Do not flash cam9 first** (§4), and do not flash two cameras in one session: the second
  one is the control group for whatever the first one did.
- Nothing in this procedure touches `/etc` on a running camera. Keep it that way: `cam-push`
  and `cam-init` remain the only repo tools that write a device's files, and the state of
  the art on this fleet is that a write to `/etc` is permanent (§2).

## 13. Related

`cameras/README.md` (the fleet record, the reach, the faults, and the corrected storage
model), `cameras/c110.md` §8 (the original reflash route on the donor board — the IPL/
`sload`/clip detail this document does not repeat), `docs/camera-firmware-audit-2026-10-25.md`
(what was believed before the measurement, what replaced it, and the strike list of claims
that never had a source), `docs/follow-ups.md` (cam7's open admin, the PSK rotation, and the
bench session), and the recipes: `cam-preflight`, `cam-flashdump`, `cam-backup`, `cam-image`,
`cam-env`, `cam-card`.
