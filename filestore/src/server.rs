//! axum server: the JSON API (session-gated), the OIDC endpoints, the
//! streaming downloads (file / ZIP / preview) and the embedded SPA.
//!
//! This is an internal LAN-only service fronted by nginx (forceSSL + LAN
//! ACL), so the server itself is plain HTTP on 127.0.0.1.

use std::future::Future;
use std::io::{self, Read};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::extract::{FromRequestParts, Query, State};
use axum::http::header;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::fsutil::Store;
use common_oidc as oidc;

type Shared = Arc<crate::Shared>;

// ---------------------------------------------------------------------------
// auth

pub struct Authed(pub oidc::Session);

impl FromRequestParts<Shared> for Authed {
    type Rejection = Response;
    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &Shared,
    ) -> Result<Self, Self::Rejection> {
        let shared: &crate::Shared = state.as_ref();
        // Testing escape hatch (--no-auth, loopback only): every request is
        // treated as an authenticated local session so the API can be driven
        // directly from the machine without the OIDC dance.
        if shared.cfg.no_auth {
            return Ok(Authed(oidc::dev_session("local")));
        }
        let cookie = parts
            .headers
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|h| cookie_value(h, shared.oidc.cookie_name()));
        match cookie.and_then(|c| shared.oidc.session_for_cookie(&c)) {
            Some(s) => Ok(Authed(s)),
            None => Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "unauthenticated"})),
            )
                .into_response()),
        }
    }
}

fn cookie_value(header: &str, name: &str) -> Option<String> {
    for kv in header.split(';') {
        if let Some((k, v)) = kv.trim().split_once('=') {
            if k.trim() == name {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// A short random token for upload temp files.  Not a credential — the name only
/// has to be unlikely to collide with a concurrent upload.
fn random_hex(nbytes: usize) -> String {
    let mut b = vec![0u8; nbytes];
    getrandom::getrandom(&mut b).expect("getrandom");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn content_length(headers: &axum::http::HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
}

// ---------------------------------------------------------------------------
// errors

pub struct ApiError {
    pub status: StatusCode,
    pub msg: String,
}

impl ApiError {
    fn bad(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            msg: msg.into(),
        }
    }
    fn not_found(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            msg: msg.into(),
        }
    }
    fn too_large(limit: u64) -> Self {
        Self {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            msg: format!("upload exceeds the {} limit", fmt_limit(limit)),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({"error": self.msg}))).into_response()
    }
}

impl From<String> for ApiError {
    fn from(msg: String) -> Self {
        Self::bad(msg)
    }
}

impl From<io::Error> for ApiError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::NotFound => Self::not_found("no such file or directory"),
            io::ErrorKind::PermissionDenied => Self::bad("permission denied"),
            _ => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                msg: e.to_string(),
            },
        }
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn fmt_limit(n: u64) -> String {
    const U: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

// ---------------------------------------------------------------------------
// query / body types

#[derive(Deserialize)]
struct PathQ {
    path: Option<String>,
}

#[derive(Deserialize)]
struct SearchQ {
    path: Option<String>,
    q: String,
    #[serde(default)]
    deep: bool,
}

#[derive(Deserialize)]
struct OnePath {
    path: String,
}

/// Collect repeated `?path=a&path=b` values.  axum's Query extractor
/// (serde_urlencoded) cannot map repeated keys to a Vec, so the raw query
/// string is parsed by hand for this endpoint only.
fn query_paths(query: &str) -> Vec<String> {
    let mut out = Vec::new();
    for pair in query.split('&') {
        if let Some(v) = pair.strip_prefix("path=") {
            if let Ok(s) = urlencoding::decode(v) {
                let s = s.into_owned();
                if !s.is_empty() {
                    out.push(s);
                }
            }
        }
    }
    out
}

#[derive(Deserialize)]
struct TwoPath {
    from: String,
    to: String,
}

#[derive(Deserialize)]
struct ManyPath {
    paths: Vec<String>,
}

#[derive(Deserialize)]
struct UploadQ {
    /// destination dir (relative), e.g. "photos"
    path: String,
    /// file path relative to the destination dir, e.g. "sub/file.txt"
    name: String,
}

// ---------------------------------------------------------------------------
// routes

pub fn router(shared: Shared) -> Router {
    Router::new()
        .route("/auth/login", get(login))
        .route("/auth/dev-login", get(dev_login))
        .route("/auth/callback", get(callback))
        .route("/auth/logout", get(logout))
        .route("/api/whoami", get(whoami))
        .route("/api/list", get(list))
        .route("/api/search", get(search))
        .route("/api/mkdir", post(mkdir))
        .route("/api/create", post(create))
        .route("/api/rename", post(rename))
        .route("/api/copy", post(copy))
        .route("/api/delete", post(delete))
        .route("/api/upload", post(upload))
        .route("/api/download", get(download))
        .route("/api/zip", get(zip))
        .route("/api/preview", get(preview))
        .fallback(spa)
        .with_state(shared)
}

// --- OIDC ---

/// Testing-only session mint; enabled only with --dev-user.
async fn dev_login(State(shared): State<Shared>) -> Response {
    match &shared.cfg.dev_user {
        None => (StatusCode::NOT_FOUND, "disabled").into_response(),
        Some(username) => {
            let (cookie, _session) = shared.oidc.add_dev_session(username);
            (
                StatusCode::SEE_OTHER,
                [
                    (header::LOCATION, "/".to_string()),
                    (
                        header::SET_COOKIE,
                        format!(
                            "{}={cookie}; Max-Age=43200; Path=/; HttpOnly; SameSite=Lax",
                            shared.oidc.cookie_name()
                        ),
                    ),
                ],
            )
                .into_response()
        }
    }
}

async fn login(State(shared): State<Shared>) -> Response {
    let (url, _state) = shared.oidc.begin_login();
    (StatusCode::SEE_OTHER, [(header::LOCATION, url)]).into_response()
}

async fn logout(State(shared): State<Shared>, headers: axum::http::HeaderMap) -> Response {
    if let Some(cookie) = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| cookie_value(h, shared.oidc.cookie_name()))
    {
        shared.oidc.drop_session(&cookie);
    }
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, "/".to_string()),
            (
                header::SET_COOKIE,
                format!(
                    "{}=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax",
                    shared.oidc.cookie_name()
                ),
            ),
        ],
    )
        .into_response()
}

