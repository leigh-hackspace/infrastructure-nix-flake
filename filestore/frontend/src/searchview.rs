//! Search results view.

use dioxus::prelude::*;

use crate::api;
use crate::state::*;

/// Precomputed display row for a hit (no statements allowed in for bodies).
struct HitRow {
    rel: String,
    parent: String,
    name: String,
    icon: String,
    thumb: String,
    has_thumb: bool,
    parent_disp: String,
    size_str: String,
    time_str: String,
}

#[component]
pub fn ResultsView(st: AppState, sv: SearchView) -> Element {
    let scope_display = if sv.scope.is_empty() { "/".to_string() } else { sv.scope.clone() };
    let mode = if sv.deep { "deep" } else { "shallow" };
    let scan_note = if sv.scanned > 0 {
        format!(" · {} entries scanned", sv.scanned)
    } else {
        String::new()
    };
    let err = sv.error.clone().unwrap_or_default();
    let has_err = sv.error.is_some();
    let failed = st.thumb_failed.read().clone();
    let rows: Vec<HitRow> = sv
        .hits
        .iter()
        .map(|h| {
            let thumb = if !h.is_dir && common_preview::thumbnailable(&h.name) {
                api::thumb_url(&h.rel, &h.fingerprint)
            } else {
                String::new()
            };
            HitRow {
                rel: h.rel.clone(),
                parent: h.parent.clone(),
                name: h.name.clone(),
                icon: icon_for(&h.name, h.is_dir).to_string(),
                has_thumb: !thumb.is_empty() && !failed.contains(&thumb),
                thumb,
                parent_disp: if h.parent.is_empty() { "/".into() } else { h.parent.clone() },
                size_str: if h.is_dir { "—".into() } else { fmt_size(h.size) },
                time_str: fmt_time(h.mtime),
            }
        })
        .collect();

    rsx! {
        div {
            id: "file-area",
            style: "flex:1;overflow:auto;padding:10px 16px",
            div {
                style: "margin-bottom:10px;color:#555",
                "{sv.hits.len()} result(s) for "
                b { "\"{sv.q}\"" }
                " in "
                b { "{scope_display}" }
                " ({mode}){scan_note}"
                if sv.truncated { " — results truncated" }
            }
            if sv.searching {
                div {
                    style: "color:#777",
                    "searching…"
                }
            }
            if has_err && !err.is_empty() {
                div {
                    style: "color:#c00",
                    "{err}"
                }
            }
            if !rows.is_empty() {
                table {
                    style: "border-collapse:collapse;width:100%",
                    thead {
                        tr {
                            th {
                                style: "text-align:left;padding:4px 8px;border-bottom:2px solid #ccc",
                                "Name"
                            }
                            th {
                                style: "text-align:left;padding:4px 8px;border-bottom:2px solid #ccc",
                                "In folder"
                            }
                            th {
                                style: "text-align:left;padding:4px 8px;border-bottom:2px solid #ccc",
                                "Size"
                            }
                            th {
                                style: "text-align:left;padding:4px 8px;border-bottom:2px solid #ccc",
                                "Modified"
                            }
                        }
                    }
                    tbody {
                        for r in rows.iter() {
                            tr {
                                key: "{r.rel}",
                                style: "cursor:pointer",
                                ondoubleclick: {
                                    let parent = r.parent.clone();
                                    move |_| {
                                        navigate(st, &parent);
                                    }
                                },
                                td {
                                    style: "padding:3px 8px;border-bottom:1px solid #eee",
                                    if r.has_thumb {
                                        img {
                                            src: "{r.thumb}",
                                            loading: "lazy",
                                            style: "width:20px;height:20px;object-fit:contain;margin-right:6px;vertical-align:middle",
                                            onerror: {
                                                let u = r.thumb.clone();
                                                move |_| mark_thumb_failed(st, &u)
                                            },
                                        }
                                    } else {
                                        "{r.icon}"
                                    }
                                    "  {r.name}"
                                }
                                td {
                                    style: "padding:3px 8px;border-bottom:1px solid #eee;color:#555",
                                    "{r.parent_disp}"
                                }
                                td {
                                    style: "padding:3px 8px;border-bottom:1px solid #eee;color:#555",
                                    "{r.size_str}"
                                }
                                td {
                                    style: "padding:3px 8px;border-bottom:1px solid #eee;color:#555",
                                    "{r.time_str}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
