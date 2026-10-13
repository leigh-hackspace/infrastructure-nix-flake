//! Embeds the Dioxus SPA bundle into the binary; the shared implementation is in
//! `common-rs/build-spa` (see `common/frontend-build-spa.sh` for the build side).
//!
//! `frontend/dist` is git-ignored: the flake builds the wasm and points
//! `SWITCH_GUI_DIST` at it (see machines/services1/services/switch-gui.nix).  A
//! local `just build-frontend switch-gui` produces the same directory, and a
//! missing one is not an error — the binary then serves the API with no UI, which
//! is what a `cargo test` run wants.
fn main() {
    common_build_spa::embed(common_build_spa::Config {
        env_var: "SWITCH_GUI_DIST",
        dist_rel: "frontend/dist",
        app: "switch-gui",
    });
}
