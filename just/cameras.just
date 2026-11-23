# JUST — the camera fleet: read a device, push one whole file, and the firmware ceremony.
#
# Part of the justfile: the root justfile imports these, so `just` still
# resolves every recipe from the repo root and `just --list` still shows
# them.  Split by job (AGENTS.md rule 26), so no file here is over 1000
# lines, and the guard that enforces that rule now sees justfiles.

# --- the cameras (the WiFi-VLAN fleet) ------------------------------------
#
# `cam7` and `cam9` are Buildroot/OpenIPC boxes on VLAN 226; `cam5` and `cam8` are the
# thingino two — same VLAN, the same recipes, a different payload, and the build/verify and
# the two deploy ceremonies at the end of this file are theirs.  All four are rows in
# `common/dns-records.nix` (10.3.2.10/.12/.17/.11, pinned by MAC), which is the table these
# recipes read — there is no second copy of the addresses to drift.  `cam1` (10.3.2.6) is on
# the same VLAN but is neither firmware, so `cam-check-all` leaves it out.
# They run no NixOS and no
# systemd: their whole state is files under `/etc` on a **jffs2 volume on the flash's
# own settings partition** (`/dev/mtdblock4 on /overlay`, 704 KB) with `/` an `rw`
# overlay on top of it — so a write to a camera persists, a reboot undoes nothing, and
# a kernel/rootfs reflash leaves that partition alone (it is *not* a tmpfs re-read at
# boot; this comment said so until 2026-10-25 and was wrong).  Nothing in the flake
# sees them and a fault on one is invisible from the box
# they are consumed on (services1's frigate pulls their RTSP through its go2rtc).
# Read `cameras/README.md` before changing anything on one.  The device-side story
# (hardware, flash layout, how to flash) lives in the firmware's own repository,
# `cjdell/thingino-firmware` branch `ssc333-infinity6b0`, whose camera guide is `tapo-c110.md`;
# the pre-thingino bench guide (`cameras/c110.md`) was deleted on 2026-11-18: git history.
# `cameras/inventory.sh` is the busybox-ash report; `cameras/config/` holds the
# pulled /etc files.  The fleet is bootstrapped by `cam-key`, which is the one
# recipe that asks for the device password (once, interactively): the devices
# speak the operator hop key afterwards and every other recipe is key-only
# (`BatchMode=yes`, so an unkeyed device fails in seconds rather than hanging
# a call).  Same shape as the printer Pis (docs/printer-servers.md).
cam-list:
    @grep -A5 -E '^  cam[0-9] = \{' common/dns-records.nix
    @echo "--- feeds (the names services1's frigate calls them by live on the box:"
    @echo "    /srv/frigate/config/config.yaml + the container's go2rtc stream table)"

cam-addr name:
    @grep -A4 "^  {{ name }} = {" common/dns-records.nix | grep -oE '10\.3\.[0-9]+\.[0-9]+' | head -1

# Is a camera alive, without any credentials?  Three interfaces, three answers,
# and each one is a different fact: RTSP :554 answers `401 Authentication
# required` when majestic is up and gated (a 200 here is a finding, not a win —
# an ungated feed is one any LAN client can pull); HTTP :80 answers its
# redirect page when the web admin is up; and the `*.int` name resolving at all
# is the router's half of the story (`just dns-sync-*` no longer exists — the
# router renders `common/dns-records.nix`, and a name missing there is a name
# nothing but the hub can answer).
cam-probe name:
    #!/usr/bin/env bash
    set -euo pipefail
    a=$(grep -A4 "^  {{ name }} = {" common/dns-records.nix | grep -oE '10\.3\.[0-9]+\.[0-9]+' | head -1)
    [ -n "$a" ] || { echo "no {{ name }} row in common/dns-records.nix" >&2; exit 1; }
    echo "rtsp ${a}:554   -> $(curl -s -m 6 -o /dev/null -w '%{http_code}' "rtsp://${a}:554/stream=0" || echo down)  (401 = up and gated)"
    echo "http {{ name }}.int   -> $(curl -s -m 6 -o /dev/null -w '%{http_code}' "http://{{ name }}.int.leighhack.org/" || echo no-dns)  (401 = the web admin is up; 000/no-dns = the router is not answering this name)"
    echo "rtsp {{ name }}.int   -> $(curl -s -m 6 -o /dev/null -w '%{http_code}' "rtsp://{{ name }}.int.leighhack.org:554/stream=0" || echo down)"
    if [ -n "${CAM_FEED:-}" ]; then
        echo "go2rtc feed ${CAM_FEED} (:8554) -> $(curl -s -m 6 -o /dev/null -w '%{http_code}' "rtsp://127.0.0.1:8554/${CAM_FEED}" || echo down)  (200 = the restream end of the pipe is alive; only true when run on services1, where go2rtc lives)"
    else
        echo "go2rtc feed end not checked: set CAM_FEED to this camera's go2rtc stream key. The device -> stream -> frigate-camera table is owned by cameras/README.md (and by services1's untracked /srv/frigate/config/config.yaml, fault 8 there); there is deliberately no second copy in this recipe, because the copy that drifts is the one written from memory"
    fi

