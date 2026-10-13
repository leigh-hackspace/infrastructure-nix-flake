//! Embedded web assets.  The contents of the SPA bundle are copied into the
//! build and exposed here as a static table by `build.rs` (generated as
//! assets_gen.rs in OUT_DIR; the bundle itself is built by the flake — see
//! `machines/aibox/frigate-monitor.nix`, which sets FRIGATE_MONITOR_DIST — and
//! `frontend/dist` is git-ignored).  When no bundle is present during local
//! development a placeholder index.html is generated instead.

include!(concat!(env!("OUT_DIR"), "/assets_gen.rs"));
