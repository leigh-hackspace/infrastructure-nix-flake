#!/usr/bin/env bash
# thingino-rescue-card.sh — write the one FAT32 card the flashed thingino units
# take as their payload update: RESCUE.KRN + RESCUE.RFS at the root of the card.
#
#   usage: sudo cameras/thingino-rescue-card.sh --device /dev/sdX [--images DIR] [--yes]
#
# How the mechanism works (proven on cam8 2026-10-29 through 2026-10-31, four
# live rescues — bench execution log §5k and §7d item 6, and the bootcmd is
# checked by cam-fw-verify): the flashed u-boot's bootcmd starts with
# `if fatload mmc 0 … RESCUE.KRN; then sf erase/write ${kernaddr}; if fatload …
# RESCUE.RFS; then sf erase/write ${rootaddr}; fi; fi` and then **falls through**
# to the normal `sf read; bootm`.  So a card present at power-on rewrites kernel
# and rootfs and boots them; S98sd-rescue-clear deletes the markers only inside
# a Linux that actually booted, which makes the card one-shot *and* retryable —
# a payload that dies before S98 is rescued again on the next power cycle.
# Nothing else on the card is read, and uenv.txt deliberately does not go on it
# (that is a provisioning file; the rescue flow never rewrites the env).
#
# Why this is the payload path at all: the installed units have no serial and
# thingino has no trusted network writer — writing mtd3 from a Linux whose root
# is mtd3 is the watchdog-reset that killed the first restore attempt at 92 %
# (bench §7d item 3).  A card plus a power cycle is the boring proven way.
#
# This DESTROYS the card.  It never touches a camera by itself.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=thingino-manifest.sh
. "$here/thingino-manifest.sh"

DEV=""; ASSUME_YES=0; IMAGES="${THINGINO_TREE:-$FW_TREE_DEFAULT}/${FW_IMAGES_SUBDIR}"
while [ $# -gt 0 ]; do case $1 in
    --device) DEV=$2; shift 2;;
    --images) IMAGES=$2; shift 2;;
    --yes) ASSUME_YES=1; shift;;
    *) echo "usage: sudo $0 --device /dev/sdX [--images DIR] [--yes]" >&2; exit 2;;
esac; done

[ "$(id -u)" = 0 ] || { echo "run under sudo: this repartitions and formats a block device." >&2; exit 1; }
[ -n "$DEV" ] || { echo "refusing to guess a card device: pass --device /dev/sdX" >&2; exit 2; }
[ -b "$DEV" ] || { echo "$DEV is not a block device" >&2; exit 1; }

# A card reader holds what the bootcmd calls mmc 0 inside the camera; on the host
# it is whatever /dev/sdX the reader answers.  Refuse the disk / lives on, and
# refuse a device with partitions mounted — everything else is the operator's.
rootdisk=$(lsblk -no PKNAME "$(findmnt -no SOURCE /)" 2>/dev/null || true)
[ -n "$rootdisk" ] && [ "$(basename "$DEV")" = "$rootdisk" ] && {
    echo "$DEV is the disk / lives on. No." >&2; exit 1; }
mounted=$(lsblk -rno NAME,MOUNTPOINT "$DEV" 2>/dev/null | awk 'NR>1 && $2 != "" {print $1}')
[ -z "$mounted" ] || { echo "$DEV has mounted partitions ($mounted); unmount them first" >&2; exit 1; }

echo "=== card write target — read this before answering ==="
lsblk -o NAME,SIZE,TYPE,MODEL,SERIAL,TRAN "$DEV" || lsblk "$DEV"
echo "The WHOLE device loses its partition table and FAT volume. The proven bench"
echo "rescue card was a 58 GB card, so size is not the check — the device name is."
if [ "$ASSUME_YES" != 1 ]; then
    read -r -p "type the device name to continue ($(basename "$DEV")): " ans
    [ "$ans" = "$(basename "$DEV")" ] || { echo "not typed; nothing written." >&2; exit 1; }
