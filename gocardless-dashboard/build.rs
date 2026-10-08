//! Embeds the Dioxus SPA bundle into the binary; the shared helper in
//! common-rs/build-spa does the work (see the gocardless-dashboard service
//! module for the derivation that builds the bundle and sets
//! GOCARDLESS_DASHBOARD_DIST).

fn main() {
    common_build_spa::embed(common_build_spa::Config {
        env_var: "GOCARDLESS_DASHBOARD_DIST",
        dist_rel: "frontend/dist",
        app: "gocardless-dashboard",
    });
}
