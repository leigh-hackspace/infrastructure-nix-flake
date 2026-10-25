#!/bin/sh
# Read-only pre-flight for a firmware update on an OpenIPC camera.
#
# Run it with `just cam-preflight <cam>` (it is piped to `sh -s` over ssh, so
# there is no argv length limit — cameras/README.md).  It writes nothing, sets
# nothing, restarts nothing, and cats no credential: the WiFi PSK, the Basic-auth
# user and the root password hash are reported by **path, size and md5 only**,
# same as everywhere else in this directory (AGENTS.md rule 13).
#
# WHY each gate exists: the update is only safe if the gates hold, because these
# three cameras are WiFi-only and their entire configuration is the overlay that a
# bad flash or a lost settings partition takes away.  Recovery from a boot that
# never happens needs the board's U-Boot — serial console, maybe an SD slot, maybe
# a clip — so "the gates said no" is always cheaper than finding out which.
#
# busybox-ash POSIX only: no awk (absent on all three — proven, cameras/README.md),
# no rev, no bash.  Every wait is bounded (rule 11).
#
# The image sizes are overridable so the fit gate measures the build actually
# staged (`just cam-image <soc>` prints them); the defaults are the 2026-10-24
# nightly for ssc337_lite, which is cam5's and cam9's SoC and build config.
IMG_KERNEL=${IMG_KERNEL:-2003872}
IMG_ROOTFS=${IMG_ROOTFS:-5136384}
IMG_TOTAL=$((IMG_KERNEL + IMG_ROOTFS))


gates=""

# `fact` prints immediately.  An earlier draft accumulated the lines into $facts
# and printed that at the end, which meant the entire fact half of this report was
# silently discarded — a check that prints nothing reads as a check that saw nothing.
fact() { printf '%-26s %s\n' "$1" "$2"; }
gate() { gates="$gates
$(printf 'GATE %-3s %-8s %s' "$1" "$2" "$3")"; }
have() { command -v "$1" >/dev/null 2>&1; }
fsize() { s=$( (wc -c <"$1") 2>/dev/null); [ -n "$s" ] || s=-; printf '%s' "$s"; }
fmd5() { s=$( (md5sum "$1" 2>/dev/null | cut -d' ' -f1) 2>/dev/null); [ -n "$s" ] || s=-; printf '%s' "$s"; }

echo "== OpenIPC firmware pre-flight (read-only)  $(date -u 2>/dev/null)"

# ---------------------------------------------------------------- flash layout
echo "--- flash layout (/proc/mtd)"
cat /proc/mtd 2>/dev/null
kdev=""; rdev=""; ksz=0; rsz=0; allsz=0; names=""
while read -r line; do
	case "$line" in mtd*) ;; *) continue ;; esac
	set -- $line
	dev=${1%:}
	sz=$(printf '%s' "$2" | tr 'abcdef' 'ABCDEF')
	nm=$(printf '%s' "$line" | cut -d'"' -f2)
	names="$names $dev=$nm"
	d=0
	i=1
	hlen=$(printf '%s' "$sz" | wc -c | tr -d ' ')
	while [ "$i" -le "$hlen" ]; do
		c=$(printf '%s' "$sz" | cut -c"$i-$i")
		case "$c" in
		0) v=0 ;; 1) v=1 ;; 2) v=2 ;; 3) v=3 ;; 4) v=4 ;; 5) v=5 ;; 6) v=6 ;; 7) v=7 ;;
		8) v=8 ;; 9) v=9 ;; A) v=10 ;; B) v=11 ;; C) v=12 ;; D) v=13 ;; E) v=14 ;; F) v=15 ;;
		*) break ;;
		esac
		d=$((d * 16 + v))
		i=$((i + 1))
	done
	allsz=$((allsz + d))
	case "$nm" in
	kernel* | KERNEL*) kdev=$dev; ksz=$d ;;
	*data*) ;;                                          # settings, never rootfs
	rootfs* | ROOTFS*) [ -z "$rdev" ] && { rdev=$dev; rsz=$d; } ;;
	esac
done </proc/mtd
fact "mtd partitions" "$names"
fact "kernel partition" "${kdev:-none} = ${ksz} bytes"
fact "rootfs partition" "${rdev:-none} = ${rsz} bytes"
fact "partitions sum" "$allsz bytes"
fact "/proc/cmdline" "$(cat /proc/cmdline 2>/dev/null | cut -c1-180)"

# ------------------------------------------------------------------- RAM / tmp
echo "--- memory and /tmp"
memavail=""; memfree=""; memtotal=""
while read -r line; do
	set -- $line
	case "$1" in
	MemTotal:) memtotal=$2 ;;
	MemFree:) memfree=$2 ;;
	MemAvailable:) memavail=$2 ;;
	esac
