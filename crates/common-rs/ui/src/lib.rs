//! `common-ui` — the component library the Dioxus SPAs use.
//!
//! KIND: VENDORED CRATE. The widgets here are not ours: they are a pruned copy of
//! the unstyled `primitives` crate of **`dioxus-components`** (DioxusLabs), chosen
//! because it is the only component library in the Dioxus world with a test suite
//! that actually runs — 34 Playwright specs against WAI-ARIA authoring practices,
//! sharded in CI, plus a job that adds every component and re-checks it. The
//! narrative, the candidates that were measured and rejected, and the order the
//! SPAs get converted in: **`docs/component-library.md`**.
//!
//! ## The pin
//!
//! `github:DioxusLabs/dioxus-components` @ `9a758255ea26e2b20c8cecf4c1c946feb9e71da7`
//! (2026-09-08), `primitives/src/`.
//!
//! The pin is a git ref, not a crate version, because the crate is not usable on
//! crates.io: `dioxus-primitives` has exactly one published version, `0.0.0`, with
//! no dependencies — its `lib.rs` is `pub fn add(left, right)`, a name reservation
//! (measured by downloading the `.crate` tarball: 884 bytes). So the source is
//! copied here once, and each SPA path-depends on this crate through the
//! symlink/`sharedCrates` machinery (AGENTS.md rule 2: one copy, never five).
//!
//! Re-verify a file against upstream before changing it:
//!
//! ```bash
//! curl -s https://raw.githubusercontent.com/DioxusLabs/dioxus-components/9a758255ea26e2b20c8cecf4c1c946feb9e71da7/primitives/src/checkbox.rs \
//!   | diff - src/checkbox.rs
//! ```
//!
//! ## Why only some modules
//!
//! Only what a SPA uses, and the hooks those modules need. Pruning is what keeps
//! every file inside rule 26 — upstream's `calendar.rs` is 91 KB — and keeps the
//! dependency graph to `dioxus` alone. The whole crate would also pull `palette`,
//! `num-integer`, `serde`, `dioxus-sdk-time` and a `build.rs` that shells out to
//! **`bun`** (a TypeScript→JS step; it is a no-op while the committed hash matches,
//! so the pinned tree must not have its `.ts` files edited) into every bundle.
//!
//! Two files upstream are deliberately not here, for the same reason:
//! `dioxus-attributes` (a proc-macro crate, unpublished) — used by `tooltip`,
//! `dropdown_menu`, `select`; and `src/ts/focus-trap.ts` + its generated JS —
//! `dialog` calls `window.createFocusTrap`, which is a **runtime asset**, and
//! `common/frontend-build-spa.sh` emits only the wasm, its glue and `index.html`.
//! A SPA that wants `dialog` has to add that asset to the build recipe first.
//!
//! ## The one deviation
//!
//! `use_unique_id` below is upstream's, minus the `fullstack!` block. That block is
//! the SSR half of a client/server split: on a plain wasm build it is inert, and
//! keeping it would need the `fullstack` feature, which drags `axum`/`reqwest` into
//! a wasm bundle for nothing. None of these SPAs render on a server.

use dioxus::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};

pub mod checkbox;

/// Generate a runtime-unique id.
fn use_unique_id() -> Signal<String> {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

    #[allow(unused_mut)]
    let mut initial_value = use_hook(|| {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let id_str = format!("dxc-{id}");
        id_str
    });

    use_signal(|| initial_value)
}

/// Allows some state to be either controlled or uncontrolled.
pub fn use_controlled<T: Clone + PartialEq + 'static>(
    prop: ReadSignal<Option<T>>,
    default: T,
    on_change: Callback<T>,
) -> (Memo<T>, Callback<T>) {
    let mut internal_value = use_signal(|| prop.cloned().unwrap_or(default));
    let value = use_memo(move || prop.cloned().unwrap_or_else(&*internal_value));

    let set_value = use_callback(move |x: T| {
        internal_value.set(x.clone());
        on_change.call(x);
    });

    (value, set_value)
}
