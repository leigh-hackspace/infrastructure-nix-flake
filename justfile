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
        services1|services2|services3|aibox|gw) ;;
        *) echo "unknown machine: $machine — expected services1, services2, services3, aibox or gw" >&2; exit 2 ;;
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
        # `&&` and a real exit test, never a pipeline: confirming a switch that died is how
        # a half-activated generation gets marked good and the rollback net is disarmed.
        if sudo nixos-rebuild switch --flake ".#$machine" && sudo nixos-confirm; then
            just pending-reboot || true
        else
            echo "switch FAILED here: nothing was confirmed." >&2
            echo "  If /run/current-system is already the new generation, the auto-rollback" >&2
            echo "  timer is armed and this box reboots itself in five minutes — decide" >&2
            echo "  inside that window: fix the activation and confirm, or let it roll back." >&2
            exit 1
        fi
        exit 0
    fi
    # Where the commit comes from when the target cannot reach GitHub itself (only the
    # operator's key can, and a box without one has no business holding a copy of it).
    # `here` is empty on a machine that is not in hosts.nix — then the target's own fetch has
    # to work, and if it cannot the recipe says so instead of syncing a tree over the top.
    case "$me" in gw) mkey=router ;; *) mkey=$me ;; esac
    here=$(nix eval --impure --raw --expr "let h = import ./common/hosts.nix; in if builtins.hasAttr \"$mkey\" h then h.$mkey else \"\"" 2>/dev/null || echo "")
    repo=$(git rev-parse --show-toplevel)
    # aibox's /etc/fstab bind-mounts the Pi Room netboot squashfs **from a store path**, so
    # every nixpkgs bump moves the mount's source and activation's restart of a
    # boot-generated unit fails: `switch-to-configuration` exits 4 *after* flipping
    # /run/current-system, which arms the five-minute rollback countdown on the network's
    # builder.  Unmounting first (lazily, so a client mid-boot keeps its handles) makes
    # activation start the mount on the new path and exit 0 - measured 2026-11-15, and the
    # step lives *inside* the remote heredoc below rather than in a heredoc of its own,
    # because `just` strips one indent level and a nested terminator survives indented,
    # which bash reads as an unterminated here-document.  docs/network-upgrade-runbook.md
    # Phase 7 owns the procedure.
    echo "=== $machine ($addr): fetch, switch and confirm on the box"
    ssh -i "$HOME/.ssh/agent-hop-key" -o BatchMode=yes -o ConnectTimeout=5 \
        "leigh-admin@$addr" bash -s -- "$machine" "$here" "$repo" <<'REMOTE'
    set -euo pipefail
    machine=$1 here=$2 repo=$3
    cd ~/Projects/infrastructure-nix-flake
    echo "  disk before: / $(df --output=avail -BG / | tail -1 | tr -dc '0-9')G free"
    if [ "$machine" = aibox ]; then
        if findmnt -no SOURCE /exports/netboot-squashfs >/dev/null 2>&1; then
            echo "  aibox: NFS clients on :2049 = $(sudo -n ss -tn sport = :2049 2>/dev/null | tail -n +2 | wc -l)"
            sudo -n umount -f -l /exports/netboot-squashfs \
                && echo "  unmounted (lazy): activation will mount the new store path"
        else
            echo "  aibox: netboot export not mounted"
        fi
    fi
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
    # The switch succeeded and is confirmed; say out loud whether the box is finished.
    # A GPU box here is very likely running the new NVIDIA user-space against the old
    # kernel module, which is an outage the unit now reports — but the operator should
    # hear it from the deploy, not from the endpoint.  Read-only; does not gate.
    just pending-reboot "$machine" || true

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
# The whole tree is kept clean so `nix fmt -- --check` is a real gate rather than a
# warning about the last file someone touched.
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
      # information; the diagnostics and the linter summary are what is wanted.
      # No apostrophes anywhere inside this single-quoted `bash -c` argument: one closes
      # the string and the outer shell then evaluates the rest — which is exactly what
      # happened here from 2026-10-25 until 2026-11-16, when a comment reading "clippy" +
      # possessive made the whole tail of the script belong to the outer shell and
      # `just clippy` failed on every run with `out: unbound variable` at the first line
      # after it. A gate that exits 1 for a reason unrelated to linting trains people to
      # ignore it, so: if this recipe needs prose with an apostrophe, put it OUTSIDE the
      # quoted script (above), not inside.
      grep -vE "^(Compiling|Checking|Finished|    Finished)" "$out" || true
      exit $status
    '

# --- frontend (the five Dioxus SPAs) ---

