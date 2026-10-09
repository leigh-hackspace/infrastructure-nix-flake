# filestore — gotchas learned the hard way (2026-10-05)

Notes for anyone touching `filestore/` (or any Dioxus wasm SPA in this repo).
These are the things that silently break and are invisible until you build or
open the page.

## 1. The pinned dioxus version decides the whole API

`frontend/Cargo.toml` pins **dioxus 0.7.10** (and `wasm-bindgen 0.2.128`, which
must match `wasm-bindgen-cli` exactly — see the wasm-bindgen gotcha in the main
AGENTS.md). Code written against the 0.6 API does not compile, and the
differences are not deprecations, they are removals:

- `use_signal` / `use_future` / `use_effect` take `FnOnce` / `FnMut`, not `Fn`.
  The 0.6 idiom `use_signal(move || st.clone())` fails; write `use_signal(move || st)`
  and make the state bundle `Copy` (all-`Signal` structs are `Copy`) so each
  closure captures its own copy.
- `Signal<T>` requires `T: PartialEq + 'static` — every state struct must derive
  `PartialEq`.
- Signals need `mut` bindings (`let mut st = ...`, `let mut input = ...`) to
  call `.set()`; `for`-loop closures capture by `&` and then can't mutate.
- `Event::node()` is gone. Anything that styled/hit `e.node()` must be rewritten
  (the context-menu hover is now plain CSS: `.fs-menu-item:hover`).
- `Event` derefs to `Rc<T>`; `prevent_default()` / `stop_propagation()` take
  `&self`.
- Coordinates: `event.client_x()` → `event.coordinates().client().x`.
  Modifiers: `event.modifiers` → `event.modifiers()`.
  `trigger_button()` returns `Option<MouseButton>` (None for move/enter), so a
  left-click test needs `matches!(e.trigger_button(), None | Some(Primary))`.
- `Key` comes from `dioxus::html::Key` (re-exported `keyboard_types`), not the
  `keyboard_types` crate directly.
- `DataTransfer` is `dioxus::html::DataTransfer`, and `get_data()` returns
  `Option<String>` (0.6 returned `String`).
- rsx event handlers must be `Fn`/`FnMut`: a `Box<dyn FnOnce()>` action passed
  into a helper that builds an `onclick` fails with E0525. Use `Box<dyn Fn()>`
  and clone captured values inside the closure body.
- `for`-loop bodies in rsx cannot contain statements — precompute display
  structs (`Row`, `UpRow`, `HitRow`) before the `rsx!`.
- `ondblclick` is `ondoubleclick`.
- `wasm_bindgen::JsValue::unchecked_ref()` → `unchecked_into::<T>()` (needs
  `use wasm_bindgen::JsCast;`), and `add_event_listener_with_callback` wants
  `&js_sys::Function`.

## 2. `wasm-bindgen --target web` has no entry point

A `pub fn main()` in a wasm lib target is dead code. The generated glue only
runs `main` if it is annotated `#[wasm_bindgen(start)]`, and `index.html` must
call the default export (`init()`) — not `init.then(() => init.call())`. Without
both, the page loads and never renders (no error in the console).

`dioxus::launch(App)` takes a plain `fn() -> Element`, so `launch(App)` from a
`#[wasm_bindgen(start)]` hook is fine (it spawns a local task). The default
dioxus-web root id is `"main"` — `index.html` must have `<div id="main">`.

## 3. `gloo` crates are not dioxus-version-locked

- `gloo-timers 0.3` has `gloo_timers::future::sleep(Duration)`, not the 0.2
  `Sleep` struct.
- `gloo-net 0.6`: `RequestBuilder::json_body()` is gone — `.json(&value)` returns
  `Result<Request, Error>`, so you must build the request then `send()`.

## 4. rsx input binding

`value: signal` alone does not update the signal — an `oninput` handler is
required, otherwise typing in a modal does nothing (no compile error, just a
dead input).

## 5. Nix build specifics

- The Nix build is **offline**: `cargo build` must be `--offline`, so the
  `Cargo.lock` must be committed and every crate must be in the local registry
  or fetched by `fetchCargoVendor`.
- `build.rs` embeds the SPA from `frontend/dist` (local) or `$FILESTORE_DIST`
  (Nix). If the Nix derivation is built without `FILESTORE_DIST` set, the binary
  silently ships with no assets and returns 500 for every page.
- `frontend/dist/` is git-ignored and never committed; rebuild with
  `nix develop --command bash -c 'cd filestore/frontend && ./build.sh'`.
- The flake has no `packages` output for filestore, so `nix flake show` won't
  list it; to build just the binary use a standalone `nix build -f <expr>` with
  `rustPlatform.buildRustPackage` + `FILESTORE_DIST`.