# One ssh pass, one report: versions, board, sensor, uptime, load, memory,
# overlay usage, what the log ring holds (majestic rejects, RTSP stalls, night
# flips, sensor reinits), the feed geometry, and the md5s of every /etc file.
# It cats no credential and no binary (the WiFi PSK and ~115 KB of sensor
# calibration both live in the directories it lists).  Key-only: run
# `just cam-key <cam>` first if the device has never been keyed.
cam-check name:
    #!/usr/bin/env bash
    set -euo pipefail
    a=$(grep -A4 "^  {{ name }} = {" common/dns-records.nix | grep -oE '10\.3\.[0-9]+\.[0-9]+' | head -1)
    [ -n "$a" ] || { echo "no {{ name }} row in common/dns-records.nix" >&2; exit 1; }
    if ! ssh -i "{{ env_var('HOME') }}/.ssh/agent-hop-key" -oBatchMode=yes -oConnectTimeout=6 \
        -oStrictHostKeyChecking=accept-new root@"${a}" sh -s < cameras/inventory.sh; then
        echo "cannot ssh key-only into {{ name }}: run just cam-key {{ name }} first (this is the bootstrap, and it is the one recipe that asks for the device password), or the device is down — try just cam-probe {{ name }}" >&2
        exit 1
    fi

cam-check-all:
    #!/usr/bin/env bash
    set -euo pipefail
    # Every OpenIPC-or-thingino camera that `cam-key` can key — the four the fleet owns,
    # not the three this recipe was written for.  `cam1` (10.3.2.6) is deliberately not
    # here: it runs Xiongmai-family application firmware, so `cameras/inventory.sh` does not
    # apply to it (cameras/README.md, "not proven").  On the thingino two the OpenIPC-specific
    # lines come back empty — no `/tmp/webui/signature.txt`, no `OPENIPC_VERSION` — which is the
    # report telling the truth about a different firmware, not a fault.
    for c in cam5 cam7 cam8 cam9; do echo "===== $c"; just cam-check "$c"; done

# Pull a camera's /etc files into cameras/config/<cam>/ and print what changed.
# These are the files the fleet is meaningless without; they are **not**
# deployed by anything, they are the record of what a device is running, so a
# reflash (which wipes /etc) can be survived by pushing them back.  `/etc/`
# files that hold credentials are pulled by md5 in `cam-check` instead of cat'd
# here: the WiFi PSK (`/etc/wlanpass`, `/etc/wpa_supplicant.conf`) and the
# vendor crontab comment on cam7 that carries a Telegram bot API key.
cam-pull name:
    #!/usr/bin/env bash
    set -euo pipefail
    a=$(grep -A4 "^  {{ name }} = {" common/dns-records.nix | grep -oE '10\.3\.[0-9]+\.[0-9]+' | head -1)
    [ -n "$a" ] || { echo "no {{ name }} row in common/dns-records.nix" >&2; exit 1; }
    mkdir -p "cameras/config/{{ name }}"
    key="{{ env_var('HOME') }}/.ssh/agent-hop-key"
    for f in /etc/majestic.yaml /etc/httpd.conf /etc/crontabs/root /etc/resolv.conf \
             /etc/init.d/S95majestic /etc/init.d/S96autonight /etc/init.d/S99sdconf.sh; do
        out=$(ssh -i "$key" -oBatchMode=yes -oConnectTimeout=6 root@"${a}" "cat $f 2>/dev/null" || true)
        [ -n "$out" ] || { echo "-- missing on device: $f"; continue; }
        printf '%s\n' "$out" > "cameras/config/{{ name }}/$(basename "$f")"
        echo "-- wrote cameras/config/{{ name }}/$(basename "$f") ($(wc -c < "cameras/config/{{ name }}/$(basename "$f")") bytes)"
    done

