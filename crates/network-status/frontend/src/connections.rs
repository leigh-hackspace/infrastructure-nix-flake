//! The connections panel — the `--conn-min-age` question, in one line, next to the table
//! it filters.
//!
//! Two things the port must not lose:
//!
//! - **The "moving now" filter is applied by the backend, before the row limit.** If it
//!   were applied here, the first hundred idle rows would hide the traffic that is actually
//!   moving, so the checkbox sets `min_bps` on the request rather than filtering the
//!   response.
//! - **Top talkers are not filtered by the age slider.** "Who is the biggest talker" cannot
//!   depend on the filter you set to answer it, so that row comes from the backend's
//!   unfiltered roll-up.
//!
//! The old file carried an `esc()` because it built HTML strings. It is gone: Dioxus renders
//! text nodes, and a hostname here is a string a device chose for itself — the vnode layer
//! escapes it, so there is no string to escape.

use dioxus::prelude::*;

use crate::api::{ConnResponse, ConnRow, HostAgg};
use crate::format::{fmt_dur, fmt_rate, fmt_total};
use crate::state::{ConnControls, Store};
use common_ui::checkbox::{Checkbox, CheckboxIndicator, CheckboxState};

const AGE_OPTIONS: [(i64, &str); 7] = [
    (-1, "server default"),
    (0, "no minimum"),
    (5, "5 s"),
    (15, "15 s"),
    (30, "30 s"),
    (60, "1 min"),
    (300, "5 min"),
];

const SORT_OPTIONS: [(&str, &str); 6] = [
    ("rate", "busiest now"),
    ("down", "downloading"),
    ("up", "uploading"),
    ("bytes", "most transferred"),
    ("age", "oldest"),
    ("host", "device"),
];

const KIND_OPTIONS: [(&str, &str); 5] = [
    ("", "every direction"),
    ("out", "device → internet"),
    ("in", "internet → device"),
    ("lan", "device ↔ device"),
    ("wan", "neither end is ours"),
];

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn kind_label(kind: &str) -> &str {
    match kind {
        "out" => "out",
        "in" => "in",
        "lan" => "LAN",
        "wan" => "other",
        other => other,
    }
}

