//! Serving the embedded SPA bundle.
//!
//! The bundle itself is embedded by `build.rs` (`common-rs/build-spa`), so this is the
//! delivery half: name → asset, and the page shown when nothing has been built. `find_asset`
//! is generated into `OUT_DIR` and included here. MIME types are not decided here: `build.rs`
//! labels every asset when it writes the table (`mime_of`, including `application/wasm`), so a
//! second extension→MIME map in this file would be a second answer to the same question
//! (rule 2).
//!
//! ## Why the body is bytes, not a String
//!
//! This crate used to serve a prebuilt SolidJS bundle out of `--static-dir`, and the
//! response plumbing it shared took a `String` body. That was harmless while every asset
//! was text — HTML, CSS, JS — and stopped being harmless the day the bundle became a
//! `.wasm`: a `String` cannot hold a binary file, and a bundle decoded as UTF-8 is either
//! an error or a mangled one. `common_web::Reply` carries `Vec<u8>` for exactly this
//! reason (`crates/common-rs/web/src/lib.rs`, "a server that also serves a compiled SPA
//! bundle wants the opposite for its assets"), so nothing on this path reinterprets the
//! bundle's bytes as text.
//!
//! ## Path containment
//!
//! The old `--static-dir` path rejected any `..` by walking the request path component by
//! component. Serving an embedded asset cannot traverse, because there is no directory to
//! traverse into: `find_asset` matches a name against the table `build.rs` wrote at compile
//! time, so a `..` in a request can only fail to match. Same guarantee, now structural
//! rather than a loop — which is why there is no `within_dir` here any more.

use common_web::Reply;

include!(concat!(env!("OUT_DIR"), "/assets_gen.rs"));

/// The page shown when the SPA has not been built: the API is still useful on its own, so
/// a missing bundle should say so rather than 404.
pub(crate) fn fallback_html(title: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>{title}</title></head>
<body style="font:16px system-ui;background:#0d1117;color:#d7e0e8;padding:40px">
<h1>{title}</h1>
<p>The frontend has not been built yet. Build it and rebuild the binary:</p>
<pre>just build-frontend network-status</pre>
<p>The dashboard will appear once the bundle is embedded.</p>
</body>
</html>"#,
    )
}

/// Serve one embedded asset by name.
pub(crate) fn serve_asset(name: &str) -> Option<Reply> {
    let asset = find_asset(name)?;
    let mut reply = Reply::new(200, asset.mime, asset.data.to_vec());
    // A megabyte of wasm re-fetched on every page load is not "instant". Only the assets
    // are cached: index.html is the file that names them, so caching it would pin a page
    // to a bundle it no longer refers to.
    if !asset.mime.starts_with("text/html") {
        reply.headers.push((
            "Cache-Control".to_string(),
            "public, max-age=604800, immutable".to_string(),
        ));
    }
    Some(reply)
}

/// How many assets this binary was built with. 1 is the placeholder `build.rs` writes
/// when there is no bundle, which is the difference between "the UI is not built" and
/// "the UI is built and this request is broken" — and the startup line prints it.
pub(crate) fn embedded_count() -> usize {
    ASSETS.len()
}

/// The SPA shell, or the "not built yet" page when there is no bundle to serve.
pub(crate) fn page(title: &str) -> Reply {
    match serve_asset("index.html") {
        Some(r) => r,
        None => Reply::html(200, &fallback_html(title)),
    }
}