# Rebuild a Dioxus SPA into its frontend/dist using the wasm devShell
# (`nix develop .#wasm`: cargo/rustc/lld + the pinned wasm-bindgen-cli).  Same recipe the
# flake runs (common/frontend-build-spa.sh vs CRANE.wasmSpa), so the local dist/ matches
# the one it embeds; those dist/ directories are git-ignored and rebuilt at
# deploy time, so this is only for local iteration.
#
#   just build-frontend                 # all five
#   just build-frontend filestore       # one of them
# (just 1.51 passes any extra argument straight into the single parameter, so
# there are no flags here — a bad name just fails the whitelist check.)
build-frontend crate="all": _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    # The parameter arrives through just's quote() — shell-quoted, and the case
    # below is a whitelist anyway.
    case {{ quote(crate) }} in
        all) crates="frigate-monitor filestore gocardless-dashboard network-status switch-gui" ;;
        *) crates={{ quote(crate) }} ;;
    esac
    for c in $crates; do
        if [ ! -f "crates/$c/frontend/build.sh" ]; then
            echo "no crates/$c/frontend/build.sh — expected one of: frigate-monitor filestore gocardless-dashboard network-status switch-gui all" >&2
            exit 2
        fi
        echo "=== build-frontend $c"
        nix develop .#wasm --command bash -c "exec ./crates/$c/frontend/build.sh"
    done

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
    addrs=$(nix eval --impure --raw --expr 'let h = import ./common/hosts.nix; in builtins.concatStringsSep " " [ h.router h.services1 h.services2 h.services3 h.aibox ]')
    ssh-keyscan -T 5 -t rsa,ed25519 ${addrs} 2>/dev/null | grep -v '^#'

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
        # The scheme is CUPS's vocabulary (`net+ippt`, `ipp`); curl only speaks http,
        # and the fact being asked is about the endpoint behind the scheme.
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
        printf '         matches the row? '
        case "$verdict" in
            IPP-native*)
                case "$uri" in *ippt*) echo "yes — row is IPP-native like the endpoint" ;;
                *) echo "NO — endpoint is IPP-native, row says a CUPS share: fix ./printers-data.nix's scheme" ;;
                esac ;;
            CUPS-shaped*)
                case "$uri" in *ippt*) echo "NO — endpoint is a CUPS share, row says IPP-native: fix the scheme" ;;
                *) echo "yes on the shape; the queue name is still the row's assertion to settle" ;;
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
    # The voice assistant's device boundary: no host, URL, vendor name or child process in
    # the turn logic, and the adapters still there to own those facts instead.  This is what
    # keeps "a new microphone is a new driver, not a refactor" true after the next person
    # finds it easier to test `source.starts_with("rtsp://")` in the loop than to write one
    # (docs/esp32-voice-satellites.md is the contract the boundary is drawn for).
    echo "=== voice platform boundary (core <-> drivers)"
    nix eval --impure --raw -f common/check-platform-boundary.nix
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

# --- upgrade safety probes ------------------------------------------------------
#
# Three things cost a day each on the 2026-11-15 whole-network repin, and none of them
# were visible from the recipes then available: a disk that filled mid-gate, a store lock
# held by an editor, and a GPU box switched but not rebooted.  Each has a probe now.
# Receipts: docs/work-log-2026-11.md; the procedure text is in the runbook.

# Free space on /, and the refusal the build recipes use.
#
# `just check-build` builds five system toplevels and nix copies every one of them back
# into the store of the box you run it from: on 2026-11-15 that took services1 from 72 %
# to 99 % (3.6 GB free) while it was serving postgres, nginx, SSO and frigate.  Asking
# before starting costs one `df`; discovering it means a GC in the middle of a deploy.
_disk-headroom need_gib="":
    #!/usr/bin/env bash
    set -euo pipefail
    free_gib=$(df --output=avail -BG / | tail -1 | tr -dc '0-9')
    if [ -n "{{ need_gib }}" ] && [ "$free_gib" -lt "{{ need_gib }}" ] && [ -z "${INFRA_ALLOW_LOW_DISK:-}" ]; then
        echo "refusing: / has ${free_gib}G free and this wants {{ need_gib }}G." >&2
        echo "  Reclaim first — sudo nix-collect-garbage — but NEVER between the build gate" >&2
        echo "  and the deploy: 'nix build --no-link' creates no GC root, so a GC there" >&2
        echo "  deletes the very outputs the gate produced and the deploy recompiles.  Use" >&2
        echo "  'just check-build', which roots its own outputs, then 'just check-build-clean'." >&2
        echo "  Override with INFRA_ALLOW_LOW_DISK=1 if you really mean it." >&2
        exit 1
    fi
    echo "=== disk: ${free_gib}G free on /"$([ -n "{{ need_gib }}" ] && echo " (wants {{ need_gib }}G)")

