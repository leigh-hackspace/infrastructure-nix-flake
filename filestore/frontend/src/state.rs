//! Shared app state, navigation, data loading, toasts and formatting.

use std::collections::HashSet;
use std::time::Duration;

use dioxus::prelude::*;
use gloo_timers::future::sleep;
use serde::Deserialize;

use crate::api::*;
use common_preview::Kind;

/// Fire-and-forget task on the ROOT scope.
///
/// Plain `spawn` attaches the task to the *current component's* scope, and dioxus
/// cancels a task when its component is dropped.  These handlers change state that
/// unmounts the very component that handled the event (IconGrid -> "loading…", the
/// context menu closing, the modal closing), which silently killed the request.
/// One-shot API calls must not be tied to a view's lifetime, so they go on the root
/// scope instead.
pub fn spawn_task(fut: impl std::future::Future<Output = ()> + 'static) {
    dioxus::core::spawn_forever(fut);
}

// ---------------------------------------------------------------------------
// value types

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    Name,
    Size,
    Mtime,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Icons,
    Details,
}

#[derive(Clone, PartialEq)]
pub struct SearchView {
    pub scope: String,
    pub q: String,
    pub deep: bool,
    pub searching: bool,
    pub error: Option<String>,
    pub hits: Vec<Hit>,
    pub truncated: bool,
    pub scanned: u64,
}

#[derive(Clone, PartialEq)]
pub struct CtxMenu {
    pub x: f64,
    pub y: f64,
    /// None = background (empty area), Some = a row's name
    pub target: Option<String>,
}

#[derive(Clone, PartialEq)]
pub enum Modal {
    NewFolder,
    NewFile,
    Rename(String),
    Delete(Vec<String>),
}

#[derive(Clone, PartialEq)]
pub enum PreviewState {
    Loading,
    /// Media the browser renders itself from the preview URL: an <img>, a
    /// <video>, an <audio>, or the PDF viewer in an <iframe>.
    Render(Kind),
    Text(String, bool),
    Error(String),
}

#[derive(Clone, PartialEq)]
pub struct Preview {
    pub path: String,
    pub name: String,
    pub state: PreviewState,
}

#[derive(Clone, PartialEq, Deserialize)]
pub struct Upload {
    pub rel: String,
    pub pct: f64,
    pub done: bool,
    pub error: Option<String>,
}

#[derive(Clone, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub msg: String,
    pub is_err: bool,
}

// ---------------------------------------------------------------------------
// state

/// All fields are Copy signals, so the whole bundle is Copy — event-handler
/// closures can each capture their own copy.
#[derive(Clone, Copy, PartialEq)]
pub struct AppState {
    pub whoami: Signal<Option<Whoami>>,
    pub path: Signal<String>,
    pub entries: Signal<Vec<Entry>>,
    pub loading: Signal<bool>,
    pub sel: Signal<HashSet<String>>,
    /// Index (into `grid::sorted()`) of the row the last click/arrow landed on.
    /// Doubles as the anchor for shift-click / shift-arrow ranges.
    pub focus: Signal<Option<usize>>,
    pub sort_field: Signal<SortField>,
    pub sort_asc: Signal<bool>,
    pub view: Signal<View>,
    pub search: Signal<Option<SearchView>>,
    pub ctx: Signal<Option<CtxMenu>>,
    pub modal: Signal<Option<Modal>>,
    pub preview: Signal<Option<Preview>>,
    pub uploads: Signal<Vec<Upload>>,
    pub toasts: Signal<Vec<Toast>>,
    pub hist: Signal<Vec<String>>,
    pub hidx: Signal<usize>,
    /// Thumbnail URLs the server has refused (not decodable, over the size limit,
    /// or the cache is disabled).  Remembered so a row that cannot be thumbnailed
    /// falls back to the emoji once instead of re-requesting on every render.
    pub thumb_failed: Signal<HashSet<String>>,
}

thread_local! {
    static APP: std::cell::RefCell<Option<AppState>> = const { std::cell::RefCell::new(None) };
}

impl AppState {
    pub fn new() -> Self {
        let st = Self {
            whoami: Signal::new(None),
            path: Signal::new(String::new()),
            entries: Signal::new(Vec::new()),
            loading: Signal::new(true),
            sel: Signal::new(HashSet::new()),
            focus: Signal::new(None),
            sort_field: Signal::new(SortField::Name),
            sort_asc: Signal::new(true),
            view: Signal::new(View::Icons),
            search: Signal::new(None),
            ctx: Signal::new(None),
            modal: Signal::new(None),
            preview: Signal::new(None),
            uploads: Signal::new(Vec::new()),
            toasts: Signal::new(Vec::new()),
            hist: Signal::new(vec![String::new()]),
            hidx: Signal::new(0),
            thumb_failed: Signal::new(HashSet::new()),
        };
        APP.with(|a| *a.borrow_mut() = Some(st));
        st
    }

