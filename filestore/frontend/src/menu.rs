//! Right-click context menu.

use std::collections::HashSet;

use dioxus::prelude::*;

use crate::api::{download_url, join_rel};
use crate::grid::row_open;
use crate::state::*;

#[component]
pub fn ContextMenu(st: AppState, menu: CtxMenu) -> Element {
    let sel: HashSet<String> = st.sel.read().clone();
    let target = menu.target.clone();
    let path = st.path.read().clone();

    // Effective selection: the right-clicked target if given, else the
    // current selection (background menu).
    let eff: Vec<String> = match &target {
        Some(t) => vec![t.clone()],
        None => sel.iter().cloned().collect(),
    };
    let single = eff.len() == 1;
    let first_name = eff.first().cloned();
    let first_entry = first_name
        .as_ref()
        .and_then(|n| st.entries.read().iter().find(|e| e.name == *n).cloned());
    let first_is_previewable = first_name.as_ref().is_some_and(|n| is_previewable(n))
        && first_entry.as_ref().map(|e| !e.is_dir).unwrap_or(false);
    let first_is_dir = first_entry.as_ref().map(|e| e.is_dir).unwrap_or(false);
    let n_items = eff.len();

    // Actions are built up front: each closure takes its own copy of the
    // (Copy) state bundle plus owned clones of the names it needs.
    let act_open = {
        let n = first_name.clone().unwrap_or_default();
        Box::new(move || {
            if let Some(e) = st.entries.read().iter().find(|x| x.name == n).cloned() {
                row_open(st, &n, e.is_dir);
            }
        }) as Box<dyn Fn()>
    };
    let act_preview = {
        let n = first_name.clone().unwrap_or_default();
        let p = path.clone();
        Box::new(move || {
            let full = join_rel(&p, &n);
            crate::preview::open_preview(st, &full, &n);
        }) as Box<dyn Fn()>
    };
    let act_download = {
        let n = first_name.clone().unwrap_or_default();
        let p = path.clone();
        Box::new(move || {
            let full = join_rel(&p, &n);
            download(&download_url(&full));
        }) as Box<dyn Fn()>
    };
    let act_zip = {
        let eff = eff.clone();
        let p = path.clone();
        Box::new(move || {
            let paths: Vec<String> = eff.iter().map(|n| join_rel(&p, n)).collect();
            zip_selected(st, paths);
        }) as Box<dyn Fn()>
    };
    let act_rename = {
        let n = first_name.clone().unwrap_or_default();
        Box::new(move || {
            let mut s = st;
            s.modal.set(Some(Modal::Rename(n.clone())));
        }) as Box<dyn Fn()>
    };
    let act_delete = {
        let eff = eff.clone();
        Box::new(move || {
            let mut s = st;
            s.modal.set(Some(Modal::Delete(eff.clone())));
        }) as Box<dyn Fn()>
    };
    let act_new_folder = Box::new(move || {
        let mut s = st;
        s.modal.set(Some(Modal::NewFolder));
    }) as Box<dyn Fn()>;
    let act_new_file = Box::new(move || {
        let mut s = st;
        s.modal.set(Some(Modal::NewFile));
    }) as Box<dyn Fn()>;
    let act_upload = Box::new(move || {
        let _ = js_eval(r#"document.getElementById('fs-upload-input').click()"#);
    }) as Box<dyn Fn()>;
    let act_refresh = Box::new(move || load_dir(st)) as Box<dyn Fn()>;
    let act_select_all = Box::new(move || {
        let all: HashSet<String> = st.entries.read().iter().map(|x| x.name.clone()).collect();
        let mut s = st;
        s.sel.set(all);
    }) as Box<dyn Fn()>;
    let act_clear = Box::new(move || {
        let mut s = st;
        s.sel.set(HashSet::new());
    }) as Box<dyn Fn()>;

    // Hover highlight is done with CSS (.fs-menu-item:hover) rather than
    // poking at the node from the event, which dioxus 0.7 no longer exposes.
    let item = |label: String, icon: &str, enabled: bool, action: Box<dyn Fn()>| -> Element {
        let op = if enabled { "1" } else { "0.4" };
        rsx! {
            div {
                class: "fs-menu-item",
                style: "padding:5px 18px;cursor:pointer;white-space:nowrap;opacity:{op}",
                onclick: move |_| {
                    if enabled {
                        st.ctx.set(None);
                        action();
                    }
                },
                "{icon} {label}"
            }
        }
    };

    let sep = rsx! { div { style: "height:1px;background:#ddd;margin:3px 0" } };

    rsx! {
        div {
            style: "position:fixed;z-index:1000;min-width:190px;background:#fff;border:1px solid #999;box-shadow:2px 2px 8px rgba(0,0,0,0.25);padding:3px 0;left:{menu.x as i32}px;top:{menu.y as i32}px",
            onmousedown: move |e| e.stop_propagation(),
            oncontextmenu: move |e| e.prevent_default(),

            if !eff.is_empty() {
                if single && first_name.is_some() {
                    { item("Open".to_string(), "📂", true, act_open) }
                }
                if first_is_previewable && first_name.is_some() {
                    { item("Preview".to_string(), "👁", true, act_preview) }
                }
                if single && !first_is_dir && first_name.is_some() {
                    { item("Download".to_string(), "⬇", true, act_download) }
                }
                { item(format!("Download as ZIP ({n_items})"), "📦", true, act_zip) }
                { sep.clone() }
                if single && first_name.is_some() {
                    { item("Rename".to_string(), "✏️", true, act_rename) }
                }
                { item(format!("Delete ({n_items})"), "🗑", true, act_delete) }
                { sep.clone() }
            }
            { item("New folder".to_string(), "📁", true, act_new_folder) }
            { item("New file".to_string(), "📄", true, act_new_file) }
            { item("Upload files…".to_string(), "⬆", true, act_upload) }
            { item("Refresh".to_string(), "⟳", true, act_refresh) }
            { item("Select all".to_string(), "✓", true, act_select_all) }
            if !sel.is_empty() {
                { item("Clear selection".to_string(), "✕", true, act_clear) }
            }
        }
    }
}
