# Services and tools — what each one is, and what to know before editing it

Moved out of `AGENTS.md` on 2026-10-14 when that file was reduced to rules.
`AGENTS.md` keeps the layout map and the cross-cutting rules (Rust only, shared
code in `crates/common-rs/`, the OIDC client contract, "never write NVRAM").
This file is the per-service detail: the API quirks, the ports, the flags, the
design decisions and the traps that only matter while you are in that one crate.

**Read the section for the crate you are about to touch.** Each one ends with the
deeper document that owns the long version, if there is one — this file is the
index, not the final word.

Machines: `services1` (10.3.1.20, the services box and an **origin** — it holds no
TLS certificate and no public name), `services2` (10.3.1.21, strata — the
observability stack that used to live there moved to services1 on 2026-10-17),
`services3` (10.3.1.22 — **wired on VLAN 225 since 2026-11-25**, when it came off the
bench's Wi-Fi dongle and its 10.3.2.213 lease and joined the other services boxes; llama
.cpp/Ternary-Bonsai-2 on a GTX 1080 Ti since 2026-11-12, Strata's Coder pack since
2026-11-21), `machines/services3/README.md` owning both halves),
`aibox` (10.3.1.32, the Strata HIP engine and 59 GiB of RAM — the network's build machine
from 2026-10-17 to 2026-11-23), and `gw`
(10.3.1.1, the router and **front door**: TLS, the certificate, `nginx-sso`,
`anubis`, `headscale`/`headplane` and every vhost).
Every address is in `common/hosts.nix`.

---

## Table of contents