#[component]
pub fn ConnectionsPanel(store: Store) -> Element {
    let data_ref = store.conns.read();
    let data = &*data_ref;
    let err_ref = store.conns_err.read();
    let err = &*err_ref;
    let controls_ref = store.controls.read();
    let controls = &*controls_ref;
    // `effective_min_age` subscribes to config as well as to the control, so the line
    // prints the number actually used rather than the word "default".
    let effective_age = store.effective_min_age();

    let Some(d) = data else {
        return if err.is_some() {
            let e = err.clone().unwrap_or_default();
            rsx! {
                div {
                    class: "conn-summary mono",
                    span { class: "pill bad", "no connection data" }
                    span { class: "muted", "{e}" " — the table will appear as soon as the router answers." }
                }
            }
        } else {
            rsx! { div { class: "conn-summary mono", span { class: "muted", "reading the connection table…" } } }
        };
    };

    let truncated = d.matched > d.returned;
    let hosts: Vec<HostAgg> = d.hosts.iter().take(6).cloned().collect();
    let max: f64 = d
        .connections
        .iter()
        .map(|c| c.down_bps.unwrap_or(0.0).max(c.up_bps.unwrap_or(0.0)))
        .fold(1.0_f64, f64::max);
    let empty_line = empty_why(d, controls);

    let mut why: Vec<String> = vec![
        "Age is how long this page has seen the connection — conntrack records no start time — and rates are differenced between samples, so a connection needs two samples to have one.".to_string(),
        "“down” is always peer → device, whichever way the connection started. Addresses come from the router's DNS records and dnsmasq's lease file (the DHCP leases tab has the same rows, with what each client calls itself); anything in neither stays a number.".to_string(),
    ];
    if d.unaccounted > 0 {
        why.push(format!(
            "{} connection{} the kernel is not counting bytes for (they began before net.netfilter.nf_conntrack_acct was on) — shown as –, not as zero.",
            d.unaccounted,
            plural(d.unaccounted)
        ));
    }
    if d.unparsed > 0 {
        why.push(format!(
            "{} table rows would not parse and were dropped.",
            d.unparsed
        ));
    }

    rsx! {
        div {
            class: "chart-head",
            h2 { "Active connections" }
            label {
                class: "ctl",
                "older than "
                select {
                    value: "{controls.min_age}",
                    onchange: move |e| {
                        let v = e.value().parse::<i64>().unwrap_or(-1);
                        store.edit_controls(|c| c.min_age = v);
                    },
                    for (v, l) in AGE_OPTIONS {
                        option { value: "{v}", selected: v == controls.min_age, "{l}" }
                    }
                }
            }
            label {
                class: "ctl",
                "sort "
                select {
                    value: "{controls.sort}",
                    onchange: move |e| {
                        let v = e.value();
                        store.edit_controls(|c| c.sort = v);
                    },
                    for (v, l) in SORT_OPTIONS {
                        option { value: "{v}", selected: v == controls.sort, "{l}" }
                    }
                }
            }
            label {
                class: "ctl",
                select {
                    value: "{controls.kind}",
                    onchange: move |e| {
                        let v = e.value();
                        store.edit_controls(|c| c.kind = v);
                    },
                    for (v, l) in KIND_OPTIONS {
                        option { value: "{v}", selected: v == controls.kind, "{l}" }
                    }
                }
            }
            label {
                class: "ctl",
                Checkbox {
                    checked: Some(if controls.busy_only {
                        CheckboxState::Checked
                    } else {
                        CheckboxState::Unchecked
                    }),
                    name: "busy-only",
                    aria_label: "moving now",
                    on_checked_change: move |s| {
                        store.edit_controls(|c| c.busy_only = s == CheckboxState::Checked);
                    },
                    CheckboxIndicator { "✓" }
                }
                " moving now"
            }
            input {
                class: "ctl q",
                type: "search",
                placeholder: "name, address, port…",
                value: "{controls.q}",
                oninput: move |e| {
                    let v = e.value();
                    store.edit_controls(|c| c.q = v);
                },
            }
        }

        div {
            class: "conn-summary mono",
            span { class: "downc", "▼ {fmt_rate(Some(d.totals.down_bps))}" }
            " "
            span { class: "upc", "▲ {fmt_rate(Some(d.totals.up_bps))}" }
            " "
            span {
                class: "muted",
                "across {d.active} live connection{plural(d.active)}, "
                "{d.matched} over {fmt_dur(Some(effective_age))}"
                if truncated { ", showing the top {d.returned} of {d.matched}" }
                " · sampled every {d.interval}s"
            }
            // The rule in full: a failed fetch keeps the table it had, *and* the panel has to
            // say that what is on screen is not current. Showing the reason only when there is
            // no data — which is what the SolidJS page did — leaves a stale table reading as a
            // fresh one, and a table this page last heard about three minutes ago is exactly
            // the thing an operator would act on.
            if err.is_some() {
                span { class: "pill bad", "last attempt failed" }
                span { class: "muted", "{err.clone().unwrap_or_default()}" }
            }
        }

        div {
            class: "conn-hosts",
            if hosts.is_empty() {
                span { class: "muted", "no device is moving anything right now" }
            } else {
                for h in hosts {
                    {host_chip(&h)}
                }
            }
        }

        div {
            class: "conn-scroll",
            table {
                class: "conn",
                thead {
                    tr {
                        th { "Device" }
                        th { "Peer" }
                        th { "Service" }
                        th { class: "num", "Age" }
                        th { class: "num", "Down" }
                        th { class: "num", "Up" }
                    }
                }
                tbody {
                    if d.connections.is_empty() {
                        tr {
                            td { colspan: "6", class: "muted empty", "{empty_line}" }
                        }
                    } else {
                        for c in d.connections.iter() {
                            {conn_row(c, max)}
                        }
                    }
                }
            }
        }

        div {
            class: "conn-note muted",
            for w in why {
                div { "{w}" }
            }
        }
    }
}

