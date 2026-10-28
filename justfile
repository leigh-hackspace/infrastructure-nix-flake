# https://just.systems

default:
    @ {{ just_executable() }} --list --justfile {{ justfile() }} --unsorted

SOPS_KEY_FILE := "{{env_var_or_error('HOME')}}/.config/sops/age/keys.txt"

# Hard reboot via the sysrq trigger — no sync of the running system beyond
# `sync`, no unmount, no orderly shutdown.  Named so it is unlikely to be hit by
# tab completion where `reboot`/`switch` are what you meant.
hard-reboot:
    sync
    sudo bash -c "echo b > /proc/sysrq-trigger"

# Both recipes chain `sudo nixos-confirm` in the same shell invocation: every
# machine in the flake sets system.autoRollback.enable = true, so the auto-rollback
# timer rolls back to the last confirmed generation within a minute or two of a
# switch.  Confirming in a later session can land _after_ the rollback and mark
# the old generation good, which defeats the point (see AGENTS.md, GOLDEN RULE).
boot:
    sudo nixos-rebuild boot --flake . && sudo nixos-confirm

switch:
    sudo nixos-rebuild switch --flake . && sudo nixos-confirm

# --- where builds may run ----------------------------------------------------
#
# The router must not compile anything, and since 2026-10-21 it cannot: gw delegates every
# build to aibox and sets `nix.settings.max-jobs = 0`, which is the half that removes the
# silent local fallback (common/build-farm.nix documents that `--max-jobs 0` "does not enable
# delegation, it removes the local fallback").
#
# The guard stays anyway, and the reason is not compilation: **evaluation still happens on
# the box you are standing on**, and a 4-core router is also the network's resolver.
# `check-eval` and `fmt` are eval/format only and stay allowed there; `check` is allowed too
# but skips its flake step here, with the reason printed (see that recipe).  Every
# build-shaped recipe below depends on this guard — `deploy`/`switch` deliberately do not:
# they are how a machine installs itself, and the build is aibox's either way.  What it replaced — the devShell that *was*
# a build, and the split toolchain that closed it — the full allowed/refused table, and what to
# read if rustc ever appears on the router again: docs/nix-dev-workflow.md,
# "Never build on the router".
#
# Build on aibox (the network's builder).  If you genuinely need to build on the
# router, say so explicitly.  Deploying the router is *not* a build on the router and is
# not guarded: `just switch` here, or `just deploy gw` from anywhere, evaluates on gw and
# compiles on aibox — which is also what every other box does for itself (`just deploy`).
_build-host-guard:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "$(hostname)" = "gw" ] && [ -z "${INFRA_ALLOW_BUILD_ON_ROUTER:-}" ]; then
        echo "refusing: this is gw, the network's router — builds delegate to aibox and evaluation costs this box." >&2
        echo "  Run this from aibox or services1 instead (AGENTS.md, 'Don't build on the router')." >&2
        echo "  Nothing here compiles locally either way: nix.settings.max-jobs is 0, so there is no fallback to fall back to." >&2
        echo "  Override with INFRA_ALLOW_BUILD_ON_ROUTER=1 only if you really mean it." >&2
        exit 1
    fi

