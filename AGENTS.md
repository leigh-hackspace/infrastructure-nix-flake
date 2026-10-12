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
  Machines: `services1` (the main services box, 10.3.1.20), `services2` (the
  observability box — gatus, Prometheus, Grafana — and the network's build
  machine, 10.3.1.21 — see `machines/services2/README.md`) and `aibox`.
- `docs/` — narrative write-ups, post-mortems and audits (gtx1060 saga,
  sso-redesign, filestore gotchas, the repo audit). They live here rather than
  next to the modules so the flake source stays config only; module comments
  point into them when they need the backstory.
- `common/` — shared modules imported by both machines (`tools.nix`,
  `users.nix`, `sops.nix`, `nixbld.nix` — the build-user-group invariant, see
  the "Nix build users" section — plus `crane.nix` — the shared crane build
  helpers, see the Rust builds section — and `frontend-build-spa.sh`, the one SPA build
  recipe all three Dioxus frontends call). Network-wide facts live here too, not
  per machine: `config.nix` (the LOCAL_NETWORK ACL, the pg_hba table, the SSO
  domains — it used to be `machines/services1/config.nix`, which is what stopped a
  second box serving any vhost), `hosts.nix` (one table of machine addresses, read
  by the nginx proxies, the Prometheus scrape targets and the gatus checks so they
  cannot disagree), `nginx/` (`sso-vhost.nix`, `int-vhost.nix`,
  `int-vhost-names.nix`), `exporters.nix` (the per-box metrics exporters),
  `build-farm.nix` (GC + remote building) and `monitoring-endpoints.nix` (the gatus
  endpoint data). `common/nginx/` and `common/config.nix` are shared because
  services1 is the TLS terminator for the whole network: a vhost defined there can
  proxy to a service running on another box.
- `common-rs/` — shared **in-repo** Rust crates used as cargo path dependencies:
  `json` (hand-rolled parser: dns-sync, moonraker-exporter), `oidc` (authentik
  login + `Infra` group gate: filestore, gocardless-dashboard), `preview` (the
  filestore preview table: extension → what a browser can render it as, read by
  **both** the API and the SPA so they cannot disagree) and `build-spa` (the
  `build.rs` that embeds a Dioxus bundle: all three SPA apps). Each consuming
  crate has a **git-ignored** `common-rs -> ../common-rs` symlink and depends on
  `path = "./common-rs/<name>"`; `just shared-rs` (or entering `nix develop`)
  creates the symlinks, and the service modules pass `sharedCrates = ["oidc"]`
  etc. so the Nix build copies the crates in. Same convention as
  `gocardless-dashboard/frontend/dto`. `CRANE.wasmSpa` takes `sharedCrates` too,
  for a SPA that reads one (filestore's).
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
  The “Members sync” page re-fetches the audit log every 15 s and shows the
  error on failure; it used to be a single fire-and-forget fetch with the error
  swallowed, so one failed request left it stuck on “loading…” until the page
  was reloaded.
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
  Preview support is the table in `common-rs/preview`: images (incl. SVG), video,
  audio, PDF and text — only types a browser can render itself, since there is no
  server-side converter, so `image/tiff`/`image/heic`/AVI and Office documents are
  deliberately not previewable.  The icon grid shows a **thumbnail** for image rows
  (`filestore/src/thumb.rs`): generated on demand and cached in temporary storage
  (`--thumb-cache`, `/run/filestore-thumbs` on services1 — tmpfs, so it is gone on a
  reboot), resized to `--thumb-max-side` (128 px) and bounded by `--thumb-cache-max`
  (256M, oldest entries pruned).  Freshness is structural, not invalidation: the cache
  key is a hash of the file's identity (`mtime + ctime + size + inode`), so any change
  to the file is a different entry and a stale thumbnail cannot be served.  The same
  identity is the `v` in the URL the SPA builds — which is what keys the *browser*
  cache — and the API reports it per entry (`Entry.fingerprint`) so the SPA does not
  recompute it; the server always rehashes from the current stat, so a stale `v`
  regenerates.  A file the decoder cannot read gets a cached failure marker (so it is
  not re-decoded on every render) and the row falls back to the emoji.
  `common_preview::thumbnailable` is the shared list of what the decoder can rasterise:
  `Kind::Image` is broader (SVG is XML, AVIF needs `ravif`), so the API and the SPA
  cannot disagree about which rows get a thumbnail. Media is streamed raw (with `Range` support, so a
  seek does not re-download the file) and text is returned as truncated JSON.
  Media responses carry `nosniff` and `default-src 'none'` CSP: an `<img>` cannot
  run script, but the same URL opened as a document can, which is the SVG case.
  While a preview is open, left/right step through the previewable files in the
  folder (clamped, not wrapped) and the row selection follows. A bare letter
  type-aheads to the entry starting with it, cycling on repeat presses. The current
  folder is mirrored into the URL hash with `replaceState` — the app keeps its own
  back/forward history (`hist`/`hidx`, Backspace), so the browser must not build a
  second one — and a hard refresh lands back in the folder.
  During a drag the directory row under the pointer is marked as the target
  (`data-fs-drop`: open-folder icon, dashed outline and a `move here` / `copy here`
  badge — `upload here` for a drag from outside the page) and the dragged rows are
  dimmed (`data-fs-drag`). The hover is decided on the real event in
  `frontend/src/js.rs` (`ensure_drag_hover`) rather than in a row handler, because
  `relatedTarget` is what tells a real leave apart from crossing to a child of the same
  row — a row-level `dragleave` cleared the highlight on every child crossing. `dragend`
  is the only event that always runs (including when the drop is refused or the drag
  leaves the page), so it is what ends the drag. A file row is never a target, and a
  folder is never a target for itself.
  The SPA was written against the dioxus 0.6 API and ported to the pinned 0.7.10;
  the port and the SPA's other traps (keyboard selection, context-menu clamping,
  drag-out to the OS, upload body limits) are written up in
  `docs/filestore-gotchas-2026-10-05.md`.
  There is a permanent headless-browser test suite in `filestore/tests/` (Node +
  Playwright, run with `just filestore-test`); see `filestore/tests/README.md`
  for the test hooks the SPA carries (`data-fs-name`, `#fs-menu`, `#fs-modal`, …)
  and the bugs it has already caught.
  Path containment: `Store::resolve` rejects any `..` component and the root is
  canonicalised at startup, but symlinks **inside** the share are followed
  wherever they point (the NAS uses them) — the guarantee is "no traversal from
  the API", not "nothing outside the share is reachable". See `filestore/src/fsutil.rs`.
