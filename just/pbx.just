# JUST — the intercom, and the test that rings a real handset.
#
# Part of the justfile: the root justfile imports these, so `just` still
# resolves every recipe from the repo root and `just --list` still shows
# them.  Split by job (AGENTS.md rule 26), so no file here is over 1000
# lines, and the guard that enforces that rule now sees justfiles.

# --- The intercom (machines/services1/services/pbx.nix) --------------------------------
#
# The phone system moved off the `mercury` Proxmox VM here on 2026-11-16, and it answers
# 10.3.1.166 (a second address on vlan225) because that IP is written into two Snom
# handsets. `asterisk -rx` needs root because the CLI socket belongs to the `asterisk`
# account, so every line here is sudo'd; run it on services1 (read `hostname` first —
# rule 27). Design, receipts and the retirement of the VM: docs/pbx-on-services1-2026-11-16.md.

# "Is the intercom up, and are the two handsets on it?" — one command, no credentials.
pbx-status:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "=== unit ==="
    systemctl is-active asterisk || echo "  asterisk is NOT running (systemctl status asterisk)"
    echo "=== the secret the phones authenticate against ==="
    if sudo test -s /var/lib/asterisk/pjsip_auth_secrets.conf; then
        echo "  rendered: $(sudo grep -c '^type=auth' /var/lib/asterisk/pjsip_auth_secrets.conf) auth sections (expected 5)"
    else
        echo "  MISSING: /run/secrets/pbx_pjsip_auth never reached the box — preStart refuses to start without it"
    fi
    echo "=== registered handsets ==="
    sudo asterisk -rx "pjsip show contacts" 2>/dev/null | sed -n '/^  Contact/p' | head -8
    echo "=== what is loaded ==="
    printf "  endpoints=%s  aors=%s  auths=%s  (5 each: 1 Frontspace, 2 Workshop, 3 test, 123 Raccoon, 1337 phyu)\n" \
        "$(sudo asterisk -rx 'pjsip show endpoints' 2>/dev/null | grep -cE '^[[:space:]]+Endpoint:[[:space:]]+[0-9]' || true)" \
        "$(sudo asterisk -rx 'pjsip show aors' 2>/dev/null | grep -cE '^[[:space:]]+Aor:[[:space:]]+[0-9]' || true)" \
        "$(sudo asterisk -rx 'pjsip show auths' 2>/dev/null | grep -cE '^[[:space:]]+Auth:[[:space:]]+[0-9]' || true)"
    printf "  ACL denials since this start: %s  (an endpoint ACL that does not resolve fails CLOSED and looks like a wrong password — see docs/pbx-on-services1-2026-11-16.md)\n" \
        "$(sudo journalctl -u asterisk --since "$(systemctl show asterisk -p ActiveEnterTimestamp --value)" --no-pager 2>/dev/null | grep -c 'invalid ACL' || true)"
    echo "=== the soundboard: dial one of these from a handset ==="
    grep -aE '^; [a-z].*plays custom/' /etc/asterisk/extensions.conf \
        | sed -E 's/^; ([^:]+): plays custom\/([^ ]+) to the caller, then rings (.*)$/  \1  \2 -> rings \3/' | sed 's/^[[:space:]]*/  /'
    echo "=== last calls this week (CDR lands in the journal) ==="
    sudo journalctl -t asterisk --since "-7 days" --no-pager -o short-iso 2>/dev/null \
        | grep -aiE "Hangup|Dial.*PJSIP" | tail -6 || echo "  no calls recorded in the journal window"

# End-to-end test that goes through the path a person uses: it rings a handset and plays a
# soundboard file into the call.  `just pbx-test phone=2 sound=w95` — the Workshop phone
# physically rings, which is the only proof that matters for a device you cannot see.
pbx-test phone="2" sound="w95":
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Originating a call to PJSIP/{{phone}} playing custom/{{sound}} — pick it up."
    sudo asterisk -rx "channel originate PJSIP/{{phone}} application Playback custom/{{sound}}"
    sleep 1
    sudo asterisk -rx "pjsip show contacts" | sed -n '/^  Contact/p' | head -4
