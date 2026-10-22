# https://just.systems

default:
    @ {{just_executable()}} --list --justfile {{justfile()}} --unsorted

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
# The router must not compile anything.  Since 2026-10-21 it also cannot: gw sets
# `infra.buildFarm.builderHost = HOSTS.aibox` and `nix.maxJobs = 0`, so a build started
# here is built on aibox or fails — the second half is what removes the silent local
# fallback (common/build-farm.nix documents that `--max-jobs 0` "does not enable
# delegation, it removes the local fallback").  Before that, a build here ran locally on
# 8 GB and 4 cores, and `nix develop` **is** a build: the devShell depends on
# CRANE.wasmBindgenCli, compiled from source with cargo.  On 2026-10-21 a `just check` on
# the gateway (`fmt-check` → `nix develop`) put four rustc processes on the box that
# answers DNS for the whole network.
#
# The guard stays anyway, and the reason is not compilation: **evaluation still happens
# on the box you are standing on**, and a 4-core router is also the network's resolver.
# `check` and `fmt` are eval/format only and stay allowed there; every build-shaped
# recipe below depends on this guard.
#
# Build on aibox (the network's builder) or services1.  If you genuinely need to
# build on the router, say so explicitly.
_build-host-guard:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "$(hostname)" = "gw" ] && [ -z "${INFRA_ALLOW_BUILD_ON_ROUTER:-}" ]; then
        echo "refusing: this is gw, the network's router — builds delegate to aibox and evaluation costs this box." >&2
        echo "  Run this from aibox or services1 instead (AGENTS.md, 'Don't build on the router')." >&2
        echo "  Nothing here compiles locally either way: nix.maxJobs is 0, so there is no fallback to fall back to." >&2
        echo "  Override with INFRA_ALLOW_BUILD_ON_ROUTER=1 only if you really mean it." >&2
        exit 1
    fi

# --- the router (gw)
#
# It is the network's router as of 2026-10-21 (`machines/gw/coexist.nix` is
# `enable = false`), so it answers on `10.3.1.1`. `10.3.1.254` was the co-existence
# address: it belongs to generations built before the cutover, so pass
# `GW_HOST=10.3.1.254` only if you are working on one of those.
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
      ./ leigh-admin@{{GW_HOST}}:/home/leigh-admin/Projects/infrastructure-nix-flake/

switch-gw: _build-host-guard sync-gw
    #!/usr/bin/env bash
    set -euo pipefail
    NIX_SSHOPTS="-i $HOME/.ssh/agent-hop-key -o BatchMode=yes -o ConnectTimeout=5" \
      nixos-rebuild switch --flake .#gw \
        --target-host leigh-admin@{{GW_HOST}} \
        --elevate=sudo \
      && ssh -i ~/.ssh/agent-hop-key -o BatchMode=yes -o ConnectTimeout=5 \
           leigh-admin@{{GW_HOST}} 'sudo nixos-confirm'

# --- Nix formatting ---

# Format every tracked .nix file with alejandra (the repo standard: it is what
# .zed/settings.json runs on save).  `nix fmt` takes the formatter from the flake's
# pinned nixpkgs (flake.nix's `formatter`), i.e. from the binary cache — NOT from
# the devShell, because `nix develop` would build the pinned wasm-bindgen-cli and
# a formatter must never be the reason a box compiles Rust (see `_build-host-guard`).
# docs/repo-audit-2026-10-07.md §1.3 is why the whole tree is kept clean.
fmt:
    nix fmt -- $(git ls-files '*.nix')

# The check half of the same rule, wired into `just check`.
fmt-check:
    #!/usr/bin/env bash
    set -euo pipefail
    nix fmt -- --check $(git ls-files '*.nix')

# --- Rust crates (toolchain from `nix develop`: cargo/rustc/rustfmt/clippy +
# --- the pinned wasm-bindgen-cli)
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
# (Needs `nix develop`, which builds the devShell — hence the guard.)
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

