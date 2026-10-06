# AGENTS

Guidance for AI agents working in this repository. Read this before making changes.

## Language preference

- **Use Rust for new programs/tools.** Do not write new services, daemons,
  scripts-as-programs, or CLI tools in Python (or other languages) without a
  strong reason. See `status-dashboard/` (top level) for the
  house style: built with
  `pkgs.rustPlatform.buildRustPackage` + `cargoLock`.
- Shell one-liners inside NixOS `ExecStart`/activation scripts are fine for
  small glue, but anything substantial belongs in Rust.

## Layout

- `flake.nix` — the flake. Inputs include private repos via
  `git+file:///home/leigh-admin/Projects/...` (gocardless-tools, pi-room-sys).
  Machines: `services1` (the main services box, 10.3.1.20) and `aibox`.
- `common/` — shared modules imported by both machines (`tools.nix`,
  `users.nix`, `sops.nix`, plus `crane.nix` — the shared crane build helpers,
  see the Rust builds section).
- `dns-sync/` — top-level Rust tool (zero external crates) that keeps the
  router's dnsmasq and DigitalOcean DNS in step with the `*.int.leighhack.org`
  nginx vhosts. Wired in via `machines/services1/dns-sync.nix`, which also
  renders the expected-name list to `/etc/dns-sync/expected-int-names`.
  Run with `just dns-sync-check` / `just dns-sync-sync` / `just dns-sync-prune`
  (router + DO facts below).
- `frigate-monitor/` — top-level Rust tool (one external crate: `image`) on
  aibox that snapshots the Frigate `main_space` RTSP stream every 10 s and
  records an event when a static change appears and the area _settles_ — a
  region must differ from the background and be completely still for N
  consecutive snapshots (moving people are dismissed). The `before` image is
  taken from just before the change began; recorded objects are absorbed
  into the background so their later removal is caught too. Events store
  before/after/diff JPEGs with the changed regions boxed, zoom crops, and a
  thumbnail under `/var/lib/frigate-monitor/events/`. Web UI on
  aibox:8090 is a Dioxus SPA (frontend/, compiled to wasm by the flake as
  `frontendDist` — no bundle is committed), proxied by services1 as
  `frigate-monitor.int.leighhack.org` (LAN-only; see
  `machines/aibox/frigate-monitor.nix` and
  `machines/services1/services/frigate-monitor.nix`).