async fn callback(
    State(shared): State<Shared>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let err = q
        .get("error")
        .cloned()
        .or_else(|| q.get("error_description").map(|d| format!("error: {d}")));
    let code = q.get("code").cloned();
    let state = q.get("state").cloned();

    match (err, code, state) {
        (Some(e), _, _) => (StatusCode::FORBIDDEN, format!("login failed: {e}")).into_response(),
        (None, Some(c), Some(s)) => match shared.oidc.finish_login(&c, &s).await {
            Ok((cookie, _session)) => (
                StatusCode::SEE_OTHER,
                [
                    (header::LOCATION, "/".to_string()),
                    (
                        header::SET_COOKIE,
                        format!(
                            "{}={cookie}; Max-Age=43200; Path=/; HttpOnly; SameSite=Lax",
                            shared.oidc.cookie_name()
                        ),
                    ),
                ],
            )
                .into_response(),
            Err(e) => (StatusCode::FORBIDDEN, format!("login failed: {}", e.0)).into_response(),
        },
        _ => (
            StatusCode::BAD_REQUEST,
            "missing code or state — start login again".to_string(),
        )
            .into_response(),
    }
}

// --- API ---

async fn whoami(Authed(s): Authed) -> Json<Value> {
    Json(json!({"username": s.username, "email": s.email}))
}

async fn list(
    State(shared): State<Shared>,
    Authed(_): Authed,
    Query(q): Query<PathQ>,
) -> ApiResult<Json<Value>> {
    let rel = q.path.unwrap_or_default();
    let abs = shared.store.resolve(&rel)?;
    // download/preview/zip already answer 404 for missing paths; keep list consistent
    if !abs.is_dir() {
        return Err(ApiError::not_found("no such directory"));
    }
    let root = shared.store.root.clone();
    let entries = tokio::task::spawn_blocking(move || Store { root }.list(&abs))
        .await
        .map_err(|e| ApiError::bad(e.to_string()))??;
    Ok(Json(json!({"path": rel, "entries": entries})))
}