# --- deploying a machine: from that machine ------------------------------------
#
# `just switch` is this box.  `just deploy <machine>` is any box: it sshs to the target,
# brings *the target's own checkout* to the commit this box is on, and runs
# `nixos-rebuild switch` **there** — so the machine that evaluates, activates and confirms is
# the machine being deployed.  The compilation is still aibox's (the target's daemon delegates
# to it, `common/build-farm.nix`); the point is that no box is the fleet's deploy host.
#
# This replaces the pair that made services1 the centre of the fleet: `switch-gw` built *here*
# and pushed the closure to the router with `--target-host`, and `sync-gw` rsynced this whole
# working tree over the router's checkout with `--delete`.  Both were written when gw had no
# builder and no GitHub key.  Neither is true any more: gw delegates to aibox and holds
# `max-jobs = 0` (2026-10-21), so it builds its own toplevel without compiling anything, and
# the commit travels as a git fetch from whichever box ran the recipe rather than as an rsync
# of a working tree — which is also why `sync-gw`'s `.git/config`-eating bug
# (`docs/follow-ups.md`) is closed by deleting the recipe instead of by another exclusion.
#
# What it costs: the target evaluates its own configuration, and the target pays for it.  On
# gw that is seconds of the network's resolver per deploy, which is the honest price of not
# making another machine the deploy host; nothing compiles there either way (`max-jobs = 0`).
#
# Run from any box that has the commit.  On the target itself the recipe degenerates to
# `just switch`, so `just deploy gw` does the right thing wherever it is typed.
deploy machine:
    #!/usr/bin/env bash
    set -euo pipefail
    machine={{ quote(machine) }}
    case "$machine" in
        services1|services2|aibox|gw) ;;
        *) echo "unknown machine: $machine — expected services1, services2, aibox or gw" >&2; exit 2 ;;
    esac
    # The address is `common/hosts.nix`, not a second copy of it — one table of addresses,
    # which is also what `just known-hosts` reads for the same job.  `gw` is the hostname and
    # `router` is the table's key for it (the box has been `gw` since it replaced the OPNsense
    # appliance, and `common/hosts.nix` calls its address after its job).
    case "$machine" in gw) hkey=router ;; *) hkey=$machine ;; esac
    addr=$(nix eval --impure --raw --expr "let h = import ./common/hosts.nix; in h.$hkey")
    me=$(hostname)
    if [ "$me" = "$machine" ]; then
        echo "=== $machine is this box: switching here"
        sudo nixos-rebuild switch --flake ".#$machine" && sudo nixos-confirm
        exit $?
    fi
    # Where the commit comes from when the target cannot reach GitHub itself (only the
    # operator's key can, and a box without one has no business holding a copy of it).
    # `here` is empty on a machine that is not in hosts.nix — then the target's own fetch has
    # to work, and if it cannot the recipe says so instead of syncing a tree over the top.
    case "$me" in gw) mkey=router ;; *) mkey=$me ;; esac
    here=$(nix eval --impure --raw --expr "let h = import ./common/hosts.nix; in if builtins.hasAttr \"$mkey\" h then h.$mkey else \"\"" 2>/dev/null || echo "")
    repo=$(git rev-parse --show-toplevel)
    echo "=== $machine ($addr): fetch, switch and confirm on the box"
    ssh -i "$HOME/.ssh/agent-hop-key" -o BatchMode=yes -o ConnectTimeout=5 \
        "leigh-admin@$addr" bash -s -- "$machine" "$here" "$repo" <<'REMOTE'
    set -euo pipefail
    machine=$1 here=$2 repo=$3
    cd ~/Projects/infrastructure-nix-flake
    if git fetch -q origin 2>/dev/null; then
        git merge -q --ff-only origin/main
    else
        [ -n "$here" ] || { echo "cannot fetch origin, and no peer address to fetch from" >&2; exit 1; }
        # IdentitiesOnly + the hop key: a box that offers every identity it has is refused by
        # gw's sshd with `Too many authentication failures` before the right one is tried.
        GIT_SSH_COMMAND="ssh -i $HOME/.ssh/agent-hop-key -o BatchMode=yes -o IdentitiesOnly=yes -o ConnectTimeout=5" \
            git fetch -q "ssh://leigh-admin@$here$repo" main
        git merge -q --ff-only FETCH_HEAD
    fi
    echo "=== $machine now at $(git log --oneline -1)"
    sudo nixos-rebuild switch --flake ".#$machine"
    sudo nixos-confirm
    REMOTE

# Kept because the docs and the router's own README name it: deploying the router is
# deploying gw like any other box — the difference is only that gw cannot compile, and it
# never could since it delegates to aibox (`machines/gw/default.nix`).
switch-gw: (deploy "gw")

# --- Nix formatting ---

# Format every tracked .nix file with alejandra (the repo standard: it is what
# .zed/settings.json runs on save).  `nix fmt` takes the formatter from the flake's
# pinned nixpkgs (flake.nix's `formatter`), i.e. from the binary cache — NOT from a
# devShell, because the wasm shell would build the pinned wasm-bindgen-cli and a
# formatter must never be the reason a box compiles Rust (see `_build-host-guard`).
# docs/repo-audit-2026-10-07.md §1.3 is why the whole tree is kept clean.
fmt:
    nix fmt -- $(git ls-files '*.nix')

# The check half of the same rule, wired into `just check`.
fmt-check:
    #!/usr/bin/env bash
    set -euo pipefail
    nix fmt -- --check $(git ls-files '*.nix')

# --- Rust crates (toolchain from `nix develop`: cargo/rustc/rustfmt/clippy, lld,
# --- just, git, alejandra, node — nothing that compiles on entry; the SPA recipes
# --- below ask for `nix develop .#wasm`, which adds the pinned wasm-bindgen-cli)
#
# Everything native is one cargo workspace (crates/Cargo.toml), so the whole set
# is `cargo <cmd> --workspace` from there: one lock file, one target dir, and one
# dependency build shared by every crate.  The wasm SPAs are deliberately not
# members (the reason is in that manifest); build one with `just build-frontend`.

