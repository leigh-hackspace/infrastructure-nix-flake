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
///
/// `st.focus` is the shared anchor (also driven by the arrow keys), so a
/// shift-click extends from the row the last plain click — or the last arrow —
/// landed on.
fn row_select(mut st: AppState, name: &str, i: usize, names: &[String], me: &MouseEvent) {
    let mut s = st.sel.read().clone();
    if me.modifiers().shift() {
        if let Some(li) = *st.focus.read() {
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
    st.focus.set(Some(i));
}

/// Move the selection with an arrow key (`delta` is in row units: one column in
/// the icon grid, one row in the details table).
///
/// A plain arrow jumps to the next row and replaces the selection; shift+arrow
/// extends the range from the anchor, matching shift+click (union semantics, so
/// ctrl-clicked stragglers survive).
pub fn move_selection(mut st: AppState, delta: i64, shift: bool) {
    let names: Vec<String> = sorted(&st).iter().map(|e| e.name.clone()).collect();
    if names.is_empty() {
        return;
    }
    let n = names.len() as i64;
    let anchor = *st.focus.read();
    let i = match anchor {
        Some(a) => (a as i64 + delta).clamp(0, n - 1),
        None => 0,
    };

    let mut s = st.sel.read().clone();
    if shift {
        if let Some(a) = anchor {
            let (lo, hi) = if (a as i64) < i { (a as i64, i) } else { (i, a as i64) };
            for j in lo..=hi {
                s.insert(names[j as usize].clone());
            }
        } else {
            s.insert(names[i as usize].clone());
        }
    } else {
        s.clear();
        s.insert(names[i as usize].clone());
    }
    st.sel.set(s);
    st.focus.set(Some(i as usize));
    scroll_row_into_view(&names[i as usize]);
}

/// Number of grid columns currently painted (1 for the details table), read from
/// the rendered `auto-fill` track list so arrow up/down matches what the user sees.
pub fn grid_columns(st: &AppState) -> usize {
    if *st.view.read() == View::Details {
        return 1;
    }
    // js_eval_string only sees string results, so the count is returned as text.
    js_eval_string(
        r#"(function(){var a=document.getElementById('file-area');if(!a)return '1';var t=getComputedStyle(a).gridTemplateColumns;if(!t||t==='none')return '1';return ''+t.split(' ').length;})()"#,
    )
    .and_then(|s| s.parse::<usize>().ok())
    .unwrap_or(1)
}

/// Open the row the keyboard focus is on (Enter).
///
/// `st.focus` is the arrow-key anchor; when the selection was made with the
/// mouse only, a single selected row is used instead.  Multiple selections have
/// no single "open" meaning, so Enter does nothing for them.
pub fn open_focused(mut st: AppState) {
    let entries = sorted(&st);
    if entries.is_empty() {
        return;
    }
    let idx = match *st.focus.read() {
        Some(i) => i,
        None => {
            let sel = st.sel.read().clone();
            if sel.len() != 1 {
                return;
            }
            match entries.iter().position(|e| sel.contains(&e.name)) {
                Some(i) => i,
                None => return,
            }
        }
    };
    // Clone before calling: row_open writes signals.
    let (name, is_dir) = (entries[idx].name.clone(), entries[idx].is_dir);
    row_open(st, &name, is_dir);
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
                    // dioxus's DataTransfer::get_data returns Some("") for formats
                    // that were never set, so is_some() is always true — an internal
                    // drag would then always copy instead of move.
                    let copy = dt
                        .get_data("application/x-filestore-copy")
                        .is_some_and(|s| !s.is_empty());
                    let dest = api::join_rel(&st.path.read(), row_name);
                    move_or_copy(st, srcs, dest, copy);
                }
            }
        }
    }
}

/// The URL an external drop target should be given for one row.
///
/// Files have a download URL; directories have no single file, so they point at
/// their streaming ZIP.
pub fn external_uri(dir: &str, name: &str, is_dir: bool) -> String {
    let full = api::join_rel(dir, name);
    if is_dir {
        api::zip_url(&[full])
    } else {
        api::download_url(&full)
    }
}

