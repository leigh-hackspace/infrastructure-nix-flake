# thingino firmware from this repo: building it, and what "deploy" means

**2026-10-31.** [`./camera-thingino-migration-2026-10-31.md`](./camera-thingino-migration-2026-10-31.md)
owns *what a thingino camera is* — endpoints, geometry, the data-partition rule, the
failure modes. This file owns the two verbs the fleet needed the day that port became
production firmware: **build the image** and **get it onto a camera**, from this repo and
with this repo's checks. The build tree itself is the operator's bench tree
(`~/Projects/tapo/thingino-c110`, a `themactep/thingino-firmware` clone on branch
`ssc333-infinity6b0`); this repo deliberately vendors no copy of it — it vendors the
*pins and hashes* (`cameras/thingino-manifest.sh`) so a build can be refused when it is
not the build the flashed units are running.

## 1. The pins, and the one hard dependency

| pin | value | meaning |
| --- | --- | --- |
| branch | `ssc333-infinity6b0` | the SigmaStar port: new `tplink_tapo_c110_ssc333_sc3338_ssw101b` target, MIIC I2C driver fixes (without them the sensor bus never wrote a byte — bench §5q), ric IR-cut polarity, hush + SD-rescue u-boot, pinned `256k/64k/1984k/5376k/512k` layout, baked Wi-Fi (Leighhack + The Lab 2.4GHz) |
| tree commit | `1286efbd3` | what `cam5` and `cam8` run today, per the migration doc §1 table (kernel md5 `cb14c981…`, rootfs md5 `ce3b7ef9…`) |
| buildroot | submodule pinned at `727b904c` | restore with plain `git submodule update --init --recursive` — **never `--remote`**: the fork's Makefile rule moves it, and the work is pinned against it |
| bench narrative | `~/Projects/tapo/docs/thingino-c110-execution.md` | hashes, failure stories, §5k (rescue design) and §7d (migration rehearsal) — read it before changing the port, not before using these recipes |

**The branch lives on the bench only.** It is not pushed to `themactep/thingino-firmware`
or to the `johnchia` fork the tree also carries as a remote, so `cam-fw-build` works on
a machine that has the tree — today, the bench. Filing in `docs/follow-ups.md`: push the
branch (or land the upstream PRs), which is what lets aibox or a second box build the
fleet's firmware at all.

## 2. Building: `just cam-fw-build`

Wraps `cameras/thingino-build.sh`: a containerised Buildroot run
(`ghcr.io/themactep/thingino-builder-image` via podman/docker), cold ~40 min, needs
~10 GB free on the tree's filesystem and a `/bin/bash` on the host (NixOS:
`sudo ln -sf "$(command -v bash)" /bin/bash`). The exact invocation it pins:

```sh
WORKFLOW=1 make -f Makefile.container container-make \
  SHELL="$(command -v bash)" \
  CAMERA=tplink_tapo_c110_ssc333_sc3338_ssw101b MAKECMDGOALS=br-all
```

Three traps make that line longer than it looks (receipts in the bench tree's
`AGENTS.md` tips 11–12 and §5h): with **no goal** `Makefile.container` pulls the builder
image and exits 0 having built nothing; `container-make` is the goal that actually runs
work — look for `Running make … in container` in the log as proof; and
**`MAKECMDGOALS=all` is poison on this port** — it reaches Ingenic-only `uboot-dirclean`
rules and dies at the *end* of a long build. `br-all` is the SigmaStar-correct goal.

The script refuses before building when HEAD ≠ the manifest commit, the tree is dirty,
the buildroot submodule is missing or off its pin, or the disk is short — each refusal
prints its override, and the honest override is "move the manifest with your build, in
the same commit". It finishes by running the verify step below, so the recipe's last
line is always a hash statement, never "make exited 0" (a green build with a stale
artifact inside is bench tip 13, and it has bitten this fleet's neighbour twice).

`just cam-fw-verify [images-dir]` runs `cameras/thingino-verify.sh` alone: bytes, md5
and sha256 for all five artifacts, the geometry out of the build's own `uenv.txt`, and
the presence of the `RESCUE.KRN` gate in its bootcmd.

**What a rebuild does not preserve, measured the hard way 2026-10-31.** A warm
incremental `cam-fw-build` at the pinned commit finished in about a minute and produced
a rootfs with md5 `f14ae863…` where the units run `ce3b7ef9…` (SquashFS embeds build
timestamps — bench tip 7, and `SOURCE_DATE_EPOCH` is the open §7c-5 item), and a u-boot
container of **244,676 B where the flashed one is 244,668** — container padding moves
between builds, which is the same arithmetic trap tip 22 hit from the read-back side.
The conclusion matters more than the annoyance: **the bytes cam5/cam8 flash are unique
to the units and to the staged files**; nothing regenerates them. So `thingino-build.sh`
refuses to write over a non-empty images dir (it moves it to `trash/` first), and the
restore path for staged bytes that a rebuild ate is: read `mtd0` back from a unit
(fleet-identical boot0 — pure read, no flash hazard), take `head -c` at **source
length**, reassemble `rootfs.squashfs` from the migration run's carve slices
(`carve-IMG-K` tail + `carve-IMG-R`), and rebuild the full `.bin` with post-image's
exact dd layout — which hashed back to `e75c9514…` on the day of the incident and is
how the bench's images dir is trustworthy again. Until determinism lands,
`cam-fw-verify` failing after a rebuild is the pin working, not the build broken.

## 3. The artifacts (manifest values, all measured 2026-10-31)

| file | bytes | md5 | what it is |
| --- | --- | --- | --- |
| `uImage` | 1,924,064 | `cb14c981…` | kernel 4.9.84 + MIIC fixes; `RESCUE.KRN` |
| `rootfs.squashfs` | 5,300,224 | `ce3b7ef9…` | squashfs root, raptor `rvd`/`rsd`, dropbear 2026.94, baked Wi-Fi; `RESCUE.RFS` |
| `u-boot-ssc333-nor.bin` | 244,668 | `7a8643b8…` | SPL+U-Boot container, **fleet-identical**, hush parser, banner `I61286efbd` — verify reads at *source length*, the file is not a round number (tip 22) |
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
proven end-to-end on cam8 on 2026-10-31 (bench §7d, and
[`./camera-thingino-migration-2026-10-31.md`](./camera-thingino-migration-2026-10-31.md)
§2 for the network-facing narrative). Two phases over one session:

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
them **by re-reading the card** — the copy-chain md5s are what made §7d's rescue
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

A firmware rebuild moves, **in one commit**: the tree's HEAD, `FW_COMMIT*`, every
affected artifact line in `cameras/thingino-manifest.sh`, and §3's table here. A commit
that moves hashes without a narrative line here is the drift this whole file exists to
prevent. The migration script's hashes move only with a fresh end-to-end rehearsal —
bench first, this repo second, both copies changed together (rule 2's spirit, one
provenance).

## 6. Re-verify commands

```
just cam-fw-verify                              # artifacts + geometry vs manifest
bash -n cameras/thingino-*.sh                   # the scripts themselves
md5sum cameras/thingino-migrate.sh              # vs MIGRATE_MD5 in the manifest
git -C ~/Projects/tapo/thingino-c110 rev-parse HEAD    # vs FW_COMMIT
just cam-probe cam7                             # the OpenIPC side is still OpenIPC
```