- `switch-gui/` — top-level Rust tool (zero external crates) that manages the
  **ProCurve core switch** (J9280A 2510G-48, `Y.11.44`, `10.3.1.2` —
  `common/hosts.nix` → `switch`) from a browser. The switch speaks only Telnet,
  so the tool ships its own minimal Telnet client (`src/telnet.rs`), parses the
  CLI's fixed-width tables (`src/parse.rs`) and merges the four `show` commands
  that split a port's name, link state and VLAN membership between them.
  Server-rendered dashboard on the shared `common-rs/web` (same shape as
  status-dashboard), **cached** switch state (`--ttl`, default 30 s, plus a
  Refresh button) because a full read is six-plus round-trips against a 1990s
  CPU that pages its own output. Login is OIDC against authentik gated on the
  `Infra` group — hand-written (`src/oidc.rs`, S256 PKCE over `curl`, keeping the
  zero-dependency rule like `dns-sync`'s DO API calls) because `common-rs/oidc`
  would mean taking reqwest and tokio. LAN-only vhost
  `switch.int.leighhack.org` → `127.0.0.1:8097`, packaging in
  `machines/services1/services/switch-gui.nix`.
  **`agent` is a Manager, not read-only:** `enable` on this firmware is a
  *second login* — it re-asks for a login name, not just a password — and the
  same `agent` credentials are accepted, taking the prompt from `switch1>` to
  `switch1#`. The app therefore defaults its enable credentials to the login
  credentials (`--read-only` is the switch that asks for Manager not at all),
  and writes run **inside config mode**: at the plain Manager Exec prompt every
  configuration command answers `Invalid input`, so `Switch::apply` sends
  `config` first and `exit` after. Nothing saves to NVRAM, so a reboot undoes
  any change. `switch-gui/README.md` documents the CLI's
  traps (LF-only line endings, the banner swallowing input, `-- MORE --` paging
  with `NAWS` ignored, ANSI screen painting, trunk members having no rows) and
  the finding that **`Can't reach RADIUS server.` is this firmware's generic
  "authentication failed" message** — see the RADIUS entry under Known
  follow-ups for the root cause.
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
- **Core switch** is the ProCurve 2510G-48 at **10.3.1.2** (`common/hosts.nix`),
  managed over **Telnet only** (no API; the web UI needs a Manager login). Its
  `agent` login goes out over **RADIUS (authentik)** and falls back to the
  switch's local account database when RADIUS refuses, which is what makes it
  print `Can't reach RADIUS server.` on a successful login — see the RADIUS entry
  under Known follow-ups. `switch-gui/` wraps it in a browser GUI and its README
  records the CLI's quirks.
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
  MACs `c8:d3:ff:a5:b2:25`/`c8:d3:ff:a5:be:7c`), services2 (10.3.1.21, MAC
  `70:85:c2:53:f1:d4`), aibox (10.3.1.32), nas1/nas2 (10.3.1.5/10.3.1.6),
  apps1 (10.3.1.30), yunohost (10.3.1.15).
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
  members of the superuser groups `Infra` (or `pgina`), e.g. `cjdell`. The
  admin REST API is **not** usable from the box (its nginx proxies
  `/api/v3/...` collection endpoints to the UI SPA — they 404 with HTML), so
  everything goes through the `ak shell` ORM; the token-minting snippet and the
  log-noise filter are in `docs/authentik-oidc-notes.md`.