# Recreate the git-ignored symlinks the wasm SPAs need to resolve their shared
# path dependencies (frontend/dto, filestore's frontend/common-rs).  The native
# crates need nothing: their path dependencies resolve inside the workspace.
# Run once after a fresh clone; `nix develop` does it automatically.
shared-rs:
    @bash common/shared-rs-links.sh

# cargo test over every crate in the workspace.
# (The default devShell: substituted, not built — the SPAs are not workspace members.)
test: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    bash common/shared-rs-links.sh
    nix develop --command bash -c 'cd crates && cargo test --workspace --offline'

# cargo clippy over the same set.  Warnings are reported, not fatal: the point is to see
# them (they used to go entirely unchecked).  A **failure** is fatal, though, and it was
# not: the body used to pipe clippy into `grep … || true`, so the recipe exited 0 whatever
# clippy said, and a lint gate that cannot fail is worse than no gate at all.  The status is
# now carried out of the filter explicitly.
clippy: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    bash common/shared-rs-links.sh
    nix develop --command bash -c '
      set -euo pipefail
      cd crates
      out=$(mktemp)
      trap "rm -f \"$out\"" EXIT
      status=0
      cargo clippy --workspace --all-targets --offline >"$out" 2>&1 || status=$?
      # One Compiling/Checking line per crate is most of the output and none of the
      # information; the diagnostics and clippy's own summary are what is wanted.
      grep -vE "^(Compiling|Checking|Finished|    Finished)" "$out" || true
      exit $status
    '

# --- frontend (the four Dioxus SPAs) ---

# Rebuild a Dioxus SPA into its frontend/dist using the wasm devShell
# (`nix develop .#wasm`: cargo/rustc/lld + the pinned wasm-bindgen-cli).  Same recipe the
# flake runs (common/frontend-build-spa.sh vs CRANE.wasmSpa), so the local dist/ matches
# the one it embeds; those dist/ directories are git-ignored and rebuilt at
# deploy time, so this is only for local iteration.
#
#   just build-frontend                 # all four
#   just build-frontend filestore       # one of them
# (just 1.51 passes any extra argument straight into the single parameter, so
# there are no flags here — a bad name just fails the whitelist check.)
build-frontend crate="all": _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    # The parameter arrives through just's quote() — shell-quoted, and the case
    # below is a whitelist anyway.
    case {{ quote(crate) }} in
        all) crates="frigate-monitor filestore gocardless-dashboard switch-gui" ;;
        *) crates={{ quote(crate) }} ;;
    esac
    for c in $crates; do
        if [ ! -f "crates/$c/frontend/build.sh" ]; then
            echo "no crates/$c/frontend/build.sh — expected one of: frigate-monitor filestore gocardless-dashboard switch-gui all" >&2
            exit 2
        fi
        echo "=== build-frontend $c"
        nix develop .#wasm --command bash -c "exec ./crates/$c/frontend/build.sh"
    done

# network-status' SPA is the exception: it is SolidJS + esbuild, not Dioxus, and
# its npm deps are not nixpkgs-cached, so the flake cannot rebuild it offline and
# crates/network-status/frontend/dist is COMMITTED (see the note in
# machines/gw/network-status.nix, where the service has run since 2026-10-21).  Build
# it with npm, then commit the
# result; `just network-status-dist-check` catches a bundle that has drifted from
# src/.
network-status-frontend:
    #!/usr/bin/env bash
    set -euo pipefail
    cd crates/network-status/frontend
    npm install
    npm run build

# Fails if the committed network-status bundle is not what `npm run build`
# produces from the current src/ — the one drift the committed dist/ allows.
network-status-dist-check:
    #!/usr/bin/env bash
    set -euo pipefail
    cd crates/network-status/frontend
    if [ ! -d node_modules ]; then
        echo "node_modules missing — run 'npm install' here once (or 'just network-status-frontend')" >&2
        exit 2
    fi
    npm run build >/dev/null
    if ! git diff --quiet -- dist; then
        echo "crates/network-status/frontend/dist has drifted from src/ — commit the rebuild:" >&2
        git --no-pager diff --stat -- dist
        exit 1
    fi
    echo "network-status dist/ matches a fresh build"

# --- the router's ssh host keys

