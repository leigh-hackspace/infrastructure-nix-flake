# services2.int.leighhack.org

Second services box (10.3.1.21), a clone of services1 on different hardware:
AMD Ryzen 7 5700G (Cezanne Radeon iGPU), NVIDIA RTX 3060 (12 GB, GA106) for
compute, Intel I211 NIC, 250 GB Kingston NVMe, 30 GiB RAM. Installed 2026-10-10,
replacing the Windows install that shipped on the NVMe; the RTX 3060 arrived
2026-10-14 and runs Strata (see below).

## What it runs

The observability stack, its own status dashboard, and a Strata instance:

- `./gatus.nix` — the endpoint checks and the Slack alerting.
- `./monitoring.nix` — Prometheus (9091) + Grafana (3000) and the alert rules.
- `./status.nix` — services2's own dashboard, proxied by services1 as
  `services2-status.int.leighhack.org`.
- `./strata.nix` + `./strata-package.nix` — the Qwen3.8-Flash-Next **Coder**
  engine on the RTX 3060 (`http://10.3.1.21:8080/v1`), the box's second Strata
  instance after aibox's. `./nvidia.nix` is the driver for that card. The
  memory sizing and the one-time model prep are in `./STRATA.md`.
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
- **It does have `~/.ssh/agent-hop-key`** (verified 2026-10-15: same md5 as services1's,
  and `ssh -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.20` hops out). The earlier claim that
  only `authorized_keys` was installed is stale — `common/users.nix` installed the key too,
  or it was rsynced in with the repo. What is still true is that it has **no GitHub key of
  its own**: reach GitHub from here with `ssh -A` from services1 (agent forwarding — the key
  must be *in* the agent, `ssh-add ~/.ssh/id_ed25519`), and check `ssh-add -l` before
  suspecting the forwarding. Its own `nix.conf` also lives on a read-only symlink into the store, so
  a stateful edit needs replacing that symlink with a real file (see
  `docs/nix-build-sigkill-2026-10-11.md`).
- **Pushing from here needs a relay through services1** (which authenticates to GitHub as
  `cjdell`). Commit here, then, from services1, fetch this tree into `FETCH_HEAD` and push
  that — so services1's own checkout, which lags `origin/main` and carries its own dirty
  files, is never touched:

  ```bash
  # on services1, from its own checkout
  GIT_SSH_COMMAND='ssh -i ~/.ssh/agent-hop-key -o IdentitiesOnly=yes' \
    git fetch ssh://leigh-admin@10.3.1.21/home/leigh-admin/Projects/infrastructure-nix-flake main
  git push origin "$(git rev-parse FETCH_HEAD):refs/heads/main"
  ```

  Two traps, both hit on 2026-10-15. **`GIT_SSH_COMMAND` applies to every ssh transport**, so
  exporting it forces the LAN hop key on the GitHub remote too and the push dies with
  `Permission denied (publickey)` — the same error a missing GitHub key gives, on the box that
  has one; set it inline on the `git fetch` only. And services1 offers several identities, so
  the LAN fetch dies with `Too many authentication failures` unless the hop key is passed with
  `IdentitiesOnly=yes`. Guard the push with
  `git merge-base --is-ancestor "$(git ls-remote origin refs/heads/main | cut -f1)" FETCH_HEAD`
  so it can only ever fast-forward. Until this box fetches, `git status -sb` here reports
  `ahead 1` forever — that is a stale `origin/main` ref, not an unpushed commit.
- **No swap *partition*** — the NVMe is only p1 (512 MiB ESP) + p2 (232 GiB ext4 root), so
  there is no spare partition like services1's `nvme0n1p3` (18 GiB) or aibox's. It does have
  **a 16 GiB swap *file*, `/var/swap/services2`**, declared in `./hardware-configuration.nix`
  (added 2026-10-15: strata alone measured 28.3 GiB RSS against 30 GiB of RAM, which is what
  made the box go unresponsive — journald was logging "Under memory pressure, flushing caches"
  — and a build here had nowhere to go). `size` in that entry is what makes nixpkgs create and
  `mkswap` the file at boot; see the comment there for why a file and not a partition.
- `just check` runs here: the flake has no inputs outside its own tree, so
  evaluating aibox needs nothing that is not in the checkout. (Until 2026-10-15 it
  could not — `whisper-ws` and `pi-room-sys` were `git+file:///home/leigh-admin/Projects/…`
  inputs and aibox would not evaluate without those checkouts. They are in-tree now,
  as `crates/whisper-ws/` and `machines/pi-room-sys/`.)

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

This flake evaluates here with no local checkouts of its own: every input is a
network-wide source, and the two programs it builds live in the tree
(`crates/whisper-ws/`, `machines/pi-room-sys/`). `nix flake check` / `just check`
evaluate all five configurations, so they work from this box too — that is the
change the 2026-10-15 move bought, and it is what makes services2 usable as the
network's build machine for *checks* and not only for builds.

There is no `system-good` generation yet, so `auto-rollback` is inert until the
first `nixos-confirm` (which `just switch` runs for you).

(Done 2026-10-11: the first confirm ran on the deploy that moved monitoring here.)

## Install notes (2026-10-10)

Installed from the NixOS 26.05 live USB (the Ventoy stick it booted first) by
building `.#services2` on services1, `nix copy`ing the closure to the installer,
and running `nixos-install --system <path>` there. The NVMe is GPT with a 512 MiB
ESP (`NIXBOOT`, UUID `FBB6-8ECF`) plus an ext4 root (`NIXROOT`, UUID
`b4f380f9-e3a0-4e1b-aacc-538f78f4e36a`), no swap partition, systemd-boot; the Windows
install that occupied the disk was wiped first. The systemd-boot entry is first
in the EFI boot order (the stale Windows Boot Manager entry was removed). The
sops age key was placed at `/var/lib/sops-nix/key.txt` before first boot, and
this repo was rsynced to `~/Projects/infrastructure-nix-flake`.
