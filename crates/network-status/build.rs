//! Embeds the Dioxus SPA bundle into the binary; the shared implementation is in
//! `common-rs/build-spa` (see `common/frontend-build-spa.sh` for the build side).
//!
//! `frontend/dist` is git-ignored: the flake builds the wasm and points
//! `NETWORK_STATUS_DIST` at it (see machines/gw/network-status.nix).  A local
//! `just build-frontend network-status` produces the same directory, and a missing one
//! is not an error — the binary then serves the API with no UI, which is what a
//! `cargo test` run wants.
//!
//! This crate is the one that *used* to be the exception: its frontend was SolidJS, its
//! npm deps were not nixpkgs-cached, and so its `dist/` was committed (AGENTS.md rule 19).
//! It is a Dioxus SPA now and follows the same rule as the other four.
fn main() {
    common_build_spa::embed(common_build_spa::Config {
        env_var: "NETWORK_STATUS_DIST",
        dist_rel: "frontend/dist",
        app: "network-status",
    });
}
