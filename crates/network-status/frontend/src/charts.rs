//! The chart components: bandwidth card, connection-count chart, load chart, sparkline.
//!
//! Why the draw waits for `onmounted`: a `<canvas>` has to exist in the document before
//! anything can draw on it, and a component's effect runs before the node it just produced
//! is attached on the first render. So each chart sets a `mounted` flag from `onmounted`
//! and then draws — and the draw is an effect, so it re-runs on every data change, which is
//! what the old `createEffect` gave.
//!
//! Each draw effect also reads `store.tab`, because a canvas in a hidden tabpanel has no
//! width: without that, a chart drawn while its tab was hidden keeps a 300 px picture until
//! the next poll. Reading the tab means the picture is redrawn as soon as the tab is shown.

use dioxus::prelude::*;

use crate::api::{History, Snapshot};
use crate::chart::{Plot, canvas, draw, sparkline, COLOR_DOWN, COLOR_UP};
use crate::format::{fmt_list_load, fmt_rate, fmt_total};
use crate::panels::interesting_ifaces;
use crate::state::Store;

/// The select's options: `total`, plus the interfaces worth looking at.
fn options(snap: Option<&Snapshot>, wan: &str) -> Vec<(String, String)> {
    let mut opts = vec![("total".to_string(), "total (all interfaces)".to_string())];
    if let Some(s) = snap {
        for i in interesting_ifaces(s) {
            let label = if i.name == wan {
                format!("{} (WAN)", i.name)
            } else {
                i.name.clone()
            };
            opts.push((i.name, label));
        }
    }
    opts
}

/// The last `window` points for one interface, or the summed series for `total`.
/// Right-aligned, as the old `totalSeries` was: an interface that appears later in the
/// history must not shift the totals.
fn window_series(hist: &History, name: &str, window: usize) -> (Vec<Option<f64>>, Vec<Option<f64>>) {
    let n = hist.ts.len();
    if n == 0 {
        return (Vec::new(), Vec::new());
    }
    if name == "total" {
        let mut up = vec![None; n];
        let mut down = vec![None; n];
        for s in hist.ifaces.values() {
            let off = n - s.up.len();
            for i in 0..s.up.len() {
                if let Some(v) = s.up[i] {
                    up[off + i] = up[off + i].map(|a| a + v).or(Some(v));
                }
                if let Some(v) = s.down[i] {
                    down[off + i] = down[off + i].map(|a| a + v).or(Some(v));
                }
            }
        }
        (tail(up, window), tail(down, window))
    } else {
        match hist.ifaces.get(name) {
            Some(s) => (tail(s.up.clone(), window), tail(s.down.clone(), window)),
            None => (Vec::new(), Vec::new()),
        }
    }
}

fn tail(v: Vec<Option<f64>>, window: usize) -> Vec<Option<f64>> {
    if v.len() <= window {
        return v;
    }
    v[v.len() - window..].to_vec()
}

#[component]
pub fn BwCard(store: Store) -> Element {
    let mut mounted = use_signal(|| false);

    use_effect(move || {
        if !*mounted.read() {
            return;
        }
        let hist_ref = store.hist.read();
        let hist = &*hist_ref;
        store.redraw.read();
        store.tab.read();
        let selected_ref = store.selected.read();
        let selected = &*selected_ref;
        let Some(el) = canvas("chart-bw") else { return };
        let (up, down) = window_series(hist, selected, 120);
        // `draw` formats the gridline labels, which are plain numbers, so the formatter
        // takes an f64 rather than the Option the rest of this crate passes around.
        draw(
            &el,
            &[
                Plot::new(&up, COLOR_UP).filled(true),
                Plot::new(&down, COLOR_DOWN).filled(true),
            ],
            |v| fmt_rate(Some(v)),
            220.0,
            &hist.ts,
        );
    });

    let selected_ref = store.selected.read();
    let selected = &*selected_ref;
    let snap_ref = store.last.read();
    let snap = &*snap_ref;
    let cfg_ref = store.config.read();
    let cfg = &*cfg_ref;

    // The legend lines are the same values the chart drew, from the same window.
    let (cur_up, cur_down, tot_up, tot_down) = match snap {
        Some(s) if selected == "total" => {
            let mut cu = None;
            let mut cd = None;
            let mut tu = 0u64;
            let mut td = 0u64;
            for i in &s.ifaces {
                if let Some(v) = i.up_bps {
                    cu = cu.map(|a| a + v).or(Some(v));
                }
                if let Some(v) = i.down_bps {
                    cd = cd.map(|a| a + v).or(Some(v));
                }
                tu += i.up_total;
                td += i.down_total;
            }
            (cu, cd, tu, td)
        }
        Some(s) => {
            let cur = s.ifaces.iter().find(|i| &i.name == selected);
            (
                cur.and_then(|i| i.up_bps),
                cur.and_then(|i| i.down_bps),
                cur.map(|i| i.up_total).unwrap_or(0),
                cur.map(|i| i.down_total).unwrap_or(0),
            )
        }
        None => (None, None, 0, 0),
    };

    let opts = options(snap.as_ref(), &cfg.wan);
    let all_tag = if selected == "total" { "all: " } else { "" };

    rsx! {
        div {
            div {
                class: "chart-head",
                h2 { "Bandwidth" }
                select {
                    value: "{selected}",
                    onchange: move |e| store.selected.set(e.value()),
                    for (name, label) in opts {
                        option { value: "{name}", selected: name == *selected, "{label}" }
                    }
                }
                span {
                    class: "now mono",
                    span { class: "upc", "▲ {fmt_rate(cur_up)}" }
                    " "
                    span { class: "downc", "▼ {fmt_rate(cur_down)}" }
                }
            }
            canvas { id: "chart-bw", style: "height:220px", onmounted: move |_| mounted.set(true) }
            div {
                class: "legend",
                span {
                    span { class: "sw", style: "background:var(--up)" }
                    "up (to internet)"
                }
                span {
                    span { class: "sw", style: "background:var(--down)" }
                    "down (from internet)"
                }
                span {
                    class: "muted",
                    "{all_tag}▼ {fmt_total(Some(tot_down))} · ▲ {fmt_total(Some(tot_up))}"
                }
            }
        }
    }
}

