#!/usr/bin/env bash
# BREAK GLASS — put the network back on the hotspot by hand, without the flake.
#
# Run on gw, over the LAN (ssh to 10.3.1.1 survives a dead WAN) or at the
# physical console:
#
#   sudo /root/wan-backup-emergency.sh          # apply (refuses if already online)
#   sudo /root/wan-backup-emergency.sh --force  # apply even if the router is online
#   sudo /root/wan-backup-emergency.sh --tunnel-over-wwan
#                                               # ... and move the AAISP L2TP
#                                               # endpoint onto the hotspot, so the
#                                               # public /29, the /48 and the
#                                               # tailnet control plane come back
#   sudo /root/wan-backup-emergency.sh --check  # dry run: can this script work?
#
# `--tunnel-over-wwan` is the manual form of what `wan-backup-route` now does by
# itself while the hub is dead: route 194.4.172.12 out the hotspot at metric 5 and
# bounce xl2tpd so it re-dials over it.  It is here for the case where the
# declarative units are stopped/broken and the tunnel has to be moved by hand.
#
# WHY THIS IS NOT NIX.  It has to work when the thing that broke is the switch,
# the wan-backup-* units, systemd-networkd's generated config, or the nftables
# ruleset — i.e. when "deploy the flake" is not an option because deploying is
# what took the network away.  So it recreates exactly the 2026-11-07 recipe that
# was measured working on this box, in `/run` and in the live kernel, and it
# depends on nothing but binaries already in the current system.
#
# It is a snapshot on purpose: if the LAN's IPv4 blocks, the SSID or the sops key
# name change, update them here too.  The canonical copy is
# `machines/gw/wan-backup-emergency.sh` in the repository (also installed at
# `/etc/wan-backup-emergency.sh` by `machines/gw/wan-backup.nix`); the `/root`
# copy is the one taken immediately before the 2026-11-07 switch, when the only
# working internet was this hand-built path.
#
# WHAT IT DOES, in order: stop the declarative units so they cannot fight it;
# find the Wi-Fi interface and a wpa_supplicant binary; read the passphrase
# (hand-built config → /run/secrets → sops) and derive the PMK on stdin; write
# /run/wpa-backup.conf (mode 0600); associate; write the hand-built
# /run/systemd/network/10-wan-backup.network (it sorts before the flake's
# `20-wwan.network`, so it wins) and let networkd do DHCP; install the default
# route in the main table and table 300 plus the two `ip rule`s at priority 50;
# add the `masquerade` row for the hotspot if the live ruleset lacks it.
#
# A REBOOT ENDS ALL OF IT — networkd's `/run` files and the kernel's routes and
# rules are gone, and the flake's declarative version comes up instead.  That is
# the intended way out of break-glass.
set -euo pipefail

PATH=/run/current-system/sw/bin:/run/wrappers/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export PATH

REPO=${REPO:-/home/leigh-admin/Projects/infrastructure-nix-flake}
SOPS_FILE="$REPO/secrets/secrets.yaml"
SECRET_NAME=wan_backup_wifi_psk
SSID='Leighhack WAN Backup'
WPA_CONF=/run/wpa-backup.conf
NET_CONF=/run/systemd/network/10-wan-backup.network

DRY=0
FORCE=0
TUNNEL=0
for arg in "$@"; do
  case "$arg" in
    --check | --dry-run) DRY=1 ;;
    --force) FORCE=1 ;;
    --tunnel-over-wwan) TUNNEL=1 ;;
    -h | --help) sed -n '2,55p' "$0"; exit 0 ;;
    *)
      echo "unknown argument: $arg" >&2
      exit 2
      ;;
  esac
done

say() { printf '%s\n' "$*"; }
die() {
  printf '\nEMERGENCY: %s\n' "$*" >&2
  exit 1
}

[ "$(id -u)" -eq 0 ] || die "run this as root (sudo $0)"

