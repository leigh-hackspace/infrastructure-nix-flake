//! Embedded web assets.  The contents of frontend/dist are copied into the
//! build and exposed here as a static table by `build.rs` (see
//! assets_gen.rs in OUT_DIR).  frontend/dist is committed to git (the Dioxus
//! SPA is built locally, not by Nix — same rule as network-status/frontend),
//! so production binaries embed the real SPA; when dist is absent during
//! development a placeholder index.html is generated instead.

include!(concat!(env!("OUT_DIR"), "/assets_gen.rs"));