# Print the host key lines a client needs to verify 10.3.1.1. Nothing ships them
# any more: the pin used to be /etc/dns-sync/known_hosts, read by a tool that
# ssh'd from services1 to edit the network's DNS, and that tool is gone
# (common/int-dns.nix and `just switch-gw` are how `*.int` gets a record now).
# `crates/network-status`'s remote probe mode still takes --known-hosts and
# refuses to connect without it, which is the point: a missing file is no ssh,
# never an unverified one. Run this after a router reinstall, keep the output in
# a file, and hand that file to whatever is connecting.
#
# As leigh-admin, not root: the router's root login is prohibit-password, and
# this recipe used to ssh in as root, which meant it could not regenerate the
# pin for the box it replaced. Keyscanning over the network (rather than reading
# /etc/ssh/ssh_host_*.pub on the box) is deliberate: it records the key a client
# is actually offered, and the client will connect over the same path. Types the
# router does not offer simply print nothing, so the output is what it presents
# today — two lines for gw, three for the OPNsense box it replaced.
router-known-hosts:
    ssh -i ~/.ssh/agent-hop-key -o BatchMode=yes leigh-admin@10.3.1.1 'sh -c "ssh-keyscan -t ed25519,ecdsa,rsa 10.3.1.1 2>/dev/null"'

# The pinned host keys of the four flake machines, in the form
# common/ssh-known-hosts.nix carries them (which is what
# `programs.ssh.knownHosts` turns into /etc/ssh/ssh_known_hosts, and therefore
# what the *nix daemon's* ssh reads when it runs a remote-build hop).  Run this
# after any box is reinstalled or rotates its host keys, paste the new lines into
# that DATA table, deploy; until then the hop to that box fails with
# `Host key verification failed` (docs/nix-dev-workflow.md,
# "Remote building: the hop is not the operator's ssh").
#
# Keyscanned over the wire rather than read from /etc/ssh/ssh_host_*.pub: the
# client connects to an address and is offered whatever that address answers now,
# which is the only thing that can make the hop work.  -T 5 because ssh-keyscan's
# one-second default drops lines when several addresses are queried at once, and
# a half-written pin is worse than a missing one.
known-hosts:
    #!/usr/bin/env bash
    set -euo pipefail
    addrs=$(nix eval --impure --raw --expr 'let h = import ./common/hosts.nix; in builtins.concatStringsSep " " [ h.router h.services1 h.services2 h.aibox ]')
    ssh-keyscan -T 5 -t rsa,ed25519 ${addrs} 2>/dev/null | grep -v '^#'

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

# Rebuild+switch for the netboot setup, then reboot the netbooted clients.
#
# The image is `machines/pi-room-sys` — an in-tree machine, so edit it and
# rebuild: the `git+file://` input it used to boot from is gone, and so is its
# `just update-netboot-input`, deleted because a pin going stale in silence is exactly
# what that recipe existed to avoid and nobody ran it.  Boot flow and what aibox
# exports: machines/pi-room-sys/README.md.
switch-netboot:
    #!/usr/bin/env bash
    set -euo pipefail

    sudo bash -c 'umount -f -l /exports/netboot-squashfs | true'
    sudo nixos-rebuild switch --flake .
    sudo nixos-confirm
    sudo mount -a

    # Configuration
    BASE_IP="10.3.14."
    START=101
    END=110
    COMMAND="sudo reboot"
    TIMEOUT=10  # seconds
    LOG_FILE="sysrq_log.txt"

    # Clear log file
    > "$LOG_FILE"

    # Loop over IP range
    for i in $(seq $START $END); do
        HOST="${BASE_IP}${i}"
        echo -n "Sending reboot to $HOST... "

        # Run SSH with timeout
        if timeout "$TIMEOUT" ssh -o ConnectTimeout=5 -o BatchMode=yes -o StrictHostKeyChecking=no -o ServerAliveInterval=1 -o ServerAliveCountMax=2 root@"$HOST" "$COMMAND" 2>/dev/null; then
            echo "SUCCESS: (SSH returned success)" | tee -a "$LOG_FILE"
        else
            EXIT_CODE=$?
            echo "FAILED (SSH error: exit code $EXIT_CODE)" | tee -a "$LOG_FILE"
        fi
    done

    echo "All operations completed. Log saved to $LOG_FILE"

