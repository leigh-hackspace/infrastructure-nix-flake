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
# build-shaped recipe below depends on this guard.  What it replaced — the devShell that *was*
# a build, and the split toolchain that closed it — the full allowed/refused table, and what to
# read if rustc ever appears on the router again: docs/nix-dev-workflow.md,
# "Never build on the router".
#
# Build on aibox (the network's builder) or services1.  If you genuinely need to
# build on the router, say so explicitly.
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

# --- the router (gw)
#
# It is the network's router as of 2026-10-21, so it answers on `10.3.1.1` —
# `machines/gw/coexist.nix` and the whole co-existence mode were deleted on 2026-10-22.
# `10.3.1.254` was the co-existence address: it belongs to generations built before the
# cutover, so pass `GW_HOST=10.3.1.254` only if you are working on one of those.
GW_HOST := "10.3.1.1"
#
# Deployed by building *here* and pushing to the router.  It used to build on the
# router itself ("it has the checkout, git, the sops key"), but since 2026-10-21 the
# router's system contains a Rust crate (machines/gw/network-status.nix), and
# building Rust on the box that answers DNS/DHCP for the network is exactly what
# AGENTS.md's "Never build on the router" forbids.  So this recipe is build-shaped
# and refuses to run on gw, like the rest; run it from services1 (where the tree
# is edited) or aibox.  Nixos-rebuild builds locally — on services1 that delegates
# to aibox — then copies the closure to the target and runs activation there.
#
# `sync-gw` still runs first: the router keeps a checkout as a mirror for reading
# and for the emergency fallback, and it must not drift from the tree being
# deployed.
#
# The switch and the confirm are chained with &&: AGENTS.md's GOLDEN RULE, and
# `nixos-confirm` has to run on the machine that was switched (in the same shell
# invocation), so a failed activation is never confirmed.  NIX_SSHOPTS carries the
# machine-hop key: the default ssh identity is not authorised on gw, and
# nixos-rebuild-ng uses plain ssh to reach --target-host.
#
# `--flake .#gw` names the machine explicitly; it cannot pick the local hostname
# by accident the way `--flake .` could if this ever ran somewhere else.

# Mirror this checkout onto the router.  The excludes are the build trees: 6.7 GB
# of crates/target and node_modules that the flake rebuilds itself (and that
# `lib.cleanSourceFilter` does not read .gitignore, so carrying them over is how a
# stray 3 GB directory ends up next to a router's config — see
# docs/nix-dev-workflow.md).  `--delete` so a file removed here is gone there too:
# the router's checkout is a mirror, not a second place to edit.
#
# `.git/config` is the one file that is *not* mirrored, because it is the only part
# of `.git` that is per-box rather than per-commit: the router's own remotes (it has
# an `infra` remote pointing at services1's checkout, because it has no GitHub key of
# its own) are wiped by a mirror of services1's config, which is how the first
# version of this recipe silently deleted it.
sync-gw:
    #!/usr/bin/env bash
    set -euo pipefail
    rsync -a --delete \
      --exclude='target/' --exclude='node_modules/' --exclude='/result' --exclude='/result-*' \
      --exclude='/.git/config' \
      -i -e "ssh -i ~/.ssh/agent-hop-key -o BatchMode=yes -o ConnectTimeout=5" \
      ./ leigh-admin@{{ GW_HOST }}:/home/leigh-admin/Projects/infrastructure-nix-flake/

switch-gw: _build-host-guard sync-gw
    #!/usr/bin/env bash
    set -euo pipefail
    NIX_SSHOPTS="-i $HOME/.ssh/agent-hop-key -o BatchMode=yes -o ConnectTimeout=5" \
      nixos-rebuild switch --flake .#gw \
        --target-host leigh-admin@{{ GW_HOST }} \
        --elevate=sudo \
      && ssh -i ~/.ssh/agent-hop-key -o BatchMode=yes -o ConnectTimeout=5 \
           leigh-admin@{{ GW_HOST }} 'sudo nixos-confirm'

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

# cargo clippy over the same set.  Warnings are reported, not fatal: the point
# is to see them (they used to go entirely unchecked).
clippy: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    bash common/shared-rs-links.sh
    nix develop --command bash -c '
      cd crates && cargo clippy --workspace --all-targets --offline 2>&1 \
        | grep -vE "^(Compiling|Checking|Finished|    Finished)" || true'

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

pxe-client-bios:
    sudo qemu-system-x86_64 \
        -m 4096 \
        -accel kvm \
        -smp 4 \
        -netdev tap,id=net0,br=br227,helper=$(type -p qemu-bridge-helper) \
        -device virtio-net-pci,netdev=net0 \
        -display vnc=:0 \
        -vga qxl \
        -boot n \

pxe-client-uefi:
    #!/usr/bin/env bash
    set -euo pipefail

    OVMF_PATH=$(nix build --print-out-paths nixpkgs#OVMF.fd)

    mkdir -p ./tmp

    cp $OVMF_PATH/FV/OVMF_CODE.fd ./tmp
    cp $OVMF_PATH/FV/OVMF_VARS.fd ./tmp

    sudo chown leigh-admin:users ./tmp/*.fd
    chmod +xwr ./tmp/*.fd

    sudo qemu-system-x86_64 \
    -m 4096 \
    -cpu host \
    -accel kvm \
    -smp 4 \
    -machine q35,smm=on -global driver=cfi.pflash01,property=secure,value=on \
    -object rng-random,id=virtio-rng0,filename=/dev/urandom \
    -device virtio-rng-pci,rng=virtio-rng0,id=rng0,bus=pcie.0,addr=0x9 \
    -netdev tap,id=net0,br=br227,helper=$(type -p qemu-bridge-helper) \
    -device virtio-net-pci,netdev=net0 \
    -drive file=./tmp/OVMF_CODE.fd,if=pflash,format=raw,unit=0,readonly=on \
    -drive file=./tmp/OVMF_VARS.fd,if=pflash,format=raw,unit=1 \
    -display vnc=:0 \
    -vga qxl \
    -boot n \

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
    echo "=== alejandra --check"
    just fmt-check

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
        echo "go2rtc feed end not checked: set CAM_FEED to the name in services1's go2rtc stream table (see cameras/README.md: cam5 -> main_space, cam7 -> pi_room, cam9 -> woodwork)"
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
