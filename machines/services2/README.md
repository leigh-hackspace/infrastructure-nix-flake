# services2.int.leighhack.org

Second services box (10.3.1.21), a clone of services1 on different hardware:
AMD Ryzen 7 5700G (Cezanne Radeon iGPU), Intel I211 NIC, 250 GB Kingston NVMe,
16 GB RAM. Installed 2026-10-10, replacing the Windows install that shipped on
the NVMe.

## State: clone with no services (yet)

This machine intentionally runs **nothing**: the module list in `flake.nix` is
services1's minus containers/crane, and `./networking.nix` has the LAN/VLAN
wiring but none of services1's NAT, WireGuard or `extraHosts`. When the box is
given services, mirror the change on services1 as well (shared code belongs in
`common/`, per AGENTS.md) and add the missing network pieces then.

What it already has, so it can take over at any point:

- `common/base.nix`, `common/tools.nix`, `common/users.nix`, `common/sops.nix`,
  the NAS mounts (`common/nas.nix`, the same three exports as services1) and the
  auto-rollback timer.
- The shared `machine-hop-key`, so `ssh -i ~/.ssh/agent-hop-key
  leigh-admin@10.3.1.21` works with no password.
- The sops age key at `/var/lib/sops-nix/key.txt`, so `common/sops.nix`'s
  secrets decrypt at boot like on the other machines.

## Network

- Headless; no WiFi (`networkmanager` is off). The Intel I211 (`enp8s0`, MAC
  `70:85:c2:53:f1:d4`) is renamed `lan` and is the trunk uplink; addressing is on
  the tagged `vlan225` child.
- The IPv4 address is a **DHCP reservation** on the router, not a static config:
  OPNsense dnsmasq host override `services2` → `10.3.1.21` for that MAC (VLAN 225
  set tag). It also publishes `services2` / `services2.int.leighhack.org` in the
  router's dnsmasq. Added 2026-10-10 (backup:
  `/conf/config.xml.services2-*` on the router).
- **The switch port must carry VLAN 225 tagged** (a trunk), exactly like
  services1's port; an access port leaves `vlan225` with no address.

## Rebuilding from this box

```bash
cd ~/Projects/infrastructure-nix-flake
sudo nixos-rebuild switch --flake .#services2 && sudo nixos-confirm
```

This flake evaluates here without the two local `git+file://` inputs
(`whisper-ws`, `pi-room-sys`): they are aibox-only, and Nix only fetches the
inputs an output actually uses (verified at install time — `.#services2`
evaluated to the same derivation on this box and on services1). `nix flake
check` / `just check` do evaluate aibox, so those need the checkouts under
`~/Projects/` before they will pass here.

There is no `system-good` generation yet, so `auto-rollback` is inert until the
first `nixos-confirm` (which `just switch` runs for you).

## Install notes (2026-10-10)

Installed from the NixOS 26.05 live USB (the Ventoy stick it booted first) by
building `.#services2` on services1, `nix copy`ing the closure to the installer,
and running `nixos-install --system <path>` there. The NVMe is GPT with a 512 MiB
ESP (`NIXBOOT`, UUID `FBB6-8ECF`) plus an ext4 root (`NIXROOT`, UUID
`b4f380f9-e3a0-4e1b-aacc-538f78f4e36a`), no swap, systemd-boot; the Windows
install that occupied the disk was wiped first. The systemd-boot entry is first
in the EFI boot order (the stale Windows Boot Manager entry was removed). The
sops age key was placed at `/var/lib/sops-nix/key.txt` before first boot, and
this repo was rsynced to `~/Projects/infrastructure-nix-flake`.
