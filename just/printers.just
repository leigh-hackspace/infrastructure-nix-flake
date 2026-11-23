# JUST — the two paper printers the Pi Room image prints to.
#
# Part of the justfile: the root justfile imports these, so `just` still
# resolves every recipe from the repo root and `just --list` still shows
# them.  Split by job (AGENTS.md rule 26), so no file here is over 1000
# lines, and the guard that enforces that rule now sees justfiles.

# --- the two PAPER printers, and the Pi Room image that prints to them ----------
#
# `just paper-printers` — run from any flake box, gw included: it is curls and one
# read of the router's lease table, nothing evaluates a machine and nothing builds,
# so it stays legal where the build-shaped recipes do not.
#
# What it is for: "can the Pi Room print, and to which of the two".  The rows come
# OUT of machines/pi-room-sys/printers-data.nix — evaluated, not re-typed here — so
# the verb cannot report a device URI the image does not have, which is the drift that
# otherwise survives quietly (a table edited and a probe still asking the old address
# reads the same as the gatus checks that pointed at names that never existed).  The
# eval is one DATA file, the same `-f` one-file pattern common/check-printer-states.nix
# runs by; a file is not a directory, so there is nothing for AGENTS.md rule 9's
# `../crates` trap to resolve.
#
# What it is NOT, and this is the whole reason the verb ends by saying it: none of it
# is the proof.  An endpoint that answers, a queue that is registered and an
# `lpstat` that is green is the same shape as the MQTT name that 404ed while every
# HTTP sensor kept counting (docs/mqtt-int-legacy-answer-2026-11-02.md) — the failure
# this rule exists to prevent is a queue that exists, enables, and prints nothing, and
# every one of these requests passes through that.  The only proof that counts is a
# sheet of paper out of the machine; docs/paper-printers.md has the client-side
# commands (boot the image, ssh into the guest, print inside it).
#
# The IPP request body is the 10-byte Get-Printer-Attributes header — version 1,
# opcode 1, request id 1, group length 0 — which is the smallest IPP message an IPP
# server will answer, and answering at all is the fact being tested.