# Who holds the nix store big lock, with the parent process that explains it.
#
# Empty/"free" means the store is idle.  A nix command that hangs with no CPU, no output
# and no timeout is queued behind this lock, and "slow" is what it looks like — measured
# twice on 2026-11-15, once for 1 h 56 min, both times held not by a build but by `nixd`
# (an editor's language server) via a `nix-daemon` child, which no `pgrep -f "nix eval|nix
# build"` finds.  `/proc/locks` names the inode and the pid directly, so this needs no
# scan and no sudo.  Run it before blaming a slow build; the fix is in docs/nix-dev-workflow.md.
# Who holds the nix store big lock, with the parent that explains it (nixd is not a build).
store-lock:
    #!/usr/bin/env bash
    set -euo pipefail
    inode=$(stat -c %i /nix/var/nix/db/big-lock)
    hits=$(awk -v i=":$inode " 'index($0, i)' /proc/locks || true)
    if [ -z "$hits" ]; then
        echo "store lock: free"
        exit 0
    fi
    echo "store lock: HELD"
    for pid in $(echo "$hits" | awk '{print $5}'); do
        cmd=$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null || echo "(process gone)")
        ppid=$(awk '/^PPid:/{print $2}' "/proc/$pid/status" 2>/dev/null || echo "?")
        pcmd=$(tr '\0' ' ' <"/proc/$ppid/cmdline" 2>/dev/null | cut -c1-70 || echo "?")
        age=$(ps -o etimes= -p "$pid" 2>/dev/null | tr -d ' ' || echo "?")
        echo "  pid=$pid held=${age}s  ${cmd:0:70}"
        echo "    parent pid=$ppid  $pcmd"
    done
    echo "  If the parent is nixd / an editor server: that is not a build to wait out."
    echo "  kill -9 the holder and its parent (SIGTERM does nothing to a process blocked"
    echo "  in flock), then re-run.  docs/nix-dev-workflow.md, 'the nix big lock'."

# A switched-but-not-rebooted box, and whether that is cosmetic or an outage.
#
# A nixpkgs bump moves the kernel and the NVIDIA driver together.  `switch` installs both
# into the new generation but changes neither running one, so on a GPU box the GPU unit
# restarts into new user-space with the old kernel module still loaded — and the unit does
# not fail, it just cannot reach the card: llama-server sat in `activating`, strata came
# back 502, both with zero failed units.  common/gpu-driver-match.nix makes the unit say
# so; this says so about the *box*, from either side of the ssh.
#
# Read-only.  Exit code is 0 either way (it is a report, not a gate); the verdict line is
# the thing to read.
# Switched but not rebooted? Running kernel vs this generation's, plus the NVIDIA driver.
pending-reboot machine="":
    #!/usr/bin/env bash
    set -euo pipefail
    probe='set -u
        new=$(readlink /run/current-system/kernel 2>/dev/null | grep -oE "linux-[0-9][0-9.]*" | head -1 | sed "s/linux-//")
        run=$(uname -r)
        echo "kernel: running=$run generation=${new:-unknown}"
        if [ -n "${new:-}" ] && [ "$new" != "$run" ]; then
            echo "  pending reboot: the running kernel is not the one this generation was built for."
        fi
        if [ -e /sys/module/nvidia/version ]; then
            loaded=$(cat /sys/module/nvidia/version)
            echo "nvidia: loaded module=$loaded"
            if out=$(nvidia-smi --query-gpu=driver_version --format=csv,noheader 2>&1); then
                echo "  nvidia-smi: ok ($(echo "$out" | head -1)) — user-space and module agree"
            else
                echo "  BROKEN NOW: $(echo "$out" | head -1)"
                echo "  The GPU endpoint on this box cannot work until a reboot. Fix:"
                echo "    just deploy $(hostname)   # or: sudo reboot — the guarded unit self-heals after it"
            fi
        fi'
    if [ -z "{{ machine }}" ] || [ "{{ machine }}" = "$(hostname)" ]; then
        echo "=== $(hostname)"; bash -c "$probe"
    else
        case "{{ machine }}" in
            services1|services2|services3|aibox|gw) ;;
            *) echo "unknown machine: {{ machine }}" >&2; exit 2 ;;
        esac
        case "{{ machine }}" in gw) hkey=router ;; *) hkey="{{ machine }}" ;; esac
        addr=$(nix eval --impure --raw --expr "let h = import ./common/hosts.nix; in h.$hkey")
        echo "=== {{ machine }} ($addr)"
        # Over stdin, not as ssh arguments: ssh concatenates its command arguments into one
        # string for the remote shell, which word-splits the script and runs `bash -c set`
        # (i.e. prints the environment) instead of the probe.
        printf '%s\n' "$probe" | ssh -i "$HOME/.ssh/agent-hop-key" -o BatchMode=yes -o ConnectTimeout=5 \
            "leigh-admin@$addr" bash -s
    fi

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
#
# Two things it does that a bare `nix build` loop does not, both learned the hard way on
# 2026-11-15 (docs/work-log-2026-11.md):
#
#   * It **roots what it built**, in /nix/var/nix/gcroots/check-build/<machine>.  A
#     `--no-link` build output has no GC root, so a `nix-collect-garbage` anywhere in the
#     interval between this gate and the deploy deletes every output it produced and the
#     deploy silently recompiles — that cost ~700 derivations on services1 and a second
#     full engine build on services2 in one run.  Release it with `just check-build-clean`
#     once the fleet is landed, or the roots hold superseded closures forever.
#   * It checks the **disk** first, because it assembles all five system closures in the
#     store of whichever box you ran it on.
#
# `just check-build <machine>` builds one machine's toplevel (20 GiB headroom) instead of
# all five (60 GiB), for when only one box's config moved.
# Build every machine's (or one machine's) system toplevel without activating it.
check-build machine="all": (_disk-headroom "60") _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    target="{{ machine }}"
    case "$target" in
        all) machines=(services1 services2 services3 aibox gw) ;;
        services1|services2|services3|aibox|gw) machines=("$target"); just _disk-headroom 20 >/dev/null ;;
        *) echo "unknown machine: $target — expected all, or one of services1 services2 services3 aibox gw" >&2; exit 2 ;;
    esac
    sudo -n mkdir -p /nix/var/nix/gcroots/check-build
    for m in "${machines[@]}"; do
        echo "=== nix build $m toplevel"
        out=$(nix build --no-link --print-out-paths ".#nixosConfigurations.$m.config.system.build.toplevel")
        # A symlink in a gcroots directory *is* a GC root — verified by deleting an
        # unrooted copy of the same path first (it went), then registering it (it stayed).
        sudo -n ln -sfn "$out" "/nix/var/nix/gcroots/check-build/$m"
        echo "    $out  [rooted: /nix/var/nix/gcroots/check-build/$m]"
        echo "    / free now: $(df --output=avail -BG / | tail -1 | tr -dc '0-9')G"
    done
    echo "all requested systems build"
    echo "when the fleet has landed: just check-build-clean"

