//! frigate-monitor web UI — a Dioxus (wasm) SPA.
//!
//! Shows recorded scene-change events as an infinitely-scrolling grid of
//! thumbnails.  Clicking a thumbnail opens a popup showing the before/after/
//! diff images (changed regions are outlined as rectangles on the images)
//! plus zoom crops of the largest region.  The grid stays mounted under the
//! fixed popup, so the scroll position is preserved when it closes.

use dioxus::prelude::*;
use gloo_net::http::Request;
use serde::Deserialize;

const PAGE: u32 = 24;

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Region {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct EventMeta {
    id: u64,
    ts: String,
    width: u32,
    height: u32,
    regions: Vec<Region>,
}

#[derive(Debug, Deserialize)]
struct EventPage {
    has_more: bool,
    events: Vec<EventMeta>,
}

#[derive(Clone, Debug, Deserialize)]
struct Status {
    frames: u64,
    last_frame_ts: i64,
    scene_resets: u64,    #[serde(default)]
    last_error: String,
}

fn file_url(id: u64, name: &str) -> String {
    format!("/files/{id}/{name}")
}

async fn get_json<T: for<'de> Deserialize<'de>>(url: &str) -> Result<T, String> {
    let resp = Request::get(url).send().await.map_err(|e| format!("{url}: {e}"))?;
    if !resp.ok() {
        return Err(format!("{url}: HTTP {}", resp.status()));
    }
    resp.json::<T>().await.map_err(|e| format!("json: {e}"))
}

fn fmt_age(ts: i64) -> String {
    if ts <= 0 {
        return "no frame yet".into();
    }
    let age = now_unix() - ts;
    if age < 90 {
        format!("{age}s ago")
    } else {
        format!("{}m ago", age / 60)
    }
}

// Wall-clock seconds.  std::time::SystemTime is NOT implemented on
// wasm32-unknown-unknown (it panics with "time not implemented on this
// platform"), so on wasm we read the clock from JS instead.
fn now_unix() -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        (date_now_ms() / 1000.0) as i64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    #[wasm_bindgen::prelude::wasm_bindgen(js_namespace = Date, js_name = now)]
    fn date_now_ms() -> f64;
}