# --- 0. Do not tear down a working path ---------------------------------------
# The check is the router's own connectivity.  If the router is online but the
# LAN is not, that is the NAT case — pass --force.
if [ "$DRY" -eq 0 ] && [ "$FORCE" -eq 0 ]; then
  if curl -fsS -m 10 -o /dev/null https://one.one.one.one 2>/dev/null; then
    say "This box already has internet — nothing to do."
    say "If the LAN is broken while the router is fine (NAT missing), re-run with --force."
    exit 0
  fi
fi

# --- 1. Find the pieces -------------------------------------------------------
find_iface() {
  local d n u
  # The interface the deployed unit names first: it is the one this box's config
  # pinned, and the other USB port may hold a dongle that associates to the same
  # AP with a worse radio (measured 2026-11-07: the RTL8821CU gave -68 dBm where
  # the Ralink gives -57).
  u=$(sed -n 's|^ExecStart=.* -i \([^ ]*\).*|\1|p' \
    /etc/systemd/system/wan-backup-wifi.service 2>/dev/null | head -n 1 || true)
  if [ -n "${u:-}" ] && [ -e "/sys/class/net/$u" ] &&
    { [ -e "/sys/class/net/$u/wireless" ] || [ -e "/sys/class/net/$u/phy80211" ]; }; then
    printf '%s\n' "$u"
    return 0
  fi
  for d in /sys/class/net/*; do
    n=${d##*/}
    [ "$n" = "lo" ] && continue
    if [ -e "$d/wireless" ] || [ -e "$d/phy80211" ]; then
      printf '%s\n' "$n"
      return 0
    fi
  done
  return 1
}

# The supplicant, in order of preference: the deployed system's own, the binary
# of a supplicant already running (the hand-built one), and the path named by the
# deployed unit.  Anything else would need the flake to resolve, which is the one
# thing this cannot need.
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
  t=$(sed -n 's|^ExecStart=\(/[^ ]*/bin/wpa_supplicant\).*|\1|p' \
    /etc/systemd/system/wan-backup-wifi.service 2>/dev/null | head -n 1 || true)
  [ -n "${t:-}" ] && [ -x "$t" ] && {
    printf '%s\n' "$t"
    return 0
  }
  return 1
}

