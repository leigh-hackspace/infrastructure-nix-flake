# JUST — the router’s WAN, and handing back from break-glass.
#
# Part of the justfile: the root justfile imports these, so `just` still
# resolves every recipe from the repo root and `just --list` still shows
# them.  Split by job (AGENTS.md rule 26), so no file here is over 1000
# lines, and the guard that enforces that rule now sees justfiles.

# --- the router's WAN: handing back from break-glass -------------------------
#
# `machines/gw/wan-backup-emergency.sh` is the lifeline when the uplink is gone.
# It stops `wan-manager`, tries every USB-Ethernet, wireless and plain-Ethernet
# candidate it can find, writes a `/run` network file, types a metric-50 default
# in the main table and tables 200/300, puts two `ip rule`s at pref 50 and adds
# NAT. What it leaves behind is *not* recoverable by restarting one thing, so
# this recipe is the handover and it is idempotent: running it on a box the flake
# already owns changes nothing.
#
# Run it on gw (`hostname` first — a hop to the box you are already on succeeds
# silently).
wan-backup-handback:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "$(hostname)" != "gw" ]; then
        echo "refusing: this repairs gw's live routing, so run it on gw" >&2
        exit 1
    fi

    # 1. Start the daemon while the emergency metric-50 route is still in place.
    #    Do NOT touch networkd yet: a `networkctl reload` can flush the promoted
    #    routes, and removing the emergency route before there is a replacement
    #    is how the 2026-11-09 handbacks took the LAN down.
    sudo systemctl start wan-manager

    # 2. Wait for the daemon to report a non-tunnel egress and for internet to
    #    work *while the emergency route is still present*. Up to 90 s.
    ok=0
    for _ in $(seq 1 18); do
        sleep 5
        state=$(sudo sed -n 's/.*"state":"\([^"]*\)".*/\1/p' /run/wan-manager/status.json 2>/dev/null | head -1)
        sel=$(sudo sed -n 's/.*"selected":{"role":"egress","interface":"\([^"]*\)".*/\1/p' /run/wan-manager/status.json 2>/dev/null | head -1)
        if [ -n "${sel:-}" ] && curl -fsS -m 4 -o /dev/null https://one.one.one.one 2>/dev/null; then
            echo "daemon egress: $sel (state ${state:-?})"
            ok=1
            break
        fi
        echo "waiting for wan-manager (state ${state:-?}, egress ${sel:-none})"
    done
    if [ "$ok" -ne 1 ]; then
        echo "FAILED: wan-manager has no egress; leaving the emergency path untouched." >&2
        echo "Do not remove the metric-50 route. Diagnose: journalctl -u wan-manager -n 80" >&2
        exit 1
    fi

    # 3. Only now remove the hand-built metric-50 default. No `networkctl
    #    reload` and no touching /run/systemd/network: those are what flushed
    #    the daemon's route on the first attempt. The daemon's metric-100 route
    #    is already installed and proven.
    for dev in $(ip -o link show | awk -F': ' '{print $2}' | cut -d@ -f1); do
        sudo ip -4 route del default dev "$dev" metric 50 2>/dev/null || true
        sudo ip -4 route del default dev "$dev" table 200 metric 50 2>/dev/null || true
        sudo ip -4 route del default dev "$dev" table 300 metric 50 2>/dev/null || true
    done
    while sudo ip rule del pref 50 2>/dev/null; do :; done
    sudo ip -4 route flush table 300 2>/dev/null || true
    sudo ip route flush cache

    # 4. The emergency network files and supplicant copy are now inert (the
    #    daemon's 05- file wins) but remove them without reloading networkd.
    sudo find /run/systemd/network -maxdepth 1 \( -name '10-wan-emergency-*.network' -o -name '10-wan-backup.network' \) -delete 2>/dev/null || true
    sudo rm -f /run/wpa-backup.conf

    # 5. Verify the daemon's own route is what remains and that the path works.
    sleep 2
    if ! curl -fsS -m 8 -o /dev/null https://one.one.one.one; then
        echo "FAILED: the daemon route is not carrying traffic. Re-run the emergency script:" >&2
        echo "  sudo /etc/wan-backup-emergency.sh --force" >&2
        exit 1
    fi
    echo
    ip -4 route show default
    echo
    sudo cat /run/wan-manager/decision 2>/dev/null || true
    systemctl is-active wan-manager.service
    echo "handback complete: wan-manager is the WAN, no metric-50 route remains"
