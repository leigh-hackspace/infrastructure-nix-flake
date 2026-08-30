# AGENTS

Guidance for AI agents working in this repository. Read this before making changes.

## Language preference

- **Use Rust for new programs/tools.** Do not write new services, daemons,
  scripts-as-programs, or CLI tools in Python (or other languages) without a
  strong reason. See `status-dashboard/` (top level) for the
  house style: zero external crates (stdlib only), built with
  `pkgs.rustPlatform.buildRustPackage` + `cargoLock`.
- Shell one-liners inside NixOS `ExecStart`/activation scripts are fine for
  small glue, but anything substantial belongs in Rust.

## Layout

- `flake.nix` — the flake. Inputs include private repos via
  `git+file:///home/leigh-admin/Projects/...` (gocardless-tools, pi-room-sys).
  Machines: `services1` (the main services box, 10.3.1.20) and `aibox`.
- `common/` — shared modules imported by both machines (`tools.nix`,
  `users.nix`, `sops.nix`).
- `dns-sync/` — top-level Rust tool (zero external crates) that keeps the
  router's dnsmasq and DigitalOcean DNS in step with the `*.int.leighhack.org`
  nginx vhosts. Wired in via `machines/services1/dns-sync.nix`, which also
  renders the expected-name list to `/etc/dns-sync/expected-int-names`.
  Run with `just dns-sync-check` / `just dns-sync-sync` / `just dns-sync-prune`
  (router + DO facts below).
