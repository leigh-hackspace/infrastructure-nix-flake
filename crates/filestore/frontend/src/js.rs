//! JS glue: external drag-drop walking + XHR uploads with progress, plus
//! internal move/copy and file-picker uploads.

use std::time::Duration;

use dioxus::prelude::*;
use gloo_timers::future::sleep;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

use crate::api::*;
use crate::state::*;

/// Register the global JS glue once: an upload helper with progress
/// (window.__fs_uploads, polled from Rust) and global drop listeners that
/// walk webkitGetAsEntry trees (files AND directories), plus the change
/// handler for the file-picker input.  Internal drags (application/x-filestore)
/// are handled by the Dioxus row handlers.
pub fn ensure_js_glue(mut st: AppState) {
    static ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if ONCE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }

    let js = r#"
        window.__fs_uploads = [];
        window.__fs_upload_one = function(dir, rel, file) {
            return new Promise(function(resolve, reject) {
                var xhr = new XMLHttpRequest();
                var url = '/api/upload?path=' + encodeURIComponent(dir) + '&name=' + encodeURIComponent(rel);
                var entry = { rel: rel, pct: 0, done: false, error: null };
                window.__fs_uploads.push(entry);
                function update(pct, done, error) { entry.pct = pct; entry.done = done; entry.error = error; }
                xhr.open('POST', url);
                xhr.upload.addEventListener('progress', function(e) {
                    if (e.lengthComputable) update(e.loaded / e.total, false, null);
                });
                xhr.onload = function() {
                    if (xhr.status >= 200 && xhr.status < 300) {
                        update(1, true, null);
                        setTimeout(function(){ window.__fs_uploads = window.__fs_uploads.filter(function(x){return x !== entry;}); }, 2500);
                        resolve();
                    } else {
                        var msg = 'HTTP ' + xhr.status;
                        try { msg = JSON.parse(xhr.responseText).error || msg; } catch (e2) {}
                        update(0, true, msg);
                        setTimeout(function(){ window.__fs_uploads = window.__fs_uploads.filter(function(x){return x !== entry;}); }, 6000);
                        reject(new Error(msg));
                    }
                };
                xhr.onerror = function() {
                    update(0, true, 'network error');
                    setTimeout(function(){ window.__fs_uploads = window.__fs_uploads.filter(function(x){return x !== entry;}); }, 6000);
                    reject(new Error('network error'));
                };
                xhr.send(file);
            });
        };
        // Walk a DataTransfer: files and directories (webkitGetAsEntry).
        // Returns [{rel, file}, …] with rel relative to the drop target.
        window.__fs_entries = async function(dt) {
            var items = Array.from(dt.items || []).filter(function(i){ return i.kind === 'file'; });
            var entries = items.map(function(i){ return (i.webkitGetAsEntry) ? i.webkitGetAsEntry() : null; }).filter(Boolean);
            if (entries.length === 0) {
                return Array.from(dt.files || []).map(function(f){ return { rel: f.name, file: f }; });
            }
            var out = [];
            var readEntry = async function(entry, prefix) {
                if (entry.isFile) {
                    var file = await new Promise(function(res, rej){ entry.file(res, rej); });
                    out.push({ rel: prefix + entry.name, file: file });
                } else if (entry.isDirectory) {
                    var reader = entry.createReader();
                    var batch;
                    do {
                        batch = await new Promise(function(res, rej){ reader.readEntries(res, rej); });
                        for (var e of batch) await readEntry(e, prefix + entry.name + '/');
                    } while (batch.length);
                }
            };
            for (var e of entries) await readEntry(e, '');
            return out;
        };
        window.__fs_do_uploads = async function(files, dir) {
            var failed = [];
            for (var f of files) {
                var rel = f.webkitRelativePath || f.name;
                try { await window.__fs_upload_one(dir, rel, f); }
                catch (err) { failed.push(rel + ': ' + err.message); }
            }
            return { count: files.length, failed: failed };
        };
        document.addEventListener('dragover', function(ev) {
            var area = document.getElementById('file-area');
            if (!area || !area.contains(ev.target)) return;
            var dt = ev.dataTransfer;
            if (dt && Array.from(dt.types || []).indexOf('application/x-filestore') === -1) {
                ev.preventDefault();
                dt.dropEffect = 'copy';
            }
        });
        document.addEventListener('drop', function(ev) {
            var area = document.getElementById('file-area');
            if (!area || !area.contains(ev.target)) return;
            var dt = ev.dataTransfer;
            if (!dt) return;
            if (Array.from(dt.types || []).indexOf('application/x-filestore') !== -1) return;
            ev.preventDefault();
            var row = ev.target.closest ? ev.target.closest('[data-fs-path]') : null;
            var attr = row ? row.getAttribute('data-fs-path') : null;
            var dir = (attr && attr.length > 0) ? attr : (window.__fs_current_dir || '');
            window.__fs_do_uploads(Array.from(dt.files || []), dir);
        });
        var picker = document.getElementById('fs-upload-input');
        if (picker) {
            picker.addEventListener('change', function(ev) {
                var files = Array.from(ev.target.files || []);
                if (files.length === 0) return;
                window.__fs_do_uploads(files, window.__fs_current_dir || '');
                ev.target.value = '';
            });
        }
    "#;
    let _ = js_sys::eval(js);

    // Keep window.__fs_current_dir in sync, mirror upload progress into the
    // signal, and refresh the dir once a batch finishes.
    spawn_task(async move {
        let mut prev = 0usize;
        loop {
            let dir = st.path.read().clone();
            let js = format!(r#"window.__fs_current_dir = {};"#, serde_json::json!(dir));
            let _ = js_eval(&js);
            if let Some(s) = js_eval_string(r#"JSON.stringify(window.__fs_uploads || [])"#) {
                if let Ok(v) = serde_json::from_str::<Vec<Upload>>(&s) {
                    let now = v.len();
                    st.uploads.set(v);
                    if prev > 0 && now == 0 {
                        load_dir(st);
                    }
                    prev = now;
                }
            }
            sleep(Duration::from_millis(400)).await;
        }
    });
}

