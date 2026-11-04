# thingino firmware from this repo: the pins, and what "deploy" means here

**2026-10-31; the build half was folded into the firmware's own repository on 2026-11-04.**
The C110 and its firmware are documented **where the firmware is**: the SSOT is
`git@github.com:cjdell/thingino-firmware.git`, branch `ssc333-infinity6b0`, **private — read
§7 before anyone touches that visibility, or assumes the baked PSKs are secret**. In that repo,
`docs/cameras/tapo-c110.md` owns the device (revisions, the flash geometry an image is built
for, the programmer install from stock, the rescue card, updating a running unit, recovery,
the outcome log of what has actually been pushed to `cam5`) and its `AGENTS.md` owns how to
build it (make goals, the container build, the SigmaStar `pack`/repack trap, the host-dependency
check, `build-container.sh`). This repo vendors no copy of either and restates neither.

What lives here is what that repository cannot know: the **pins and artifact hashes of the bytes
the fleet actually runs** (`cameras/thingino-manifest.sh`), the verbs that build, verify,
fetch, card and change over against those pins, and the rule that a payload update is a card and
not an ssh write of mtd3. [`./camera-thingino-migration-2026-10-31.md`](./camera-thingino-migration-2026-10-31.md)
owns *what a thingino camera is on this network* — endpoints, the data-partition rule, the
overlay-shadowing failure mode, per-unit state.

## 1. The pins, and the one hard dependency

| pin | value | meaning |
| --- | --- | --- |
| repo + branch | `cjdell/thingino-firmware` @ `ssc333-infinity6b0` | the SigmaStar port: the `tplink_tapo_c110_ssc333_sc3338_ssw101b` target, MIIC I2C driver fixes (without them the sensor bus never wrote a byte), ric IR-cut polarity, hush + SD-rescue u-boot, the pinned `256k/64k/1984k/5376k/512k` layout, baked Wi-Fi (Leighhack + The Lab 2.4GHz) |
| tree commit | `1286efbd3` = tag `c110-fleet-20261031` | the §3 bytes, and what `cam5`/`cam8` were flashed with (migration doc §1: kernel md5 `cb14c981…`, rootfs md5 `ce3b7ef9…`). **Reachable only by that tag now — see below** |
| buildroot | submodule at `727b904c` | restore with plain `git submodule update --init --recursive` — **never `--remote`**: the tree's own Makefile rule moves it, and this work is pinned against it |
| firmware narrative | that repo, not this one | `docs/cameras/tapo-c110.md` plus `git log c110-fleet-20261031..ssc333-infinity6b0`. Read it before changing the port, not before using these recipes |

**The pin is reachable only by its tag (measured 2026-11-04).** Recreating the SSOT as a
standalone repo (§7) replayed the branch, so `1286efbd3` is on **no** branch — `git
merge-base --is-ancestor c110-fleet-20261031 ssc333-infinity6b0` exits 1 — and the tag's tree
(`bf8e4c5f`) survives as the tree of the replayed branch commit `46602f946`. Two consequences:
`git clone -b ssc333-infinity6b0` lands on the branch tip and `cam-fw-build` correctly **refuses
it** (HEAD ≠ `FW_COMMIT`), so a fresh machine builds the fleet image from
`git -C ~/Projects/thingino-firmware checkout c110-fleet-20261031` or does not build it at all;
and the branch is **30 commits ahead of the pin — 10 of them genuinely new work** (the
speaker/audio bring-up and the `prudyntctl` shim that makes the webui audio page and Hold-to-Talk
work, the two night-latch fixes, the Wi-Fi reconnect watchdog, `docs/cameras/tapo-c110.md`
itself, and the docs of the repack/container traps); the other 20 are the replayed equivalents
of commits already in the tag's lineage, because the standalone re-creation rewrote every SHA
and `git log A..B` compares reachability, not content. That is the firmware
moving, not this file being stale — but it does make §3 a statement about the **release bytes**,
not about what is running: see §3.

**A machine joins the fleet in two commands.**
`git clone -b ssc333-infinity6b0 git@github.com:cjdell/thingino-firmware.git
~/Projects/thingino-firmware && git submodule update --init --recursive` (plain — never
`--remote`) for the tree `FW_TREE_DEFAULT` points at, and `just cam-fw-fetch` to restage the
flashed bytes of record into it and verify them against the manifest: no 40-minute build needed
to be able to write a rescue card. `cam-fw-build` was verified end-to-end from a fresh GitHub
clone on 2026-10-31 (§2), so any machine holding the repo and podman — aibox included — can
build the fleet's firmware. **The exact bytes the units flash are release assets of tag
`c110-fleet-20261031`** (with `c110-1.bin`, the stock 8 MiB dump that is the only undo button,
and the retired bench docs bundle), because a rebuild reproduces equivalents and not those bytes.