paper-printers:
    #!/usr/bin/env bash
    set -euo pipefail

    printf '\\x01\\x01\\x00\\x00\\x00\\x01\\x00\\x00\\x00\\x00' >/tmp/pp-ipp.req

    if ! rows=$(nix eval --impure --raw --expr 'let t = import ./machines/pi-room-sys/printers-data.nix; in builtins.concatStringsSep "\n" (map (n: "${n}\t${t.${n}.record}\t${t.${n}.deviceUri}\t${builtins.toString t.${n}.default}") (builtins.attrNames t))' 2>/tmp/pp-eval.err); then
        echo "== the printer table does not evaluate — nothing below is checked, and the error names why"
        sed -n '1,14p' /tmp/pp-eval.err
        exit 1
    fi

    echo "== the rows $(printf '%s\n' "$rows" | wc -l), read out of the table itself"
    printf '%s\n' "$rows" | while IFS=$'\t' read -r queue record uri dflt; do
        [ -n "$queue" ] || continue
        # The scheme is CUPS's vocabulary (`ipp`, `ipps`); curl only speaks http, and
        # the fact being asked is about the endpoint behind the scheme.
        url="http://${uri#*://}"
        host="${url#http://}"; host="${host%%/*}"

        # Live lease, read from the router: a printer that is switched off still holds
        # its reservation, so an address that answers nothing and leases is a device,
        # not an outage — and this is what tells the two apart.  The router's own API
        # is tried first because it is the shortest read, and the front door is the
        # fallback for a box whose VLAN cannot reach 10.3.1.1 directly.
        lease="$(curl -sG -m 5 "http://10.3.1.1:8091/api/leases" --data-urlencode "q=$record" 2>/dev/null || true)"
        [ -n "$lease" ] || lease="$(curl -sG -m 5 "https://network-info.int.leighhack.org/api/leases" --data-urlencode "q=$record" 2>/dev/null || true)"
        leaseTag="leases API unreadable from here — getent/dns-records instead"
        if [ -n "$lease" ]; then
            m=$(printf '%s' "$lease" | grep -oE '"matched":[0-9]+' | head -1 | cut -d: -f2)
            x=$(printf '%s' "$lease" | grep -oE '"expired":[0-9]+' | head -1 | cut -d: -f2)
            st=$(printf '%s' "$lease" | grep -oE '"stale":[^,]*' | head -1 | cut -d: -f2)
            leaseTag="lease matched=${m:-?} expired=${x:-?}"
            # Rule 29: the lease table is a channel, and it can die while still answering.
            # A stale read is not a quiet network, it is a frozen one.
            case "$st" in
                null) leaseTag="$leaseTag, channel live" ;;
                *) leaseTag="$leaseTag, STALE TABLE ($st) — trust the probe, not this" ;;
            esac
        fi

        # The three requests that tell the two endpoint shapes apart, plus the answer.
        get=$(curl -s -m 6 -o /dev/null -w '%{http_code}' "$url" 2>/dev/null || true)
        opt=$(curl -s -m 6 -o /dev/null -w '%{http_code}' -X OPTIONS "$url" 2>/dev/null || true)
        srv=$(curl -s -m 6 -i -X OPTIONS "$url" 2>/dev/null | grep -oiE '^(Server|Allow): .*' | head -2 | tr '\n' '|' || true)
        post=$(curl -s -m 6 -o /tmp/pp-out.bin -w '%{http_code} %{size_download}' -X POST -H 'content-type: application/ipp' --data-binary @/tmp/pp-ipp.req "$url" 2>/dev/null || true)

        verdict="nothing answers — powered off, off-network, or the wrong port"
        if [ "$get" = "405" ] || [ "${post%% *}" = "200" ]; then
            verdict="IPP-native endpoint at this path (GET 405 = it is there, POST answers)"
        elif [ "$opt" = "200" ] && printf '%s' "$srv" | grep -qi 'CUPS'; then
            verdict="CUPS-shaped share: the endpoint is there, but the queue NAME inside it cannot be read from the network"
        elif [ "$get" = "404" ] || [ "$opt" = "404" ]; then
            verdict="the device's HTTP server answers but this path is not its endpoint — wrong scheme/path, wrong row"
        fi

        printf '%-8s %-10s %-44s %s\n' "$queue" "$record" "${uri##*://}" "$dflt"
        printf '         lease: %s\n' "$leaseTag"
        printf '         GET %s | OPTIONS %s | IPP POST %s %s\n' "$get" "$opt" "$post"
        printf '         server says: %s\n' "${srv:-nothing}"
        printf '         verdict: %s\n' "$verdict"
        # What the row claims that this probe can still decide: the path it dials is the
        # path that answers (by construction — the probe dials the row's own URI), and
        # the scheme is one `lpadmin -v` accepts.  It used to also check that an
        # IPP-native endpoint was spelled `net+ippt`, which was wrong in both halves:
        # `net+ippt` is not a device-uri scheme (CUPS 2.4.19 refuses it with `lpadmin:
        # Bad device-uri scheme`, and that one row took the whole scheduler down on a
        # booted client, 2026-11-21 — docs/paper-printers.md).
        printf '         matches the row? '
        case "$verdict" in
            IPP-native*|CUPS-shaped*)
                case "$uri" in
                    ipp://*|ipps://*) echo "yes — the path this row dials is the endpoint, and the scheme is one lpadmin accepts" ;;
                    *) echo "NO — the endpoint answers, but this scheme is not one lpadmin accepts: fix ./printers-data.nix's scheme" ;;
                esac ;;
            *) echo "cannot be decided from here — see docs/paper-printers.md's open items" ;;
        esac
    done

    echo
    echo "== what is NOT answered here, in one line each"
    echo "   * that a page comes out. Every request above passes through a queue that prints nothing."
    echo "     docs/paper-printers.md: boot the image (just pxe-client-uefi, on aibox), ssh into the"
    echo "     guest, print inside it — the guest is another host to ssh into, so this is the client's"
    echo "     own answer rather than a host's guess about it."
    echo "   * that the queue NAME the image dials exists. On a CUPS-shaped share nothing on the network"
    echo "     can enumerate it: that server sends no body on GET /printers/, /admin/ or /jobs/, refuses"
    echo "     POST, and OPTIONS answers 200 for every name. It is one line in ./printers-data.nix and"
    echo "     docs/follow-ups.md carries where the truth is."
    echo "   * what the switch of the image cost: docs/paper-printers.md flags the squashfs size and the"
    echo "     aibox switch time as unmeasured, and says to read them rather than assume they are free."
