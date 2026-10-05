//! Icon grid + sortable details (table) view, shared row model and
//! selection/drag handlers.

use dioxus::prelude::*;
use dioxus::events::{DragEvent, MouseEvent};

use crate::api::{self, download_url, Entry};
use crate::js::{ctrl_held, move_or_copy};
use crate::state::*;

const ROW_BG_SEL: &str = "#d8ecff";
const ROW_BORDER_SEL: &str = "#7ab";

/// Precomputed display row — for-loop bodies may not contain statements,
/// so everything is resolved before the rsx.
pub struct Row {
    pub i: usize,
    pub name: String,
    pub icon: String,
    pub is_dir: bool,
    pub is_sel: bool,
    pub border_c: String,
    pub bg_c: String,
    /// relative path of this row's directory (None for files) — used as
    /// the drop-target attribute for external drags
    pub dpath: String,
    pub size_str: String,
    pub time_str: String,
}

pub fn sorted(st: &AppState) -> Vec<Entry> {
    let mut v = st.entries.read().clone();
    let field = *st.sort_field.read();
    let asc = *st.sort_asc.read();
    v.sort_by(|a, b| {
        // directories first, always
        let base = b.is_dir.cmp(&a.is_dir);
        if base != std::cmp::Ordering::Equal {
            return base;
        }
        let ord = match field {
            SortField::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortField::Size => a.size.cmp(&b.size),
            SortField::Mtime => a.mtime.cmp(&b.mtime),
        };
        if asc { ord } else { ord.reverse() }
    });
    v
}

pub fn build_rows(st: &AppState) -> Vec<Row> {
    let entries = sorted(st);
    let sel = st.sel.read().clone();
    let path = st.path.read().clone();
    entries
        .into_iter()
        .enumerate()
        .map(|(i, e)| {
            let is_sel = sel.contains(&e.name);
            Row {
                i,
                name: e.name.clone(),
                icon: icon_for(&e.name, e.is_dir).to_string(),
                is_dir: e.is_dir,
                is_sel,
                border_c: if is_sel { ROW_BORDER_SEL.into() } else { "transparent".into() },
                bg_c: if is_sel {
                    ROW_BG_SEL.into()
                } else if i % 2 == 0 {
                    "#fff".into()
                } else {
                    "#fafafa".into()
                },
                dpath: if e.is_dir { api::join_rel(&path, &e.name) } else { String::new() },
                size_str: if e.is_dir { "—".into() } else { fmt_size(e.size) },
                time_str: fmt_time(e.mtime),
            }
        })
        .collect()
}

/// Selection: click / ctrl+click toggle / shift+click range.
fn row_select(
    mut st: AppState,
    name: &str,
    i: usize,
    names: &[String],
    me: &MouseEvent,
    mut last: Signal<Option<usize>>,
) {
    let mut s = st.sel.read().clone();
    if me.modifiers().shift() {
        if let Some(li) = *last.read() {
            let (lo, hi) = if li < i { (li, i) } else { (i, li) };
            for j in lo..=hi {
                s.insert(names[j].clone());
            }
        } else {
            s.insert(name.to_string());
        }
    } else if ctrl_held(me.modifiers()) {
        if !s.remove(name) {
            s.insert(name.to_string());
        }
    } else {
        s.clear();
        s.insert(name.to_string());
    }
    st.sel.set(s);
    last.set(Some(i));
}

/// Open: dirs navigate, previewable files preview, others download.
pub fn row_open(st: AppState, name: &str, is_dir: bool) {
    let full = api::join_rel(&st.path.read(), name);
    if is_dir {
        navigate(st, &full);
    } else if is_previewable(name) {
        crate::preview::open_preview(st, &full, name);
    } else {
        download(&download_url(&full));
    }
}

/// Internal-drag drop onto a directory row.
pub fn row_drop_internal(st: AppState, dt: &dioxus::html::DataTransfer, row_name: &str) {
    if let Some(json) = dt.get_data("application/x-filestore") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
            if let Some(paths) = v["paths"].as_array() {
                let srcs: Vec<String> =
                    paths.iter().filter_map(|p| p.as_str().map(|s| s.to_string())).collect();
                if !srcs.is_empty() {
                    let copy = dt.get_data("application/x-filestore-copy").is_some();
                    let dest = api::join_rel(&st.path.read(), row_name);
                    move_or_copy(st, srcs, dest, copy);
                }
            }
        }
    }
}

