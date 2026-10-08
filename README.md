# Leigh Hackspace Infrastructure Nix Flake

Provides the configuration for various servers running Leigh Hackspack
infrastructure.

## Applying Config

```bash
just boot               # Build for next reboot (chains sudo nixos-confirm)
just switch             # Build and apply now (chains sudo nixos-confirm)
just update-netboot-input  # Move the pi-room-sys (netboot image) input pin
just switch-netboot     # Rebuild/switch the netboot setup and restart clients
just hard-reboot        # sysrq hard reboot — no orderly shutdown

sudo nixos-confirm      # Mark this generation as good as we don't get rollbacked

list-generations        # List generations including the last "good", the current and what was booted
```

`just switch`/`just boot` already run `sudo nixos-confirm` in the same shell
invocation, which is what the auto-rollback timer needs (see below); running it
again by hand is harmless.

Unless `sudo nixos-confirm` is run within 5 minutes of a new generation being
applied, the previous known good configuration will be restored and the system
rebooted. This is to prevent remotely bricking a machine.

## Machines

### Services 1

Path: `machines/services1`

Various hackspace services for members. See
[README.md](machines/services1/README.md).

### AI Box

Path: `machines/aibox`

Experimental machine with LLMs and Stable Diffusion. Always hosts the netboot
server for the Pi Room computers.