## 6. Testing without a browser

- `--dev-user <name>` + `GET /auth/dev-login` mints a session cookie; then
  `curl -c jar .../auth/dev-login` and `curl -b jar /api/list?path=` exercises
  the whole API. Never pass `--dev-user` in production.
- To check that a client id/secret pair in the sops env file actually matches
  the authentik provider, POST a bogus code to
  `https://id.leighhack.org/application/o/token/`: `invalid_grant` means the
  client is valid, `invalid_client` means the secret is wrong.
- `GET /auth/login` through nginx shows the exact authorize URL (client id,
  redirect URI, scopes, PKCE challenge) — a good sanity check for the provider
  config without a browser.

## 7. sops editing (non-interactive)

- `sops edit` is a TUI; scripting `EDITOR` is fragile. Prefer
  `sops exec-env` (values in the environment) + `sops -d` → modify → `sops -e`.
- `sops -e` fails with *"no matching creation rules found"* when run from the
  repo root, because `.sops.yaml` creation rules only match `secrets/*`. Run it
  from outside the repo (e.g. `/tmp`) or pass `--age <recipient>`.
- `env_file` is one secret holding the whole `.env`; if it is a multi-line value
  it decrypts to a **block scalar**, so a naive `grep -v '^env_file:'` leaves the
  indented continuation lines orphaned and the file becomes invalid YAML.
- Client secrets are write-only through the API but readable via the ORM shell
  (`OAuth2Provider.client_secret`) on the authentik box.

## 8. Selection is keyboard-driven as well as mouse-driven

The arrow keys move the selection and shift+arrow extends the range from the
anchor (`AppState::focus`, the same state shift-click uses), with the column
count read from the rendered grid so up/down matches what the user sees rather
than what the model assumes. Enter opens the focused row (folder → navigate,
previewable file → preview, otherwise download) and is deliberately left to the
focused control when a text field or button has focus, so the search box and the
modal input still work.

## 9. The context menu can only be measured after the first paint

It is clamped to the viewport, but its size is unknown until it has been
rendered once, so `menu.rs` caches the size between opens. It closes on any click
outside it.

## 10. Drag-out to the OS: a page can only hand over a URL or bytes it holds

Rows publish `text/uri-list` on `dragstart` (the download URL, or the ZIP URL for
a folder) as well as `application/x-filestore`, because the internal format is
only understood by this page and an external drop target would refuse the drag.
A browser cannot stream a remote file to Finder, so the drop becomes a `.webloc`
shortcut; the drag therefore also carries `text/html` with the file name as the
link text, because Finder names the shortcut from the link text — otherwise every
drop is called `filestore.int.leighhack.org:.webloc`.

## 11. Uploads are raw request bodies, so axum's body limit does not apply

`DefaultBodyLimit` does not cover them: the server enforces `--max-upload`
(2G in production) and the vhost's `client_max_body_size` must match it,
otherwise nginx 413s the request first and the upload popup prints nginx's HTML
error page.

## 12. The preview table has to be one table

`is_previewable()` in the SPA and the classifier in `/api/preview` were two
hand-copied lists, and they drifted: the menu advertised SVG while the API refused
it (bug #6 in `filestore/tests/README.md`). The table is now `common-rs/preview`,
read by both halves, so they cannot disagree. That also means the SPA's crate needs
the shared crate copied into its source tree — `CRANE.wasmSpa` gained `sharedCrates`
for it, because the git-ignored `frontend/common-rs` symlink is not part of the
flake source.

Two constraints decide what can be in it:

- **Only what a browser can render.** There is no server-side converter, so a
  preview is whatever an `<img>`, `<video>`, `<audio>` or the browser's own PDF
  viewer shows. `image/tiff` and `image/heic` are real image types that no browser
  decodes, `video/x-msvideo` is a real type that nothing plays, and docx/xlsx are
  zipped XML. Putting them in the table would make the UI promise a preview it
  cannot deliver, so the table is a browser-support table, not a MIME list.
- **Content from the store is untrusted.** Media is served from the app's own
  origin, so the response carries `nosniff` and `default-src 'none'`. SVG is
  accepted because it is rendered through an `<img>`, which cannot run script — but
  the same URL can be opened as a document, which is why the CSP sits on the
  response instead of the old "refuse SVG" rule.

Media responses answer a single `Range` request with 206 + `Content-Range`. Without
it every seek in a video or audio file re-transfers the whole file from the NAS.

Text is still returned as JSON (truncated at 512 KiB), so the SPA's text path is
unchanged. A file whose extension the table does not know is not advertised by the
UI, but the API still tries it as text — which is what makes an oddly-named
plain-text file previewable when fetched directly, and what the suite checks both
ways.
