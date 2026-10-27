# services2.int.leighhack.org

Second services box (10.3.1.21), a clone of services1 on different hardware:
AMD Ryzen 7 5700G (Cezanne Radeon iGPU), NVIDIA RTX 3060 (12 GB, GA106) for
compute, Intel I211 NIC, 250 GB Kingston NVMe, 30 GiB RAM. Installed 2026-10-10,
replacing the Windows install that shipped on the NVMe; the RTX 3060 arrived
2026-10-14 and runs Strata (see below).

## What it runs

Its own status dashboard, the Strata engine, and the exporters — that is all.
The observability stack that used to be here moved to services1 on 2026-10-17:

- `./status.nix` — services2's own dashboard, proxied by the front door (`gw`)
  as `services2-status.int.leighhack.org`.  It is the thing that keeps answering
  while strata is thrashing, and it is the box's only self-measurement.
- `./strata.nix` + `./strata-package.nix` — the Qwen3.8-Flash-Next **Coder**
  engine on the RTX 3060 (`http://10.3.1.21:8080/v1`), the network's only live
  Strata instance since aibox's was parked on 2026-10-17 (aibox is the build
  machine now — `common/build-farm.nix`).  `./nvidia.nix` is the driver for that
  card.  The memory sizing and the one-time model prep are in `./STRATA.md`.
- `common/exporters.nix` — the per-box exporters, scraped by services1's
  Prometheus at fixed IPs (`10.3.1.21:9100`, `:9558`, `:9633`).
- What is **not** here any more, and where it went: `gatus.nix` →
  `machines/services1/services/gatus.nix`, `monitoring.nix` (Prometheus + Grafana
  + the alert rules) → `machines/services1/services/observability.nix`, and its
  dashboards → `machines/services1/services/monitoring-dashboards.nix`.  The
  vhosts stayed where they were (`machines/services1/services/monitoring.nix`),
  so they now proxy to a loopback bind on services1 rather than across the LAN.

The reason for the move is the one that used to be the reason for the other
arrangement: **this box's 30 GiB of RAM is strata's.** The Coder engine measured
28.3 GiB RSS against it on 2026-10-15, and journald was already logging "Under
memory pressure, flushing caches" with gatus, Prometheus and Grafana living
alongside it.  Whatever else lives here is what the kernel reaps when the engine
asks for the next page — and the OOM victim does not have to be the engine, so
the monitor was the thing that died while the thing it watches stayed up (the
record is in `docs/work-log-2026-10.md`).  services1 has the headroom and already
serves these vhosts, so the apps moved to it.

What that gives up is the rule it was built on — "the box that answers *is this
thing down* must not be the box being asked" — for services1 itself: gatus,
Prometheus, Grafana and services1's own exporters now live on the box they
measure, so a dead services1 goes down with its own monitor and reports nothing.
What still works from here, and is the half that keeps being worth having:

- gatus's ping of **10.3.1.21** (the check that replaced the one of services1 —
  see `common/monitoring-endpoints.nix`), plus its checks on aibox, the router,
  the NAS, the switches, the printers and the cameras;
- services1's Prometheus scraping **this box's** exporters, so a strata-driven
  death on services2 is still visible from the box that cannot report it;
- `services2-status.int.leighhack.org`, this box's own dashboard, which is what
  answers "is services2 down" while the engine is thrashing.

It **was** the network's build machine until 2026-10-17 (`common/build-farm.nix`):
211 GB of free store against services1's busier root, and no services worth
protecting from a build.  That role moved to aibox when its strata engine was
parked — this box's 30 GiB of RAM is the Coder engine's, and a delegated build
competing with it was the box's worst failure mode (the memory-pressure and OOM
story above).  Aibox now serves builds; this box is a client with no builder of
its own, and keeps its 211 GB for its own deployments and the engine's packs.
`just build-remote` offloads to aibox, not here.

The vhosts no longer depend on this box at all: they are rows in
`common/front-door.nix`, terminated by `gw` (the network's front door), and they
proxy to this box over the LAN.  A dead services2 shows up as a dead *check*
rather than as an unreachable dashboard.

Things that bind `0.0.0.0` because the scrape/proxy comes from another box: the
exporters and services2's status dashboard.  Grafana, gatus and Prometheus used
to be in that list; they are on services1 now and bind loopback.

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
- **The other relay, used on 2026-10-27: agent-forward the push back into this box.** One
  command, run *on services1*, whose agent holds services1's GitHub key and whose `ssh -A`
  carries it into a `git push` that runs **here** — cheaper than the `FETCH_HEAD` dance when
  both trees already agree (they do, when this box was fetched from GitHub rather than rsynced):

  ```bash
  # on services1 only — the push itself runs on services2
  ssh-agent -t 300 sh -c "ssh-add ~/.ssh/id_ed25519; \
    ssh -A -i ~/.ssh/agent-hop-key -o BatchMode=yes leigh-admin@10.3.1.21 \
    'cd ~/Projects/infrastructure-nix-flake && git push origin main'"
  ```

  The trap names its direction: services2 has an `id_ed25519` of its own, but it is not the
  credential GitHub accepts (services1's is, as `cjdell`), so an agent started on **this** box
  adds the wrong key and the push dies with the same `Permission denied (publickey)` a missing
  key gives — start the agent on services1, and pass `-A` on the hop back.
- **`git commit -S` fails on this box: there is no `gpg`** (`error: cannot run gpg: No such
  file or directory`, then `fatal: failed to write commit object` — gnupg is not in this
  system's package set, and `git log` here shows no `gpgsig` on any commit). Commit unsigned
  here — that is what the history on this box already is; a signature is made at commit time,
  so there is nothing to sign afterwards.
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
change the 2026-10-15 move bought.  It no longer makes this box the network's
builder (that is aibox since 2026-10-17), but it does mean a check or a build run
here is meaningful rather than a partial answer.

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
