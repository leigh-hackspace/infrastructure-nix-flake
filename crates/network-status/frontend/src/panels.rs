//! The panels: header, the outage banner, the interface cards, the issue list.
//!
//! Same shape as the old `panels.tsx`, in Dioxus rather than imperative DOM: the old
//! version hand-assigned `textContent` inside `createEffect` because there was no vnode
//! layer, so every panel had a ref and a manual write. Here the render *is* the reactive
//! half — a panel that reads a signal redraws when it changes, and nothing has to be told
//! twice.

use dioxus::prelude::*;

use crate::api::{Iface, Snapshot};
use crate::charts::Sparkline;
use crate::format::{fmt_dur, fmt_mem, fmt_rate, fmt_time};
use crate::state::Store;

/// Interfaces worth a card: active, or carrying traffic.
pub fn interesting_ifaces(snap: &Snapshot) -> Vec<Iface> {
    let mut picked: Vec<Iface> = snap
        .ifaces
        .iter()
        .filter(|i| i.active || i.down_total > 0 || i.up_total > 0)
        .cloned()
        .collect();
    picked.sort_by_key(|i| std::cmp::Reverse(i.down_total + i.up_total));
    picked
}

#[component]
pub fn Header(store: Store) -> Element {
    let snap_ref = store.last.read();
    let snap = &*snap_ref;
    let cfg_ref = store.config.read();
    let cfg = &*cfg_ref;

    // `match` arms have to agree on the type, so the pill text is built as a String in
    // every arm rather than a mix of literals and `format!`.
    let (pill_text, pill_class) = match snap {
        Some(s) if s.issues.iter().any(|i| i.level == "bad") => ("problem detected".to_string(), "pill bad"),
        Some(s) if !s.ok => ("router unreachable".to_string(), "pill bad"),
        Some(s) if !s.issues.is_empty() => {
            let n = s.issues.len();
            (
                format!("{n} warning{}", if n > 1 { "s" } else { "" }),
                "pill warn",
            )
        }
        Some(_) => ("all good".to_string(), "pill good"),
        None => ("waiting…".to_string(), "pill good"),
    };

    let meta = snap.as_ref().map(|s| {
        let r = &s.router;
        let mut out = format!("router up {}", fmt_dur(r.uptime_secs));
        if let Some(n) = r.ncpu {
            out.push_str(&format!(" · {n} CPUs"));
        }
        if r.cpu.user.is_some() {
            out.push_str(&format!(" · cpu {:.1}%", 100.0 - r.cpu.idle.unwrap_or(0.0)));
        }
        if r.mem.free.is_some() {
            out.push_str(&format!(" · {} mem free", fmt_mem(r.mem.free)));
        }
        out.push_str(&format!(" · {}", fmt_time(s.ts)));
        out
    });

    let meta_text = meta.unwrap_or_default();

    rsx! {
        header {
            h1 { "{cfg.title}" }
            span { class: "sub mono", "{meta_text}" }
            span { class: "{pill_class}", style: "margin-left:auto", "{pill_text}" }
        }
    }
}

/// The outage banner: shown when the router could not be read, and worded so that the
/// data still on screen is *named as* last-known, which is the whole point of keeping it.
#[component]
pub fn Banner(store: Store) -> Element {
    let snap_ref = store.last.read();
    let snap = &*snap_ref;
    let err_ref = store.snap_err.read();
    let err = &*err_ref;
    let failed = snap.as_ref().map(|s| !s.ok).unwrap_or(false) || err.is_some();
    if !failed {
        return rsx! {};
    }
    let shown = err.as_deref().unwrap_or("unknown error").to_string();
    rsx! {
        div {
            class: "banner",
            "Cannot reach the router — {shown}. Showing last known data; will retry automatically."
        }
    }
}

#[component]
pub fn IfaceCards(store: Store) -> Element {
    let snap_ref = store.last.read();
    let snap = &*snap_ref;
    let Some(s) = snap else {
        return rsx! {};
    };
    let cfg_ref = store.config.read();
    let cfg = &*cfg_ref;
    let cards = interesting_ifaces(s);

    rsx! {
        div {
            class: "iface-grid",
            for i in cards.iter() {
                {iface_card(store, i, &cfg.wan)}
            }
        }
    }
}

fn iface_card(store: Store, i: &Iface, wan: &str) -> Element {
    let name = i.name.clone();
    let dot = if i.active { "up" } else { "down" };
    let state = if i.active { "linked" } else { "no carrier" };
    let wan_tag = if name == wan { " (WAN)" } else { "" };

    rsx! {
        div {
            key: "{name}",
            class: "iface",
            div {
                class: "head",
                span { class: "dot {dot}" }
                span { class: "name", "{name}" }
                if !wan_tag.is_empty() { span { class: "muted", "{wan_tag}" } }
                span { class: "muted mono", style: "margin-left:auto", "{state}" }
            }
            Sparkline { store: store, name: "{name}", panel: "interfaces" }
            div {
                class: "rates mono",
                span { class: "upc", "▲ {fmt_rate(i.up_bps)}" }
                span { class: "downc", "▼ {fmt_rate(i.down_bps)}" }
            }
        }
    }
}

#[component]
pub fn Issues(store: Store) -> Element {
    let snap_ref = store.last.read();
    let snap = &*snap_ref;
    let Some(s) = snap else {
        return rsx! { div { span { class: "muted", "waiting for the first probe…" } } };
    };
    if s.issues.is_empty() {
        return rsx! { div { span { class: "muted", "✔ no issues detected" } } };
    }
    let rows: Vec<(String, String, String)> = s
        .issues
        .iter()
        .map(|i| {
            let mark = if i.level == "bad" { "●" } else { "⚠" };
            (i.level.clone(), mark.to_string(), i.message.clone())
        })
        .collect();

    rsx! {
        div {
            for (level, mark, message) in rows {
                div { class: "issue {level}", "{mark} {message}" }
            }
        }
    }
}