| Service | Lives in | Serves | Deeper detail |
| --- | --- | --- | --- |
| frigate-monitor | `crates/frigate-monitor/` | aibox:8090 | `machines/aibox/frigate-monitor.nix` |
| gocardless-dashboard | `crates/gocardless-dashboard/` | 127.0.0.1:8095 | `docs/authentik-oidc-notes.md` |
| network-status | `crates/network-status/` | gw:8091 (10.3.1.1) | `machines/gw/network-status.nix` |
| filestore | `crates/filestore/` | 127.0.0.1:8096 | `docs/filestore-gotchas-2026-10-05.md` |
| switch-gui | `crates/switch-gui/` | 127.0.0.1:8097 | `crates/switch-gui/README.md` |
| status-dashboard | `crates/status-dashboard/` | both boxes | `common/status-dashboard.nix` |
| whisper-ws | `crates/whisper-ws/` | aibox:8083 | `crates/whisper-ws/README.md` |
| moonraker-exporter | `crates/moonraker-exporter/` | metrics | `docs/printer-servers.md` |
| hackspace-mcp + slack-agent (the Slack bot) | `crates/hackspace-mcp/`, `crates/slack-agent/`, `machines/services1/services/slack-agent.nix` | 127.0.0.1:8191 (MCP), 127.0.0.1:8192 (health); Slack over an **outbound** socket, no inbound port | `docs/slack-agent-2026-11-16.md` (design + the 2026-11-16 build receipt) |
| voice tier: gateway + camera satellite + wake engine | `crates/voice-assistant/`, `crates/voice-wake/`, `machines/services1/services/voice-assistant.nix` | 127.0.0.1:8193 (gateway) and 127.0.0.1:8195 (wake, reachable **only** through the gateway's `/v1/wake/*` proxy) → `voice.int.leighhack.org`; `voice-satellite` listens on nothing and writes a status file the gateway serves; the answer path is `slack-agent listen` on 127.0.0.1:8194 | `docs/voice-assistant-2026-11-16.md` (the design and the receipts), `crates/voice-assistant/README.md` (the pipeline's traps), `docs/esp32-voice-satellites.md` (the wire contract for any new microphone) |
| MQTT broker (mosquitto) | `machines/services1/services/mqtt.nix` | services1:1883 (no HTTP, no 8883) | `docs/mqtt-int-legacy-answer-2026-11-02.md` |
| Frigate (NVR) | `machines/services1/services/frigate.nix` (podman; the **camera config is off-flake**, `/srv/frigate/config/config.yaml`) | services1:5000 insecure, :8971 secure, :1984 go2rtc, :8554-5 | `docs/frigate-false-positives-2026-11-12.md` (detection quality, detectors, and why there is no GPU offload) |
| strata | `machines/*/strata.nix` (services3 also `strata-package.nix`, CUDA 12.8 / sm_61) | services2:8080 → `strata.int.leighhack.org` (LAN, key injected) and `strata.leighhack.org` (public, API key); aibox:8080 → `strata2.int.leighhack.org`; services3:8080 → `strata3.int.leighhack.org` (both LAN, key injected) | `machines/aibox/STRATA.md`, `machines/services2/STRATA.md`, `machines/services3/README.md` |
| Home Assistant | `machines/services1/services/home-assistant.nix` (podman, **host network**, image **pinned**; state is `/srv/homeassistant`, which the flake only mounts) | `127.0.0.1:8123` here → `ha.int.leighhack.org` via the door; `10.3.1.30:8123` is a logging forwarder, not the app | `docs/home-assistant-on-services1-2026-11-16.md` (the move, the client map, the Zigbee-radio trap, `just ha-legacy-watch`) |
| The space calendar (Home Assistant `local_calendar`) | `/srv/homeassistant/.storage/local_calendar.*.ics` — **runtime state the flake only mounts**; read by the Space API's `/events` and `/events.ics`, and by `hackspace-mcp`'s `space_events` | the three entities below; nothing serves it directly | `docs/space-calendar.md` (adding an event, and why the file is not where you write one) |
| Space API (the hackspace's public status feed) | `machines/services1/services/hackspace-api.nix` (podman; **its credential is runtime state at `/var/lib/hackspace-api/env`, deliberately not sops** — read the module header before changing that) | `127.0.0.1:8081` here → `api.int.leighhack.org` / `api.leighhack.org` (public, `acl = "none"`) | `docs/home-assistant-on-services1-2026-11-16.md` (what it is, the 40 h of external pollers, why the env file is not in the store) |
| ups-watchdog | `crates/ups-watchdog/` | no port: it is a oneshot, and it calls out (ssh `wall`, ssh `poweroff`) | `docs/ups-and-fleet-shutdown.md`, `machines/gw/ups.nix` |

---

## Adding a service module to a machine — the import list is explicit, and silence is not success

**A new file under a machine's `services/` directory does nothing until that directory's
`default.nix` names it.** `machines/services1/services/default.nix`'s `imports` is a hand-written
list (that is also how rule 23's `# PARKED` lines work), so a module dropped beside thirty-odd
imported files is **never evaluated**: no error, no warning, no changed derivation for the thing
you added, and `just check` stays green because nothing was ever asked to read the file.

Measured 2026-11-16 with Home Assistant: `machines/services1/services/home-assistant.nix` was
written, formatted, `git add`ed and passed `just check`; `nix build` of services1's toplevel then
**did not contain `etc/systemd/system/podman-home-assistant.service`**. The list is the fix.

So the test of "did my new service get added" is never `ls`, and never `just check` — it is the
rendered artefact out of a real build:

```bash
out=$(nix build ".#nixosConfigurations.services1.config.system.build.toplevel" --no-link --print-out-paths)
ls "$out/etc/systemd/system/" | grep -F <unit-name>          # absent = the module is not imported
```

…which is rule 10's discipline (read the rendered unit) doing double duty as rule 5's (an
untracked/unwired file is invisible to the flake). The general form: **`grep` the build output,
not the source tree, for what a machine actually runs** — the same reason `just check` cannot
approve a parked module, and the reason `machines/gw/lib/check-network-tables.nix` is written
against the tables rather than against a comment saying what they contain.

---

## Removed: `crates/dns-sync/` (deleted 2026-10-22)

It kept the router's dnsmasq host overrides and the DigitalOcean zone in step with
`services.nginx.virtualHosts`, by ssh'ing from services1 into the router and appending
lines to a mutable `/var/lib/dnsmasq/names.hosts` whenever someone ran
`just dns-sync-sync`. The names are `common/int-dns.nix` now, rendered by
`machines/gw/dns.nix` into a store path that dnsmasq is restarted for; `machines/gw/int-dns.nix`
refuses to evaluate `gw` while the table and the front door's vhost list disagree, and
`machines/services1/int-dns.nix` checks that the origin box serves exactly the table's
`origin` rows. **`docs/router-and-dns.md`** owns what "correct" means, which
names answer somewhere other than the front door, and what is left in DigitalOcean with
nothing maintaining it.

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
embedded by `build.rs` — no bundle committed) on aibox:8090, proxied by the front
door (`gw`) as `frigate-monitor.int.leighhack.org` (LAN-only). Packaging:
`machines/aibox/frigate-monitor.nix`; the vhost is a row in `common/front-door.nix`,
not a services1 module.

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

Served at `network-info.int.leighhack.org`, and **hosted on `gw` (10.3.1.1)** since
2026-10-21. The Rust binary (`src/`, **zero external crates**) runs as its own system
account with `--local`, reads the kernel's own statistics every 5 s, keeps a rolling
history and serves the JSON API (`/api/snapshot`, `/api/history`, `/api/config`,
`/api/connections`, `/api/leases`), `/metrics` and the static SPA (`--static-dir`).

It ran against the retired OPNsense box until 2026-10-21 and spoke its language:
`root`, `netstat -ibn`, `pfctl`, `ifconfig -a`, `top -b`, `sysctl`. The probe is now
local and reads `/proc/net/dev` (interfaces), `/proc/stat` + `/proc/loadavg` +
`/proc/uptime` (CPU/load), `/proc/meminfo` (memory), `/proc/net/snmp` (TCP
retransmits), `/proc/net/tcp*` (socket count),
`/proc/sys/net/netfilter/nf_conntrack_count` (the conntrack table) and
`/sys/class/net/*/carrier` (link state) — no sudo, no ssh, no key, because all of it
is world-readable. The WAN interface is **`ppp0`**, the AAISP L2TP tunnel
(`machines/gw/constants.nix`), not the hub handoff NIC; ppp0 only exists while the
tunnel does, so its absence is the WAN-down signal. (The binary still has the old
`--router user@host --ssh-key … --known-hosts …` mode for probing another box; the
unit does not use it.)

It listens on **10.3.1.1:8091**, not `0.0.0.0`: its only clients are the router's own
nginx, which serves `network-info.int.leighhack.org` from loopback since the front-door
cutover (`common/front-door.nix`, `docs/router-front-door-migration.md`), and Prometheus
on services1, which scrapes `/metrics`
(`machines/services1/services/observability.nix`) — and `machines/gw/firewall.nix`
opens the port to `services1` alone. The JSON API is unauthenticated by design; the
vhost ACL is the access control, which is the sentence that carries weight now: see the
connection table below for what an unauthenticated read of this box's `/proc` exposes.

### The connection table (`/api/connections`)

Added 2026-10-25 because "what is downloading, from where, how fast, for how long" was
not answerable from the byte counters: those are per-second totals, and a 2 GB pull and
a 2 MB page look the same for the four seconds they are visible. It is a table of
**flows**, not sessions, because one large transfer is one tuple and that is the unit
the question is asked in.

- **Source: `/proc/net/nf_conntrack`, read locally, no root.** Each row already carries
  its own byte and packet totals, both directions, which nothing else in this design
  does: nft counters are per rule and the wire bytes are per second. Accounting must be
  on (`net.netfilter.nf_conntrack_acct=1`) and the kernel hands the file to exactly one
  group, so `machines/gw/conntrack-reader.nix` creates `conntrack-readers`, turns
  accounting on, gives the file that group in a root `ExecStartPre=+`, and defines the
  dashboard's service account with that group as its **primary** group.
  **Read that module before changing any of it.** Its header carries four measurements
  made against the live file, and one of them is the reason the service is not a
  `DynamicUser`: `DynamicUser=yes` with `SupplementaryGroups=` is *denied* here (systemd
  261 gives a dynamic uid no supplementary groups), so the table reads empty and nothing
  says so — which is what the first version of this shipped, along with a comment
  claiming it had been tested. It had not.
- **Age is age, not kernel age.** Conntrack records "seen", not "started", and does not
  say when. A flow's `first_seen_us` is the first table in which *this service* saw the
  tuple, so the default filter — "older than 5 s" — means "this tuple has been in the
  kernel's table across at least two of our samples". It cannot date a connection that
  began between two samples, and a flow that ages out and is seen again is a new flow.
- **One tuple in, one row out.** `key` is the tuple, canonicalised so an origin and a
  reply tuple are the same flow; the counters go into *directions* (as/at the original
  tuple = the request side, as/at the reply tuple = the response side), which is what
  lets one row say "180 MB down, 40 kB up" for a download.
- **Internal/external comes from `common/vlans.nix`.** `--vlan` is derived from
  the VLAN table (`machines/gw/lib/network-tables.nix`'s `vlanPrefixes`: every VLAN's
  block, plus the /64 of every address the VLAN answers on or advertises, each labelled
  with its VLAN device name), so a device on VLAN 226 cannot be labelled "external"
  because someone added a VLAN and not a flag.
  `machines/gw/lib/check-network-tables.nix` checks the derived prefixes against the
  network's own ACL with the same containment relation the firewall uses, and asserts
  that the labelled list is the *same set* as the unlabelled one — the lease table
  groups by those labels, so a drift there would be one address described two ways.
  Names come from `/etc/dnsmasq-records.hosts` (static records, world-readable) and,
  since the lease-reader below, from dnsmasq's lease file as well.
- **Counters are 32-bit and wrap**, and the total is reported once it has wrapped — a
  5 GB flow reads 4.3 GB smaller. Rates are measured over a *flow's own* interval, not
  the global sample interval, because a flow seen every third sample has not been
  transferring at its average over the last five seconds.
- **A row whose `accounted` is false is invisible to rate and byte filters**, because
  there is nothing to compare — the alternative is that turning on accounting silently
  changes which rows the threshold shows. `totals.unaccounted` counts them and the SPA
  says so in the header line.
- **Query contract** (all of it optional, all defaults from the CLI flags):
  `min_age`/`min_bytes`/`min_bps` (thresholds), `q` (a substring of an address, name,
  or `:port`), `kind` (`any|internal|out|in|external`), `sort`
  (`down|up|total|age|rate|host`), `limit`. `totals` describes the **matched** flows,
  not the whole table, so the page's footer is the sum of what it shows. Host totals
  roll up every flow a host is one end of — *before* the `limit` is applied, because a
  host with twelve small flows is the point of the column. `down` is always
  peer→host, so an `in` row reads its bytes out of the original direction; that
  inversion is the single easiest thing to get backwards here (`flows/tests.rs` keeps it
  true).

The SPA's "Active connections" card is one 5 s poll of this endpoint with the filters
in the query string, same race-control as the history poll
(`crates/network-status/frontend/README.md`). `/metrics` carries four gauges of the
same table — `router_connections_tracked` (seen now or within `--conn-stale`),
`_active` (in the kernel's table at the last sample), `_long_running` (the
`--conn-min-age` subset, i.e. what the page shows by default: the series that
distinguishes "nothing this old" from "nothing at all") and `_unaccounted` (active
flows with no kernel counters, which is the number that decays to zero after the acct
sysctl goes on) — all under the binary's existing `router_` prefix rather than a
`network_status_` one, because gw also runs node_exporter and its conntrack collector
is scraped by the same Prometheus: two series answering nearly the same question with
different units on one dashboard is a finding nobody notices.

### The lease table (`/api/leases`)

Added 2026-10-25 with the connection table, and **live on 2026-10-26** once the
permission question was decided. It answers the other half of "what is on my network":
the connection table says which address is talking, dnsmasq's lease file says which
device that address belongs to, until when, and on which VLAN it was handed out.

- **Source: `/var/lib/dnsmasq/dnsmasq.leases`, format `<expiry-epoch> <mac> <ip>
  <hostname> <client-id>`.** It is `0640 root:root`, and handing it to the dashboard is
  a decision about who may see which device was seen doing what —
  **`machines/gw/lease-reader.nix` makes it, in its header, and is the module to read
  before changing any of this.** It reuses the `conntrack-readers` group
  `machines/gw/conntrack-reader.nix` already defines (one member: the dashboard's own
  account, which has no login, no shell and `ProtectSystem=strict`), keeps mode 0640, and
  re-applies the group from three places because dnsmasq owns and rewrites that file: a
  tmpfiles rule (`f` then `z`), `lease-reader.service` started by `lease-reader.path`
  watching `/var/lib/dnsmasq`, and an `ExecStartPre=+` on network-status so a boot never
  races the path unit. **What actually costs the group is dnsmasq starting, which chowns
  the file `root:root`** (measured 2026-10-31 on the same inode, so the rewrite is
  in-place and the earlier "a rename replaces the inode" note in this file was the wrong
  inference); the watch is on the *directory* so that a path replaced by anything else —
  which is what froze this table for 2 d 17 h — is caught too.
- **The endpoint reports the health of its own source.** `/api/leases` answers
  `source_mtime`, `source_age` and `stale`, where `stale` is `"all-expired"` when every
  row in the file has lapsed (a DHCP server that is serving always has one that has not),
  `"unwritten"` when the file is older than two lease lengths (dnsmasq rewrites it on
  every lease event), and `null` otherwise; `leases.rs::stale` is the only definition of
  it, the leases panel banners it, and `lease-reader-watch.timer` on `gw` fails on it
  rather than re-deriving it. This exists because a frozen table is not visibly frozen:
  for 2 d 17 h this endpoint answered `"total":56,"expired":56` truthfully while holding
  none of the network's 92 live leases, and an operator concluded a device was absent.
- **A lease's *length* is recovered, not read.** dnsmasq stores an absolute expiry and
  no TTL. Every `dhcp-range` `machines/gw/lib/network-tables.nix` generates is generated
  at `leaseTtl` (24 h), which reaches the binary as `--lease-ttl` from that same table,
  so a lease inside a block the router was told about (`--vlan`) has a true fraction left
  and is marked `known: true`; a tailnet or hand-written address gets the same arithmetic
  with `known: false` and a dimmed bar, because what it actually ran at is not knowable
  here. `issued` is `expiry - leaseTtl` for the same reason, and `null` when unknown.
- **dnsmasq's own rows are rows.** `ra-only`/`ra-param` write a release time with no MAC
  and no hostname; they are `generated: true`, have no bar and no `issued`, and are shown
  rather than hidden because the address *is* spoken for. A `*` in the MAC column is
  dnsmasq's marker, not a missing field.
- **Malformed lines are counted, not fatal.** dnsmasq's own header (`duid <hex>`) and a
  write caught mid-flight are `skipped`; the live file was measured with 49 lines of
  which one was the header. Both tables parse leases through the same
  `names::lease_line`, so the name column and the lease table cannot disagree about what
  a lease is.
- **Unreadable is 503 with the OS's reason, never an empty table** — "no DHCP clients"
  is a conclusion an operator would act on, and it would be wrong. Same rule as
  `--no-conns`: the endpoint says which of the three reasons applies (`--no-leases`, no
  `--leases-file`, or the read itself failing).
- **Query contract:** `q` (substring of name, address, MAC, client-id, VLAN or network),
  `vlan` (a VLAN device name), `net` (a prefix), `max_left` (keep leases expiring within
  that many seconds; negative keeps everything, lapsed rows included), `sort`
  (`expires|issued|name|vlan|addr`, `^` prefix reverses), `limit`. Counts (`expired`,
  `unknown_ttl`, `generated`, per-VLAN and per-network totals) are over **every** lease
  read, not over the filtered page: they answer "what does this router hold", which is
  the question `returned` already disambiguates for the other reading.

It lives in `machines/gw/network-status.nix`. That does mean the router's system
contains a Rust crate, and since 2026-11-23 that crate is compiled **on gw**, bounded:
`max-jobs = 2` of its 4 threads inside a 3/4.5 GiB cgroup on a 7 GiB box, so a
`just deploy gw` (or `just switch` on the box) costs that box minutes and the network
nothing — AGENTS.md rule 22. What still refuses there is the fleet's build gate.

The frontend is a **Dioxus (wasm) SPA** (`frontend/`, dioxus 0.7 + gloo-net + plain
`canvas` charts, no router/state/chart libraries). Its `dist/` is **git-ignored and built by
the flake** like the other four, and `build.rs` (`common-rs/build-spa`) embeds it in the
binary — the bundle is served as **bytes** (`common_web::Reply` carries `Vec<u8>`), because
one of the assets is a wasm binary and a bundle decoded as UTF-8 is either an error or a
mangled one. It used to be the repo's one exception: SolidJS + esbuild with a **committed**
`dist/`, because its npm deps were not nixpkgs-cached and Nix builds run offline. That ended
with the port on 2026-10-30; `just build-frontend network-status` is the rebuild now, and
`just network-status-test` is its suite. See `crates/network-status/frontend/README.md`.

It is **tabbed** (`Overview`, `Uplink`, `Connections`, `DHCP leases`, `Interfaces & issues`)
since 2026-10-26, because one column of cards holding two tables and a chart grid had
got long enough that finding a lease meant scrolling past everything else. The tabs are
by question, not by widget, and the active tab lives in the URL fragment (`#leases`) so a
reload keeps it and a link can point at one.

Two things the port changed about the tabs, both asserted by the suite:
`TabContent` renders only the selected panel — a chart in a hidden tab has no node to draw
on — so each draw effect reads the tab and redraws when it becomes visible, which is the
same guarantee the old always-mounted panels gave by construction; and the tabs are
`common_ui::tabs`, so the roles, `aria-selected` and the roving focus are the primitive's,
not written here. The arrow keys move the roving focus and no longer activate, so a
keyboard user activates with Enter/Space on the focused tab (`docs/component-library.md`,
trap 9). The other widgets it uses are the library's too: the "moving now" control is
`common_ui::checkbox`, the direction filter and the lease "show" filter are one
`common_ui::toggle_group` each, and a lease's countdown is `common_ui::progress` — which
means they ship **no CSS**, and the sheet in `frontend/index.html` styles them through what
they announce (`data-state`, `--progress-value`). That split is where this page went wrong
once: the port's rewritten sheet was keyed on class names the components do not emit, so
most of it applied to nothing, and every check still passed. Two checks at the end of the
suite now keep the markup and the sheet attached to each other (the stylesheet guard and the
painted check); `crates/network-status/tests/README.md` has why both are needed.

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
than string-matching `127.` — the gate used to end in `starts_with("127.")`, which accepted
`--bind 127.example.com` and made the flag's only guard a no-op. `frontend/dist/` and
`target/` are git-ignored.

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

**An applied batch is followed by `write memory`.** The switches keep
`running-config` in RAM and load `startup-config` at boot, so a change was undone by
the next reboot — which a mains blip on 2026-10-31 did to all three. The command is
opt-in in the code (`Config::save_command`, i.e. `SWITCH_GUI_SAVE_COMMAND`) and set
in `machines/services1/services/switch-gui.nix`; a failed save is reported on the
switch's row rather than swallowed, because "applied" and "will still be there
after a reboot" are different claims.

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

## `machines/*/strata.nix` + `strata-package.nix` — the three Strata engines

Strata (`github.com/Niko1221/Strata`) is not a crate and not llama.cpp: it is its
own ggml-based engine for Qwen3.8-Flash-Next, run as `strata.service` on services2.
It is not in the monorepo because it is an upstream program packaged per machine,
and the two builds differ only in backend:

| box | model | backend | source | detail |
| --- | --- | --- | --- | --- |
| aibox | full 125B, IQ2_XS | **HIP/gfx1030** (Radeon iGPU) + optional ROCm 10 runtime | `.#strata` | `machines/aibox/STRATA.md` |
| services2 | **Coder** IQ1_M | **CUDA 13.0/sm_86** (RTX 3060) | `.#strata-cuda` | `machines/services2/STRATA.md` |
| services3 | **Coder** IQ1_M | **CUDA 12.8/sm_61** (GTX 1080 Ti, Pascal, `-DSTRATA_EXPERIMENTAL_SM60=ON`) | `.#strata-cuda12` | `machines/services3/README.md` |

The third row is not a copy of the second: **CUDA 13 cannot compile for Pascal at
all** (`nvcc` fails CMake's compiler test with `compute_70`), so services3 builds
the CUDA 12 branch upstream ships for exactly these cards, one arch (`61`). That is
the whole reason `strata-package.nix` exists three times instead of once with
`cudaPackages` as an argument — and it is why `nix flake update strata` moves all
three engines in the same switch while the *toolkit* stays each box's own choice.

The aibox instance was parked 2026-10-17 when that box became the network's build
machine — the IQ2_XS expert arena is ~33 GiB, which is the
memory a build needs — and un-parked 2026-10-28. The trade did not change shape, only
sign: with the engine loaded aibox has ~10 GiB for builds, so a bump there competed with
the engine until 2026-11-23 killed it twice (`machines/aibox/STRATA.md`), which is what
ended the arrangement rather than another `systemctl stop strata` in a runbook: the box
builds for itself now at 4 jobs inside a capped cgroup and the engine's pages are
protected. Enabling the engine still clears `llama-server`'s `wantedBy`, which 502s
the `ai.*` vhosts. Model data is outside the store and was never removed by the
parking; that is the only reason the un-parking was a config change.

All three engines take their upstream source from the flake's **`strata` input,
tracked on `main`** (`nix flake update strata` moves all three — aibox's package
pinned its own
`fetchFromGitHub` until 2026-10-28 and drifted nine releases behind). Each
`strata-package.nix` pins the vendored llama.cpp revision from `setup.py`'s
`LLAMA_CPP_COMMIT`. Model data (GGUF, pack, MTP runtime) is deliberately outside
the store and prepared once by hand — the units have `ConditionPathExists` on the
pack index so they never crash-loop before that. **Read the box's `STRATA.md`
before changing engine flags**: the knobs were measured on different hardware and
do not transfer, and `machines/aibox/STRATA.md` is also the bump checklist (what to
re-verify in the tree — the ggml pin, `data/` hashes, the flag list, the arch gate —
before trusting a release note).

Because the engine's Python layer is *copied into the store by hand* in each
`strata-package.nix`, a `nix flake update strata` can move upstream's module
layout under it (0.1.46 renamed `apps/cli` → `strata_cli` and added
`strata_control`, which `serve/lifeline.py` imports at module scope) and the
service dies at import on boot. Both `strata-package.nix` files therefore import
the server's module graph at install time, so a copy list that lags upstream fails
the build rather than the boot (the aibox copy of that guard is what caught nothing
on 2026-10-28 and would have caught the crash-loop it was written for); the update
ritual — build on aibox, `nix copy --no-check-sigs` as root, switch, never confirm a
failed activation — is in `machines/services2/STRATA.md`.

`gw` terminates TLS for services2's engine as `https://strata.int.leighhack.org`
(LAN-only behind the `LOCAL_NETWORK` ACL) and as the public
`https://strata.leighhack.org`; both are rows in `common/front-door.nix`, and the
door proxies straight to `10.3.1.21:8080`. aibox's engine is the third name,
`https://strata2.int.leighhack.org` → `http://10.3.1.32:8080`, LAN-only and
`strataBearer`-injected. One engine, two gates: the public name is
open to the world and the engine's `api_key` (sops `strata_api_key`) is what makes
that safe, while the `.int` name injects that same key at nginx so LAN clients — and
the engine's own dashboard, which is nothing but `/v1/*` calls — stay keyless.
The key is **one secret for all three engines**, because `strataBearer` renders one
`proxy_set_header Authorization`: a per-engine key would 401 the door's path.
**A LAN vhost name and the engine's
`allowed_hosts` are one fact in two files**: upstream's server refuses any `Host`
it does not answer to (DNS-rebinding protection) and nginx forwards the incoming
header upstream, so a vhost added without the name in
`machines/services2/strata.nix` — or `machines/aibox/strata.nix`, whose name is
`strata2.int.leighhack.org` — answers 403 to every request.  That protection is
switched off by the API key, which is why the public name is *not* in the list:
remove the key and the public vhost 403s instead of serving an open engine.

## `machines/services3/strata.nix` + `strata-package.nix` — Strata (CUDA 12.8 / sm_61)

services3's engine, and the network's third Strata instance: the **Coder IQ1_M**
pack (the same file services2 serves) on a **GTX 1080 Ti** (GP102, Pascal, compute
capability 6.1) in a box with 40 GB of RAM and 11 GB of VRAM. It replaced that
box's llama.cpp on **2026-11-21** — one GPU, one engine — and
`machines/services3/llama.nix` is parked with its measurements intact.

Why it is a different build rather than an argument to services2's: CUDA 13
dropped Pascal and Volta, so `nvcc` cannot emit `sm_61`. The toolkit here is
**CUDA 12.8** (12.9 clashes with glibc >= 2.43 in the toolkit's math headers, and
this box runs glibc 2.44) and the tree is built with
`-DSTRATA_EXPERIMENTAL_SM60=ON` — upstream's opt-in path for GPUs below compute
capability 7.5, which is the community shape the GTX 10 series runs under.

| | |
| --- | --- |
| Listens | `10.3.1.22:8080` — **the LAN address, not loopback**, because the front door is another box and this one has no firewall (`machines/services3/vlan225.nix`, like every other services box: the network's filtering is the router's nftables), so the LAN ACL at the door is the gate. It used to answer from the Wi-Fi dongle's lease `10.3.2.213` on VLAN 226, which made the door's upstream a cross-block hop; since 2026-11-25 the origin and the door are in the same block |
| Served as | `strata3.int.leighhack.org` — a row in `common/front-door.nix` with the LAN ACL, TLS at `gw`, `proxyBuffering = false` for SSE, and `strataBearer = true` (it is a Strata engine, so it takes the one `strata_api_key` the door injects; the name is in the engine's `allowed_hosts`) |
| Paths | OpenAI-compatible (`/v1/chat/completions`, `/v1/messages`, `/v1/models`) plus `/health`, `/props`, `/metrics` and the engine's own dashboard at `/` (which is `/v1/*` calls, so it needs the door's injected key to work — same as services2's) |
| Data | `/srv/strata` on the **476 GB NVMe** (`nvme0n1`, one ext4 label `STRATA`, wiped 2026-11-21): 54.4 GB of GGUF shards, the prepared pack, the MTP draft. Outside the store, prepared once by hand; the unit has `ConditionPathExists` on the pack index |
| Sizing | experts' arena ~23.4 GiB resident in RAM (`--resident-experts`), ~5 GiB of dense weights + buffers in VRAM, `--expert-cache auto` (≈3,800 slots) and **32K context** — every +32K of context is ~0.44 GiB taken from the cache, and the cache is what decides whether an expert is a VRAM read or a RAM/SSD read |
| Card-specific | `STRATA_DENSE_F32=1` (consumer Pascal runs FP16 at 1/64 of FP32, so cuBLAS's FP16-operand GEMMs are the slowest thing on the card); sm_61's wide dequant, expert-mode 8 and interleaved matvec are on by default; the fused int8 prompt kernels are **not** (they need sm_80+) |
| Speed | **18.2 tok/s decode, 464–539 tok/s prefill** (1,830 → 77,612 tokens), measured 2026-11-21 on the live unit at **128K context** — a correct needle at 77,612 tokens in 152.8 s of prefill. 10,449 MiB of the card's 11,264 at load, **418 MiB free after a 77K request**, which is the engine's own `below 512 MiB` warning: this is the card's edge, and `--vram-reserve-mib` / `context = 65536` are the two one-line ways back. All of it, including why a 65-token prompt reads at 18 tok/s and must not be quoted, is `machines/services3/README.md` |

Four checks, in the order that answers something:

```bash
systemctl is-active strata                                 # and: journalctl -u strata -n 50
curl -sS -m 5 http://10.3.1.22:8080/health                # "ok" — not localhost: see Listens
sudo nvidia-smi --query-compute-apps=pid,used_memory       # the engine holding ~9-10 GiB of the 11,264
grep -E "experts|expert cache|chunk" /var/lib/strata/strata.log | tail -5   # what the load actually sized
```

The build-side facts — why CUDA 12.8 and not 13, what `STRATA_EXPERIMENTAL_SM60`
compiles in, why the arch is `61` on its own — are the header of
`machines/services3/strata-package.nix`; the flags, the VRAM budget and the hand-run
pack preparation are `machines/services3/strata.nix`. The box's own half (the NVMe,
the Wi-Fi uplink, what happened to the old llama.cpp) is
`machines/services3/README.md`.

## `crates/status-dashboard/` and `crates/moonraker-exporter/`

`status-dashboard` is the house style for a small Rust service: shared module
`common/status-dashboard.nix`, wired per machine
(`machines/services1/services/status.nix`,
`machines/aibox/status-dashboard.nix`), run on both boxes with services2's own copy, and
served by the front door on `gw` as `services1-status.int` / `services2-status.int` /
`aibox-status.int` — LAN names only, since the two public twins were deleted on
2026-11-16 with no DNS record behind them (`docs/follow-ups.md`). The `*.int` rows carry
`lanToken = true`, which is what lets `POST /api/restart` work on the LAN without an SSO
sign-in. `moonraker-exporter` polls the two
print servers' APIs; see `machines/services1/services/printer-monitoring.nix` and
`docs/printer-servers.md`.

## The voice tier — four units, one answer path, no secrets at all

```
  cam8 mic ──go2rtc restream──▶ voice-satellite ─┐
  browser mic (whisper.int) ─────────────────────┤
  any device that can POST ──────────────────────┤
                                                 ▼
                             voice-gateway :8193 (loopback, no auth)
                               ├ whisper.cpp on aibox :8082      speech → text
                               ├ slack-agent listen :8194        text → answer
                               └ piper en_GB-alba-medium         answer → speech
                             voice-wake :8195 (loopback, no auth) — detection only,
                               and reachable off-box only as the gateway's /v1/wake/*
```

Four things an agent changing any part of this needs before it changes anything:

1. **The answers are not this tier's.** They come from `hackspace-mcp`'s ten tools through
   `slack-agent listen` — the same loop the Slack bot runs, minus Slack. A voice feature that
   computes its own answer (about temperature, opening hours, the calendar) is a second brain
   in public, which is the fault `docs/slack-agent-2026-11-16.md` §16 is about.
2. **`listen` answers one turn at a time** and refuses a second caller with 429
   (`crates/slack-agent/src/listen.rs`); the gateway gives up at 150 s
   (`crates/voice-assistant/src/agent.rs`). That serial lock, not CPU, is what a fleet of
   microphones hits first.
3. **No secret is anywhere near it** — no `EnvironmentFile`, no sops entry. Which is why the
   two unauthenticated sockets **refuse to bind off loopback**, and why the only way in from
   another machine is the front door's `voice.int.leighhack.org` row. The reach is the whole
   security model; read that row and `CONFIG.ROUTER_ONLY` before widening anything.
4. **The device boundary is enforced, not intended.** The turn logic drives a
   `crate::source::Source` (frames in) and a `crate::sink::Sink` (noise out), and every host
   name, URL, vendor and child process lives in `crates/voice-assistant/src/drivers/`.
   `common/check-platform-boundary.nix` substring-checks both halves — that the core has no
   device facts, and that the adapters still do — because a boundary nobody checks is a
   suggestion, and adding a microphone is supposed to cost a driver, not a refactor
   (`docs/esp32-voice-satellites.md`).
5. **The measured budget** (`docs/voice-assistant-2026-11-16.md`): whisper **2.7 s** for 3 s of
   audio, on **CPU** (strata owns the iGPU); piper **2.5 s** for 31 words, because a fresh
   process reloads the voice every request; a one-tool spoken answer is **~13 s** end to end;
   and the deterministic fast path answers time / open-closed / a room's temperature /
   "what's on" in **2.5–3.4 s** with **zero** model rounds. `speaker = "off"` is the deployed
   default for the camera, and `wake.threshold = 0.5` is upstream's default, **not** a figure
   earned on this room.

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

## MQTT — `machines/services1/services/mqtt.nix` (the broker), and what dials it

Mosquitto in podman on services1, publishing **1883 only** to `0.0.0.0` (one `listener`
in `/srv/mosquitto/config/mosquitto.conf`, one port mapping in `podman ps`; there is no
8883/TLS listener, whatever a comment here once said); its conf lives under
`/srv/mosquitto` and is not generated by the flake, which mounts the directory read of
it. **The flake never names the broker target — the clients do**, and all three dial the
name `mqtt.int.leighhack.org` inside runtime files the flake only bind-mounts: Home
Assistant's MQTT config entry (`apps1`'s `.storage/core.config_entries`), Frigate's
publisher (`/srv/frigate/config/config.yaml`), zigbee2mqtt's `server:`
(`/srv/zigbee2mqtt/configuration.yaml`). That is the one place where the answer to a name
and the socket that answers on it can disagree without anything noticing: the name used to
be a `legacy` row in `common/int-dns.nix` — "the front door answers, no vhost serves" —
and the front door moved to `gw` on 2026-10-24, so all three dials went to a box with
nothing on 1883 and hung for a week. The symptom is not an error at the broker; it is Home
Assistant rendering every Frigate **camera** entity Unavailable (its `_available` flips
only on a broker message, and its person/fps **sensors** are plain HTTP) while the sensors
keep counting. `mqtt.int` now answers with this box — `answeredBy` in
`common/int-dns.nix`, `via = "services1"`, one IPv4 line, guarded by
`machines/gw/lib/check-network-tables.nix` rather than by either drift guard, because no
vhost serves a socket — **deployed 2026-11-03**, and the three dials then went back to the
name, so nothing now hardcodes this box's address. Read the module header,
`docs/mqtt-int-legacy-answer-2026-11-02.md` and `docs/router-and-dns.md` before touching
the name, the port, or any of the three config files.

**Three reads that answer "is it alive", and one that looks like an answer and is not.** The
journal is the dated one — `sudo journalctl -u podman-mqtt.service | grep -E "New client
connected|disconnected"` names each client, the address it arrives from and when it died,
and `keepalive` is 60 s so a dead session becomes a `disconnected: exceeded timeout` within
about two minutes: eighteen quiet minutes is therefore proof, not absence of evidence.
`$SYS` answers the same question right now, and it has to be asked **inside** the container:
`sudo podman exec mqtt mosquitto_sub -h 127.0.0.1 -p 1883 -t '$SYS/broker/clients/connected'
-C 1 -W 5`. The read that misleads is `ss` on services1, which shows **no** sessions at all —
podman's published port is forwarded by `conmon` and the accepted sockets live in the
container's namespace (measured 2026-11-03: four clients connected, empty `ss`). And the
availability payload a client depends on is **one topic for the whole server**, `frigate/available`
→ `online`: there is no `frigate/<camera>/available`, so a subscription with a `+` in that
position finds nothing and reads like a dead broker.

## `crates/ups-watchdog/` — the outage notices, and the power-off that goes gw last

Not a service in the usual sense: four modes, no port, no listener, and two
systemd units that nothing starts on boot. NUT (`power.ups` on `gw`, configured in
`machines/gw/ups.nix`) owns the UPS and the outage countdown; this owns the two
things NUT has never heard of — **telling every server, and shutting them down in
order before the router shuts itself down.** Started by `upssched`'s CMDSCRIPT (via
`ups-trigger`), by `upsmon`'s `SHUTDOWNCMD`, and by hand.

The three decisions worth knowing before editing it, because each looks like
over-engineering from the outside: the countdown lives in `upssched.conf` and *not*
in Rust (a second timer is a second timer to disagree with); the re-check of
`ups.status` at the moment of action is advisory in both directions (`plan::decide`
is the tested function that says which way); and `gw` is refused from the ssh list
by *address* rather than by name, because the hop key is authorized on every box
including this one and a self-hop exits 0 (`AGENTS.md` rule 27).

`docs/ups-and-fleet-shutdown.md` is the runbook — including §7, what has never been
exercised on a live battery, and the two things (does this firmware implement
`shutdown.return`, and does this BIOS power on after AC loss) that decide whether
the network comes back by itself.

## `crates/ups-watchdog/` — the outage notices, and the power-off that goes gw last

`ups-watchdog` is the one part of the power-cut path that is not NUT, and it is a
program with no port, no daemon and no state: four modes
(`run`/`broadcast`/`plan`/`status`), started by `upssched`'s CMDSCRIPT, by
`upsmon`'s `SHUTDOWNCMD`, and by hand. NUT (`power.ups` on `gw`, configured in
`machines/gw/ups.nix`) sees the UPS, counts the 300 s of grace and cancels it when
mains return; this delivers the two notices that are not part of a shutdown and
then powers `services1` → `services2` → `aibox` off over the hop key **before** the
router powers itself off, because `gw` is the router and a router dying with its
clients still drawing current is a hard shutdown for all of them.