## 2. Building: `just cam-fw-build`, and what it refuses

Wraps `cameras/thingino-build.sh`: a containerised Buildroot run
(`ghcr.io/themactep/thingino-builder-image` via podman/docker), cold ~40 min, needs
~10 GB free on the tree's filesystem and a `/bin/bash` on the host (NixOS:
`sudo ln -sf "$(command -v bash)" /bin/bash`). The exact invocation it pins:

```sh
WORKFLOW=1 make -f Makefile.container container-make \
  SHELL="$(command -v bash)" \
  CAMERA=tplink_tapo_c110_ssc333_sc3338_ssw101b MAKECMDGOALS=br-all
```

Why each word of that line is there — no goal pulls the image and exits 0 having built
nothing, `container-make` is the goal that runs work, `MAKECMDGOALS=all` is poison on this
port because it reaches the Ingenic-only `uboot-dirclean` rules and dies at the *end*, and
how to repack incrementally on SigmaStar instead — is documented in **that repository's
`AGENTS.md`** ("Firmware image", "Container Builds"), next to the Makefiles it describes,
and is not repeated here.

What *is* this repo's business is the four refusals in front of the build: HEAD ≠ the
manifest commit (see §1: on a fresh clone that refusal is the correct answer, and the fix is
checking out the tag, not overriding it), a dirty tree, a buildroot submodule missing or off
`727b904c`, a short disk. Each prints its override and the honest override is "move the
manifest with your build, in the same commit". It finishes by running the verify step below,
so the recipe's last line is always a hash statement, never "make exited 0".

`just cam-fw-verify [images-dir]` runs `cameras/thingino-verify.sh` alone: bytes, md5
and sha256 for all five artifacts, the geometry out of the build's own `uenv.txt`, and
the presence of the `RESCUE.KRN` gate in its bootcmd.

**The fresh-clone verification, 2026-10-31.** A clean clone of the SSOT plus the plain
submodule init (buildroot landed on `727b904c`) plus `cam-fw-build` produced a complete image
whose post-image asserts all passed (compiled-in partition table, the 512 KB jffs2 floor) and
whose artifact set was **content-identical to the bytes the units run**: zero files added or
removed, every config byte-identical, dropbear/libcurl/uhttpd byte-identical, busybox differing
in 6 bytes and every raptor binary in 5 — the embedded `__DATE__`/`__TIME__` stamps, with the
date-stripped `strings` diffs empty. `cam-fw-verify` correctly **FAILed** that rebuild: the pin
refusing a non-reproducible build is the pin working. Not covered by it: booting rebuilt bytes
on hardware, which needs a card in a reader
(`sudo THINGINO_CARD_ALLOW_OTHER_BUILD=1 cameras/thingino-rescue-card.sh --device /dev/sdX --images <fresh-images-dir>`,
then the power cycle in §4b).

**What a rebuild does not preserve, measured the hard way 2026-10-31.** A warm incremental
build at the pinned commit finished in about a minute and produced a rootfs with md5
`f14ae863…` where the units run `ce3b7ef9…` (SquashFS embeds build timestamps, and this tree
sets no `SOURCE_DATE_EPOCH` — the open determinism item), and a u-boot container of **244,676 B
where the flashed one is 244,668** — container padding moves between builds. The conclusion
matters more than the annoyance: **the bytes cam5/cam8 flash are unique to them and to the
staged files**; nothing regenerates them. So `thingino-build.sh` refuses to write over a
non-empty images dir (it moves it to `trash/` first), and the restore path for staged bytes a
rebuild ate is this: read `mtd0` back from a unit (fleet-identical boot0, a pure read), take
`head -c` at
**source length**, reassemble `rootfs.squashfs` from the migration run's carve slices, and
rebuild the full `.bin` with post-image's exact dd layout — which hashed back to `e75c9514…` on
the day of the incident.

## 3. The artifacts (manifest values, all measured 2026-10-31)

**This table is the bytes of tag `c110-fleet-20261031` — the release `cam-fw-fetch` stages and
`cam-fw-verify` accepts. It is not a statement about what a unit is running today**: the
firmware repository is 30 commits ahead of that tag — 10 of them new work, see §1 — and its own
`docs/cameras/tapo-c110.md` outcome log records a payload pushed to `cam5` over ssh on
2026-11-02 whose artifact md5s are **not** the two below. Payload work has been happening in
that repo, which is fine — but then this table and that unit disagree, and only one of them is
the device. What a unit runs is read off the unit (`cat /etc/raptor.conf`, `uname -a`, the
mtd read-back of the migration doc §5), never out of here.