fn host_chip(h: &HostAgg) -> Element {
    let addr = h.addr.clone().unwrap_or_default();
    let who = h.name.clone().unwrap_or(addr.clone());
    let named = h.name.is_some();
    let many = h.conns > 1;

    rsx! {
        span {
            class: "chip",
            span { class: "who", "{who}" }
            if named { " " span { class: "muted mono", "{addr}" } }
            if many { " " span { class: "muted", "{h.conns}c" } }
            " "
            span { class: "downc", "▼ {fmt_rate(Some(h.down_bps))}" }
            " "
            span { class: "upc", "▲ {fmt_rate(Some(h.up_bps))}" }
        }
    }
}

fn conn_row(c: &ConnRow, max: f64) -> Element {
    let svc: Vec<String> = [
        c.service.clone().unwrap_or(c.proto.clone()),
        c.state.clone().filter(|s| s != "ESTABLISHED").unwrap_or_default(),
    ]
    .into_iter()
    .filter(|x| !x.is_empty())
    .collect();

    let dev_name = c.host_name.clone().unwrap_or_default();
    let dev_addr = format!(
        "{}{}",
        c.host,
        c.host_port.map(|p| format!(":{p}")).unwrap_or_default()
    );
    let peer_name = c.peer_name.clone().unwrap_or_default();
    let peer_addr = format!(
        "{}{}",
        c.peer,
        c.peer_port.map(|p| format!(":{p}")).unwrap_or_default()
    );
    let row_class = format!(
        "kind-{}{}",
        c.kind,
        if c.accounted { "" } else { " uncounted" }
    );
    let uncounted = if c.accounted {
        fmt_total(c.bytes_down)
    } else {
        "–".to_string()
    };
    let uncounted_up = if c.accounted {
        fmt_total(c.bytes_up)
    } else {
        "–".to_string()
    };

    rsx! {
        tr {
            class: "{row_class}",
            td {
                span { class: "kind k-{c.kind}", title: "{c.kind} connection", "{kind_label(&c.kind)}" }
                " "
                if dev_name.is_empty() {
                    span { class: "mono", "{dev_addr}" }
                } else {
                    "{dev_name}"
                    " "
                    span { class: "muted mono addr", "{dev_addr}" }
                }
            }
            td {
                if peer_name.is_empty() {
                    span { class: "mono", "{peer_addr}" }
                } else {
                    "{peer_name}"
                    " "
                    span { class: "muted mono addr", "{peer_addr}" }
                }
            }
            td {
                for (i, s) in svc.iter().enumerate() {
                    if i > 0 {
                        span { class: "muted", "·" }
                        " "
                    }
                    "{s}"
                }
            }
            td { class: "num muted", "{fmt_dur(Some(c.age))}" }
            td {
                class: "num",
                {rate_bar(c.down_bps, max, "down")}
                "{fmt_rate(c.down_bps)}"
                br {}
                span { class: "muted", "{uncounted}" }
            }
            td {
                class: "num",
                {rate_bar(c.up_bps, max, "up")}
                "{fmt_rate(c.up_bps)}"
                br {}
                span { class: "muted", "{uncounted_up}" }
            }
        }
    }
}

/// A rate cell's background is its share of the busiest row, so the table can be scanned
/// rather than read.
fn rate_bar(bps: Option<f64>, max: f64, cls: &str) -> Element {
    let Some(v) = bps else { return rsx! {} };
    if v <= 0.0 {
        return rsx! {};
    }
    let pct = ((v / max) * 100.0).min(100.0);
    rsx! { span { class: "bar {cls}", style: format!("width:{pct:.1}%") } }
}

fn empty_why(d: &ConnResponse, c: &ConnControls) -> String {
    if d.active == 0 {
        return "no connections in the router's table right now".to_string();
    }
    let _ = c;
    format!(
        "{} live connection{} on the live filter, none matching the search box or direction ({} tracked). Short connections come and go: try a lower minimum age.",
        d.active,
        plural(d.active),
        d.tracked
    )
}
