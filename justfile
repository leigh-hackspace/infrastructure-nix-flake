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

# --- the router (gw, 10.3.1.254)
#
# Deployed the same way as every other machine — built and switched from its own
# checkout, because that box has the repo, `git`/`just`, root's `safe.directory`
# for the checkout and the sops key (machines/gw/README.md says how it got them).
# It is a ssh recipe rather than something you run there because the change you are
# deploying is in *this* working tree, and the router cannot read it until it is
# copied over — which is what `sync-gw` is for, and why `switch-gw` depends on it.
#
# The switch and the confirm are in one remote invocation, chained with &&, on the
# machine that was switched: AGENTS.md's GOLDEN RULE, and `nixos-confirm` here on
# services1 would confirm nothing (it confirms the local system).
#
# `--flake .` on the router resolves against its hostname, which is `gw`, so it
# cannot build the wrong machine by forgetting an argument.

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
      ./ leigh-admin@10.3.1.254:/home/leigh-admin/Projects/infrastructure-nix-flake/

switch-gw: sync-gw
    #!/usr/bin/env bash
    set -euo pipefail
    ssh -i ~/.ssh/agent-hop-key -o BatchMode=yes -o ConnectTimeout=5 leigh-admin@10.3.1.254 \
      'cd ~/Projects/infrastructure-nix-flake && sudo nixos-rebuild switch --flake . && sudo nixos-confirm'

# --- Nix formatting ---

# Format every tracked .nix file with alejandra (the repo standard: it is what
# .zed/settings.json runs on save and what the devshell ships).  The whole tree
# was reformatted in one sweep, so keep it that way — a stray unformatted file
# makes the next real diff unreadable (docs/repo-audit-2026-10-07.md §1.3).
fmt:
    nix develop --command bash -c 'git ls-files "*.nix" | xargs -r alejandra'

# The check half of the same rule: lists every file that is not alejandra-clean
# and exits non-zero.  Wired into `just check`.
fmt-check:
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop --command bash -c '
      bad=0
      for f in $(git ls-files "*.nix"); do
        alejandra --check "$f" >/dev/null 2>&1 || { echo "not alejandra-clean: $f"; bad=1; }
      done
      exit $bad'

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
test:
    #!/usr/bin/env bash
    set -euo pipefail
    bash common/shared-rs-links.sh
    nix develop --command bash -c 'cd crates && cargo test --workspace --offline'

# cargo clippy over the same set.  Warnings are reported, not fatal: the point
# is to see them (they used to go entirely unchecked).
clippy:
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
build-frontend crate="all":
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
# machines/services1/network-status.nix).  Build it with npm, then commit the
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


# --- DNS sync (keeps router dnsmasq + DigitalOcean DNS in step with the
# --- *.int.leighhack.org nginx vhosts; requires the machine-hop key and a
# --- deployed dns-sync on services1) ---

# Report whether every *.int.leighhack.org nginx vhost is present in the
# router/DO DNS (change nothing; exit 0 when nothing is missing or stale).
dns-sync-check:
    ssh -i ~/.ssh/agent-hop-key -o BatchMode=yes leigh-admin@10.3.1.20 'sudo -n dns-sync check'

# Add missing DNS records (router host override aliases + DO CNAMEs).
# Additive only — never deletes or rewrites existing records.
dns-sync-sync:
    ssh -i ~/.ssh/agent-hop-key -o BatchMode=yes leigh-admin@10.3.1.20 'sudo -n dns-sync sync'

# Remove stale records left behind by vhost renames/removals: names dns-sync
# manages that were expected previously but no longer are. Run after
# dns-sync-sync when a vhost name changed. Never touches records that
# predate dns-sync.
dns-sync-prune:
    ssh -i ~/.ssh/agent-hop-key -o BatchMode=yes leigh-admin@10.3.1.20 'sudo -n dns-sync prune'

# Print the router's pinned host key line. dns-sync and network-status ssh to
# it as root with StrictHostKeyChecking=yes against /etc/dns-sync/known_hosts
# (machines/services1/dns-sync.nix), so after a router reinstall run this and
# paste the result there — the tools refuse to connect rather than trusting a
# new key silently.
router-known-hosts:
    ssh -i ~/.ssh/agent-hop-key -o BatchMode=yes root@10.3.1.1 'sh -c "ssh-keyscan -t ed25519,ecdsa,rsa 10.3.1.1 2>/dev/null"'

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

# Nix-level checks: the flake evaluates, both machines' option assertions hold,
# the hand-rolled config serializers and the generated printer-state tables match
# their expectations, and every tracked .nix file is alejandra-clean.  Fast
# (evaluation only, no compilation).  The rest of the test surface: `just test`
# (cargo tests), `just clippy` (lints), `just filestore-test` (headless-browser
# suite) and `just check-build` (the deploy-readiness build).
check:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "=== nix flake check"
    nix flake check
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
check-build:
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
build-remote machine="services1":
    #!/usr/bin/env bash
    set -euo pipefail
    nixos-rebuild build --flake ".#{{machine}}" --max-jobs 0

# Start an unauthenticated filestore on 127.0.0.1 for poking at the API/UI by
# hand. --no-auth is only honoured on a loopback bind, so this can never expose
# the store on a routable interface; the deployed service never uses it.
filestore-dev:
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop --command bash -c 'cd crates && cargo build --release --offline -p filestore && exec ./target/release/filestore --root "${FS_TEST_ROOT:-/tmp/filestore-test-root}" --no-auth --port "${FS_TEST_PORT:-18097}"'

# Builds the SPA + binary, starts filestore with --no-auth on 127.0.0.1 against
# a scratch fixture, and drives the UI with headless Chromium (Playwright).
# See crates/filestore/tests/README.md.
filestore-test:
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop --command bash -c 'exec crates/filestore/tests/run.sh'

# The switch-gui headless-browser suite: builds the SPA + binary, starts a fake
# ProCurve console (a real Telnet server with an in-memory model), points the
# service at it with --no-auth on a loopback bind and a scratch Postgres schema,
# and drives the matrix with headless Chromium.  Nothing can reach a real switch:
# the only address the service is given is 127.0.0.1.  Needs the local Postgres
# cluster (see SG_TEST_DB_URL in crates/switch-gui/tests/run.sh).
switch-gui-test:
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