Three things about it are decisions rather than incidentals, and all three are the
test suite's job rather than a reviewer's: the outage countdown lives in
`upssched.conf` and deliberately **not** in here (a second timer is a second timer
to disagree with); the last-second re-check of `ups.status` is advisory in both
directions, so `OL` cancels a grace-timer shutdown but a broken `upsc` never
prevents one (`plan::decide` is the function); and `gw` is refused from the target
list **by address**, because the hop key is authorized on every box including this
one and a self-hop exits 0 — `AGENTS.md` rule 27, mechanised.

`docs/ups-and-fleet-shutdown.md` is the runbook: §6 is how to prove each half
without unplugging anything (`just ups-status`, `just ups-notice-test`,
`just ups-drill`), and §7 is what has never been exercised on a live battery —
including whether this firmware implements `shutdown.return`, which is the thing
that decides whether the network comes back by itself.

## The space calendar — Home Assistant's `local_calendar`, and where an event has to be written

The hackspace's calendar is not a service of its own, it is **three Home Assistant
entities**, and everything else in the chain is a reader of them (measured on `services1`
2026-11-23): `core.config_entries` carries three `local_calendar` entries —
`calendar.public_events` (storage key `leigh_hackspace_events`),
`calendar.member_events`, `calendar.fabrication_bookings` — each backed by
`/srv/homeassistant/.storage/local_calendar.<storage-key>.ics`. The public one is what
the world sees: `hackspace-api` asks HA for `/api/calendars/{entity_id}` **by entity id**
(`hackspaceapi/models/config.py`'s `hackspace_public_calendar` /
`hackspace_member_calendar`), and that is what publishes `/events?calendar=public|members`,
`/events.ics` (448 subscribers, `docs/home-assistant-on-services1-2026-11-16.md`), the
deterministic `space_events` "what's on" and the Monday 09:00 digest.

