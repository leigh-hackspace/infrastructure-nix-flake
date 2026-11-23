# JUST — Home Assistant, and the legacy 10.3.1.30 forwarder that names its clients.
#
# Part of the justfile: the root justfile imports these, so `just` still
# resolves every recipe from the repo root and `just --list` still shows
# them.  Split by job (AGENTS.md rule 26), so no file here is over 1000
# lines, and the guard that enforces that rule now sees justfiles.

# --- Home Assistant, and the legacy 10.3.1.30:8123 forwarder ----------------------
#
# Home Assistant moved from `apps1` to services1 on 2026-11-16: the container is
# machines/services1/services/home-assistant.nix and the app answers 127.0.0.1:8123
# here.  10.3.1.30:8123 still answers, because `apps1` keeps that address (three
# front-doored origins, CUPS and its own Prometheus still live there, so the VM
# cannot be retired and the address cannot move the way authentik's .36 did) and a
# socat forwarder on that box now relays to services1 **and logs every source
# address**.  That log is the whole point: it is how we find out which devices
# hardcoded the old IP.  The Space API moved here the same afternoon, which removed
# the last known client, so as of 2026-11-16 16:30 the forwarder has been idle and
# its 30-day retirement clock is running.  Read the design, the client map, the unit
# text and the retirement criterion in docs/home-assistant-on-services1-2026-11-16.md.
#
# Run it on services1 (read `hostname` first — rule 27).  It reads two things off
# apps1 and changes nothing: the forwarder's journal (who used the old address
# *after* the move) and a pre-move SYN capture started before the switch (who used
# it while the app still lived there, which is the baseline this comparison needs).
#
# Two ssh facts this recipe encodes because they are otherwise invisible: root@apps1
# needs `-o IdentitiesOnly=yes` — apps1 is deliberately not in common/hosts.nix, so
# rule 32's system ssh_config block does not reach it and ssh otherwise walks
# through to "Too many authentication failures" — and apps1's host key is pinned
# only in the operator's known_hosts, never in /etc/ssh/ssh_known_hosts.

ha-legacy-watch days="7":
    #!/usr/bin/env bash
    set -euo pipefail
    APPS1="10.3.1.30"
    SSH=(ssh -o BatchMode=yes -o ConnectTimeout=5 -o IdentitiesOnly=yes
         -o StrictHostKeyChecking=yes -i "$HOME/.ssh/agent-hop-key" "root@${APPS1}")
    tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT

    # One line per TCP connection: `accepting connection from AF=2 <ip>:<port> on …`.
    if ! "${SSH[@]}" "journalctl -u ha-legacy-proxy --since '{{days}} days ago' --no-pager -o short-iso" \
            >"$tmp/proxy" 2>/dev/null || [ ! -s "$tmp/proxy" ]; then
        echo "no ha-legacy-proxy output in the last {{days}} days — is the unit running?"
        echo "    ssh -i ~/.ssh/agent-hop-key -o IdentitiesOnly=yes root@${APPS1} 'systemctl status ha-legacy-proxy'"
    fi

    # The pre-move baseline: SYN-only, so one line per connection attempt.
    "${SSH[@]}" 'cat /root/ha8123-syn.log 2>/dev/null' >"$tmp/baseline" || true

    echo "=== sources of 10.3.1.30:8123, last {{days}} days, through the forwarder ==="
    sed -nE 's/.*accepting connection from AF=2 ([0-9.]+):[0-9]+.*/\1/p' "$tmp/proxy" \
        | sort | uniq -c | sort -rn >"$tmp/now" || true
    # File lines are `<count> <ip>`, which is what `uniq -c` prints: the address is $2.
    if [ -s "$tmp/now" ]; then
        awk '{print $2, $1}' "$tmp/now" | while read -r ip n; do
            case "$ip" in
                127.*)    tag="apps1 ITSELF — loopback. Its Prometheus was repointed to 10.3.1.20 on 2026-11-16, so anything still arriving here is something else local to that box." ;;
                10.3.1.20) tag="services1 — this box. Nothing here should dial the old address since the move; the usual cause of a line here is a curl typed at it during an interactive check. If it persists, re-read the proxyPass in machines/services1/http.nix and the --hass-url in machines/services1/services/slack-agent.nix." ;;
                172.17.*) tag="a CONTAINER ON apps1's own docker bridge — NOT a device on the LAN, so it will never have a lease. Both known ones have now moved to services1 (hackspace-api 2026-11-16, prometheus's hass job repointed), so a line here is either something still on that VM that names HA or the old address, or a stale address from before a container restart. Map them with: docker network inspect bridge" ;;
                *)        tag="*** A REAL CLIENT *** — this device hardcoded 10.3.1.30:8123 and needs retargeting to https://ha.int.leighhack.org" ;;
            esac
            printf '%-15s %7s  %s\n' "$ip" "$n" "$tag"
        done
        echo
        echo "Real clients (excluding apps1, this box, and apps1's own containers):"
        curl -s -m 5 "http://10.3.1.1:8091/api/leases" >"$tmp/leases" || true
        awk '$2 !~ /^127\./ && $2 !~ /^172\.17\./ && $2 != "10.3.1.20" {print $2}' "$tmp/now" | sort -u | while read -r ip; do
            name=$(python3 -c 'import json,sys; L=json.load(open(sys.argv[1])).get("leases",[]); m=[r for r in L if r["ip"]==sys.argv[2]]; print(((m[0]["name"] or "(lease, no name)")+"  mac "+m[0]["mac"]) if m else "no lease — a static reservation in common/dns-records.nix, or off-DHCP")' "$tmp/leases" "$ip" 2>/dev/null || echo "leases API unreadable — see http://10.3.1.1:8091/leases")
            printf '  %-15s %s\n' "$ip" "$name"
        done
    else
        echo "  (nothing yet — zero connections is the goal, but see the baseline before believing it)"
    fi

    echo
    echo "=== baseline: who dialled 10.3.1.30:8123 BEFORE the move, from /root/ha8123-syn.log ==="
    # tcpdump prints `HH:MM:SS.ffffff IP <src>.<port> > 10.3.1.30.8123: Flags [S] …`
    grep -oE 'IP [0-9.]+(\.[0-9]+)? > 10\.3\.1\.30\.8123' "$tmp/baseline" 2>/dev/null \
        | awk '{print $2}' | sed -E 's/\.[0-9]+$//' | sort | uniq -c | sort -rn | head -20 \
        || echo "  (empty — the capture is only as long as it has been running; a daily or weekly client needs days to show up)"