# Drop the gate's GC roots once the fleet has landed (see check-build).
check-build-clean:
    #!/usr/bin/env bash
    set -euo pipefail
    sudo -n rm -rf /nix/var/nix/gcroots/check-build
    echo "dropped /nix/var/nix/gcroots/check-build (the gate's outputs are collectable again)"

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

# The network-status headless-browser suite: builds the SPA + binary, starts the service on a
# loopback bind with `--local` and with the connection table and the lease file switched off,
# and drives the page with headless Chromium.  Nothing can reach the router: the two files
# that would make a result depend on the box are the router's group-read files
# (`machines/gw/conntrack-reader.nix`, `machines/gw/lease-reader.nix`), so the panels' error
# path is what is under test.  See crates/network-status/tests/README.md.
network-status-test: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop .#wasm --command bash -c 'exec crates/network-status/tests/run.sh'

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

# --- pi's inference endpoints (common/pi-agent.nix, docs/pi-agent-models.md) ------
#
# `programs.pi-agent` declares the network's inference endpoints once
# (common/pi-endpoints.nix) and `crates/pi-agent-sync` asks each one, on every box,
# which models it is running and with how large a context, writing
# `~/.pi/agent/models.json`. A timer does it every 15 minutes; these recipes are
# the operator's half — read what the last run saw, or make it run now. All of them
# invoke the **installed** binary, so none is build-shaped and all are safe on the
# router.
#
# There is deliberately no recipe that edits `models.json`: it is generated, and a
# hand-written provider is preserved by id but overwritten the moment an endpoint of
# that name is probed. Put an endpoint in the table instead.
pi-agent-status user="leigh-admin":
    @pi-agent-sync --report "/home/{{user}}/.pi/agent/pi-agent-status.json"

pi-agent-sync user="leigh-admin":
    #!/usr/bin/env bash
    set -euo pipefail
    unit="pi-agent-{{user}}.service"
    if ! systemctl list-unit-files "$unit" --no-legend | grep -q .; then
        echo "$unit is not a unit on $(hostname): programs.pi-agent is not enabled here,"
        echo "or this box has not switched to a generation that has it. 'just pi-agent-status'"
        echo "is the other half and needs no unit at all."
        exit 1
    fi
    # A oneshot: `restart` blocks until it exits. A dead engine is not a failed unit
    # (the tool exits 0 and records the failure in the status file), so `is-failed`
    # here means the config or the filesystem, and the journal is the answer.
    sudo systemctl restart "$unit"
    if systemctl is-failed "$unit"; then
        echo "$unit failed:"
        journalctl -u "$unit" -n 30 --no-pager
        exit 1
    fi
    pi-agent-sync --report "/home/{{user}}/.pi/agent/pi-agent-status.json"

