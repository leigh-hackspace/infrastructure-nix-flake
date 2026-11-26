# 802.15.4 radios — the verbs behind `common/radio-dongles.nix` and
# `radios/firmware-pins.json`. Zigbee and Thread are the same wire: both are
# IEEE 802.15.4 on 2.4 GHz, channels 11-26, and the Wi-Fi in this building is
# full of the same 2.4 GHz. This network's Zigbee coordinator is a SMLIGHT
# SLZB-06M on `advanced: channel: 11` (read from
# /srv/zigbee2mqtt/configuration.yaml on 2026-11-26), and the Thread radio is
# deliberately at 26 — as far along the channel space as the standard allows,
# because two radios on one channel drown each other and "they must match" is
# advice about MultiPAN, one chip running both.
#
# `radio-status` is the verb that answers "which dongle is which", which is the
# question `common/radio-dongles.nix` exists to stop people asking: the kernel
# numbers USB serial ports in plug order, so a reboot can hand the Thread radio
# to zigbee2mqtt and hand the Zigbee coordinator to OpenThread, with both units
# green and every Zigbee device in Home Assistant quietly arriving from a mesh
# that is no longer being maintained. Read docs/thread-border-router-2026-11-26.md
# before acting on what it prints.

# the radios table as JSON — the shape the other verbs read
radio-json:
    @nix eval --impure --raw --expr 'builtins.toJSON (import ./common/radio-dongles.nix).radios' | jq -c 'to_entries'

# what this flake says the radios are, against what is in a USB port, against the
# channels each stack is actually using. Read before acting: a dongle in a port is
# a physical fact, and a green guard does not mean the dongle in the port is the
# dongle the table means.
radio-status:
    #!/bin/sh
    nix eval --impure --raw --expr 'builtins.toJSON (import ./common/radio-dongles.nix).radios' | jq 'to_entries' > /tmp/radio-table.json
    : > /tmp/radio-devices
    jq -r '.[] | .value.device | select(. != "")' < /tmp/radio-table.json >> /tmp/radio-devices
    echo "=== the table (common/radio-dongles.nix)"
    jq -r '.[] | "  " + .key + "  role=" + .value.role + "  channel=" + (.value.channel|tostring) + "  device=" + .value.device + "  mountAt=" + .value.mountAt' < /tmp/radio-table.json
    echo "=== /dev/serial/by-id on this box, and whether the table names it (the by-id name is the fact; the number is what it points at today)"
    for n in $(ls /dev/serial/by-id/ 2>/dev/null); do
      if grep -qxF "/dev/serial/by-id/$n" /tmp/radio-devices; then
        echo "  /dev/serial/by-id/$n -> $(readlink -f "/dev/serial/by-id/$n")  (in the table)"
      else
        echo "  /dev/serial/by-id/$n -> $(readlink -f "/dev/serial/by-id/$n")  NOT IN THE TABLE"
      fi
    done
    echo "=== rows that name no device (a plan, not a lie — the thread-rcp row is what `infra.thread.enable` refuses until it is filled)"
    jq -r '.[] | select(.value.device == "") | "  " + .key + "  role=" + .value.role + "  has no device" ' < /tmp/radio-table.json
    echo "=== rows that name no device (a plan, not a lie — the thread-rcp row is what `infra.thread.enable` refuses until it is filled)"
    jq -r '.[] | select(.value.device == "") | "  " + .key + "  role=" + .value.role + "  has no device" ' < /tmp/radio-table.json
    echo "=== in the table, but not plugged in"
    jq -r '.[] | .value.device | select(. != "")' < /tmp/radio-table.json | while read -r d; do
      [ -e "$d" ] || echo "  MISSING: $d"
    done
    echo "=== which number the kernel handed out today, and what the table mounts instead"
    jq -r '.[] | select(.value.device != "") | .value.device + " " + .value.mountAt' < /tmp/radio-table.json | while read -r d m; do
      if [ -e "$d" ]; then
        t=$(readlink "$d")
        echo "  $d -> $t; the flake mounts $m, so what zigbee2mqtt and otbr are handed follows the dongle rather than that number ($([ "$(basename "${t:-none}")" = "$(basename "$m")" ] && echo "same node today" || echo "a different node today — the mount is what keeps the pairing"))"
      fi
    done
    echo "=== the flake's claim about the table, checked against zigbee2mqtt's own runtime config"
    runtime_port=$(grep -m1 -A2 '^serial:' /srv/zigbee2mqtt/configuration.yaml | grep 'port:' | tr -d ' ' | cut -d: -f2)
    runtime_channel=$(grep -m1 'channel:' /srv/zigbee2mqtt/configuration.yaml | tr -dc '0-9')
    table_port=$(jq -r '.[] | select(.value.role == "zigbee-coordinator") | .value.mountAt' < /tmp/radio-table.json)
    table_channel=$(jq -r '.[] | select(.value.role == "zigbee-coordinator") | .value.channel' < /tmp/radio-table.json)
    [ "$runtime_port" = "$table_port" ] && echo "  node: table $table_port == runtime $runtime_port" || echo "  NODE DISAGREES: table $table_port, runtime $runtime_port — zigbee2mqtt's runtime config is not the flake's, and one of them is wrong about the coordinator"
    [ "$runtime_channel" = "$table_channel" ] && echo "  channel: table $table_channel == runtime $runtime_channel" || echo "  CHANNEL DISAGREES: table $table_channel, runtime $runtime_channel — the guard's channel arithmetic is only as honest as this number"
    echo "=== zigbee2mqtt's runtime config: the node it is handed, and the channel it uses"
    grep -n -A2 '^serial:' /srv/zigbee2mqtt/configuration.yaml 2>/dev/null | grep 'port:' || echo "  (no serial port line)"
    grep -n 'channel:' /srv/zigbee2mqtt/configuration.yaml 2>/dev/null || echo "  (no channel line — zigbee2mqtt picked one at runtime, and the table would then be lying)"
    echo "=== otbr's own state (the Thread network's PAN ID and channel live inside it)"
    ls -ld /srv/otbr 2>/dev/null || echo "  no /srv/otbr: infra.thread.enable is off, so this box has no Thread network at all"
    echo "=== the IPv6 Thread needs, read off this box (measured 2026-11-26: forwarding 0, accept_ra 0, accept_ra_rt_info_max_plen 0 on vlan225)"
    for k in forwarding accept_ra accept_ra_rt_info_max_plen; do
      echo "  vlan225.$k = $(cat /proc/sys/net/ipv6/conf/vlan225/$k)"
    done

