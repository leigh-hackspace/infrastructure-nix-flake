#!/bin/sh
# sdconf.sh — the WiFi recovery payload, run at boot by /etc/init.d/S99sdconf.sh
# from /mnt/mmcblk0p1.  Nothing else on these boards reads the card, so this file
# *is* the no-console, no-network recovery path.
#
# Card layout (FAT32, root of the card):
#   /sdconf.sh                    this file
#   /sdrecover/wifi.txt           line 1 = SSID, line 2 = PSK
#   /sdrecover/files/…            optional: files to install, paths relative to /
#                                 (only /etc/** and /lib/modules/** are accepted)
#
# Why a file and not the U-Boot env: the credentials on these boards live in
# `wlanssid`/`wlanpass`, and /etc/network/interfaces.d/wlan0 regenerates
# /tmp/wpa_supplicant.conf from them on every ifup.  This payload instead writes
# /etc/wpa_supplicant.conf plus a stanza that reads *that*, so the recovery never
# writes mtd1 — the env partition that also holds bootcmd and ethaddr and has no
# redundant copy.  See docs/camera-wifi-recovery.md.
#
# Applied once: the payload directory is renamed to sdrecover.done, so a card left
# in the slot does nothing on later boots.  To retry, rename it back on a computer.
#
# POSIX sh + busybox applets only (no awk, no bash) — these boards have neither.

set -u
CARD=/mnt/mmcblk0p1
PAYLOAD=$CARD/sdrecover
DONE=$CARD/sdrecover.done
FLAG=/tmp/sdrecover.changed

[ -d "$PAYLOAD" ] || exit 0

echo "sdrecover: payload found"
rm -f "$FLAG"

# 1. Credentials -> /etc/wpa_supplicant.conf (overlay) + a stanza that reads it.
if [ -f "$PAYLOAD/wifi.txt" ]; then
	ssid=$(sed -n 1p "$PAYLOAD/wifi.txt" | tr -d '\r')
	psk=$(sed -n 2p "$PAYLOAD/wifi.txt" | tr -d '\r')
	if [ -n "$ssid" ] && [ -n "$psk" ]; then
		mkdir -p /etc/network/interfaces.d
		if wpa_passphrase "$ssid" "$psk" > /etc/wpa_supplicant.conf; then
			# Same trick the stock stanza uses: drop the cleartext #psk comment
			# and add scan_ssid in its place.
			sed -i 's/#psk.*/scan_ssid=1/g' /etc/wpa_supplicant.conf
			printf '%s\n' \
				'iface wlan0 inet dhcp' \
				'    pre-up wpa_supplicant -B -i wlan0 -D nl80211,wext -c /etc/wpa_supplicant.conf' \
				'    post-down killall -q wpa_supplicant' \
				> /etc/network/interfaces.d/wlan0
			echo "sdrecover: credentials written"
			: > "$FLAG"
		else
			echo "sdrecover: wpa_passphrase failed; credentials left alone"
		fi
	else
		echo "sdrecover: wifi.txt needs two non-empty lines; ignored"
	fi
fi

# 2. Optional files (a sensor module, tunings) — nothing outside /etc and
#    /lib/modules, so a mistyped path cannot land on the boot region.
if [ -d "$PAYLOAD/files" ] && cd "$PAYLOAD/files"; then
	find . -type f 2>/dev/null | while read -r f; do
		dest=${f#.}
		case "$dest" in
			/etc/* | /lib/modules/*) ;;
			*) echo "sdrecover: refusing $dest"; continue ;;
		esac
		case "$dest" in
			*..*) echo "sdrecover: refusing $dest"; continue ;;
		esac
		[ -f "$dest" ] && cmp -s "$PAYLOAD/files$dest" "$dest" && continue
		mkdir -p "$(dirname "$dest")"
		if cp -f "$PAYLOAD/files$dest" "$dest"; then
			echo "sdrecover: installed $dest"
			: > "$FLAG"
		fi
	done
	cd /
fi

if [ -f "$FLAG" ]; then
	sync
	rm -rf "$DONE"
	mv "$PAYLOAD" "$DONE" 2>/dev/null || echo "sdrecover: could not rename payload (card read-only?)"
	echo "sdrecover: applied; rebooting"
	reboot
else
	echo "sdrecover: nothing to change; payload left for the next attempt"
fi
exit 0
