# Leigh Hackspace Infrastructure Nix Flake

Provides the configuration for various servers running Leigh Hackspack
infrastructure.

## Layout

```text
crates/        every Rust crate; one cargo workspace (crates/Cargo.toml)
  common-rs/     the shared in-tree crates (json, oidc, web, pg, service, …)
  <program>/     one directory per service or tool; its Dioxus SPA in frontend/
                 (five are not workspace members: the four SPAs' `frontend/`s,
                 which are wasm, and `whisper-ws/`, which is packaged on its own)
common/        Nix modules shared by every machine (crane helpers, hosts, nginx)
machines/      one directory per machine; services/ one file per service
               (`pi-room-sys/` is the image aibox netboots the Pi Room with, so it
               runs on no box of its own, and `gw/` is the router, deployed at
               10.3.1.254 *beside* the live router — machines/gw/README.md)
secrets/       sops-encrypted
docs/          reference docs (services, builds, DNS, switches, backups) plus the
               write-ups, post-mortems and audits the backstory points to
```

Build anything in `crates/` with `cargo build -p <name>` from there; `just test`
and `just clippy` run the whole workspace. `AGENTS.md` is the rules file — the
detail it points at lives in `docs/` (services, builds, DNS, switches, backups,
follow-ups).

## Applying Config

```bash
just boot               # Build for next reboot (chains sudo nixos-confirm)
just switch             # Build and apply now (chains sudo nixos-confirm)
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
