//! Embeds the Dioxus SPA bundle into the binary (shared `build.rs` helper).
//!
//! The SPA is a separate wasm crate. In Nix builds the flake compiles it (see
//! `machines/*/services/*.nix`) and points here at the result via the `env_var`
//! named in the `Config`. For local development the bundle is built with
//! `./build.sh` in `frontend/` and picked up from `frontend/dist`; if neither is
//! present a placeholder page is embedded so the crate still compiles.
//!
//! Call it from a `build.rs`:
//!
//! ```ignore
//! fn main() {
//!     common_build_spa::embed(common_build_spa::Config {
//!         env_var: "FILESTORE_DIST",
//!         dist_rel: "frontend/dist",
//!         app: "filestore",
//!     });
//! }
//! ```
//!
//! and pull the generated table in with
//! `include!(concat!(env!("OUT_DIR"), "/assets_gen.rs"));`. It defines
//! `Asset`, `ASSETS` and `find_asset(name)`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
pub struct Config<'a> {
    /// Name of the environment variable the flake sets to the built bundle
    /// directory, e.g. `FILESTORE_DIST`.
    pub env_var: &'a str,
    /// Where the local development bundle lives, relative to the crate root.
    pub dist_rel: &'a str,
    /// App name, used in the placeholder page's title and text.
    pub app: &'a str,
}

fn dist_dir(cfg: Config) -> PathBuf {
    // Nix build: SPA bundle produced by the frontendDist derivation.
    if let Ok(dir) = env::var(cfg.env_var) {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    // Local dev: bundle built by frontend/build.sh.
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest).join(cfg.dist_rel)
}

fn mime_of(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "wasm" => "application/wasm",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn collect(dir: &Path, prefix: &str, files: &mut Vec<(String, String)>) {
    if !dir.is_dir() {
        return;
    }
    let mut entries: Vec<_> = fs::read_dir(dir)
        .expect("read dist")
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            let sub = p.file_name().unwrap().to_string_lossy().to_string();
            collect(&p, &format!("{prefix}{sub}/"), files);
        } else {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            files.push((format!("{prefix}{name}"), name));
        }
    }
}

pub fn embed(cfg: Config) {
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    let dist = dist_dir(cfg);

    let mut files: Vec<(String, String)> = Vec::new(); // (out-path, file-name)
    collect(&dist, "", &mut files);

    let _ = fs::create_dir_all(format!("{out_dir}/assets"));

    let mut entries: Vec<String> = Vec::new();
    for (rel, name) in &files {
        let src = dist.join(rel);
        let dest = format!("{out_dir}/assets/{rel}");
        if let Some(parent) = Path::new(&dest).parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::copy(&src, &dest).expect("copy asset");
        entries.push(format!(
            "Asset {{ name: {:?}, mime: {:?}, data: include_bytes!({:?}) }},",
            rel,
            mime_of(name),
            format!("{out_dir}/assets/{rel}")
        ));
    }

    if entries.is_empty() {
        // Development fallback so the crate still builds before the SPA has
        // been built once (no committed bundle; see dist_dir() above).
        let dest = format!("{out_dir}/assets/index.html");
        fs::write(
            &dest,
            format!(
                "<!doctype html><meta charset=\"utf-8\"><title>{app}</title>\
                 <body style=\"background:#111;color:#eee;font-family:sans-serif\">\
                 <p>{app} web UI not built. Run <code>./build.sh</code> in frontend/ \
                 (local) or set {env} (Nix), then rebuild.</p>",
                app = cfg.app,
                env = cfg.env_var,
            ),
        )
        .expect("write fallback");
        entries.push(format!(
            "Asset {{ name: \"index.html\", mime: \"text/html; charset=utf-8\", data: include_bytes!({:?}) }},",
            dest
        ));
    }

    let gen = format!(
        "pub struct Asset {{ pub name: &'static str, pub mime: &'static str, pub data: &'static [u8] }}\n\
         pub static ASSETS: &[Asset] = &[\n{}\n];\n\
         pub fn find_asset(name: &str) -> Option<&'static Asset> {{\n    ASSETS.iter().find(|a| a.name == name)\n}}",
        entries.join("\n")
    );
    fs::write(format!("{out_dir}/assets_gen.rs"), gen).expect("write assets_gen");

    // Rebuild when the bundle changes (env override or the local dist dir).
    println!("cargo:rerun-if-env-changed={}", cfg.env_var);
    println!("cargo:rerun-if-changed={}", dist.display());
}
