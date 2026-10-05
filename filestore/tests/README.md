# filestore headless-browser test suite

Drives the real filestore web UI in headless Chromium against a scratch
fixture, plus the JSON API. Run it from the repo root:

```bash
just filestore-test
```

`run.sh` builds the SPA (`frontend/build.sh`) and the binary, starts
`filestore --no-auth` on `127.0.0.1` against `/tmp/filestore-test-root`, and
runs `suite.mjs`. Every test rebuilds the fixture and reloads the page, so
tests are order-independent.

## Why `--no-auth` exists

The API is session-gated by OIDC, which a headless browser cannot complete.
`--no-auth` treats every request as an authenticated local session, and it is
**only honoured on a loopback bind** (`main.rs` refuses `--no-auth` with a
non-loopback `--bind`), so the escape hatch can never expose the store on a
routable interface. It is never passed in production — the NixOS service runs
without it, and the suite asserts that a server started without `--no-auth`
still answers 401.

## Playwright on NixOS

Playwright's own browser download is unwrapped and cannot find `libglib`, so
`run.sh` uses nixpkgs' wrapped browsers
(`nix build nixpkgs#playwright.browsers-chromium`) and passes the executable
path explicitly. Override with `FS_TEST_BROWSER`.

Node is used here deliberately: Playwright is the only practical headless
driver, and its Rust bindings are not usable offline in this repo.

## Hooks the suite relies on

The SPA is otherwise hard to drive from a browser, so the UI carries stable
test hooks (harmless in production):

- every grid/table row has `data-fs-path` and `data-fs-name`
- stable ids: `#fs-content`, `#fs-status`, `#fs-menu`, `#fs-modal`,
  `#fs-modal-input`, `#fs-preview`, `#fs-search`, `#fs-toasts`, `#fs-uploads`
- `index.html` uses an empty `data:` icon so Chromium does not request
  `/favicon.ico` (which would show up as a console error)

## Bugs this suite found

All of these were found by running the suite and are covered by tests.

1. **Fire-and-forget tasks were cancelled.** `spawn()` attaches a task to the
   current component's scope, and dioxus cancels it when that component is
   dropped. Handlers that change state which unmounts them (grid → "loading…",
   menu closing, modal closing) therefore did nothing: double-clicking a folder,
   the modal OK button, and the context menu actions all silently failed. Fixed
   by `state.rs::spawn_task`, which uses the root scope (`spawn_forever`).
2. **The delete modal sent the wrong paths.** It passed names relative to the
   current folder to `/api/delete`, which takes store-root-relative paths — so
   deleting a file in a subfolder targeted a same-named file at the root.
3. **The background context menu was unreachable.** An overlay div covering the
   grid swallowed every right-click, so the menu only ever opened on rows.
4. **Internal drags always copied.** `DataTransfer::get_data()` returns
   `Some("")` for formats that were never set, so `is_some()` was always true
   and Ctrl-less drags copied instead of moving.
5. **The ↑ button panicked.** It held a read guard on `st.path` while
   `navigate()` wrote the same signal — a re-entrant signal borrow, which shows
   up in wasm as `unreachable`.
6. **`is_previewable()` advertised SVG** but `/api/preview` refuses SVG (script
   risk), so the menu offered Preview and then showed "not previewable". The
   allow-list now matches the server.
7. **Shallow search reported `scanned: 0`** (and never `truncated`), so the
   results header always claimed nothing had been scanned.
8. **`/api/list` returned 400 for a missing directory** while every other
   endpoint returns 404.