# --- the hackspace Slack agent ---------------------------------------------------
#
# `hackspace-mcp` (the tools) and `slack-agent` (the harness). The design, the
# measurements behind it and the phase plan are in docs/slack-agent-2026-11-16.md.
#
# Every verb here proves something **without posting to Slack**, which is the
# constraint the whole build order was written around: the Slack leg is the last
# thing switched on, and `--dry-run` rehearses it while sending nothing.
#
# Run them on services1 (read `hostname` first — rule 27). `tools`, `call` and `ask`
# talk to the *deployed* MCP server on 127.0.0.1:8191, and that unit is the only
# process holding the Home Assistant token, so no verb here needs a secret in the
# environment, let alone in argv.

slack-agent-tools: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    # Non-zero if a tool group is missing: a bot that cannot see Home Assistant will
    # still answer, and it will be wrong with confidence.
    nix develop --command bash -c 'cd crates && cargo run -q -p slack-agent --offline -- tools'

slack-agent-probe: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    # Which engine answers, which model id it is serving right now, how fast it decodes.
    # Run after `nix flake update strata`: that bump moves both engines at once, and the
    # ids here are discovered from /v1/models rather than declared anywhere.
    nix develop --command bash -c 'cd crates && cargo run -q -p slack-agent --offline -- probe'

# Call one tool on the running MCP server, raw. No model, no token, no Slack — this
# is the verb that answers "is the workshop temperature tool broken, or is the model".
slack-agent-call tool="space_status" arguments='{}': _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    # Through the environment rather than interpolated into a quoted shell string:
    # the arguments are JSON, and JSON is full of the characters a nested quote level
    # eats (measured: `'{"search":"cam8"}'` arrived as `{search:cam8}`).
    # Tool names and JSON arguments contain no apostrophes by construction (JSON uses
    # double quotes), so one level of single quoting is enough here.
    export MCP_TOOL='{{tool}}'
    export MCP_ARGS='{{arguments}}'
    nix develop --command bash -c 'cd crates && cargo run -q -p slack-agent --offline -- call "$MCP_TOOL" "$MCP_ARGS"' 

slack-agent-ask question: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    # The whole loop, printed: engine chosen, tool calls made, tokens, and the answer.
    # A quoted heredoc, and `ask -`: just has no shell-quoting builtin at this pin, and
    # prose through two shell levels loses its apostrophes (measured: "what's on
    # tonight?" arrives mangled). Stdin cannot be mangled by quoting, and the delimiter
    # being quoted means `$` and backticks in a question stay text.
    # The body and the terminator are indented with the recipe: just dedents recipe
    # lines, and an unindented line would end the recipe.
    cat <<'AGENT_QUESTION' | nix develop --command bash -c 'cd crates && cargo run -q -p slack-agent --offline -- ask -'
    {{question}}
    AGENT_QUESTION

# The fixture: the questions the bot is supposed to be able to answer, run one by one
# with timing, for a human to read. `nix flake update strata`, a tool change or a
# prompt edit all move answers, and "it seemed fine" is not how this fleet decides
# that. Hand-scored by design: what "correct" means for "is it cold" is a judgement
# about a building, and the day that can be asserted is the day it becomes a test.
slack-agent-eval: _build-host-guard
    #!/usr/bin/env bash
    set -uo pipefail
    cd crates
    cargo build -q -p slack-agent --offline
    pass=0; fail=0
    while IFS= read -r line; do
        case "$line" in ''|'#'*) continue ;; esac
        echo "=============================================================="
        echo "Q: $line"
        started=$(date +%s)
        out=$(./target/debug/slack-agent ask "$line" 2>&1) || true
        printf '%s\n' "$out" | sed -n '/^answer:/,$p'
        elapsed=$(( $(date +%s) - started ))
        if printf '%s\n' "$out" | grep -q '^answer:'; then
            pass=$(( pass + 1 )); echo "[answered in ${elapsed}s]"
        else
            fail=$(( fail + 1 )); echo "[NO ANSWER in ${elapsed}s] $(printf '%s\n' "$out" | tail -2)"
        fi
    done < slack-agent/questions.txt
    echo "=============================================================="
    echo "$pass answered, $fail not. Read them: only you know if they are right."

# What the two units, the socket and the engines are doing right now.
# The deterministic calendar path, offline: which tool call a question would produce and
# exactly what the bot would post. No model, no engine, no Slack.
slack-agent-events question: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    # stdin for the same reason `ask` takes it: just has no quoting builtin at this pin
    # and "what's on tonight?" loses its apostrophe through two shell levels.
    cat <<'AGENT_QUESTION' | nix develop --command bash -c 'cd crates && cargo run -q -p slack-agent --offline -- events -'
    {{question}}
    AGENT_QUESTION

