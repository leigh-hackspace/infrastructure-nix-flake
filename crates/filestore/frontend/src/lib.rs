//! filestore web UI — a Dioxus (wasm) SPA.
//!
//! A Windows-Explorer-style file browser: toolbar, breadcrumb address bar,
//! icon grid / sortable details view, right-click context menus, multi-
//! selection with bulk actions, HTML5 drag-and-drop (external files and
//! folders via webkitGetAsEntry, internal move/copy by dragging), popup
//! previews (image, video, audio, PDF, text; left/right step through the folder),
//! shallow/deep name search, type-ahead jump by first letter, the current folder
//! mirrored in the URL hash so a hard refresh lands back where you were, and ZIP
//! downloads of single or multiple items.
//!
//! Modules: api (HTTP client), state (shared state + helpers), js (JS
//! glue: uploads + external drops), grid (icon/details views), ui (app
//! shell: toolbar/nav/search), menu (context menu), modal (dialogs),
//! preview (popup previews), searchview (results), misc (toasts/uploads).

#![allow(clippy::too_many_arguments)]

mod api;
mod grid;
mod js;
mod menu;
mod misc;
mod modal;
mod preview;
mod searchview;
mod state;
mod ui;

use ui::App;
use wasm_bindgen::prelude::*;

pub fn launch() {
    dioxus::launch(App);
}

// `wasm-bindgen --target web` has no entry point of its own: index.html calls
// the generated `init()` and this `start` hook launches the app.
#[cfg(target_family = "wasm")]
#[wasm_bindgen(start)]
pub fn main() {
    launch();
}