#[component]
pub fn ConnChart(store: Store) -> Element {
    let mut mounted = use_signal(|| false);

    use_effect(move || {
        if !*mounted.read() {
            return;
        }
        let hist_ref = store.hist.read();
        let hist = &*hist_ref;
        store.redraw.read();
        store.tab.read();
        let Some(el) = canvas("chart-conntrack") else { return };
        // conntrack counts are integers on the wire; the plot layer takes one series type.
        let series: Vec<Option<f64>> = hist.conntrack.iter().map(|v| v.map(|n| n as f64)).collect();
        draw(
            &el,
            &[Plot::new(&series, "#a78bfa").filled(true).window(240)],
            |v| (v.round() as i64).to_string(),
            150.0,
            &hist.ts,
        );
    });

    let snap_ref = store.last.read();
    let snap = &*snap_ref;
    let now_text = snap.as_ref().map(|s| {
        let mut out = match s.router.conntrack {
            Some(n) => format!("{n} conntrack entries"),
            None => "–".to_string(),
        };
        if let Some(n) = s.router.own_tcp {
            out.push_str(&format!("  ·  {n} router sockets"));
        }
        if let Some(r) = s.router.retrans_rate {
            out.push_str(&format!("  ·  retrans {r:.2}/s"));
        }
        out
    }).unwrap_or_else(|| "–".to_string());

    rsx! {
        div {
            canvas { id: "chart-conntrack", style: "height:150px", onmounted: move |_| mounted.set(true) }
            div {
                class: "legend",
                span { class: "mono", "{now_text}" }
            }
        }
    }
}

#[component]
pub fn LoadChart(store: Store) -> Element {
    let mut mounted = use_signal(|| false);

    use_effect(move || {
        if !*mounted.read() {
            return;
        }
        let hist_ref = store.hist.read();
        let hist = &*hist_ref;
        store.redraw.read();
        store.tab.read();
        let Some(el) = canvas("chart-load") else { return };
        draw(
            &el,
            &[Plot::new(&hist.load, "#f1c40f").filled(true).window(240)],
            |v| format!("{v:.1}"),
            150.0,
            &hist.ts,
        );
    });

    let snap_ref = store.last.read();
    let snap = &*snap_ref;
    let now_text = snap
        .as_ref()
        .map(|s| fmt_list_load(&s.router.load.unwrap_or([0.0, 0.0, 0.0]), s.router.ncpu))
        .unwrap_or_else(|| "–".to_string());

    rsx! {
        div {
            canvas { id: "chart-load", style: "height:150px", onmounted: move |_| mounted.set(true) }
            div {
                class: "legend",
                span { class: "mono", "{now_text}" }
            }
        }
    }
}

#[component]
pub fn Sparkline(store: Store, name: String, panel: String) -> Element {
    let mut mounted = use_signal(|| false);
    let id = format!("chart-{panel}-{name}");
    // Cloned before the effect because the effect and the markup both need it.
    let id_for_effect = id.clone();

    use_effect(move || {
        if !*mounted.read() {
            return;
        }
        let hist_ref = store.hist.read();
        let hist = &*hist_ref;
        store.redraw.read();
        store.tab.read();
        let Some(el) = canvas(&id_for_effect) else { return };
        let Some(s) = hist.ifaces.get(&name) else { return };
        sparkline(
            &el,
            &[Plot::new(&s.down, COLOR_DOWN), Plot::new(&s.up, COLOR_UP)],
            60,
        );
    });

    rsx! {
        canvas { id: "{id}", style: "height:46px", onmounted: move |_| mounted.set(true) }
    }
}
