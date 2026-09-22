//! Embeds the Dioxus SPA bundle into the binary.
//!
//! The SPA is a separate wasm crate.  In Nix builds the flake compiles it
//! (machines/aibox/frigate-monitor.nix) and points here at the result via
//! FRIGATE_MONITOR_DIST.  For local development the bundle is built with
//! ./build.sh in frontend/ and picked up from frontend/dist; if neither is
//! present a placeholder page is embedded so the crate still compiles.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const DIST: &str = "frontend/dist";

fn dist_dir() -> PathBuf {
    // Nix build: SPA bundle produced by the frontendDist derivation.
    if let Ok(dir) = env::var("FRIGATE_MONITOR_DIST") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    // Local dev: bundle built by frontend/build.sh.
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest).join(DIST)
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

fn main() {
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    let dist = dist_dir();

    let mut files: Vec<(String, String)> = Vec::new(); // (out-path, mime-key)
    let mut fallback = false;

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
    collect(&dist, "", &mut files);

    let assets_out = out_dir.clone();
    let _ = fs::create_dir_all(format!("{assets_out}/assets"));

    let mut entries: Vec<String> = Vec::new();
    for (rel, name) in &files {
        let src = dist.join(rel);
        let dest = format!("{assets_out}/assets/{rel}");
        if let Some(parent) = Path::new(&dest).parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::copy(&src, &dest).expect("copy asset");
        entries.push(format!(
            "Asset {{ name: {:?}, mime: {:?}, data: include_bytes!({:?}) }},",
            rel,
            mime_of(name),
            format!("{assets_out}/assets/{rel}")
        ));
    }

    if entries.is_empty() {
        // Development fallback so the crate still builds before the SPA has
        // been built once (no committed bundle; see dist_dir() above).
        fallback = true;
        let dest = format!("{assets_out}/assets/index.html");
        fs::write(
            &dest,
            "<!doctype html><meta charset=\"utf-8\"><title>frigate-monitor</title>\
             <body style=\"background:#111;color:#eee;font-family:sans-serif\">\
             <p>frigate-monitor web UI not built. Run <code>./build.sh</code> in frontend/ \
             (local) or set FRIGATE_MONITOR_DIST (Nix), then rebuild.</p>",
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
         pub fn find(name: &str) -> Option<&'static Asset> {{\n    ASSETS.iter().find(|a| a.name == name)\n}}",
        entries.join("\n")
    );
    fs::write(format!("{assets_out}/assets_gen.rs"), gen).expect("write assets_gen");

    // Rebuild when the bundle changes (env override or the local dist dir).
    println!("cargo:rerun-if-env-changed=FRIGATE_MONITOR_DIST");
    println!("cargo:rerun-if-changed={}", dist.display());
    let _ = fallback;
}
