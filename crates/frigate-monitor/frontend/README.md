# frigate-monitor web UI

A small Dioxus (Rust → wasm) single-page app for the frigate-monitor scene
camera. It shows the recorded events as an infinitely-scrolling grid of
thumbnails; clicking one opens a popup with the before / after / diff images
(the changed regions are boxed) plus zoomed crops. The grid stays mounted
under the popup, so closing it leaves the scroll position untouched.

The bundle in `dist/` is **build output and never committed** — in Nix the
flake compiles this crate to wasm itself (`frontendDist` in
`machines/aibox/frigate-monitor.nix`) and the frigate-monitor binary embeds
the result at build time via its `build.rs` (`FRIGATE_MONITOR_DIST`). For
local development run `./build.sh` to produce `dist/`; build.rs picks that
up when the env var is unset.

## Rebuilding (local dev)

```sh
# one-time prerequisites
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.128   # must match Cargo.lock

# rebuild into dist/ (gitignored)
./build.sh
```

`build.sh` runs `cargo build --release --target wasm32-unknown-unknown`,
then `wasm-bindgen --target web` into `dist/` and copies `index.html`.
After changing this SPA you must rebuild the bundle **and** the Rust binary
(redeploy) for changes to appear.

## API the SPA talks to

Served by the frigate-monitor binary (same origin):

| route | purpose |
| --- | --- |
| `/api/events?limit=N&before=<id>` | event metas, newest first; `before` is the infinite-scroll cursor |
| `/api/status` | frames / last frame age / scene resets / event count |
| `/api/live?t=…` | latest raw snapshot (cache-busted) |
| `/files/<id>/<name>` | thumb.jpg / before.jpg / after.jpg / diff.jpg / before_z.jpg / after_z.jpg |

## Code layout

- `src/lib.rs` — the whole app (single component tree, no router/state libs).
  Infinite scroll: an `onscroll` handler on the scroll container fetches the
  next page when near the bottom. Popup close is driven from a
  `document::eval` bridge (Esc key) plus a 15 s status tick.
- `index.html` — mount point (`#main`) and all styling; the module script
  boots the wasm (`frigate_monitor_web.js` → `frigate_monitor_web_bg.wasm`,
  with `snippets/` imported by the wasm-bindgen glue).
- `dist/` — local build output (gitignored; built by Nix as `frontendDist`).