async fn search(
    State(shared): State<Shared>,
    Authed(_): Authed,
    Query(q): Query<SearchQ>,
) -> ApiResult<Json<Value>> {
    let needle = q.q.trim().to_string();
    if needle.is_empty() {
        return Err(ApiError::bad("empty query"));
    }
    let rel = q.path.unwrap_or_default();
    let abs = shared.store.resolve(&rel)?;
    let root = shared.store.root.clone();
    let deep = q.deep;
    let out = tokio::task::spawn_blocking(move || {
        let s = Store { root };
        s.search(&abs, &needle, deep, 10_000, 300_000)
    })
    .await
    .map_err(|e| ApiError::bad(e.to_string()))??;
    Ok(Json(json!({"path": rel, "results": out})))
}

async fn mkdir(
    State(shared): State<Shared>,
    Authed(_): Authed,
    Json(b): Json<OnePath>,
) -> ApiResult<Json<Value>> {
    let abs = shared.store.resolve(&b.path)?;
    if abs.exists() {
        return Err(ApiError::bad("already exists"));
    }
    // Recursive so "a/b" works even when "a" does not exist yet.
    tokio::fs::create_dir_all(&abs)
        .await
        .map_err(|e| ApiError::bad(format!("cannot create: {e}")))?;
    Ok(Json(json!({"path": b.path})))
}

async fn create(
    State(shared): State<Shared>,
    Authed(_): Authed,
    Json(b): Json<OnePath>,
) -> ApiResult<Json<Value>> {
    let abs = shared.store.resolve(&b.path)?;
    if abs.exists() {
        return Err(ApiError::bad("already exists"));
    }
    tokio::fs::write(&abs, b"")
        .await
        .map_err(|_| ApiError::bad("parent directory does not exist or cannot be created"))?;
    Ok(Json(json!({"path": b.path})))
}

async fn rename(
    State(shared): State<Shared>,
    Authed(_): Authed,
    Json(b): Json<TwoPath>,
) -> ApiResult<Json<Value>> {
    let from = shared.store.resolve(&b.from)?;
    let to = shared.store.resolve(&b.to)?;
    if to.exists() {
        return Err(ApiError::bad("destination already exists"));
    }
    if from == to {
        return Ok(Json(json!({"ok": true})));
    }
    tokio::fs::rename(&from, &to)
        .await
        .map_err(|_| ApiError::bad("cannot move (source missing?)"))?;
    Ok(Json(json!({"ok": true})))
}

async fn copy(
    State(shared): State<Shared>,
    Authed(_): Authed,
    Json(b): Json<TwoPath>,
) -> ApiResult<Json<Value>> {
    let from = shared.store.resolve(&b.from)?;
    let to = shared.store.resolve(&b.to)?;
    if to.exists() {
        return Err(ApiError::bad("destination already exists"));
    }
    let (f, t) = (from.clone(), to.clone());
    tokio::task::spawn_blocking(move || copy_recursive(&f, &t))
        .await
        .map_err(|e| ApiError::bad(e.to_string()))??;
    Ok(Json(json!({"ok": true})))
}

/// Synchronous recursive copy (run in a blocking task — it is heavy IO
/// and the recursion is unbounded).
fn copy_recursive(from: &std::path::Path, to: &std::path::Path) -> io::Result<()> {
    let meta = std::fs::metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir(to)?;
        let rd = std::fs::read_dir(from)?;
        for e in rd {
            let e = e?;
            copy_recursive(&e.path(), &to.join(e.file_name()))?;
        }
    } else {
        std::fs::copy(from, to).map(|_| ())?;
    }
    Ok(())
}

async fn delete(
    State(shared): State<Shared>,
    Authed(_): Authed,
    Json(b): Json<ManyPath>,
) -> ApiResult<Json<Value>> {
    let mut deleted = 0;
    let mut errors: Vec<String> = Vec::new();
    for p in b.paths {
        match shared.store.resolve(&p) {
            Ok(abs) => match tokio::fs::remove_dir_all(&abs).await {
                Ok(()) => deleted += 1,
                Err(_) => match tokio::fs::remove_file(&abs).await {
                    Ok(()) => deleted += 1,
                    Err(e) => errors.push(format!("{p}: {e}")),
                },
            },
            Err(e) => errors.push(format!("{p}: {e}")),
        }
    }
    if deleted == 0 && !errors.is_empty() {
        return Err(ApiError::bad(errors.join("; ")));
    }
    Ok(Json(json!({"deleted": deleted, "errors": errors})))
}

