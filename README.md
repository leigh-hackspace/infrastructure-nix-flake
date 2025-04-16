# Leigh Hackspace Infrastructure Nix Flake

Provides the configuration for various servers running Leigh Hackspack infrastructure.

## Applying Config

```bash
sudo nixos-rebuild boot   --flake . --impure --max-jobs 1    # Build for next reboot
sudo nixos-rebuild switch --flake . --impure --max-jobs 1    # Build and apply now
```
