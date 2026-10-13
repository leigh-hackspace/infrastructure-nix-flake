//! switch-gui web UI — a Dioxus (wasm) SPA over the shadow copy.
//!
//! One page per switch, read from Postgres rather than from the switches: the
//! server's sync loop owns the Telnet sessions and writes every `show` into the
//! shadow copy (`src/shadow.rs`), so this SPA loads in the time it takes to fetch
//! a JSON document.  Every edit is queued as a `change` row and the page keeps
//! showing the port as pending until the queue says otherwise — nothing here ever
//! waits for a switch.
//!
//! The three files split along the API, not along a design pattern: `lib.rs` is
//! the router, the fetch helpers and the index; `page.rs` is `GET /api/switch`
//! (the port table, the VLAN table and the membership matrix); `queue.rs` is the
//! change rows (`/api/pending`, `/api/changes`) and their three actions.
//!
//! rsx notes (dioxus 0.7): text interpolation only captures identifiers in scope
//! (no trailing `name = expr` args), and `for` loop bodies / `match` arms must
//! each be a single node — so per-row locals are bound in a braced block ending
//! in one `rsx!`.

use dioxus::prelude::*;
use wasm_bindgen::JsCast;
use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use switch_gui_dto as dto;

mod page;
mod queue;

use page::SwitchView;

/// How often a page re-reads the shadow copy.
///
/// The sync's own tick is two seconds, so polling faster than that only re-serves
/// the same rows; slower and the "pending sync" notice outstays the change that
/// caused it.  Five seconds is the compromise, and it is a *read of Postgres* —
/// nothing here costs a switch a Telnet round trip.
pub const POLL_SECS: u64 = 5;

// ------------------------------------------------------------------ routing

/// Navigate the whole page.  Only for the sign-in round trip, which has to go
/// through the server: the session cookie is HttpOnly and the OIDC redirect is the
/// server's to make.
fn navigate(url: &str) {
    web_sys::window().expect("window").location().set_href(url).ok();
}

/// The path the browser is on, which is the route on first load.
fn current_path() -> String {
    web_sys::window()
        .and_then(|w| w.location().pathname().ok())
        .unwrap_or_else(|| "/".into())
}

/// The switch named by `path` (Some on `/switch/<name>`).
fn path_to_switch(path: &str) -> Option<String> {
    path.strip_prefix("/switch/")
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_end_matches('/').to_string())
}

/// Push a history entry for `path` (forward navigation only; does not fire
/// popstate, so the caller also sets the route signal directly).
fn push_path(path: &str) {
    if let Some(w) = web_sys::window() {
        if let Ok(h) = w.history() {
            let _ = h.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(path));
        }
    }
}

/// Install the popstate listener once and seed the route from the URL we actually
/// landed on, so a reload or a pasted `/switch/switch3` link opens switch3.
fn init_popstate(mut route: Signal<Option<String>>) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    route.set(path_to_switch(&current_path()));
    let closure = wasm_bindgen::closure::Closure::wrap(Box::new(move || {
        route.set(path_to_switch(&current_path()));
    }) as Box<dyn FnMut()>);
    // addEventListener wants the underlying JS Function; leak the Rust closure so
    // its Drop (which invalidates that function) never runs for the page's
    // lifetime.
    let listener: js_sys::Function = closure.as_js_value().clone().unchecked_into();
    let _ = window.add_event_listener_with_callback("popstate", &listener);
    std::mem::forget(closure);
}

// ---------------------------------------------------------------------- API

/// A signed-in user, from `GET /api/whoami`.
///
/// Only used as a fallback: `/api/state` carries the user too, and the SPA prefers
/// that, but a page that has to say a name before the first state read uses this.
#[derive(Clone, Debug, Deserialize)]
struct Whoami {
    user: String,
}

/// `GET url` and decode.
///
/// A non-JSON answer means the session expired: an unauthenticated request is
/// redirected to `/auth/login` by the server, `fetch` follows that, and what comes
/// back is authentik's HTML with a 200 on it.  Decoding that as `T` would be a
/// parse error the operator reads as "the API is broken", so the SPA notices first
/// and sends them to sign in — which is what the browser would have done had the
/// request been a page load.
async fn get_json<T: for<'de> Deserialize<'de>>(url: &str) -> Result<T, String> {
    let resp = Request::get(url).send().await.map_err(|e| format!("{url}: {e}"))?;
    decode(resp, url).await
}

/// `POST url` with a JSON body and decode the reply.
///
/// The status is deliberately not checked: `POST /api/intent` answers 200 with
/// `ok: false` when the operator's request is refused (a trunk member, a VLAN with
/// members, a name over the CLI's limit), and that sentence is the thing they need
/// to read.  Only a transport failure or a non-JSON reply is an `Err` here.
async fn post_json<B: Serialize, T: for<'de> Deserialize<'de>>(
    url: &str,
    body: &B,
) -> Result<T, String> {
    let body = serde_json::to_string(body).map_err(|e| e.to_string())?;
    let req = Request::post(url)
        .header("content-type", "application/json")
        .body(body)
        .map_err(|e| format!("{url}: {e}"))?;
    let resp = req.send().await.map_err(|e| format!("{url}: {e}"))?;
    decode(resp, url).await
}