    pub fn global() -> Option<AppState> {
        APP.with(|a| *a.borrow())
    }
}

// ---------------------------------------------------------------------------
// navigation

/// Bump a nav change into the back/forward history and load the dir.
pub fn navigate(mut st: AppState, path: &str) {
    let path = path.to_string();
    let mut hist = st.hist.read().to_vec();
    let hidx = *st.hidx.read();
    hist.truncate(hidx + 1);
    hist.push(path.clone());
    let new_idx = hist.len() - 1;
    st.hist.set(hist);
    st.hidx.set(new_idx);
    st.sel.set(HashSet::new());
    st.focus.set(None);
    st.search.set(None);
    st.ctx.set(None);
    st.path.set(path);
    sync_url(&st);
    load_dir(st);
}

/// Restore the folder from the URL hash on first load.
///
/// It becomes the *current* history entry: the root you never visited should not
/// be a step back.
pub fn restore_from_url(mut st: AppState) {
    let p = url_path();
    if p.is_empty() {
        load_dir(st);
        return;
    }
    st.hist.set(vec![p.clone()]);
    st.hidx.set(0);
    st.path.set(p);
    sync_url(&st);
    load_dir(st);
}

/// Mirror the current path into the URL hash, so a hard refresh lands back where
/// you were.
///
/// `replaceState`, not `location.hash = …`: the app keeps its own back/forward
/// history (`hist`/`hidx`, Backspace), and letting the browser build a second one
/// would make the two disagree about where you are.
pub fn sync_url(st: &AppState) {
    let js = format!(
        "(function(){{var h={p}===''?'':'#'+encodeURIComponent({p});history.replaceState(null,'',location.pathname+h);}})()",
        p = serde_json::json!(st.path.read().clone())
    );
    let _ = js_eval(&js);
}

/// The path in the URL hash, or "" for the store root.
pub fn url_path() -> String {
    js_eval_string(
        "(function(){{try{{return decodeURIComponent((location.hash || '').replace(/^#/, ''));}}catch(e){{return '';}}}})()",
    )
    .unwrap_or_default()
}

pub fn go(mut st: AppState, delta: i64) {
    let hidx = (*st.hidx.read() as i64 + delta).max(0);
    let hist = st.hist.read().clone();
    if (hidx as usize) >= hist.len() {
        return;
    }
    let path = hist[hidx as usize].clone();
    st.hidx.set(hidx as usize);
    st.sel.set(HashSet::new());
    st.focus.set(None);
    st.search.set(None);
    st.ctx.set(None);
    st.path.set(path);
    sync_url(&st);
    load_dir(st);
}

// ---------------------------------------------------------------------------
// data loading

#[derive(Deserialize)]
struct ListResp {
    path: String,
    entries: Vec<Entry>,
}

pub fn load_dir(mut st: AppState) {
    st.loading.set(true);
    let path = st.path.read().clone();
    spawn_task(async move {
        match get_json::<ListResp>(&format!("/api/list?path={}", enc_path(&path))).await {
            Ok(r) => st.entries.set(r.entries),
            Err(e) => {
                if e == "unauthenticated" {
                    redirect("/auth/login");
                    return;
                }
                toast(st, &format!("load {path}: {e}"), true);
            }
        }
        st.loading.set(false);
    });
}

