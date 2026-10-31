# The component library — what is vendored, and what it cost to find out

`crates/common-rs/ui/` is the component library the Dioxus SPAs use. It is not ours:
it is a pruned copy of the `primitives` crate of **`DioxusLabs/dioxus-components`**.
This file is why that one, why it is vendored as source instead of pulled as a
dependency, and the things that bit during the first conversion (`switch-gui`). Read it
before adding a component to `crates/common-rs/ui/`, before wiring another SPA to it, or
before changing a vendored file.

The pilot is `switch-gui` because it already had a headless-browser suite
(`crates/switch-gui/tests/`), so a component can be proven through the path a user takes
rather than through a type check.

## The decision, and what was measured first

The requirement was a **tested** library, not a set of hand-rolled primitives: before
this, every SPA wrote its own widget and each one announced nothing. Candidates, as
measured on 2026-10-30:

| Candidate | Why it was not chosen |
| --- | --- |
| `DioxusLabs/dioxus-components` | **Chosen.** 34 Playwright specs against WAI-ARIA authoring practices, CI that runs them sharded, a job that adds every component and re-checks it, stylelint. Unstyled primitives — the shape this repo wants. |
| `tdymel/libero` | Published and well tested (931 test files), but pins `=dioxus 0.8.0-alpha.3` and `=wasm-bindgen 0.2.127`, which is not this repo's pin (rule 18), and it pulls desktop-heavy deps (`blitz-dom`, `dbus`, `rfd`, `objc2-app-kit`) into a wasm bundle. |
| `dioxus-primitives` on crates.io | A name reservation. One published version, `0.0.0`, no dependencies; the tarball is 884 B and its `lib.rs` is `pub fn add(left, right)`. |
| `dioxus-tw-components`, `eq_ui`, `freyr`, `dioxus-charts`, `lumen-blocks` | No meaningful test coverage, and some need a build step this repo cannot run offline. |

## Why the source is vendored, not depended on

`dioxus-components` is distributed by `dx components add` — a shadcn-style copy into the
consumer's tree. That needs the `dx` CLI and network access at dev time, and it gives
each SPA its own copy, which is rule 2 (shared code has one copy). So the source is
copied into `crates/common-rs/ui/` once, every SPA path-depends on it through the
symlink/`sharedCrates` machinery, and the pin is recorded in that crate's `lib.rs`
header with the command that re-diffs a file against upstream.

