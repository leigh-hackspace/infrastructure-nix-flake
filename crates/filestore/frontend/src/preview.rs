//! Popup preview.  Media the browser can render itself (image, video, audio,
//! PDF) is pointed at the preview URL — no JSON round trip, the element streams
//! it.  Text is fetched here (truncated) and shown as source.
//!
//! Which kind a file is comes from the shared table in common-rs/preview, the
//! same one the server classifies with, so the menu can never offer a Preview the
//! API refuses.

use dioxus::prelude::*;
use serde::Deserialize;

use crate::api::*;
use crate::grid::sorted;
use crate::state::*;
use crate::ui::TBTN;
use common_preview::Kind;

#[derive(Deserialize)]
struct Prev {
    kind: String,
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    text: String,
}

/// Open a preview for a file: media immediately (the element fetches the URL),
/// text after fetching it.
pub fn open_preview(mut st: AppState, path: &str, name: &str) {
    st.ctx.set(None);
    match common_preview::kind(name) {
        Some(Kind::Image | Kind::Video | Kind::Audio | Kind::Pdf) => {
            st.preview.set(Some(Preview {
                path: path.to_string(),
                name: name.to_string(),
                state: PreviewState::Render(common_preview::kind(name).unwrap()),
            }));
        }
        Some(Kind::Text) => {
            st.preview.set(Some(Preview {
                path: path.to_string(),
                name: name.to_string(),
                state: PreviewState::Loading,
            }));
            let path = path.to_string();
            spawn_task(async move {
                let state = match get_json::<Prev>(&preview_url(&path)).await {
                    Ok(p) if p.kind == "text" => PreviewState::Text(p.text, p.truncated),
                    Ok(_) => PreviewState::Error("not previewable".into()),
                    Err(e) => PreviewState::Error(e),
                };
                set_preview(st, &path, state);
            });
        }
        None => st.preview.set(Some(Preview {
            path: path.to_string(),
            name: name.to_string(),
            state: PreviewState::Error("not previewable".into()),
        })),
    }
}

/// Arrow keys step the preview to the previous/next previewable file in the
/// current folder, in the order the grid shows them, and the selection follows so
/// closing the popup leaves you on the file you last looked at.
///
/// It steps across every previewable file rather than only images: a folder of
/// images with a notes.txt in it steps through the text file too, because that is
/// the order on screen.  It clamps at the ends rather than wrapping.
pub fn step_preview(mut st: AppState, delta: i64) {
    let entries = sorted(&st);
    let Some(pv) = st.preview.read().clone() else {
        return;
    };
    let names: Vec<String> = entries
        .iter()
        .filter(|e| !e.is_dir && is_previewable(&e.name))
        .map(|e| e.name.clone())
        .collect();
    let Some(pos) = names.iter().position(|n| *n == pv.name) else {
        return;
    };
    let next = (pos as i64 + delta).clamp(0, names.len() as i64 - 1) as usize;
    if next == pos {
        return;
    }
    let name = names[next].clone();
    let row = entries.iter().position(|e| e.name == name);
    let full = join_rel(&st.path.read(), &name);

    open_preview(st, &full, &name);

    let mut s = st.sel.read().clone();
    s.clear();
    s.insert(name.clone());
    st.sel.set(s);
    if let Some(i) = row {
        st.focus.set(Some(i));
    }
    scroll_row_into_view(&name);
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

const MEDIA: &str = "max-width:100%;max-height:68vh;display:block";

/// The element a kind is rendered in.  Each one streams the preview URL itself;
/// the server answers a Range request, so scrubbing a video does not re-download
/// the whole file.
fn media_element(kind: Kind, src: &str, name: &str) -> Element {
    match kind {
        Kind::Image => rsx! {
            img {
                src: src.to_string(),
                style: MEDIA,
                alt: name.to_string(),
            }
        },
        Kind::Video => rsx! {
            video {
                src: src.to_string(),
                controls: true,
                style: MEDIA,
            }
        },
        Kind::Audio => rsx! {
            audio {
                src: src.to_string(),
                controls: true,
                style: "width:100%;min-width:320px",
            }
        },
        Kind::Pdf => rsx! {
            iframe {
                src: src.to_string(),
                style: "width:78vw;height:70vh;border:0;background:#eee;display:block",
                title: name.to_string(),
            }
        },
        // Text is not a media element: it is fetched as JSON and shown as source.
        Kind::Text => rsx! { div { "not previewable" } },
    }
}

#[component]
pub fn PreviewBox(st: AppState, preview: Preview) -> Element {
    let src = preview_url(&preview.path);
    let body = match &preview.state {
        PreviewState::Loading => rsx! {
            div {
                style: "color:#777",
                "loading…"
            }
        },
        PreviewState::Render(kind) => media_element(*kind, &src, &preview.name),
        PreviewState::Text(text, truncated) => rsx! {
            pre {
                style: "margin:0;font-size:12.5px;line-height:1.45;white-space:pre-wrap;word-break:break-word",
                {text.clone()}
                if *truncated {
                    div {
                        style: "color:#999;margin-top:8px",
                        "— preview truncated (first 512 KiB) —"
                    }
                }
            }
        },
        PreviewState::Error(e) => rsx! {
            div {
                style: "color:#c00",
                "Cannot preview: {e}"
            }
        },
    };

    let label = match common_preview::kind(&preview.name) {
        Some(k) => kind_label(k),
        None => "no inline preview",
    };

    rsx! {
        div {
            id: "fs-preview",
            style: "position:fixed;inset:0;z-index:800;background:rgba(0,0,0,0.45);display:flex;align-items:center;justify-content:center",
            onmousedown: move |_| st.preview.set(None),
            div {
                style: "max-width:86vw;max-height:86vh;background:#fff;border-radius:6px;box-shadow:0 6px 24px rgba(0,0,0,0.4);display:flex;flex-direction:column;overflow:hidden",
                onmousedown: move |e| e.stop_propagation(),
                div {
                    style: "display:flex;align-items:center;gap:8px;padding:8px 12px;background:#f0f0f0;border-bottom:1px solid #ccc",
                    b {
                        id: "fs-preview-name",
                        "{preview.name}"
                    }
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
                    {body}
                }
                div {
                    id: "fs-preview-kind",
                    style: "padding:6px 12px;background:#f0f0f0;border-top:1px solid #ccc;font-size:11.5px;color:#666",
                    "{label}"
                }
            }
        }
    }
}

fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Image => "image",
        Kind::Video => "video",
        Kind::Audio => "audio",
        Kind::Pdf => "pdf",
        Kind::Text => "text",
    }
}