# --- PXE clients on VLAN 227, for testing the netboot without a board -----------
#
# `just pxe-client-bios` / `just pxe-client-uefi` — run on aibox.
#
# `br227` is aibox's own port in the workshop's VLAN, so the guest takes its lease
# from the router, fetches across the router, and walks the same path the boards do —
# which is the only way this test means anything.  The guest is just qemu: the
# netboot *server* is the deployed config, never a fixture.
#
# Why not the bridge helper: `qemu-bridge-helper` is in no package aibox installs
# (`virtualisation.libvirtd.allowedBridges = ["br227"]` is libvirt's own allow-list
# and does not put the helper in the system), so `helper=$(type -p
# qemu-bridge-helper)` expanded to nothing and qemu refused the netdev — these two
# recipes could not run at all, which is how the 2026-10-26 netboot outage sat for
# five days with nothing able to show a boot from the VLAN.  A tap enslaved to the
# bridge works.  `bridge_slave state` is not an option on the way in — iproute2
# refuses `state forwarding` here, and the port does not need it: a fresh tap port
# reads `disabled` only while nothing has the tap open, and it goes to `forwarding`
# the moment qemu attaches.  Worth knowing because a `disabled` port in `bridge
# link` while the guest is still booting looks exactly like the bug it is not, and
# the DISCOVER that gets no answer looks exactly like a dead router.
#
# BIOS and UEFI are not interchangeable here.  SeaBIOS's embedded iPXE has no IPv6,
# so the BIOS client cannot hit a dead AAAA and walks straight past the failure that
# took the room down: **the BIOS client passing is not evidence.** Boot the UEFI one
# too.  `machines/pi-room-sys/README.md` owns the flow and what each stage proves.
#
# The console is the guest's serial port, and the menu script's own
# `console --picture` moves the console to the graphical display — so the last thing
# you see is the menu fetch and the judgement finishes in aibox's
# `/var/log/nginx/access.log` (menu, logo, bzImage, initrd = the boot works).  With
# no tty (a script, an agent) it runs headless and tails the serial log instead.
# Ctrl-A d leaves qemu; the tap is removed on exit either way, and the guest leases
# from the dynamic pool (10.3.14.150-.253), so a run costs a lease and nothing else.

