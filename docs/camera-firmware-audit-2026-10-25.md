# Camera firmware: what updating an OpenIPC camera here would actually take (2026-10-25)

**Asked:** update one of `cam5`/`cam7`/`cam9`, *guarantee* WiFi and SSH still work
afterwards (because restoring firmware was believed to need soldering), and see whether an
SD card is a viable unbrick.

**Answer, in short:** no update was attempted, and the reason is not that access was
missing. (1) The thing that update threatens is **the settings partition, not the boot
chain** — and on a split-NOR OpenIPC box the settings move *because* the rootfs changed
size, and upstream's own comments record a bug that lost them on **every** NOR upgrade for
a while. (2) The current build **probably does not fit the chip**, and the arithmetic lands
within ~109 KB of fitting, on a partition layout **this repo has never recorded for any of
the three**. (3) There is **no rescue userspace on this SoC family**: OpenIPC's `failsafe`
(guest OS + network + SSH after repeated boot failures) is built for Goke gk7205 only, so a
crash-looping ssc33x is a bench visit — no cable, and a console that may be under a shield.
(4) The SD question is **unanswerable from a running camera** and cheaply answerable from
the dump, which is now one command.

Nothing on a device was written. Everything below is either measured today (the command is
named) or already in this repo (the file is named), and §9 lists the claims the audit
**struck** because they could not be sourced — including numbers this session had written
into three files an hour earlier.

Legend: ✅ measured today · 📄 recorded in this repo · ❓ not established.

---

## 1. State of the fleet at the start

| | ✅/📄 | how |
| --- | --- | --- |
| All three up, RTSP `401`, HTTP `401` on cam5/cam9 | ✅ | `just cam-probe <cam>`; `curl -o /dev/null -w '%{http_code}' http://10.3.2.{10,12,17}/` |
| **cam7's web admin answers `200` unauthenticated** on `:80` and `:85` | ✅ | same, plus `curl -D-` (228-byte body, `Access-Control-Allow-Origin: *`); contradicts 📄 `cameras/README.md` (*proven*, 401 on all three) |
| None of the three is ssh-keyed | ✅ | `just cam-keytest <cam>` → `NOT keyed` |
| Keying needs a human: the vendor password is stored on no machine | 📄 | `cameras/inventory.sh` comment; `just cam-key` prompts |
| Versions: cam5/cam9 `2.4.08.22` (`master+9371591`, 2024-08-21), cam7 `2.3.11.22` (2023.02.7) | 📄 | `cameras/README.md` table (sysinfo signature) |
| SoCs: cam5/cam9 `ssc337 (infinity6b0 family)`, cam7 `ssc333`; **8 MB nor flash** on each | 📄 | same signature lines |
| `/etc/sensors/*.bin` pads: cam5+cam9 share a pad (`a1356469…`), cam7 has its own (`f9e10d2c…`); **only cam5 pins one** | 📄 | `cameras/README.md` |
| No `/dev/mmc*` while Linux runs; `/proc/partitions` = `mtdblock0…mtdblock4` | 📄 | `cameras/README.md` |

## 2. Upstream is not what `c110.md` §8 describes

✅ Downloaded and unpacked, not read about:

```
openipc.ssc337-nor-lite.tgz   7,137,552 B   sha256 56b98abe…  (build nightly-20261024-856f22e)
  uImage.ssc337               2,003,872 B   [md5 ok against the .md5sum inside the tarball]
  rootfs.squashfs.ssc337      5,136,384 B   [md5 ok]
openipc.ssc333-nor-lite.tgz   (build nightly-20261024-856f22e) uImage 2,003,856 B, rootfs 5,136,384 B
```

- The NOR tgz is **kernel + rootfs + md5s** — no IPL, no U-Boot, no `art`. §8's "each tgz
  contains an 8 MB full-disk image" and `sf erase 0x0 0x800000` are the bench route now.
