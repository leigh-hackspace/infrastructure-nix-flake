# services2.int.leighhack.org

Second services box (10.3.1.21), a clone of services1 on different hardware:
AMD Ryzen 7 5700G (Cezanne Radeon iGPU), Intel I211 NIC, 250 GB Kingston NVMe,
16 GB RAM. Installed 2026-10-10, replacing the Windows install that shipped on
the NVMe.

## What it runs

The observability stack, and its own status dashboard:

- `./gatus.nix` — the endpoint checks and the Slack alerting.
- `./monitoring.nix` — Prometheus (9091) + Grafana (3000) and the alert rules.
- `./status.nix` — services2's own dashboard, proxied by services1 as
  `services2-status.int.leighhack.org`.
- `common/exporters.nix` — the per-box exporters, so this box is measured too.

Monitoring lives here rather than on services1 for one reason: **the box that
answers "is this thing down" must not be the box being asked.** Gatus on services1
could never report services1 as down, and a services1 failure took the alerting with
it — including the checks for everything else on the LAN. The endpoint list is data in
`common/monitoring-endpoints.nix`; the drift guard that goes with it stays on services1,
where the vhosts are.

It is also the network's build machine (`common/build-farm.nix`): 211 GB of free store
against services1's 84%-full root, and no services worth protecting from a build.

The vhosts stay on services1, which terminates TLS for the whole network, so they are in
`machines/services1/services/monitoring.nix` and `machines/services1/services/status.nix`
and proxy here. That is the one dependency this box cannot remove: if services1 is down
the dashboards are unreachable even though gatus is still alerting to Slack.

Things that bind `0.0.0.0` because the proxy/scrape comes from another box: Grafana
(`http_addr`), gatus, Prometheus, the exporters, and services2's status dashboard.

## What it does not have

- No services1-style NAT, WireGuard or `extraHosts` in `./networking.nix`. Fine for a
  box that hosts no apps; mirror services1's pieces here if it ever does.
- **No `~/.ssh/agent-hop-key`.** The README previously claimed the shared machine-hop key
  was in place; it is not — only `authorized_keys` is installed (by `common/users.nix`),
  which lets you hop *in* but not *out*. So services2 cannot ssh to services1, and it has
  no GitHub key of its own: reach GitHub from it with `ssh -A` from services1 (agent
  forwarding — the key must be *in* the agent, `ssh-add ~/.ssh/id_ed25519`), or install the
  key deliberately. Its own `nix.conf` also lives on a read-only symlink into the store, so
  a stateful edit needs replacing that symlink with a real file (see
  `docs/nix-build-sigkill-2026-10-11.md`).
- No swap partition (services1 has 18 GB). Not a problem at the current load; worth
  matching if this box starts running anything heavier.
- `just check` cannot run here: it evaluates aibox, which needs the `whisper-ws` and
  `pi-room-sys` checkouts under `~/Projects/`. Run checks from services1.

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

(Done 2026-10-11: the first confirm ran on the deploy that moved monitoring here.)

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