async fn decode<T: for<'de> Deserialize<'de>>(
    resp: gloo_net::http::Response,
    url: &str,
) -> Result<T, String> {
    let ctype = resp
        .headers()
        .get("content-type")
        .unwrap_or_default()
        .to_string();
    if !ctype.contains("json") {
        navigate("/auth/login");
        return Err(format!("{url}: not a JSON answer (session expired?)"));
    }
    if !resp.ok() {
        return Err(format!("{url}: HTTP {}", resp.status()));
    }
    resp.json::<T>().await.map_err(|e| format!("{url}: {e}"))
}

// ------------------------------------------------------------------ helpers

/// "3s", "4m", "2h" — the age of the shadow copy, as the server computed it.
///
/// The number is the server's (`age_secs`) and never a browser clock minus a
/// timestamp: a laptop two timezones away would otherwise claim the switch was
/// read in the future.
fn age(secs: Option<i64>) -> String {
    match secs {
        None => "not read yet".to_string(),
        Some(s) if s < 60 => format!("{s}s"),
        Some(s) if s < 3600 => format!("{}m{:02}s", s / 60, s % 60),
        Some(s) => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// An em dash for a field the switch left blank, so a table never shows a gap that
/// could be a missing column.
fn dash(s: &str) -> &str {
    if s.is_empty() {
        "—"
    } else {
        s
    }
}

/// URL-encode a switch name for a query string.  The names are `switch1`-shaped,
/// but this is a query parameter and the day one has a `#` in it is the day a
/// hand-rolled `format!` silently asks for the wrong device.
fn enc(s: &str) -> String {
    js_sys::encode_uri_component(s)
        .as_string()
        .unwrap_or_else(|| s.to_string())
}

/// The switch list, as the nav bar on every page — the same rows the index shows,
/// so the two cannot disagree about which switches exist.
#[component]
fn Nav(current: String, route: Signal<Option<String>>) -> Element {
    let state = use_resource(|| async move { get_json::<dto::State>("/api/state").await });
    let rows = match state.read().as_ref() {
        Some(Ok(s)) => s.switches.clone(),
        _ => Vec::new(),
    };
    if rows.is_empty() {
        return rsx! {};
    }
    rsx! {
        nav {
            for r in rows {
                {
                    let here = r.name == current;
                    let to = r.name.clone();
                    let mut route = route.clone();
                    rsx! {
                        a {
                            href: "/switch/{to}",
                            class: if here { "badge cur" } else { "badge" },
                            onclick: move |e| {
                                e.prevent_default();
                                route.set(Some(to.clone()));
                                push_path(&format!("/switch/{to}"));
                            },
                            "{to}"
                        }
                    }
                }
            }
        }
    }
}

/// The banner every action writes its result into.  `None` renders nothing: the
/// page is drawn before anything has happened, and an empty box that says "no
/// message" is worse than no box.
#[component]
fn Note(note: Signal<Option<(bool, String)>>) -> Element {
    let n = note.read().clone();
    match n {
        None => rsx! {},
        Some((ok, text)) => rsx! {
            div {
                // `banner` pins it: this is the only place an action's answer
                // appears, and the matrix it answers is at the bottom of a long
                // page.  Double-click still clears it, which is how an operator
                // gets the pinned line out of the way once they have read it.
                class: if ok { "note banner ok" } else { "note banner err" },
                ondoubleclick: move |_| note.set(None),
                "{text}"
            }
        },
    }
}

/// The signed-in name, which the server knows and the SPA only ever displays.
#[component]
fn UserLine(user: Signal<String>) -> Element {
    let u = user.read().clone();
    if u.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "muted", style: "margin:-8px 0 14px", "signed in as {u}" }
    }
}

#[component]
fn App() -> Element {
    let route = use_signal(|| path_to_switch(&current_path()));
    let user = use_signal(String::new);

    // The browser's back/forward buttons drive the route (runs once: no reactive
    // reads inside, and `route` is Copy).
    use_effect(move || {
        init_popstate(route);
    });

    use_effect(move || {
        let mut user = user.clone();
        spawn(async move {
            if let Ok(w) = get_json::<Whoami>("/api/whoami").await {
                user.set(w.user);
            }
        });
    });

    let name = route.read().clone();
    // `key` so switching switches rebuilds the page's state instead of reusing
    // the previous switch's signals: a stale port table under a new header is the
    // kind of bug that gets a port renamed on the wrong device.
    rsx! {
        div { class: "app",
            if let Some(name) = name {
                SwitchView { key: "{name}", name: name.clone(), route, user }
            } else {
                Index { route, user }
            }
        }
    }
}