# print the /dev/serial/by-id names of whatever is in a USB port now. The table's
# `device` must be one of these, verbatim: `common/radio-tables.nix` refuses a row
# that names a bare /dev/ttyUSB<N>, refuses two rows claiming one node, and refuses
# a Thread radio with no channel or one within 8 of the coordinator's. That guard
# is not belt-and-braces — see `common/radio-tables.nix`'s header and
# `machines/services1/services/home-assistant.nix`'s "two Zigbee stacks on one
# coordinator" landmine, which is the same fault one file over.
radio-plug:
    #!/bin/sh
    echo "in a USB port on this box (/dev/serial/by-id):"
    for n in $(ls /dev/serial/by-id/ 2>/dev/null); do
      p=$(readlink -f "/dev/serial/by-id/$n")
      echo "  /dev/serial/by-id/$n"
      echo "  $p"
    done
    echo
    echo "fill the row in common/radio-dongles.nix with one of those, verbatim."
    echo "the kernel number is not a name: /dev/ttyUSB1 is only /dev/ttyUSB1 until"
    echo "the next reboot, and a reboot is how a Thread radio becomes the"
    echo "Zigbee coordinator of the space's whole mesh."

# the firmware artifacts the Thread radio plan is built from, pinned by hash, and
# whether the upstream releases and the container images still exist. `flashed:
# false` means this network recorded the artifact and has never put it on a dongle.
thread-pins:
    #!/bin/sh
    echo "=== pinned in radios/firmware-pins.json"
    jq -r '.firmware[] | .slug + "  " + .repo + "@" + .release + "  " + .file + "  flashed=" + (.flashed|tostring)' radios/firmware-pins.json
    echo "=== the pinned URLs (a 200 means the release is still there; the hash is what says it is the same file)"
    for u in $(jq -r '.firmware[] | .url' radios/firmware-pins.json); do
      echo "  $(curl -m 25 -s -o /dev/null -L -w '%{http_code}' "$u")  $u"
    done
    echo "=== container image tags (an image with only latest/edge has no version to pin, which is why the doc says so)"
    for pkg in smnws/ha-otbr-docker matter-js/matterjs-server home-assistant/app-openthread-border-router; do
      tok=$(curl -m 15 -s "https://ghcr.io/token?scope=repository:$pkg:pull" | jq -r .token)
      printf '  %s: ' "$pkg"
      curl -m 20 -s -H "Authorization: Bearer $tok" "https://ghcr.io/v2/$pkg/tags/list" | jq -r '(.tags // ["(no tag list)"]) | join(" ")'
      echo
    done