get_psk() {
  local v
  if [ -r "$WPA_CONF" ] && grep -q '^[[:space:]]*psk=' "$WPA_CONF" 2>/dev/null; then
    say "passphrase: $WPA_CONF (the hand-built config)" >&2
    v=$(sed -n 's/^[[:space:]]*psk=//p' "$WPA_CONF" | head -n 1)
    v=${v#\"}
    v=${v%\"}
    printf '%s\n' "$v"
    return 0
  fi
  if [ -r "/run/secrets/$SECRET_NAME" ]; then
    say "passphrase: /run/secrets/$SECRET_NAME" >&2
    cat "/run/secrets/$SECRET_NAME"
    return 0
  fi
  [ -r "$SOPS_FILE" ] || return 1
  if [ -z "${SOPS_AGE_KEY_FILE:-}" ] && [ -r /var/lib/sops-nix/key.txt ]; then
    export SOPS_AGE_KEY_FILE=/var/lib/sops-nix/key.txt
  fi
  say "passphrase: sops ($SOPS_FILE)" >&2
  sops -d --extract "[\"$SECRET_NAME\"]" "$SOPS_FILE" 2>/dev/null
}

iface=$(find_iface) || die "no wireless interface found (is the USB adapter plugged in?)"
wpa=$(find_wpa) || die "no wpa_supplicant binary in the system, /proc, or the deployed unit"
wpa_dir=${wpa%/*}
[ -x "$wpa_dir/wpa_passphrase" ] || die "no wpa_passphrase next to $wpa"

psk=$(get_psk || true)
[ -n "$psk" ] || die "no Wi-Fi passphrase in $WPA_CONF, /run/secrets/$SECRET_NAME, or sops ($SOPS_FILE)"
[ "${#psk}" -ge 8 ] || die "passphrase is too short (${#psk} chars) — refusing"

# Derive the PMK on stdin, so the passphrase never appears in this script, in a
# process listing, or in the config file.
pmk=$(printf '%s\n' "$psk" | "$wpa_dir/wpa_passphrase" "$SSID" 2>/dev/null |
  sed -n 's/^[[:space:]]*psk=//p' | head -n 1)
[ "${#pmk}" -eq 64 ] || die "wpa_passphrase did not produce a 64-hex PMK"
case "$pmk" in
  *[!0-9a-f]*) die "wpa_passphrase produced something that is not hex" ;;
esac

if [ "$DRY" -eq 1 ]; then
  say "interface:  $iface"
  say "link:       $(cat "/sys/class/net/$iface/operstate" 2>/dev/null || echo unknown)"
  say "supplicant: $wpa"
  say "passphrase: ${#psk} chars (recovered)"
  say "PMK:        64 hex (derived)"
  say "sops file:  $SOPS_FILE $( [ -r "$SOPS_FILE" ] && echo readable || echo MISSING )"
  say "OK — every input resolves.  Re-run without --check to apply."
  exit 0
fi

# --- 2. Stop the declarative path so the two cannot fight ---------------------
say "== stopping the declarative units"
systemctl stop wan-backup-route.timer wan-backup-route.service wan-backup-wifi.service 2>/dev/null || true

# --- 3. Associate -------------------------------------------------------------
say "== bringing $iface up and starting the supplicant"
ip link set "$iface" up
pkill -f "wpa_supplicant.*$iface" 2>/dev/null || true
sleep 1

umask 077
cat >"$WPA_CONF" <<EOF
country=GB
network={
  ssid="$SSID"
  psk=$pmk
  key_mgmt=WPA-PSK
  scan_ssid=1
}
EOF
chmod 0600 "$WPA_CONF"

"$wpa" -B -i "$iface" -c "$WPA_CONF" -D nl80211,wext
say "   waiting for the AP (software association, then carrier)"
for _ in $(seq 1 20); do
  [ "$(cat "/sys/class/net/$iface/carrier" 2>/dev/null || echo 0)" = "1" ] && break
  sleep 1
done
[ "$(cat "/sys/class/net/$iface/carrier" 2>/dev/null || echo 0)" = "1" ] ||
  die "no carrier on $iface after 20 s — check: $wpa_dir/wpa_cli -i $iface status"

# --- 4. DHCP ----------------------------------------------------------------
# Sorts before the flake's 20-wwan.network, so networkd prefers this one.  It is
# in /run, so a reboot removes it and the declarative config takes over.
say "== asking networkd for a lease (a /run .network file overrides the flake's)"
mkdir -p /run/systemd/network
cat >"$NET_CONF" <<EOF
[Match]
Name=$iface

[Network]
DHCP=ipv4
IPv6AcceptRA=no

[DHCPv4]
UseDNS=no
UseRoutes=yes
RouteMetric=50
EOF

networkctl reload 2>/dev/null || systemctl restart systemd-networkd
networkctl reconfigure "$iface" 2>/dev/null || true

gateway=""
for _ in $(seq 1 30); do
  gateway=$(ip -4 route show default dev "$iface" proto dhcp 2>/dev/null |
    sed -n 's/.* via \([0-9.]*\).*/\1/p' | head -n 1 || true)
  [ -n "$gateway" ] && break
  sleep 1
done
[ -n "$gateway" ] ||
  die "no DHCP default via $iface after 30 s — check: networkctl status $iface ; journalctl -u systemd-networkd -n 30"

# --- 5. Routes, rules and NAT -------------------------------------------------
say "== installing routes and policy rules (gateway $gateway)"
ip -4 route replace default via "$gateway" dev "$iface" metric 50
ip -4 route replace default via "$gateway" dev "$iface" table 300 metric 100
while ip rule del pref 50 2>/dev/null; do :; done
ip rule add pref 50 fwmark 1 lookup 300
ip rule add pref 50 fwmark 2 lookup 300
ip route flush cache

say "== making sure the hotspot egress is NATed"
if nft list chain ip routerNat srcnat >/dev/null 2>&1; then
  if ! nft list chain ip routerNat srcnat | grep -q "oifname \"$iface\""; then
    nft add rule ip routerNat srcnat oifname "$iface" ip saddr 10.3.0.0/16 masquerade
    nft add rule ip routerNat srcnat oifname "$iface" ip saddr 192.168.2.0/24 masquerade
    nft add rule ip routerNat srcnat oifname "$iface" ip saddr 100.64.0.0/10 masquerade
    say "   added the masquerade rows to ip routerNat srcnat"
  else
    say "   already present in ip routerNat srcnat"
  fi
else
  say "   WARNING: ip routerNat is missing — installing a bare NAT table."
  say "   The box has no firewall in that state; fix the flake when you can."
  nft list table ip emergencyNat >/dev/null 2>&1 || nft add table ip emergencyNat
  nft list chain ip emergencyNat postrouting >/dev/null 2>&1 ||
    nft add chain ip emergencyNat postrouting '{ type nat hook postrouting priority srcnat; }'
  nft add rule ip emergencyNat postrouting oifname "$iface" ip saddr 10.3.0.0/16 masquerade
  nft add rule ip emergencyNat postrouting oifname "$iface" ip saddr 192.168.2.0/24 masquerade
fi

# --- 5b. Optionally move the AAISP session onto the hotspot ------------------
# This is what `wan-backup-route` does by itself while the hub is dead; here it is
# explicit, for when the units are stopped.  `try-restart` re-dials over the new
# route; the old session's packets change source and the LNS drops them.
if [ "$TUNNEL" -eq 1 ]; then
  say "== routing the AAISP endpoint out $iface and re-dialling the tunnel"
  ip -4 route replace 194.4.172.12 via "$gateway" dev "$iface" metric 5
  ip route flush cache
  systemctl try-restart xl2tpd-aaisp.service 2>/dev/null || true
  say "   the tunnel is dialling over the hotspot; watch for ppp0:  ip -br addr show ppp0"
fi

# --- 6. Prove it --------------------------------------------------------------
say ""
say "== state"
ip -4 route show default
ip -4 route show default table 300
ip -4 rule show | sed -n '1,6p'
say ""
if curl -fsS -m 10 -o /dev/null https://one.one.one.one; then
  say "router → internet: OK"
else
  say "router → internet: FAILED — the association or the lease is the problem, not NAT."
fi
say ""
say "The LAN is on $iface now, by the hand-built path."
say "A reboot ends it and the flake's declarative config comes up instead."
if [ "$TUNNEL" -eq 1 ]; then
  say "The AAISP session is dialling over the hotspot: public /29, /48 and the"
  say "tailnet control plane come back when ppp0 is up (ip -br addr show ppp0)."
fi
say "To hand it back once the flake is fixed, do not reverse this by hand."
say "Run this on gw instead:"
say ""
say "  just wan-backup-handback"
say ""
say "It has to remove $NET_CONF (which is written with UseRoutes=no, so the"
say "reconciler has no DHCP route to read while it exists), delete the metric-50"
say "default and the two pref-50 rules, restart wan-backup-wifi, start"
say "wan-backup-route **and its .timer** — the service is a oneshot, so starting it"
say "alone installs one static route with nothing watching it, which is how the box"
say "sat with no reconciler for 45 minutes on 2026-11-07 and needed this script"
say "again — and re-dial the AAISP session, because stopping the units is what"
say "leaves ppp0 down (no IPv6, no /29, no inbound) while the LAN looks fine."