- OAuth2 providers/apps are managed via the ORM/UI (no declarative config).
  Client secrets are **write-only**: generate your own, pass it on
  create/save, and store it in sops — they can never be read back.
- **Before creating an OIDC provider, read `docs/authentik-oidc-notes.md`.**
  Three invisible bites, summarised: copy the flows + `signing_key` from a
  known-good provider (Grafana, pk 3) and match redirect URIs strictly; duration
  fields must be `key=value` (`"minutes=5"`, not `"5m"` — the latter saves fine
  then 500s at `/application/o/authorize/`); a custom scope needs a real
  **ScopeMapping child row**, a plain `PropertyMapping` is silently ignored.
- **Client-side OIDC contract** (what `gocardless-dashboard` implements;
  keep new clients consistent): authorization-code flow with **S256 PKCE**,
  where the challenge must be **unpadded** base64url — authentik recomputes
  `urlsafe_b64encode(sha256(verifier)).replace("=","")` and compares
  equality, so a padded challenge fails at token time with `invalid_grant`
  ("Code challenge not matching"). Scopes `openid profile email groups`;
  group claims come from the groups scope mapping, not a user profile
  attribute.
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

### Runbook — bringing the VMs back after a power cut

Full procedure (verified 2026-09-30): **`docs/monster-powercut-runbook.md`**.
The short version, and the parts that are easy to get wrong:

- `pvesm status` must show `nas2-nfs` **active** before any VM start; the NAS is
  still importing after an outage and every start fails with
  `storage 'nas2-nfs' is not online`. Boots take many minutes — that is pool
  latency, not a VM fault.
- HA services left in `error` must be cleared **disable → wait → start**
  (`ha-manager set vm:<id> --state disabled`, confirm with `ha-manager status`,
  then `--state started`). Doing both in one loop races and the start is
  rejected.
- HA-managed VMs (108, 109, 127, 128) are started with `ha-manager set …
  --state started`, never `qm start`. The non-HA autostart VMs (107, 132) are
  `qm start`ed by hand; a `got timeout` from `qm start` during the storm is
  pool latency — retry.
- Leave the `onboot: 0` lab VMs alone; ~24 GB of the host's 32 GB is already
  committed by the six infra VMs.

## SSH between machines (machine-hop-key)

- Every flake-managed machine shares the **machine-hop-key**: the same
  private key at `~/.ssh/agent-hop-key` on each machine, with the matching
  public key authorized for the `leigh-admin` user in `common/users.nix`
  (commit 729b1e1). Use it to hop from any flake machine to any other
  (aibox ↔ services1) without a password.
- **services2 is the exception:** it has the authorized_keys entry but not the
  private key, so you can hop *in* to 10.3.1.21 but it cannot hop *out*. See
  `machines/services2/README.md`.
