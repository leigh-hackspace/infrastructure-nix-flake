//! Modal dialogs: new folder, new file, rename, delete confirmation.

use dioxus::prelude::*;

use crate::api::{join_rel, post_json};
use crate::state::*;
use crate::ui::TBTN;

/// Apply a name modal (new folder / new file / rename) in the current dir.
fn submit_modal(st: AppState, modal: Modal, name: &str) {
    let dir = st.path.read().clone();
    let modal = modal.clone();
    let name = name.to_string();
    spawn(async move {
        let (url, body) = match &modal {
            Modal::NewFolder => (
                "/api/mkdir",
                serde_json::json!({ "path": join_rel(&dir, &name) }),
            ),
            Modal::NewFile => (
                "/api/create",
                serde_json::json!({ "path": join_rel(&dir, &name) }),
            ),
            Modal::Rename(old) => (
                "/api/rename",
                serde_json::json!({
                    "from": join_rel(&dir, old),
                    "to": join_rel(&dir, &name),
                }),
            ),
            _ => return,
        };
        match post_json(url, &body).await {
            Ok(()) => {
                toast(st, &format!("ok: {name}"), false);
                load_dir(st);
            }
            Err(e) => toast(st, &e, true),
        }
    });
}

#[component]
pub fn ModalBox(st: AppState, modal: Modal) -> Element {
    let is_delete = matches!(modal, Modal::Delete(_));
    let initial: String = match &modal {
        Modal::Rename(old) => old.clone(),
        _ => String::new(),
    };
    let del_list: Vec<String> = match &modal {
        Modal::Delete(items) => items.clone(),
        _ => Vec::new(),
    };
    let del_extra: usize = del_list.len().saturating_sub(50);
    let mut input = use_signal(move || initial);

    // focus + select the input on open
    use_effect(move || {
        let _ = js_eval(
            "var el=document.getElementById('fs-modal-input'); if(el){el.focus(); el.select();}",
        );
    });

    // Enter-to-submit handler (each is a separate move closure, so each
    // captures its own copy of the state bundle).
    let on_enter = |mut st: AppState, input: Signal<String>, modal: Modal| {
        move |e: dioxus::events::KeyboardEvent| {
            if e.key() == dioxus::html::Key::Enter {
                let name = input.read().trim().to_string();
                if !name.is_empty() {
                    st.modal.set(None);
                    submit_modal(st, modal.clone(), &name);
                }
            }
            if e.key() == dioxus::html::Key::Escape {
                st.modal.set(None);
            }
        }
    };

    let input_style = "width:100%;margin-top:6px;padding:4px 6px;border:1px solid #aaa;border-radius:3px;box-sizing:border-box";

    let name_field = |label: &str| -> Element {
        rsx! {
            div { "{label}" }
            input {
                id: "fs-modal-input",
                value: input,
                style: input_style,
                oninput: move |e| input.set(e.value()),
                onkeydown: on_enter(st, input, modal.clone()),
            }
        }
    };

    rsx! {
        div {
            style: "position:fixed;inset:0;z-index:900;background:rgba(0,0,0,0.25);display:flex;align-items:center;justify-content:center",
            onmousedown: move |_| st.modal.set(None),
            div {
                style: "width:400px;max-width:90vw;background:#fff;border:1px solid #888;border-radius:6px;box-shadow:0 4px 18px rgba(0,0,0,0.3);padding:16px",
                onmousedown: move |e| e.stop_propagation(),

                if matches!(modal, Modal::NewFolder) {
                    {name_field("New folder name:")}
                }
                if matches!(modal, Modal::NewFile) {
                    {name_field("New file name:")}
                }
                if let Some(old) = match &modal {
                    Modal::Rename(old) => Some(old.clone()),
                    _ => None,
                } {
                    div { "Rename " b { "\"{old}\"" } " to:" }
                    {name_field("")}
                }
                if !del_list.is_empty() {
                    div {
                        "Delete "
                        b { "{del_list.len()} item(s)?" }
                        " This cannot be undone."
                    }
                    div {
                        style: "margin-top:8px;max-height:140px;overflow:auto;color:#555;font-family:monospace;font-size:12px",
                        for i in del_list.iter().take(50).cloned() {
                            div { "{i}" }
                        }
                        if del_extra > 0 {
                            div { "… and {del_extra} more" }
                        }
                    }
                }

                div {
                    style: "display:flex;justify-content:flex-end;gap:8px;margin-top:14px",
                    button {
                        style: TBTN,
                        onclick: move |_| st.modal.set(None),
                        "Cancel"
                    }
                    if !is_delete {
                        button {
                            style: TBTN,
                            onclick: move |_| {
                                let name = input.read().trim().to_string();
                                if !name.is_empty() {
                                    st.modal.set(None);
                                    submit_modal(st, modal.clone(), &name);
                                }
                            },
                            "OK"
                        }
                    } else {
                        button {
                            style: "padding:3px 12px;border:1px solid #a33;background:#e55;color:#fff;border-radius:3px;cursor:pointer",
                            onclick: move |_| {
                                if del_list.is_empty() {
                                    return;
                                }
                                let items = del_list.clone();
                                st.modal.set(None);
                                spawn(async move {
                                    let n = items.len();
                                    match post_json("/api/delete", &serde_json::json!({ "paths": items })).await {
                                        Ok(()) => {
                                            toast(st, &format!("deleted {n} item(s)"), false);
                                            load_dir(st);
                                        }
                                        Err(e) => toast(st, &e, true),
                                    }
                                });
                            },
                            "Delete"
                        }
                    }
                }
            }
        }
    }
}