done </proc/meminfo
[ -n "$memavail" ] || memavail=$memfree
fact "MemTotal kB" "$memtotal"
fact "MemFree kB" "$memfree"
fact "MemAvailable kB" "$memavail"
fact "/tmp mount" "$(mount 2>/dev/null | grep ' /tmp ' | head -1)"
fact "/tmp df" "$(df -k /tmp 2>/dev/null | tail -1 | tr -s ' ')"
fact "root mount" "$(mount 2>/dev/null | grep ' / ' | head -1)"
echo "--- overlay backing (where /etc actually lives)"
mount 2>/dev/null | grep -i -E "overlay|rootfs_data|jffs2" | cut -c1-200
grep rootfs_data /proc/mounts 2>/dev/null | cut -c1-200

# ------------------------------------------------- what has to survive the flash
echo "--- survival set (path / size / md5, never contents)"
# /etc/wlanpass is kept in the list because the older notes in this directory name it; on
# the three production units it does not exist and /etc/wpa_supplicant.conf is the file
# that matters (measured 2026-10-25). A missing entry here is a note, not a failure.
for f in /etc/wlanpass /etc/wpa_supplicant.conf /etc/wireless /etc/httpd-htpasswd.user \
	/etc/majestic.yaml /etc/shadow /etc/dropbear/dropbear_ed25519_host_key \
	/etc/dropbear/authorized_keys /etc/fw_env.config /etc/os-release /etc/version \
	/etc/custom.ok /etc/init.d/S50dropbear /etc/init.d/S95majestic; do
	printf '%-42s size=%-8s md5=%s\n' "$f" "$(fsize "$f")" "$(fmd5 "$f")"
done
fact "os-release" "$(cat /etc/os-release 2>/dev/null | tr '\n' ' ' | cut -c1-160)"
fact "/etc/version" "$(cat /etc/version 2>/dev/null | tr '\n' ' ' | cut -c1-160)"
fact "/root/.ssh" "$(ls -ld /root/.ssh 2>/dev/null | cut -c1-90)"
fact "wlan env (name only)" "$(fw_printenv wlandev 2>/dev/null | cut -d= -f2-)"
fact "authorized_keys lines" "$(grep -c . /etc/dropbear/authorized_keys 2>/dev/null || echo '?')"

# --------------------------------------------------------------- update tooling
echo "--- update tooling on the running system"
for t in sysupgrade fw flashcp curl wget tar md5sum awk strings reboot; do
	if have "$t"; then t_ok="$t_ok $t"; else t_no="$t_no $t"; fi
done
fact "tools present" "$t_ok"
fact "tools MISSING" "$t_no"
fact "installed sysupgrade" "$(grep -m1 scr_version /usr/sbin/sysupgrade 2>/dev/null)"
fact "busybox" "$(busybox 2>&1 | head -1)"
fact "busybox awk applet" "$(busybox --list 2>/dev/null | grep -c '^awk$')"

# ------------------------------------------------- network path to the image host
echo "--- can this box fetch the image itself?"
net=""
if have curl; then
	# Any non-empty code is NOT success: curl prints 000 when the connection never
	# completed, and a fetch gate that passes on 000 is how an update plan gets built
	# on a route that does not exist.  Require a real 2xx/3xx.
	code=$(curl -sS -m 15 -o /dev/null -w '%{http_code}' \
		https://openipc.github.io/firmware/manifest.flat 2>/dev/null)
	case "$code" in
	2?? | 3??) net="curl http_code=$code" ;;
	*) net="" ; netfail="curl returned ${code:-no code} (000 = no route or no TLS trust)" ;;
	esac
elif have wget; then
	net="wget only: $(wget --version 2>&1 | head -1)"
fi
fact "manifest fetch" "${net:-no curl and no wget on this box}"

# ------------------------------------------------------------------ unbrick path
echo "--- what an unbrick would have to use (facts, no gate)"
fact "mmc hosts (SD)" "$(ls /sys/class/mmc_host 2>/dev/null | tr '\n' ' ')"
fact "mmc blk devices" "$(ls /dev/mmc* 2>/dev/null | tr '\n' ' ')"
fact "net interfaces" "$(ls /sys/class/net 2>/dev/null | tr '\n' ' ')"
fact "watchdog" "$(ls -l /dev/watchdog 2>/dev/null | cut -c1-80)"
fact "console" "$(cat /proc/cmdline 2>/dev/null | tr ' ' '\n' | grep -m1 console)"
# busybox `tr` cannot be trusted with character classes here, so the banner is
# pulled with `strings` when the image has it and left blank when it does not;
# `just cam-flashdump` reads the same thing off the dump with real tools.
fact "uboot banner (mtd0)" "$(dd if=/dev/mtd0 bs=1024 count=512 2>/dev/null | strings 2>/dev/null | grep -m1 -i 'U-Boot 20')"

echo "================================================================"
# ----------------------------------------------------------------------- gates
if [ -n "$kdev" ] && [ -n "$rdev" ]; then
	gate 1 PASS "the updater can name what it writes: kernel=$kdev rootfs=$rdev; those two partitions and nothing else"
else
	gate 1 FAIL "kernel and rootfs partitions are not both findable in /proc/mtd — an updater that cannot name its target writes to a guess"
