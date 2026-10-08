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

# Both recipes chain `sudo nixos-confirm` in the same shell invocation: both
# machines set system.autoRollback.enable = true, so the auto-rollback timer
# rolls back to the last confirmed generation within a minute or two of a
# switch.  Confirming in a later session can land _after_ the rollback and mark
# the old generation good, which defeats the point (see AGENTS.md, GOLDEN RULE).
boot:
    sudo nixos-rebuild boot --flake . && sudo nixos-confirm

switch:
    sudo nixos-rebuild switch --flake . && sudo nixos-confirm

# --- Rust crates (toolchain from `nix develop`: cargo/rustc/rustfmt/clippy +
# --- the pinned wasm-bindgen-cli) ---

# Recreate the git-ignored symlinks that let cargo resolve the in-repo path
# dependencies (each crate's common-rs/, gocardless-dashboard/frontend/dto).
# Run once after a fresh clone; `nix develop` does it automatically.
shared-rs:
    @bash common/shared-rs-links.sh

# cargo test over the shared crates and every app crate.
test:
    #!/usr/bin/env bash
    set -euo pipefail
    bash common/shared-rs-links.sh
    nix develop --command bash -c '
      crates="common-rs/json common-rs/oidc common-rs/build-spa common-rs/web dns-sync moonraker-exporter status-dashboard network-status filestore gocardless-dashboard frigate-monitor"
      for c in $crates; do echo "=== test $c"; (cd $c && cargo test --offline); done'

# cargo clippy over the same set.  Warnings are reported, not fatal: the point
# is to see them (they used to go entirely unchecked).
clippy:
    #!/usr/bin/env bash
    set -euo pipefail
    bash common/shared-rs-links.sh
    nix develop --command bash -c '
      crates="common-rs/json common-rs/oidc common-rs/build-spa common-rs/web dns-sync moonraker-exporter status-dashboard network-status filestore gocardless-dashboard frigate-monitor"
      for c in $crates; do echo "=== clippy $c"; (cd $c && cargo clippy --offline --all-targets 2>&1 | grep -vE "^(Compiling|Checking|Finished|    Finished)" | head -40); done'

# --- frontend (frigate-monitor web UI) ---

# Rebuild the Dioxus SPA into frigate-monitor/frontend/dist using the
# devshell's toolchain (cargo/rustc/lld + pinned wasm-bindgen-cli 0.2.128).
# Same toolchain the flake uses for its frontendDist; dist/ is git-ignored
# and rebuilt by the flake on deploy, so this is just for local iteration.
build-frontend:
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop --command bash -c 'cd frigate-monitor/frontend && exec ./build.sh'


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

# Pull the netboot input (pi-room-sys) to its latest revision.  Split out of
# switch-netboot: a deploy recipe silently rewriting flake.lock is a side effect
# nobody expects, and the pin should be an explicit act.
update-netboot-input:
    nix flake update pi-room-sys

# Rebuild+switch for the netboot setup, then reboot the netbooted clients.
# Assumes the pi-room-sys pin is already right — run update-netboot-input first
# if you mean to move it.
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

update-gocardless-input:
    #!/usr/bin/env bash
    set -euo pipefail

    pushd /home/leigh-admin/Projects/gocardless-tools
    git pull
    popd

    nix flake update gocardless-tools

update-pkgs:
    nix flake update nixpkgs

# --- filestore headless-browser test suite ---

# Nix-level checks: the flake evaluates, both machines' option assertions hold,
# and the hand-rolled config serializers match their expectations.  Fast (no
# compilation).  The rest lives in `just test` (cargo tests), `just clippy`
# (lints) and `just filestore-test` (headless-browser suite).
check:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "=== nix flake check"
    nix flake check
    echo "=== config-to-gitlab assertions"
    nix eval --impure --raw -f machines/services1/lib/check-config-to-gitlab.nix

# Start an unauthenticated filestore on 127.0.0.1 for poking at the API/UI by
# hand. --no-auth is only honoured on a loopback bind, so this can never expose
# the store on a routable interface; the deployed service never uses it.
filestore-dev:
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop --command bash -c 'cd filestore && cargo build --release --offline && exec ./target/release/filestore --root "${FS_TEST_ROOT:-/tmp/filestore-test-root}" --no-auth --port "${FS_TEST_PORT:-18097}"'

# Builds the SPA + binary, starts filestore with --no-auth on 127.0.0.1 against
# a scratch fixture, and drives the UI with headless Chromium (Playwright).
# See filestore/tests/README.md.
filestore-test:
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop --command bash -c 'exec filestore/tests/run.sh'

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
