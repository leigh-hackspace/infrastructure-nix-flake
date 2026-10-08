//! Embeds the Dioxus SPA bundle into the binary; the shared helper in
//! common-rs/build-spa does the work (see the frigate-monitor service module on
//! aibox for the derivation that builds the bundle and sets
//! FRIGATE_MONITOR_DIST).

fn main() {
    common_build_spa::embed(common_build_spa::Config {
        env_var: "FRIGATE_MONITOR_DIST",
        dist_rel: "frontend/dist",
        app: "frigate-monitor",
    });
}
