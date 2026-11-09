#!/usr/bin/env bash
# BREAK GLASS — put the network back online without the flake, by trying every
# possible WAN until one carries packets.
#
# Run on gw, over the LAN (ssh to 10.3.1.1 survives a dead WAN) or at the
# physical console:
#
#   sudo /etc/wan-backup-emergency.sh            # try everything, retry until online
#   sudo /etc/wan-backup-emergency.sh --check    # dry run: what would it try?
#   sudo /etc/wan-backup-emergency.sh --force    # act even if the router is online
#   sudo /etc/wan-backup-emergency.sh --once     # one pass, do not retry
#   sudo /etc/wan-backup-emergency.sh --watch    # keep watching and re-repair
#   sudo /etc/wan-backup-emergency.sh --no-tunnel
#                                               # do not move the AAISP session
#   sudo /etc/wan-backup-emergency.sh --tunnel-over-wwan
#                                               # same as the default here
#
# WHAT IT TRIES, in order, with no hardcoded WAN:
#   1. every USB Ethernet netdev (a tethering phone), DHCP;
#   2. every wireless interface, scanning for "Leighhack WAN Backup" and
#      associating with the passphrase from /run/secrets (or sops);
#   3. every other Ethernet interface that is not a VLAN/LAN/tunnel, DHCP.
# The first path whose own gateway answers and that can reach the internet
# through itself wins. It then installs the main default, the table 200 (hub
# mark) and table 300 (break-glass mark) defaults, the AAISP endpoint /32, the
# two `ip rule`s at pref 50, and NAT — including a standalone NAT table if the
# flake's `ip routerNat` is missing.
#
# WHY IT IS NOT NIX. It has to work when the thing that broke is `nixos-rebuild`,
# systemd-networkd's generated files, the `wan-manager` unit or the nftables
# ruleset — i.e. when "deploy the flake" is the thing that took the network away.
# It depends only on binaries already in the current system.
#
# A REBOOT ENDS ALL OF IT. The `/run/systemd/network` files, the routes, the
# rules, the nft set elements and the supplicant are gone; `wan-manager` starts
# and adopts the box again. To hand back without a reboot, run on gw:
#
#   just wan-backup-handback
set -uo pipefail

PATH=/run/current-system/sw/bin:/run/wrappers/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export PATH

REPO=${REPO:-/home/leigh-admin/Projects/infrastructure-nix-flake}
SOPS_FILE="$REPO/secrets/secrets.yaml"
SECRET_NAME=wan_backup_wifi_psk
SSID="${SSID:-Leighhack WAN Backup}"
STATE=/run/wan-emergency
MARKER="$STATE/active"
NET_METRIC=50
L2TP_METRIC=5
# The network's own blocks, not a WAN fact. These are what NAT has to translate.
NAT_SOURCES=("10.3.0.0/16" "192.168.2.0/24" "100.64.0.0/10")

DRY=0
FORCE=0
ONCE=0
WATCH=0
TUNNEL=1
DELAY=10
for arg in "$@"; do
  case "$arg" in
    --check | --dry-run) DRY=1 ;;
    --force) FORCE=1 ;;
    --once) ONCE=1 ;;
    --watch) WATCH=1 ;;
    --no-tunnel) TUNNEL=0 ;;
    --tunnel-over-wwan) TUNNEL=1 ;;
    --delay=*) DELAY=${arg#--delay=} ;;
    -h | --help) sed -n '2,52p' "$0"; exit 0 ;;
    *)
      echo "unknown argument: $arg" >&2
      exit 2
      ;;
  esac
done
case "$DELAY" in '' | *[!0-9]*) DELAY=10 ;; esac
[ "$DELAY" -ge 3 ] || DELAY=3

say() { printf '%s\n' "$*"; }
log() { printf '%s\n' "$*" >&2; }
die() {
  printf '\nEMERGENCY: %s\n' "$*" >&2
  exit 1
}

[ "$(id -u)" -eq 0 ] || die "run this as root (sudo $0)"
mkdir -p "$STATE"

online_any() {
  curl -fsS -m 8 -o /dev/null https://one.one.one.one 2>/dev/null
}
online_iface() {
  curl -fsS -m 12 --interface "$1" -o /dev/null https://one.one.one.one 2>/dev/null
}

# --- discovery ----------------------------------------------------------------
is_wireless() {
  [ -e "/sys/class/net/$1/wireless" ] || [ -e "/sys/class/net/$1/phy80211" ]
}
is_usb_ether() {
  [ "$(cat "/sys/class/net/$1/type" 2>/dev/null || echo 0)" = "1" ] || return 1
  readlink -f "/sys/class/net/$1/device" 2>/dev/null | grep -q '/usb' || return 1
}
is_plain_ether() {
  [ "$(cat "/sys/class/net/$1/type" 2>/dev/null || echo 0)" = "1" ] || return 1
  is_usb_ether "$1" && return 1
  case "$1" in
    lo | vlan* | tailscale* | tun* | tap* | wg* | docker* | podman* | br-* | veth*) return 1 ;;
  esac
  return 0
}