- `gocardless-dashboard/` — GoCardless Pro sync + operator dashboard on
  services1: a Rust binary syncs customers/mandates/subscriptions/payments/
  refunds/payouts from the GoCardless Pro API into local Postgres
  (`gocardless_dashboard`) every 15 min, and serves a Dioxus SPA (frontend/,
  wasm-built by the flake and embedded by build.rs — no bundle committed)
  on 127.0.0.1:8095. Login is OIDC against authentik (provider pk 33,
  `Infra` group gate; see the Authentik section). LAN-only vhost
  `gocardless.int.leighhack.org`; packaging in
  `machines/services1/services/gocardless-dashboard.nix`. The GoCardless Pro
  API is at `https://api.gocardless.com` with **no** `/v1` path prefix and
  requires the `GoCardless-Version: 2015-07-06` header; list responses wrap
  items under the resource key (e.g. `customers`) and paginate with
  `?after=<meta.cursors.after>`. It also owns the authentik `Members` group
  sync (replaces the old Python `gocardless-tools` job, 2026-10): the same
  binary run as a daily oneshot (`gocardless-authentik-sync.timer`, 01:00,
  `--authentik-sync`) adds customers with an active subscription whose name
  contains "membership" to the `Members` group, removes no-longer-valid ones
  (users unknown to GoCardless are never touched), keeps the
  `leighhack.org/gocardless-customer-id` attribute in step, and writes an
  audit log to `authentik_sync_log` (GUI: “Members sync” page; on-demand via
  `POST /api/authentik-sync`). The old script's bugs (only read the first 20
  users; user creation always failed on username collision) are fixed —
  username collisions are now logged as `skipped_existing_username`.
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
- `filestore/` — top-level Rust tool (axum + reqwest) on services1: a web file
  browser over the NAS `filestore` share. Serves the JSON API (`/api/list`,
  `/api/search`, `/api/mkdir|create|rename|copy|delete|upload|download|zip|preview`,
  `/api/whoami`) on 127.0.0.1:8096 plus a Dioxus SPA (frontend/, wasm-built by
  the flake as `frontendDist` and embedded by `build.rs` — no bundle is
  committed) at `filestore.int.leighhack.org` (LAN-only vhost, see
  `machines/services1/services/filestore.nix`; it replaces the old apps1:8001
  deployment). Login is OIDC against authentik gated on the `Infra` group
  (same client contract as gocardless-dashboard); the client id/secret live in
  the shared env-file sops secret as `FILESTORE_OIDC_CLIENT_ID` /
  `FILESTORE_OIDC_CLIENT_SECRET` (authentik provider pk 34, app slug `filestore`,
  `Infra` group gate). `--dev-user <name>` is a testing-only escape hatch and is
  never passed in production. `--no-auth` is a second testing-only escape hatch
  (treats every request as a local session) and is **only honoured on a loopback
  bind** — `main.rs` exits if it is combined with a non-loopback `--bind`.
  `frontend/dist/` and `target/` are git-ignored.
  The SPA was originally written against the dioxus 0.6 API and had to be ported
  to the pinned 0.7.10 — see the filestore gotchas below.
  There is a permanent headless-browser test suite in `filestore/tests/` (Node +
  Playwright, run with `just filestore-test`); see `filestore/tests/README.md`
  for the test hooks the SPA carries (`data-fs-name`, `#fs-menu`, `#fs-modal`, …)
  and the bugs it has already caught.
  Selection is keyboard-driven as well: the arrow keys move it and shift+arrow
  extends the range from the anchor (`AppState::focus`, shared with shift-click),
  with the column count read from the rendered grid so up/down matches what the
  user sees. Enter opens the focused row (folder → navigate, previewable file →
  preview, otherwise download) and is left to the focused control when a text
  field or button has focus, so the search box and the modal input still work.
  The context menu is clamped to the viewport (it can only be measured
  after the first paint, so `menu.rs` caches the size between opens) and closes on
  any click outside it.
  Rows publish `text/uri-list` on `dragstart` (the download URL, or the ZIP URL
  for a folder) as well as `application/x-filestore`, because the internal format
  is only understood by this page and an external drop target would refuse the
  drag.
  Uploads are raw request bodies, so axum's `DefaultBodyLimit` does not apply to
  them: the server enforces `--max-upload` (2G in production) and the vhost's
  `client_max_body_size` must match it, otherwise nginx 413s the request and the
  upload popup prints its HTML error page.
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

### 3D print servers (hackspace LAN, 10.3.14.0/24)

`3d-blue` (10.3.14.62) and `3d-lime` (10.3.14.61) are Raspberry Pi 3 print
servers running **Klipper + Moonraker** (Mainsail on nginx `:80`, Moonraker API
on `:7125`). The `moonraker-exporter` tool in
`machines/services1/services/printer-monitoring.nix` polls their APIs (these
are the "blue"/"lime" targets). Targets are fixed IPs on purpose so the
exporter never depends on DNS — historically needed because the two Pis were
cloned from one SD card, which gave them the same `/etc/machine-id` and hence
the same NetworkManager DHCPv6 DUID, so the `3d-*` names advertised each
other's IPv6 addresses. Lime's machine-id and SSH host keys were regenerated
on 2026-10-03 (blue still carries the old identity), but fixed IPv4 is
deliberately kept.

**SSH access (permanent):** the Pi's user is `hackspace` (not `pi`/`root`) and
the host is keyed with the **machine-hop-key**:

```bash
ssh -i ~/.ssh/agent-hop-key hackspace@10.3.14.62   # 3d-blue
ssh -i ~/.ssh/agent-hop-key hackspace@10.3.14.61   # 3d-lime
```

