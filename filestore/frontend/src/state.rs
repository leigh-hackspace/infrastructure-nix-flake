//! Shared app state, navigation, data loading, toasts and formatting.

use std::collections::HashSet;
use std::time::Duration;

use dioxus::prelude::*;
use gloo_timers::future::sleep;
use serde::Deserialize;

use crate::api::*;

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
    Image,
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
    st.search.set(None);
    st.ctx.set(None);
    st.path.set(path);
    load_dir(st);
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
    st.search.set(None);
    st.ctx.set(None);
    st.path.set(path);
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

pub fn is_previewable(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    // Must match the server's preview allow-list: /api/preview refuses svg (an
    // inline SVG can carry script, and it would be served same-origin), so the
    // UI must not advertise it as previewable.
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp"
            | "txt" | "md" | "log" | "csv" | "tsv" | "json" | "yaml" | "yml"
            | "toml" | "ini" | "conf" | "xml" | "html" | "css" | "js" | "ts"
            | "sh" | "py" | "rs" | "c" | "h" | "cpp" | "go" | "nix"
    )
}

pub fn is_image(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp")
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