# The shared implementation: $0 is the guest kind, `bios` or `uefi`.
_pxe-client kind:
    #!/usr/bin/env bash
    set -euo pipefail
    kind={{ quote(kind) }}
    [ "$(hostname)" = aibox ] || { echo "run this on aibox: br227 is its port on VLAN 227" >&2; exit 1; }

    tap=tap-pxe-$kind
    serial=/tmp/pxe-client-$kind-serial.log
    # Random, not fixed: a fixed MAC pins one address in the workshop's dynamic pool
    # and every run then tests the same lease.
    mac=$(printf '52:54:00:%02x:%02x:%02x' $((RANDOM % 256)) $((RANDOM % 256)) $((RANDOM % 256)))

    if [ "$kind" = uefi ]; then
        ovmf=$(nix build --no-link --print-out-paths nixpkgs#OVMF.fd)/FV
        cp "$ovmf/OVMF_CODE.fd" /tmp/pxe-OVMF_CODE.fd
        cp "$ovmf/OVMF_VARS.fd" /tmp/pxe-OVMF_VARS.fd
    fi

    guest=$(mktemp)
    cat >"$guest" <<'GUEST'
    set -eu
    tap=$TAP mac=$MAC kind=$KIND serial=$SERIAL
    ip link del "$tap" 2>/dev/null || true
    ip tuntap add dev "$tap" mode tap
    ip link set "$tap" master br227
    ip link set "$tap" up
    cleanup() {
        # Kill the VM before taking its tap away, and take signals as well as exit.
        # With `trap cleanup EXIT` alone bash does not run the trap when an untrapped
        # signal kills it, so an interrupted run left qemu alive behind a tap that no
        # longer existed — three orphaned VMs by the end of one afternoon, which is how I
        # learned it.  The five-second poll is so the chardev log is closed before the
        # caller tails it.
        if [ -n "${qemu_pid:-}" ] && kill -0 "$qemu_pid" 2>/dev/null; then
            kill "$qemu_pid" 2>/dev/null || true
            for _ in 1 2 3 4 5; do kill -0 "$qemu_pid" 2>/dev/null || break; sleep 1; done
            kill -9 "$qemu_pid" 2>/dev/null || true
        fi
        ip link del "$tap" 2>/dev/null || true
        rm -f /tmp/pxe-OVMF_CODE.fd /tmp/pxe-OVMF_VARS.fd
    }
    trap cleanup EXIT INT TERM HUP

    args=( -m 2048 -accel kvm -smp 2 -cpu host
           -netdev "tap,id=net0,ifname=$tap,script=no,downscript=no"
           -device "virtio-net-pci,netdev=net0,mac=$mac"
           -boot n -no-reboot )
    if [ "$kind" = uefi ]; then
        args+=( -machine q35,smm=on
                -global driver=cfi.pflash01,property=secure,value=on
                -drive file=/tmp/pxe-OVMF_CODE.fd,if=pflash,format=raw,unit=0,readonly=on
                -drive file=/tmp/pxe-OVMF_VARS.fd,if=pflash,format=raw,unit=1 )
    fi
    # A tty gets the guest's console on stdio; without one, -nographic has nothing to
    # grab, so log the serial stream to a file an agent can read.  `-serial mon:stdio`
    # plus an outer redirect looks equivalent and is not: with stdin at /dev/null the
    # mux sees EOF and the log stays empty.  `-serial file:` is not a char driver here.
    if [ -t 0 ]; then
        args+=( -nographic )
    else
        args+=( -display none -chardev "file,id=ser0,path=$serial" -serial chardev:ser0 )
    fi

    echo "== $kind client, mac $mac, on br227 (VLAN 227).  Ctrl-A d to leave."
    qemu_pid=""
    qemu-system-x86_64 "${args[@]}" & qemu_pid=$!
    wait "$qemu_pid"
    GUEST
    if [ -t 0 ]; then
        sudo env TAP="$tap" MAC="$mac" KIND="$kind" bash "$guest"
    else
        # Headless: qemu writes the guest's console to $serial (owned by root, world
        # readable), and the summary below points at it and at the access log.
        sudo env TAP="$tap" MAC="$mac" KIND="$kind" SERIAL="$serial" bash "$guest"
        echo "== guest console: $serial"
        tail -20 "$serial" 2>/dev/null || true
        echo "== and aibox's /var/log/nginx/access.log for the fetches themselves"
    fi
    rm -f "$guest"

# A BIOS PXE client on VLAN 227 (no IPv6, so it cannot show a dead-AAAA hang).  Run on aibox.
pxe-client-bios: (_pxe-client "bios")

# A UEFI (OVMF) PXE client on VLAN 227: the shape the Pi Room boards boot.  Run on aibox.
pxe-client-uefi: (_pxe-client "uefi")

update-pkgs:
    nix flake update nixpkgs

# --- checks and the test surface ---

# Nix-level checks: the flake evaluates, the machines' option assertions hold, the
# hand-rolled config serializers and the generated printer-state tables match their
# expectations, and every tracked .nix file is alejandra-clean.  Fast (no compilation) —
# except that its `nix flake check` step is not eval-only in practice and is skipped on `gw`
# with the reason printed, so read that recipe before assuming what a green `just check` on
# the router covered.  The rest of the test surface: `just test` (cargo tests),
# `just clippy` (lints), `just filestore-test` (headless-browser suite) and
# `just check-build` (the deploy-readiness build).
check: check-eval
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "$(hostname)" = "gw" ]; then
        echo "=== nix flake check: SKIPPED on gw"
        echo "    It cannot pass here and pretending otherwise costs a confusing failure:"
        echo "    evaluating the machines reads a derivation (crane stubs each SPA's source tree"
        echo "    at eval time, because a SPA's path dependencies live outside its directory), so"
        echo "    the line needs one trivial source copy and --no-build refuses to do it —"
        echo "    directly or over the hop to aibox.  The router's gate is 'just check-eval',"
        echo "    which just ran; run the full 'just check' on aibox or services1."
        echo "    The class, the two doors still open, and the honest detector:"
        echo "    docs/nix-dev-workflow.md, 'Closed 2026-10-25, one read at a time'."
        exit 0
    fi
    echo "=== nix flake check"
    # Not eval-only in practice: on a flake source nothing has evaluated yet it needs the stub
    # described above, so a fresh worktree or clone fails this line on *any* box, warm store or
    # not, and one full evaluation (or any deploy) writes them and it passes. Which is why
    # `check-eval` exists as the half you can always trust.
    nix flake check --no-build