# the pinned digests as one line per artifact (`just thread-fw-verify` reads this)
thread-pin-digests:
    @jq -r '.firmware[] | [.slug, (.sha256 // ""), (.sha256_zip // ""), (.sha256_inner_hex // "")] | join(" ")' radios/firmware-pins.json > /tmp/thread-pin-digests

# fetch one pinned artifact into radios/fw/ and check it. Slug names are what
# `just thread-pins` prints.
thread-fw-fetch slug:
    #!/bin/sh
    url=$(jq -r --arg s "{{slug}}" '.firmware[] | select(.slug == $s) | .url' radios/firmware-pins.json)
    f=$(jq -r --arg s "{{slug}}" '.firmware[] | select(.slug == $s) | .file' radios/firmware-pins.json)
    if [ -z "{{slug}}" ] || [ -z "$f" ]; then
      echo "no such slug. the slugs are:"
      jq -r '.firmware[].slug' radios/firmware-pins.json
      exit 1
    fi
    # Outside the flake source, on purpose: `just cam-image`'s comment is the
    # measurement — a tarball under here is a path every evaluation of this flake
    # copies into the store, and a 110 KB RCP is a cheap-looking version of the
    # 26 MB that cost 669 MB. `radios/fw/` stays git-ignored as the guard against
    # a mistaken repo-local staging, not as the destination.
    out="$HOME/firmware-images"
    mkdir -p "$out"
    code=$(curl -m 180 -s -L -o "$out/$f" -w '%{http_code}' "$url")
    if [ "$code" != 200 ]; then
      echo "http=$code for $url"
      rm -f "$out/$f"
      exit 1
    fi
    echo "$out/$f ($(stat -c%s "$out/$f") bytes)"
    just thread-fw-verify "$out/$f"

# hash an artifact on disk against the pins, and — when it is a ZIP — hash the
# payload inside it too, because the file that gets flashed is the .hex inside the
# .zip and both digests are pinned. `just cam-fw-verify` is the same shape for the
# camera firmware; this is the reason the pins file keeps two numbers per row.
thread-fw-verify path:
    #!/bin/sh
    just thread-pin-digests
    p="{{path}}"
    if [ ! -f "$p" ]; then
      echo "no such file: $p — fetch it with just thread-fw-fetch, and the slugs are what just thread-pins prints"
      exit 1
    fi
    h=$(sha256sum "$p" | cut -d" " -f1)
    if grep -q "$h" /tmp/thread-pin-digests; then
      echo "pinned: $p  $h"
    else
      echo "NOT PINNED: $p  $h — this network recorded these digests on 2026-11-26"
    fi
    if unzip -l "$p" >/dev/null 2>&1; then
      inner=$(unzip -p "$p" | sha256sum | cut -d" " -f1)
      echo "payload inside $p hashes $inner"
      grep -q "$inner" /tmp/thread-pin-digests && echo "  pinned" || echo "  NOT PINNED"
    fi

# what the border router and the Matter controller are doing, or why they are not.
# Neither runs on this box as shipped. The journal lines this prints are the ones
# that matter: an otbr-agent that crashed its radio reads as a working Thread
# network in Home Assistant's integration, and the class of crash to read for is
# in docs/thread-border-router-2026-11-26.md.
thread-status:
    #!/bin/sh
    for u in podman-otbr-service podman-matter-server-service; do
      echo "=== $u"
      echo "  active: $(systemctl is-active "$u" 2>/dev/null || echo unknown)"
      echo "  result: $(systemctl show -p Result -p ActiveState --value "$u" 2>/dev/null)"
      journalctl -u "$u" --no-pager -n 40 2>/dev/null | grep -E 'RadioSpinel|RCP|TREL|otbr|Error|error|warning' | tail -10 || true
    done
    echo "=== the border router's REST API, which is what Home Assistant's Thread integration drives (loopback by design, and 8196 is this network's — see `machines/services1/services/thread-router.nix` and docs/thread-border-router-2026-11-26.md, because the guide's 8081 is the Space API's)"
    curl -fsS -m 5 http://127.0.0.1:8196/ -o /dev/null -w '  http=%{http_code}\n' || echo "  nothing listening on 127.0.0.1:8196"
    echo "=== ports the two services would take, and whether they are free (all three answered refused on 2026-11-26)"
    for p in 127.0.0.1:8196 127.0.0.1:8098 127.0.0.1:5580; do
      printf '  %s -> %s' "$p" "$(curl -sS -m 4 -o /dev/null -w "%{http_code}" "$p" 2>/dev/null || echo refused)"
      echo
    done
    echo "=== the state dirs"
    ls -ld /srv/otbr /srv/matter-server 2>/dev/null || echo "  neither exists: infra.thread.enable and infra.matter.enable are both off on this box"