/// An event target (or a `dragleave`'s relatedTarget) as an element.
fn as_el(t: &Option<web_sys::EventTarget>) -> Option<&web_sys::Element> {
    t.as_ref()?.dyn_ref::<web_sys::Element>()
}

/// The directory row a drop would land in, for the element under the pointer (or the
/// element a `dragleave` is moving to).
///
/// Files carry an empty `data-fs-path`, so only directory rows are targets — a file
/// row can never receive a drop.
fn drop_row(el: Option<&web_sys::Element>) -> Option<String> {
    let row = el?.closest("[data-fs-path]").ok()?;
    let row = row?;
    if row.get_attribute("data-fs-path")?.is_empty() {
        return None;
    }
    row.get_attribute("data-fs-name")
}

/// Mark the row the pointer is over during a drag, so it can show that releasing
/// there drops into it.
///
/// This has to run on the real event rather than in a Dioxus row handler:
/// `relatedTarget` is what tells a real leave apart from crossing from a row's icon
/// to its own label, and `DragData` does not expose it.  A row-level `dragleave`
/// would clear the highlight on every child crossing, so the target flickered while
/// the pointer was still inside the row it was on.
pub fn ensure_drag_hover(mut st: AppState) {
    static ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if ONCE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }

    let handler: Closure<dyn FnMut(wasm_bindgen::JsValue)> = Closure::new(move |v: wasm_bindgen::JsValue| {
        let Ok(e) = v.dyn_into::<web_sys::DragEvent>() else {
            return;
        };
        // DragEvent extends MouseEvent, which is where relatedTarget and the modifier
        // state live.
        let me = e.unchecked_ref::<web_sys::MouseEvent>();
        match e.type_().as_str() {
            "dragover" => {
                set_drop_target(st, drop_row(as_el(&e.target())));
                // ctrl/cmd held right now, so the badge and the drop cannot disagree.
                set_drag_copy(st, me.ctrl_key() || me.meta_key());
            }
            "dragleave" => {
                // Only a leave that really exits the row: a relatedTarget still inside
                // the same row is the pointer crossing one of its children.
                let to = drop_row(as_el(&me.related_target()));
                if to.is_none() {
                    set_drop_target(st, None);
                }
            }
            "drop" => set_drop_target(st, None),
            // dragend is the only event that always runs — it fires on the row the drag
            // started from after the drop, and also when the drop was refused or the
            // drag left the page.  It is what ends the drag.
            "dragend" => {
                st.drag.set(None);
                set_drop_target(st, None);
            }
            _ => {}
        }
    });

    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        let target: &web_sys::EventTarget = doc.unchecked_ref();
        let func: &js_sys::Function = handler.as_js_value().unchecked_ref::<js_sys::Function>();
        for ev in ["dragover", "dragleave", "drop", "dragend"] {
            let _ = target.add_event_listener_with_callback(ev, func);
        }
        std::mem::forget(handler);
    }
}

/// Move (default) or copy (ctrl) internal paths into `dest_dir`.
pub fn move_or_copy(st: AppState, srcs: Vec<String>, dest_dir: String, copy: bool) {
    spawn_task(async move {
        let dir = st.path.read().clone();
        let mut errors = Vec::new();
        for s in &srcs {
            let from = join_rel(&dir, s);
            let to = join_rel(&dest_dir, s.rsplit('/').next().unwrap_or(s));
            let url = if copy { "/api/copy" } else { "/api/rename" };
            match post_json(url, &serde_json::json!({ "from": from, "to": to })).await {
                Ok(()) => {}
                Err(e) => errors.push(format!("{s}: {e}")),
            }
        }
        if errors.is_empty() {
            let verb = if copy { "copied" } else { "moved" };
            toast(st, &format!("{verb} {} item(s)", srcs.len()), false);
        } else {
            toast(st, &errors.join("; "), true);
        }
        load_dir(st);
    });
}

/// Whether a dioxus MouseData/KeyboardData has Ctrl or Meta (Cmd) held.
pub fn ctrl_held(mods: dioxus::html::Modifiers) -> bool {
    mods.ctrl() || mods.meta()
}
