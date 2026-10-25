# network-status frontend

SolidJS + TypeScript single-page app for the `network-status` backend
(`network-info.int.leighhack.org`). It polls the backend's JSON API every 5
seconds and renders per-interface bandwidth, nftables conntrack entry counts,
CPU/memory/load, and "potential issues" on hand-rolled canvas charts.

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
  app.tsx           top-level layout + interface-selector state
  store.ts          reactive state: rolling history + last snapshot + poll loop
  api.ts            API types + fetchers (/api/config, /api/snapshot, /api/history)
  connections.tsx   the "Active connections" card: filters + flow table
  format.ts         byte / rate / time helpers
  chart.ts          dependency-free canvas chart + sparkline drawing
  charts.tsx        Bandwidth / Connections / Load average components
  panels.tsx        header, status banner, interface cards, issues
```

`connections.tsx` is the only component that owns poll state of its own (the filters),
and `store.ts` is the only place that decides *whether* the flow table is polled at all
— see its `connectionsWanted`. The card and the `charts.tsx::ConnChart` graph above it
are different things: one is a table of flows, the other is the size of the kernel's
conntrack table over time, from the history the backend already keeps.

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
| `GET /api/config`   | `{"wan":"ppp0","title":"Network — router","connections":{…}}` — the flow table's defaults and the prefixes it calls "ours" |
| `GET /api/snapshot` | latest snapshot JSON (or 503 before first probe)  |
| `GET /api/history`  | full rolling history JSON (ts, per-if rates, conntrack, load) |
| `GET /api/connections` | the flow table: `{connections, hosts, totals, filter, defaults, tracked, active, matched, returned, unaccounted, unparsed, ts, interval}` — see `machines/gw/network-status.nix` and `docs/services.md` for the query contract (`min_age`, `min_bytes`, `min_bps`, `q`, `kind`, `sort`, `limit`) and what each direction means |

`/api/connections` is polled on the same 5 s tick as `/api/history`, and only while
the card is actually going to show something (`store.ts`'s `connectionsWanted`): with
the default 5 s threshold and no filters a young table is empty, and polling it every
five seconds would be a client asking a router for work to throw away.