# md5 the repo copy against the device for every file we hold.  Run this before
# `cam-push`: the only honest operation on a camera's config is to read the
# diff and then replace the whole file, because the file the vendor's build
# wrote is not a file this repo generates.
cam-diff name:
    #!/usr/bin/env bash
    set -euo pipefail
    a=$(grep -A4 "^  {{ name }} = {" common/dns-records.nix | grep -oE '10\.3\.[0-9]+\.[0-9]+' | head -1)
    [ -n "$a" ] || { echo "no {{ name }} row in common/dns-records.nix" >&2; exit 1; }
    for f in cameras/config/{{ name }}/*; do
        [ -f "$f" ] || continue
        case "$(basename "$f")" in
        root) dev=/etc/crontabs/root ;;
        S9*) dev=/etc/init.d/$(basename "$f") ;;
        *) dev=/etc/$(basename "$f") ;;
        esac
        remote=$(ssh -i "{{ env_var('HOME') }}/.ssh/agent-hop-key" -oBatchMode=yes -oConnectTimeout=6 \
            root@"${a}" "md5sum $dev 2>/dev/null" | cut -d' ' -f1 || echo gone)
        local=$(md5sum "$f" | cut -d' ' -f1)
        if [ "$remote" = "gone" ]; then echo "ABSENT on device   $dev (we hold it: $local)"
        elif [ "$remote" = "$local" ]; then echo "same             $dev"
        else echo "DRIFT            $dev  device=$remote repo=$local"
        fi
    done

# Replace one whole file on a camera, then prove it landed by md5 on both sides
# before touching the daemon: a write that did not land must not be followed by
# a restart (the restart is what makes the config live, and majestic keeps
# running from a deleted-but-open binary).  `file` is a path under
# cameras/config/<cam>/, i.e. you review the diff first.  Nothing here reflashes.
cam-push name file:
    #!/usr/bin/env bash
    set -euo pipefail
    [ -f "{{ file }}" ] || { echo "no such repo file: {{ file }}" >&2; exit 1; }
    a=$(grep -A4 "^  {{ name }} = {" common/dns-records.nix | grep -oE '10\.3\.[0-9]+\.[0-9]+' | head -1)
    [ -n "$a" ] || { echo "no {{ name }} row in common/dns-records.nix" >&2; exit 1; }
    case "$(basename {{ file }})" in
    root) dev=/etc/crontabs/root ;;
    S9*) dev=/etc/init.d/$(basename {{ file }}) ;;
    *) dev=/etc/$(basename {{ file }}) ;;
    esac
    echo "pushing {{ file }} -> {{ name }}:$dev"
    ssh -i "{{ env_var('HOME') }}/.ssh/agent-hop-key" root@"${a}" "cat > $dev" < "{{ file }}"
    now=$(ssh -i "{{ env_var('HOME') }}/.ssh/agent-hop-key" -oBatchMode=yes root@"${a}" "md5sum $dev" | cut -d' ' -f1 || echo gone)
    want=$(md5sum "{{ file }}" | cut -d' ' -f1)
    if [ "$now" != "$want" ]; then
        echo "REFUSED to restart: device md5=$now repo md5=$want — the write did not land" >&2
        exit 1
    fi
    echo "landed (md5 $want); majestic restart is what makes it live:"
    echo "  just cam-init {{ name }} S95majestic restart"

# Run an init.d script on a camera.  These boxes have busybox init, no systemd,
# and no service manager: S95majestic is the stream server, S96autonight the
# day/night flipper, S99sdconf.sh the SD hook, and `restart` (not `stop
# && start`) is what a camera understands.
cam-init name script action:
    #!/usr/bin/env bash
    set -euo pipefail
    a=$(grep -A4 "^  {{ name }} = {" common/dns-records.nix | grep -oE '10\.3\.[0-9]+\.[0-9]+' | head -1)
    ssh -i "{{ env_var('HOME') }}/.ssh/agent-hop-key" root@"${a}" "/etc/init.d/{{ script }} {{ action }}"; echo "restarting majestic remounts the overlay and reinits the sensor: expect a ~30 s window where the feed is down (frigate retries, retry_interval: 120)"

# The bootstrap: put the operator hop key in the device's authorized_keys, so
# every recipe above can be run without a password and a wedged ssh cannot
# happen.  This is the ONE recipe expected to need the device password, typed
# by the operator on their own tty (ssh reads it there; this file never has it,
# never prints it, and must never learn it — rule 13).  dropbear (2022.82 on
# these boxes) reads the same authorized_keys OpenSSH does; `/root/.ssh` is a
# symlink to /etc/dropbear on these devices.  The key line is the fleet's, not
# a secret to this repo: AGENTS.md, "SSH between machines".
cam-key name:
    #!/usr/bin/env bash
    set -euo pipefail
    a=$(grep -A4 "^  {{ name }} = {" common/dns-records.nix | grep -oE '10\.3\.[0-9]+\.[0-9]+' | head -1)
    key="{{ env_var('HOME') }}/.ssh/agent-hop-key"
    [ -f "$key" ] || { echo "no hop key at $key" >&2; exit 1; }
    ssh -i "$key" -oConnectTimeout=6 root@"${a}" "mkdir -p /root/.ssh; cat >> /root/.ssh/authorized_keys" < "$key".pub
    echo "appended.  it may already be there (a duplicate line is harmless); if the line below is not a clean pubkey, fix it before trusting the login:"
    ssh -i "$key" -oConnectTimeout=6 root@"${a}" "tail -1 /root/.ssh/authorized_keys | cut -c1-60"

# Does key-only ssh work?  This is the health check the other recipes assume:
# a BatchMode ssh that returns is keyed; one that asks is not, and the fix is
# `just cam-key <cam>`.  Nothing here prompts: BatchMode makes it fail fast.
cam-keytest name:
    #!/usr/bin/env bash
    set -euo pipefail
    a=$(grep -A4 "^  {{ name }} = {" common/dns-records.nix | grep -oE '10\.3\.[0-9]+\.[0-9]+' | head -1)
    if ssh -i "{{ env_var('HOME') }}/.ssh/agent-hop-key" -oBatchMode=yes -oConnectTimeout=6 \
        -oStrictHostKeyChecking=accept-new root@"${a}" 'echo keyed'; then
        echo "keyed"
    else
        echo "NOT keyed (or unreachable): run just cam-key {{ name }}"
    fi

# ------------------------------------------------------------------- firmware
# A firmware update on these boxes is a ceremony, not a command, and the four
# recipes below are the ceremony in order.  The pre-thingino bench guide still described
# the 2024 route — an 8 MB full-chip image written from U-Boot with `sf erase` /
# `sf write` — and that is *not* what upstream ships now: a current nightly tgz
# holds `uImage.<soc>` + `rootfs.squashfs.<soc>` and nothing else, and the tool is
# `/usr/sbin/sysupgrade`, which writes **only the kernel and rootfs MTD partitions**
# and carries the settings overlay over by itself.  That is why every gate below is
# about the overlay and not about the image: on a WiFi-only camera with no cable,
# no serial console proven, and no clip on the bench, the settings partition is the
# whole difference between "it came back" and "someone solders".
#
#   1. just cam-preflight <cam>   read-only; every gate PASS, or argued with a
#                                 measurement and never with hope
#   2. just cam-flashdump <cam>   the chip, partition by partition, to a root-only
#                                 directory: the only undo this fleet has
#   3. just cam-backup    <cam>   /etc + /root as a tar: what WiFi and SSH are made of
#   4. just cam-image <soc>       stage the exact build on services1, md5-verified
#
# There is deliberately **no update recipe**.  A recipe that reboots a camera
# cannot guarantee the WiFi association and the ssh authorisation come back with
# it, and a tool that pretends otherwise is how the fleet loses a device.  The
# update itself stays a human decision, taken in a window, with `cam-probe` before
# and after, once 1–4 are green and read.
#
# 2026-10-31: that still describes every *OpenIPC* unit (cam7, cam9), and it is the
# policy the thingino side grew around rather than broke — the thingino build/verify
# and the two deploy ceremonies (rescue card, changeover) are the section at the end
# of this file, and neither reflashes anything without a human reading its output.
# Everything about the firmware *itself* — how it is built, what the C110's flash
# layout is, how to flash one from stock, what a payload update costs — is documented
# in the firmware's own repository (cjdell/thingino-firmware, branch
# ssc333-infinity6b0; its tapo-c110.md camera guide and its AGENTS.md).
cam-preflight name:
    #!/usr/bin/env bash
    set -euo pipefail
    a=$(grep -A4 "^  {{ name }} = {" common/dns-records.nix | grep -oE '10\.3\.[0-9]+\.[0-9]+' | head -1)
    [ -n "$a" ] || { echo "no {{ name }} row in common/dns-records.nix" >&2; exit 1; }
    # The image sizes come from `just cam-image <soc>`; the defaults are the
    # 2026-10-24 ssc337_lite nightly, i.e. cam5's and cam9's SoC and build config.
    k="${IMG_KERNEL:-2003872}" r="${IMG_ROOTFS:-5136384}"
    if ! ssh -i "{{ env_var('HOME') }}/.ssh/agent-hop-key" -oBatchMode=yes -oConnectTimeout=6 \
        -oStrictHostKeyChecking=accept-new root@"${a}" \
        "export IMG_KERNEL=${k} IMG_ROOTFS=${r}; sh -s" < cameras/preupdate-check.sh; then
        echo "cannot ssh key-only into {{ name }}: just cam-key {{ name }} first, or the device is down (just cam-probe {{ name }})" >&2
        exit 1
    fi

# Every MTD partition, dumped raw, plus the layout that makes the chip
# reconstructible.  This is the answer to "restoring the firmware needs
# soldering": with a verified dump of every partition there is no need to read the
# chip again ever, and a restore is a `flashcp` (or a U-Boot `sf write` over TFTP)
# rather than a clip.  Run it before the first update of each device and keep the
# directory; it is ~8 MB and it is the cheapest insurance on this fleet.
cam-flashdump name:
    #!/usr/bin/env bash
    set -euo pipefail
    a=$(grep -A4 "^  {{ name }} = {" common/dns-records.nix | grep -oE '10\.3\.[0-9]+\.[0-9]+' | head -1)
    [ -n "$a" ] || { echo "no {{ name }} row in common/dns-records.nix" >&2; exit 1; }
    key="{{ env_var('HOME') }}/.ssh/agent-hop-key"
    dir="$HOME/camera-backups/{{ name }}-flash-$(date -u +%Y%m%dT%H%M%SZ)"
    mkdir -p "$dir"; chmod 700 "$dir"
    # A *partial* flash dump is worse than none, because it looks like one. The only
    # thing that makes this directory trustworthy is mtd-verified.txt, so a run that
    # died before writing it takes the bytes with it, and one that died after it says
    # so loudly instead of quietly.
    trap 'rc=$?; if [ "$rc" -ne 0 ]; then if [ -s "$dir/mtd-verified.txt" ]; then echo "FAILED with $rc: $dir holds a PARTIALLY verified dump — read mtd-verified.txt before trusting any of it" >&2; else rm -rf "$dir"; echo "FAILED with $rc: removed the unverified partial dump" >&2; fi; fi' EXIT
    ssh -i "$key" -oBatchMode=yes -oConnectTimeout=6 root@"${a}" \
        'for d in /sys/class/mtd/mtd[0-9]*; do case "$(basename "$d")" in *ro) continue ;; esac; printf "%s %s %s %s\n" "$(basename "$d")" "$(cat "$d/name" 2>/dev/null)" "$(cat "$d/offset" 2>/dev/null || echo 0)" "$(cat "$d/size" 2>/dev/null)"; done' \
        > "$dir/mtd-layout.txt"
    echo "layout (dev name offset size):" >&2; cat "$dir/mtd-layout.txt" >&2
    while read -r dev name offset size; do
        [ -n "$dev" ] || continue
        safe=$(printf '%s' "$name" | tr -c 'A-Za-z0-9._-' '_')
        out="$dir/${dev}-${safe}.bin"
        echo "dumping /dev/$dev ($name, $size bytes) -> $out" >&2
        # `-n` is load-bearing: without it this ssh swallows the rest of the layout
        # file, which is the loop's stdin, and the dump silently stops after mtd0.
        ssh -n -i "$key" -oBatchMode=yes -oConnectTimeout=6 root@"${a}" \
            "dd if=/dev/${dev} bs=4096 2>/dev/null" > "$out"
        got=$(wc -c < "$out" | tr -d ' ')
        want="$size"
        rmd5=$(ssh -n -i "$key" -oBatchMode=yes root@"${a}" "md5sum /dev/${dev} 2>/dev/null" | cut -d' ' -f1 || echo none)
        ltd=$(md5sum "$out" | cut -d' ' -f1)
        chmod 600 "$out"
        if [ "$got" != "$want" ]; then
            echo "TRUNCATED: /dev/$dev read $got bytes, sysfs says $want — this dump is not usable" >&2; exit 1
        fi
        if [ "$rmd5" != "$ltd" ]; then
            echo "MD5 MISMATCH on /dev/$dev (device=$rmd5 local=$ltd) — do not trust this dump" >&2; exit 1
        fi
        printf '%s %s %s %s device_md5=%s local_md5=%s\n' "$dev" "$name" "$offset" "$size" "$rmd5" "$ltd" >> "$dir/mtd-verified.txt"
    done < "$dir/mtd-layout.txt"
    want=$(grep -c "^mtd" "$dir/mtd-layout.txt")
    got=$(grep -c "" "$dir/mtd-verified.txt" 2>/dev/null || echo 0)
    if [ "$got" != "$want" ]; then
        echo "INCOMPLETE: $got of $want partitions verified in $dir — this dump is NOT a backup" >&2
        exit 1
    fi
    {
        echo "Whole-chip reconstruction from this directory (offsets are bytes, from mtd-layout.txt):"
        echo "  dd if=/dev/zero bs=1 count=0 seek=8388608 of=chip.bin   # sparse 8 MiB"
        while read -r dev name offset size _; do
            [ -n "$dev" ] || continue
            safe=$(printf '%s' "$name" | tr -c 'A-Za-z0-9._-' '_')
            echo "  dd conv=notrunc bs=1 seek=${offset} if=${dev}-${safe}.bin of=chip.bin"
        done < "$dir/mtd-layout.txt"
        echo "A restore writes back with flashcp per partition, or from U-Boot over TFTP with sf write at the same offsets."
    } > "$dir/RECONSTRUCT.txt"
    ( cd "$dir" && find . -maxdepth 1 -type f ! -name SHA256SUMS -printf '%P\n' | sort | xargs -r sha256sum > SHA256SUMS )
    echo "OK: {{ name }} flash dump in $dir (mode 700)" >&2
    ls -l "$dir" >&2

# The archive that carries WiFi and SSH: all of /etc plus /root, streamed to a
# root-only directory, never into this repo (`cam-pull` puts files in a nix store
# path; the WiFi PSK and the host keys must not go there — AGENTS.md rule 13).
# This prints sizes and hashes only: read them, and do not cat the files.
cam-backup name:
    #!/usr/bin/env bash
    set -euo pipefail
    a=$(grep -A4 "^  {{ name }} = {" common/dns-records.nix | grep -oE '10\.3\.[0-9]+\.[0-9]+' | head -1)
    [ -n "$a" ] || { echo "no {{ name }} row in common/dns-records.nix" >&2; exit 1; }
    key="{{ env_var('HOME') }}/.ssh/agent-hop-key"
    dir="$HOME/camera-backups/{{ name }}-etc-$(date -u +%Y%m%dT%H%M%SZ)"
    mkdir -p "$dir"; chmod 700 "$dir"
    # Drop the directory when nothing was archived (unkeyed device, dead camera) so the
    # backups tree cannot fill with empty mode-700 shrines to failed runs; keep it when a
    # *suspect* archive exists, because that archive and its .err are the artefact to read.
    trap 'rc=$?; if [ "$rc" -ne 0 ] && [ ! -s "$dir/etc.tar" ]; then rm -rf "$dir"; fi' EXIT
    # Every informational capture ends in `|| true`: with `set -e`, a remote command that
    # legitimately exits non-zero on one build (2.3.11.22 has no /etc/version) aborted the
    # whole backup before the tar was ever taken, and the trap then deleted the directory,
    # so the failure looked like a device problem rather than a recipe bug.
    ssh -i "$key" -oBatchMode=yes -oConnectTimeout=6 root@"${a}" 'cat /proc/mtd' > "$dir/proc-mtd.txt" || true
    ssh -i "$key" -oBatchMode=yes root@"${a}" 'cat /proc/cmdline' > "$dir/proc-cmdline.txt" || true
    ssh -i "$key" -oBatchMode=yes root@"${a}" 'cat /etc/version /etc/os-release 2>/dev/null' > "$dir/versions.txt" || true
    ssh -i "$key" -oBatchMode=yes root@"${a}" 'dmesg' > "$dir/dmesg.txt" || true
    # The env holds the WiFi PSK: capture it to a 600 file, never to a terminal.
    ssh -n -i "$key" -oBatchMode=yes root@"${a}" 'fw_printenv 2>/dev/null' > "$dir/uboot-env.txt" || echo "fw_printenv failed" > "$dir/uboot-env.txt"
    chmod 600 "$dir/uboot-env.txt"
    ssh -i "$key" -oBatchMode=yes root@"${a}" 'sh -s' < cameras/backup-etc.sh > "$dir/etc.tar" 2> "$dir/etc-tar.err"
    n=$(tar tf "$dir/etc.tar" 2>/dev/null | wc -l | tr -d ' ')
    bytes=$(wc -c < "$dir/etc.tar" | tr -d ' ')
    if [ "$bytes" -lt 10000 ] || [ "$n" -lt 10 ]; then
        echo "SUSPICIOUS archive: ${bytes} bytes, ${n} entries. Read $dir/etc-tar.err before believing this backup." >&2
        exit 1
    fi
    chmod 600 "$dir/etc.tar"
    ( cd "$dir" && find . -maxdepth 1 -type f ! -name SHA256SUMS -printf '%P\n' | sort | xargs -r sha256sum > SHA256SUMS )
    echo "OK: {{ name }} /etc archive = ${bytes} bytes, ${n} entries, sha256 $(sha256sum "$dir/etc.tar" | cut -c1-16)…" >&2
    echo "    in $dir (mode 700). It holds the WiFi PSK, the root hash and the host keys:" >&2
    echo "    never move it into the repo, never cat it, never paste a member of it." >&2

# Stage one OpenIPC build for a SoC on services1 and prove it by its own md5s.
# <soc> is a manifest platform, i.e. ssc333_lite / ssc337_lite / ssc337de_ultimate
# — cam5 and cam9 are ssc337 (their signature lines say so), cam7 is ssc333.
# It then prints the exact `cam-preflight` line to run, because the fit gate has
# to measure the build that is actually staged and not a remembered size.
cam-image soc:
    #!/usr/bin/env bash
    set -euo pipefail
    base="{{ env_var('HOME') }}/camera-images"
    mkdir -p "$base"
    curl -sS -m 60 -o "$base/manifest.flat" https://openipc.github.io/firmware/manifest.flat
    line=$(grep -E " {{ soc }} nor " "$base/manifest.flat" | sort | tail -1)
    [ -n "$line" ] || { echo "no '{{ soc }}' rows in the manifest. Platforms present:"; awk '{print $2}' "$base/manifest.flat" | sort -u | head -30; exit 1; }
    build=$(printf '%s' "$line" | cut -d' ' -f1)
    url=$(printf '%s' "$line" | cut -d' ' -f5)
    tgz="$base/${build}-{{ soc }}.tgz"
    echo "build $build"
    echo "url   $url"
    if [ ! -f "$tgz" ]; then
        curl -sSL -m 600 -o "$tgz" "$url"
    fi
    d="$base/${build}-{{ soc }}"
    mkdir -p "$d"; tar xzf "$tgz" -C "$d"
    ( cd "$d" && md5sum -c *.md5sum )
    # The tgz carries a .md5sum beside each member, so the member is found by
    # excluding those, not by a glob that matches both.
    kfile=$(find "$d" -maxdepth 1 -name 'uImage*' ! -name '*.md5sum' | head -1)
    rfile=$(find "$d" -maxdepth 1 -name 'rootfs.squashfs*' ! -name '*.md5sum' | head -1)
    [ -n "$kfile" ] && [ -n "$rfile" ] || { echo "staged tgz held no uImage and rootfs.squashfs: what is in $d?" >&2; ls -l "$d" >&2; exit 1; }
    ksz=$(wc -c < "$kfile" | tr -d ' ')
    rsz=$(wc -c < "$rfile" | tr -d ' ')
    echo "staged: $tgz"
    echo "  sha256  $(sha256sum "$tgz" | cut -d' ' -f1)"
    echo "  uImage  ${ksz} bytes"
    echo "  rootfs  ${rsz} bytes"
    echo "next, the fit gate for the build you just staged:"
    echo "  IMG_KERNEL=${ksz} IMG_ROOTFS=${rsz} just cam-preflight <cam>"
# The U-Boot env, which on these boards is where the whole recovery story lives
# (`setsdcard`, `uknor`, `urnor`, `ubnor`, `kernaddr`, `rootaddr`) — and also where the
# WiFi PSK lives (`wlanpass`).  Values of credential-shaped keys are masked *here*, in
# the recipe, by key name: an early version of this work ran a bare `fw_printenv` and put
# the network's PSK into a transcript, which is the incident this masking exists to
# prevent (AGENTS.md rule 13).  Read-only: this never runs fw_setenv.
cam-env name:
    #!/usr/bin/env bash
    set -euo pipefail
    a=$(grep -A4 "^  {{ name }} = {" common/dns-records.nix | grep -oE '10\.3\.[0-9]+\.[0-9]+' | head -1)
    [ -n "$a" ] || { echo "no {{ name }} row in common/dns-records.nix" >&2; exit 1; }
    # One sed pass over the stream, deliberately not a read loop: a `case`-in-`while read`
    # loop here silently swallowed all but one of the 34 env lines on bash 5.3, which is
    # the worst possible failure for a mask (it looks like a short env, not a broken loop).
    ssh -n -i "{{ env_var('HOME') }}/.ssh/agent-hop-key" -oBatchMode=yes -oConnectTimeout=6 \
        root@"${a}" 'fw_printenv 2>/dev/null' |
        sed -E 's/^([A-Za-z0-9_]*([Pp]ass|[Pp]sk|[Kk]ey|[Ss]ecret|[Tt]oken)[A-Za-z0-9_]*)=.*/\1=<MASKED>/' 