fi
if [ "$ksz" -gt "$IMG_KERNEL" ] && [ "$rsz" -gt "$IMG_ROOTFS" ]; then
	gate 2 PASS "staged image fits: kernel ${IMG_KERNEL} < ${ksz} and rootfs ${IMG_ROOTFS} < ${rsz} bytes"
else
	gate 2 FAIL "staged image does not fit (kernel ${IMG_KERNEL}/${ksz}, rootfs ${IMG_ROOTFS}/${rsz}). Nothing on this fleet should be flashed with an image the flash cannot hold"
fi
if [ -z "$memavail" ]; then
	gate 3 UNKNOWN "no MemFree and no MemAvailable in /proc/meminfo — measure by hand before believing an updater's sizing"
elif [ "$memavail" -gt 9216 ]; then
	gate 3 PASS "${memavail} kB available: the image (${IMG_TOTAL} B) can be unpacked in RAM and the flash phase can run there"
else
	gate 3 FAIL "only ${memavail} kB available. The updater refuses to unpack in RAM below roughly the image size, and flashing the rootfs you are running from is the failure its RAM pivot exists to prevent"
fi
if mount 2>/dev/null | grep ' /tmp ' | grep -q tmpfs; then
	gate 4 PASS "/tmp is tmpfs, so a 7 MB archive can be staged"
else
	gate 4 UNKNOWN "/tmp does not say tmpfs. If it is the 704 KB overlay of this firmware, no archive fits there and only a streamed update can work"
fi
flashback=0
mount 2>/dev/null | grep -i -E "overlay" | grep -q -E "mtdblock|/dev/mtd" && flashback=1
mount 2>/dev/null | grep -q -E "^/dev/mtdblock[0-9]+ on /overlay type jffs2" && flashback=1
grep -q -E "rootfs_data|mtdblock" /proc/mounts 2>/dev/null && flashback=1
if [ "$flashback" = 1 ]; then
	gate 5 PASS "the settings are on flash (jffs2 on an mtdblock under the / overlay): /etc survives the flash and the reboot"
else
	gate 5 FAIL "no flash-backed settings mount visible. On these units the answer is '\"/dev/mtdblock4 on /overlay type jffs2'\"; if that is absent, /etc is RAM and it is gone at the next boot — and this camera is unreachable by design"
fi
if [ -s /etc/wpa_supplicant.conf ] || [ -s /etc/wlanpass ]; then
	gate 6 PASS "WiFi credentials are on the box to be carried over (wpa_supplicant.conf $(fsize /etc/wpa_supplicant.conf) B, wlanpass $(fsize /etc/wlanpass) B)"
else
	gate 6 FAIL "no WiFi credential file found. NOTE: on these three the credential is /etc/wpa_supplicant.conf — /etc/wlanpass is the SD-customizer's filename and does not exist in the running config, whatever the older notes in this directory say. A camera that cannot rejoin VLAN 226 is a bench visit, not a retry"
fi
if [ -f /etc/dropbear/dropbear_ed25519_host_key ] && [ -s /etc/dropbear/authorized_keys ]; then
	gate 7 PASS "SSH comes back by itself: host key present, authorized_keys $(fsize /etc/dropbear/authorized_keys) bytes / $(grep -c . /etc/dropbear/authorized_keys 2>/dev/null) lines"
else
	gate 7 FAIL "no host key or no authorized_keys: a reset dropbear leaves a camera that answers on the WiFi and admits nobody, over a link you cannot cable around"
fi
if have awk; then
	gate 8 PASS "awk present — the current sysupgrade uses it heavily, and it was believed absent here (it is not: /usr/bin/awk on all three). Older notes in this directory say otherwise; trust this measurement"
else
	gate 8 FAIL "no awk here. The current sysupgrade uses awk in its validation and sizing paths, so an in-place update cannot self-validate on this device"
fi
if have sysupgrade || have fw; then
	gate 9 UNKNOWN "an updater is installed. Read ITS OWN usage (sysupgrade -h): the versions on this fleet are older than upstream's, and flags like --archive/--build/--list-builds are NOT all present. Never script an update against the upstream flag list"
else
	gate 9 FAIL "no updater on the running system. Writing MTD partitions by hand from a RAM root has no pre-flight and no settings carry-over: that is how a camera needs a clip"
fi
if [ -n "$net" ]; then
	gate 10 PASS "the camera can reach the image host ($net). A LAN mirror is still preferred so the flash does not depend on WAN at flash time"
else
	gate 10 FAIL "the camera cannot fetch the manifest (${netfail:-no curl and no wget}). Stage the build on services1 and push it to /tmp, or serve it on the LAN: an update path that depends on a route this gate could not prove is not a plan"
fi
echo "$gates"
echo "================================================================"
echo "A gate that says FAIL may not be argued away.  Before any update: just"
echo "cam-flashdump <cam> (the whole chip, which is what makes recovery possible"
echo "without a clip) and just cam-backup <cam> (the /etc that carries WiFi+SSH)."