/// Raw-body upload.  The body is streamed to a temp file in the destination
/// directory, then renamed into place (atomic; no partial files).
async fn upload(
    State(shared): State<Shared>,
    Authed(_): Authed,
    headers: axum::http::HeaderMap,
    Query(q): Query<UploadQ>,
    body: Body,
) -> ApiResult<Json<Value>> {
    let limit = shared.cfg.max_upload;
    // Check Content-Length first so an oversized upload is refused before any
    // bytes are written, then guard the stream as well (chunked bodies have no
    // Content-Length).
    if content_length(&headers) > Some(limit) {
        return Err(ApiError::too_large(limit));
    }

    let abs = shared.store.resolve(&join_rel(&q.path, &q.name))?;
    if abs.exists() {
        return Err(ApiError::bad("a file with that name already exists"));
    }

    let parent = abs
        .parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| ApiError::bad("invalid path"))?;
    tokio::fs::create_dir_all(&parent).await?;

    let token = random_hex(8);
    let tmp = parent.join(format!(".filestore-uploading-{token}"));
    let mut guard = UploadGuard {
        tmp: tmp.clone(),
        done: false,
    };

    let mut stream = body.into_data_stream();
    let mut file = tokio::fs::File::create(&tmp).await?;
    use tokio::io::AsyncWriteExt;
    let mut written: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|_| ApiError::bad("upload stream error"))?;
        written += bytes.len() as u64;
        if written > limit {
            return Err(ApiError::too_large(limit));
        }
        file.write_all(&bytes).await?;
    }
    file.flush().await?;
    drop(file);
    tokio::fs::rename(&tmp, &abs).await?;
    guard.done = true;

    Ok(Json(json!({"path": join_rel(&q.path, &q.name)})))
}

/// Removes the temp file if the upload is dropped before completion.
struct UploadGuard {
    tmp: std::path::PathBuf,
    done: bool,
}
impl Drop for UploadGuard {
    fn drop(&mut self) {
        if !self.done {
            let _ = std::fs::remove_file(&self.tmp);
        }
    }
}

/// Join a dir and a relative name with clean "/" separators.
fn join_rel(dir: &str, name: &str) -> String {
    let mut s = dir.trim_end_matches('/').to_string();
    if !s.is_empty() {
        s.push('/');
    }
    s.push_str(name);
    s
}

// --- streaming downloads ---

fn content_disposition(name: &str) -> String {
    // RFC 5987 form for non-ASCII names.
    let ascii: String = name
        .chars()
        .map(|c| if c.is_ascii() { c } else { '?' })
        .collect();
    format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        ascii,
        urlencoding::encode(name)
    )
}

async fn file_path(
    shared: &Shared,
    q: &PathQ,
) -> ApiResult<(std::path::PathBuf, std::fs::Metadata)> {
    let rel = q.path.clone().unwrap_or_default();
    let abs = shared.store.resolve(&rel)?;
    let meta = match tokio::fs::metadata(&abs).await {
        Ok(m) => m,
        Err(_) => return Err(ApiError::not_found("no such file")),
    };
    Ok((abs, meta))
}

async fn download(
    State(shared): State<Shared>,
    Authed(_): Authed,
    Query(q): Query<PathQ>,
) -> ApiResult<Response> {
    let (abs, meta) = file_path(&shared, &q).await?;
    if meta.is_dir() {
        return Err(ApiError::bad("use the zip endpoint for directories"));
    }
    let name = abs
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let mime = mime_guess::from_path(&abs).first_or_octet_stream();
    let stream = FileStream::spawn(abs, 1 << 20)?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (header::CONTENT_LENGTH, meta.len().to_string()),
            (header::CONTENT_DISPOSITION, content_disposition(&name)),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}

async fn zip(State(shared): State<Shared>, Authed(_): Authed, uri: Uri) -> ApiResult<Response> {
    let paths = query_paths(uri.query().unwrap_or(""));
    if paths.is_empty() {
        return Err(ApiError::bad("no paths"));
    }
    let mut targets = Vec::new();
    for p in &paths {
        let abs = shared.store.resolve(p)?;
        match tokio::fs::metadata(&abs).await {
            Ok(_) => targets.push(abs),
            Err(_) => return Err(ApiError::not_found(format!("no such file: {p}"))),
        }
    }
    let name = match targets.len() {
        1 => targets[0]
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "archive".to_string()),
        _ => "archive".to_string(),
    };
    let stream = crate::zipstream::zip(&targets);
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (
                header::CONTENT_DISPOSITION,
                content_disposition(&format!("{name}.zip")),
            ),
        ],
        Body::from_stream(stream.map(|r| match r {
            Ok(b) => Ok::<Bytes, io::Error>(b),
            Err(e) => Err(io::Error::other(e)),
        })),
    )
        .into_response())
}