pub fn run_search(mut st: AppState, scope: String, q: String, deep: bool) {
    st.ctx.set(None);
    st.search.set(Some(SearchView {
        scope: scope.clone(),
        q: q.clone(),
        deep,
        searching: true,
        error: None,
        hits: Vec::new(),
        truncated: false,
        scanned: 0,
    }));
    spawn_task(async move {
        let url = format!(
            "/api/search?path={}&q={}&deep={}",
            enc_path(&scope),
            q.split('/').map(encode_component).collect::<Vec<_>>().join("/"),
            deep
        );
        match get_json::<SearchResult>(&url).await {
            Ok(r) => {
                let cur = st.search.read().clone();
                if let Some(mut sv) = cur {
                    sv.searching = false;
                    sv.hits = r.results.hits;
                    sv.truncated = r.results.truncated;
                    sv.scanned = r.results.scanned;
                    st.search.set(Some(sv));
                }
            }
            Err(e) => {
                let cur = st.search.read().clone();
                if let Some(mut sv) = cur {
                    sv.searching = false;
                    sv.error = Some(e);
                    st.search.set(Some(sv));
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// toasts

static TOAST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn toast(mut st: AppState, msg: &str, is_err: bool) {
    let id = TOAST_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut list = st.toasts.read().clone();
    list.push(Toast { id, msg: msg.to_string(), is_err });
    st.toasts.set(list);
    spawn_task(async move {
        sleep(Duration::from_millis(4500)).await;
        let mut list = st.toasts.read().clone();
        list.retain(|x| x.id != id);
        st.toasts.set(list);
    });
}

/// Trigger a browser download (same-origin; the session cookie is sent).
pub fn download(url: &str) {
    let js = format!(
        "(function(){{var a=document.createElement('a');a.href={url};document.body.appendChild(a);a.click();a.remove();}})()",
        url = serde_json::json!(url)
    );
    let _ = js_eval(&js);
}

pub fn zip_selected(st: AppState, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    download(&zip_url(&paths));
    toast(st, &format!("zipping {} item(s)…", paths.len()), false);
}

/// A thumbnail that came back broken.  The row falls back to the emoji icon, and
/// the URL is remembered so the same broken file is not re-requested on every
/// render.
///
/// Keyed by URL rather than by path, because the URL carries the file's identity
/// (see `api::thumb_url`): when the file changes the URL changes, so a file that
/// becomes decodable gets another try instead of staying stuck on the emoji.
pub fn mark_thumb_failed(mut st: AppState, url: &str) {
    st.thumb_failed.write().insert(url.to_string());
}

/// Scroll the row with this name into view (arrow-key navigation should not
/// move the selection somewhere the user cannot see).
pub fn scroll_row_into_view(name: &str) {
    let js = format!(
        "(function(){{var r=document.querySelector('[data-fs-name=' + JSON.stringify({n}) + ']');if (r) r.scrollIntoView({{block:'nearest',inline:'nearest'}});}})()",
        n = serde_json::json!(name)
    );
    let _ = js_eval(&js);
}

pub fn redirect(url: &str) {
    let js = format!("window.location.href = {};", serde_json::json!(url));
    let _ = js_eval(&js);
}

// ---------------------------------------------------------------------------
// icons + formatting

pub fn icon_for(name: &str, is_dir: bool) -> &'static str {
    if is_dir {
        return "📁";
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" | "ico" => "🖼️",
        "txt" | "md" | "log" | "rst" | "tex" => "📝",
        "csv" | "tsv" => "📊",
        "json" | "yaml" | "yml" | "toml" | "ini" | "conf" => "⚙️",
        "rs" | "go" | "py" | "c" | "h" | "cpp" | "hpp" | "js" | "ts" | "sh" | "nix" | "rb" | "java" => "💻",
        "html" | "css" => "🌐",
        "mp3" | "flac" | "ogg" | "wav" | "m4a" => "🎵",
        "mp4" | "mkv" | "webm" | "mov" | "avi" => "🎬",
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" | "tgz" => "📦",
        "pdf" => "📕",
        "doc" | "docx" | "odt" | "ppt" | "pptx" | "odp" | "xls" | "xlsx" | "ods" => "📄",
        "iso" | "img" => "💿",
        _ => "📄",
    }
}

/// Whether a row offers "Preview" (and what double-click / Enter does with it).
///
/// The table is the shared one in `common-rs/preview`, so the UI cannot advertise
/// something the API refuses — when this was a hand-copied list it did exactly
/// that for SVG (bug #6 in `filestore/tests/README.md`).
pub fn is_previewable(name: &str) -> bool {
    common_preview::kind(name).is_some()
}

pub fn fmt_size(n: u64) -> String {
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

pub fn js_eval(js: &str) -> Option<wasm_bindgen::JsValue> {
    js_sys::eval(js).ok()
}

pub fn js_eval_string(js: &str) -> Option<String> {
    js_eval(js)?.as_string()
}

pub fn fmt_time(t: i64) -> String {
    let js = format!(
        "(function(){{var d=new Date({t}*1000);var p=function(n){{return n<10?'0'+n:''+n;}};return d.getFullYear()+'-'+p(d.getMonth()+1)+'-'+p(d.getDate())+' '+p(d.getHours())+':'+p(d.getMinutes());}})()"
    );
    js_eval_string(&js).unwrap_or_else(|| t.to_string())
}

/// Whether a dioxus MouseEvent was triggered by the left mouse button.
/// (`trigger_button` is `None` for non-button events like move/enter.)
pub fn is_left_click(e: &dioxus::events::MouseEvent) -> bool {
    matches!(
        e.trigger_button(),
        None | Some(dioxus::html::input_data::MouseButton::Primary)
    )
}
