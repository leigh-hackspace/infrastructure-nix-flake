//! Serving the prebuilt SPA: extension → MIME, containment inside
//! `--static-dir`, and the fallback page for when nobody has built it.

/// `<!doctype html>` for a build that has not happened, so the backend is still
/// self-explanatory when run without `--static-dir`.
pub(crate) const FALLBACK_HTML: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>__TITLE__</title></head>
<body style="font:16px system-ui;background:#0d1117;color:#d7e0e8;padding:40px">
<h1>__TITLE__</h1>
<p>The frontend has not been built yet. Build it and point <code>--static-dir</code> at the output:</p>
<pre>cd network-status/frontend &amp;&amp; npm install &amp;&amp; npm run build</pre>
<p>The dashboard will appear once the static files are served.</p>
</body>
</html>
"#;

/// MIME type for a URL path (best-effort, by extension).
pub(crate) fn static_content_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "map" | "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        _ => "application/octet-stream",
    }
}

/// Join `path` onto `dir`, refusing any `..` that would escape the directory.
pub(crate) fn within_dir(dir: &std::path::Path, path: &str) -> Option<std::path::PathBuf> {
    let mut out = std::path::PathBuf::from(dir);
    for comp in path.split(['/', '\\']) {
        match comp {
            "" | "." => {}
            ".." => return None,
            c => out.push(c),
        }
    }
    Some(out)
}

/// Serve a path from `static_dir`, with SPA fallback to index.html.
/// Returns `None` when no static dir is configured.
pub(crate) fn serve_static(static_dir: &Option<String>, path: &str) -> Option<(u16, String, String)> {
    let dir = std::path::Path::new(static_dir.as_ref()?);
    // Drop any query string.
    let clean = path.split('?').next().unwrap_or("/");
    let mut file = within_dir(dir, clean)?;
    if file.is_dir() {
        file.push("index.html");
    }
    match std::fs::read(&file) {
        Ok(bytes) => Some((
            200,
            static_content_type(&file.to_string_lossy()).to_string(),
            String::from_utf8_lossy(&bytes).into_owned(),
        )),
        Err(_) => {
            // SPA fallback: hand back index.html for unrecognised routes.
            match std::fs::read(dir.join("index.html")) {
                Ok(bytes) => Some((
                    200,
                    "text/html; charset=utf-8".to_string(),
                    String::from_utf8_lossy(&bytes).into_owned(),
                )),
                Err(_) => None,
            }
        }
    }
}

