//! The app shell: toolbar, nav/breadcrumb bar, search bar, layout,
//! keyboard shortcuts.

use dioxus::prelude::*;
use wasm_bindgen::JsCast;

use crate::api::*;
use crate::grid::{DetailsView, IconGrid};
use crate::js::ensure_js_glue;
use crate::menu::ContextMenu;
use crate::modal::ModalBox;
use crate::misc::{Toasts, UploadBar};
use crate::preview::PreviewBox;
use crate::searchview::ResultsView;
use crate::state::*;

pub const TBTN: &str = "padding:3px 8px;border:1px solid #bbb;background:linear-gradient(#fff,#eee);border-radius:3px;cursor:pointer;font-size:12px";

// ---------------------------------------------------------------------------
// App

#[component]
pub fn App() -> Element {
    let mut st = use_hook(AppState::new);

    // CSS + JS glue
    use_effect(move || {
        let _ = js_eval(
            r#"
                document.body.style.cssText = 'margin:0;font-family:system-ui,"Segoe UI",sans-serif;font-size:13px;color:#222';
                document.documentElement.style.height = '100%';
                document.body.style.height = '100%';
                var st = document.createElement('style');
                st.textContent = '.fs-menu-item:hover{background:#d8ecff}';
                document.head.appendChild(st);
            "#,
        );
        ensure_js_glue(st);
    });

    // whoami + initial dir load
    use_future(move || async move {
        match get_json::<Whoami>("/api/whoami").await {
            Ok(w) => st.whoami.set(Some(w)),
            Err(_) => {
                redirect("/auth/login");
                return;
            }
        }
        load_dir(st);
    });

    // keyboard shortcuts
    use_effect(move || {
        let handler: wasm_bindgen::closure::Closure<dyn FnMut(wasm_bindgen::JsValue)> =
            wasm_bindgen::closure::Closure::new(move |v: wasm_bindgen::JsValue| {
                let Ok(e) = v.dyn_into::<web_sys::KeyboardEvent>() else {
                    return;
                };
                let Some(mut st) = AppState::global() else {
                    return;
                };
                let ctrl = e.ctrl_key() || e.meta_key();
                match e.key().as_str() {
                    "Escape" => {
                        st.ctx.set(None);
                        st.modal.set(None);
                        st.preview.set(None);
                        if st.search.read().is_some() {
                            st.search.set(None);
                        } else {
                            st.sel.set(std::collections::HashSet::new());
                        }
                    }
                    "Delete" => {
                        if st.modal.read().is_some() {
                            return;
                        }
                        let sel: Vec<String> = st.sel.read().iter().cloned().collect();
                        if !sel.is_empty() {
                            st.modal.set(Some(Modal::Delete(sel)));
                        }
                    }
                    "a" if ctrl => {
                        e.prevent_default();
                        let all: std::collections::HashSet<String> =
                            st.entries.read().iter().map(|x| x.name.clone()).collect();
                        st.sel.set(all);
                    }
                    "f" if ctrl => {
                        e.prevent_default();
                        load_dir(st);
                    }
                    "Backspace" => go(st, -1),
                    _ => {}
                }
            });
        if let Some(window) = web_sys::window() {
            let func: &js_sys::Function = handler.as_js_value().unchecked_ref::<js_sys::Function>();
            let _ = window.add_event_listener_with_callback("keydown", &func);
            std::mem::forget(handler);
        }
    });

    let whoami = st.whoami.read().clone();
    let search = st.search.read().clone();
    let entries = st.entries.read().clone();
    let sel = st.sel.read().clone();
    let loading = *st.loading.read();
    let view = *st.view.read();
    let ctx = st.ctx.read().clone();
    let modal = st.modal.read().clone();
    let preview = st.preview.read().clone();
    let uploads = st.uploads.read().clone();
    let toasts = st.toasts.read().clone();

    let sel_note = if !sel.is_empty() {
        format!("· {} selected", sel.len())
    } else {
        String::new()
    };
    let user_note = whoami.as_ref().map(|w| format!("· {}", w.username));

    rsx! {
        div { id: "root", style: "height:100vh;display:flex;flex-direction:column",
            Toolbar { st }

            div {
                style: "display:flex;align-items:center;gap:6px;padding:4px 8px;background:#fff;border-bottom:1px solid #d0d0d0",
                NavButtons { st }
                Breadcrumbs { st }
                SearchBar { st }
            }

            div {
                id: "fs-content",
                style: "flex:1;display:flex;min-height:0;background:#fff",
                if let Some(sv) = search {
                    ResultsView { st, sv }
                } else if loading {
                    div {
                        style: "padding:24px;color:#777",
                        "loading…"
                    }
                } else if entries.is_empty() {
                    div {
                        style: "padding:24px;color:#999",
                        "Empty folder. "
                        button {
                            style: "color:#06c;border:none;background:none;cursor:pointer",
                            onclick: move |_| st.modal.set(Some(Modal::NewFolder)),
                            "New folder"
                        }
                        ", "
                        button {
                            style: "color:#06c;border:none;background:none;cursor:pointer",
                            onclick: move |_| st.modal.set(Some(Modal::NewFile)),
                            "New file"
                        }
                        ", or drop files here."
                    }
                } else if view == View::Icons {
                    IconGrid { st }
                } else {
                    DetailsView { st }
                }
            }

            div {
                id: "fs-status",
                style: "display:flex;gap:16px;padding:3px 10px;background:#f0f0f0;border-top:1px solid #d0d0d0;color:#555;font-size:12px",
                "{entries.len()} item(s){sel_note}"
                "· /mnt/filestore"
                if user_note.is_some() {
                    {user_note.clone().unwrap_or_default()}
                }
            }

            if !uploads.is_empty() {
                UploadBar { st }
            }
            if let Some(m) = ctx {
                ContextMenu { st, menu: m }
            }
            if let Some(m) = modal {
                ModalBox { st, modal: m }
            }
            if let Some(p) = preview {
                PreviewBox { st, preview: p }
            }
            Toasts { st, list: toasts }
        }
    }
}