# Preview the Monday digest — the exact text and the resolved channel id, posted nowhere.
# The deployed timer runs the same command WITHOUT --dry-run, so this is the one check that
# --- the voice assistant: browser page, cam8 mic, spoken answers --------------------
#
# The gateway is loopback-only on services1, so every one of these asks it *through the
# front door* (`https://voice.int.leighhack.org`) — the same path a phone takes, which is
# the point: if it works here it works in the room.  No recipe here plays anything out
# loud: `voice-say` writes a WAV and stops.  Design and the measured latency budget:
# crates/voice-assistant/README.md.

# What the gateway, the mic and the last answers look like right now.
voice-wake-recs:
    # List what the wake recorder caught, newest first, with what each one is.  The recordings
    # only exist while `infra.voiceAssistant.wake.record` is on and a page has armed server wake,
    # so "nothing here" usually means the flag is off rather than that nobody spoke.
    sudo ls -lht /var/lib/voice-assistant/wake-record 2>/dev/null | head -20 \
      || echo "no recording directory — is infra.voiceAssistant.wake.record on?"

voice-status:
    #!/usr/bin/env bash
    set -euo pipefail
    curl -fsS -m 10 https://voice.int.leighhack.org/status.json | python3 -c '
    import json, sys
    s = json.load(sys.stdin)
    print("gateway: %d requests, %d transcribes, %d asks, %d tts, %d error(s) [%s]"
          % (s["requests"], s["transcribes"], s["asks"], s["tts"], s["errors"], s["last_error"] or "clean"))
    print("config:  whisper=%s agent=%s voice=%s@%d max=%d chars origins=%s record=%s"
          % (s["whisper"].split("//")[1], s["agent"], s["voice"]["name"], s["voice"]["sample_rate"],
             s["max_speech_chars"], ",".join(s["allow_origins"]), s["record_questions"]))
    for q in s.get("recent", [])[:6]:
        print("  %-9s %-14s %-46s %s" % (q.get("at", ""), q.get("route", ""),
                                         (q.get("detail") or "")[:46],
                                         ("ok %.1fs" % q["secs"]) if q.get("ok") else ("FAIL %.1fs" % q["secs"])))
    m = s.get("satellite")
    if not m:
        print("mic:     no satellite reporting (infra.voiceAssistant.satellite.enable?)")
    else:
        print("mic:     %s  source=%s  up %.0fs" % (m["speaker"], m["source"], m["uptime_secs"]))
        print("         floor %.0f dBFS  threshold %.0f  loudest %.0f  speech %d/%.0fs"
              % (m["floor_dbfs"], m["threshold_dbfs"], m["peak_dbfs"], m["speech_frames"], m["captured_secs"]))
        gap = m["threshold_dbfs"] - m["peak_dbfs"]
        if m["windows"] == 0 and m["speech_frames"] == 0:
            print("         -> nothing has been loud enough: %.0f dB below threshold (quiet room, or raise the level)" % gap)
        elif m["windows"] == 0:
            print("         -> %d loud frames never made a window: run gate-check on the clip" % m["speech_frames"])
        else:
            print("         -> %d window(s), %d ignored, %d asked, %d answered, %d unplayed(speaker), %d ffmpeg restart(s)"
                  % (m["windows"], m["ignored"], m["asked"], m["answers"], m["unplayed"],
                     # Named for the event, not the tool, since the ingest is a trait now.  Read
                     # both spellings: this verb runs against whichever generation is deployed,
                     # and a KeyError here loses the whole status render for want of one line.
                     m.get("ingest_restarts", m.get("ffmpeg_restarts", 0))))
        if m["last_transcript"]:
            print("         last: %r -> %s" % (m["last_transcript"][:60], m["last_action"]))
        if m["last_error"]:
            print("         last error: %s" % m["last_error"])
    '

# One question, spoken-answer included, and no audio path at all: prints the text, the
# timing, the tool calls, and writes the speech to a file instead of playing it.
voice-ask question:
    #!/usr/bin/env bash
    set -euo pipefail
    # stdin, for the reason every other recipe here gives: just has no quoting builtin at
    # this pin and "what's on tonight?" loses its apostrophe through two shell levels.
    # The question goes through stdin into a python one-liner that *encodes* it, because
    # just has no quoting builtin at this pin and a question is prose: it has apostrophes,
    # and the JSON string needs those and any quotes inside it escaped by something that
    # understands JSON rather than by a shell.
    cat <<'VO_ASK' | python3 -c 'import json,sys; sys.stdout.write(json.dumps({"text": sys.stdin.read().strip()}))' \
        | curl -fsS -m 180 -X POST -H 'content-type: application/json' \
            --data-binary @- https://voice.int.leighhack.org/v1/ask > /tmp/voice-ask.json
    {{question}}
    VO_ASK
    # A question this cheap is worth the two lines of output it costs: the same turn is
    # 8-150 s depending on whether the model called tools, and a number is the only way to
    # know which one you just waited for.
    python3 -c 'import json,base64; a=json.load(open("/tmp/voice-ask.json"))
    print("answer: " + a["answer"])
    print("speech: " + a.get("speech","(nothing to say)"))
    if a.get("audio"):
        open("/tmp/voice-answer.wav","wb").write(base64.b64decode(a["audio"]))
        print("wrote:  /tmp/voice-answer.wav (%.1f s) — nothing has played" % a.get("audio_secs",0))
    print("[%.1fs total · agent %.1fs · %s · %d round(s) · tools: %s]" % (a["secs"], a.get("agent_secs",0), a.get("engine","-"), a.get("rounds",0), ", ".join(a.get("tools") or []) or "none"))
    for w in a.get("warnings") or []: print("WARN: " + w)
    '

