//! Popup preview: images (streamed through <img>) and text (truncated).

use dioxus::prelude::*;
use serde::Deserialize;

use crate::api::*;
use crate::state::*;
use crate::ui::TBTN;

#[derive(Deserialize)]
struct Prev {
    kind: String,
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    text: String,
}

/// Open a preview for a file: images immediately, text after fetching.
pub fn open_preview(mut st: AppState, path: &str, name: &str) {
    st.ctx.set(None);
    if is_image(name) {
        st.preview.set(Some(Preview {
            path: path.to_string(),
            name: name.to_string(),
            state: PreviewState::Image,
        }));
        return;
    }
    st.preview.set(Some(Preview {
        path: path.to_string(),
        name: name.to_string(),
        state: PreviewState::Loading,
    }));
    let path = path.to_string();
    spawn(async move {
        let state = match get_json::<Prev>(&preview_url(&path)).await {
            Ok(p) if p.kind == "text" => PreviewState::Text(p.text, p.truncated),
            Ok(_) => PreviewState::Error("not previewable".into()),
            Err(e) => PreviewState::Error(e),
        };
        set_preview(st, &path, state);
    });
}

fn set_preview(mut st: AppState, path: &str, state: PreviewState) {
    let cur = st.preview.read().clone();
    if let Some(mut pv) = cur {
        if pv.path == path {
            pv.state = state;
            st.preview.set(Some(pv));
        }
    }
}

#[component]
pub fn PreviewBox(st: AppState, preview: Preview) -> Element {
    let img_src = if is_image(&preview.name) {
        Some(preview_url(&preview.path))
    } else {
        None
    };
    let text: Option<String> = match &preview.state {
        PreviewState::Text(t, _) => Some(t.clone()),
        _ => None,
    };
    let truncated = matches!(&preview.state, PreviewState::Text(_, true));
    let err = match &preview.state {
        PreviewState::Error(e) => Some(e.clone()),
        _ => None,
    };

    let loading = matches!(preview.state, PreviewState::Loading);
    let is_image_state = matches!(preview.state, PreviewState::Image);
    let is_text_state = matches!(preview.state, PreviewState::Text(_, _));
    let is_err_state = matches!(preview.state, PreviewState::Error(_));

    rsx! {
        div {
            style: "position:fixed;inset:0;z-index:800;background:rgba(0,0,0,0.45);display:flex;align-items:center;justify-content:center",
            onmousedown: move |_| st.preview.set(None),
            div {
                style: "max-width:86vw;max-height:86vh;background:#fff;border-radius:6px;box-shadow:0 6px 24px rgba(0,0,0,0.4);display:flex;flex-direction:column;overflow:hidden",
                onmousedown: move |e| e.stop_propagation(),
                div {
                    style: "display:flex;align-items:center;gap:8px;padding:8px 12px;background:#f0f0f0;border-bottom:1px solid #ccc",
                    b { "{preview.name}" }
                    div { style: "flex:1" }
                    button {
                        style: TBTN,
                        title: "Download",
                        onclick: move |_| download(&download_url(&preview.path)),
                        "⬇"
                    }
                    button {
                        style: TBTN,
                        onclick: move |_| st.preview.set(None),
                        "✕"
                    }
                }
                div {
                    style: "overflow:auto;padding:12px;min-width:240px;min-height:120px",
                    if loading {
                        div {
                            style: "color:#777",
                            "loading…"
                        }
                    }
                    if is_image_state {
                        if img_src.is_some() {
                            img {
                                src: img_src.clone().unwrap_or_default(),
                                style: "max-width:100%;max-height:68vh;display:block",
                                alt: "{preview.name}"
                            }
                        }
                    }
                    if is_text_state {
                        if text.is_some() {
                            pre {
                                style: "margin:0;font-size:12.5px;line-height:1.45;white-space:pre-wrap;word-break:break-word",
                                {text.clone().unwrap_or_default()}
                                if truncated {
                                    div {
                                        style: "color:#999;margin-top:8px",
                                        "— preview truncated (first 512 KiB) —"
                                    }
                                }
                            }
                        }
                    }
                    if is_err_state {
                        if err.is_some() {
                            div {
                                style: "color:#c00",
                                "Cannot preview: {err.clone().unwrap_or_default()}"
                            }
                        }
                    }
                }
            }
        }
    }
}