# Does a card slot actually exist on this board?  Not answerable from the file system: the
# host driver probes whether or not a card is present, and there is no block device until a
# card answers.  So: put a FAT32 card in, run this, read the verdict, take it out.
#
# `cameras/sd-card-check.sh` writes one thing — `1` to a sysfs `rescan` attribute, which
# re-enumerates the MMC bus.  Nothing is written to flash or to /etc, and no read-write
# root is needed.  That is worth stating plainly on this fleet, because `/` here is an
# overlay whose upper half is jffs2 on the settings partition: a stray write *persists*
# (which is the opposite of what this directory claimed until 2026-10-25).
#
# Separate recipe rather than part of `cam-check`, because it makes the kernel poke a bus
# and the answer only matters while someone is holding a card.
cam-card name:
    #!/usr/bin/env bash
    set -euo pipefail
    a=$(grep -A4 "^  {{ name }} = {" common/dns-records.nix | grep -oE '10\.3\.[0-9]+\.[0-9]+' | head -1)
    [ -n "$a" ] || { echo "no {{ name }} row in common/dns-records.nix" >&2; exit 1; }
    # No -n here, unlike the flashdump loop: this recipe's whole payload arrives on stdin,
    # and `ssh -n` replaces it with /dev/null, which runs an empty script and prints nothing.
    ssh -i "{{ env_var('HOME') }}/.ssh/agent-hop-key" -oBatchMode=yes -oConnectTimeout=6 \
        root@"${a}" 'sh -s' < cameras/sd-card-check.sh

