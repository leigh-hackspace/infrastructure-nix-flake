# Services and tools — what each one is, and what to know before editing it

Moved out of `AGENTS.md` on 2026-10-14 when that file was reduced to rules.
`AGENTS.md` keeps the layout map and the cross-cutting rules (Rust only, shared
code in `crates/common-rs/`, the OIDC client contract, "never write NVRAM").
This file is the per-service detail: the API quirks, the ports, the flags, the
design decisions and the traps that only matter while you are in that one crate.

**Read the section for the crate you are about to touch.** Each one ends with the
deeper document that owns the long version, if there is one — this file is the
index, not the final word.

Machines: `services1` (10.3.1.20, the services box and TLS terminator),
`services2` (10.3.1.21, strata — the observability stack that used to live there
 moved to services1 on 2026-10-17), `aibox` (10.3.1.32, the network's build machine
 since 2026-10-17).
Every address is in `common/hosts.nix`.

---

## Table of contents

| Service | Lives in | Serves | Deeper detail |
| --- | --- | --- | --- |
| dns-sync | `crates/dns-sync/` | (no listener) | `docs/router-and-dns.md` |
| frigate-monitor | `crates/frigate-monitor/` | aibox:8090 | `machines/aibox/frigate-monitor.nix` |
| gocardless-dashboard | `crates/gocardless-dashboard/` | 127.0.0.1:8095 | `docs/authentik-oidc-notes.md` |
| network-status | `crates/network-status/` | 0.0.0.0:8091 | `machines/services1/network-status.nix` |
| filestore | `crates/filestore/` | 127.0.0.1:8096 | `docs/filestore-gotchas-2026-10-05.md` |
| switch-gui | `crates/switch-gui/` | 127.0.0.1:8097 | `crates/switch-gui/README.md` |
| status-dashboard | `crates/status-dashboard/` | both boxes | `common/status-dashboard.nix` |
| whisper-ws | `crates/whisper-ws/` | aibox:8083 | `crates/whisper-ws/README.md` |
| moonraker-exporter | `crates/moonraker-exporter/` | metrics | `docs/printer-servers.md` |
| strata | `machines/*/strata.nix` | services2:8080 → `strata.int.leighhack.org` (aibox's is parked) | `machines/aibox/STRATA.md`, `machines/services2/STRATA.md` |

---

## `crates/dns-sync/` — router + DigitalOcean DNS in step with the vhosts

Rust, **zero external crates**. Reads `services.nginx.virtualHosts` out of the
flake (the expected-name list is rendered to `/etc/dns-sync/expected-int-names`
by `machines/services1/dns-sync.nix`) and keeps the router's dnsmasq host
overrides and the DigitalOcean zone in step with it.

Run it with `just dns-sync-check` / `just dns-sync-sync` / `just dns-sync-prune`
(prune removes only the records the tool manages). After any vhost
add/rename/removal, `just dns-sync-sync` then `just dns-sync-check`.

What "correct" means, which names legitimately resolve elsewhere, the router's
config layout and the manual fallback: **`docs/router-and-dns.md`**.

## `crates/frigate-monitor/` — change detection on the Frigate stream

Rust, one external crate (`image`). Runs on **aibox**: snapshots the Frigate
`main_space` RTSP stream every 10 s and records an event when a static change
appears **and the area settles** — a region must differ from the background and
be completely still for N consecutive snapshots, so moving people are dismissed.
The `before` image is taken from just before the change began; recorded objects
are absorbed into the background so their later removal is caught too.

Events store before/after/diff JPEGs with the changed regions boxed, zoom crops
and a thumbnail, under `/var/lib/frigate-monitor/events/`.

Web UI is a Dioxus SPA (`frontend/`, wasm-built by the flake as `frontendDist`,
embedded by `build.rs` — no bundle committed) on aibox:8090, proxied by
services1 as `frigate-monitor.int.leighhack.org` (LAN-only). Packaging:
`machines/aibox/frigate-monitor.nix` and
`machines/services1/services/frigate-monitor.nix`.

## `crates/gocardless-dashboard/` — GoCardless sync + operator dashboard

Rust on services1. Two modes of one binary:

- **Sync** — pulls customers/mandates/subscriptions/payments/refunds/payouts
  from the GoCardless Pro API into local Postgres (`gocardless_dashboard`) every
  15 min.
- **Dashboard** — Dioxus SPA (same embedding pattern as above) on
  127.0.0.1:8095, LAN-only vhost `gocardless.int.leighhack.org`. Packaging in
  `machines/services1/services/gocardless-dashboard.nix`. Login is OIDC against
  authentik, `Infra` group gate (provider pk 33).

**The GoCardless Pro API** is at `https://api.gocardless.com` with **no** `/v1`
path prefix and requires the `GoCardless-Version: 2015-07-06` header. List
responses wrap items under the resource key (e.g. `customers`) and paginate with
`?after=<meta.cursors.after>`.

**It also owns the authentik `Members` group sync** (replaced the old Python
`gocardless-tools` job in 2026-10): the same binary run as a daily oneshot
(`gocardless-authentik-sync.timer`, 01:00, `--authentik-sync`) adds customers
with an active subscription whose name contains "membership" to the `Members`
group, removes no-longer-valid ones (users unknown to GoCardless are never
touched), keeps the `leighhack.org/gocardless-customer-id` attribute in step and
writes an audit log to `authentik_sync_log` (GUI: "Members sync" page; on demand
via `POST /api/authentik-sync`). The old script's bugs are fixed — it only read
the first 20 users and user creation always failed on username collision;
collisions are now logged as `skipped_existing_username`.

The "Members sync" page **re-fetches the audit log every 15 s and shows the
error on failure**. It used to be a single fire-and-forget fetch with the error
swallowed, so one failed request left it stuck on "loading…" until reload — when
you touch it, keep both halves: poll, and surface the failure.

## `crates/network-status/` — the router network dashboard

Served at `network-info.int.leighhack.org`. The Rust binary (`src/`, **zero
external crates**) ssh's to the router every 5 s, keeps a rolling history and
serves the JSON API (`/api/snapshot`, `/api/history`, `/api/config`) plus static
files via `--static-dir`.

It listens on **0.0.0.0:8091**, not loopback: Prometheus scrapes its `/metrics`
(`machines/services1/services/observability.nix`) and the bind used to have to be
wide because the scraper was on services2.  It is on this box now, so the wide
bind is a habit rather than a requirement — access control is the vhost ACL, not
the bind, and the JSON API has always been unauthenticated.

The frontend is **SolidJS + TypeScript** (`frontend/`, esbuild +
`esbuild-plugin-solid`, no router/state/chart libraries) and is the **one SPA
whose `dist/` is committed** — Nix builds are offline and the npm deps are not
nixpkgs-cached, so the flake copies `frontend/dist/` into the store rather than
building it (`machines/services1/network-status.nix`). **After editing the SPA
run `npm install && npm run build` in `frontend/` and commit the result** (or
`just network-status-frontend`). See `crates/network-status/frontend/README.md`.

The unit still runs as `User=leigh-admin`, which is a live follow-up —
`docs/follow-ups.md`.

## `crates/filestore/` — file browser over the NAS `filestore` share

Rust (axum + reqwest) on services1. JSON API (`/api/list`, `/api/search`,
`/api/mkdir|create|rename|copy|delete|upload|download|zip|preview`,
`/api/whoami`) on 127.0.0.1:8096 plus a Dioxus SPA at
`filestore.int.leighhack.org` (LAN-only vhost,
`machines/services1/services/filestore.nix`; it replaced the old apps1:8001
deployment). OIDC via authentik, `Infra` gate (provider pk 34, app slug
`filestore`); client id/secret are `FILESTORE_OIDC_CLIENT_ID` /
`FILESTORE_OIDC_CLIENT_SECRET` in the shared env-file secret.

**Escape hatches, both testing-only, both refused in production:**
`--dev-user <name>` (pretend to be a user) is never passed in production;
`--no-auth` (treat every request as a local session) is **only honoured on a
loopback bind** — `main.rs` exits if it is combined with a non-loopback `--bind`,
using `common_service::is_loopback`, which parses the host as an `IpAddr` rather
than string-matching `127.` (the bug that made that gate a no-op is in
`docs/work-log-2026-10.md`). `frontend/dist/` and `target/` are git-ignored.

**Preview support is one table**: `crates/common-rs/preview` maps extension →
what a browser can render itself (images incl. SVG, video, audio, PDF, text).
There is no server-side converter, so `image/tiff`, `image/heic`, AVI and Office
documents are deliberately not previewable. Media is streamed raw with `Range`
support (a seek does not re-download the file); text is truncated JSON. Media
responses carry `nosniff` and `default-src 'none'` CSP: an `<img>` cannot run
script but the same URL opened as a document can — that is the SVG case.
`common_preview::thumbnailable` is the shared list of what the *decoder* can
rasterise, because `Kind::Image` is broader (SVG is XML, AVIF needs `ravif`);
the API and the SPA read the same list so they cannot disagree.

**Thumbnails** (`crates/filestore/src/thumb.rs`) are generated on demand and cached in
temporary
storage (`--thumb-cache`, `/run/filestore-thumbs` on services1 — tmpfs, gone on
reboot), resized to `--thumb-max-side` (128 px) and bounded by
`--thumb-cache-max` (256M, oldest pruned). Freshness is structural, not
invalidation: the cache key hashes the file's identity (`mtime + ctime + size +
inode`), so any change is a different entry and a stale thumbnail cannot be
served. The same identity is the `v` in the URL the SPA builds — which is what
keys the *browser* cache — and the API reports it as `Entry.fingerprint` so the
SPA does not recompute it; the server always rehashes from the current stat, so a
stale `v` regenerates. A file the decoder cannot read gets a cached failure
marker (not re-decoded every render) and the row falls back to the emoji.

**Interaction rules that were expensive:** while a preview is open, left/right
step through the previewable files in the folder (clamped, not wrapped) and the
row selection follows; a bare letter type-aheads to the entry starting with it,
cycling on repeat presses. The current folder is mirrored into the URL hash with
`replaceState` because the app keeps its **own** back/forward history
(`hist`/`hidx`, Backspace) — the browser must not build a second one — and a hard
refresh lands back in the folder. During a drag, the directory row under the
pointer is marked (`data-fs-drop`: open-folder icon, dashed outline, `move here`
/ `copy here` badge, `upload here` for a drag from outside the page) and dragged
rows are dimmed (`data-fs-drag`); the hover is decided from the real event in
`frontend/src/js.rs` (`ensure_drag_hover`) rather than a row handler, because
`relatedTarget` is what tells a real leave apart from crossing to a child of the
same row (a row-level `dragleave` cleared the highlight on every child crossing),
and `dragend` is the only event that always runs, so it is what ends the drag. A
file row is never a target and a folder is never a target for itself. The grid is
infinitely scrollable — **no "load more" button**; the `onscroll` handler appends
the next page when the bottom is within ~600px.

**Path containment:** `Store::resolve` rejects any `..` component and the root is
canonicalised at startup, but symlinks **inside** the share are followed wherever
they point (the NAS uses them). The guarantee is "no traversal *from the API*",
not "nothing outside the share is reachable" — see `src/fsutil.rs`.

**Read before editing the SPA:**
`docs/filestore-gotchas-2026-10-05.md` (the dioxus 0.6 → pinned 0.7.10 port,
keyboard selection, context-menu clamping, drag-out to the OS, upload body
limits, sops editing) and `crates/filestore/tests/README.md` (the Playwright
hooks the SPA carries — `data-fs-name`, `#fs-menu`, `#fs-modal`, … — and the bugs
each one caught). The suite is `just filestore-test`.

