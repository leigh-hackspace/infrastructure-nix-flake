//! Small floating chrome: upload progress bar and toasts.

use dioxus::prelude::*;

use crate::state::{AppState, Toast};

/// Precomputed per-upload display values (for-loop bodies may not contain
/// statements).
struct UpRow {
    rel: String,
    status_text: String,
    has_err: bool,
    bar_w: u32,
    show_bar: bool,
}

#[component]
pub fn UploadBar(st: AppState) -> Element {
    let uploads = st.uploads.read().clone();
    let rows: Vec<UpRow> = uploads
        .iter()
        .take(8)
        .map(|u| UpRow {
            rel: u.rel.clone(),
            status_text: match &u.error {
                Some(e) => format!("✗ {e}"),
                None if u.done => "✓".to_string(),
                None => format!("{}%", (u.pct * 100.0) as u32),
            },
            has_err: u.error.is_some(),
            bar_w: (u.pct * 100.0) as u32,
            show_bar: !u.done && u.error.is_none(),
        })
        .collect();

    rsx! {
        div {
            style: "position:fixed;right:12px;bottom:34px;z-index:700;width:300px;background:#fff;border:1px solid #888;border-radius:6px;box-shadow:0 3px 12px rgba(0,0,0,0.3);padding:10px",
            div {
                style: "font-weight:bold;margin-bottom:6px",
                "Upload"
            }
            for r in rows.iter() {
                div {
                    key: "{r.rel}",
                    style: "margin:3px 0",
                    div {
                        style: "display:flex;justify-content:space-between;font-size:11.5px",
                        span {
                            style: "overflow:hidden;text-overflow:ellipsis;white-space:nowrap",
                            "{r.rel}"
                        }
                        if r.has_err {
                            span {
                                style: "color:#c00",
                                "{r.status_text}"
                            }
                        } else {
                            span { "{r.status_text}" }
                        }
                    }
                    if r.show_bar {
                        div {
                            style: "height:4px;background:#ddd;border-radius:2px;overflow:hidden",
                            div {
                                style: "height:100%;background:#3a7afe;width:{r.bar_w}%"
                            }
                        }
                    }
                }
            }
            button {
                style: "margin-top:6px;float:right;border:none;background:none;cursor:pointer;color:#06c",
                onclick: move |_| st.uploads.set(Vec::new()),
                "dismiss"
            }
        }
    }
}

#[component]
pub fn Toasts(st: AppState, list: Vec<Toast>) -> Element {
    let rows: Vec<(u64, String, String)> = list
        .iter()
        .map(|t| (
            t.id,
            t.msg.clone(),
            if t.is_err { "#c0392b" } else { "#2c3e50" }.to_string(),
        ))
        .collect();

    rsx! {
        div {
            style: "position:fixed;left:12px;bottom:34px;z-index:1100;display:flex;flex-direction:column;gap:6px",
            for (id, msg, bg) in rows.iter() {
                div {
                    key: "{id}",
                    style: "padding:8px 14px;border-radius:5px;color:#fff;box-shadow:0 2px 8px rgba(0,0,0,0.3);background:{bg}",
                    "{msg}"
                }
            }
        }
    }
}
