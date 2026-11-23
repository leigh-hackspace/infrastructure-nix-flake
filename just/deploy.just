# JUST — the recipes that change a running machine: switch, boot, deploy, hard reboot, and the guard that says where a build may happen.
#
# Part of the justfile: the root justfile imports these, so `just` still
# resolves every recipe from the repo root and `just --list` still shows
# them.  Split by job (AGENTS.md rule 26), so no file here is over 1000
# lines, and the guard that enforces that rule now sees justfiles.

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
# The router may compile, but only for itself, and only a little: since 2026-11-23 gw
# builds its own closures at `nix.settings.max-jobs = 2` inside a `nix-daemon` cgroup
# capped at 3 GiB throttled / 4.5 GiB refusing, out of its 7 GiB
# (`machines/gw/default.nix`).  Before that it delegated everything to aibox and held
# `max-jobs = 0`, which was the half that removed the silent local fallback rather than
# the half that enabled delegation.
#
# The guard stays, and the reason is not the compile alone: **evaluation still happens on
# the box you are standing on**, and a 4-core router is also the network's resolver.  A
# five-toplevel `just check-build` or a crate test suite on 7 GiB is the thing being
# refused, not the router's own switch.
# `check-eval` and `fmt` are eval/format only and stay allowed there; `check` is allowed too
# but skips its flake step here, with the reason printed (see that recipe).  Every
# build-shaped recipe below depends on this guard — `deploy`/`switch` deliberately do not:
# they are how a machine installs itself, and the compile it needs is its own, bounded.
# What it replaced — the devShell that *was*
# a build, and the split toolchain that closed it — the full allowed/refused table, and what to
# read if rustc ever appears on the router again: docs/nix-dev-workflow.md,
# "The router builds for itself, bounded".
#
# If you genuinely need the router to compile the fleet, say so explicitly.  Deploying the
# router is *not* a build on the router in this guard's sense and is not guarded: `just
# switch` here, or `just deploy gw` from anywhere, evaluates and activates on gw and
# compiles there only if a crate or nixpkgs moved — which is what every other box does for
# itself.
_build-host-guard:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "$(hostname)" = "gw" ] && [ -z "${INFRA_ALLOW_BUILD_ON_ROUTER:-}" ]; then
        echo "refusing: this is gw, the network's router — 4 cores, 7 GiB, and it answers DNS." >&2
        echo "  It builds for itself (max-jobs 2, capped cgroup); it is not the fleet's build host."
        echo "  Run this from aibox instead (AGENTS.md rule 22)." >&2
        echo "  Override with INFRA_ALLOW_BUILD_ON_ROUTER=1 only if you really mean it." >&2
        exit 1
    fi

# --- deploying a machine: from that machine ------------------------------------
#
# `just switch` is this box.  `just deploy <machine>` is any box: it sshs to the target,
# brings *the target's own checkout* to the commit this box is on, and runs
# `nixos-rebuild switch` **there** — so the machine that evaluates, activates and confirms is
# the machine being deployed.  Since 2026-11-23 the compile is the **target's own** (nothing
# delegates anywhere; `common/nix-gc.nix`'s header is the withdrawal), and the point is
# unchanged: no box is the fleet's deploy host.
#
# This replaces the pair that made services1 the centre of the fleet: `switch-gw` built *here*
# and pushed the closure to the router with `--target-host`, and `sync-gw` rsynced this whole
# working tree over the router's checkout with `--delete`.  Both were written when gw had no
# builder and no GitHub key.  Neither is true any more: gw builds its own toplevel (bounded,
# `machines/gw/default.nix`), it has a GitHub key of its own for fetching the commit
# (rule 33), and pushing a closure to it would need a daemon-trust grant it no longer has
# (`machines/gw/configuration.nix`).  So
# the commit travels as a git fetch from whichever box ran the recipe rather than as an rsync
# of a working tree — which is also why `sync-gw`'s `.git/config`-eating bug
# (`docs/follow-ups.md`) is closed by deleting the recipe instead of by another exclusion.
#
# What it costs: the target evaluates its own configuration, and the target pays for it.  On
# gw that is seconds of the network's resolver per deploy, which is the honest price of not
# making another machine the deploy host; if a crate or nixpkgs moved, it also pays a bounded
# compile there (`max-jobs = 2`, 3/4.5 GiB cgroup).
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
    # /run/current-system, which arms the five-minute rollback countdown on the box that
    # hosts the engine.  Unmounting first (lazily, so a client mid-boot keeps its handles) makes
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
# deploying gw like any other box — the difference is only that a compile there is capped
# to 2 jobs and 4.5 GiB, because it is the network's resolver (`machines/gw/default.nix`).
switch-gw: (deploy "gw")
