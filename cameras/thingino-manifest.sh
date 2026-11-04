# DATA — pins and hashes for the thingino SigmaStar port: the one table the build,
# verify and rescue-card scripts read.  Nothing below is a guess: every hash is
# `md5sum`/`sha256sum` of the artifact as built from FW_COMMIT on 2026-10-31, cross-read
# against the flashed state of cam5/cam8 as recorded in
# docs/camera-thingino-migration-2026-10-31.md §1.  The firmware's own narrative is in
# the firmware's own repository (SSOT below): `docs/cameras/tapo-c110.md` for the device
# and the flashing procedure, its `AGENTS.md` for the build, and the retired bench
# execution log preserved as `tapo-bench-docs.tar.gz` in release `c110-fleet-20261031`.
# This file pins; it does not document.
#
# What a hash is FOR: `just cam-fw-verify` refuses to stage an image the fleet has
# never run, `cam-migrate` refuses to flash what it cannot verify, and the rescue
# card re-reads the two payload files back off the card after writing them.  A
# rebuilt firmware changes FW_COMMIT and every artifact hash together — the pin
# table is where a "works on my bench" build becomes a deployable one, so a commit
# that touches firmware source moves FW_COMMIT in the same commit that moves
# hashes here, and the story goes into docs/camera-thingino-build-deploy-2026-10-31.md.
#
# The exact bytes hashed below are NOT reproducible from this tree (timestamps in
# the squashfs, in the raptor/busybox binaries' 5-byte __DATE__ strings, and in the
# kernel's compressed build stamp — docs §2), so they are ALSO preserved as assets
# of FW_RELEASE: that release is the artifact of record, and the bench tree and any
# fresh clone are ways to *rebuild equivalents*, not these bytes.

# --- the build tree -----------------------------------------------------------
# SSOT: cjdell/thingino-firmware (private), branch ssc333-infinity6b0.  PRIVATE ON
# PURPOSE: the overlay commits the baked Wi-Fi PMK hashes, and they stay in git
# history — see docs/camera-thingino-build-deploy-2026-10-31.md §7 before ever
# flipping this public (and rotate both PSKs before that day).
FW_GIT_URL="git@github.com:cjdell/thingino-firmware.git"
FW_BRANCH="ssc333-infinity6b0"
# The pin survives only as its tag: recreating the SSOT as a standalone repo (§7 of the
# build-deploy doc) replayed the branch, so `1286efbd3` is on no branch and a fresh
# `git clone -b ssc333-infinity6b0` lands past it — `thingino-build.sh` then refuses, as it
# should.  Build the fleet image from `git checkout c110-fleet-20261031`; the branch tip is a
# different image and needs its own pin move.  Measured 2026-11-04: `git log
# c110-fleet-20261031..ssc333-infinity6b0` counts 30 commits, 20 of them replays of work already
# in the tag's lineage and 10 genuinely new (audio, night latch, wifi watchdog, its own
# tapo-c110.md guide), and the firmware repo's own outcome log records a payload pushed to cam5
# on 2026-11-02 whose artifact md5s are NOT the two below.  So ARTIFACTS is the bytes of record,
# not a read of any unit; what a unit runs is read off the unit.
FW_COMMIT="1286efbd3b5a348ea49f27fba8dea7b62e71e222"          # "c110 overlay: bake Leighhack as a second Wi-Fi network"
FW_COMMIT_SHORT="1286efbd3"
FW_REPO="cjdell/thingino-firmware"
# The bench scratchpad that grew the port is retired; the fleet's checkout of the
# SSOT lives outside it.  THINGINO_TREE overrides per invocation.
FW_TREE_DEFAULT="$HOME/Projects/thingino-firmware"
FW_CAMERA="tplink_tapo_c110_ssc333_sc3338_ssw101b"
FW_IMAGES_SUBDIR="output/ssc333-infinity6b0/${FW_CAMERA}-4.9-musl/images"
# The artifact bytes live on live units are preserved as release assets:
FW_RELEASE="c110-fleet-20261031"
FW_RELEASE_URL="https://github.com/cjdell/thingino-firmware/releases/tag/${FW_RELEASE}"