fn row_dragstart(de: &DragEvent, st: &AppState, name: &str, is_sel: bool, is_dir: bool) {
    let dt = de.data_transfer();
    let dir = st.path.read().clone();
    let sel: std::collections::HashSet<String> = st.sel.read().clone();
    let entries = st.entries.read().clone();

    // The whole selection when the dragged row is part of it, otherwise just
    // that row.
    let items: Vec<(String, bool)> = if is_sel && sel.contains(name) {
        entries
            .iter()
            .filter(|e| sel.contains(&e.name))
            .map(|e| (e.name.clone(), e.is_dir))
            .collect()
    } else {
        vec![(name.to_string(), is_dir)]
    };
    let paths: Vec<String> = items.iter().map(|(n, _)| n.clone()).collect();

    let json = serde_json::json!({ "paths": paths }).to_string();
    let _ = dt.set_data("application/x-filestore", &json);
    if ctrl_held(de.modifiers()) {
        let _ = dt.set_data("application/x-filestore-copy", "1");
    }

    // `application/x-filestore` is only understood by this page, so a drag that
    // leaves the browser (file manager, desktop, an editor) had no payload the
    // target could accept and the drop was simply refused.  text/uri-list is the
    // format OS drag-and-drop actually speaks, so publish the download URLs as
    // well: file managers fetch them, and targets that cannot fetch write a
    // shortcut/link instead.  Without this, dragging a row out does nothing.
    let origin = js_eval_string("(location.origin)").unwrap_or_default();
    let uris: Vec<String> = items
        .iter()
        .map(|(n, d)| format!("{origin}{}", external_uri(&dir, n, *d)))
        .collect();
    let list = uris.join("\n");
    let _ = dt.set_data("text/uri-list", &list);
    let _ = dt.set_data("text/plain", &list);
}

// ---------------------------------------------------------------------------
// icon grid

#[component]
pub fn IconGrid(st: AppState) -> Element {
    let rows = build_rows(&st);
    let names: Vec<String> = rows.iter().map(|r| r.name.clone()).collect();

    rsx! {
        div {
            id: "file-area",
            style: "flex:1;overflow:auto;padding:10px;display:grid;grid-template-columns:repeat(auto-fill,minmax(96px,1fr));gap:6px;align-content:start",
            oncontextmenu: move |ce| {
                ce.prevent_default();
                bg_menu(st, &ce);
            },
            for r in rows.iter() {
                div {
                    key: "{r.name}",
                    "data-fs-path": r.dpath.clone(),
                    "data-fs-name": r.name.clone(),
                    "data-fs-sel": if r.is_sel { "1" } else { "0" },
                    style: "display:flex;flex-direction:column;align-items:center;gap:3px;padding:8px 4px;border:1px solid {r.border_c};background:{r.bg_c};border-radius:4px;cursor:default;user-select:none",
                    draggable: "true",
                    ondragstart: {
                        let name = r.name.clone();
                        let is_sel = r.is_sel;
                        let is_dir = r.is_dir;
                        move |de| row_dragstart(&de, &st, &name, is_sel, is_dir)
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
                                row_select(st, &name, i, &names, &me);
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
                            ce.stop_propagation();
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
            // background context menu: the container itself handles clicks that
            // are not on a row (row handlers stop propagation).
        }
    }
}

/// Open the context menu for a background (non-row) right-click.
fn bg_menu(mut st: AppState, ce: &MouseEvent) {
    st.ctx.set(Some(CtxMenu {
        x: ce.coordinates().client().x,
        y: ce.coordinates().client().y,
        target: None,
    }));
}

// ---------------------------------------------------------------------------
// details (table) view

#[component]
pub fn DetailsView(st: AppState) -> Element {
    let rows = build_rows(&st);
    let names: Vec<String> = rows.iter().map(|r| r.name.clone()).collect();
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
            oncontextmenu: move |ce| {
                ce.prevent_default();
                bg_menu(st, &ce);
            },
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
                            "data-fs-name": r.name.clone(),
                            "data-fs-sel": if r.is_sel { "1" } else { "0" },
                            style: "cursor:default;user-select:none;background:{r.bg_c}",
                            draggable: "true",
                            ondragstart: {
                                let name = r.name.clone();
                                let is_sel = r.is_sel;
                                let is_dir = r.is_dir;
                                move |de| row_dragstart(&de, &st, &name, is_sel, is_dir)
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
                                        row_select(st, &name, i, &names, &me);
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
                                    ce.stop_propagation();
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