Klipper config/logs live under `/home/hackspace/printer_data/`
(`config/printer.cfg`, `logs/klippy.log`, `logs/moonraker.log`). The MCU is an
Arduino Mega 2560 clone on a **CH340** USB-serial adapter
(`/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0`). Logs can also be read
unauthenticated via Moonraker on the trusted LAN
(`http://10.3.14.62:7125/server/files/logs/klippy.log`).

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
  of `id.int`) legitimately resolve elsewhere and are left as-is; `gitlab`,
  `ldap`, `mqtt`, `nginx`, `tailscale` predate dns-sync and are reported "never
  expected" / left as-is. `filestore.int` is expected by the flake vhost and
  resolves to 10.3.1.20 in both the router and DigitalOcean.
- DHCP reservations live in the same dnsmasq config: services1 (10.3.1.20,
  MACs `c8:d3:ff:a5:b2:25`/`c8:d3:ff:a5:be:7c`), aibox (10.3.1.32),
  nas1/nas2 (10.3.1.5/10.3.1.6), apps1 (10.3.1.30), yunohost (10.3.1.15).
- DigitalOcean zone `leighhack.org`: the DO token is `DO_AUTH_TOKEN` in
  `/var/lib/secrets/.env` (referenced via `CONFIG.ENV_FILE`; read+write scope)
  — never print it or copy it off services1. `*.int` names are CNAMEs →
  `nginx.int.leighhack.org` (managed by dns-sync); the apex points at GitHub
  Pages, public apps CNAME → `nginx.leighhack.org`, `yunohost` → A
  81.187.195.18.

## Authentik (id.leighhack.org, 10.3.1.36)

- Runs out-of-band on its own box (docker compose in `/srv/authentik`), not
  flake-managed. Web UI: https://id.leighhack.org (also served as
  `authentik.int.leighhack.org`); nginx on the box proxies to 127.0.0.1:9000.
- Admin access: the default `akadmin` user is **disabled**. Real admins are
  members of the superuser groups `Infra` (or `pgina`), e.g. `cjdell`. To
  use the admin API, mint a token as an active admin via the ORM:

  ```bash
  docker exec -i authentik-server-1 ak shell <<'PY'
  from authentik.core.models import Token, User
  t = Token(identifier="setup", user=User.objects.get(username="cjdell"),
            intent="api", expiring=False)
  t.save(); print(t.key)   # shown once — delete the token when done
  PY
  ```

  The admin REST API is **not** usable from the box (the box's nginx
  proxies `/api/v3/...` collection endpoints to the UI SPA — they 404 with
  HTML), so manage everything through the ORM shell above. `ak shell` prints
  a banner and swallows tracebacks unless you keep stderr and filter the log
  noise: `docker exec -i authentik-server-1 ak shell 2>&1 <<'PY' | grep -viE
  "imported related module" | grep -vE '"level": "(debug|info)"'`.

- OAuth2 providers/apps are managed via the ORM/UI (no declarative config).
  Client secrets are **write-only**: generate your own, pass it on
  create/save, and store it in sops — they can never be read back.
