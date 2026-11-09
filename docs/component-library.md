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
What is here today, and why each one is: `tabs` + `collection` (network-status's tab strip,
and the roving-focus state `tabs` is built on), `checkbox` (switch-gui's matrix cells and
network-status's "moving now"), and `progress` + `toggle` + `toggle_group` (network-status's
lease countdown and its two segmented filters, added 2026-11-09).

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
10. **A vendored prop is a signal, so a plain function cannot supply one.** Several props
    are `ReadSignal<Option<T>>` (`Tabs::value`, `Checkbox::checked`, `ToggleGroup::pressed`,
    `Progress::value`). A component can build one with `ReadSignal::from(use_memo(…))`, but a
    helper *function* called from inside a render cannot call `use_memo` at all — so the row
    that wants a `Progress` becomes a small `#[component]` whose props are plain data
    (`LeaseProgress { pct, lapsed }` in `leases.rs`). That is the shape to reach for: props
    in, memo at the edge, primitive underneath.
11. **`Progress` draws nothing unless the indicator reads its variable.** Upstream puts
    `--progress-value` on the `role=progressbar` element and leaves `ProgressIndicator` a bare
    `div`; the app's CSS must set the indicator's `width: var(--progress-value)`, or every bar
    is zero-height nothing. And its `data-state` is **`loading` for a known value and
    `indeterminate` for `None`** — there is no "complete" state, so styling `[data-state=complete]`
    silently styles nothing. `value: None` is the honest way to say "this proportion is not
    known here" (an out-of-block DHCP lease), and it is worth a striped track rather than an
    invented number.
12. **A toggle group lets you click the pressed segment again to un-press it.** Upstream's
    `set_pressed(id, false)` then hands the callback an **empty** `HashSet`. Where the group
    stands for one value of a filter ("direction", "show leases"), that state cannot mean
    anything, so the app maps the set back through the option table and *ignores* the empty
    one — which makes it behave like radios while remaining the library's component. Assert
    the mapping in both directions (`aria-pressed` and the request that follows).
13. **Unstyled primitives make a lost stylesheet invisible.** Nothing in the page breaks when
    a rule stops matching: the DOM still renders, the roles are still right, and only the
    colours are missing. That is how network-info shipped for nine days with no interface
    cards, no chart legend and a permanently grey status pill — see "The stylesheet is app
    code too" below, and the **stylesheet guard** at the end of that SPA's suite, which is the
    detector.

## What a vendored component drags in

A component is not a file: it is the file plus the hooks it calls. `tabs` needs `collection`
(the roving-focus state every focus-trap component upstream is built on), `use_id_or` (for
`aria-controls`) and `use_effect_cleanup`; `checkbox` needs `use_controlled` and
`use_unique_id`; `toggle_group` needs `collection`, `use_controlled` **and** `toggle`, which
is itself only `use_controlled`. That is why `collection.rs` and `toggle.rs` are here even
though no SPA uses either directly — vendoring `tabs` alone would have been a file that does
not compile.

The size of the bill is one command: read the candidate's `use` block. If it names only
`dioxus::prelude` and `crate::…`, it is in; if it names `dioxus_attributes`, or calls
`document::eval` on the render path, or needs a new crate, it is out (`progress` came in on
2026-11-09 having passed exactly that test; `collapsible` — the thing that wanted to be the
notes footer on two panels — went out on `merge_attributes` and the attributes macro).

## The stylesheet is app code too

The primitives ship **no CSS**. What they ship is a contract: `role`, `aria-*`, and
`data-state` / `data-orientation` / `data-disabled` / `--progress-value`, each one documented
in the file. Styling is therefore the app's job, and it has two halves that must not drift:
the class the component emits, and the selector the sheet uses.

Measured cost of not treating that as code. When network-status was ported from SolidJS
(`f5256aa`, 2026-10-30) its `styles.css` was rewritten from scratch as an inline `<style>` and
keyed on names the new Rust never emits — `.pill.ok` where the component says `pill good`,
`.issues li` where it says `.issue.bad`, `.controls select` where it says `.ctl` — and no rule
at all for `iface`, `iface-grid`, `dot`, `rates`, `legend`, `sw`, `chart-head`, `conn-*`,
`ctl`, `q`, `who`, `addr`, `lapsed` or `low`. The page still *worked*: every check in its
suite passed. What a user saw was a wall of unstyled text with three white OS `<select>`s on a
dark page, interface cards with no card, invisible status dots, an invisible legend swatch,
rate bars whose `width` was set on an inline element (so ignored), and an "all good" pill that
could never turn green. The `Uplink` tab, added the day this was written up, had never been
styled at all either.

Two rules follow, and both are enforced rather than advised:

- **Style the primitive through what it announces** (`[role=tab][data-state=active]`,
  `.segs [data-state=on]`, `.lease .fill { width: var(--progress-value) }`), and keep the
  app's own `class` props for layout hooks. A widget's role and state attributes are
  upstream's stable surface; an invented class is nobody's.
