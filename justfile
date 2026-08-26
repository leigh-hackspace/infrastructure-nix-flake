# https://just.systems

default:
    @ {{just_executable()}} --list --justfile {{justfile()}} --unsorted

reboot:
    sync
    sudo bash -c "echo b > /proc/sysrq-trigger"

boot:
    sudo nixos-rebuild boot --flake . --impure

switch:
    sudo nixos-rebuild switch --flake . --impure

# --- DNS sync (keeps router dnsmasq + DigitalOcean DNS in step with the
# --- *.int.leighhack.org nginx vhosts; requires the machine-hop key and a
# --- deployed dns-sync on services1 — see ROUTER.md / DO.md) ---

# Report whether every *.int.leighhack.org nginx vhost is present in the
# router/DO DNS (change nothing; exit 0 when nothing is missing).
dns-sync-check:
    ssh -i ~/.ssh/agent-hop-key -o BatchMode=yes leigh-admin@10.3.1.20 'sudo -n dns-sync check'

# Add missing DNS records (router host override aliases + DO CNAMEs).
# Strictly additive — never deletes or rewrites existing records.
dns-sync-sync:
    ssh -i ~/.ssh/agent-hop-key -o BatchMode=yes leigh-admin@10.3.1.20 'sudo -n dns-sync sync'

switch-netboot:
    #!/usr/bin/env bash
    set -euo pipefail

    nix flake update pi-room-sys

    sudo bash -c 'umount -f -l /exports/netboot-squashfs | true'
    sudo nixos-rebuild switch --flake . --impure
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

install-sops-key:
    #!/usr/bin/env bash
    set -euo pipefail

    mkdir -p /var/lib/sops-nix
    chmod 500 /var/lib/sops-nix
    nix-shell -p ssh-to-age --run "ssh-to-age -private-key -i /home/leigh-admin/.ssh/id_ed25519_leigh_admin > /var/lib/sops-nix/key.txt"
    chmod 400 /var/lib/sops-nix/key.txt

edit-secrets:
    sudo sops edit secrets/secrets.yaml