- **Creating an OIDC provider that actually works** (all three of these
  bites are invisible until login is tried):
  - Copy `authentication_flow` / `authorization_flow` /
    `invalidation_flow` / `signing_key` from a known-good provider (Grafana,
    pk 3). Redirect URIs match **strictly**.
  - Duration fields (`access_code_validity`, `access_token_validity`,
    `refresh_token_validity`) must be the `key=value` format, e.g.
    `"minutes=5"` (what `authentik.lib.utils.time.timedelta_from_string`
    parses). ISO-8601 values like `"5m"` save fine but crash
    `/application/o/authorize/` with a 500 (`ValueError` in
    `timedelta_from_string`).
  - Custom scopes need a **ScopeMapping child row** (multi-table
    inheritance). A plain `PropertyMapping` attached to the provider
    silently does not advertise its scope — the token is issued without it
    (log line: "Application requested scopes not configured, setting to
    overlap"). Create it with:
    ```python
    from authentik.core.models import PropertyMapping
    from authentik.providers.oauth2.models import ScopeMapping
    pm = PropertyMapping.objects.create(name="... groups scope",
                                        expression='''return {
        "groups": [g.name for g in request.user.ak_groups.all()],
    }''')
    sm = ScopeMapping(pk=pm.pk, scope_name="groups")  # child reuses parent pk!
    sm.save()
    # then include pm in provider.property_mappings.set([...])
    ```
    `get_or_create(propertymapping_ptr=...)` does NOT work (it tries to
    insert a second parent row → IntegrityError). The stock OpenID mappings
    (openid/email/profile) are regular scope mappings already present.
  - There is no `code_challenge_methods` field on this version's
    `OAuth2Provider` (AttributeError) — S256 PKCE is just available.
- **Client-side OIDC contract** (what `gocardless-dashboard` implements;
  keep new clients consistent): authorization-code flow with **S256 PKCE**,
  where the challenge must be **unpadded** base64url — authentik recomputes
  `urlsafe_b64encode(sha256(verifier)).replace("=","")` and compares
  equality, so a padded challenge fails at token time with `invalid_grant`
  ("Code challenge not matching"). Scopes `openid profile email groups`;
  group claims come from the groups scope mapping, not a user profile
  attribute.
- Debugging: `docker logs authentik-server-1` is JSON; `system_exception`
  events carry full tracebacks, `authentik.asgi` lines carry request +
  status. OIDC endpoints: `/application/o/authorize/` (browser GET),
  `/application/o/token/` (client POST). A POST to `/application/o/authorize/`
  with an API token 403s on CSRF — expected, not a bug.
- Deployed integrations:
  - Grafana (services1): provider pk 3, client_id
    `8TMM2mYHBV2YQCovNbgGKbuEp0LJcxo2TjpqNN9s`, client secret in sops as
    `grafana_oidc_client_secret` (runtime secret on services1). Role mapping:
    authentik group `Infra` → GrafanaAdmin, else Viewer.
  - gocardless-dashboard (services1): provider pk 33 (name
    "gocardless-dashboard"), client_id
    `UonE3N21HsJH5ia12zxUSqo8wzoG19Hk6LQbOfBw`, client secret in the shared
    env-file sops secret as `GOCARDLESS_DASHBOARD_OIDC_CLIENT_SECRET`. App
    slug `gocardless-dashboard`. The client (the Rust binary) gates on the
    `Infra` group from the `groups` scope claim.
- The DB is reachable from the box:
  `docker exec authentik-postgresql-1 psql -U authentik -d authentik`

## Monster (Proxmox hypervisor, 10.3.1.11)

- `monster1` runs **Proxmox VE 9.x** and hosts the off-flake VMs (apps1,
  authentik, web1, mercury, the GOAD/k8s lab, …) — these are *not* managed by
  this flake; nothing here is declarative. Web UI:
  `https://monster.int.leighhack.org` (services1 nginx →
  `https://10.3.1.11:8006`, see `machines/services1/http.nix`).
- Unlike the flake machines, **root SSH is keyed** with the machine-hop-key:

  ```bash
  ssh -i ~/.ssh/agent-hop-key root@10.3.1.11   # monster1 (Proxmox)
  ```

  Convenience commands: `qm list`, `qm config <vmid>`, `qm start <vmid>`,
  `ha-manager status`, `pvesm status`.
- **VM disks live on the NAS**: storage `nas2-nfs` =
  `10.3.1.6:/mnt/sas-10k/proxmox-monster-ds1` mounted at `/mnt/pve/nas2-nfs`
  (`nas1-nfs` → 10.3.1.5 is disabled; `nas1`/10.3.1.5 does not answer ping).
  If the NAS is still booting after a power cut, every VM start fails with
  `TASK ERROR: storage 'nas2-nfs' is not online` (visible in
  `ha-manager status` as `service vm:<id> (monster1, error)` and in
  `/var/log/pve/tasks/active`). Fix is to wait for the export and retry —
  check `pvesm status` / `mount | grep nas2-nfs` before blaming the VMs, and
  do **not** treat the failed start tasks as a VM-level fault.
- HA-managed VMs (must be started with `ha-manager set vm:<id> --state started`,
  not `qm start`): `108` authentik, `109` apps1, `127` web1, `128` mercury.
  `onboot: 1` (autostart) on `107` mx1, `108` authentik, `109` apps1, `127`
  web1, `128` mercury and `132` Terraria; everything else is `onboot: 0` and
  started by hand (GOAD lab at 192.168.10.0/24, rtsp, windows/AD labs, …).
  Starting all six at once needs ~24 GB of the host's 32 GB RAM.

### Runbook — bringing the VMs back after a power cut (verified 2026-09-30)

Everything is slow right after a cold start because the NAS is still
importing and all VM disks are `cache=direct` qcow2s on `nas2-nfs`: measured
NFS READ RTT ~25 ms steady / up to 1.8 s during the start storm (a LAN NFS
read should be <1 ms), NAS `aqu-sz` ~6.5 and `%util` 82–84 % on the 7200 rpm
`sas-10k` disks. Expect guest boots to take many minutes and don't chase it as
a VM fault.

```bash
ssh -i ~/.ssh/agent-hop-key root@10.3.1.11
qm list                       # everything 'stopped'
pvesm status                  # nas2-nfs must say 'active' BEFORE starting anything
ha-manager status             # HA services may be in 'error' after 5 failed restarts
systemd-analyze blame | head  # host boot: ~44 s, of which pve-guests ~8 s on NFS
```

1. **Wait for the NAS export.** `pvesm status` / `mount | grep nas2-nfs` must
   show it online. NAS = `nas2` (10.3.1.6). `leigh-admin@10.3.1.6` is keyed
   with the machine-hop-key but has **no passwordless sudo**, so only
   `zpool list`, `lsblk`, `iostat`, `/proc/spl/kstat/zfs/...` work
   (`zpool status`/SMART need root — use the TrueNAS UI). Its shell is zsh:
   never `echo ===` (zsh reads `===` as command expansion).
2. **Clear HA error state — disable, *wait*, then start.** Doing both back to
   back in one loop races: the start is rejected with `service 'vm:127' in
   error state, must be disabled and fixed first`.

   ```bash
   for i in 108 109 127 128; do ha-manager set vm:$i --state disabled; done
   ha-manager status        # wait until all four report 'disabled'
   for i in 108 109 127 128; do ha-manager set vm:$i --state started; sleep 3; done
   ```

   Use `ha-manager set … --state started`, **not** `qm start`, for
   HA-managed VMs.
3. **Start the non-HA autostart VMs by hand:** `qm start 107; qm start 132`.
   During the storm `qm start` can fail with
   ``start failed: … failed: got timeout`` (qemu can't daemonize before the
   timeout while opening its qcow2). That is a pool-latency symptom, not a VM
   problem — just retry; 132 then started fine on the second attempt.
4. **Verify:** `qm list | grep -v stopped`, `ha-manager status` all `started`,
   then `qm guest cmd <id> ping` (agents answer late — `mercury` came up
   first while the rest were still booting).
5. **Leave the `onboot: 0` lab VMs alone for a bit** and start them one at a
   time (each is a burst of random reads against the slow pool); ~24 GB of
   the host's 32 GB RAM is already committed by the six infra VMs.

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
  ssh -i ~/.ssh/agent-hop-key root@10.3.1.11          # monster1 (Proxmox; root)
  ```

- Gotcha: on aibox, `/etc/hosts` maps `aibox` to `127.0.0.2` (self
  reference), so use the IPs above rather than hostnames.

## Workflow

- Deploy with the justfile: `just switch` / `just boot`
  (`sudo nixos-rebuild switch --flake .`).
- **GOLDEN RULE — confirm immediately after every switch/boot:**
  `system.autoRollback.enable = true` is set on **both** machines
  (services1 _and_ aibox, via nixos-utils). The `auto-rollback.timer` rolls
  the machine back to the last confirmed-good generation if the current one
  is not confirmed, and it acts within a minute or two of the switch. So:
  1. Run `sudo nixos-confirm` **immediately** after `switch`/`boot`
     finishes — ideally in the **same shell invocation**
     (`... switch ... && sudo nixos-confirm`), on the machine that was
     switched. A confirm run later (or via a separate ssh session after a
     delay) may land _after_ the rollback and mark the old generation good,
     which defeats the purpose.
  2. After confirming, verify `readlink -f /run/current-system` still points
     at the new generation.
  3. When switching a remote machine over ssh, chain it all in one command:
     `ssh ... 'cd ~/Projects/infrastructure-nix-flake && sudo nixos-rebuild
switch --flake .#<machine> && sudo nixos-confirm'` (the `cd`
     matters — `--flake .` resolves against the cwd).
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