## `crates/switch-gui/` — the three ProCurve switches from a browser

Rust. Manages `switch1` (J9280A 2510G-48, `Y.11.44`, 10.3.1.2), `switch2` (same
model, 10.3.1.3) and `switch3` (J4903A 2824, `I.10.107`, 10.3.1.4) — all three in
`common/hosts.nix` under those names, which are also each device's own `System
Name` and the names the GUI's URLs use (`/switch/switch2`). The switches speak
**Telnet only**, so the tool ships its own minimal Telnet client (`src/telnet.rs`)
and parses the CLI's fixed-width tables (`src/parse.rs`), merging the six `show`
commands that split a port's name, link state and VLAN membership between them.

**Nothing reads a switch while serving a request.** A background sync
(`src/sync.rs`) re-reads each switch into a **shadow copy in Postgres** and drains
a **queue** of wanted changes in the same database, which is also the audit trail
(rows with Retry / Discard / Revert — `Discard` only ever deletes a row that has
not been written). The UI is a Dioxus SPA over a JSON API (`/api/state`,
`/api/switch`, `/api/pending`, `/api/changes`, `/api/intent`,
`/api/change/{retry,forget,revert}`, `/api/export`); wire types are the shared
`switch-gui/dto` crate. Login is OIDC (`common-rs/oidc`, `Infra` gate, provider
pk 35) — the hand-written `src/oidc.rs` + `curl` is gone: the pool and the query builder
were already bringing tokio, so the zero-dependency rule stopped buying anything there. Database `switch_gui` over the **unix socket**, which `CONFIG.PG_AUTH`
trusts for local users — the unit needs no database credential and stays a
`DynamicUser`. LAN-only vhost `switch.int.leighhack.org` → 127.0.0.1:8097;
packaging in `machines/services1/services/switch-gui.nix` (switch list reaches
the unit as `SWITCH_GUI_SWITCHES` from `common/hosts.nix`; `--interval` is 60 s
there rather than the 30 s default because each read is a login — see
`docs/procurve-switches.md`).

**Port names come only from `show running-config`.** The `Name` column of
`show vlan <id>` is blank for every dynamically-learned member (all fifty ports on
switch1) and `show port name` does not exist on this firmware. That command is
also the only read whose output carries secrets, so it goes through
`Switch::secret_command` (failures never quote its output) and `parse::port_names`
returns names and nothing else.

**The VLAN matrix is the only place membership is edited.** One row per port, one
vertical column per VLAN, the untagged VLAN a dropdown. The form sends the whole
wanted row and `commands::port_members` turns it into a diff against the shadow
copy (removals, then the native-VLAN move, then additions: a port cannot be tagged
and untagged on one VLAN at once). The row is marked changed until Apply — the edit
is an overlay over the polled rows, so the poll keeps running underneath it. A
trunk member **or** trunk group is locked: `dto::Port::membership_editable` is the
one rule and it checks `trunk_group` (set on members) *and* the id (the group's own
row has no `trunk_group`), because the switch refuses the first and this tool has
never exercised the second. A port is never tagged in its own native VLAN, and
that rule is kept true **by the form** rather than only refused by the server: the
column of the row's native VLAN is not tickable (a tick already there can always be
removed) and moving the native VLAN off a ticked VLAN unticks it.
`commands::port_members` still refuses the impossible state — the form just never
asks for it, **because a refusal the operator cannot see looks exactly like a dead
button.**

The Telnet client, the parsers and the command generator are still **crate-free on
purpose**; what took crates is the other half (sqlx + sea-query + tokio + reqwest,
and `common-rs/{json,web,oidc,pg,build-spa}`).

**This is the crate with the most accumulated lore — read its README before
touching it:** `crates/switch-gui/README.md` covers the two firmwares ("Two
firmwares, one parser"), the CLI's traps (LF-only line endings, the banner
swallowing input, `-- MORE --` paging with `NAWS` ignored, ANSI screen painting,
trunk members having no rows), `enable`/config-mode privilege, the write path and
what was wrong with it, and "Can't reach RADIUS server". The device-level facts
and the RADIUS root cause are in `docs/procurve-switches.md`; the browser suite
(`just switch-gui-test`, a real fake-console Telnet server) is
`crates/switch-gui/tests/README.md`.

## `machines/*/strata.nix` + `strata-package.nix` — the two Strata engines

Strata (`github.com/Niko1221/Strata`) is not a crate and not llama.cpp: it is its
own ggml-based engine for Qwen3.8-Flash-Next, run as `strata.service` on services2.
It is not in the monorepo because it is an upstream program packaged per machine,
and the two builds differ only in backend:

| box | model | backend | source | detail |
| --- | --- | --- | --- | --- |
| aibox (parked 2026-10-17) | full 125B, IQ2_XS | **HIP/gfx1030** (Radeon iGPU) | `.#strata` | `machines/aibox/STRATA.md` |
| services2 | **Coder** IQ1_M | **CUDA/sm_86** (RTX 3060) | `.#strata-cuda` | `machines/services2/STRATA.md` |

The aibox instance was disabled when that box became the network's build machine
(2026-10-17, `common/build-farm.nix`): the IQ2_XS expert arena is ~33 GiB, which is
the memory a build needs. Its module, measurements and `.#strata` package are kept
so it can be brought back; the model data is outside the store and is not removed
by the flake.

services2 takes its upstream source from the flake's **`strata` input, tracked on
`main`** (`nix flake update strata`); aibox's package pins its own
`fetchFromGitHub` (it was the older HIP build, kept with the parked service). Each
`strata-package.nix` pins the vendored llama.cpp revision from `setup.py`'s
`LLAMA_CPP_COMMIT`. Model data (GGUF, pack, MTP runtime) is deliberately outside
the store and prepared once by hand — the units have `ConditionPathExists` on the
pack index so they never crash-loop before that. **Read the box's `STRATA.md`
before changing engine flags**: the knobs were measured on different hardware and
do not transfer.

services1 terminates TLS for services2's engine as
`https://strata.int.leighhack.org` (`machines/services1/services/strata-proxy.nix`,
LAN-only behind the `LOCAL_NETWORK` ACL). **The vhost name and the engine's
`allowed_hosts` are one fact in two files**: upstream's server refuses any `Host`
it does not answer to (DNS-rebinding protection) and nginx forwards the incoming
header upstream, so a vhost added without the name in
`machines/services2/strata.nix` answers 403 to every request.

## `crates/status-dashboard/` and `crates/moonraker-exporter/`

`status-dashboard` is the house style for a small Rust service: shared module
`common/status-dashboard.nix`, wired per machine
(`machines/services1/services/status.nix`,
`machines/aibox/status-dashboard.nix`), run on both boxes and reverse-proxied by
services1 as `status.*` / `aibox.status.*`. `moonraker-exporter` polls the two
print servers' APIs; see `machines/services1/services/printer-monitoring.nix` and
`docs/printer-servers.md`.

## `crates/whisper-ws/` — OpenAI Realtime in front of whisper.cpp

A stdlib-only Rust bridge that speaks the OpenAI Realtime transcription subset on
`ws://10.3.1.32:8083/v1/realtime` and forwards each committed audio chunk to
whisper-server as a WAV; whisper.cpp's own server has no WebSocket. Runs on aibox
as `whisper-ws.service` behind `whisper-server.service` (both units,
`machines/aibox/whisper.nix` — including the note that the GPU `--device 1` was
pinned to was removed on 2026-09-02, so `whisper-server` needs a repin before
either comes up). Its status page is a Solid.js/Vite app embedded in the binary,
which is why this one crate is packaged by `crates/whisper-ws/package.nix` rather
than by crane: `docs/building-rust-code.md`, "the fifth non-member".

## `crates/common-rs/` — where new shared code goes

Plain path dependencies (`common-json = { path = "../common-rs/json" }`), no
symlinks:

| Crate | What it owns | Used by |
| --- | --- | --- |
| `json` | hand-rolled parser **and** the string/error-object emitters the hand-built API bodies use | dns-sync, moonraker-exporter, status-dashboard, network-status, frigate-monitor, switch-gui |
| `oidc` | authentik login + `Infra` group gate | filestore, gocardless-dashboard, switch-gui |
| `web` | HTTP/1.1 plumbing for the `tiny_http` services: request head, query/cookie parsing, responses | the tiny services |
| `pg` | the pool, the schema runner, the sea-query→sqlx glue, the row readers | gocardless-dashboard, switch-gui |
| `service` | startup plumbing that is not the service: flag reader, `EnvironmentFile` parser, the loopback test that gates every `--no-auth` | every service |
| `preview` | the filestore preview table, read by **both** the API and the SPA | filestore |
| `build-spa` | the `build.rs` that embeds a Dioxus bundle | all four SPA apps |

**New shared code goes here, not into a second copy in the second crate.**
`service` and `json`'s emitters exist because both had already drifted between
three callers; the 2026-10-13 consolidation (`docs/work-log-2026-10.md`) is what
found the `--no-auth` loopback bug while deduplicating them.