/// Preview: text files are returned truncated (first 512 KiB, lossy UTF-8);
/// images are streamed raw so the browser can render them in an <img>.
async fn preview(
    State(shared): State<Shared>,
    Authed(_): Authed,
    Query(q): Query<PathQ>,
) -> ApiResult<Response> {
    const MAX_TEXT: u64 = 512 * 1024;
    let (abs, meta) = file_path(&shared, &q).await?;
    if meta.is_dir() {
        return Err(ApiError::bad("cannot preview a directory"));
    }

    let mime = mime_guess::from_path(&abs).first_or_octet_stream();
    let kind = if mime.as_ref().starts_with("image/") {
        "image"
    } else {
        "text"
    };

    if kind == "image" && mime.as_ref() == "image/svg+xml" {
        // SVG is XML — don't render it directly (script risk).
        return Err(ApiError::bad("SVG preview not supported"));
    }

    if kind == "image" {
        let stream = FileStream::spawn(abs, 1 << 20)?;
        return Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, mime.to_string()),
                (header::CONTENT_LENGTH, meta.len().to_string()),
                (header::CACHE_CONTROL, "private, max-age=60".to_string()),
            ],
            Body::from_stream(stream),
        )
            .into_response());
    }

    // text: read the head synchronously in a blocking task (bounded size).
    let n = std::cmp::min(meta.len(), MAX_TEXT);
    let truncated = meta.len() > MAX_TEXT;
    let (bytes, bin) = tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut f = std::fs::File::open(&abs).map_err(io::Error::other)?;
        let mut buf = vec![0u8; n as usize];
        let got = f.read_exact(&mut buf).map(|()| buf.len()).unwrap_or(0);
        let buf = buf[..got].to_vec();
        let bin = buf.iter().take(8).any(|&b| b == 0);
        Ok::<(Vec<u8>, bool), io::Error>((buf, bin))
    })
    .await
    .map_err(|e| ApiError::bad(e.to_string()))??;
    if bin {
        return Err(ApiError::bad("binary files cannot be previewed as text"));
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(Json(json!({"kind": "text", "truncated": truncated, "text": text})).into_response())
}

// --- a Stream over a file's contents ---

/// A Stream over the file's contents, read on a blocking thread in chunks.
/// (Simpler and equally fast for large NAS files than polling tokio's file
/// IO from the async runtime; the bounded channel gives natural
/// backpressure.)
struct FileStream {
    rx: tokio::sync::mpsc::Receiver<Result<Bytes, io::Error>>,
}

impl FileStream {
    fn spawn(path: std::path::PathBuf, chunk: usize) -> ApiResult<Self> {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, io::Error>>(16);
        std::thread::spawn(move || {
            let mut f = match std::fs::File::open(&path) {
                Ok(f) => f,
                Err(e) => {
                    let _ = tx.blocking_send(Err(e));
                    return;
                }
            };
            let mut buf = vec![0u8; chunk];
            loop {
                match f.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx
                            .blocking_send(Ok(Bytes::copy_from_slice(&buf[..n])))
                            .is_err()
                        {
                            break; // client went away
                        }
                    }
                    Err(e) => {
                        let _ = tx.blocking_send(Err(e));
                        break;
                    }
                }
            }
        });
        Ok(Self { rx })
    }
}

impl Stream for FileStream {
    type Item = Result<Bytes, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut fut = Box::pin(self.rx.recv());
        Pin::as_mut(&mut fut).poll(cx)
    }
}

// --- SPA ---

async fn spa(uri: Uri) -> Response {
    let path = uri.path();
    let asset = if path == "/" {
        "index.html"
    } else {
        path.trim_start_matches('/')
    };
    if let Some(a) = crate::find_asset(asset) {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, a.mime.to_string())],
            a.data.to_vec(),
        )
            .into_response();
    }
    // Client-side routes / refreshes: serve index.html for extension-less
    // GET paths.
    let is_file = path.rsplit('/').next().is_some_and(|s| s.contains('.'));
    if !is_file {
        if let Some(a) = crate::find_asset("index.html") {
            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, a.mime.to_string())],
                a.data.to_vec(),
            )
                .into_response();
        }
    }
    (StatusCode::NOT_FOUND, "not found").into_response()
}