The flake is pure and its sources come from the git tree, so everything
below must run from this repo root. `nixos-rebuild` is the primary tool and
handles the flake plumbing; avoid hand-rolling `import flake.nix` scripts
unless you need to extract a single value (see the gotcha there).

**Test a change without applying** (the standard, preferred way):

```bash
# services1 (the default)
nixos-rebuild dry-run --flake .

# aibox specifically
nixos-rebuild dry-run --flake .#aibox

# Actually build the toplevel locally (no deploy) to get the store path:
nixos-rebuild build --flake .#aibox   # prints the new system path
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
nix build --no-link --out-link /tmp/aibox-sys .#aibox
ls /tmp/aibox-sys/bin            # symlinks into the system
```

Gotcha: `flake.outputs` is a **function** (its args are
`{ nixpkgs, nixos-utils, llama-cpp, ... }`), and `flake.nix` itself is a plain
set — import it with no args, then call `outputs flake.inputs`. But beware:
evaluating `nixosConfigurations.aibox.config` via `import flake.nix` fails with
`attribute 'lib' missing`, because `specialArgs`/`nixpkgs.lib` need the
flake-resolved inputs that `nixos-rebuild` provides. So reach for the flake
reference form (`nix build .#aibox`) for building, and avoid
`nix eval`/`nix build` against a bare path from the repo root — those resolve
against the repo flake, not nixpkgs.

