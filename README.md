# Leigh Hackspace Infrastructure Nix Flake

Provides the configuration for various servers running Leigh Hackspack infrastructure.

## Applying Config

```bash
scripts/boot.sh      # Build for next reboot
scripts/switch.sh    # Build and apply now

sudo nixos-confirm   # Mark this generation as good as we don't get rollbacked

list-generations     # List generations including the last "good", the current and what was booted
```

Unless `sudo nixos-confirm` is run within 5 minutes of a new generation being applied, the previous known good configuration will be restored and the system rebooted. This is to prevent remotely bricking a machine.

## Machines

### Services 1

Path: `machines/services1`

Various hackspace services for members. See [README.md](machines/services1/README.md).

### AI Box

Path: `machines/aibox`

Experimental machine with LLMs and Stable Diffusion