fi

# Never write a card from an unverified build: the whole value of the rescue flow
# is that md5s match through the copy chain, and it starts at the source files.
"$here/thingino-verify.sh" "$IMAGES" >/dev/null
md5_k=$(printf '%s\n' "$ARTIFACTS" | awk -F'|' '$1 == "uImage" {print $3}')
md5_r=$(printf '%s\n' "$ARTIFACTS" | awk -F'|' '$1 == "rootfs.squashfs" {print $3}')

echo "=== repartitioning $DEV (single FAT32 LBA partition, type 0C, starts at 4 MiB)"
wipefs -a "$DEV" >/dev/null
sfdisk -q "$DEV" <<EOF
label: dos
start=8192, type=c, bootable
EOF
partprobe "$DEV" 2>/dev/null || true
# Partition naming: SCSI-style disks take a bare digit (/dev/sdb1 — what a card
# reader answers), everything else takes the p suffix (mmcblk0p1, nvme0n1p1).
case "$DEV" in
    /dev/[shv]d[a-z]) PART="${DEV}1" ;;
    *) PART="${DEV}p1" ;;
esac
tries=0
while [ ! -b "$PART" ] && [ "$tries" -lt 20 ]; do sleep 0.5; partprobe "$DEV" 2>/dev/null || true; tries=$((tries + 1)); done
[ -b "$PART" ] || { echo "partition $PART never appeared (try another reader?)" >&2; exit 1; }
mkfs.fat -F 32 -n THINGINO "$PART" >/dev/null

echo "=== staging RESCUE.KRN (uImage) and RESCUE.RFS (rootfs.squashfs)"
mnt=$(mktemp -d)
mount "$PART" "$mnt"
cp "$IMAGES/uImage" "$mnt/RESCUE.KRN"
cp "$IMAGES/rootfs.squashfs" "$mnt/RESCUE.RFS"
sync
umount "$mnt"

# Re-read through the filesystem, not the page cache: this is the step §7d proved
# ("md5 cb14c981…/ce3b7ef9… verified through the copy chain"), and dropping it
# makes the recipe print success for a card a flaky reader never really wrote.
mount -o ro "$PART" "$mnt"
got_k=$(md5sum "$mnt/RESCUE.KRN" | cut -d' ' -f1)
got_r=$(md5sum "$mnt/RESCUE.RFS" | cut -d' ' -f1)
umount "$mnt"; rmdir "$mnt"
if [ "$got_k" != "$md5_k" ] || [ "$got_r" != "$md5_r" ]; then
    echo "CARD REJECTED on re-read: KRN $got_k (want $md5_k), RFS $got_r (want $md5_r)." >&2
    echo "The card or reader is lying; write nothing to a camera with it." >&2
    exit 1
fi

cat <<EOF

Card written and verified: RESCUE.KRN $got_k, RESCUE.RFS $got_r — exactly the
manifest payloads (FW_COMMIT $FW_COMMIT_SHORT).

To flash a thingino unit with it:
  1. identify the unit by MAC against the cameras/README.md table, not by name;
  2. power the camera off, seat the card, power on. With serial you will see
     "reading RESCUE.KRN", erase+write at 0x50000, then RESCUE.RFS at 0x240000
     (erase ~13 s), then "Starting kernel"; expect ~40 s before ssh answers;
  3. the boot that was rescued deletes both markers (S98sd-rescue-clear), so the
     next power cycle is a normal boot. To *prove* the one-shot, cycle once more
     with the card still in: the log must show the failed "reading RESCUE.KRN"
     and boot anyway.
  This rewrites kernel+rootfs ONLY. The data partition is untouched — which is
  correct for an update and fatal for a changeover (wipe it; the migration flow
  does — docs/camera-thingino-migration-2026-10-31.md §2).
EOF
