# AGENTS

Guidance for AI agents working in this repository. Read this before making changes.

## Language preference

- **Use Rust for new programs/tools.** Do not write new services, daemons,
  scripts-as-programs, or CLI tools in Python (or other languages) without a
  strong reason. See `crates/status-dashboard/` for the
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
- `crates/` — **every Rust crate lives under here**, and `crates/Cargo.toml` makes
  the native ones **one cargo workspace**: one `Cargo.lock`, one `target/`, and
  `cargo build|test|clippy --workspace` (or `-p <name>`) from `crates/` covers the
  whole fleet, which is what rust-analyzer wants too. The programs are top-level
  directories here; the shared libraries are `crates/common-rs/`.
- `crates/common-rs/` — the shared **in-repo** crates, plain path dependencies
  (`common-json = { path = "../common-rs/json" }`, no symlinks needed):
  `json` (hand-rolled parser **and** the string/error-object emitters the
  hand-built API bodies use: dns-sync, moonraker-exporter, status-dashboard,
  network-status, frigate-monitor, switch-gui), `oidc` (authentik login + `Infra`
  group gate: filestore, gocardless-dashboard, switch-gui), `web` (the HTTP/1.1
  plumbing the tiny_http services share: request head, query/cookie parsing,
  responses), `pg` (the pool, the schema runner, the sea-query→sqlx glue and the
  row readers), `service` (the startup plumbing that is not the service: the flag
  reader, the `EnvironmentFile` parser, and the loopback test that gates every
  `--no-auth`), `preview` (the filestore preview table: extension → what a browser
  can render it as, read by **both** the API and the SPA so they cannot disagree)
  and `build-spa` (the `build.rs` that embeds a Dioxus bundle: all four SPA apps).
  **New shared code goes here, not into a second copy in the second crate** — the
  reason `service` and `json`'s emitters exist is that both had already drifted
  between three callers.
  The four Dioxus SPAs are **not** workspace members: each `frontend/` is its own
  workspace (own `Cargo.lock`, wasm32, pinned wasm-bindgen) and reaches its shared
  wire types through a **git-ignored** symlink (`frontend/dto -> ../dto`,
  filestore's `frontend/common-rs`) that `just shared-rs` (or entering `nix
  develop`) creates; the Nix build materialises a real copy instead, via
  `sharedCrates = ["oidc"]` etc. — a symlink cannot travel in a flake source. The
  reason none of this is a symlink for the *native* crates, and why the SPAs are
  excluded, is written out at the top of `crates/Cargo.toml`.
- `crates/dns-sync/` — Rust tool (zero external crates) that keeps the
  router's dnsmasq and DigitalOcean DNS in step with the `*.int.leighhack.org`
  nginx vhosts. Wired in via `machines/services1/dns-sync.nix`, which also
  renders the expected-name list to `/etc/dns-sync/expected-int-names`.
  Run with `just dns-sync-check` / `just dns-sync-sync` / `just dns-sync-prune`
  (router + DO facts below).
- `crates/frigate-monitor/` — Rust tool (one external crate: `image`) on
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
- `crates/gocardless-dashboard/` — GoCardless Pro sync + operator dashboard on
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
- `crates/network-status/` — the router network dashboard (served at
  `network-info.int.leighhack.org`). The Rust binary (`src/`, zero external
  crates) ssh's to the router every 5s, keeps a rolling history, serves the
  JSON API (`/api/snapshot`, `/api/history`, `/api/config`) and static files
  via `--static-dir`. The frontend is a SolidJS + TypeScript SPA in
  `frontend/` (esbuild + `esbuild-plugin-solid`, minimal deps — no router, state, or chart libs). `frontend/dist/` is **committed** and copied into the Nix store
  by `machines/services1/network-status.nix` (Nix builds are offline and the
  npm deps aren't nixpkgs-cached, so the SPA is not rebuilt in Nix): after
  editing the SPA, run `npm install && npm run build` in `frontend/` and commit
  the result.
- `crates/filestore/` — Rust tool (axum + reqwest) on services1: a web file
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
  (`crates/filestore/src/thumb.rs`): generated on demand and cached in temporary storage
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
- `crates/switch-gui/` — Rust tool that manages the
  **three ProCurve switches** from a browser: `switch1` (J9280A 2510G-48,
  `Y.11.44`, `10.3.1.2`), `switch2` (same model, `10.3.1.3`) and `switch3`
  (J4903A 2824, `I.10.107`, `10.3.1.4`) — all three in `common/hosts.nix` under
  those names, which are also each device's own `System Name` and the names the
  GUI's URLs use (`/switch/switch2`). The switches speak only Telnet, so the tool
  ships its own minimal Telnet client (`src/telnet.rs`), parses the CLI's
  fixed-width tables (`src/parse.rs`) and merges the six `show` commands that
  split a port's name, link state and VLAN membership between them. **Port names
  come only from `show running-config`**: the `Name` column of `show vlan <id>` is
  blank for every dynamically-learned member (all fifty ports on switch1) and
  `show port name` does not exist on this firmware. That command is also the only
  read whose output carries secrets, so it goes through `Switch::secret_command`
  (failures never quote its output) and `parse::port_names` returns names and
  nothing else. The Telnet client, the parsers and the command generator are still
  **crate-free on purpose**; what took crates is the other half (below): sqlx +
  sea-query + tokio + reqwest, and `common-rs/{json,web,oidc,pg,build-spa}`.
  Below the port table is a **VLAN membership matrix** — one row per port, one
  **vertical** column per VLAN, with the untagged VLAN a dropdown — which is the
  only place membership is edited. The form sends the whole wanted row and
  `commands::port_members` turns it into the diff against the shadow copy
  (removals, then the native-VLAN move, then additions: a port cannot be tagged and
  untagged on one VLAN at once); the row is marked changed until Apply (the edit is
  an overlay over the polled rows, so the poll keeps running underneath it), and a
  trunk member **or** trunk group is locked — `dto::Port::membership_editable` is
  the one rule, and it checks `trunk_group` (set on members) *and* the id (the
  group's own row has no `trunk_group`), because the switch refuses the first and
  this tool has never exercised the second. A third rule is kept true **by the
  form** rather than only refused by the server: a port is never tagged in its own
  native VLAN, so the column of the row's native VLAN is not tickable (a tick
  already there can always be removed) and moving the native VLAN off a ticked
  VLAN unticks it. `commands::port_members` still refuses the impossible state; the
  form just never asks for it, because a refusal the operator cannot see looks
  exactly like a dead button.
  **Nothing reads a switch while serving a request.** A background sync
  (`src/sync.rs`) re-reads each switch into a **shadow copy in Postgres** and drains
  a **queue** of wanted changes in the same database, which is also the audit trail
  (rows, with Retry / Discard / Revert — `Discard` only ever deletes a row that has
  not been written). The UI is a Dioxus SPA (`frontend/`, wasm-built by the flake,
  embedded by `build.rs`, no bundle committed) over a JSON API (`/api/state`,
  `/api/switch`, `/api/pending`, `/api/changes`, `/api/intent`,
  `/api/change/{retry,forget,revert}`, `/api/export`), and the wire types are the
  shared `switch-gui/dto` crate (`switch-gui-dto`), exactly as
  gocardless-dashboard shares `gdash-dto`. Login is OIDC against authentik gated on
  the `Infra` group through `common-rs/oidc` (the hand-written `src/oidc.rs` +
  `curl` is gone: the pool and the query builder were already bringing tokio, so
  the zero-dependency rule stopped buying anything). The database is `switch_gui`
  (role and DB from `machines/services1/services/postgres.nix`) over the **unix
  socket**, which `CONFIG.PG_AUTH` trusts for local users — so the unit needs no
  database credential and stays a `DynamicUser`. LAN-only vhost
  `switch.int.leighhack.org` → `127.0.0.1:8097`, packaging in
  `machines/services1/services/switch-gui.nix` (the switch list reaches the unit as
  `SWITCH_GUI_SWITCHES`, built from `common/hosts.nix`; `--interval` is how often
  the sync re-reads, 60 s there rather than the default 30 because each read is a
  **login** and every login currently logs two `login_failed` events in authentik —
  see the RADIUS entry under Known follow-ups).
  **The 2824 is not the same firmware**, and three things bite a client written
  against the 2510G: `show vlan` has an extra `Voice` column (so the number of
  trailing columns must come from the header), `show spanning-tree` puts
  `Cost/Priority/State` in the same `|`-block as the port, and its banner ends
  with `Press any key to continue`, which the client must answer before typing a
  login name — otherwise the first character is eaten by the keypress, `agent`
  arrives as `gent`, and RADIUS refuses it. All three are caught by fixtures taken
  from the live devices; see `switch-gui/README.md` ("Two firmwares, one parser").
  **`agent` is a Manager, not read-only:** `enable` on this firmware is a
  *second login* — it re-asks for a login name, not just a password — and the
  same `agent` credentials are accepted, taking the prompt from `switch1>` to
  `switch1#`. The app therefore defaults its enable credentials to the login
  credentials (`--read-only` is the switch that asks for Manager not at all),
  and writes run **inside config mode**: at the plain Manager Exec prompt every
  configuration command answers `Invalid input`, so `Switch::apply` sends
  `config` first and then exits until the prompt stops naming a context (one
  `exit` from `switch1(eth-23)#` only reaches `(config)#`). Nothing saves to
  NVRAM, so a reboot undoes any change. `switch-gui/README.md` documents the CLI's
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
    dashboard (shared Rust code in `crates/status-dashboard/`, shared
    module in `common/status-dashboard.nix`, run on both machines;
    services1 reverse-proxies both as `status.*` / `aibox.status.*`).
  - `lib/` — nginx SSO helpers (`nginx-sso-helper.nix` etc.).
  - `config.nix` — paths to secrets under `/var/lib/secrets` (do not read
    secret contents; reference the paths).
- `secrets/` — sops-encrypted.

## Infrastructure facts

- NAS is TrueNAS at **10.3.1.6**. NFS shares: `/mnt/cameras`,
  `/mnt/filestore`, `/mnt/backups`. It is slow to come up after a power cut.
  `leigh-admin` has **passwordless sudo** there (verified 2026-10-13). Set it as the
  middleware **user record** (`sudo_commands_nopasswd = ["ALL"]`, via the REST API on
  `/run/middleware/middlewared.sock`), not by editing `/etc/sudoers`: middleware
  regenerates that file from `middlewared/etc_files/local/sudoers.mako` at boot, so a
  hand edit is wiped by the next `etc.generate('user')` — and the generated file has no
  `#includedir`, so a `/etc/sudoers.d` drop-in is never read either.
- **The borg backup server** is an **Incus container** on the NAS named `borg-server`
  (out-of-flake, like the rest of the NAS): `sudo incus exec borg-server -- <cmd>`.
  The repo dataset `/mnt/sas-10k/backups` is mounted in it as `/backups`, and its
  **proxy device forwards host 3022 → container 22**, which is the port the
  `borgbackup` job in `machines/services1/services/backup.nix` uses. The `backups`
  user inside it has no forced command in `authorized_keys` (a single key,
  `backups@services1.int.leighhack.org`). borg there is a **standalone PyInstaller
  binary** at `/usr/local/bin/borg` — the image has no pip — so upgrading means
  dropping in the GitHub release asset (`borg-linux-glibc235-x86_64-gh`) and keeping
  the previous copy for rollback. Upgraded to **1.4.5** on 2026-10-13 (was 1.4.3; the
  container image is Ubuntu noble, serial `20251210_07:42`). Keep the server on the same
  1.4.x line as the client: nixpkgs ships borg **1.4.5** on services1, and a borg 2.x
  `serve` cannot talk to a 1.4 client, so the 2.0.0bx releases are not an option while
  the job is a 1.4 client.
- **Core switches** are the ProCurve 2510G-48s at **10.3.1.2** (`switch1`) and
  **10.3.1.3** (`switch2`), plus the 2824 at **10.3.1.4** (`switch3`)
  (`common/hosts.nix`), managed over **Telnet only** (no API; the web UI needs a
  Manager login). Their `agent` login goes out over **RADIUS (authentik)** and
  falls back to the switch's local account database when RADIUS refuses, which is
  what makes it print `Can't reach RADIUS server.` on a successful login — see the
  RADIUS entry under Known follow-ups. `switch-gui/` wraps them in a browser GUI
  and its README records the CLI's quirks.
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
  tests, `just clippy` the lints, `just filestore-test` and `just switch-gui-test` the
  headless-browser suites (the latter drives a fake ProCurve console — see
  `switch-gui/tests/README.md`), and
  `just build-remote` offloads a build to services2 (see `common/build-farm.nix`).
- **GOLDEN RULE — confirm immediately after every switch/boot:**
  `system.autoRollback.enable = true` is set on **all three** machines
  (services1, services2 and aibox, via nixos-utils). The `auto-rollback.timer` rolls
  the machine back to the last confirmed-good generation if the current one
  is not confirmed. What it actually does, read off the unit script rather than guessed:
  activating a configuration **restarts the timer**, which fires **once**, five minutes
  later (`OnActiveSec=5 min`, `RemainAfterElapse=false`); on firing it compares
  `readlink /run/current-system` with `/nix/var/nix/profiles/system-good` and, if they
  differ, walls, sleeps 30 s, and **reboots the box into `system-good`**
  (`switch-to-configuration boot` then `sync; echo b > /proc/sysrq-trigger`). So the
  window is five minutes, not a minute — but it is still the same shell invocation, and
  not only because a late confirm marks the *old* generation good: a rollback reboots
  the machine out from under whatever the next tool call is doing. `nixos-confirm` is
  what repoints `system-good`, and the state is readable without nix at all:

  ```bash
  readlink /run/current-system /nix/var/nix/profiles/system-good   # equal = confirmed
  ```

  So:
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
- **Long jobs are polled, never slept through** — no `sleep` over 30 s in a tool call.
  See "Long jobs: poll, don't sleep".
- Formatting: the whole tree is alejandra-clean and `just fmt` keeps it that way
  (`just fmt-check`, run by `just check`, fails otherwise). Alejandra is the only
  formatter — it is in the devshell and configured in both `.zed/settings.json`
  and `.vscode/settings.json`, so editor and CI agree. Do not hand-format or
  switch to nixfmt.


## Long jobs: poll, don't sleep

**No `sleep` longer than 30 seconds in a tool call.** A long sleep is a blind wait: it
reports nothing until it ends, so a job that failed in its first second still costs the
whole wait. There is no point sitting through five minutes for a build that has already
failed — the error is only useful if it is read as soon as it appears.

- Poll instead. Read the state, act on what it says, re-read on the next tool call. The
  loop is the agent, not `sleep` in the shell.
- Cheap polls, one per tool call: `systemctl is-active <unit>`,
  `systemctl show -p Result --value -p ActiveState <unit>`, `journalctl -u <unit> -n 50`,
  `pgrep -f <pattern>`, `tail -1 /tmp/deploy.log`, `pvesm status | grep nas2-nfs`,
  `curl -fsS -m 5 <url>`, `nixos-rebuild dry-run --flake .#<machine>`.
- Bound every wait so a hang reports as a failure rather than as a tool-call timeout:
  `curl -m 5`, `ssh -o ConnectTimeout=5`, `timeout 60 ...`. A poll that times out *is* the
  answer.
- Short sleeps are fine when they are the only way to catch a transition (a timer that
  fires in a few seconds, a service settling after a restart, a retry backoff). The rule is
  about waiting for a *result* that could have been read immediately.
- Anything that genuinely takes minutes goes in a detached unit writing to a log (see
  "Building in a detached unit"), then gets polled: check whether the process is still
  running, read the log, and stop the moment it exits — not at the estimated duration.
  Polling also catches the failure modes a sleep hides: a build that dies immediately, a
  unit that exits 0 but did nothing, a remote builder that silently falls back to local.
- **An aborted tool call does not stop what it started, and a nix client holds the
  global store lock for its whole evaluation.** Seen 2026-10-14: a `nix eval` whose tool
  call reported `Command aborted` went on running for minutes, holding
  `/nix/var/nix/db/big-lock` (nix's exclusive NixDB lock). Every nix command after it
  queued behind that lock and did *nothing* — no CPU, no output, no timeout — which is
  indistinguishable from "this eval is just slow", and it read as several dead evals on
  top of the one that was actually stuck. So: **after aborting a nix tool call, find the
  survivor and kill it before issuing another nix command**; and when a nix command
  hangs, look for the lock holder before believing it is evaluating. None of these run
  nix, so they work while it is wedged:

  ```bash
  pgrep -a -f "nix eval|nix build"     # survivors of aborted calls — kill these
  ps -o pid,stat,etimes -p <pid>       # D = blocked in I/O, not computing
  sudo ls -l /proc/<pid>/fd | grep big-lock   # the holder is a nix-daemon child
  ```

  Killing the client takes the daemon's child, and the lock, with it. `nix eval --raw
  --expr '"ok"'` returning instantly is the proof the store is free again. (The stuck
  eval itself was my own doing — see the path-literal gotcha under "Nix commands".)

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

Poll `/tmp/deploy.log` and the unit state after starting it (`systemctl is-active
deploy-build`, `journalctl -u deploy-build`), do not sleep to the expected finish time.

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
- **Never evaluate this repo through a path literal.** `import
  /home/leigh-admin/Projects/infrastructure-nix-flake/common/crane.nix` resolves the
  `../crates` inside it against the **real working tree**, and `lib.cleanSourceFilter`
  drops only VCS directories and `~`/`.o`/`.so`/`result*` names — it does **not** read
  `.gitignore` — so `cleanSourceWith { src = ../crates; }` copies `crates/target/`
  (**3.1 GB** of cargo artifacts) into the store while holding the big lock above. The
  flake path is unaffected: through `self` the same directory arrives already filtered to
  git-tracked files (the store copy is 132K), which is why nothing in a deploy hints at
  this and it only bites a throwaway `nix eval --file` that measures something. Read
  values out of the flake instead — `nix flake check --no-build`, `nixos-rebuild
  dry-build`, then `$SYS/etc/systemd/system/<unit>.service` — or, for a value with no
  flake output, `nix eval --impure --expr` against `builtins.getFlake` (flake sources
  are git-filtered; path literals are not).
- **`nix flake check --no-build` is expected to be warning-free**, so a warning is a
  finding rather than noise. The four classes that were live until 2026-10-14: crane
  guessing `pname`/`version` from the workspace root (it is a *virtual* manifest — pass
  both to `buildDepsOnly`, see `common/crane.nix`), the renamed `nix.trustedUsers`
  (now `nix.settings.trusted-users`), and a unit `after` `network-online.target`
  without `wants`/`requires` it. It still ends with
  `error: path '...sops-secret-grafana_secret_key.drv' is not valid`, which predates
  all of that and is not yours to chase — verify against clean HEAD (`git stash`) before
  hunting any error you think your change caused.
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
one-line change. `crane` reaches modules through the flake's `specialArgs`.

Call style is `CRANE.crate { pname; version; member; sharedCrates; }` — one
workspace member, built with `cargo build -p <pname>`. `crane` takes `cargoLock`
as a path, not nixpkgs' `cargoLock.lockFile` attrset, and vendors the crates
itself, so there is no `cargoDeps` hash to maintain.

**Because `crates/` is one cargo workspace there is one dependency derivation for
the whole fleet** (`CRANE.workspaceArtifacts`), which is the main win: before the
workspace, filestore, gocardless-dashboard, frigate-monitor and switch-gui each
compiled their own copy of reqwest, because each had its own lock file and so its
own vendor dir. The two things that keep the per-crate build times anyway:

- **The source tree is the workspace with stub sources everywhere except the
  member being built** (`memberSrc`, built from `craneLib.mkDummySrc`). Cargo
  cannot build one member without loading every member's manifest — and it refuses
  a manifest whose declared targets are missing — so the tree has to carry the
  whole workspace. Stubs are what stop that meaning every `.rs` edit rebuilds all
  eight programs: cargo sees the other members' manifests and no code.
- **The lock file reaches crane through `CRANE.lockFile`** (a `writeText` keyed on
  its **contents**), so the vendor derivation is keyed on dependencies rather than
  on the source tree. Changing a dependency anywhere invalidates the shared dep
  build for everyone — that is the price of a workspace, and the same price cargo
  charges locally.

`sharedCrates = ["<name>"]` names the `crates/common-rs/<name>` crates the member
reads: they are already in the tree as stubs (enough for cargo to *resolve* them)
and this swaps in the real code. **A crate that reads none must pass none** —
listing one it does not depend on makes its source an input of the build, so
editing it triggers a rebuild that changes nothing. `dto`-style crates that live
inside the app's own tree need no entry.

A `preBuild` that points `build.rs` at a wasm bundle is *not* an input to the
shared dependency derivation (the hooks are never passed to `buildDepsOnly`), so a
SPA-only change still does not rebuild a binary's deps. Pass `cargoArtifacts` only
to override the shared derivation deliberately.

Local Rust workflow: `nix develop` (cargo/rustc/rustfmt/**clippy** + the pinned
wasm-bindgen-cli, no network on entry) creates the SPAs' path-dependency symlinks
for you. `just test` and `just clippy` are `cargo test|clippy --workspace` from
`crates/` — one command, every crate. `just shared-rs` recreates the SPA symlinks.

The pinned `wasm-bindgen-cli` (0.2.128, which must match the wasm-bindgen crate in
each frontend's Cargo.lock exactly) and the wasm build recipe live in
`common/crane.nix` as `CRANE.wasmSpa` instead of being duplicated per machine.

## SPA frontends — local development

The same recipe applies to all four Dioxus SPAs
(`crates/frigate-monitor/frontend/`, `crates/filestore/frontend/`,
`crates/gocardless-dashboard/frontend/`, `crates/switch-gui/frontend/`): each is
**its own cargo workspace**, not a member of the native one (see
`crates/Cargo.toml`), which is why it has its own `Cargo.lock` and why its shared
path dependencies arrive by symlink/copy rather than by relative path:
each is built to wasm and embedded into its service as `frontendDist` (see
`machines/aibox/frigate-monitor.nix`, `machines/services1/services/filestore.nix`,
`machines/services1/services/gocardless-dashboard.nix` and
`machines/services1/services/switch-gui.nix`). Their `dist/`
directories are **git-ignored and never committed** — the flake rebuilds them at
deploy time. To iterate locally:

```bash
nix develop            # toolchain: cargo/rustc/rustfmt + lld + just + git + alejandra
just build-frontend                    # all four SPAs
just build-frontend filestore          # just one
# or, equivalently, per frontend:
cd crates/frigate-monitor/frontend && nix develop --command bash build.sh
```

(`crates/network-status/frontend/` is the exception — SolidJS + npm, and its
`dist/` is committed; see the network-status bullet above and
`just network-status-frontend`.)

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

- **`crates/target/` is not excluded from `workspaceSrc` (open, one line)** —
  `common/crane.nix` filters the four SPA directories out of `workspaceSrc` and nothing
  else, and `lib.cleanSourceFilter` does not read `.gitignore`, so an evaluation that
  reaches `crates/` as a **path literal** copies 3.1 GB of cargo artifacts into the store
  while holding the nix big lock — which is what wedged the box on 2026-10-14 (see the
  gotcha under "Nix commands (local dev)"). Harmless through the flake, since `self`
  arrives git-filtered, so this is hardening rather than a fault: adding `target` to that
  filter makes the mistake impossible to repeat.
- **Evaluation warnings cleaned up (2026-10-14, deployed)** — `nix flake check
  --no-build` is warning-free now. `buildDepsOnly` states `pname`/`version` instead of
  letting crane guess them from the *virtual* workspace manifest (it warned about both on
  every evaluation); `nix.trustedUsers` became `nix.settings.trusted-users`, which
  renders the identical `nix.conf` (`trusted-users = root leigh-admin`, so remote
  building is untouched); and `filestore.service` and `gocardless-dashboard.service`
  gained the `wants = [ "network-online.target" ]` their `after` had always implied —
  the house pattern every other net-dependent unit already had. Naming the dependency
  derivation changes its name, so the first build after it recompiles the shared
  workspace deps once; already paid (`crates-workspace-deps-0.1.0` is in the store).

- **`crates/` + the cargo workspace, and the `--no-auth` loopback bug it fixed
  (2026-10-13)** — every Rust crate moved under `crates/` and the native ones
  became one cargo workspace (`crates/Cargo.toml`): one `Cargo.lock`, one
  `target/`, `cargo --workspace` for test/clippy, and one shared dependency
  derivation instead of eight. The SPAs stayed separate workspaces and kept their
  symlinks; the reason is at the top of `crates/Cargo.toml`.
  While moving it I deduplicated the startup plumbing into a new
  `crates/common-rs/service`, and **that is where the bug was**: filestore's
  `is_loopback` ended in `host.starts_with("127.")`, so `--no-auth` — the escape
  hatch whose *only* guard is "this bind is loopback" — accepted
  `--bind 127.example.com`, a hostname anyone with a DNS zone can point at a
  routable interface. Verified against the deployed binary before the change
  (accepted, and it went on to try to bind) and after (refused). switch-gui had
  already written the careful version, which is the one now shared: parse the host
  as an `IpAddr` and ask `is_loopback()`, accept `localhost` by name and nothing
  else that will not parse. `common_service::is_loopback` carries the tests that
  were scattered across the three callers, including `127.example.com`,
  `127.0.0.1.evil.com` and `::1234`.
  Also shared out at the same time: `common_json::quoted`/`error_object` (four
  copies of a JSON string escaper existed, one of them — frigate-monitor's — not
  escaping control characters at all) and the `pg` row readers (`text`/`opt_text`/
  `integer`/`opt_integer`, duplicated between switch-gui's two DB modules).
  Nothing here is deployed yet: the tree builds clean for all three machines
  (`nixos-rebuild build` for services1 and aibox, `dry-build` for services2),
  `cargo test --workspace` is green with no new clippy warnings, and the filestore
  binary built from it was run by hand through the loopback gate above.


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

- **services1 could not `CREATE DATABASE` (found and fixed 2026-10-13)** —
  `sudo -u postgres psql -c "CREATE DATABASE x"` failed with
  `ERROR: template database "template1" has a collation version mismatch` (created
  under collation version 2.42, the box now runs 2.44 — a glibc upgrade, and Postgres
  17 refuses to copy a template whose recorded version differs). Nothing existing
  broke because no new database had been asked for since the upgrade; `switch_gui`
  was the first. **Fixed by refreshing the recorded version on the maintenance
  databases** — `ALTER DATABASE template1 REFRESH COLLATION VERSION;` (and `postgres`;
  `template0` refuses with `invalid collation version change` because it records no
  version, and does not need one: `CREATE DATABASE` copies `template1`). Verified by
  creating and dropping a probe database.
  What the refresh asserts is that the on-disk collation still matches the indexes,
  which for a 2.42 → 2.44 bump is almost certainly true but is not *proven* — the
  strict route is `REINDEX` of the collated indexes. The **application databases still
  record 2.42** and so still log `database "…" has a collation version mismatch` on
  connect; refreshing them is a one-liner each, but it is a statement about their
  indexes, so it wants a decision per database rather than a loop. Note the
  interaction with deploys: `switch-gui.service` can start before
  `postgresql-setup.service` has created its role, so its first start exits with
  `role "switch_gui" does not exist` and the never-give-up policy brings it up five
  seconds later — seen live, and correct behaviour, not a fault.
- **switch-gui: reworked onto a Postgres shadow copy + a write queue + a Dioxus SPA
  (2026-10-13) — built and deployed.** The page used to read the switches
  inside the request and keep its cache and audit log in memory; it now reads a
  shadow copy that a background sync maintains in Postgres and writes through a
  queue in the same database (see the switch-gui section above and
  `switch-gui/README.md`, "Where the page's data comes from"). What that means for
  whoever deploys it next:
  1. **It needs the database to start at all.** `switch_gui` (role + DB) is created
     by `machines/services1/services/postgres.nix` — **done, and deployed: reading all
     three switches as Manager since 2026-10-13 14:18**. The DSN is the unix socket, so
     there is no new secret to add and `switch_gui_env_file` keeps its four keys.
  2. The old in-memory audit log is gone and there is nothing to migrate; the first
     read of each switch seeds its row, so the pages say "not read yet" until the
     sync has been round (Refresh forces one).
  3. `--ttl` is gone (it was the page's cache); `--interval` is the sync's read
     interval and is 60 s in the unit, not the 30 s default, because every read is a
     login and every login is two authentik `login_failed` events until the RADIUS
     password below is fixed.
  4. Verified so far: 108 crate tests + 7 dto tests, the wasm bundle builds with the
     pinned wasm-bindgen 0.2.128, `nix flake check` and the system build are clean,
     and the new binary was run against a **live Postgres on services1** (in a scratch
     schema, `--no-auth`, loopback, a dead switch address so no device was touched):
     migrations create `switch`/`vlan`/`port`/`change`, `/api/state` and
     `/api/switch` return the read failure rather than a healthy-looking page,
     `/` and `/switch/<name>` serve the SPA, `/assets/switch_gui_web.js` and
     `/assets/switch_gui_web_bg.wasm` are 200, and `POST /api/intent` queues a row
     (`createdBy` recorded) while the refusals name the switch, the missing VLAN and
     the port that is not in the shadow copy. After the deploy the live service reads
     all three switches with `privilege = manager`, `warnings` empty on switch1/2 and
     the known RADIUS note on switch3, 26/46/1 named ports, and `change` empty (no
     writes have been queued through it). Three bugs came out of that work and are
     fixed. **`--no-auth` was parsed and validated but never reached `Shared`**, so
     every request got the "sign-in is not configured" 401 (it is now
     `Shared::no_auth` and the audit line reads `local`). **The default DSN was
     libpq's socket form**, which sqlx rejects with `error with configuration: empty
     host` — the socket has to be the percent-encoded host
     (`postgresql://switch_gui@%2Frun%2Fpostgresql/switch_gui`), and
     `the_default_dsn_is_one_sqlx_can_parse` now asserts both halves. And the rewrite
     had dropped the **"the enable credentials default to the login pair"** fallback,
     so every session sat in the Operator context, `show running-config` was refused,
     and the port names vanished (switch1: 4 named ports instead of 26) — restored, with
     `enable_defaults_to_the_login_account`. That default has now been lost **twice**,
     which is why `main::enable_creds` records that it is deliberately *not* gated on
     `--read-only`: the read needs Manager too. Both of those "not verified"s are
     now verified, and both turned out to be broken — see
     "switch-gui: three queue/shadow-copy bugs, and a browser suite to keep them fixed".
- **switch-gui: deployed, and serving all three switches** — the code, the
  service and the secret are all in place (`systemd.services.switch-gui` is live
  on services1, 100 unit tests pass, and it has been run read-only against the
  live devices: switch1 10 VLANs / 50 ports, switch2 6 VLANs / 49 ports, switch3
  6 VLANs / 24 ports). Done already:
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
  2. ~~`just dns-sync-sync` after deploying~~ — done for `switch.int`; the vhost is
     picked up from `services.nginx.virtualHosts` automatically. The per-switch
     names `switch1.int` / `switch2.int` / `switch3.int` are **not** dnsmasq
     records, so the gatus "Network Switches" pings now use the addresses from
     `common/hosts.nix` rather than those names. If the names are wanted, add them
     to the router's host overrides (or drop them and keep the IP pings).
  3. `switch3` answers slowly: a login there can take 15-25 s because RADIUS is
     tried first and the retry ladder is ~5 s per attempt, so the login reads use a
     separate timeout (`SWITCH_GUI_LOGIN_TIMEOUT`, default 45 s) rather than the 20 s
     command timeout — at 20 s the read times out before the prompt arrives. Its
     login also needs the `Press any key to continue` gate answered first (see the
     switch-gui section).
  Note the provider's `groups` scope reuses gocardless-dashboard's ScopeMapping
  row rather than owning one: creating a fresh child row through the ORM trips a
  unique-violation on `propertymapping.name` (see the authentik notes).
- **switch-gui: writes work (2026-10-13) and are enabled in the deployment** —
  the write path was never broken. `is_prompt` required a line to be *nothing but*
  a prompt, and this CLI paints the echoed command and the next prompt on the
  **same row** (`interface 23 name "x"switch1(config)#`), so every silent config
  command "timed out", the retry resent it, and the error claimed the console had
  never echoed it — while `show running-config` showed the change applied. The
  earlier suspect (telnet option negotiation) was wrong: a client answering
  `WONT`/`DONT` like the app works fine, and `nc` works because it answers
  nothing. Fixed in `is_prompt` (a prompt is a **suffix** of the line),
  `telnet::ReadFail` (a timed-out read keeps the partial buffer) and
  `Switch::command` (**no echo → resend once; echoed then silent → never
  resend**, because resending an applied config command applies it twice);
  `leave_config` exits until the prompt stops naming a context. Proven end to end
  through the HTTP API on a dead port: 0.8 s, `applied:`, written once, reverted.
  (**That proof did not exercise the queue** — it drove the console directly, which
  is why it passed while the queue swallowed every change for a day. The console
  half really was fixed; the claim half was not, see below.)
  `--read-only` is still in `main.rs` as the escape hatch. Still unverified: the
  VLAN-management commands (create/rename/delete, the per-VLAN `tagged`
  endpoint). Nothing in the app writes NVRAM, so a change lives in RAM until
  someone runs `write`; keep it that way unless asked (`write` is this firmware's
  command — `wr mem`/`write memory` is the other firmware's spelling, and
  `write ?` is already `Invalid input` here).
- **switch-gui: three queue/shadow-copy bugs, and a browser suite to keep them fixed
  (2026-10-13, all three deployed)** — the symptom was "I tick a VLAN, press Apply,
  the tick unsets itself and nothing happens". Three separate faults, each invisible
  from the layer above:
  1. **`port.tagged` was written as JSON strings and read as numbers.**
     `store_snapshot` reused `jsonb_array` (`["225"]`) while `number_list` parsed a
     `Vec<i64>` and fell back to empty, so **every tagged membership on every switch
     vanished from the page** while sitting correct in the database — and
     `commands::port_members`, which diffs against the shadow copy, could not see a
     membership to remove (unticking a VLAN queued nothing). `jsonb_int_array` now
     writes numbers, which is also what `vlan_membership`'s `tagged @> '[225]'::jsonb`
     containment needs (jsonb containment is exact about element type, so the
     "VLAN in use" delete guard never matched either). `number_list` tolerates both
     shapes; the column rewrites itself from the switch on every sync.
  2. **`claim()` had no `RETURNING`.** An `UPDATE` without it answers with a command
     tag and **no row**, so `fetch_optional` yielded `None`, `claim` returned "nothing
     queued", and the queue silently applied nothing — the row left `applying` with
     its attempt spent (observed live: `attempt` 277 of `max_attempts` 5, no error,
     no journal line). Its predicate also matched `applying`, so the row it had just
     claimed was claimable again on the next tick. `claim_sql()` is now a function the
     test asserts on (the old test rebuilt the query by hand and was missing the same
     `RETURNING`), it returns `RETURNING id`, and it takes only `pending`.
  3. **A pass that claimed changes and never reached the console recorded nothing.**
     `sync::run` returned early on a poisoned console lock or a failed connect,
     naming neither `applied` nor `failed`, and `record` only writes the rows it is
     named — so claimed changes sat `applying`, invisible, until `recover_stale`
     released them 15 min later. With `BATCH > 1` one dead console orphaned a whole
     batch, and `switch3`'s 15-25 s login makes a failed connect routine. Every
     no-console path now fails the batch (`run`'s `fail_all`, `claim_batch` on a
     mid-loop claim error, `pass` if the console task dies).
  What makes this testable is **`just switch-gui-test`** (`switch-gui/tests/`, see its
  README): a fake ProCurve console — a real Telnet server with an in-memory model, so
  the console protocol is exercised rather than stubbed — plus headless Chromium on
  `--no-auth`/loopback and a scratch Postgres schema per run. Each test asserts on the
  **device's** state, not just the page's, and `assertNothingStuck()` is a
  post-condition of every test. The page carries test hooks for it
  (`tr[data-port]`, `input[data-vlan]`, `button[data-apply]`), in the same spirit as
  filestore's `data-fs-*`. Also needed for the harness: a switch address may now name
  a port (`switch1=127.0.0.1:22323`, split at the last colon, IPv6-safe) because a
  test cannot bind port 23. 110 cargo tests + 10 browser tests pass.
- **switch-gui: "I tick a VLAN, press Apply, the row resets and nothing is queued" —
  reported live on switch1 port 2 (2026-10-13), and it was a correct refusal with no
  visible answer.** Port 2 is *untagged* in 225, so tagging it in 225 is a state the
  switch has no room for and `commands::port_members` refused it before queueing —
  right, and unit-tested since the start. The page was the bug: it offered the native
  VLAN's own checkbox as a legal tick, `Apply` dropped the row's overlay **before** the
  reply (so the row snapped back to the switch whatever the answer was), the only
  explanation was the banner at the top of a page with fifty port rows above the matrix,
  and a refusal wrote nothing at all to the journal. Four fixes: the native column is
  not tickable and moving the native VLAN off a ticked VLAN unticks it (the same rule,
  from the other side); a refused intent puts the overlay back, so the edit survives its
  own refusal; the banner is pinned (`position:sticky`) so the answer is read with the
  row that caused it; and `POST /api/intent` logs refusals (`refusal_line`, one line,
  control characters stripped because the intent was never validated).
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

  `switch-gui` treats the message as a **note, not a failure, when a prompt
  follows it** (`src/switch.rs::check_auth`): the session is usable, and refusing
  the connect on it would make the GUI unusable on a switch that is answering
  normally — which is what happened on `switch3` before it was a note. The note
  goes into the snapshot's warnings so the page shows it.

- **borg: the 2026-10-12 job run failed and nothing recorded why** (found 2026-10-13
  while upgrading the server). `services1-backup-srv-2026-10-12T00:00:01.failed` is in
  the repo, and the journal for that run shows only two "file changed while we backed it
  up" warnings (`/srv/unifi/logs/tasks.log`, `/srv/frigate/config/frigate.db-wal`) before
  `status=1/FAILURE`. borg exits 1 for a fatal error and 2 for warnings, so there was a
  real error that the journal did not capture; the runs on 10-11 and 10-13 are clean.
  Two things never done here: no `borg check` has been run against the repo (422 GB
  compressed, ~1.0M chunks), and nothing monitors the borg server —
  `common/monitoring-endpoints.nix` has no check for `nas2.int:3022`, so a dead
  `borg-server` container would only show up as a failed nightly job.