/// The landing page: one row per switch, so a switch that is down is visible
/// before anyone has to click into it and find an empty table.
#[component]
fn Index(route: Signal<Option<String>>, user: Signal<String>) -> Element {
    let state = use_resource(|| async move { get_json::<dto::State>("/api/state").await });
    let res = state.read().clone();
    let (rows, ready, err) = match res.as_ref() {
        Some(Ok(s)) => (s.switches.clone(), s.ready, None),
        Some(Err(e)) => (Vec::new(), false, Some(e.clone())),
        None => (Vec::new(), false, None),
    };

    rsx! {
        header {
            div {
                h1 { "network switches" }
                div { class: "muted", "ProCurve, read over Telnet, shadowed in Postgres" }
            }
            div { class: "spacer" }
            a { href: "/logout", button { "Sign out" } }
        }
        UserLine { user }
        if let Some(e) = err.as_ref() {
            div { class: "note err", "{e}" }
        }
        if !ready && err.is_none() {
            div { class: "note info",
                "Nothing has been read yet. The sync reads every switch on its own \
                 interval — this page will fill in by itself."
            }
        }
        table {
            thead {
                tr {
                    th { "Switch" }
                    th { "Address" }
                    th { "System" }
                    th { "Location" }
                    th { "Revision" }
                    th { class: "n", "Ports" }
                    th { class: "n", "VLANs" }
                    th { class: "n", "Queue" }
                    th { "Shadow copy" }
                }
            }
            tbody {
                for r in rows.iter() {
                    {
                        let name = r.name.clone();
                        let mut route = route.clone();
                        let dead = r.error.is_some() && r.ports == 0;
                        let pending = r.pending;
                        let privilege = r.privilege.clone();
                        rsx! {
                            tr {
                                td {
                                    b {
                                        a {
                                            href: "/switch/{name}",
                                            onclick: move |e| {
                                                e.prevent_default();
                                                route.set(Some(name.clone()));
                                                push_path(&format!("/switch/{name}"));
                                            },
                                            "{name}"
                                        }
                                    }
                                }
                                td { class: "muted", "{r.address}" }
                                td { "{dash(&r.system_name)}" }
                                td { class: "muted", "{dash(&r.location)}" }
                                td { class: "muted", "{dash(&r.software)}" }
                                td { class: "n", if dead { "—" } else { "{r.ports}" } }
                                td { class: "n", if dead { "—" } else { "{r.vlans}" } }
                                td {
                                    class: "n",
                                    if pending > 0 {
                                        span { class: "badge pending", title: "changes queued for this switch", "{pending}" }
                                    } else {
                                        span { class: "muted", "—" }
                                    }
                                }
                                td {
                                    if let Some(e) = &r.error {
                                        span { class: "err", title: "{e}", "{age(r.age_secs)} · failed" }
                                    } else {
                                        span { class: "muted", "{age(r.age_secs)}" }
                                    }
                                    if privilege == "operator" {
                                        span { class: "badge ro", title: "the CLI session is in the Operator context: this tool cannot write to this switch", "Operator" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if rows.is_empty() && err.is_none() {
            p { class: "empty", "No switches configured." }
        }
        p { class: "muted", "switch-gui · read over Telnet, written with the ProCurve CLI, audited in Postgres." }
    }
}

/// A row of the membership matrix as the operator has it **now**: the untagged
/// VLAN as the select shows it ("" = leave it alone) and the tagged set.
///
/// Only the rows the operator has touched are stored; everything else renders
/// straight from the shadow copy.  That is what lets the poll keep running: the
/// snapshot is the truth and the edits are an overlay on it, so a refresh cannot
/// lose a tick and cannot invent one.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RowEdit {
    pub native: String,
    pub tagged: Vec<i64>,
}

/// The row as the switch has it.
pub fn row_from_snapshot(p: &dto::Port) -> RowEdit {
    RowEdit {
        native: p.native.map(|v| v.to_string()).unwrap_or_default(),
        tagged: p.tagged.clone(),
    }
}

/// The row as the form has it — the edit if there is one, the switch otherwise.
pub fn row_wanted(edits: &BTreeMap<String, RowEdit>, p: &dto::Port) -> RowEdit {
    edits
        .get(&p.id)
        .cloned()
        .unwrap_or_else(|| row_from_snapshot(p))
}

/// Whether a row is about to be written.  "Different from the switch", not
/// "different from what the page guessed" — the same rule the server-rendered page
/// used, and the reason a row ticked back to its original state stops being dirty.
pub fn row_dirty(edits: &BTreeMap<String, RowEdit>, p: &dto::Port) -> bool {
    row_wanted(edits, p) != row_from_snapshot(p)
}

/// Any row dirty?  Used to pause the poll while the operator is mid-edit.
pub fn any_dirty(edits: &BTreeMap<String, RowEdit>) -> bool {
    !edits.is_empty()
}

// ---------------------------------------------------------------- entry point

/// Wasm entry point: runs when the module initialises (the JS glue's `init()`).
///
/// This is what roots the component tree.  Without it nothing below [`App`] is
/// reachable and the whole crate compiles to a pile of dead code — which is a
/// warning here and a blank page in the browser.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen(start))]
pub fn start() {
    dioxus::launch(App);
}

/// Host entry point (so `cargo run` still type-checks off-wasm).
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    dioxus::launch(App);
}