# The eval-only half of `just check`, and the whole gate on `gw`: the assertion evals and the
# formatter check.  None of it evaluates a *machine*, so none of it can be made to build
# something by a crane helper that reads sources at eval time — the property that makes this
# the honest thing to run on the router, where evaluation is paid for by the box that answers
# DNS for the network (AGENTS.md, "Don't build on the router").
check-eval:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "=== config-to-gitlab assertions"
    nix eval --impure --raw -f machines/services1/lib/check-config-to-gitlab.nix
    echo "=== printer-state tables (exporter <-> dashboards/alerts)"
    nix eval --impure --raw -f common/check-printer-states.nix
    # The router's two network tables: common/vlans.nix and common/dns-records.nix
    # generate dnsmasq's conf, nftables' rules and the VLAN devices, so a VLAN id
    # that is not in the first table or an address claimed twice is a client that
    # gets the wrong address rather than a build failure.  The same assertions run
    # as a module (machines/gw/lib/assert-network-tables.nix) and so inside `check`'s
    # `nix flake check` step; this is the version that prints which record is wrong.
    echo "=== router network tables (vlans <-> dns-records <-> ACL)"
    nix eval --impure --raw -f machines/gw/lib/check-network-tables.nix
    # The instructions themselves: every `just <name>` in AGENTS.md/README.md/justfile is
    # a recipe, every repo path they name exists, every docs/*.md is linked from
    # somewhere, and AGENTS.md does not call the router a shadow. Eval-only, like the
    # three above, so it is safe on gw — which is the reason it is a Nix evaluator and
    # not the compiled linter the audit proposed (see its header).
    echo "=== doc pointers (recipes, paths, docs, the router's own status)"
    nix eval --impure --raw -f common/check-doc-pointers.nix
    # The gatus endpoint guard is a module assertion in
    # machines/services1/services/monitoring.nix, so `check`'s `nix flake check` step
    # already runs it: every https *.int endpoint in common/monitoring-endpoints.nix
    # must be a vhost services1 serves.  Worth naming here because the guard now
    # lives on a different machine from gatus itself, so it is easy to forget which
    # eval catches a drift.
    # AGENTS.md rule 26.  Eval-only — one `wc -l` over the tracked tree — so it is in
    # this half rather than left for `check`, which the router cannot finish: a file
    # is not allowed to grow past 1000 lines on the way past the router.
    echo "=== source file sizes (AGENTS.md rule 26)"
    just check-source-size
    echo "=== alejandra --check"
    just fmt-check

check-source-size:
    #!/usr/bin/env bash
    # AGENTS.md rule 26: no source file over 1000 lines unless it can be helped.
    # Four files are already over and are named here rather than hidden — a new one
    # fails, and if one of these gets shorter the guard says so, because a baseline
    # nobody prunes is a baseline nobody trusts.  Generated and machine-owned trees
    # are skipped: `*/dist/*` is a minified bundle.  Prose is not source, and a Nix
    # data table is NOT exempt — `monitoring-dashboards.nix` is on the baseline.
    set -euo pipefail
    baseline="crates/switch-gui/src/switch.rs crates/switch-gui/src/shadow.rs crates/filestore/src/server.rs machines/services1/services/monitoring-dashboards.nix"
    over=$(git ls-files '*.rs' '*.nix' '*.ts' '*.tsx' '*.sh' '*.cjs' '*.py' '*.js' '*.css' \
        | grep -v '/dist/' \
        | xargs -r wc -l 2>/dev/null \
        | grep -v ' total$' \
        | awk '$1 > 1000 { print $2 }' | sort)
    offenders=""
    for f in $over; do
        case " $baseline " in
            *" $f "*) ;;
            *) offenders="$offenders $f" ;;
        esac
    done
    for f in $baseline; do
        if [ ! -f "$f" ]; then
            echo "  gone from the tree, drop it from the baseline: $f" >&2
        elif [ "$(wc -l < "$f")" -le 1000 ]; then
            echo "  under 1000 now, drop it from the baseline: $f ($(wc -l < "$f"))" >&2
        fi
    done
    if [ -n "$offenders" ]; then
        echo "over 1000 lines and not on the baseline — split it (AGENTS.md rule 26):" >&2
        for f in $offenders; do echo "  $(wc -l < "$f") $f" >&2; done
        exit 1
    fi
    echo "source sizes ok: $(echo "$over" | grep -c . || true) file(s) over 1000, all on the baseline"

# Deploy-readiness: build every machine's system derivation without touching the
# running system.  `nix flake check` only *evaluates*, so a module that evaluates
# but fails to build (bad package, bad derivation arg, broken SPA bundle) still
# reaches `just switch`.  This does the whole build and stops short of activating
# anything — deliberately not `switch`/`boot`.
#
# This is `nix build` on the toplevel, NOT `nixos-rebuild dry-build`.  This box's
# nixos-rebuild is nixos-rebuild-ng, and its man page defines dry-build as "Show what
# store paths would be built or downloaded by any of the operations above, but
# otherwise do nothing": on the 2026-10-15 network upgrade (first run of
# docs/network-upgrade-runbook.md) it evaluated all three systems, printed a
# 786-derivation "these derivations will be built" list, exited 0 in under a minute
# and had compiled nothing.  `nixos-rebuild build`/`boot` do compile, but they also
# write a boot loader entry, so the next reboot comes up on a configuration nobody
# ever activated.  `nix build` on config.system.build.toplevel builds the very same
# derivation with neither side effect: no profile, no boot entry, no
# /run/current-system, so the autoRollback timer stays unarmed.
#
# Slower than `just check` — a nixpkgs bump is hundreds of derivations — so run it
# before a deploy rather than on every edit, detached, and poll it (rule 11).  The
# work is delegated to aibox (common/build-farm.nix), which has 59 GiB, 6 cores and
# no strata engine any more, so poll load/swap there as well as this log: a build
# storm competes with the apps aibox hosts (frigate, immich, whisper).
check-build: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    for m in services1 services2 aibox gw; do
        echo "=== nix build $m toplevel"
        nix build --no-link --print-out-paths ".#nixosConfigurations.$m.config.system.build.toplevel"
    done
    echo "all four systems build"