fn row_dragstart(de: &DragEvent, st: &AppState, name: &str, is_sel: bool) {
    let dt = de.data_transfer();
    let paths: Vec<String> = {
        let s: std::collections::HashSet<String> = st.sel.read().clone();
        if is_sel && s.contains(name) {
            s.iter().cloned().collect()
        } else {
            vec![name.to_string()]
        }
    };
    let json = serde_json::json!({ "paths": paths }).to_string();
    let _ = dt.set_data("application/x-filestore", &json);
    if ctrl_held(de.modifiers()) {
        let _ = dt.set_data("application/x-filestore-copy", "1");
    }
}

// ---------------------------------------------------------------------------
// icon grid

#[component]
pub fn IconGrid(st: AppState) -> Element {
    let rows = build_rows(&st);
    let names: Vec<String> = rows.iter().map(|r| r.name.clone()).collect();
    let last = use_signal(|| Option::<usize>::None);

    rsx! {
        div {
            id: "file-area",
            style: "flex:1;overflow:auto;padding:10px;display:grid;grid-template-columns:repeat(auto-fill,minmax(96px,1fr));gap:6px;align-content:start",
            for r in rows.iter() {
                div {
                    key: "{r.name}",
                    "data-fs-path": r.dpath.clone(),
                    style: "display:flex;flex-direction:column;align-items:center;gap:3px;padding:8px 4px;border:1px solid {r.border_c};background:{r.bg_c};border-radius:4px;cursor:default;user-select:none",
                    draggable: "true",
                    ondragstart: {
                        let name = r.name.clone();
                        let is_sel = r.is_sel;
                        move |de| row_dragstart(&de, &st, &name, is_sel)
                    },
                    ondragover: {
                        let is_dir = r.is_dir;
                        move |de| {
                            if is_dir {
                                de.prevent_default();
                            }
                        }
                    },
                    ondrop: {
                        let name = r.name.clone();
                        let is_dir = r.is_dir;
                        move |de| {
                            if is_dir {
                                de.prevent_default();
                                row_drop_internal(st, &de.data_transfer(), &name);
                            }
                        }
                    },
                    onmousedown: {
                        let name = r.name.clone();
                        let names = names.clone();
                        let i = r.i;
                        move |me| {
                            if is_left_click(&me) {
                                row_select(st, &name, i, &names, &me, last);
                            }
                        }
                    },
                    ondoubleclick: {
                        let name = r.name.clone();
                        let is_dir = r.is_dir;
                        move |_| row_open(st, &name, is_dir)
                    },
                    oncontextmenu: {
                        let name = r.name.clone();
                        move |ce| {
                            ce.prevent_default();
                            let mut s = st.sel.read().clone();
                            if !s.contains(&name) {
                                s.insert(name.clone());
                                st.sel.set(s);
                            }
                            st.ctx.set(Some(CtxMenu {
                                x: ce.coordinates().client().x,
                                y: ce.coordinates().client().y,
                                target: Some(name.clone()),
                            }));
                        }
                    },
                    div {
                        style: "font-size:34px;line-height:1",
                        "{r.icon}"
                    }
                    div {
                        style: "font-size:11.5px;text-align:center;word-break:break-all;max-height:32px;overflow:hidden",
                        title: "{r.name}",
                        "{r.name}"
                    }
                }
            }
            // background context menu on the empty area
            div {
                style: "position:absolute;inset:0;z-index:-1",
                oncontextmenu: move |ce| {
                    ce.prevent_default();
                    st.ctx.set(Some(CtxMenu {
                        x: ce.coordinates().client().x,
                        y: ce.coordinates().client().y,
                        target: None,
                    }));
                },
            }
        }
    }
}

// ---------------------------------------------------------------------------
// details (table) view