**The procedure, the hazards and the receipts are `docs/space-calendar.md`** — the
runbook for adding (or trying to change) an event. Two sentences of it are worth here,
for a reader who is *not* changing an event but is about to read or move these files:
the `.ics` HA parses at setup is not a file anything re-reads while HA runs, so a
hand edit is invisible to every reader below until HA next starts; and any HA-side edit
rewrites **the whole** file, so HA owns those bytes and a hand-written row is erasable
by the next edit. Both are measured, with numbers, there.

## `crates/hackspace-mcp/` + `crates/slack-agent/` — the hackspace Slack bot

**Replaces the `vulcan-bot` container on `apps1` (10.3.1.30) — stopped, disabled, file-removed
and masked there on 2026-11-16** (parked at `/var/lib/systemd/disabled-units/vulcan-bot.service`
with its restore note; container and `/srv/vulcanbot` kept for a week). It took over the old
bot's command words as well as its chat. Of its two scheduled jobs the **Monday digest is ported
and better** (`slack-agent-digest.timer`, `Mon 09:00`, `Persistent=true`, deterministic — sorted
by start, grouped by day, times as the calendar published them), and the **`#doors` MQTT access
feed is dropped by decision** ("doors not needed", 2026-11-16) — see
`docs/slack-agent-2026-11-16.md` §14 for both.
`docs/slack-agent-2026-11-16.md` is the whole story: what Vulcan's 265 Python lines actually
do, which two of its behaviours must **never** go near an LLM (the `#doors` MQTT access feed
and the Monday event digest — phase 3, still Vulcan's), every rejected library option, and the
2026-11-16 build receipt with the five things it cost. Read that before changing either unit.

Two units, both `DynamicUser`, both on loopback, both `EnvironmentFile=
/run/secrets/slack_agent_env` (`SLACK_BOT_TOKEN`, `SLACK_APP_TOKEN`, `HACKSPACE_MCP_HA_TOKEN`
— no `--token` flag exists on either binary, and neither prints its environment):

- **`hackspace-mcp.service`** — `127.0.0.1:8191`, `POST /mcp` (stateless Streamable HTTP,
  JSON-RPC 2.0, via `crates/common-rs/mcp`). Ten read-only tools: `space_status`,
  `space_events`, `home_assistant_states`, `home_assistant_history`, `camera_activity`,
  `fleet_metric`, `network_clients`, plus the **basics tier** — `what_time`, `calculate`,
  `convert` — which exists because the alternative was the prompt telling a quantised coder
  model to do daylight-saving arithmetic, division and unit lookups from memory. They live
  here rather than in a third-party MCP server so there is still exactly **one** clock, one
  value of an inch, and one set of tools every harness sees. It only ever issues HTTP **GET**s, there is no Home
  Assistant *service-call* path in the code at all, and a non-loopback bind is refused without
  `--allow-non-loopback`. Tool failures come back as MCP `isError` content rather than
  JSON-RPC errors, because the model has to be able to *read* the failure sentence.
  `--list-tools`, `--self-test` and `--call <tool> '<json>'` work with no model and no Slack.
- **`slack-agent.service`** — the harness. Slack Socket Mode over an outbound websocket (no
  vhost, no DNS row, no inbound port), an agent loop over OpenAI-compatible tool calling, and
  `127.0.0.1:8192` for `/health` + `/metrics`. Model ids are **discovered** from each
  endpoint's `/v1/models` (rule 31 — nothing in the tree names a model), endpoints come from
  `common/llm-endpoints.nix`, and no LLM credential is held: the front door injects it.
  `infra.slackAgent.dryRun` is the kill switch (option default `true`, services1 sets `false`
  since 2026-11-16). **Policy: it answers in the channel a request came from, unmentioned** —
  `--channels "*"` — which is only safe because unmentioned messages must also pass
  `triage::answers` (`src/triage.rs`): question-or-hackspace-topic yes, human-to-human chatter
  no. While `vulcan-bot` is still connected to the same Slack app, a message that is *only* one
  of Vulcan's bare commands is deferred to it (`triage::vulcan_command`), because Slack
  duplicates events to both consumers and two replies is the one visible failure.

Three behaviours that are load-bearing and easy to undo by accident: it ignores
`bot_id`/`subtype` events (**this Slack app receives its own and other bots' messages** — that
is the self-reply loop, proven from Vulcan's journal), it dedups redelivered `event_id`s
(Socket Mode redelivers anything not acked in ~3 s, so ack-then-work is mandatory), and it
drops `reasoning_content` rather than replaying it. `--max-silence-secs` exits the process
when the socket goes quiet so systemd reconnects it — rule 29 applied to a bot: a channel that
still answers `/health` is not a channel that is still working.

A **deterministic calendar path** sits in front of the model: `triage::events_window` recognises
"what's on"-shaped messages and calls `space_events` directly, so the most-asked question answers
in the time of one HTTP GET instead of ~17 s and ~3 000 tokens, and cannot have a row dropped. It
falls through to the model if the tool errors, and `just slack-agent-digest` previews the weekly
post without sending it.

Verbs (all local, none of them posting): `just slack-agent-tools`, `-probe`,
`-call tool arguments`, `-ask "question"`, `-eval`, `-status`, `-dry-run`.

## `crates/common-rs/` — where new shared code goes

Plain path dependencies (`common-json = { path = "../common-rs/json" }`), no
symlinks:

| Crate | What it owns | Used by |
| --- | --- | --- |
| `json` | hand-rolled parser **and** the string/error-object emitters the hand-built API bodies use | moonraker-exporter, status-dashboard, network-status, frigate-monitor, switch-gui |
| `oidc` | authentik login + `Infra` group gate | filestore, gocardless-dashboard, switch-gui |
| `web` | HTTP/1.1 plumbing for the `tiny_http` services: request head, query/cookie parsing, responses | the tiny services |
| `pg` | the pool, the schema runner, the sea-query→sqlx glue, the row readers | gocardless-dashboard, switch-gui |
| `service` | startup plumbing that is not the service: flag reader, `EnvironmentFile` parser, the loopback test that gates every `--no-auth` | every service |
| `preview` | the filestore preview table, read by **both** the API and the SPA | filestore |
| `build-spa` | the `build.rs` that embeds a Dioxus bundle | all four SPA apps |
| `http` | a **plain-HTTP/1.1 client** for LAN origins only: `Content-Length`, chunked and EOF bodies, and an explicit refusal of `https://` rather than a silent downgrade (the origins it dials are all loopback/VLAN and the ones that are HTTPS belong to the front door) | hackspace-mcp, and `mcp`'s client |
| `mcp` | one Model Context Protocol, **server and client in the same crate** so `hackspace-mcp` and `slack-agent` cannot drift: stateless Streamable HTTP over JSON-RPC 2.0 (`initialize`, `notifications/initialized`, `ping`, `tools/list`, `tools/call`), and tool failures returned as `isError` content, not protocol errors, so the model can read the failure | hackspace-mcp, slack-agent |

**A crate added here must be named in every consumer's `sharedCrates` list, transitively.**
`CRANE.crate` builds a member from a tree where every `common-rs/` crate it does not list is
crane's **manifest-only stub**, so the failure is invisible to `cargo build` and `just test`
and appears only in the Nix build (`just check-build`), as
`cannot find function post_json in crate common_http` — which is exactly what `slack-agent`
hit on 2026-11-16 because it uses `mcp`, and `mcp` uses `http`. The receipt is
`docs/slack-agent-2026-11-16.md` §13; deriving that list from the members' path dependencies
instead of hand-maintaining it is open in `docs/follow-ups.md`.


**New shared code goes here, not into a second copy in the second crate.**
`service` and `json`'s emitters exist because both had already drifted between
three callers; the 2026-10-13 consolidation is what found the `--no-auth` loopback bug while
deduplicating them.