- `network-status/` — the router network dashboard (served at
  `network-info.int.leighhack.org`). The Rust binary (`src/`, zero external
  crates) ssh's to the router every 5s, keeps a rolling history, serves the
  JSON API (`/api/snapshot`, `/api/history`, `/api/config`) and static files
  via `--static-dir`. The frontend is a SolidJS + TypeScript SPA in
  `frontend/` (esbuild + `esbuild-plugin-solid`, minimal deps — no router, state, or chart libs). `frontend/dist/` is **committed** and copied into the Nix store
  by `machines/services1/network-status.nix` (Nix builds are offline and the
  npm deps aren't nixpkgs-cached, so the SPA is not rebuilt in Nix): after
  editing the SPA, run `npm install && npm run build` in `frontend/` and commit
  the result.
- `machines/services1/` — the services box:
  - `hardware-configuration.nix` — NFS mounts for the NAS and their explicit
    automount units (see gotcha below).
  - `nfs-client.nix` — `wait-for-nas.service`, the gate that NAS-dependent
    services must wait on.
  - `containers.nix` — podman + the "never give up" restart policy for all
    `podman-*` services.
  - `services/` — one file per service; `status.nix` wires up the status
    dashboard (shared Rust code in top-level `status-dashboard/`, shared
    module in `common/status-dashboard.nix`, run on both machines;
    services1 reverse-proxies both as `status.*` / `aibox.status.*`).
  - `lib/` — nginx SSO helpers (`nginx-sso-helper.nix` etc.).
  - `config.nix` — paths to secrets under `/var/lib/secrets` (do not read
    secret contents; reference the paths).
- `secrets/` — sops-encrypted.

## Infrastructure facts

- NAS is TrueNAS at **10.3.1.6**. NFS shares: `/mnt/cameras`,
  `/mnt/filestore`, `/mnt/backups`. It is slow to come up after a power cut.
- **NAS dependency rule:** any service that needs the NAS must declare
  `after = [ "wait-for-nas.service" ]` and `requires = [ "wait-for-nas.service" ]`
  (see `nfs-client.nix`), and must keep restarting forever — the container
  policy in `containers.nix` (`Restart=always`, `StartLimitIntervalSec=0`)
  applies automatically to all `virtualisation.oci-containers.containers`.
- **Automount gotcha:** `x-systemd.automount` in mount `Options=` is only
  honoured by systemd-fstab-generator. For unit-file mounts (what NixOS
  `systemd.mounts` generates) it is dead config — the `.automount` units must
  be defined explicitly (they are, in `hardware-configuration.nix`). Never
  rely on the mount option alone.
- `wait-for-network.service` (common/tools.nix) pings the NAS; both it and
  `wait-for-nas.service` have `TimeoutStartSec = 0` and must stay that way
  ("never give up" is a hard requirement of this infra).
- NixOS systemd units: `Restart`/`RestartSec` live in `serviceConfig`;
  `StartLimitIntervalSec`/`StartLimitBurst` live on the unit itself
  (`systemd.services.<name>.startLimitIntervalSec`), not in `serviceConfig`.
- nginx: public apps use `mkSSOVirtualHost` (auth via nginx-sso at
  127.0.0.1:8082, injects `X-WEBAUTH-USER`); internal apps use
  `status.int`-style vhosts with `CONFIG.LOCAL_NETWORK` ACLs. The ACME cert
  covers `*.leighhack.org` / `*.int.leighhack.org` (DNS challenge).

## Router (gw) & DigitalOcean DNS

- The router (OPNsense 26.7, FreeBSD) is managed out-of-band — no flake
  config. Access: `ssh root@10.3.1.1` with the machine-hop-key; web UI at
  `https://firewall.int.leighhack.org` (→ 10.3.1.1:60443). Gotcha: the root
  shell is **tcsh** — `2>` redirection fails, and grep patterns containing
  `<`/`>` must be double-quoted.
- The router runs **dnsmasq** (LAN resolver 10.3.1.1): `/conf/config.xml` is
  the OPNsense source of truth (host overrides under `<opnsense><dnsmasq><hosts>`);
  `/var/etc/dnsmasq-hosts` is the generated addn-hosts dump that actually
  answers `*.int.leighhack.org`. Never edit the auto-generated
  `/usr/local/etc/dnsmasq.conf`. Manual fallback if `dns-sync` is unavailable:
  edit the host override in the OPNsense web UI (Services → Dnsmasq DNS →
  Host overrides) or `/conf/config.xml`, then restart dnsmasq; verify with
  `ssh root@10.3.1.1 'grep <name> /var/etc/dnsmasq-hosts'`.
- Reading `dns-sync check` output: `aibox.int` → 10.3.1.32 (own record) and
  `authentik.int` → 10.3.1.36 (own override; nginx also serves it as an alias
  of `id.int`) legitimately resolve elsewhere and are left as-is; `filestore`,
  `gitlab`, `ldap`, `mqtt`, `nginx`, `tailscale` predate dns-sync and are
  reported "never expected" / left as-is.
- DHCP reservations live in the same dnsmasq config: services1 (10.3.1.20,
  MACs `c8:d3:ff:a5:b2:25`/`c8:d3:ff:a5:be:7c`), aibox (10.3.1.32),
  nas1/nas2 (10.3.1.5/10.3.1.6), apps1 (10.3.1.30), yunohost (10.3.1.15).
- DigitalOcean zone `leighhack.org`: the DO token is `DO_AUTH_TOKEN` in
  `/var/lib/secrets/.env` (referenced via `CONFIG.ENV_FILE`; read+write scope)
  — never print it or copy it off services1. `*.int` names are CNAMEs →
  `nginx.int.leighhack.org` (managed by dns-sync); the apex points at GitHub
  Pages, public apps CNAME → `nginx.leighhack.org`, `yunohost` → A
  81.187.195.18.

## SSH between machines (machine-hop-key)

- Every flake-managed machine shares the **machine-hop-key**: the same
  private key at `~/.ssh/agent-hop-key` on each machine, with the matching
  public key authorized for the `leigh-admin` user in `common/users.nix`
  (commit 729b1e1). Use it to hop from any flake machine to any other
  (aibox ↔ services1) without a password.
- Connect as **`leigh-admin`, never `root`** — root SSH is not keyed on
  either machine (verified both directions). `leigh-admin` has passwordless
  sudo, so run `sudo nixos-rebuild ...` / `sudo nixos-confirm` after
  logging in.
- No ssh-agent is running, so pass the key explicitly:

  ```bash
  ssh -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.20   # services1
  ssh -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.32   # aibox
  ```

- Gotcha: on aibox, `/etc/hosts` maps `aibox` to `127.0.0.2` (self
  reference), so use the IPs above rather than hostnames.

## Workflow

- Deploy with the justfile: `just switch` / `just boot`
  (`sudo nixos-rebuild switch --flake . --impure`).
- **Confirm after applying:** `system.autoRollback.enable = true` is set on
  services1 (via nixos-utils), so a failed boot rolls back automatically.
  Always run `sudo nixos-confirm` after `switch`/`boot` to mark the current
  generation as good.
- **New vhosts need DNS records** before they resolve: public `*.leighhack.org`
  names point at the box's public IP, `*.int.leighhack.org` at 10.3.1.20.
  The wildcard ACME cert already covers both, so no cert work is needed.
  For `*.int` names, sync DNS after switching: `just dns-sync-sync`. If a
  vhost was renamed/removed, also run `just dns-sync-prune` (removes the
  stale records the tool manages) and `just dns-sync-check` to verify.
- **New files must be `git add`-ed** before they are visible to the flake
  (flake sources come from the git tree; untracked files are excluded).
- Do not commit unless asked. Staging (`git add`) is fine and often required.
- Formatting: the repo nominally uses alejandra (see `.zed/settings.json`),
  but many pre-existing files are not alejandra-clean. Match the surrounding
  file's style; do not reformat whole pre-existing files.

## Nix commands (local dev)

The flake is `--impure` and sources come from the git tree, so everything
below must run from this repo root. `nixos-rebuild` is the primary tool and
handles the flake plumbing; avoid hand-rolling `import flake.nix` scripts
unless you need to extract a single value (see the gotcha there).

**Test a change without applying** (the standard, preferred way):

```bash
# services1 (the default)
nixos-rebuild dry-run --flake . --impure

# aibox specifically
nixos-rebuild dry-run --flake .#aibox --impure

# Actually build the toplevel locally (no deploy) to get the store path:
nixos-rebuild build --flake .#aibox --impure   # prints the new system path
```

`dry-run` alone may not build all dependencies, so for anything touching
packages/bins, run `build` and inspect the result under `/nix/store/` (it is
printed on success). Use `--target-host` to target a remote host without
deploying, e.g. `nixos-rebuild dry-run --target-host leigh-admin@aibox ...`
(log in as `leigh-admin` — root is not keyed — with its passwordless sudo),
but local `build` is enough to validate config.

**Inspect generated output once you have a system path $SYS**

```bash
cat $SYS/etc/systemd/system/<unit>.service   # exact rendered unit
grep -rn something $SYS/etc/                   # search the built config
```

This is the reliable way to confirm what a service actually runs, what port
it binds, and what paths/flags end up in `ExecStart`.

**Extract a single value/package** (handy for checking help/paths):

```bash
# Build the toplevel, then read a store path out of it:
nix build --no-link --out-link /tmp/aibox-sys .#aibox --impure
ls /tmp/aibox-sys/bin            # symlinks into the system
```

Gotcha: `flake.outputs` is a **function** (its args are
`{ nixpkgs, nixos-utils, llama-cpp, ... }`), and `flake.nix` itself is a plain
set — import it with no args, then call `outputs flake.inputs`. But beware:
evaluating `nixosConfigurations.aibox.config` via `import flake.nix` fails with
`attribute 'lib' missing`, because `specialArgs`/`nixpkgs.lib` need the
flake-resolved inputs that `nixos-rebuild` provides. So reach for the flake
reference form (`nix build .#aibox --impure`) for building, and avoid
`nix eval`/`nix build` against a bare path from the repo root — those resolve
against the repo flake, not nixpkgs.

**Common checks**

```bash
nix flake show --impure      # what the flake exposes (aibox, services1)
git status --short           # untracked files are EXCLUDED from the flake!
nix flake metadata           # locked inputs / rev
```

**Gotchas**

- Untracked files are invisible to the flake — `git add` new/changed `.nix`
  files first (see Workflow).
- `nix eval`/`nix build` on a bare path (e.g. `nix build nixpkgs#foo --impure`)
  from the repo root will resolve against the repo flake, not nixpkgs; use
  `--file <(...)` with an explicit `import` for standalone nixpkgs lookups.
- Editing a service's `ExecStart`/flags: always re-run `nixos-rebuild build`
  and read back the rendered unit — NixOS normalises quoting and expands
  `${...}` and `
` line-continuations differently from what you typed.

## Known follow-ups

- ~~aibox NAS mounts~~ — resolved by commit 1811d93 ("Make aibox startup
  more reliable", deployed 2026-08-26): aibox now mirrors services1
  (explicit automounts + `_netdev` + `wait-for-network` ordering +
  `wait-for-nas`). Evaluating aibox locally still requires the `pi-room-sys`
  git input to exist at `/home/leigh-admin/Projects/pi-room-sys`.
