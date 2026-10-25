#!/bin/sh
# Does a card slot physically exist on this board?  Piped to `sh -s` by `just cam-card`.
#
# Why this needs its own probe: the answer is not in the file system.  The host driver
# probes whether or not a card is present (`mmc0` exists, major 179 in /proc/devices,
# dmesg says `[sdmmc] ms_sdmmc_probe`), and there is simply no block device until a card
# answers.  So the only test is: insert a card, rescan the bus, look for a child.
#
# What this touches: a write to `/sys/class/mmc_host/mmc*/rescan`, which asks the kernel to
# re-enumerate the MMC bus.  It writes nothing to flash, nothing to /etc, and it needs no
# read-write root filesystem — which matters on this fleet, where `/` is an overlay whose
# upper half is jffs2 on the settings partition, so a stray write *persists* (contrary to
# what this directory used to claim).
#
# busybox-ash POSIX only.  Every wait is bounded.

echo "== hosts"
ls -l /sys/class/mmc_host 2>/dev/null | tail -n +2
echo "== before rescan: children"
for h in /sys/class/mmc_host/mmc[0-9]*; do
	[ -d "$h" ] || continue
	echo "  $(basename "$h"): $(ls "$h" 2>/dev/null | tr '\n' ' ')"
done

echo "== rescan"
for h in /sys/class/mmc_host/mmc[0-9]*; do
	[ -d "$h" ] || continue
	if [ -w "$h/rescan" ]; then
		echo 1 >"$h/rescan" 2>/dev/null && echo "  $(basename "$h"): rescan triggered" ||
			echo "  $(basename "$h"): rescan refused"
	else
		echo "  $(basename "$h"): no writable rescan attribute"
	fi
done
# One bounded wait for the card to enumerate; a rescan is asynchronous.
i=0
while [ "$i" -lt 6 ]; do
	ls /dev/mmc* >/dev/null 2>&1 && break
	i=$((i + 1))
	sleep 1 2>/dev/null || :
done

echo "== after rescan"
found=0
for h in /sys/class/mmc_host/mmc[0-9]*; do
	[ -d "$h" ] || continue
	for c in "$h"/mmc[0-9]*; do
		[ -e "$c" ] || continue
		case "$(basename "$c")" in *mmc_host) continue ;; esac
		found=1
		echo "  CARD PRESENT: $(basename "$c") name=$(cat "$c/name" 2>/dev/null) \
manfid=$(cat "$c/manfid" 2>/dev/null) date=$(cat "$c/date" 2>/dev/null)"
	done
done
echo "  block devices: $(ls /dev/mmc* 2>/dev/null | tr '\n' ' ')"
grep -i mmc /proc/partitions 2>/dev/null || echo "  no mmc row in /proc/partitions"

echo "== the DTB's own claim about this slot (pinmux evidence, not proof of a socket)"
for n in /proc/device-tree/soc/sdmmc /proc/device-tree/soc*/sdmmc; do
	[ -d "$n" ] || continue
	echo "  $n status=$(cat "$n/status" 2>/dev/null)"
	ls "$n" 2>/dev/null | grep -i -E "slot|cd|bus-width|fmax" | tr '\n' ' '
	echo
done

if [ "$found" = 1 ]; then
	echo "VERDICT: the slot is wired and a card responds — a FAT card is a usable recovery"
	echo "medium on this unit (the env's setsdcard/uknor/urnor are the commands to use)."
	exit 0
fi
echo "VERDICT: nothing answered. With a card inserted, that means no socket on this PCB or"
echo "its pins are not in this board file. Neither is proof about the other two cameras."
exit 1