usb_ifaces() {
  for d in /sys/class/net/*; do
    n=${d##*/}
    is_usb_ether "$n" && printf '%s\n' "$n"
  done
}
wifi_ifaces() {
  for d in /sys/class/net/*; do
    n=${d##*/}
    is_wireless "$n" && printf '%s\n' "$n"
  done
}
plain_ifaces() {
  for d in /sys/class/net/*; do
    n=${d##*/}
    is_plain_ether "$n" && printf '%s\n' "$n"
  done
}

find_wpa() {
  local p t
  [ -x /run/current-system/sw/bin/wpa_supplicant ] && {
    printf '%s\n' /run/current-system/sw/bin/wpa_supplicant
    return 0
  }
  for p in /proc/[0-9]*/exe; do
    t=$(readlink -f "$p" 2>/dev/null || true)
    case "$t" in
      */bin/wpa_supplicant)
        [ -x "$t" ] && {
          printf '%s\n' "$t"
          return 0
        }
        ;;
    esac
  done
  command -v wpa_supplicant 2>/dev/null
}
find_wpa_passphrase() {
  local w
  w=$(find_wpa) || return 1
  [ -x "${w%/*}/wpa_passphrase" ] && {
    printf '%s\n' "${w%/*}/wpa_passphrase"
    return 0
  }
  command -v wpa_passphrase 2>/dev/null
}