#[allow(non_snake_case)]
fn App() -> Element {
    let events = use_signal(Vec::<EventMeta>::new);
    let has_more = use_signal(|| true);
    let mut loading = use_signal(|| false);
    let mut detail = use_signal(|| None::<EventMeta>);
    let status = use_signal(|| None::<Status>);
    let mut at_top = use_signal(|| true);
    let live_key = use_signal(|| 0u64);

    // Load the first page on mount.
    use_effect(move || {
        spawn(async move {
            let mut events = events;
            let mut has_more = has_more;
            if let Ok(page) = get_json::<EventPage>(&format!("/api/events?limit={PAGE}")).await {
                events.set(page.events);
                has_more.set(page.has_more);
            }
        });
    });

    // One long-lived bridge task: Esc closes the popup; a 15 s tick updates
    // the status line, refreshes the live frame and prepends brand-new
    // events when the user is at the top (never mid-scroll, so the grid does
    // not jump).
    use_effect(move || {
        let mut status = status;
        let mut detail = detail;
        let mut events = events;
        let mut live_key = live_key;
        spawn(async move {
            let mut eval = document::eval(
                r#"
                const onKey = (e) => {
                    if (e.key === 'Escape') dioxus.send('esc');
                };
                document.addEventListener('keydown', onKey);
                setInterval(() => dioxus.send('tick'), 15000);
                "#,
            );
            loop {
                let msg: String = match eval.recv().await {
                    Ok(m) => m,
                    Err(_) => break,
                };
                match msg.as_str() {
                    "esc" => {
                        if detail.read().is_some() {
                            detail.set(None);
                        }
                    }
                    "tick" => {
                        *live_key.write() += 1;
                        if let Ok(st) = get_json::<Status>("/api/status").await {
                            status.set(Some(st));
                        }
                        if *at_top.read() && !events.read().is_empty() {
                            if let Ok(page) =
                                get_json::<EventPage>(&format!("/api/events?limit={PAGE}")).await
                            {
                                let known: std::collections::HashSet<u64> =
                                    events.read().iter().map(|e| e.id).collect();
                                let fresh: Vec<EventMeta> = page
                                    .events
                                    .into_iter()
                                    .filter(|e| !known.contains(&e.id))
                                    .collect();
                                if !fresh.is_empty() {
                                    let mut cur = events.write();
                                    cur.splice(0..0, fresh);
                                    drop(cur);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        });
    });

    // Append the next, older page (infinite scroll), triggered by the
    // scroller's onscroll handler. `use_callback` gives a Copy handle so the
    // handler can call it.
    let load_more = use_callback(move |_: ()| {
        if *loading.read() || !*has_more.read() {
            return;
        }
        let cursor = events.read().last().map(|e| e.id);
        loading.set(true);
        spawn(async move {
            let mut events = events;
            let mut has_more = has_more;
            let mut loading = loading;
            let url = match cursor {
                Some(id) => format!("/api/events?limit={PAGE}&before={id}"),
                None => format!("/api/events?limit={PAGE}"),
            };
            match get_json::<EventPage>(&url).await {
                Ok(page) => {
                    if page.events.is_empty() {
                        has_more.set(false);
                    } else {
                        let mut cur = events.write();
                        for e in page.events {
                            cur.push(e);
                        }
                        drop(cur);
                        has_more.set(page.has_more);
                    }
                }
                Err(_) => {}
            }
            loading.set(false);
        });
    });

    let list = events.read().clone();
    rsx! {
        div { class: "app",
            Header { status: status, events: events.len(), live_key: live_key() }
            div {
                id: "scroller",
                class: "scroller",
                onscroll: move |e| {
                    let d = e.data();
                    let near = (d.scroll_height() as f64) - (d.scroll_top() + d.client_height() as f64);
                    at_top.set(d.scroll_top() < 120.0);
                    if near < 600.0 {
                        load_more(());
                    }
                },
                if list.is_empty() {
                    div { class: "empty",
                        if *loading.read() {
                            "loading…"
                        } else {
                            "No recorded changes yet.\nEvents appear when something in the scene is added, removed or moved and then stays still."
                        }
                    }
                } else {
                    div { class: "grid",
                        for ev in list {
                            ThumbCard {
                                key: "{ev.id}",
                                meta: ev.clone(),
                                onclick: move |_| detail.set(Some(ev.clone())),
                            }
                        }
                    }
                }
                // No explicit "load more" control: the scroll handler above
                // appends the next page as the grid approaches the bottom, so
                // scrolling alone reveals more thumbnails.
                if *loading.read() {
                    div { class: "loadstatus", "loading more…" }
                }
            }
        }
        if let Some(ev) = detail() {
            DetailPopup {
                meta: ev.clone(),
                onclose: move |_| detail.set(None),
            }
        }
    }
}

#[component]
fn Header(status: Signal<Option<Status>>, events: usize, live_key: u64) -> Element {
    let line = match status() {
        Some(s) => {
            let mut l = format!(
                "frames {} · {} · scene resets {} · {events} events",
                s.frames,
                fmt_age(s.last_frame_ts),
                s.scene_resets
            );
            if !s.last_error.is_empty() {
                l.push_str(&format!(" · {}", s.last_error));
            }
            l
        }
        None => "connecting…".to_string(),
    };
    rsx! {
        header {
            h1 { "main_space" }
            span { class: "statusline", "{line}" }
            img { id: "live", class: "live", src: "/api/live?t={live_key}", alt: "live" }
        }
    }
}

#[component]
fn ThumbCard(meta: EventMeta, onclick: EventHandler<()>) -> Element {
    let n = meta.regions.len();
    let label = format!("{} · {} region{}", meta.ts, n, if n == 1 { "" } else { "s" });
    // Resting image is the small "after" thumb; hovering crossfades to the
    // "before" frame, so a mouseover over the thumbnail reveals what changed.
    rsx! {
        button { class: "card", onclick: move |_| onclick.call(()),
            HoverReveal {
                rest_src: file_url(meta.id, "thumb.jpg"),
                hover_src: file_url(meta.id, "before.jpg"),
                rest_alt: "event thumbnail",
            }
            div { class: "cardmeta", "{label}" }
        }
    }
}

#[component]
fn DetailPopup(meta: EventMeta, onclose: EventHandler<()>) -> Element {
    let id = meta.id;
    let region_line = if meta.regions.len() == 1 {
        let r = &meta.regions[0];
        format!("1 changed region · {}×{}px at ({},{})", r.w, r.h, r.x, r.y)
    } else {
        format!("{} changed regions", meta.regions.len())
    };
    rsx! {
        div {
            class: "overlay",
            onclick: move |_| onclose.call(()),
            div {
                class: "popup",
                // Clicks inside the panel must not close it.
                onclick: move |e| e.stop_propagation(),
                div { class: "popuphead",
                    div {
                        div { class: "popuptitle", "{meta.ts}  ·  {region_line}" }
                        div { class: "popupsub", "{meta.width}×{meta.height} px · rectangles mark the changed regions" }
                    }
                    button {
                        id: "fm-close",
                        class: "closebtn",
                        onclick: move |_| onclose.call(()),
                        "✕"
                    }
                }
                div { class: "subhead", "full frame" }
                div { class: "pairs",
                    Figure { caption: "before", id: id, name: "before.jpg", hover_src: None }
                    Figure { caption: "after", id: id, name: "after.jpg", hover_src: None }
                    // Hovering the diff crossfades to the "before" frame.
                    Figure { caption: "diff", id: id, name: "diff.jpg", hover_src: Some(file_url(id, "before.jpg")) }
                }
                div { class: "subhead", "zoom — largest changed region" }
                div { class: "pairs zoompairs",
                    Figure { caption: "before", id: id, name: "before_z.jpg", hover_src: None }
                    Figure { caption: "after", id: id, name: "after_z.jpg", hover_src: None }
                }
            }
        }
    }
}

#[component]
fn Figure(caption: &'static str, id: u64, name: &'static str, hover_src: Option<String>) -> Element {
    let src = file_url(id, name);
    rsx! {
        figure {
            figcaption { "{caption}" }
            a { href: "{src}", target: "_blank",
                if let Some(hover) = hover_src {
                    HoverReveal { rest_src: src.clone(), hover_src: hover, rest_alt: caption }
                } else {
                    img { src: "{src}", alt: "{caption}" }
                }
            }
        }
    }
}

// Stacked pair of images that crossfades from `rest_src` to `hover_src`
// whenever the pointer is over it — used to reveal the "before" frame over
// the "after" (or "diff") on hover.
#[component]
fn HoverReveal(rest_src: String, hover_src: String, rest_alt: &'static str) -> Element {
    rsx! {
        div { class: "hover-reveal",
            img { class: "hr-rest", src: "{rest_src}", alt: "{rest_alt}" }
            img { class: "hr-hover", src: "{hover_src}", alt: "{rest_alt}" }
        }
    }
}

/// Wasm entry point: runs when the module initialises (the JS glue's init()).
#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen(start))]
pub fn start() {
    dioxus::launch(App);
}

/// Host entry point (so `cargo run` still type-checks off-wasm).
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    dioxus::launch(App);
}
