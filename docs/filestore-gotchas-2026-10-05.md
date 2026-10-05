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