get_psk() {
  local v
  if [ -r "$STATE/wpa.conf" ] && grep -q '^[[:space:]]*psk=' "$STATE/wpa.conf" 2>/dev/null; then
    v=$(sed -n 's/^[[:space:]]*psk=//p' "$STATE/wpa.conf" | head -n 1)
    v=${v#\"}
    v=${v%\"}
    printf '%s\n' "$v"
    return 0
  fi
  if [ -r "/run/secrets/$SECRET_NAME" ]; then
    cat "/run/secrets/$SECRET_NAME"
    return 0
  fi
  if [ -r /root/wan-backup.psk ]; then
    cat /root/wan-backup.psk
    return 0
  fi
  [ -r "$SOPS_FILE" ] || return 1
  if [ -z "${SOPS_AGE_KEY_FILE:-}" ] && [ -r /var/lib/sops-nix/key.txt ]; then
    export SOPS_AGE_KEY_FILE=/var/lib/sops-nix/key.txt
  fi
  sops -d --extract "[\"$SECRET_NAME\"]" "$SOPS_FILE" 2>/dev/null
}

# --- networkd ----------------------------------------------------------------
write_network_file() {
  local iface=$1 file
  mkdir -p /run/systemd/network
  file="/run/systemd/network/10-wan-emergency-$iface.network"
  cat >"$file" <<EOF
[Match]
Name=$iface

[Network]
DHCP=ipv4
IPv6AcceptRA=no

[DHCPv4]
UseDNS=no
UseRoutes=yes
RouteMetric=$NET_METRIC
EOF
  say "   wrote $file (DHCP, metric $NET_METRIC)"
}

remove_network_file() {
  rm -f "/run/systemd/network/10-wan-emergency-$1.network"
}

wait_gateway() {
  local iface=$1 gw=""
  for _ in $(seq 1 30); do
    gw=$(ip -4 route show default dev "$iface" 2>/dev/null |
      sed -n 's/.* via \([0-9.]*\).*/\1/p' | head -n 1)
    [ -n "$gw" ] && {
      printf '%s\n' "$gw"
      return 0
    }
    sleep 1
  done
  return 1
}

clear_ours() {
  local iface=$1
  ip -4 route del default dev "$iface" metric "$NET_METRIC" 2>/dev/null || true
  ip -4 route del default dev "$iface" table 200 metric "$NET_METRIC" 2>/dev/null || true
  ip -4 route del default dev "$iface" table 300 metric "$NET_METRIC" 2>/dev/null || true
  ip -4 route del 194.4.172.12/32 dev "$iface" metric "$L2TP_METRIC" 2>/dev/null || true
}

install_routes() {
  local iface=$1 gw=$2
  ip -4 route replace default via "$gw" dev "$iface" metric "$NET_METRIC"
  ip -4 route replace default via "$gw" dev "$iface" table 200 metric "$NET_METRIC"
  ip -4 route replace default via "$gw" dev "$iface" table 300 metric "$NET_METRIC"
  ip -4 route replace 194.4.172.12/32 via "$gw" dev "$iface" metric "$L2TP_METRIC"
  while ip rule del pref 50 2>/dev/null; do :; done
  ip rule add pref 50 fwmark 1 lookup 300
  ip rule add pref 50 fwmark 2 lookup 300
  ip route flush cache
  say "   routes: default via $gw dev $iface (metric $NET_METRIC; tables 200 and 300)"
}

install_nat() {
  local iface=$1 src
  if nft list table ip routerNat >/dev/null 2>&1; then
    nft list set ip routerNat wan_ifaces >/dev/null 2>&1 ||
      nft add set ip routerNat wan_ifaces '{ type ifname; }' 2>/dev/null || true
    nft add element ip routerNat wan_ifaces "{ \"$iface\" }" 2>/dev/null || true
    if ! nft list chain ip routerNat srcnat 2>/dev/null | grep -q '@wan_ifaces'; then
      for src in "${NAT_SOURCES[@]}"; do
        nft add rule ip routerNat srcnat oifname "@wan_ifaces" ip saddr "$src" masquerade 2>/dev/null || true
      done
    fi
    say "   NAT: ip routerNat, set element $iface"
    return 0
  fi
  say "   WARNING: ip routerNat is missing — installing a standalone NAT table."
  say "   The box has no firewall in that state; fix the flake when you can."
  nft list table ip emergencyNat >/dev/null 2>&1 || nft add table ip emergencyNat
  nft list chain ip emergencyNat postrouting >/dev/null 2>&1 ||
    nft add chain ip emergencyNat postrouting '{ type nat hook postrouting priority srcnat; }'
  for src in "${NAT_SOURCES[@]}"; do
    nft add rule ip emergencyNat postrouting oifname "$iface" ip saddr "$src" masquerade 2>/dev/null || true
  done
  say "   NAT: ip emergencyNat postrouting for $iface"
}

# --- per-path attempts --------------------------------------------------------
try_wifi() {
  local iface=$1 wpa pass pmk conf bssid
  wpa=$(find_wpa) || {
    log "   no wpa_supplicant binary"
    return 1
  }
  pass=$(get_psk || true)
  [ -n "$pass" ] || {
    log "   no Wi-Fi passphrase in /run/secrets/$SECRET_NAME, /root/wan-backup.psk or sops"
    return 1
  }
  [ "${#pass}" -ge 8 ] || {
    log "   Wi-Fi passphrase is too short (${#pass} chars)"
    return 1
  }
  conf="$STATE/wpa-$iface.conf"
  pmk=$(printf '%s\n' "$pass" | "$(find_wpa_passphrase)" "$SSID" 2>/dev/null |
    sed -n 's/^[[:space:]]*psk=//p' | head -n 1)
  [ "${#pmk}" -eq 64 ] || {
    log "   wpa_passphrase did not produce a 64-hex PMK"
    return 1
  }
  ip link set "$iface" up
  pkill -f "wpa_supplicant.*$iface" 2>/dev/null || true
  sleep 1
  umask 077
  cat >"$conf" <<EOF
country=GB
network={
  ssid="$SSID"
  psk=$pmk
  key_mgmt=WPA-PSK
  scan_ssid=1
}
EOF
  chmod 0600 "$conf"
  say "   associating $iface with \"$SSID\""
  "$wpa" -B -i "$iface" -c "$conf" -D nl80211,wext 2>/dev/null || true
  for _ in $(seq 1 20); do
    [ "$(cat "/sys/class/net/$iface/carrier" 2>/dev/null || echo 0)" = "1" ] && break
    sleep 1
  done
  [ "$(cat "/sys/class/net/$iface/carrier" 2>/dev/null || echo 0)" = "1" ] || {
    log "   no carrier on $iface after 20 s"
    return 1
  }
  write_network_file "$iface"
  networkctl reload 2>/dev/null || systemctl restart systemd-networkd
  networkctl reconfigure "$iface" 2>/dev/null || true
  local gw
  gw=$(wait_gateway "$iface") || {
    log "   no DHCP gateway via $iface after 30 s"
    return 1
  }
  install_routes "$iface" "$gw"
  install_nat "$iface"
  if online_iface "$iface"; then
    printf '%s %s %s\n' "$iface" wifi "$gw" >"$MARKER"
    return 0
  fi
  log "   $iface has a gateway ($gw) but no internet through it"
  clear_ours "$iface"
  return 1
}

try_dhcp_iface() {
  local iface=$1 kind=$2 gw
  ip link set "$iface" up 2>/dev/null || true
  write_network_file "$iface"
  networkctl reload 2>/dev/null || systemctl restart systemd-networkd
  networkctl reconfigure "$iface" 2>/dev/null || true
  gw=$(wait_gateway "$iface") || {
    log "   no DHCP gateway via $iface after 30 s"
    return 1
  }
  install_routes "$iface" "$gw"
  install_nat "$iface"
  if online_iface "$iface"; then
    printf '%s %s %s\n' "$iface" "$kind" "$gw" >"$MARKER"
    return 0
  fi
  log "   $iface has a gateway ($gw) but no internet through it"
  clear_ours "$iface"
  return 1
}

try_all_once() {
  local iface found=0
  while read -r iface; do
    [ -n "$iface" ] || continue
    found=1
    say "== trying USB tether $iface"
    try_dhcp_iface "$iface" usb && return 0
  done <<EOF
$(usb_ifaces)
EOF
  while read -r iface; do
    [ -n "$iface" ] || continue
    found=1
    say "== trying Wi-Fi $iface"
    try_wifi "$iface" && return 0
  done <<EOF
$(wifi_ifaces)
EOF
  while read -r iface; do
    [ -n "$iface" ] || continue
    found=1
    say "== trying wired $iface"
    try_dhcp_iface "$iface" wired && return 0
  done <<EOF
$(plain_ifaces)
EOF
  [ "$found" -eq 1 ] || log "no candidate interfaces found at all"
  return 1
}

move_tunnel() {
  local iface gw
  [ -s "$MARKER" ] || return 1
  read -r iface _ gw <"$MARKER" || true
  [ -n "$iface" ] || return 1
  say "== moving the AAISP L2TP endpoint onto $iface via $gw"
  ip -4 route replace 194.4.172.12/32 via "$gw" dev "$iface" metric "$L2TP_METRIC" 2>/dev/null || true
  ip route flush cache 2>/dev/null || true
  systemctl try-restart xl2tpd-aaisp.service 2>/dev/null || true
  say "   the tunnel is re-dialling over $iface (watch: ip -br addr show ppp0)"
}

cleanup_stale_paths() {
  local keep=""
  [ -s "$MARKER" ] && read -r keep _ _ <"$MARKER" || true
  local iface
  for iface in $(usb_ifaces) $(wifi_ifaces) $(plain_ifaces); do
    [ "$iface" = "$keep" ] && continue
    clear_ours "$iface"
  done
}

# --- main ---------------------------------------------------------------------
say "== inputs"
wpa=$(find_wpa || true)
pass=$(get_psk || true)
say "   SSID:        $SSID"
say "   supplicant:  ${wpa:-MISSING}"
say "   passphrase:  $( [ -n "$pass" ] && echo "${#pass} chars (recovered)" || echo MISSING )"
say "   USB ether:   $(usb_ifaces | tr '\n' ' ')"
say "   wireless:    $(wifi_ifaces | tr '\n' ' ')"
say "   other ether: $(plain_ifaces | tr '\n' ' ')"

if [ "$DRY" -eq 1 ]; then
  say ""
  say "dry run: every input above resolves and the candidate list is non-empty if a path exists."
  say "re-run without --check to apply."
  exit 0
fi

if [ "$FORCE" -eq 0 ] && online_any; then
  say "This box already has internet — nothing to do."
  say "If the LAN is broken while the router is fine (NAT missing), re-run with --force."
  exit 0
fi

say "== stopping the declarative supervisors so they cannot fight this"
systemctl stop wan-manager.service 2>/dev/null || true
# The old module's units, for a box that has not rebooted into the new tree yet.
systemctl stop wan-backup-route.timer wan-backup-route.service wan-backup-wifi.service 2>/dev/null || true

attempt=0
while :; do
  attempt=$((attempt + 1))
  say ""
  say "== attempt $attempt: trying every possible WAN"
  : >"$MARKER"
  if try_all_once; then
    read -r iface kind gw <"$MARKER" || true
    say ""
    say "== online through $iface ($kind, gateway $gw)"
    cleanup_stale_paths
    if [ "$TUNNEL" -eq 1 ]; then
      move_tunnel || true
    fi
    say ""
    ip -4 route show default
    say ""
    say "The LAN is on $iface now, by the break-glass path."
    say "A reboot ends it and wan-manager takes over again."
    say "To hand back now, run this on gw:  just wan-backup-handback"
    if [ "$WATCH" -eq 1 ]; then
      while :; do
        sleep 15
        if ! online_any; then
          say "internet lost again — re-running the whole search"
          : >"$MARKER"
          break
        fi
      done
      continue
    fi
    exit 0
  fi
  say ""
  say "== attempt $attempt found no working WAN"
  if [ "$ONCE" -eq 1 ]; then
    say "Giving up after one pass (--once)."
    say "Diagnose:  lsusb -t ; usb-devices ; ip -br link ; networkctl status ; journalctl -k -n 100"
    exit 1
  fi
  say "Retrying in ${DELAY}s. Ctrl-C stops. (--once for a single pass.)"
  sleep "$DELAY"
done