# Rebuild a Dioxus SPA into its frontend/dist using the devshell's toolchain
# (cargo/rustc/lld + the pinned wasm-bindgen-cli).  Same recipe the flake runs
# (common/frontend-build-spa.sh vs CRANE.wasmSpa), so the local dist/ matches
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
    case {{quote(crate)}} in
        all) crates="frigate-monitor filestore gocardless-dashboard switch-gui" ;;
        *) crates={{quote(crate)}} ;;
    esac
    for c in $crates; do
        if [ ! -f "crates/$c/frontend/build.sh" ]; then
            echo "no crates/$c/frontend/build.sh — expected one of: frigate-monitor filestore gocardless-dashboard switch-gui all" >&2
            exit 2
        fi
        echo "=== build-frontend $c"
        nix develop --command bash -c "exec ./crates/$c/frontend/build.sh"
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

# Rebuild+switch for the netboot setup, then reboot the netbooted clients.
#
# The image is `machines/pi-room-sys` — an in-tree machine, so edit it and
# rebuild: there is no pin to move any more (it used to be a `git+file://` input
# with a `just update-netboot-input` to drag flake.lock along, and the pin going
# stale in silence is exactly why that is gone).  Boot flow and what aibox
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

# --- filestore headless-browser test suite ---

# Nix-level checks: the flake evaluates, the machines' option assertions hold,
# the hand-rolled config serializers and the generated printer-state tables match
# their expectations, and every tracked .nix file is alejandra-clean.  Fast
# (evaluation only, no compilation).  The rest of the test surface: `just test`
# (cargo tests), `just clippy` (lints), `just filestore-test` (headless-browser
# suite) and `just check-build` (the deploy-readiness build).
check:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "=== nix flake check"
    nix flake check --no-build
    echo "=== config-to-gitlab assertions"
    nix eval --impure --raw -f machines/services1/lib/check-config-to-gitlab.nix
    echo "=== printer-state tables (exporter <-> dashboards/alerts)"
    nix eval --impure --raw -f common/check-printer-states.nix
    # The router's two network tables: common/vlans.nix and common/dns-records.nix
    # generate dnsmasq's conf, nftables' rules and the VLAN devices, so a VLAN id
    # that is not in the first table or an address claimed twice is a client that
    # gets the wrong address rather than a build failure.  The same assertions run
    # as a module (machines/gw/lib/assert-network-tables.nix) and so in the `nix
    # flake check` above; this is the version that prints which record is wrong.
    echo "=== router network tables (vlans <-> dns-records <-> ACL)"
    nix eval --impure --raw -f machines/gw/lib/check-network-tables.nix
    # The gatus endpoint guard is a module assertion in
    # machines/services1/services/monitoring.nix, so the `nix flake check` above
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
    nixos-rebuild build --flake ".#{{machine}}" --max-jobs 0

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
    nix develop --command bash -c 'exec crates/filestore/tests/run.sh'

# The switch-gui headless-browser suite: builds the SPA + binary, starts a fake
# ProCurve console (a real Telnet server with an in-memory model), points the
# service at it with --no-auth on a loopback bind and a scratch Postgres schema,
# and drives the matrix with headless Chromium.  Nothing can reach a real switch:
# the only address the service is given is 127.0.0.1.  Needs the local Postgres
# cluster (see SG_TEST_DB_URL in crates/switch-gui/tests/run.sh).
switch-gui-test: _build-host-guard
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop --command bash -c 'exec crates/switch-gui/tests/run.sh'

# Install the shared age key (used to edit secrets) so sops-nix can
# decrypt at runtime and build time on this machine. Run after a fresh
# 'nixos-rebuild'. The nixbld group ownership is what lets nix builds
# decrypt secrets (see common/sops.nix).
install-sops-key:
    #!/usr/bin/env bash
    set -euo pipefail

    sudo install -d -m 550 -g nixbld /var/lib/sops-nix
    sudo install -m 440 -g nixbld "{{SOPS_KEY_FILE}}" /var/lib/sops-nix/key.txt

edit-secrets:
    sops edit secrets/secrets.yaml
