//! The DHCP lease table: what the router has promised to whom.
//!
//! The countdown is the point of this table. A lease's expiry is the one number here that
//! says something *will change*: when it lapses the client renews, and a device that has
//! been moved can come back on a different VLAN. So the column shows how much is left and
//! when it runs out, against the router's own clock (`Store::router_now`), not the
//! browser's — which is why that method reads the `tick` signal and the poll keeps the skew.
//!
//! Like `connections.rs`, this file has no `esc()`: the hostname is a string the client
//! chose, the MAC and client-id are strings it sent, and dnsmasq copied all three into its
//! file verbatim. The old file escaped them because it built HTML; the vnode layer escapes
//! text nodes, so the data cannot be markup here.

use dioxus::prelude::*;

use std::collections::HashSet;

use crate::api::{LeaseResponse, LeaseRow};
use crate::format::{fmt_dur, fmt_time};
use crate::state::{LeaseControls, Store};
use common_ui::progress::{Progress, ProgressIndicator};
use common_ui::toggle_group::{ToggleGroup, ToggleItem};

const SORT_OPTIONS: [(&str, &str); 5] = [
    ("expires", "expiring first"),
    ("issued", "most recently issued"),
    ("name", "name"),
    ("vlan", "VLAN"),
    ("addr", "address"),
];

/// The "show" filter: `(value sent to the backend, caption on the segment, what it means)`.
///
/// Captions are short because these are segments of a segmented control; the sentence each
/// one stands for is the segment's accessible name and tooltip.
const LEFT_OPTIONS: [(i64, &str, &str); 4] = [
    (-1, "all", "every lease"),
    (3600, "≤ 1 h", "expiring within an hour"),
    (6 * 3600, "≤ 6 h", "expiring within 6 hours"),
    (24 * 3600, "≤ 24 h", "expiring today"),
];

