# JUST — the deploy-readiness build and the upgrade safety probes.
#
# Part of the justfile: the root justfile imports these, so `just` still
# resolves every recipe from the repo root and `just --list` still shows
# them.  Split by job (AGENTS.md rule 26), so no file here is over 1000
# lines, and the guard that enforces that rule now sees justfiles.

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

# The store lock, and the process holding it.
#
# Empty/"free" means the store is idle.  A nix command that hangs with no CPU, no output
# and no timeout is queued behind this lock, and "slow" is what it looks like — measured
# twice on 2026-11-15, once for 1 h 56 min, both times held not by a build but by `nixd`
# (an editor's language server) via a `nix-daemon` child, which no `pgrep -f "nix eval|nix
# build"` finds.  `/proc/locks` names the inode and the pid directly, so this needs no
# scan and no sudo.  Run it before blaming a slow build; the fix is in docs/nix-dev-workflow.md.
#
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
#
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

# Which derivation in a machine's build closure asks nix for the network, and what that
# does to a gate run on a box that will not grant it.  Read-only; a report, not a gate
# (exit 0 either way).
#
# A derivation that sets `__noChroot` is the one thing that makes a build **box-dependent**:
# nix refuses it outright on a daemon whose `sandbox` is `true` — `has '__noChroot' set,
# but that's not allowed when 'sandbox' is 'true'` — so a gate run on such a box stops at
# that derivation and says nothing about the change in front of you.  That is what happened
# on 2026-11-22, when the compile gate had to move from aibox to services1 (docs/work-log-2026-11.md,
# docs/nix-dev-workflow.md).  `nix.settings.sandbox = "relaxed"` is what grants it, per box,
# and `machines/aibox/configuration.nix` and `machines/services1/configuration.nix` say who
# carries it and why.  This names it, so "which box can build what" is a question you can
# ask instead of one you learn by losing an hour to a refused derivation — and it is the
# re-verification those two module comments ask for, wrapped.
#
# Cheap: one closure walk and ~7.5k greps, seconds.
#
# What asks for the network exemption in this box's (or the named machine's) closure?
noChroot machine="":
    #!/usr/bin/env bash
    set -euo pipefail
    probe='set -u
        sys=$(nix-store -qd /run/current-system)
        total=$(nix-store -qR "$sys" 2>/dev/null | grep -c "\.drv$")
        hits=$(nix-store -qR "$sys" 2>/dev/null | grep "\.drv$" | xargs -r grep -l "__noChroot" /dev/null)
        echo "closure: $total derivations"
        if [ -z "$hits" ]; then
            echo "asks: none"
            echo "VERDICT: this box builds its whole closure under sandbox = true"
        else
            printf "%s\n" "$hits" | sed "s|/nix/store/[^-]*-||"
            n=$(printf "%s\n" "$hits" | wc -l)
            echo "VERDICT: $n derivation(s) need sandbox = relaxed; a daemon without it refuses them"
        fi'
    if [ -z "{{ machine }}" ] || [ "{{ machine }}" = "$(hostname)" ]; then
        echo "=== $(hostname)"
        bash -c "$probe"
    else
        case "{{ machine }}" in
            services1|services2|services3|aibox|gw) ;;
            *) echo "unknown machine: {{ machine }}" >&2; exit 2 ;;
        esac
        case "{{ machine }}" in gw) hkey=router ;; *) hkey="{{ machine }}" ;; esac
        addr=$(nix eval --impure --raw --expr "let h = import ./common/hosts.nix; in h.$hkey")
        echo "=== {{ machine }} ($addr)"
        # Over stdin, not as ssh arguments: ssh word-splits its command arguments, which
        # runs `bash -c set` instead of the probe (same reason as just pending-reboot).
        printf "%s\n" "$probe" | ssh -i "$HOME/.ssh/agent-hop-key" -o BatchMode=yes -o ConnectTimeout=5 \
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
# before a deploy rather than on every edit, detached, and poll it (rule 11).
#
# Since 2026-11-23 it compiles on **the box you type it on** — there is no delegated
# builder any more (common/nix-gc.nix's header) — so where you type it is a decision:
# aibox, because it has 59 GiB and 12 threads and is the only box that can hold five
# system closures and keep going; not gw (7 GiB, and `_build-host-guard` refuses it
# anyway); and not services2 despite its 16 cores, because its 30 GiB is its engine's
# and its measured headroom is ~1.5 GiB.  On aibox the gate now runs inside the
# engine's box's bounds — 4 jobs and a 12/18 GiB build cgroup, ./configuration.nix — so
# a bump against a loaded arena is slower than it was when aibox had 12 unbounded jobs
# and no engine.  That is the trade that was chosen: the endpoint answers, the build
# queues.  Poll load and `/ free` as well as this log.
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

# --- what used to be here -------------------------------------------------------
#
# `build-remote machine=…` ran `nixos-rebuild build --max-jobs 0` to force a build onto
# aibox and refuse the local fallback.  It went with the rest of the remote-build config
# on 2026-11-23 (common/nix-gc.nix's header says why, common/ssh-hop.nix's footer says
# what it would take to want it back — `--max-jobs 0` is item 4 on that list, and it is
# still the right idiom for "fail loudly rather than compile here").  Nothing to
# substitute: type `just switch` or `just check-build`, and read `nix config show | grep
# max-jobs` if you are wondering what a box is allowed to spend on you.

update-pkgs:
    nix flake update nixpkgs
