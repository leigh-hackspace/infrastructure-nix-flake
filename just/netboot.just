# JUST — the Pi Room netboot image and the two PXE clients that test it without a board.
#
# Part of the justfile: the root justfile imports these, so `just` still
# resolves every recipe from the repo root and `just --list` still shows
# them.  Split by job (AGENTS.md rule 26), so no file here is over 1000
# lines, and the guard that enforces that rule now sees justfiles.

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
    # Under /tmp, not in the repo: this is a run log, the repo root is not a log directory,
    # and a file left here by a run is a dirty `git status` entry that the next `git add .`
    # stages.  Nothing else in the tree reads it by name, so the path is free to move.
    LOG_FILE="/tmp/netboot-reboot.log"

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