// ---------------------------------------------------------------------------
// nav buttons + toolbar

#[component]
pub fn NavButtons(st: AppState) -> Element {
    let back_op = if *st.hidx.read() > 0 { "1" } else { "0.35" };
    let fwd_op = if *st.hidx.read() < st.hist.read().len() - 1 { "1" } else { "0.35" };
    rsx! {
        button {
            style: "{TBTN};opacity:{back_op}",
            onclick: move |_| go(st, -1),
            "←"
        }
        button {
            style: "{TBTN};opacity:{fwd_op}",
            onclick: move |_| go(st, 1),
            "→"
        }
        button {
            style: TBTN,
            onclick: move |_| {
                // Clone first: holding a read guard on st.path while navigate()
                // writes the same signal panics (re-entrant signal borrow).
                let binding = st.path.read().clone();
                let p = binding.split('/').collect::<Vec<_>>();
                let parent = if p.len() > 1 { p[..p.len() - 1].join("/") } else { String::new() };
                navigate(st, &parent);
            },
            "↑"
        }
    }
}

#[component]
pub fn Toolbar(st: AppState) -> Element {
    let sel_count = st.sel.read().len();
    let in_search = st.search.read().is_some();
    let entries = st.entries.read().clone();
    let sel: std::collections::HashSet<String> = st.sel.read().clone();
    let view = *st.view.read();
    let sel_op = if sel_count == 0 { "0.5" } else { "1" };
    let del_label = if sel_count > 0 {
        format!("🗑 Delete ({sel_count})")
    } else {
        "🗑 Delete".to_string()
    };
    let view_label = if view == View::Icons { "☰ Details".to_string() } else { "▦ Icons".to_string() };

    rsx! {
        div {
            style: "display:flex;align-items:center;gap:4px;padding:5px 8px;background:linear-gradient(#fdfdfd,#f3f3f3);border-bottom:1px solid #ccc",
            button {
                style: "{TBTN};opacity:{sel_op}",
                title: "Zip the selection (or everything in this folder) as a ZIP download",
                onclick: {
                    let sel = sel.clone();
                    let entries = entries.clone();
                    move |_| {
                        let mut s: Vec<String> = sel.iter().cloned().collect();
                        if s.is_empty() {
                            s = entries.iter().map(|e| e.name.clone()).collect();
                        }
                        let dir = st.path.read().clone();
                        let paths: Vec<String> = s.iter().map(|n| join_rel(&dir, n)).collect();
                        zip_selected(st, paths);
                    }
                },
                "📦 Zip"
            }
            button {
                style: "{TBTN};opacity:{sel_op}",
                onclick: {
                    let sel = sel.clone();
                    move |_| {
                        let s: Vec<String> = sel.iter().cloned().collect();
                        st.modal.set(Some(Modal::Delete(s)));
                    }
                },
                "{del_label}"
            }
            if in_search {
                button {
                    style: TBTN,
                    onclick: move |_| st.search.set(None),
                    "✕ Close search"
                }
            } else {
                button {
                    style: TBTN,
                    onclick: move |_| st.modal.set(Some(Modal::NewFolder)),
                    "📁 New folder"
                }
                button {
                    style: TBTN,
                    onclick: move |_| st.modal.set(Some(Modal::NewFile)),
                    "📄 New file"
                }
                button {
                    style: TBTN,
                    title: "Upload files (and folders) from this machine",
                    onclick: move |_| {
                        let _ = js_eval(r#"document.getElementById('fs-upload-input').click()"#);
                    },
                    "⬆ Upload"
                }
                button {
                    style: TBTN,
                    onclick: move |_| load_dir(st),
                    "⟳"
                }
                div { style: "flex:1" }
                button {
                    style: TBTN,
                    onclick: move |_| st.view.set(if view == View::Icons { View::Details } else { View::Icons }),
                    "{view_label}"
                }
                button {
                    style: TBTN,
                    onclick: move |_| redirect("/auth/logout"),
                    "Logout"
                }
            }
            input {
                id: "fs-upload-input",
                r#type: "file",
                multiple: "true",
                "webkitdirectory": "",
                style: "display:none",
            }
        }
    }
}

// ---------------------------------------------------------------------------
// breadcrumbs

#[component]
pub fn Breadcrumbs(st: AppState) -> Element {
    let path = st.path.read().clone();
    // (display segment, nav path) pairs
    let mut crumbs: Vec<(String, String)> = vec![("/".to_string(), String::new())];
    if !path.is_empty() {
        let mut acc = String::new();
        for seg in path.split('/') {
            acc = if acc.is_empty() { seg.to_string() } else { format!("{acc}/{seg}") };
            crumbs.push((seg.to_string(), acc.clone()));
        }
    }

    rsx! {
        div {
            style: "flex:1;display:flex;align-items:center;gap:2px;background:#fff;border:1px solid #ccc;border-radius:3px;padding:3px 6px;overflow:hidden",
            for c in crumbs.iter() {
                button {
                    key: "{c.1}",
                    style: "border:none;background:none;cursor:pointer;padding:1px 4px;color:#06c;font-size:12.5px",
                    onclick: {
                        let p = c.1.clone();
                        move |_| navigate(st, &p)
                    },
                    "{c.0}"
                    if !c.1.is_empty() {
                        span {
                            style: "color:#888",
                            "/"
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// search bar

#[component]
pub fn SearchBar(st: AppState) -> Element {
    let mut q = use_signal(|| String::new());
    let mut deep = use_signal(|| true);
    let deep_on = *deep.read();
    let deep_op = if deep_on { "1" } else { "0.6" };
    let deep_label = if deep_on { "deep".to_string() } else { "shallow".to_string() };

    rsx! {
        div {
            style: "display:flex;align-items:center;gap:4px",
            input {
                id: "fs-search",
                value: q,
                placeholder: "Search in this folder…",
                style: "width:190px;padding:3px 6px;border:1px solid #ccc;border-radius:3px",
                oninput: move |e| q.set(e.value()),
                onkeydown: move |e| {
                    if e.key() == dioxus::html::Key::Enter {
                        let qq = q.read().trim().to_string();
                        if !qq.is_empty() {
                            run_search(st, st.path.read().clone(), qq, *deep.read());
                        }
                    }
                }
            }
            button {
                style: "{TBTN};opacity:{deep_op}",
                title: "Shallow: search only this folder. Deep: search this folder and everything under it.",
                onclick: move |_| {
                    let d = *deep.read();
                    deep.set(!d);
                },
                "{deep_label}"
            }
            button {
                style: TBTN,
                onclick: move |_| {
                    let qq = q.read().trim().to_string();
                    if !qq.is_empty() {
                        run_search(st, st.path.read().clone(), qq, *deep.read());
                    }
                },
                "🔍"
            }
        }
    }
}