# Buildroot is a submodule pinned by the tree, not by this file; the pin commit is
# what `git submodule update --init --recursive` must land on.  NEVER `--remote`
# (the fork's Makefile rule runs `git submodule update --remote` and the C110 work
# deliberately pins against it — bench AGENTS tip 4).
BUILDROOT_PIN="727b904c13bbc2a9a7a530b1f8e47d7d180c2768"

# --- the flash geometry the image is built for --------------------------------
# From the build's own uenv.txt (not retyped from prose); the layout is
# 256k(boot),64k(env),1984k(kernel),5376k(rootfs),512k(data) on the 8 MiB chip.
# data < 512 KB is a boot-looping jffs2 unit (four erase blocks is not five —
# bench AGENTS tip 21), and post-image.sh in the tree hard-errors below it.
GEOM_KERNADDR=0x50000
GEOM_KERNSIZE=0x1f0000
GEOM_ROOTADDR=0x240000
GEOM_ROOTSIZE=0x540000
GEOM_DATAADDR=0x780000
GEOM_DATASIZE=0x80000

# --- artifacts, as built from FW_COMMIT on 2026-10-31 --------------------------
# "filename|bytes|md5|sha256" — verify.sh checks all three per line and says which
# one disagreed; the rescue-card and migrate flows refuse on any FAIL.
#
# uImage      -> kernel (4.9.84 + the Infinity6B0 MIIC I2C fixes, without which the
#              sensor bus never writes — the port's MIIC fix, `git log` of the SSOT names it)
# rootfs      -> squashfs; bakes Wi-Fi for Leighhack + The Lab 2.4GHz (the U-Boot
#              env credential channel is OpenIPC-only), raptor rvd/rsd, dropbear
# u-boot      -> SPL+U-Boot container, fleet-identical (sha256 c10ea860… on every
#              dump read), carries the hush parser + the SD-rescue bootcmd once
#              u-boot-env.bin is in flash with it
# u-boot-env  -> the rescue env (RESCUE.KRN/RESCUE.RFS gate, bootdelay 3 on bench)
# full bin    -> boot+env+kernel+rootfs, 7,864,320 B = up to the data partition,
#              the sysupgrade full-write payload; NOT an 8 MiB chip image
ARTIFACTS="
uImage|1924064|cb14c981d97ffcc465949d8a44f93a5c|e2aa122b9fb5df9d4a10dd8c65d299abef65d713d1e00c8f127736a64349c208
rootfs.squashfs|5300224|ce3b7ef995453e757e851e0cbc192fd1|a5e0cc0c452dd116ea4eb9c1f7d9bc834d653e1d73d49ffe0da87443a43ebddc
u-boot-ssc333-nor.bin|244668|7a8643b8d0e895b58f4f7663e629d345|c10ea8609dc09550a5605a984b3f644272e3b32329786dd020344dfecf29d212
u-boot-env.bin|65536|f40290110d26970d7efc060090031fd2|b536662d905e23fec033458722015b0e6494ffc24c3b382c7c61834378a9f112
thingino-${FW_CAMERA}.bin|7864320|e75c95145d3a57322c7aab61f4252be0|933a2fae513c609b317ff55b24e70f19e3a2a1ee8c9da2e7d6a868c293265ed9
"

# --- the migration tool --------------------------------------------------------
# cameras/thingino-migrate.sh is a byte-identical copy of the script proven on
# cam8 2026-10-31 (rehearsal narrative: the firmware repo's docs bundle, release
# `c110-fleet-20261031`): two phases over one ssh session, sha-verified backup of every mtd first,
# data partition wiped inside the writer by construction.  cam-migrate re-checks
# this hash before it lets the script near a camera — the provenance IS the
# evidence, so a hand-edit here or there must show up as a FAIL, and the fix is a
# fresh rehearsal plus both hashes moving together.
MIGRATE_BYTES=14431
MIGRATE_MD5="e8e4f5351babecef37f85f19bdc9a91b"
MIGRATE_SHA256="894cf665b4f8aa1a4c492ee981fb5b8d1c967758dc9d48b65a8081361ea9361c"