| file | bytes | md5 | what it is |
| --- | --- | --- | --- |
| `uImage` | 1,924,064 | `cb14c981…` | kernel 4.9.84 + MIIC fixes; `RESCUE.KRN` |
| `rootfs.squashfs` | 5,300,224 | `ce3b7ef9…` | squashfs root, raptor `rvd`/`rsd`, dropbear 2026.94, baked Wi-Fi; `RESCUE.RFS` |
| `u-boot-ssc333-nor.bin` | 244,668 | `7a8643b8…` | SPL+U-Boot container, **fleet-identical**, hush parser, banner `I61286efbd` — verify reads at *source length*: the file is not a round number, and hashing a `head -c` at the padded length looks like corrupt flash |
| `u-boot-env.bin` | 65,536 | `f4029011…` | the rescue env (gated bootcmd, `bootdelay`), written to mtd1 by migration phase 2 |
| `thingino-<camera>.bin` | 7,864,320 | `e75c9514…` | boot+env+kernel+rootfs up to `dataaddr` — the `sysupgrade` full-write payload; **not** an 8 MiB chip image |

The rootfs no longer fits OpenIPC's old 5,242,880 B slot: the geometry change ships with
the firmware, which is why a rollback is the u-boot procedure of the migration doc §4
and not "flash the old rootfs back".

## 4. Deploying is two different ceremonies

**There is still no "update the fleet" recipe, and nothing here reboots a camera.**
What exists is the two proven paths, each gated by the verify step, each run by a human
who reads its output.

### 4a. Changeover OpenIPC → thingino: `just cam-migrate <cam7|cam9> 1|2|all`

`cameras/thingino-migrate.sh` is a byte-identical copy (md5 `e8e4f535…`, sha256
`894cf665…`, both re-checked against the manifest before it will run) of the script
proven end-to-end on cam8 on 2026-10-31 (the rehearsal's own narrative is in the firmware
repository's docs bundle, release `c110-fleet-20261031`; and
[`./camera-thingino-migration-2026-10-31.md`](./camera-thingino-migration-2026-10-31.md)
§2 is the network-facing version). Two phases over one session:

* **Phase 1**, from the unit's OpenIPC Linux: backs up **every mtd** to the host with
  sha256 verification and refuses to continue otherwise; rewrites the env to the
  thingino geometry with a **simple-CLI bootcmd** (the stock parser has no hush and
  would execute an `if`-gated rescue line unconditionally); carves kernel+rootfs
  against the *target's* map because thingino's `rootaddr` overlaps the old kernel
  partition's tail; runs the flash writer resident in `/tmp` **through the staged musl
  loader** — busybox here is dynamically linked, so copying the binary is not
  RAM-residency and the first attempt page-faulted its own loader off a half-erased
  squashfs; **erases the data partition inside the writer** (the cam5 rule: a surviving
  OpenIPC overlay shadows `/etc` and you get "associated, no IP"); reboots.
* **Phase 2**, from the booted thingino: `flashcp`s our u-boot to mtd0 and the rescue
  env to mtd1 — the "last opening ever" — and read-back-verifies both **at source
  length**.

It authenticates with `MIG_PASS` (the script was written for sealed units and is used
as proven — the password ssh path is the rehearsal's, not a regression; fleet cameras
that are already keyed stay key-only for everything except this). The recipe resolves
the address from `common/dns-records.nix`, refuses without `MIG_PASS` and without
`GO=1`, refuses a target that is already thingino, stages backups under
`$HOME/camera-backups/`, and runs `cam-fw-verify` first. Before it: `cam-probe`, and
the **MAC** against the cameras table (the near-miss of 2026-10-31 was a labelling
error, and only MACs close labelling errors). After it: the migration doc §3's four
diagnostic commands if anything looks wrong.

### 4b. Payload update on a thingino unit: `sudo just cam-rescue-card /dev/sdX`

Writes `RESCUE.KRN` (uImage) + `RESCUE.RFS` (rootfs) to a wiped FAT32 card and proves
them **by re-reading the card** — the copy-chain md5s are what made the rehearsed rescue
trustworthy. Power-cycle the camera with the card seated: the flashed bootcmd erases
and rewrites kernel at `0x50000` and rootfs at `0x240000`, falls through, boots them;
`S98sd-rescue-clear` eats the markers inside a Linux that actually booted, so the card
is one-shot and *retryable* — a payload that dies before S98 gets rescued again on the
next power cycle, which is the correct response, not a bug.

