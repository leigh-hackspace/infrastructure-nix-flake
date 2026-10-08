//! Embeds the Dioxus SPA bundle into the binary; the shared helper in
//! common-rs/build-spa does the work (see the filestore service module for the
//! derivation that builds the bundle and sets FILESTORE_DIST).

fn main() {
    common_build_spa::embed(common_build_spa::Config {
        env_var: "FILESTORE_DIST",
        dist_rel: "frontend/dist",
        app: "filestore",
    });
}
