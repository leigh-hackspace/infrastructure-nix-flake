//! `network-status` web UI — a Dioxus (wasm) SPA, the front end of the router's dashboard
//! at `network-info.int.leighhack.org`.
//!
//! One page, four tabs, by question: what the router is doing right now (overview), what
//! the connection table says (`connections.rs`), what the DHCP leases say (`leases.rs`),
//! and which interface is doing it (`charts.rs` + `panels.rs`). The tabs are not by
//! widget — the reason is in `app.rs`.
//!
//! Modules: `api` (the wire contract + fetch), `state` (signals, the poll, the listeners),
//! `format` (number/duration/time), `chart` (the canvas drawing), `charts` (the chart
//! components), `panels` (header/banner/cards/issues), `connections`, `leases`, `app`.
//!
//! The port from SolidJS is 2026-10-30; `README.md` keeps what the port had to preserve
//! and the two things it changed on purpose.

mod api;
mod app;
mod chart;
mod charts;
mod connections;
mod format;
mod leases;
mod panels;
mod state;

use app::App;

/// Wasm entry point: runs when the module initialises (the JS glue's `init()`).
#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen(start))]
pub fn wasm_start() {
    dioxus::launch(App);
}

/// Host entry point (so `cargo run` still type-checks off-wasm).
#[cfg(not(target_arch = "wasm32"))]
pub fn main() {
    dioxus::launch(App);
}