Why a card and not ssh, when 4a crossed a network: on a **thingino** unit the root
filesystem *is* mtd3, and erasing it under the running system takes `rvd` down, whose
hardware watchdog resets the board mid-write (measured: died at 92 % of mtd3). The full
`sysupgrade` path ships and its safe paths are verified, but it is a full-chip write
with a ten-second auto-continuing countdown — a human-present decision, not a recipe.
A unit on the bench can also be cycled by the plug; an installed one needs hands
anyway, and the card makes the hands count once.

## 5. Moving a pin

A firmware rebuild moves, **in one commit**: the tree's checked-out commit, `FW_COMMIT*`,
every affected artifact line in `cameras/thingino-manifest.sh`, and §3's table here. A commit
that moves hashes without a narrative line here is the drift this whole file exists to
prevent. The migration script's hashes move only with a fresh end-to-end rehearsal — the
firmware tree first, this repo second, both copies changed together (rule 2's spirit, one
provenance).

Since 2026-11-04 a pin has to be **picked**, not inherited: the branch tip moves on its own
and the current pin is a tag on a replayed-aside line (§1), so a pin move is either `git
fetch && git checkout ssc333-infinity6b0` and pin the tip, or a deliberate decision to keep
pinning `c110-fleet-20261031` and accept that the fleet's payload may have been updated from
the other line (which is where §3's warning comes from). Either is defensible; not writing it
down is not.

## 6. Re-verify commands

```
just cam-fw-verify                              # artifacts + geometry vs manifest
just cam-fw-fetch                               # release bytes -> tree images dir -> verify
bash -n cameras/thingino-*.sh                   # the scripts themselves
md5sum cameras/thingino-migrate.sh              # vs MIGRATE_MD5 in the manifest
git -C ~/Projects/thingino-firmware describe --tags --exact-match  # the pin is a tag, not a head (§1)
git -C ~/Projects/thingino-firmware log --oneline c110-fleet-20261031..ssc333-infinity6b0  # what the fleet is behind
git -C ~/Projects/thingino-firmware submodule status buildroot     # vs BUILDROOT_PIN
gh release view c110-fleet-20261031 --repo cjdell/thingino-firmware  # the bytes of record
just cam-probe cam7                             # the OpenIPC side is still OpenIPC
```

## 7. Why the SSOT is private, and what was exposed on 2026-10-31

**Read this before changing the repo's visibility, its remote, or anything about it.**

The port commits its baked Wi-Fi credentials: `configs/cameras/tplink_tapo_c110_…/
overlay/etc/wpa_supplicant.conf` carries the **raw PMK hashes** for Leighhack and
The Lab 2.4GHz (a 64-hex `psk=` is the Pairwise Master Key itself — anyone holding it
decrypts captured traffic with only the handshake nonces, no passphrase needed). They
entered the tree in `3243628a1` and `1286efbd3`, and they are inside the shipped
rootfs, so they stay in git history forever unless someone rewrites it.

On 2026-10-31 the port was published by **forking** `themactep/thingino-firmware` —
which GitHub wires into the parent's fork network: commits pushed to the fork stay
**readable anonymously through the public parent's API by SHA, even after the fork is
made private**. That was true for roughly half an hour before the fork was deleted and
the SSOT recreated as a standalone (non-fork) private repo — the only supported way to
leave a fork network. **The replay matters: recreating the repo re-committed the same trees
under new SHAs, so the baked PMKs are in the *current* branch history of the private repo too,
not only in the deleted fork's objects** (measured 2026-11-04: the tag's tree `bf8e4c5f` is the
tree of branch commit `46602f946`, and the overlay file is present at the branch tip). Both PMKs must be treated as **known to whoever crawls public
API traffic**: rotate the PSKs on both APs, re-bake the overlay (that also moves
FW_COMMIT and every artifact, and is a changeover-sized event for cam5/cam8/cam7/cam9 —
the credentials are *baked*, §1 of the migration doc), and keep the repo private until
that rotation has happened and the history has been rewritten or knowingly accepted.
The release assets (rootfs/full-bin) contain the same baked file — the release is
private only because the repo is.

`cameras/thingino-manifest.sh` pins hashes only, the infra docs say nothing that
identifies the PSKs, and nothing here repeats their values — this is rule 13 applied
to our own commit. **Measured after the fact: deleting the fork did NOT close it** —
an hour on, anonymous by-SHA reads through `themactep/thingino-firmware` still served
the commit, its tree and the `wpa_supplicant.conf` blob (API and web). GitHub keeps a
deleted repo restorable for 90 days, and fork-network object purges are a support
action. **Needs: a support request to purge the deleted fork's objects, and rotation
regardless** — an exposure that survived deletion is an exposure that happened.
**Open item (follow-ups):** rotate + re-bake, or record the
decision not to, with the threat named.