- The tool is **`/usr/sbin/sysupgrade`** (`general/overlay/usr/sbin/sysupgrade`, `scr_version
  = 1.0.77`, 3,685 lines), not §8.5's `fw`. Flags that matter here: `-k`/`-r`,
  `--archive=`/`--url=`, `--channel=`, `--build=`, `--list-builds`, **`-n, --wipe_overlay`**
  ("Wipe overlay partition"), `-x, --no_reboot` ("Ignored when the update rewrites the flash
  the camera is running from"), **`-f, --force_all`** ("Do not validate anything"),
  `--no_ramfs` ("Reinstates the failure this guards against; for board bring-up only"),
  `--web` ("keep majestic alive so it can stream progress over `/ws/upgrade`").
- Images are per **SoC + build config** (`openipc.ssc33{3,7}-nor-{lite,ultimate}.tgz`;
  manifest platforms `ssc333_lite`/`ssc337_lite`), **not per board**. What is per board is
  U-Boot env — 📄 `wlandev`, IR-cut=78 / backlight=14 (`c110.md` §2) — which this path does
  not touch.
- Rollback ceiling ❗ ✅ `https://openipc.github.io/firmware/manifest.flat` (10,141 lines,
  generated 2026-10-24T19:02:58Z) lists `ssc337_lite` builds only back to
  **nightly-20260623-3e98d91**. The fleet runs 2024 builds; **upstream cannot hand back what
  these cameras run today.** The only rollback is a dump taken off the device — which is why
  `just cam-flashdump` exists.
- ✅ §8.2's in-house image source is gone: `jenkins.int.leighhack.org/job/OpenIPC-Builds/…`
  answers **502** (the Jenkins root answers 301), so the custom `openipc-ssc337` build
  cam5/cam9 appear to run has no reachable source.

## 3. What an update threatens, and it is not the boot chain

✅ Read out of `do_update_firmware_split()` in the current `sysupgrade`:

```
Split NOR layout: firmware.bin at 0x…, the kernel divides it at boot
```

The kernel's device tree declares **one** `firmware` partition spanning to the end of the
chip and splits it at boot (`mtdsplit`). There are **no fixed slots**: `rootfs_data` — the
overlay, i.e. the WiFi credentials, the dropbear host key, `authorized_keys`, every file
`cam-diff` protects — **starts where the new squashfs ends**. Changing the rootfs's size
moves the settings. So:

- ✅ *"The boot sector is what's usually at stake in a brick, and it is the one thing
  neither layout nor this script can corrupt"* — the script's own comment on the boot region
  (`mxc.dtb`/U-Boot/env live below the firmware, and `-k`/`-r` never write there). The
  clip-programmer scenario is **not** what an update threatens.
- ✅ The settings cross the boundary as a blob — `printf 'OPENIPC-OVL1%012d'` then a gzipped
  tar, padded to 64 KiB — staged **in RAM before anything is written** (`tar … | gzip`, and
  a failure there dies with *"Could not copy the settings into RAM. Nothing was written"*),
  with the settings partition released first (*"let go of it before the write, or do not
  write"*), and both writes verified: *"failed: the flash does not read back as the image."*
- ✅ **And its failure mode is exactly the one we were asked to prevent:**
  *"Writing the settings at 0x… failed; **the camera starts from defaults.**"* Defaults here
  means no `wlanpass`, no `wpa_supplicant.conf`, no host key, no `authorized_keys`: a device
  that is up on nobody's network and admitting nobody, on a WiFi VLAN, behind a password we
  do not store, on a board with no proven console.
- ✅ **This has happened before, upstream's words:** *"Reading /proc/mounts alone then took
  the camera for one without settings and staged an empty blob, which the first boot honours
  by starting from defaults — **every NOR upgrade since the hand-off (#2648)**."* That is the
  honest answer to "make sure WiFi and SSH keep working": the tool has shipped a bug whose
  symptom was precisely silent settings loss on this flash layout.

**So the guarantee you asked for cannot be made by configuration.** What *can* be done is
bound it: the snapshot must be verified to exist and to be non-empty before the write, the
dump and the `/etc` tar must be off-box first, the update must happen in a window where a
serial console can be wired if the association does not come back, and the first device must
be one we can afford to put on a bench.

## 4. Does it fit? The arithmetic, and why nobody may quote a number yet

What is 📄 known: each device is an **8 MB** NOR; on the donor unit (`c110.md` §3) every
partition before the kernel spans `0x000000–0x080000` (**524,288 B**), which includes a
128 KB `config`, and the stock `rootfs_data` there is 3,839 KB-class. What is ✅ measured:
the staged build is **7,137,552 B** of kernel+rootfs.

    8,388,608 − 7,137,552 = 1,251,056 B   for boot region + env + settings

At the donor's 524,288 B boot region that is ~22 KB of slack **before** an env partition;
add the donor's own 128 KB `config` and it is **~109 KB over a chip that has no more**.
Which is to say: it is decided by tens of kilobytes — on a layout **nobody has ever recorded
for cam5, cam7 or cam9**. There is not one partition size for these three anywhere in this
repo or its history (checked with `git log -S`), and until `just cam-preflight <cam>` has
read `/proc/mtd` on one, *any* number quoted for it is invented.

Two things make "just try it and see" wrong rather than merely careless:

- ✅ the per-partition pre-check is commented **as failing open** — *"Fails OPEN. A size it
  cannot read … must never block a flash that would have worked"* — with the reason in
  upstream's history: before **#2426** the split path never looked at `flashcp`'s exit
  status, so a run that wrote **nothing** printed *"RootFS updated to <exactly the version
  you asked for>"* and rebooted (**#2238**: a camera whose rootfs partition had been resized
  in the U-Boot env without `/proc/cmdline` following — the exact state a hand-laid 8 MB
  table reaches by accident). The split path *does* have a genuine pre-write check
  (*"The new firmware and the settings do not fit the flash … Nothing was written"*), and
  ✅ `-f, --force_all` is documented as *"Do not validate anything"*.
- the fix for "it does not fit" is a **re-partition**, and a re-partition is §8's U-Boot /
  clip route. The update was supposed to avoid the bench; on these numbers it walks into it.

## 5. The sensor: a healthy-looking camera with no picture

📄 Only cam5 pins its pad (`isp:` → `sensorConfig: /etc/sensors/sc3338.bin`). cam7 and cam9
have **no `isp:` section** while `/etc/sensors/` on them holds more than one pad, and
`docs/follow-ups.md` records cam9 as `sc401ai` while **cam9's own sysinfo line reads
`sc3338`** — an unresolved conflict in this repo. A new rootfs ships a new loader and a new
default pad selection; what that selection resolves to on an unpinned device is **not
recorded anywhere**. The failure shape is already known from the other direction: this fleet
has logged `Sensor panic!` reboots. Net: **a camera can return with WiFi and SSH fine and no
video**, and only `cam-diff` afterwards would say why. That is a second, independent reason
cam9 goes last — and the reason cam5 is the *worst* first device despite looking the cleanest
(it is the only one with a pin, and the only one whose pad is shared with cam9).

## 6. Unbrick inventory

| level | state | what rescues it | status here |
| --- | --- | --- | --- |
| L1 | kernel boots, userspace crash-loops | **`failsafe-rescue`**: guest OS, network, RAM-only sshd, **overlay mounted** so `/root/.ssh` works, login banner names the failing unit | ❌ **not available**: ✅ `Config.in` — *"SoC families whose U-Boot keeps a boot counter in a reserved DRAM word … (currently Goke gk7205v200 and gk7205v300)"*; the package is *"kept out of the shared overlay so it does not ship to … families that do not reserve it"*. Nothing on ssc33x is `select`ing it |
| L2 | U-Boot alive, Linux unbootable | serial console → `sf` over TFTP, or boot from SD and `flashcp` | ✅ console proven on this silicon (`c110.md` §8.3): **3.3 V logic, 115200 8N1**, pads near `TP501`, IPL banner `IPL g77fc04e`; ❓ reachable on *our* units — §2 records the donor's pads solder-jumpered under a shield |
| L3 | bootloader/env gone | SPI clip (`c110.md` §8.2: CH341A + SOP8, **3.3 V VCC mod**, OpenIPC `snander`) | ✅ documented; ❓ never needed here; and the file's own caveat: a clip can read a chip it cannot boot |

**SD as an unbrick: unanswerable, and cheap to make answerable.** ✅ From Linux you can only
learn there is no `/dev/mmc*` node — which is about the running kernel's drivers, not about
what IPL/U-Boot can boot. 📄 The card *is* already in this hardware's install story: `c110.md`
§8.1–8.3 use a FAT32 card (≤32 GB) for staging and flash **from U-Boot**, §8.4's customizer
takes `wlanssid`/`wlanpass` off a card (so a card can re-provision the radios *if* the board
boots from one), and stock had `etc/init.d/check_upgrade` (START=13) for SD recovery — gone
with the TP-Link userspace. ❓ Not established: whether these three PCBs have slots (cam9
reports no board line at all), whether the card must carry the vendor first-stage layout,
whether the IPL on these units will boot anything but SPI-NOR, and what the env's SD init
parameters are on *our* units. **The cheap sequence:** `cam-flashdump` → read the IPL/U-Boot
in the dump (banner + `help`) → try a card on a **spare** board, never a wired-in one.

**The unbrick that is payable in advance is the dump.** `just cam-flashdump <cam>` reads
every MTD partition, size-checks each against sysfs and md5-checks it against the device,
and writes `RECONSTRUCT.txt` with the `dd seek=` lines to rebuild the chip image. With that,
the chip never has to be *read* again — only written, by whichever of L2/L3 turns out to
exist. Roughly a minute of WiFi, ~8 MB of disk, and the one thing that makes "restoring
needs soldering" untrue. It matters more here than it looks: these three **arrived already
flashed**, with their own per-device calibration, and only cam7's pad files were ever pulled
into the repo — `c110.md` §9's "keep `c110-1.bin` forever" undo button exists for none of
them (that file is not on this machine either; searched).

## 7. What was added, and what it caught

Four recipes (`justfile`) and two device-side scripts, **all read-only on the device**;
`cam-push`/`cam-init` remain the only things that write a camera:

- **`just cam-preflight <cam>`** + `cameras/preupdate-check.sh` — ten gates: kernel/rootfs
  partitions nameable; staged image fits; RAM to pivot into; `/tmp` tmpfs?; **overlay
  flash-backed?**; WiFi credentials present; host key + `authorized_keys` present; `awk`
  present (✅ the current `sysupgrade` uses `awk` 32 times, and 📄 `awk` is **absent on all
  three devices** — so that updater cannot validate or size anything on this firmware, which
  is itself a finding: an in-place update here is *not* a run of the current tool); an
  updater exists; the fetch path works. Reports credentials by path/size/md5 only.
- **`just cam-flashdump <cam>`** / **`just cam-backup <cam>`** + `cameras/backup-etc.sh` —
  the chip and the `/etc`+`/root` tar, both into mode-700 dirs under `$HOME/camera-backups`,
  never into the repo (`cam-pull` puts files in a store path; the PSK and host keys must not
  go there — `AGENTS.md` rule 13). Both clean up after themselves: a flashdump that died
  before `mtd-verified.txt` removes its bytes, because a partial dump looks like a dump.
- **`just cam-image <soc>`** — stages + verifies a build into `$HOME/camera-images`
  (deliberately **outside** the flake source: a tarball in the repo is a copy cost on every
  evaluation, `docs/flake-simplification-pass-2026-10-24.md` §1) and prints the
  `IMG_KERNEL=… IMG_ROOTFS=… just cam-preflight <cam>` line so the fit gate measures the
  build actually staged instead of a remembered size.
- **No update recipe.** A recipe that reboots a WiFi camera cannot promise the association
  and the keys come back, and §3 says the tool itself has broken that promise before.

Tests actually run: `just check-eval` green (assertions, `check-doc-pointers`, `fmt-check`);
`bash -n` on every recipe body; `sh -n` on both device scripts; the hex/`/proc/mtd` parser
unit-tested against a synthetic table (including `rootfs_data` **not** being chosen as
`rootfs`); both backup recipes exercised live against an unkeyed device so the failure path
is a clean exit 255 and no orphan directory; `cam-image` run for real for both SoCs.
**The smoke test found a bug in the pre-flight I had just written**: the facts were appended
to a variable that was never printed, so half the report silently vanished — a check that
prints nothing reads as a check that saw nothing.

## 8. Claims this audit struck, and the lesson

The following were written into `cameras/README.md`, `cameras/c110.md` and
`docs/follow-ups.md` earlier on 2026-10-25 and **removed before commit**, because `grep`
across the working tree and `git log -S` across history found no source for them:

- per-device MTD partition sizes (e.g. "cam5's rootfs partition is 3,178,496 B") — **no
  partition size for any of these three has ever been recorded**; §4 is the honest version;
- `mmc_host`/`msys_pstore`/`mmc_rescan` observations — the only recorded MMC fact is "no
  `/dev/mmc*` node while running";
- an `MMC boot mode detected (boot_from = …)` message — not in `sysupgrade`;
- `openipc.*-mmc-*.boot.zip` and `nor-dump-*-with-sd-loader.zip` release assets — that
  release has 147 assets and none of them match `mmc`, `boot` or `nor-dump`;
- "autodetect by flash ID" / `Sensor mismatch. init failed!` / four-byte pad names as 📄 —
  none of that is in `c110.md`; §5 keeps only what is;
- "1.8 V" for the console — 📄 it is **3.3 V logic**;
- an SD init string quoted as if read from a device — no env of ours has ever been dumped.

**Lesson, and it is the same one `AGENTS.md` keeps restating in new clothes:** a number in a
camera doc must name the command that produced it, and "the tool said it" is not a source.
This fleet is documented in the exact style that makes that checkable — ✅ against ❓, each
with its command — and the failure mode of an agent working in it is not inventing a wrong
fact, it is writing a *plausible* fact in the right format, where it reads better than the
truth and survives review. The strike list above is the cheapest part of this audit to keep:
it is the list of things that would otherwise have become the repo's own citations.

## 9. If someone does this properly

1. `just cam-key <cam>` — the operator, at a terminal (the password is on no machine).
2. `just cam-preflight <cam>` → fix every FAIL by measurement; the overlay gate and the
   `awk` gate are the interesting ones.
3. `just cam-flashdump <cam>` and `just cam-backup <cam>`, and **verify the tar opens**.
4. Decide the sensor: resolve `sc401ai` vs the sysinfo `sc3338` for cam9, and pin a pad on
   the test device *before* the update so there is something to compare against.
5. Read `sysupgrade`'s write path **on the device**, not in the repo: which version the
   2.4.08.22 build ships (if it ships `fw` instead, the entire §2/§3 analysis is about a tool
   that is not installed, and that is a fact worth having before, not after).
6. Only then: a device nobody is wired into, in a window, `--archive=` with the staged
   `--build=`, **never `-n`**, never `-f`, `cam-probe` before and after, `cam-diff` after,
   and the serial adapter on the desk — because §6 says nothing else is waiting if it
   crash-loops.

Order when it happens: **cam7 first** (most room, ssc333, and the least load-bearing feed),
**cam5 second**, **cam9 last or not at all** until its pad is pinned. cam7's open admin
(§1) must be closed first regardless: it is an unauthenticated upgrade button on the device
most likely to be picked as the guinea pig.

## 10. What this changes in the other docs

- `cameras/README.md` — new *Firmware updates* section (the settings-not-boot-chain model,
  the arithmetic, the struck SD/failsafe facts, the order of devices, the pointer here).
- `cameras/c110.md` — §8 carries a dated supersession block (image contents, `fw` →
  `sysupgrade`, dead jenkins artifact); §9 records that its own "undo button" applies to
  none of our three units.
- `docs/follow-ups.md` — the update blocker, cam7's unauthenticated admin, and the
  `custom.ok` self-contradiction (recorded as size 0 in one place and "absent" in another,
  in the same list).
- `justfile`, `cameras/preupdate-check.sh`, `cameras/backup-etc.sh` — §7.
- `docs/work-log-2026-10.md` — the lesson in one paragraph.