/// How long a lease has left, as a word rather than a count of seconds.
fn fmt_left(secs: i64) -> String {
    if secs < 0 {
        return format!("lapsed {} ago", fmt_dur(Some(-secs)));
    }
    if secs < 60 {
        return format!("{} s", secs);
    }
    fmt_dur(Some(secs))
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Why the backend thinks this table cannot be the truth, in the page's words.
///
/// The verdict is the backend's (`crates/network-status/src/leases.rs::stale`), so the reason
/// and the counts beside it cannot disagree; this only translates the reason it chose. An
/// unknown reason still banners — a verdict this file does not know is still a verdict that
/// something is wrong with the channel.
const STALE_WHY: [(&str, &str); 2] = [
    (
        "all-expired",
        "every lease in it has already lapsed, and a DHCP server that is serving always has one that has not",
    ),
    (
        "unwritten",
        "it has not been written for longer than two of these leases last, and dnsmasq rewrites it on every lease event",
    ),
];

fn stale_why(code: &str) -> String {
    STALE_WHY
        .iter()
        .find(|(k, _)| *k == code)
        .map(|(_, v)| (*v).to_string())
        .unwrap_or_else(|| format!("the router reported the lease file as {code}"))
}

#[component]
pub fn LeasesPanel(store: Store) -> Element {
    let data_ref = store.leases.read();
    let data = &*data_ref;
    let err_ref = store.leases_err.read();
    let err = &*err_ref;
    let controls_ref = store.lease_controls.read();
    let controls = &*controls_ref;
    let now = store.router_now();

    let Some(d) = data else {
        return if err.is_some() {
            let e = err.clone().unwrap_or_default();
            rsx! {
                div {
                    class: "conn-summary mono",
                    span { class: "pill err", "no lease data" }
                    span { class: "muted", "{e}" }
                }
            }
        } else {
            rsx! {
                div { class: "conn-summary mono", span { class: "muted", "reading dnsmasq's lease file…" } }
            }
        };
    };

    let truncated = d.matched > d.returned;
    let mut per: Vec<(&String, usize)> = d.vlans.iter().map(|(v, n)| (v, *n)).collect();
    per.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let per = per
        .iter()
        .map(|(v, n)| format!("{v} {n}"))
        .collect::<Vec<_>>()
        .join(" · ");
    let vlans = d.vlan_labels.clone();
    let empty_line = empty_why(d, controls);

    let mut why: Vec<String> = vec![
        format!(
            "dnsmasq's lease file records an expiry and no length, so “left” is the block's own lease length ({}) counted back from that expiry: every lease here is that long because that is what this router hands out.",
            fmt_dur(Some(d.lease_ttl))
        ),
        "A client that renews keeps its address and the countdown restarts; a lease that has lapsed is still listed until the client asks again, which is why lapsed rows are shown rather than hidden.".to_string(),
    ];
    if d.unknown_ttl > 0 {
        why.push(format!(
            "{} lease{} inside no block the router was told about (a tailnet or hand-written address), so the bar under “left” is drawn as indeterminate: the length it actually ran at is not known here.",
            d.unknown_ttl,
            plural(d.unknown_ttl)
        ));
    }
    if d.generated > 0 {
        why.push(format!(
            "{} row{} marked ra — dnsmasq's own router-advertisement rows, not a client's lease: no MAC, no name, and no start time anywhere to count from.",
            d.generated,
            plural(d.generated)
        ));
    }
    if d.skipped > 0 {
        why.push(format!(
            "{} lease-file line{} were not leases (dnsmasq's own header, or a write caught mid-flight) and were ignored.",
            d.skipped,
            plural(d.skipped)
        ));
    }

    rsx! {
        div {
            class: "chart-head",
            h2 { "DHCP leases" }
            ShowFilter { store: store }
            label {
                class: "ctl",
                "sort "
                select {
                    value: "{controls.sort}",
                    onchange: move |e| {
                        let v = e.value();
                        store.edit_lease_controls(|c| c.sort = v);
                    },
                    for (v, l) in SORT_OPTIONS {
                        option { value: "{v}", selected: v == controls.sort, "{l}" }
                    }
                }
            }
            label {
                class: "ctl",
                "VLAN "
                select {
                    value: "{controls.vlan}",
                    onchange: move |e| {
                        let v = e.value();
                        store.edit_lease_controls(|c| c.vlan = v);
                    },
                    option { value: "", selected: controls.vlan.is_empty(), "every VLAN" }
                    // The router's whole VLAN list, not just the ones with leases: a VLAN
                    // whose leases have all lapsed must still be selectable, or the filter
                    // would silently answer "nobody" instead of "nobody right now".
                    for v in vlans {
                        option { value: "{v}", selected: v == controls.vlan, "{v}" }
                    }
                }
            }
            input {
                class: "ctl q",
                type: "search",
                placeholder: "name, address, MAC, VLAN…",
                value: "{controls.q}",
                oninput: move |e| {
                    let v = e.value();
                    store.edit_lease_controls(|c| c.q = v);
                },
            }
        }

        if let Some(code) = &d.stale {
            // The verdict is the backend's; this is the sentence that makes it readable and
            // says what it does *not* mean. A table of lapsed leases is not a network with no
            // clients — it is a file nobody is writing to.
            div {
                class: "panel-error",
                span { class: "pill err", "not live" }
                " this table is a snapshot, not the network: {stale_why(code)}"
                if let Some(age) = d.source_age {
                    ", and it was last written {fmt_dur(Some(age))} ago"
                }
                ". dnsmasq is not writing to {d.file}, so what is missing from here is not evidence that a device is absent — the router still holds the real leases. "
                code { "machines/gw/lease-reader.nix" }
                " is what owns that file."
            }
        }

        div {
            class: "conn-summary mono",
            span { class: "mono", "{d.total}" }
            " lease{plural(d.total)}"
            if d.expired > 0 {
                " "
                span { class: "warn-text", "({d.expired} lapsed)" }
            }
            if !per.is_empty() {
                " "
                span { class: "muted", "· {per}" }
            }
            if truncated {
                " "
                span { class: "muted", "· showing {d.returned} of {d.matched}" }
            }
            " "
            span { class: "muted", "· {d.file}" }
            // Freshness in the healthy case too: "is this live?" is the first question a lease
            // table has to answer, and the banner above is only the half where it is not.
            if let Some(age) = d.source_age {
                " "
                span { class: "muted", "· written {fmt_dur(Some(age))} ago" }
            }
            // Same rule as the connection panel: keeping the last data is only half of it. A
            // table that is still on screen while its endpoint is failing has to be named as
            // last-known, or "these are the current leases" is an assumption the page makes on
            // the operator's behalf.
            if err.is_some() {
                " "
                span { class: "pill err", "last attempt failed" }
                span { class: "muted", "{err.clone().unwrap_or_default()}" }
            }
        }

        div {
            class: "conn-scroll",
            table {
                class: "conn leases",
                thead {
                    tr {
                        th { "Device" }
                        th { "Address" }
                        th { "Where" }
                        th { class: "num", "Left" }
                        th { class: "num", "Expires" }
                    }
                }
                tbody {
                    if d.leases.is_empty() {
                        tr { td { colspan: "5", class: "muted empty", "{empty_line}" } }
                    } else {
                        for l in d.leases.iter() {
                            {lease_row(l, now)}
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

/// The "show" filter as the library's segmented control — the same reasoning as
/// `DirectionFilter` in `connections.rs`, and the reason the two panels' toolbars read the
/// same way.
#[component]
fn ShowFilter(store: Store) -> Element {
    let pressed: ReadSignal<Option<HashSet<usize>>> = ReadSignal::from(use_memo(move || {
        let want = store.lease_controls.read().max_left;
        LEFT_OPTIONS
            .iter()
            .position(|(v, _, _)| *v == want)
            .map(|i| [i].into_iter().collect::<HashSet<usize>>())
    }));

    let on_change = use_callback(move |set: HashSet<usize>| {
        let Some(&i) = set.iter().min() else { return };
        let max_left = LEFT_OPTIONS[i].0;
        store.edit_lease_controls(|c| c.max_left = max_left);
    });

    rsx! {
        ToggleGroup {
            class: "segs",
            pressed: pressed,
            horizontal: true,
            allow_multiple_pressed: false,
            on_pressed_change: on_change,
            for (i, (value, caption, meaning)) in LEFT_OPTIONS.iter().enumerate() {
                ToggleItem {
                    key: "{value}",
                    index: i,
                    title: "{meaning}",
                    aria_label: "{meaning}",
                    "{caption}"
                }
            }
        }
    }
}

/// The share of a lease that still has to run, as the library's `Progress`.
///
/// This cell used to be a `span` with an inline width. The cell states a proportion of a
/// known total, which is exactly what `role=progressbar` with `aria-valuenow` announces and
/// what a styled `span` does not. The connection table's rate bars **stay** plain spans on
/// purpose: one of those is a comparison against the busiest row, not a completion, and a
/// hundred progressbars in a table is noise rather than information.
///
/// Three states, and the widget has a name for each: a running lease is `pct` of its own
/// length, a lapsed one is zero (there is none left — this is a fact, not a missing value),
/// and a lease in a block the router was not told about has `value: None`, which the
/// primitive announces as indeterminate rather than inventing a bar for.
///
/// `pct` arrives as a prop and goes out through a memo because the primitive's prop is a
/// signal: a component's props are the reactive edge, which is also why this is a component
/// and `lease_row` below is still a plain function.
#[component]
fn LeaseProgress(pct: Option<f64>, lapsed: bool) -> Element {
    let value = ReadSignal::from(use_memo(move || match (lapsed, pct) {
        (true, _) => Some(0.0),
        (false, Some(p)) => Some(p),
        (false, None) => None,
    }));
    let tone = if lapsed {
        "lapsed"
    } else {
        match pct {
            None => "unknown",
            Some(p) if p < 15.0 => "low",
            // Healthy leases get no extra class: a class nothing styles is how this page
            // lost its stylesheet in the first place, and check 12 of the suite now refuses
            // one. `ok` is the absence of a tone, so it is spelled as one.
            Some(_) => "",
        }
    };

    rsx! {
        Progress {
            class: "lease {tone}",
            value: value,
            max: 100.0,
            aria_label: "share of this lease still to run",
            ProgressIndicator { class: "fill" }
        }
    }
}

fn lease_row(l: &LeaseRow, now: i64) -> Element {
    let left = l.expires - now;
    let ident = if !l.mac.is_empty() && l.mac != "*" {
        Some(l.mac.clone())
    } else if !l.cid.is_empty() {
        Some(format!("id {}", l.cid))
    } else {
        None
    };

    // The bar is the share of the lease still to run; `None` means the length it ran at is
    // not known here, which the widget renders as indeterminate rather than as empty.
    let pct = if l.known && l.ttl > 0 {
        Some(((left as f64 / l.ttl as f64) * 100.0).clamp(0.0, 100.0))
    } else {
        None
    };
    let row_class = if left < 0 {
        "lapsed".to_string()
    } else if pct.map(|p| p < 15.0).unwrap_or(false) {
        "low".to_string()
    } else {
        String::new()
    };
    let where_ = if l.vlan.as_ref().map(|v| !v.is_empty()).unwrap_or(false) {
        format!("{} · {}", l.vlan.clone().unwrap_or_default(), l.net)
    } else if !l.net.is_empty() {
        l.net.clone()
    } else {
        "not on a VLAN".to_string()
    };
    let name = if !l.name.is_empty() {
        l.name.clone()
    } else if l.generated {
        "ra".to_string()
    } else {
        "(no name)".to_string()
    };

    rsx! {
        tr {
            class: "{row_class}",
            td {
                span { title: if l.generated { "dnsmasq's own router-advertisement row, not a client's lease" } else { "" }, "{name}" }
                if let Some(ident) = ident {
                    " "
                    span { class: "muted mono addr", "{ident}" }
                }
            }
            td { class: "mono", "{l.ip}" }
            td { span { class: "muted mono", "{where_}" } }
            td {
                class: "num left-cell",
                "{fmt_left(left)}"
                LeaseProgress { pct, lapsed: left < 0 }
            }
            td { class: "num muted", "{fmt_time(l.expires)}" }
        }
    }
}

fn empty_why(d: &LeaseResponse, c: &LeaseControls) -> String {
    if d.total == 0 {
        return "the lease file is readable but empty: no client has been given an address since dnsmasq last started".to_string();
    }
    let mut filters: Vec<String> = Vec::new();
    if c.max_left >= 0 {
        filters.push(format!("expiring within {}", fmt_dur(Some(c.max_left))));
    }
    if !c.vlan.is_empty() {
        filters.push(format!("on {}", c.vlan));
    }
    if !c.q.is_empty() {
        filters.push(format!("matching “{}”", c.q));
    }
    format!(
        "{} lease{} in the file, none {}",
        d.total,
        plural(d.total),
        if filters.is_empty() {
            "matching".to_string()
        } else {
            filters.join(" and ")
        }
    )
}
