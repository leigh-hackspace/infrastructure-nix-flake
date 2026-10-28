# network-status frontend

SolidJS + TypeScript single-page app for the `network-status` backend
(`network-info.int.leighhack.org`). It polls the backend's JSON API every 5
seconds and renders per-interface bandwidth, nftables conntrack entry counts,
CPU/memory/load, the connection table, the DHCP lease table and "potential issues"
on hand-rolled canvas charts.

## Stack & why it's small

- **solid-js** — the only runtime dependency. No router, no state library, no
  charting library (charts are plain `canvas`).
- **esbuild + esbuild-plugin-solid** — the build. We deliberately avoid
  Vite + `@vitejs/plugin-solid`: the `@vitejs/*` namespace is unreachable from
  this network's registry, so the toolchain is kept to the two unscoped
  packages that resolve here.
- **typescript** — type-checking only (`tsc --noEmit` runs as part of `build`).

## Layout

```
index.html          HTML shell; references /bundle.js and /bundle.css
build.cjs           esbuild entry point (bundle, minify, copy index.html)
tsconfig.json       tsc config (type-check, JSX → solid-js)
src/
  main.tsx          mounts <App/> into #app, imports styles.css
  app.tsx           top-level layout: the four tabs and which one is showing
  store.ts          reactive state: rolling history, snapshot, connection table,
                    lease table and the poll loop
  api.ts            API types + fetchers (/api/config, /api/snapshot, /api/history,
                    /api/connections, /api/leases)
  connections.tsx   the "Active connections" panel: filters + flow table
  leases.tsx        the "DHCP leases" panel: filters + lease table with countdowns
  format.ts         byte / rate / time helpers
  chart.ts          dependency-free canvas chart + sparkline drawing
  charts.tsx        Bandwidth / Connections / Load average components
  panels.tsx        header, status banner, interface cards, issues
```

`connections.tsx` and `leases.tsx` are the only components that own poll state of their
own (the filters), and `store.ts` is the only place that polls. Each table is a second
and third endpoint that can fail on its own (a router started `--no-conns`, a lease file
that cannot be read), so each carries its own error into its panel rather than leaving a
screen of nothing where an answer was. The flow table and the `charts.tsx::ConnChart`
graph above it are different things: one is a table of flows, the other is the size of
the kernel's conntrack table over time, from the history the backend already keeps.

The page is **tabbed** since 2026-10-26 — `Overview`, `Connections`, `DHCP leases`,
`Interfaces & issues` — by question rather than by widget. All four panels stay mounted
(an unmounted chart is a blank chart until its next 5 s sample) and the active tab lives
in the URL fragment (`#leases`), so a reload keeps it and a link can point at one.

The lease table's countdown is computed against the **router's** clock: `/api/leases`
carries the `now` it read, `store.ts` keeps the offset from the browser's clock, and
`store.routerNow()` is the only place either is read. A browser whose clock is hours
fast would otherwise draw every lease as already lapsed.

## Build

```bash
npm install      # populates node_modules from package-lock.json
npm run build    # tsc --noEmit  then  node build.cjs  → dist/  (no .map by default)
```

`dist/` contains `bundle.js`, `bundle.css`, `index.html`. Set
`SOURCEMAP=true npm run build` for sourcemaps.

`dist/` is committed to the git tree: the flake (`machines/gw/
network-status.nix`) copies it straight into the Nix store — no npm step at
build time, so the deploy stays hermetic (Nix builds run offline). After
editing the SPA source, rebuild and commit `dist/`.

The `network-status` Rust binary serves this directory (via
`--static-dir`), falling back to `index.html` for unknown routes (client-side
routing).

## API (consumed by this SPA)

| Endpoint        | Returns                                             |
| --------------- | --------------------------------------------------- |
| `GET /api/config`   | `{"wan":"ppp0","title":"Network — router","connections":{…},"leases":{…}}` — the flow table's defaults and the prefixes it calls "ours", and the lease table's `ttl`, row limit and VLAN list |
| `GET /api/snapshot` | latest snapshot JSON (or 503 before first probe)  |
| `GET /api/history`  | full rolling history JSON (ts, per-if rates, conntrack, load) |
| `GET /api/connections` | the flow table: `{connections, hosts, totals, filter, defaults, tracked, active, matched, returned, unaccounted, unparsed, ts, interval}` — see `machines/gw/network-status.nix` and `docs/services.md` for the query contract (`min_age`, `min_bytes`, `min_bps`, `q`, `kind`, `sort`, `limit`) and what each direction means |
| `GET /api/leases`   | the DHCP lease table: `{leases, vlans, networks, vlan_labels, filter, defaults, now, lease_ttl, file, total, skipped, matched, returned, expired, unknown_ttl, generated, max_left}` — query contract (`q`, `vlan`, `net`, `max_left`, `sort`, `limit`) and why a lease's length is recovered rather than read are in `docs/services.md` |

Both tables are polled on the same 5 s tick as `/api/history`. The lease table re-reads
dnsmasq's file on the router side (a 3.5 KB read for ~50 rows), so polling it is cheap
and it keeps the countdowns honest without a second timer.

`/api/leases` answers **503 with the operating system's reason** when it cannot read the
file, and the panel shows that as an error. It never answers an empty table for an
unreadable file: "no DHCP clients" is a conclusion an operator would act on.
`machines/gw/lease-reader.nix` is the module that decides who may read the file.