# Text-only: synthesise speech to a WAV and say nothing.  This is the half that is safe to
# run while nobody has confirmed what a C110 speaker sounds like across a working room.
voice-say text:
    #!/usr/bin/env bash
    set -euo pipefail
    cat <<'VO_SAY' | python3 -c 'import json,sys; sys.stdout.write(json.dumps({"text": sys.stdin.read().strip()}))' \
        | curl -fsS -m 60 -X POST -H 'content-type: application/json' \
            --data-binary @- -o /tmp/voice-say.wav https://voice.int.leighhack.org/v1/tts
    {{text}}
    VO_SAY
    python3 -c 'import wave; w=wave.open("/tmp/voice-say.wav"); print("/tmp/voice-say.wav: %d ch, %d Hz, %.1f s — play it yourself if you want to hear it" % (w.getnchannels(), w.getframerate(), w.getnframes()/w.getframerate()))'

# Does the camera's microphone path work, and what would the gate have done with it?
# Runs the deployed binary on services1 (it is a system package) against go2rtc's cam8
# restream, transcribes, and STOPS: no question reaches the assistant and no answer is
# spoken.  The measurement tool for the one thing this cannot answer at a desk: how often
# the space's music looks like speech.
voice-probe seconds="8": _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    # The binary is a package of services1's system, so this is a services1 command:
    # build-shaped recipes already refuse on gw, and the PATH check says which box it
    # actually wants.
    if ! command -v voice-assistant >/dev/null; then
        echo "voice-assistant is not on PATH here." >&2
        echo "  It is a package of services1's system: run this on services1," >&2
        echo "  or build it: nix build .#packages.x86_64-linux.voice-assistant" >&2
        exit 1
    fi
    # `--state-dir` is the tmpfs one, not /var/lib/voice-assistant: the unit owns that
    # directory and a hand-run probe writing answers into it would leave root-owned files
    # where the DynamicUser has to write.  The clip lands in the caller's /tmp instead.
    sudo voice-assistant probe \
        --state-dir /tmp/voice-probe-state \
        --seconds "{{seconds}}" \
        --out /tmp/voice-probe.wav

# a renamed #general or a revoked token shows up before a member notices on a Monday.
slack-agent-digest: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    cd crates && cargo build -q -p slack-agent --offline
    # The tokens go to the process environment, never argv (world-readable on Linux), and
    # nothing here prints them: the digest's own output is the calendar.
    sudo -n bash -c 'set -a; . /run/secrets/slack_agent_env; set +a; ./target/debug/slack-agent digest --dry-run'

slack-agent-status:
    #!/usr/bin/env bash
    set -uo pipefail
    for unit in hackspace-mcp.service slack-agent.service slack-agent-digest.service slack-agent-digest.timer; do
        printf '%-24s %s / %s\n' "$unit" "$(systemctl is-enabled "$unit" 2>&1)" "$(systemctl is-active "$unit" 2>&1)"
    done
    echo "--- socket and turn liveness (127.0.0.1:8192) ---"
    curl -fsS -m 5 http://127.0.0.1:8192/health || echo "health endpoint not answering"
    curl -fsS -m 5 http://127.0.0.1:8192/metrics | grep -E '^slack_agent_' || true
    echo "--- tool server (127.0.0.1:8191) ---"
    curl -fsS -m 5 http://127.0.0.1:8191/health || echo "mcp health endpoint not answering"
    curl -fsS -m 5 http://127.0.0.1:8191/metrics | grep -E '^hackspace_mcp_' || true
    echo "--- recent journal ---"
    journalctl -u slack-agent.service -n 15 --no-pager -o cat 2>/dev/null | tail -15

# Serve with the real Slack events but **nothing posted**. This is the last check
# before the bot speaks to people: the events flow, the decisions log, and no member
# of the workspace can tell the difference. Ctrl-C ends it.
slack-agent-dry-run: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop --command bash -c 'cd crates && cargo run -q -p slack-agent --offline -- serve --dry-run'