Only the modules a SPA actually uses are vendored. That is what keeps files inside rule
26 (upstream's `calendar.rs` is 91 KB) and keeps the dependency graph to `dioxus` alone.

## Two things the vendored tree must not lose

* **`src/ts` hashes.** Upstream's `build.rs` uses `lazy-js-bundle 0.7.9`, which shells out
  to `bun build` **only when the `.ts` files no longer match the committed
  `src/js/hash.txt`**. A Nix build has no `bun`, so a pruned tree must never edit a `.ts`
  file: preserve the hash, or the build becomes a network-and-bun build.
* **Runtime JS assets.** `dialog` calls `window.createFocusTrap` through
  `dioxus::document::eval`. `common/frontend-build-spa.sh` emits the wasm, its glue and
  `index.html` — nothing else — so any component that needs an asset or the `dioxus.recv`
  runtime cannot be adopted until the build recipe carries it. This is not hypothetical:
  `Checkbox`'s hidden form input (`BubbleInput`) drives the input through `document::eval`,
  and in this repo's build the SPA panicked at render (`pageerror: unreachable`) — the
  matrix never drew. The render is skipped in the vendored copy, with the reason stated
  in the file. A SPA that genuinely needs form submission has to add the eval runtime to
  the recipe first.

## Traps, all hit here

1. **A hook inside a hook panics at first render.** `use_hook(|| use_signal(..))` gives
   `The hook list is already borrowed` — the page renders its header and never draws the
   matrix. `use_signal` is already a hook; call it at the top of the component.
2. **Controlled state must be a signal, not a prop value.** The library's `Checkbox` is
   controlled by `ReadSignal`. A `bool` prop captured at the first render announces the
   state as it was when the row was drawn while the app's overlay moves underneath it. The
   effect re-runs only because it *reads* the overlay signal — that is the whole design of
   the cell.
3. **`disabled` is `ReadSignal<bool>`.** Passing a plain `bool` at the call site lands on
   the button as the HTML `disabled` attribute and the widget's own `data-disabled` is
   never set — the two channels then disagree.
4. **No `aria-disabled`.** For a `button[role=checkbox]` the library emits the HTML
   `disabled` attribute and `data-disabled`; it does not emit `aria-disabled`. Playwright's
   `isChecked()`/`check()` do not apply to a button either, so the suite asserts the DOM
   state and `data-disabled`, and asserts they agree.
5. **A visually-hidden box can swallow the click.** The reason text shown on focus is
   `position:absolute` with `pointer-events:none`. Without `pointer-events:none` it is 1px
   on screen and still the hit target. And it must **stay** absolutely positioned when
   shown: a click focuses the control on mousedown, and if revealing the text puts it back
   in flow the cell relayouts between mousedown and mouseup, so the browser delivers the
   `click` to the common ancestor — measured as `TD` on the first click, the button on the
   second.
6. **Ids that start with a digit are not CSS selectors.** `#23-228` is a legal HTML id and
   an invalid selector; `querySelectorAll` throws. Cell ids are `why-<port>-<vlan>`.
7. **The binary, not the dist.** `build.rs` embeds `dist/`, so rebuilding `dist/` alone
   still serves the SPA that was compiled into the binary. A probe that rebuilds only the
   frontend reports the old DOM. Build the crate, or run the suite, which does both.
8. **`crates/common-rs/ui` is not its own workspace.** It reaches the SPAs through a
   symlink inside each SPA's directory, so cargo makes it a member of *that* SPA's
   workspace; declaring a workspace root here fails with "multiple workspace roots found
   in the same workspace". It is kept out of the **native** workspace by `exclude` in
   `crates/Cargo.toml`, so `cargo build --workspace` never compiles it for the host — which
   also means `just test` and `just clippy` do not see it. **The gate for this crate is the
   Playwright suite**, not cargo.
9. **Roving focus is not activation.** `collection`'s `focus_next` moves focus; it does not
   select. A tab that had its own key handling before the library (network-status) activated
   on ArrowRight, so converting to `common_ui::tabs` is a behaviour change: keyboard users
   now move with the arrows and activate with Enter/Space on the focused trigger. Its suite
   asserts the two halves separately (`crates/network-status/tests/suite.mjs`) rather than
   asserting the old combined behaviour, so a change upstream shows up as a failure instead
   of a passing test that no longer matches the primitive.

## What a vendored component drags in

A component is not a file: it is the file plus the hooks it calls. `tabs` needs `collection`
(the roving-focus state every focus-trap component upstream is built on), `use_id_or` (for
`aria-controls`) and `use_effect_cleanup`; `checkbox` needs `use_controlled` and
`use_unique_id`. That is why `collection.rs` is here even though no SPA uses a collection
widget directly — vendoring `tabs` alone would have been a file that does not compile.

## What stays app-specific

The library has no table and no chart. Tables stay app markup, built on the library's
`collection`/`selectable` primitives where keyboard focus and selection matter; the canvas
charts stay as they are. Naming, meaning and the reason a cell is locked are app facts —
the widget cannot know what "VLAN 225 is this port's untagged VLAN" means, so that sentence
stays on the app side and is handed to `aria-label` / `aria-describedby`.

## Order of rollout

1. `switch-gui` — done (matrix cell, accessibility assertions, the suite's a11y test).
2. `filestore`
3. `network-status` — **done 2026-10-30**: it was the last non-Dioxus SPA, converted and
   wired to `common_ui::tabs` (the `tabs` and `collection` modules exist because of it) and
   `common_ui::checkbox` for the "moving now" control. It had tab ARIA attributes with no
   keyboard handling, which is exactly what the library adds — and `crates/network-status/tests/`
   was added with the conversion so the primitives can be asserted through the path a user
   takes, not through a type check.
4. `gocardless-dashboard`
5. `frigate-monitor`

## Re-verify

```bash
# a single vendored file against the pin
curl -s https://raw.githubusercontent.com/DioxusLabs/dioxus-components/9a758255ea26e2b20c8cecf4c1c946feb9e71da7/primitives/src/checkbox.rs \
  | diff - crates/common-rs/ui/src/checkbox.rs

# the gate for a component change — not on the router, which cannot build
just build-frontend switch-gui && just switch-gui-test
just build-frontend network-status && just network-status-test
```