**Common checks**

```bash
nix flake show               # what the flake exposes (aibox, services1)
git status --short           # untracked files are EXCLUDED from the flake!
nix flake metadata           # locked inputs / rev
```

**Gotchas**

- Untracked files are invisible to the flake — `git add` new/changed `.nix`
  files first (see Workflow).
- `nix eval`/`nix build` on a bare path (e.g. `nix build nixpkgs#foo`)
  from the repo root will resolve against the repo flake, not nixpkgs; use
  `--file <(...)` with an explicit `import` for standalone nixpkgs lookups.
- Editing a service's `ExecStart`/flags: always re-run `nixos-rebuild build`
  and read back the rendered unit — NixOS normalises quoting and expands
  `${...}` and `
` line-continuations differently from what you typed.

## Rust builds (crane)

The Rust crates are packaged through **crane** (`github:ipetkov/crane`, shared
helper in `common/crane.nix`) rather than `pkgs.rustPlatform.buildRustPackage`, so
the dependency tree is compiled once into a cached derivation and a source-only
edit recompiles just the crate: measured on filestore, ~4 min cold vs ~50 s for a
one-line change.

Call style is `CRANE.cached { pname; version; src; cargoLock; }` — crane takes
`cargoLock` as a path, not nixpkgs' `cargoLock.lockFile` attrset, and vendors the
crates itself, so there is no `cargoDeps` hash to maintain. `crane` reaches modules
through the flake's `specialArgs`.

Two things decide whether the cache actually holds:

- Pass the lock file through `CRANE.lockFile` (a `writeText` keyed on the file's
  **contents**). Handing crane a lock file that lives inside the crate's own source
  directory makes the vendor and dummy-src derivations depend on the whole source
  tree, so any edit — even a comment in a `.rs` file — invalidates the dependency
  build.