#[component]
pub fn DetailsView(st: AppState) -> Element {
    let rows = build_rows(&st);
    let names: Vec<String> = rows.iter().map(|r| r.name.clone()).collect();
    let last = use_signal(|| Option::<usize>::None);
    let sf = *st.sort_field.read();
    let asc = *st.sort_asc.read();

    let col = |field: SortField, label: &str, width: &str| -> Element {
        let arrow = if sf == field {
            if asc { "▲".to_string() } else { "▼".to_string() }
        } else {
            String::new()
        };
        rsx! {
            th {
                style: "text-align:left;padding:4px 10px;border-bottom:2px solid #ccc;cursor:pointer;user-select:none;position:sticky;top:0;background:#f7f7f7;width:{width}",
                onclick: move |_| {
                    if *st.sort_field.read() == field {
                        let a = *st.sort_asc.read();
                        st.sort_asc.set(!a);
                    } else {
                        st.sort_field.set(field);
                        st.sort_asc.set(true);
                    }
                },
                "{label} {arrow}"
            }
        }
    };

    rsx! {
        div {
            id: "file-area",
            style: "flex:1;overflow:auto",
            table {
                style: "border-collapse:collapse;width:100%",
                thead {
                    tr {
                        {col(SortField::Name, "Name", "50%")}
                        {col(SortField::Size, "Size", "16%")}
                        {col(SortField::Mtime, "Modified", "34%")}
                    }
                }
                tbody {
                    for r in rows.iter() {
                        tr {
                            key: "{r.name}",
                            "data-fs-path": r.dpath.clone(),
                            style: "cursor:default;user-select:none;background:{r.bg_c}",
                            draggable: "true",
                            ondragstart: {
                                let name = r.name.clone();
                                let is_sel = r.is_sel;
                                move |de| row_dragstart(&de, &st, &name, is_sel)
                            },
                            ondragover: {
                                let is_dir = r.is_dir;
                                move |de| {
                                    if is_dir {
                                        de.prevent_default();
                                    }
                                }
                            },
                            ondrop: {
                                let name = r.name.clone();
                                let is_dir = r.is_dir;
                                move |de| {
                                    if is_dir {
                                        de.prevent_default();
                                        row_drop_internal(st, &de.data_transfer(), &name);
                                    }
                                }
                            },
                            onmousedown: {
                                let name = r.name.clone();
                                let names = names.clone();
                                let i = r.i;
                                move |me| {
                                    if is_left_click(&me) {
                                        row_select(st, &name, i, &names, &me, last);
                                    }
                                }
                            },
                            ondoubleclick: {
                                let name = r.name.clone();
                                let is_dir = r.is_dir;
                                move |_| row_open(st, &name, is_dir)
                            },
                            oncontextmenu: {
                                let name = r.name.clone();
                                move |ce| {
                                    ce.prevent_default();
                                    let mut s = st.sel.read().clone();
                                    if !s.contains(&name) {
                                        s.insert(name.clone());
                                        st.sel.set(s);
                                    }
                                    st.ctx.set(Some(CtxMenu {
                                        x: ce.coordinates().client().x,
                                        y: ce.coordinates().client().y,
                                        target: Some(name.clone()),
                                    }));
                                }
                            },
                            td {
                                style: "padding:3px 10px;border-bottom:1px solid #eee;white-space:nowrap;overflow:hidden;text-overflow:ellipsis",
                                "{r.icon}  {r.name}"
                            }
                            td {
                                style: "padding:3px 10px;border-bottom:1px solid #eee;color:#555",
                                "{r.size_str}"
                            }
                            td {
                                style: "padding:3px 10px;border-bottom:1px solid #eee;color:#555",
                                "{r.time_str}"
                            }
                        }
                    }
                }
            }
            // background context menu
            div {
                style: "position:absolute;inset:0;z-index:-1",
                oncontextmenu: move |ce| {
                    ce.prevent_default();
                    st.ctx.set(Some(CtxMenu {
                        x: ce.coordinates().client().x,
                        y: ce.coordinates().client().y,
                        target: None,
                    }));
                },
            }
        }
    }
}
