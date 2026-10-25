#!/bin/sh
# Stream the survival set off an OpenIPC camera as one tar on **stdout**.
#
#   ssh root@<cam> sh -s < cameras/backup-etc.sh > etc.tar      (see: just cam-backup)
#
# This is the archive that makes a firmware update recoverable without a clip
# programmer: `/etc` on these boxes *is* the configuration — the WiFi credentials
# that let the camera rejoin VLAN 226, the dropbear host key and authorized_keys
# that let ssh back in, the sensor pads majestic's loader needs, the init scripts.
# A flash does not usually destroy them (the updater carries the settings over),
# but "usually" is not what a camera you cannot cable to deserves.
#
# Diagnostics go to **stderr** and the archive to stdout, so the caller's `2>`
# captures the noise instead of corrupting the tar.  Nothing here cats a
# credential into a transcript: the archive is *binary*, and where it lands is
# root-only (AGENTS.md rule 13) — never in this repo, because `cam-pull` puts
# files into a nix store path and this file's contents must not go there.
#
# busybox-ash POSIX only: no awk, no rev, no bash (cameras/README.md).
cd / || exit 3

count=0
for f in etc/wlanpass etc/wpa_supplicant.conf etc/httpd-htpasswd.user \
	etc/majestic.yaml etc/dropbear etc/shadow etc/fw_env.config \
	etc/sensors etc/init.d etc/crontabs etc/network etc/ssl; do
	if [ -e "$f" ]; then
		count=$((count + 1))
	else
		# Missing is information, not an error: cam7 does not carry every file
		# cam5 does, and a silent skip here is how a backup ends up without the
		# thing it was taken for.
		echo "note: $f is not on this device (skipped)" >&2
	fi
done
echo "archiving etc/ and root/ (${count} of the expected paths present)" >&2

# `etc` is the whole point, so archive all of it rather than a list that can
# drift from what the firmware actually puts in there.  `root` comes along too:
# on these devices /root/.ssh is a symlink into /etc/dropbear, but a real file in
# /root is exactly the kind of thing nobody remembers to back up.  tar's own
# stderr is deliberately *not* redirected: the caller sent it to a file, and a
# write that quietly skipped a directory is a backup that reads as a good one.
tar cf - etc root
rc=$?
[ "$rc" -le 1 ] || {
	echo "tar failed with $rc: the archive on stdout is not trustworthy, do not use it" >&2
	exit "$rc"
}
echo "archived (tar exit $rc; 1 means it skipped something — read its message above)" >&2
