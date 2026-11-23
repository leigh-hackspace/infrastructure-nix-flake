# JUST — the UPS on gw: what it says, the notice that goes out first, the drills.
#
# Part of the justfile: the root justfile imports these, so `just` still
# resolves every recipe from the repo root and `just --list` still shows
# them.  Split by job (AGENTS.md rule 26), so no file here is over 1000
# lines, and the guard that enforces that rule now sees justfiles.

# --- the UPS, the outage, and the notice that goes out first -------------------
#
# **Run these on `gw`.** They talk to NUT's units, to `ups-watchdog`'s units, and
# to the UPS on this box's USB port; from anywhere else they fail on a missing
# socket, which is not a fact about your power.  Policy — the grace period, the
# notice texts, who is powered off in what order — lives in `machines/gw/ups.nix`;
# `docs/ups-and-fleet-shutdown.md` has the reasoning, the battery maths, and what
# has not been verified on a live battery yet.
#
# The three that matter on a dark day: the UPS says `OB`, `ups-observing` fires,
# and `ups-notice.service` walls the building with the countdown in it.  Five
# minutes later `ups-fleet-shutdown.service` broadcasts again, waits its notice
# window, and starts powering servers off over the hop key — gw last, locally.

# What the UPS says right now, and whether the chain that reads it is up.
#
# Three layers, three lines, and only the first one is a fact about the mains:
# `upsdrv` (the HID driver holding /dev/hidraw4), `upsd` (the server on loopback
# :3493) and `upsmon` (the client whose notifications start the countdown).
# `ups-watchdog status` exits 0 on battery, 10 on mains, 20 unreadable — so the
# command itself is a test, and a dead driver reads as 20 rather than as "fine".
ups-status:
    #!/usr/bin/env bash
    set -uo pipefail
    for u in upsdrv upsd upsmon; do
        printf '%-9s %s\n' "$u" "$(systemctl is-active $u.service 2>&1)"
    done
    echo
    sudo ups-watchdog status --ups apc@localhost --upsc-bin "$(command -v upsc)"
    echo
    echo "exit 0 = on battery, 10 = mains fine, 20 = cannot tell (see the units above)"

# Broadcast a notice to every server, for real, without touching a power cable.
#
# This is the drill that proves the *message* path: the hop key, ssh, `wall` on
# the far side, and this box's own terminals.  It writes the notice file the same
# way `ups-trigger` does (line 1 is the headline, the rest is the body) and starts
# the same unit, so what you see on every server is what an outage will say —
# which is the only way to find out that a server cannot be told *before* it needs
# to be.  Non-zero exit means somebody was missed; `journalctl -u ups-notice` says
# who.
ups-notice-test:
    #!/usr/bin/env bash
    set -euo pipefail
    printf '%s\n' \
        'UPS NOTICE TEST (not an outage)' \
        'This is the gw UPS broadcast path being tested by hand.' \
        'Nothing is happening to your power; the sender is ups-watchdog on gw.' \
        'If you can read this on a terminal, an outage notice will reach you too.' \
        | sudo tee /run/ups-watchdog/notice > /dev/null
    sudo systemctl start ups-notice.service
    echo "=== sent. On each server you should see it via: dmesg/wall; here:"
    systemctl status ups-notice.service --no-pager --lines 20 || true

# The whole shutdown sequence, run for real, except that it switches nothing off.
#
# `ups-fleet-shutdown-dry.service` is a second unit rather than a flag an operator
# remembers to pass at 03:00, and nothing but a human starting it refers to it — no
# NUT event can.  It prints the argv it would run, including the notice, and
# touches no machine: `--dry-run` skips the hops *and* the sleeps, so this is a
# command you can run twice in a row while reading the output.
ups-drill:
    #!/usr/bin/env bash
    set -euo pipefail
    sudo systemctl start ups-fleet-shutdown-dry.service
    journalctl -u ups-fleet-shutdown-dry.service --no-pager -n 40

# The one drill that other people can see: a real `ONBATT` through `upssched`, and
# the `ONLINE` that cancels it.  Between the two, every server's terminals are
# walled with `POWER OUTAGE DETECTED` and the 300 s countdown is genuinely armed —
# which is the only part of this design the dry unit cannot prove, because the dry
# unit starts `ups-watchdog` directly and this starts it the way NUT does: upsmon's
# NOTIFYCMD, upssched's rules, CMDSCRIPT, unit.
#
# Say so before running it.  And do not run the two halves by hand and stop after
# the first: an uncancelled ONBATT ends with the fleet powering off.  The recipe
# therefore runs the cancel whatever the notice did (no `set -e` in the one-liner,
# `;` not `&&`) and finishes by asking whether the daemon is still alive, which is
# the observable proof that nothing is counting down — `upssched` self-daemonises and
# exits when its timer queue empties, so a live process here means an armed timer.
#
# `NUT_CONFPATH` is not decoration: it is how the NixOS units tell NUT where its
# config is, and without it `upssched` reads the package's own `etc/` and fails with
# `Can't open …/etc/upssched.conf` — measured, and it looks exactly like a feature
# that was never wired up.  §6 of docs/ups-and-fleet-shutdown.md has the contract
# this recipe is written against, and the throwaway-config proof of it.
ups-drill-notify:
    @echo "typing this walls every server: services1, services2, aibox, and this box"
    @sudo sh -c 'NUT_CONFPATH=/etc/nut NUT_STATEPATH=/var/lib/nut UPSNAME=apc NOTIFYTYPE=ONBATT upssched; echo "--- ONBATT rc=$?"; sleep 5; systemctl show -p Result -p ExecMainStatus --value ups-notice.service; NUT_CONFPATH=/etc/nut NUT_STATEPATH=/var/lib/nut UPSNAME=apc NOTIFYTYPE=ONLINE upssched; echo "--- ONLINE rc=$?"; sleep 3; echo "--- anything still counting down?"; pgrep -a upssched || echo "no upssched daemon: nothing is armed"'
    journalctl -u ups-notice -t ups-watchdog --no-pager -n 25 || true

# How the outage policy is configured right now, as deployed.
#
# The countdown, the cancel-on-restore and the event→notice wiring are all in one
# generated file, and it is generated — so read the deployed copy rather than
# reasoning about the module (AGENTS.md rule 10).  `diff` it against
# `machines/gw/ups.nix` to see whether the box agrees with the tree.
ups-events:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "=== /etc/nut/upssched.conf (the countdown, and what each event does)"
    grep -vE '^\s*(#|$)' /etc/nut/upssched.conf
    echo
    echo "=== the three units an outage starts"
    for u in ups-notice ups-fleet-shutdown ups-fleet-shutdown-dry; do
        printf '%-26s %s\n' "$u" "$(systemctl is-enabled $u.service 2>&1) / $(systemctl is-active $u.service 2>&1)"
    done
    echo
    echo "=== anything counting down right now?"
    pgrep -a upssched || echo "no upssched daemon: no timer armed (that is the idle state)"
    echo
    echo "=== the last thing each of them did"
    journalctl -u ups-watchdog -u ups-notice -u ups-fleet-shutdown --no-pager -n 30 || true