- **Git from a box with no key of its own: use agent forwarding.** services2 has no
  GitHub key, but its sshd allows forwarding, so `-A` carries services1's agent to it:

  ```bash
  ssh -A -i ~/.ssh/agent-hop-key leigh-admin@10.3.1.21 \
    'cd ~/Projects/infrastructure-nix-flake && git fetch && git reset --hard origin/main'
  ```

  Two things must be true, and neither is the default:
  1. **The key GitHub accepts has to be *in* the agent.** Run `ssh-add ~/.ssh/id_ed25519`
     first. The agent sshd creates per session starts without it, and a forwarded key
     GitHub rejects is indistinguishable from forwarding no key at all: `ssh -T
     git@github.com` succeeds on services1 but the remote gets `Permission denied
     (publickey)`. Check `ssh-add -l` before suspecting the forwarding.
  2. **The connection must pass `-A`** (forwarding is off by default).

  `ssh -T git@github.com` from the remote is the one-line test. A `git bundle` from
  services1 is the fallback when forwarding is unavailable.
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
- **Check before you deploy:** `just check` (flake evaluation, the config
  assertions in `machines/services1/lib/check-*.nix` and `common/check-printer-states.nix`,
  and `alejandra --check` over every tracked `.nix`) and, when anything is built rather
  than just evaluated, `just check-build` (`nixos-rebuild dry-build` for all three
  machines — it compiles everything and activates nothing). `just test` runs the cargo
  tests, `just clippy` the lints, `just filestore-test` the browser suite, and
  `just build-remote` offloads a build to services2 (see `common/build-farm.nix`).
- **GOLDEN RULE — confirm immediately after every switch/boot:**
  `system.autoRollback.enable = true` is set on **all three** machines
  (services1, services2 and aibox, via nixos-utils). The `auto-rollback.timer` rolls
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
- Formatting: the whole tree is alejandra-clean and `just fmt` keeps it that way
  (`just fmt-check`, run by `just check`, fails otherwise). Alejandra is the only
  formatter — it is in the devshell and configured in both `.zed/settings.json`
  and `.vscode/settings.json`, so editor and CI agree. Do not hand-format or
  switch to nixfmt.


## Nix build users: `nixbld` is a target, not a grant

**Resolved 2026-10-12.** The 2026-10-11 incident — every nix build on services1 and
services2 SIGKILLing the admin's entire process set — was caused by this repo putting
`leigh-admin` in the `nixbld` group. `build-users-group` is not a permissions group, it is
the **pool of UIDs nix runs builds as**: nix takes a member of it for each build and, both
before and after that build, runs `setuid(uid); kill(-1, SIGKILL)` (`killUser()`) to sweep
the build's leftovers. A real account in that group therefore loses every process it owns
to every build on the machine — which is why the sweep looked indifferent to who asked, to
whether the build succeeded, and to which box it ran on. Full story, the nix 2.34.7 source
references, and the evidence: **`docs/nix-build-sigkill-2026-10-11.md`**. The invariant,
its assertion and its self-healing timer live in **`common/nixbld.nix`**.

Rules that follow:

- **Never put a real account in `nixbld`.** Not for a build farm, not "so the daemon
  accepts this client": that is `nix.settings.allowed-users` (already `*`) plus
  `nix.trustedUsers`, and `/nix/var/nix` is root-only so a non-root nix reaches the daemon
  socket either way. `common/nixbld.nix` fails the eval if the config asks for it, and its
  `nixbld-guard.timer` undoes an imperative `usermod -aG nixbld` / `gpasswd -a` within
  fifteen minutes and writes the reason to the journal. If a session ever dies during a
  build again, `journalctl -u nixbld-guard` is the first thing to read.
- **Ordinary deploys are safe again**: `just switch` / `just boot` as `leigh-admin`, with
  the GOLDEN RULE confirm chained in the same invocation (that is all `just` does). The
  `deploy`-account recipe below is history — do not reach for it.
- **Interrupting a build is harmless** (it always was, in the sense that nothing needed a
  reboot or a rollback to recover; that advice stands). The sweep now lands on a
  throwaway `nixbldN` uid that owns nothing.
- **Remote building is on and is now the default path.** With
  `nix.distributedBuilds = true` nix 2.34 *prefers* the external builder for every build
  and only falls back to local when no builder is available — `--max-jobs 0` does not
  enable delegation, it removes the local fallback. So a plain `nix build` / `just switch`
  on services1 builds on services2 and copies the result back. Flip
  `nix.distributedBuilds = false` in `common/build-farm.nix` to go back to local-only.