- **A class emitted by a component and named by no rule is a bug**, and the **stylesheet
  guard** in `crates/network-status/tests/suite.mjs` refuses one. It is cheap, it runs in the
  suite that already exists, and it is the only check that can see this fault: the failure is
  *absence of appearance*, which no behavioural assertion notices. Its companion, the
  **painted check**, reads computed styles for the cases a selector cannot rule out — a
  `width` on an inline element, a `span` with no size, a swatch with a colour and no box.

## What stays app-specific

The library has no table and no chart. Tables stay app markup, built on the library's
`collection`/`selectable` primitives where keyboard focus and selection matter; the canvas
charts stay as they are. Naming, meaning and the reason a cell is locked are app facts —
the widget cannot know what "VLAN 225 is this port's untagged VLAN" means, so that sentence
stays on the app side and is handed to `aria-label` / `aria-describedby`.

Some things the library *does* have cannot be used here, and each needs something
first: `select`, `tooltip`, `dropdown_menu` and `collapsible` (`dioxus-attributes`, a
proc-macro crate that is not published and not in this repo's inputs — so the native
`<select>` stays, dressed, and network-info's long "why" footers stay always-open text);
`dialog` and anything with a focus trap (a runtime JS asset `common/frontend-build-spa.sh`
does not emit); `Checkbox`'s hidden form input (the `document::eval` runtime — see the
deviation in `checkbox.rs`). "The library has it" is not the whole question: the build
recipe is the rest of it.

## Order of rollout

1. `switch-gui` — done (matrix cell, accessibility assertions, the suite's a11y test).
2. `filestore`
3. `network-status` — **done 2026-10-30**: it was the last non-Dioxus SPA, converted and
   wired to `common_ui::tabs` (the `tabs` and `collection` modules exist because of it) and
   `common_ui::checkbox` for the "moving now" control. It had tab ARIA attributes with no
   keyboard handling, which is exactly what the library adds — and `crates/network-status/tests/`
   was added with the conversion so the primitives can be asserted through the path a user
   takes, not through a type check.

   **Restyled 2026-11-09**, which is what found the stylesheet story above. Now also
   `common_ui::toggle_group` (the direction filter and the lease "show" filter are one
   segmented control each — every choice visible, one click to change it, where two of the
   panel's four `<select>`s used to be) and `common_ui::progress` (a lease's countdown is a
   `progressbar` with a value rather than a styled `span`). Its suite is now the one that
   covers four primitives and fourteen checks — the last two of which are the stylesheet guard
   and the painted check, and were verified to fail when a rule is deleted and when a rule is
   emptied — and it is also the SPA whose suite went red silently the same day, because tabs
   were being selected by index: see the tab-selection rule in
   `crates/network-status/tests/README.md`.
4. `gocardless-dashboard`
5. `frigate-monitor`

## Re-verify

A vendored file is *its header plus upstream's body*, so the check is a diff against the
body — `tail -n +N`, where N is that file's header length plus one. Run bare, the command in
this file's own header reported the nine-line header as a deviation on every vendored file,
which is a check that always says "something differs" and therefore says nothing; the
checkbox line below is the corrected form of it.

```bash
PIN=9a758255ea26e2b20c8cecf4c1c946feb9e71da7
U=https://raw.githubusercontent.com/DioxusLabs/dioxus-components/$PIN/primitives/src

# name      header lines in the vendored copy (so: tail -n +N+1) — run from the repo root
# every one of these prints nothing except `checkbox.rs`, whose 20 lines are the one known
# deviation (the skipped `BubbleInput` render) stated where it is made
diff <(curl -s $U/checkbox.rs)     <(tail -n +10 crates/common-rs/ui/src/checkbox.rs)
diff <(curl -s $U/tabs.rs)         <(tail -n +7  crates/common-rs/ui/src/tabs.rs)
diff <(curl -s $U/collection.rs)   <(tail -n +9  crates/common-rs/ui/src/collection.rs)
diff <(curl -s $U/progress.rs)     <(tail -n +8  crates/common-rs/ui/src/progress.rs)
diff <(curl -s $U/toggle.rs)       <(tail -n +7  crates/common-rs/ui/src/toggle.rs)
diff <(curl -s $U/toggle_group.rs) <(tail -n +8  crates/common-rs/ui/src/toggle_group.rs)
```

(The `diff - <(…) <(…)` form is not a command: `-` is a third operand and diff refuses it.
Written here because the sentence above this block used to be that, and it is the kind of
check that looks runnable while never having been run — all six lines above were run today
and the counts in this paragraph are their output.)

Adding a module means: copy it whole, prepend a header that names the pin, the reason and its
own `tail` offset, and add a line here. A vendored file no command re-checks is the next hash
table that quietly stops being true.

```bash
# the gate for a component change — not on the router, which cannot build
just build-frontend switch-gui && just switch-gui-test
just build-frontend network-status && just network-status-test
```