# ------------------------------------------------- thingino firmware: build and deploy
#
# cam5 and cam8 run the bench-built SigmaStar thingino port (the endpoints, the
# geometry, the data-partition rule: docs/camera-thingino-migration-2026-10-31.md).
# These four recipes are this repo's half of owning that: the pins and hashes the
# image has to match (`cameras/thingino-manifest.sh`), the build, the card, and the
# one changeover ceremony — the full story of what each gate is for is
# docs/camera-thingino-build-deploy-2026-10-31.md.  **The firmware itself is documented
# where it lives**, in the SSOT: cjdell/thingino-firmware, branch ssc333-infinity6b0
# (private; clone it and `git submodule update --init --recursive`, plain, never --remote).
# Its `tapo-c110.md` is the C110 guide (hardware, flash layout, flashing from stock, the
# rescue card, updating a running unit, what has been payload-pushed since the pin) and its
# own AGENTS.md is the build recipe — nothing about the firmware is restated here.  What that
# repo cannot know is the pin: the commit these hashes describe survives only as tag
# `c110-fleet-20261031` (§1 of the build-deploy doc), so a fresh clone's branch head is
# *refused* by cam-fw-build.  What has NOT changed is the
# policy above: these deploy things a human watched through a rehearsal; there is
# still no recipe that reflashes a camera unattended, and on a thingino unit a
# payload update is a card and a power cycle, never ssh (erasing mtd3 under a
# thingino root is the watchdog reset that killed the first restore at 92 %).
#
#   just cam-fw-build                    containerised Buildroot run of the pinned
#                                        commit (the tag, not the branch head), ending
#                                        in cam-fw-verify or a failure
#   just cam-fw-verify                   artifacts + geometry vs the manifest
#   just cam-fw-fetch                    restage the flashed bytes of record from the
#                                        SSOT release and verify them (no build needed)
#   sudo just cam-rescue-card /dev/sdX   RESCUE.KRN/RESCUE.RFS card, md5s re-read
#                                        off the card, then power-cycle the unit
#   MIG_PASS=... GO=1 just cam-migrate <cam> <1|2|all>   the OpenIPC→thingino
#                                        changeover, sha-verified mtd backups first
cam-fw-build: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    exec cameras/thingino-build.sh