- `network-status.service` still runs as `User=leigh-admin`. That is no longer a sweep
  amplifier (uid 1234 is not a build uid any more), but a LAN dashboard should not run as
  the admin — see "Known follow-ups".

### Building in a detached unit (optional, for long builds)

Not needed for the sweep any more; still the tidy way to run a build that must outlive the
tool call that started it, because `systemd-run` without `--no-block` waits for a oneshot
to *finish* and a harness timeout then interrupts the build:

```bash
sudo systemd-run --no-block --unit=deploy-build --collect --service-type=oneshot \
  --property=DynamicUser=no --property=TimeoutStartSec=3600 \
  --setenv=PATH=/run/current-system/sw/bin --setenv=HOME=/root \
  /run/current-system/sw/bin/bash -c \
    'cd ~/Projects/infrastructure-nix-flake && nixos-rebuild switch --flake .#services1 \
       > /tmp/deploy.log 2>&1; echo SWITCH_EXIT=$? >> /tmp/deploy.log; \
       nixos-confirm >> /tmp/deploy.log 2>&1; echo CONFIRM_EXIT=$? >> /tmp/deploy.log; \
       readlink -f /run/current-system /nix/var/nix/profiles/system >> /tmp/deploy.log'
```

Chaining `nixos-confirm` inside the unit keeps the GOLDEN RULE intact. `nixos-rebuild
switch` does link `/nix/var/nix/profiles/system`, so the boot target follows the switch;
that only needs the manual `nix-env --profile ... --set` fix when activating a closure by
hand with `switch-to-configuration switch` (see the post-mortem).
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
- Shared in-repo crates go through `sharedCrates = ["<name>"]` (in both the
  `CRANE.deps` args and the `CRANE.cached` args). The build gets the real
  `common-rs/<name>` copied into the source tree; the dependency build gets a
  **stub** (real `Cargo.toml`, empty `src/lib.rs`) keyed on that manifest's
  contents. So editing shared code does not recompile the vendored deps, while
  changing a shared crate's dependencies does. Never put the symlink itself in
  git — the flake source is the git tree, and a dangling symlink outside the
  crate's own tree cannot be copied into the store.

