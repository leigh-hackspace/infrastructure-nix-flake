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
- **New files must be `git add`-ed** before they are visible to the flake
  (flake sources come from the git tree; untracked files are excluded).
- Do not commit unless asked. Staging (`git add`) is fine and often required.
- Formatting: the repo nominally uses alejandra (see `.zed/settings.json`),
  but many pre-existing files are not alejandra-clean. Match the surrounding
  file's style; do not reformat whole pre-existing files.

## Known follow-ups

- **aibox NAS mounts:** `machines/aibox/hardware-configuration.nix` mounts
  `/mnt/filestore` and `/mnt/ds-photos` without `_netdev`, automount or
  `wait-for-network` ordering — a slow NAS can hang its boot. Apply the same
  pattern as services1 (explicit automounts + `wait-for-nas`) when next
  touching aibox. Note: evaluating aibox locally requires the `pi-room-sys`
  git input to exist at `/home/leigh-admin/Projects/pi-room-sys`.