# --- pi's web search (common/pi-search.nix, docs/pi-web-search.md) ----------------
#
# `programs.pi-agent.search.brave` (default: on wherever `programs.pi-agent` is on)
# puts one Brave key in sops, gives it to one group, declares one pinned pi package,
# and owns the two keys of `~/.pi/agent/web-search.json` that say which provider to use
# and where its credential comes from. `just pi-agent-sync` is what applies the last
# two; this recipe is the one that answers "is it working, as me, here" — the four
# facts a "the agent has no web search" report is hiding behind, in the order they
# actually break.
#
# It prints no secret: the key is read into a curl config handed to curl on **stdin**,
# never into its arguments (a command line is world-readable on Linux), and the only
# things said about it are its mode, its group and its HTTP status. Not build-shaped,
# so it is safe on gw.
pi-search-status user="leigh-admin":
    #!/usr/bin/env bash
    set -uo pipefail
    agent="/home/{{user}}/.pi/agent"
    key="${BRAVE_API_KEY_FILE:-/run/secrets/brave_api_key}"

    echo "=== $(hostname), as $(whoami)"
    echo "--- the credential"
    if [ -r "$key" ]; then
        echo "  readable: $key ($(stat -c 'mode %a, owner %U, group %G' "$key"), $(wc -c < "$key" | tr -d ' ') bytes)"
    else
        echo "  NOT READABLE: $key"
        echo "  it is 0440 root:brave-search-readers, and that group's members are"
        echo "  programs.pi-agent.users. $(whoami) is in:"; id -nG | sed 's/^/    /'
        if getent group brave-search-readers | grep -q "$(id -un)"; then
            # The common case, and not a permissions bug: /etc/group changed at the
            # switch, and a process keeps the supplementary groups it was started
            # with — so the tmux server, and every pi inside it, predates the grant.
            echo "  BUT /etc/group DOES list $(id -un): this session (and its tmux server,"
            echo "  and the pi inside it) predates the switch. New logins are fine; to fix"
            echo "  the running one, restart the tmux server (or log out and back in)."
        else
            echo "  and /etc/group does not list this account either, so this box has not"
            echo "  switched to a generation with common/pi-search.nix, or the user is not in"
            echo "  programs.pi-agent.users."
        fi
    fi

    echo "--- a search with it (Brave, count=1)"
    if [ -r "$key" ]; then
        code=$( { printf 'silent\nshow-error\nmax-time = 20\noutput = "/dev/null"\nurl = "https://api.search.brave.com/res/v1/web/search?q=nixos&count=1&text_decorations=false"\nwrite-out = "%%{http_code}"\nheader = "Accept: application/json"\nheader = "X-Subscription-Token: %s"\n' "$(cat "$key")"; } | curl --config - 2>/dev/null)
        case "$code" in
            200) echo "  200 ok — the key works and this box can reach the API" ;;
            401 | 403) echo "  $code — Brave refused the key. It is the one in sops; check it is still the current one (docs/follow-ups.md)." ;;
            402) echo "  402 — the plan behind the key has no credit left." ;;
            429) echo "  429 — rate limited. Brave's own window; try again in a minute." ;;
            000 | "") echo "  no answer — no route or no DNS from this box (services3's uplink is a USB dongle; the WAN backup gives no inbound but does give out)." ;;
            *) echo "  $code — unexpected; curl's body was thrown away (output=/dev/null), so re-run by hand if it repeats." ;;
        esac
    else
        echo "  skipped: no credential to ask with"
    fi

    echo "--- the two files the timer merges"
    for f in settings.json web-search.json; do
        if [ -e "$agent/$f" ]; then
            printf '  %-16s %s\n' "$f" "$(stat -c 'mode %a, owner %U:%G' "$agent/$f")"
        else
            printf '  %-16s MISSING\n' "$f"
        fi
    done
    if grep -q 'pi-web-access' "$agent/settings.json" 2>/dev/null; then
        echo "  pi-web-access is declared: $(grep -o 'npm:pi-web-access[^"]*' "$agent/settings.json" | head -1)"
    else
        echo "  pi-web-access is NOT declared in settings.json — the timer has not run, or it refused to touch the file: 'just pi-agent-sync' prints why."
    fi
    grep -o '"provider": *"[a-z-]*"' "$agent/web-search.json" 2>/dev/null | head -1 | sed 's/^/  /' || echo "  no provider line in web-search.json"

    echo "--- pi's own view (it installs the package on first start, from the npm registry)"
    if command -v pi >/dev/null 2>&1; then
        pi list 2>&1 | tail -4
    else
        echo "  pi is not on this account's PATH, so there is nothing to install it."
    fi

# --- the cameras (OpenIPC on the WiFi VLAN) ---------------------------------
#
# `cam5` `cam7` `cam9` are Buildroot/OpenIPC boxes on VLAN 226 (10.3.2.10/.12/.17,
# pinned by MAC in `common/dns-records.nix`, which is the table these recipes read —
# there is no second copy of the addresses to drift).  They run no NixOS and no
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