Local Rust workflow: `nix develop` (cargo/rustc/rustfmt/**clippy** + the pinned
wasm-bindgen-cli, no network on entry) creates the path-dependency symlinks for
you. `just test` runs the unit tests (`common-rs/json`, `common-rs/oidc` have
them; the app crates mostly do not), `just clippy` lints every crate (warnings
are reported, not fatal — there are ~15 pre-existing ones), `just shared-rs`
just recreates the symlinks.

The pinned `wasm-bindgen-cli` (0.2.128, which must match the wasm-bindgen crate in
each frontend's Cargo.lock exactly) and the wasm build recipe live in
`common/crane.nix` as `CRANE.wasmSpa` instead of being duplicated per machine.

## SPA frontends — local development

The same recipe applies to all three Dioxus SPAs (`frigate-monitor/frontend/`,
`filestore/frontend/`, `gocardless-dashboard/frontend/`): each is built to wasm
and embedded into its service as `frontendDist` (see
`machines/aibox/frigate-monitor.nix`, `machines/services1/services/filestore.nix`,
`machines/services1/services/gocardless-dashboard.nix`). Their `dist/`
directories are **git-ignored and never committed** — the flake rebuilds them at
deploy time. To iterate locally:

```bash
nix develop            # toolchain: cargo/rustc/rustfmt + lld + just + git + alejandra
just build-frontend                    # all three SPAs
just build-frontend filestore          # just one
# or, equivalently, per frontend:
cd frigate-monitor/frontend && nix develop --command bash build.sh
```

(`network-status/frontend/` is the exception — SolidJS + npm, and its `dist/` is
committed; see the network-status bullet above and `just network-status-frontend`.)

The devshell is self-contained: `nix develop` ships the pinned
`wasm-bindgen-cli 0.2.128` as a derivation (`CRANE.wasmBindgenCli`, the same one
the flake uses to build the bundles — no `cargo install`, no network on entry),
and the stable toolchain already bundles the `wasm32-unknown-unknown` std —
**no rustup and no network needed for the target**. `lld` is included because
the wasm target links with `wasm-ld` from it (the flake sets
`CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER=wasm-ld` and passes `pkgs.lld`).

**Gotcha — wasm-bindgen CLI must match the crate version exactly.** The SPA is
compiled against `wasm-bindgen 0.2.128` (in `frontend/Cargo.lock`). The JS glue
it generates is schema-versioned, so `wasm-bindgen-cli 0.2.121` (what nixpkgs
ships) **hard-fails** with a "schema version" error, not a warning. That is why
the devshell pins 0.2.128 and the flake pins its own copy
(`buildWasmBindgenCli` + `fetchurl` from `static.crates.io`, whose API endpoint
is blocked — use `static`). `common/frontend-build-spa.sh` reads the wanted
version out of the frontend's own `Cargo.lock` and **fails** on a mismatch (the
build would die anyway). Do **not** "fix" this by bumping the crate or
downgrading the CLI to nixpkgs' version — keep the crate pinned at 0.2.128 and
match the CLI to it.

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

- **services2: hop key and swap** — `machines/services2/README.md` claimed the
  shared machine-hop key was in place; it is not. Only `authorized_keys` is
  installed there (by `common/users.nix`), so you can hop *in* to 10.3.1.21 but
  not *out* — it cannot ssh to services1, and it has no GitHub key of its own. Fetch
  there with `ssh -A` (agent forwarding; see the SSH section) rather than copying a
  private key around. Install `~/.ssh/agent-hop-key` deliberately if you want it to hop
  out; do not copy a private key as a side effect of a deploy. services2 also has no swap
  partition (services1 has 18 GB), which is fine at the current load but worth
  matching before it runs anything heavier.

- **Remote building is verified working (2026-10-12)** — with the `nixbld` fix in
  `common/nixbld.nix` deployed to services1 and services2, a delegated build and a
  `--max-jobs 0` build both completed on services2 and copied back to services1, with no
  SIGKILL sweep on either box. It is now the default path rather than opt-in (nix 2.34
  prefers an available builder; see the "Nix build users" section). Worth knowing that
  services2 was found running a *newer* generation than its git tree (its checkout was
  two commits behind `origin/main` after the 2026-10-11 deploys were done from a dirty
  tree) — check `git log -1` there before blaming a config change for behaviour.
- **The `deploy` account is dead weight** — `useradd`ed on 2026-10-11 as a workaround for
  the build-user bug (build as a uid that owns nothing), with its own copy of the repo and
  the private flake inputs under `/home/deploy`. The bug is fixed, so nothing needs it;
  it is not declarative and a switch will not recreate it. Remove it
  (`userdel -r deploy`) unless something turns out to depend on it.
- **The `deploy` account is an imperative escape hatch** — created with `useradd` on
  2026-10-11 to deploy while every build killed the admin's session, with its own copy of
  the repo and the private flake inputs under `/home/deploy` (see the deploy recipe in the
  section above). It is not declarative and a switch will not recreate it. Decide whether to
  make it managed config or remove it once builds as `leigh-admin` are trusted again.
- **`network-status.service` runs as `User=leigh-admin`** — it should have its own user.
  During the 2026-10-11 incident that is what turned the uid-wide kill into a *service*
  outage (SIGKILLed and restarted seven times) rather than just a dead terminal; the sweep
  can no longer reach uid 1234, but a LAN dashboard running as the admin is still its own
  problem. Blocked on the hop key the unit needs (`~/.ssh/agent-hop-key` to
  reach the router): a dedicated user needs that key in sops, or a dedicated router key.

- **Two public status vhosts have no DNS record** (found 2026-10-12 while sanity-checking
  after the `nixbld` fix). `machines/services1/services/status.nix` defines the SSO vhosts
  `status.leighhack.org` and `aibox.status.leighhack.org`, and
  `common/monitoring-endpoints.nix` monitors `https://status.leighhack.org`, but both names
  are NXDOMAIN in the DigitalOcean zone, so they are unreachable and that gatus check
  cannot be passing. The `*.int` dashboards are fine
  (`services1-status.int` / `services2-status.int` / `aibox-status.int` / `network-info.int`
  all serve 200) — dns-sync only manages the `*.int` CNAMEs, public names are added by hand
  in DO DNS, and these two were never. Either add the records (public apps CNAME →
  `nginx.leighhack.org`) or drop the vhosts and the endpoint.

- **switch-gui: two steps left before it serves anything** — the code, the
  service and the secret are all in place (`nix eval` of
  `systemd.services.switch-gui` is clean, 86 unit tests pass, and it has been run
  read-only against the live switch: 10 VLANs, 50 ports). Done already:
  `secrets/secrets.yaml` has the `switch_gui_env_file` key (4 keys:
  `SWITCH_GUI_LOGIN`, `SWITCH_GUI_PASSWORD`, `SWITCH_GUI_OIDC_CLIENT_ID`,
  `SWITCH_GUI_OIDC_CLIENT_SECRET`; the unit uses it as `EnvironmentFile`, so the
  service runs as a `DynamicUser` and never opens it), and authentik has an
  **`switch-gui` OIDC provider (pk 35)** copied from gocardless-dashboard —
  strict redirect `https://switch.int.leighhack.org/auth/callback`, the three
  OpenID mappings plus a `groups` scope, and the same flows/signing key. Verified
  by a real S256 PKCE `/authorize` request: 302 into the auth flow, where a bogus
  client id gets 400. Still to do:
  1. **Commit the `secrets/secrets.yaml` change on services1** — it is written
     but uncommitted, and services1/services2 get `git reset` to `origin/main`
     periodically.
  2. `just dns-sync-sync` after deploying — the vhost is picked up from
     `services.nginx.virtualHosts` automatically (verified: `switch.int` appears
     in `/etc/dns-sync/expected-int-names`).
  Note the provider's `groups` scope reuses gocardless-dashboard's ScopeMapping
  row rather than owning one: creating a fresh child row through the ORM trips a
  unique-violation on `propertymapping.name` (see the authentik notes).
- **switch-gui: the write commands are unverified** — `src/commands.rs` builds
  `port <n> name "…"`, `port <n> vlan <id>`, `vlan <id> name "…"`,
  `no vlan <id>`, `vlan <id> tagged <ports>` from the platform's command
  reference and unit-tests them, but none has been sent to the switch. The
  Manager context is reachable (see the switch-gui entry above), so the first
  real use is the test; the switch's own `Invalid input:` text is surfaced
  verbatim, every attempt is in the audit log, and a refused command now fails
  the request instead of reporting "applied". Nothing in the app writes NVRAM,
  so **a change lives in RAM until someone saves it** — say so before trusting
  one, and expect to correct the syntax on first use.
- **The RADIUS shared secret was printed into a session transcript on
  2026-10-12** (reading `show running-config` on the switch to root-cause the
  login noise; a masking filter that expected `key:` missed the actual
  `radius-server key <secret>` form). Rotate it on both ends — the switch
  (`radius-server key`) and the authentik RADIUS outpost — and note that the
  same command also exposes the local user hashes.
- **`Can't reach RADIUS server.` from the switch means "RADIUS refused this
  login", and the cause is a password mismatch** (root-caused 2026-10-12; an
  earlier note here blaming EAP/MS-CHAP attribute mapping was wrong). Evidence:
  `ak-outpost-radius-outpost` logs `Access-Request` with `ip: 10.3.1.2`, so the
  switch reaches authentik; authentik then writes `login_failed` events with
  `client_ip: 10.3.1.2`, `username: agent`, rejected by the **identification
  stage** — i.e. bad credentials. authentik really does have an active `agent`
  user, but its password is not the switch's local `agent` password, so RADIUS
  always refuses and the switch **falls back to its own local account database**
  and lets you in, printing the misleading message on the way. That is why it
  "happens from time to time": it happens on every login, and only shows when
  you are watching. Corroborating timing: a wrong password produces it in under
  two seconds, where a real network timeout costs ~5 s per retry.
  The fix is to set the authentik `agent` password to match the switch (or drop
  RADIUS from that account's auth order). **Confirmed from the device itself**
  once in the Manager context — `show running-config` reads:

  ```text
  aaa authentication telnet login  radius local
  aaa authentication telnet enable radius local
  radius-server host 10.3.1.36
  ```

  so both the login *and* the `enable` second login go RADIUS-then-local, which
  means every GUI refresh (which logs in and enables) logs **two** failures in
  authentik. `show aaa` and `show radius` are not commands on this firmware —
  the earlier note here saying they were "refused in Operator context" was
  wrong twice over: `agent` is a Manager after `enable`, and the commands simply
  do not exist. Until this is fixed, `login_failed` alerting in authentik is
  permanently noisy because of this. Separately: `RadiusProvider` pk 7
  (application `Switches`) has `authentication_flow = None` and the flow
  actually used is `radius-authentication-flow` — not the cause here, but worth
  tidying.