# Verify an images dir against cameras/thingino-manifest.sh (bytes + md5 + sha256
# per artifact, geometry and the rescue gate from the build's own uenv.txt).  With
# no argument it checks the bench tree's current output; pass a directory to check
# a staged copy.  Exit 0 is the only honest precondition for the card and migrate
# recipes, so both run it again rather than trusting that you ran this one.
cam-fw-verify dir="":
    #!/usr/bin/env bash
    set -euo pipefail
    d={{ quote(dir) }}
    exec cameras/thingino-verify.sh ${d:+"$d"}

# The bytes cam5/cam8 actually flash are not reproducible from the tree (docs §2),
# so they live as assets of the SSOT release; this downloads exactly the payload
# files and runs the same manifest verify every flow already demands.  With the
# bench scratchpad retired this is how a fresh machine gets a flashable image
# without waiting 40 minutes on a build it cannot byte-match anyway.
cam-fw-fetch:
    #!/usr/bin/env bash
    set -euo pipefail
    exec cameras/thingino-fetch.sh

# Write the SD-rescue card for flashed thingino units: FAT32, RESCUE.KRN (uImage)
# + RESCUE.RFS (rootfs), nothing else — the flashed bootcmd's gate loads exactly
# those two names, rewrites kernel and rootfs, falls through to boot, and S98
# eats the markers only inside a Linux that booted, so the card is one-shot and
# self-retrying.  It re-reads both files off the finished card because the copy
# chain (cp → vfat → flash) is only trusted where it has been hashed; CAM_CARD_YES=1
# skips the type-the-device-name prompt for scripted use, which is exactly as
# careful as it sounds — the prompt exists because the whole device is destroyed.
cam-rescue-card device:
    #!/usr/bin/env bash
    set -euo pipefail
    # No -n and no just-env trickery with the confirm flag: this recipe exists to
    # be typed by someone holding a card reader.
    sudo cameras/thingino-rescue-card.sh --device {{ quote(device) }} ${CAM_CARD_YES:+--yes}