# Offload a build to aibox and refuse to fall back (common/build-farm.nix).
# `nix.distributedBuilds` is true, so nix already prefers the external builder for
# every build and only comes back local when no builder is available — `just switch`
# and `just check-build` are therefore already remote, and that is deliberate (a
# build that falls back is worth more than one that fails because the builder is
# down).  `--max-jobs 0` is for when you need the opposite guarantee: it removes the
# local fallback, so the build fails loudly instead of quietly compiling on the box
# you are standing on.  It is the recipe to reach for when the builder is idle and
# the local disk is climbing.
build-remote machine="services1": _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    nixos-rebuild build --flake ".#{{ machine }}" --max-jobs 0

# Start an unauthenticated filestore on 127.0.0.1 for poking at the API/UI by
# hand. --no-auth is only honoured on a loopback bind, so this can never expose
# the store on a routable interface; the deployed service never uses it.
filestore-dev: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop --command bash -c 'cd crates && cargo build --release --offline -p filestore && exec ./target/release/filestore --root "${FS_TEST_ROOT:-/tmp/filestore-test-root}" --no-auth --port "${FS_TEST_PORT:-18097}"'

# Builds the SPA + binary, starts filestore with --no-auth on 127.0.0.1 against
# a scratch fixture, and drives the UI with headless Chromium (Playwright).
# See crates/filestore/tests/README.md.
filestore-test: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop .#wasm --command bash -c 'exec crates/filestore/tests/run.sh'

# The switch-gui headless-browser suite: builds the SPA + binary, starts a fake
# ProCurve console (a real Telnet server with an in-memory model), points the
# service at it with --no-auth on a loopback bind and a scratch Postgres schema,
# and drives the matrix with headless Chromium.  Nothing can reach a real switch:
# the only address the service is given is 127.0.0.1.  Needs the local Postgres
# cluster (see SG_TEST_DB_URL in crates/switch-gui/tests/run.sh).
switch-gui-test: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop .#wasm --command bash -c 'exec crates/switch-gui/tests/run.sh'

# Install the shared age key (used to edit secrets) so sops-nix can
# decrypt at runtime and build time on this machine. Run after a fresh
# 'nixos-rebuild'. The nixbld group ownership is what lets nix builds
# decrypt secrets (see common/sops.nix).
install-sops-key:
    #!/usr/bin/env bash
    set -euo pipefail

    sudo install -d -m 550 -g nixbld /var/lib/sops-nix
    sudo install -m 440 -g nixbld "{{ SOPS_KEY_FILE }}" /var/lib/sops-nix/key.txt

edit-secrets:
    sops edit secrets/secrets.yaml

# --- the cameras (OpenIPC on the WiFi VLAN) ---------------------------------
#
# `cam5` `cam7` `cam9` are Buildroot/OpenIPC boxes on VLAN 226 (10.3.2.10/.12/.17,
# pinned by MAC in `common/dns-records.nix`, which is the table these recipes read —
# there is no second copy of the addresses to drift).  They run no NixOS and no
# systemd: their whole state is files under `/etc` in a tmpfs overlay (704 KB,
# ~450 KB free) that a reboot re-reads and a firmware reflash overwrites, so
# nothing in the flake sees them and a fault on one is invisible from the box
# they are consumed on (services1's frigate pulls their RTSP through its go2rtc).
# Read `cameras/README.md` before changing anything on one; the device-side
# story (how an OpenIPC camera is opened up at all) is `cameras/c110.md`.
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
    for c in cam5 cam7 cam9; do echo "===== $c"; just cam-check "$c"; done

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
# recipes below are the ceremony in order.  `cameras/c110.md` §8 still describes
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

# Does a card slot actually exist on this board?  Not answerable from the file system —
# the host driver probes with no card and there is no block device until one is inserted.
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
