# network-status frontend

Dioxus (wasm) single-page app for the `network-status` backend
(`network-info.int.leighhack.org`), served by `gw`. It polls the backend's JSON API every
5 seconds and renders per-interface bandwidth, nftables conntrack entry counts,
CPU/memory/load, the connection table, the DHCP lease table and "potential issues" on
hand-rolled canvas charts.

It was a SolidJS + TypeScript SPA until 2026-10-30. The port is the reason this
directory now has a `Cargo.toml` instead of a `package.json`.

## Stack

- **dioxus 0.7** (`web` feature) — the only runtime dependency. No router, no state
  library, no charting library (charts are plain `canvas`).
- **`common-rs/ui`** — the vendored `dioxus-components` subset
  (`docs/component-library.md`). The tabs are `common_ui::tabs`, and the "moving now"
  control is `common_ui::checkbox`; the roles, `aria-selected`, the roving focus and the
  `data-state` the CSS keys on belong to those primitives, so they are not written here.
- **gloo-net / gloo-timers / js-sys / web-sys** — fetch, the poll timer, `Date`, and
  `CanvasRenderingContext2d`. No `web-sys` feature beyond the canvas and the window.

`dioxus` must stay at the version `common/crane.nix` pins the `wasm-bindgen-cli` against
(AGENTS.md rule 18): the generated glue is schema-versioned, so a crate/CLI mismatch is a
build failure.

## Layout

```
index.html    HTML shell + the one stylesheet; loads /assets/network_status_web.js
build.sh      calls common/frontend-build-spa.sh network_status_web (the shared recipe)
src/
  lib.rs      module list + the wasm entry point
  app.rs      root component: the tabs and the one panel that is shown
  state.rs    reactive store (signals), the poll loop, hashchange + resize listeners
  api.rs      wire types + fetchers (/api/config, /api/snapshot, /api/history,
            /api/connections, /api/leases)
  connections.rs  the "Active connections" panel: filters + flow table
  leases.rs       the "DHCP leases" panel: filters + lease table with countdowns
  format.rs       byte / rate / time / duration formatting
  chart.rs        dependency-free canvas drawing + sparkline
  charts.rs       Bandwidth / Connections / Load average components
  panels.rs       header, status banner, interface cards, issues
```

`connections.rs` and `leases.rs` are the only components that own poll state of their own
(the filters), and `state.rs` is the only place that polls. Each table is a second and
third endpoint that can fail on its own, so each carries its own error into its panel
rather than leaving a screen of nothing where an answer was. The flow table and
`charts.rs`'s conntrack graph are different things: one is a table of flows, the other is
the size of the kernel's conntrack table over time.

The page is **tabbed** — `Overview`, `Connections`, `DHCP leases`, `Interfaces & issues` —
by question rather than by widget. The active tab lives in the URL fragment (`#leases`),
so a reload keeps it and a link can point at one; the tab and the fragment are the same
fact in `state.rs`, with `hashchange` making the browser's own history drive the tab.

The lease table's countdown is computed against the **router's** clock: `/api/leases`
carries the `now` it read, `state.rs` keeps the offset from the browser's clock, and
`Store::router_now` is the only place either is read. A browser whose clock is hours fast
would otherwise draw every lease as already lapsed.

## What the port preserved, and the two things it changed

Preserved, because these are the behaviours the imperative version guaranteed and a
reactive rewrite loses first:

- a failed fetch keeps the last data and names the panel that failed;
- a draw waits for the mount, then re-runs on every data change, and reads the tab so a
  chart in a hidden panel is not left at the size it had when it was hidden;
- device-supplied strings (lease hostnames) are text, not markup — the old files needed an
  `esc()` because they built HTML, and the vnode layer escapes text nodes;
- "moving now" is a **backend** filter (`min_bps`), applied before the row limit, so the
  first hundred idle rows cannot hide the traffic that is actually moving.

Changed on purpose:

1. **A panel that keeps stale data now says so.** The SolidJS page rendered an error only
   in its `!data` branch, so once a panel had answered once, later failures left the old
   table on screen with nothing saying it was old. `connections.rs` and `leases.rs` now add
   "last attempt failed: <reason>" to the summary line while keeping the rows.
2. **Tabs are the library's, and the suite asserts what the primitive actually promises.**
   The vendored `tabs` moves the *roving focus* on the arrow keys; it does not activate.
   Activation is the click, and `app.rs`'s `choose` is what writes the fragment. The old
   page activated on arrow keys because it had its own tab implementation, so the port is a
   small behaviour change — a keyboard user activates with Enter/Space on the focused tab.

## Build

```bash
just build-frontend network-status      # from the repo root, in the wasm devShell
```

`dist/` is **git-ignored** and built by the flake: `build.rs` (`common-rs/build-spa`)
embeds it in the `network-status` binary, which serves it under `/assets/`. This directory
used to be the repo's exception — its `dist/` was committed because the npm dependencies
were not nixpkgs-cached and the flake could not rebuild it offline (AGENTS.md rule 19). It
is now built like the other four SPAs, so that exception is closed.

The binary serves the bundle as **bytes**: `common_web::Reply` carries `Vec<u8>` because one
of the assets is a wasm binary, and a bundle decoded as UTF-8 is either an error or a
mangled one. See `crates/network-status/src/serve.rs`.

The frontend is its own cargo workspace (`[workspace]` in `Cargo.toml`) and is **not** a
member of `crates/Cargo.toml`: it builds for `wasm32-unknown-unknown`, and a member would
unify `wasm-bindgen` features against the pinned CLI. Its `Cargo.lock` therefore pins
`wasm-bindgen 0.2.128` by itself — the same version `build.sh` runs the CLI at — because a
lock that resolves a newer crate is the version mismatch recorded in
`docs/building-rust-code.md`.

Tests: `crates/network-status/tests/` — `just network-status-test`.

## API (consumed by this SPA)

| Endpoint        | Returns                                             |
| --------------- | --------------------------------------------------- |
| `GET /api/config`   | `{"wan":"ppp0","title":"Network — router","connections":{…},"leases":{…}}` — the flow table's defaults and the prefixes it calls "ours", and the lease table's `ttl`, row limit and VLAN list |
| `GET /api/snapshot` | latest snapshot JSON (or 503 before first probe)  |
| `GET /api/history`  | full rolling history JSON (ts, per-if rates, conntrack, load) |
| `GET /api/connections` | the flow table: `{connections, hosts, totals, filter, defaults, tracked, active, matched, returned, unaccounted, unparsed, ts, interval}` — see `machines/gw/network-status.nix` and `docs/services.md` for the query contract (`min_age`, `min_bytes`, `min_bps`, `q`, `kind`, `sort`, `limit`) and what each direction means |
| `GET /api/leases`   | the DHCP lease table: `{leases, vlans, networks, vlan_labels, filter, defaults, now, lease_ttl, file, total, skipped, matched, returned, expired, unknown_ttl, generated, max_left}` — query contract (`q`, `vlan`, `net`, `max_left`, `sort`, `limit`) and why a lease's length is recovered rather than read are in `docs/services.md` |

Both tables are polled on the same 5 s tick as `/api/history`. The lease table re-reads
dnsmasq's file on the router side (a 3.5 KB read for ~50 rows), so polling it is cheap and
it keeps the countdowns honest without a second timer.

`/api/leases` answers **503 with the operating system's reason** when it cannot read the
file, and the panel shows that as an error. It never answers an empty table for an
unreadable file: "no DHCP clients" is a conclusion an operator would act on.
`machines/gw/lease-reader.nix` is the module that decides who may read the file.
