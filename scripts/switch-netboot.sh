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
COMMAND="echo b > /proc/sysrq-trigger"
TIMEOUT=10  # seconds
LOG_FILE="sysrq_log.txt"

# Clear log file
> "$LOG_FILE"

# Loop over IP range
for i in $(seq $START $END); do
    HOST="${BASE_IP}${i}"
    echo -n "Sending reboot to $HOST... "

    # Run SSH with timeout
    # We expect SSH to exit with code 255 (connection closed) or 124 (timeout) or 0 if it somehow worked
    # But we treat *any* connection closure *after* command sent as success
    if timeout "$TIMEOUT" ssh -o ConnectTimeout=5 -o BatchMode=yes -o StrictHostKeyChecking=no -o ServerAliveInterval=1 -o ServerAliveCountMax=2 root@"$HOST" "$COMMAND" 2>/dev/null; then
        # SSH succeeded — unlikely, unless reboot didn't happen
        echo "WARNING: System did not reboot (SSH returned success)" | tee -a "$LOG_FILE"
    else
        # SSH failed — this is expected if reboot happened
        EXIT_CODE=$?
        if [ $EXIT_CODE -eq 255 ] || [ $EXIT_CODE -eq 124 ]; then
            echo "SUCCESS (reboot initiated — SSH connection terminated as expected) [$EXIT_CODE]" | tee -a "$LOG_FILE"
        else
            echo "FAILED (SSH error: exit code $EXIT_CODE)" | tee -a "$LOG_FILE"
        fi
    fi
done

echo "All operations completed. Log saved to $LOG_FILE"