# The changeover OpenIPC → thingino, for the units still on OpenIPC (cam7, cam9 —
# the table in cameras/README.md is the authority, check the MAC before anything
# writes).  GO=1 and MIG_PASS are both required; the backups land under
# $HOME/camera-backups before the first byte of flash changes, because the script
# sha-verifies all five mtd dumps and refuses on any miss.  Phase 2 is idempotent
# from a thingino side; `all` is what the cam8 rehearsal ran in one session.
cam-migrate name phase:
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{ phase }}" in 1|2|all) ;; *) echo "phase is 1, 2 or all" >&2; exit 2 ;; esac
    a=$(grep -A4 "^  {{ name }} = {" common/dns-records.nix | grep -oE '10\.3\.[0-9]+\.[0-9]+' | head -1)
    [ -n "$a" ] || { echo "no {{ name }} row in common/dns-records.nix" >&2; exit 1; }
    . cameras/thingino-manifest.sh
    # Two provenance gates before any camera sees a byte: the images must be the
    # manifest's, and the script must be the rehearsed one (docs §5: its hashes
    # move only with a fresh end-to-end rehearsal, bench and repo together).
    cameras/thingino-verify.sh >/dev/null
    md5=$(md5sum cameras/thingino-migrate.sh | cut -d' ' -f1)
    if [ "$md5" != "$MIGRATE_MD5" ]; then
        echo "cameras/thingino-migrate.sh is md5 $md5, rehearsed was $MIGRATE_MD5 — refusing" >&2
        exit 1
    fi
    [ -n "${MIG_PASS:-}" ] || { echo "MIG_PASS=<camera root password> is required: password ssh is the transport this script was proven with" >&2; exit 1; }
    [ "${GO:-}" = 1 ] || { echo "GO=1 required: this rewrites u-boot, env, kernel, rootfs and ERASES the data partition of {{ name }} ($a)" >&2; exit 1; }
    if [ -z "${THINGINO_MIGRATE_RESUME:-}" ] && \
        ssh -i "{{ env_var('HOME') }}/.ssh/agent-hop-key" -oBatchMode=yes -oConnectTimeout=6 \
        root@"${a}" 'grep -qi thingino /etc/os-release' 2>/dev/null; then
        echo "{{ name }} already runs thingino — payload updates are a rescue card (just cam-rescue-card), never this flow" >&2
        echo "(resuming an interrupted changeover at phase 2 is the one exception: THINGINO_MIGRATE_RESUME=1)" >&2
        exit 1
    fi
    dir="$HOME/camera-backups/{{ name }}-thingino-$(date -u +%Y%m%dT%H%M%SZ)"
    mkdir -p "$dir"; chmod 700 "$dir"
    echo "=== {{ name }} ($a): phase {{ phase }}, backups+log in $dir"
    MIG_PASS="$MIG_PASS" cameras/thingino-migrate.sh \
        --target "$a" --images "${THINGINO_IMAGES:-$FW_TREE_DEFAULT/$FW_IMAGES_SUBDIR}" \
        --backup "$dir" --phase "{{ phase }}" --go