- Keep anything that references another derivation out of the args used for
  `CRANE.deps`. A `preBuild` that points `build.rs` at the wasm bundle becomes an
  input of the dependency derivation, so a SPA-only change would rebuild the
  binary's deps. The service files therefore build a clean `args` set for the deps
  and add the hook only on the crate build.

The pinned `wasm-bindgen-cli` (0.2.128, which must match the wasm-bindgen crate in
each frontend's Cargo.lock exactly) and the wasm build recipe live in
`common/crane.nix` as `CRANE.wasmSpa` instead of being duplicated per machine.

## frigate-monitor web UI — local development

The Dioxus SPA in `frigate-monitor/frontend/` is built to wasm and embedded into
the service as `frontendDist` (see `machines/aibox/frigate-monitor.nix`).
`frigate-monitor/frontend/dist/` is **git-ignored and never committed** — the
flake rebuilds it at deploy time. To iterate locally:

```bash
nix develop            # toolchain: cargo/rustc/rustfmt + lld + just + git
just build-frontend    # = cd frigate-monitor/frontend && nix develop --command bash build.sh
# or, equivalently:
cd frigate-monitor/frontend && nix develop --command bash build.sh
```

The devshell is self-contained: `nix develop` installs the pinned
`wasm-bindgen-cli 0.2.128` (via a shellHook `cargo install -f`) so the build
works out of the box, and the stable toolchain already bundles the
`wasm32-unknown-unknown` std — **no rustup and no network needed for the
target**. `lld` is included because the wasm target links with `wasm-ld` from
it (the flake sets `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER=wasm-ld` and
passes `pkgs.lld`).

**Gotcha — wasm-bindgen CLI must match the crate version exactly.** The SPA is
compiled against `wasm-bindgen 0.2.128` (in `frontend/Cargo.lock`). The JS glue
it generates is schema-versioned, so `wasm-bindgen-cli 0.2.121` (what nixpkgs
ships) **hard-fails** with a "schema version" error, not a warning. That is why
the devshell installs 0.2.128 and the flake pins its own copy
(`buildWasmBindgenCli` + `fetchurl` from `static.crates.io`, whose API endpoint
is blocked — use `static`). The version check in `build.sh` is a _warning_, not
a guard; the real constraint is the schema match. Do **not** "fix" this by
bumping the crate or downgrading the CLI to nixpkgs' version — keep the crate
pinned at 0.2.128 and match the CLI to it.

The grid is infinitely scrollable — there is **no "load more" button**; the
`onscroll` handler appends the next page when the bottom is within ~600px.

## Known follow-ups

- ~~aibox NAS mounts~~ — resolved by commit 1811d93 ("Make aibox startup
  more reliable", deployed 2026-08-26): aibox now mirrors services1
  (explicit automounts + `_netdev` + `wait-for-network` ordering +
  `wait-for-nas`). Evaluating aibox locally still requires the `pi-room-sys`
  git input to exist at `/home/leigh-admin/Projects/pi-room-sys`.

- **3D-print servers: cloned SD identity (lime fixed, blue pending)** — the
  two Pis were cloned from one SD card, so they shared `/etc/machine-id`
  (⇒ the same NetworkManager DHCPv6 DUID) and the same SSH host keys, and the
  `3d-*` names advertised each other's IPv6 addresses. Lime's identity was
  regenerated on 2026-10-03 (`sudo rm /etc/machine-id &&
  sudo systemd-machine-id-setup`, `sudo rm /etc/ssh/ssh_host_* &&
  sudo ssh-keygen -A`, reboot); it now has its own DUID and `…::135b` lease,
  and `3d-lime.int` no longer answers as blue. Blue was off-network at the
  time and still carries the original cloned identity — harmless now that
  lime changed, but regenerate it too if blue is ever re-imaged. The
  `hackspace` user's password and `authorized_keys` are also shared by the
  clone; rotate them if the two boxes shouldn't be interchangeable.
