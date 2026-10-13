//! API client for the filestore backend.

use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Hit {
    pub rel: String,
    pub parent: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SearchOutcome {
    pub hits: Vec<Hit>,
    pub truncated: bool,
    pub scanned: u64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SearchResult {
    pub path: String,
    pub results: SearchOutcome,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Whoami {
    pub username: String,
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Deserialize)]
pub struct ApiErr {
    pub error: String,
}

fn err_from_resp(resp: &gloo_net::http::Response) -> String {
    if resp.status() == 401 {
        return "unauthenticated".to_string();
    }
    format!("HTTP {}", resp.status())
}

/// Fetch a JSON API endpoint (same-origin; the session cookie is sent).
pub async fn get_json<T: for<'de> Deserialize<'de>>(url: &str) -> Result<T, String> {
    let resp = gloo_net::http::Request::get(url)
        .send()
        .await
        .map_err(|e| format!("{url}: {e}"))?;
    if !resp.ok() {
        let msg = resp
            .json::<ApiErr>()
            .await
            .ok()
            .map(|e| e.error)
            .unwrap_or_else(|| err_from_resp(&resp));
        return Err(msg);
    }
    resp.json::<T>().await.map_err(|e| format!("bad json: {e}"))
}

pub async fn post_json(url: &str, body: &serde_json::Value) -> Result<(), String> {
    let req = gloo_net::http::Request::post(url)
        .json(body)
        .map_err(|e| format!("bad body: {e}"))?;
    let resp = req.send().await.map_err(|e| format!("{url}: {e}"))?;
    if !resp.ok() {
        let msg = resp
            .json::<ApiErr>()
            .await
            .ok()
            .map(|e| e.error)
            .unwrap_or_else(|| err_from_resp(&resp));
        return Err(msg);
    }
    Ok(())
}

/// Upload one file as the request body.

/// Join a dir and a relative name with clean "/" separators.
pub fn join_rel(dir: &str, name: &str) -> String {
    let mut s = dir.trim_end_matches('/').to_string();
    if !s.is_empty() {
        s.push('/');
    }
    s.push_str(name);
    s
}

/// Percent-encode a path segment.
pub fn encode_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// URL-encode a relative path for a query string (keep "/" as-is).
pub fn enc_path(p: &str) -> String {
    p.split('/')
        .map(encode_component)
        .collect::<Vec<_>>()
        .join("/")
}

pub fn download_url(path: &str) -> String {
    format!("/api/download?path={}", enc_path(path))
}

/// Thumbnail URL for an image row (see `icon_for`).
///
/// `fingerprint` is the file's identity as the API reports it
/// (`<mtime>-<ctime>-<size>-<inode>`), and it is what keeps a thumbnail from ever
/// being stale: the browser caches by URL, so a file that changes gets a different
/// URL and its stored thumbnail can never be shown for the new state.  The server
/// hashes the same identity for its own cache and recomputes it from the current
/// stat, so a changed file regenerates whatever `v` says.  Non-image extensions get
/// no thumbnail at all (the SPA keeps the emoji).
pub fn thumb_url(path: &str, fingerprint: &str) -> String {
    format!("/api/thumb?path={}&v={}", enc_path(path), fingerprint)
}

/// Zip one or more paths (repeated `path` query params).
pub fn zip_url(paths: &[String]) -> String {
    let q = paths
        .iter()
        .map(|p| format!("path={}", enc_path(p)))
        .collect::<Vec<_>>()
        .join("&");
    format!("/api/zip?{q}")
}

pub fn preview_url(path: &str) -> String {
    format!("/api/preview?path={}", enc_path(path))
}
