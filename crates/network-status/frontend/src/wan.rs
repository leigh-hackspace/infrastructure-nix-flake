//! The Uplink tab: what the WAN supervisor discovered, ranked and chose.
//!
//! This is the daemon's own status file, not a re-computation. The page's job
//! is to make the *rejected* paths as legible as the selected one, because the
//! question an operator asks during an outage is "why did it not use the phone",
//! and the answer lives in each candidate's `reason`.

use dioxus::prelude::*;

use crate::format::fmt_rate;
use crate::state::Store;

fn ms(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.0} ms")).unwrap_or_else(|| "–".into())
}


#[component]
pub fn WanPanel(store: Store) -> Element {
    let wan_ref = store.wan.read();
    let wan = &*wan_ref;
    let err_ref = store.wan_err.read();
    let err = &*err_ref;

    if let Some(e) = err {
        return rsx! {
            div { class: "panel-error", "WAN supervisor status unavailable — {e}" }
            div { class: "empty", "The interface charts keep their last sample; this tab reads /run/wan-manager/status.json." }
        };
    }
    let Some(w) = wan.as_ref() else {
        return rsx! { div { class: "empty", "waiting for the WAN supervisor's first tick…" } };
    };

    let selected = w.selected.as_ref();
    let tunnel = w.tunnel.as_ref();
    let selected_text = selected
        .map(|s| format!("{} ({})", s.interface, s.kind))
        .unwrap_or_else(|| "none — no path is carrying packets".into());
    let tunnel_text = tunnel
        .map(|s| format!("{} ({})", s.interface, s.kind))
        .unwrap_or_else(|| "down".into());
    let repairs = if w.repairs.is_empty() {
        "nothing this tick".to_string()
    } else {
        w.repairs.join("; ")
    };
    let usb_class = if w.usb.ok { "issue" } else { "issue warn" };

    rsx! {
        div { class: "cards",
            div { class: "card", div { class: "k", "state" } div { class: "v", "{w.state}" } }
            div { class: "card", div { class: "k", "selected egress" } div { class: "v", "{selected_text}" } }
            div { class: "card", div { class: "k", "AAISP tunnel" } div { class: "v", "{tunnel_text}" } }
            div { class: "card", div { class: "k", "ticks" } div { class: "v", "{w.ticks}" } }
            div { class: "card", div { class: "k", "uptime" } div { class: "v", "{w.uptime_secs}s" } }
            div { class: "card", div { class: "k", "SSID attempted" } div { class: "v", "{w.ssid}" } }
        }

        if let Some(e) = &w.last_error {
            div { class: "panel-error", "last tick error: {e}" }
        }

        h2 { "Candidates, ranked" }
        table {
            thead {
                tr {
                    th { "interface" }
                    th { "kind" }
                    th { "link" }
                    th { "address / gateway" }
                    th { "gateway" }
                    th { "internet" }
                    th { "latency" }
                    th { "throughput" }
                    th { "score" }
                    th { "verdict" }
                }
            }
            tbody {
                for c in w.candidates.iter() {
                    tr {
                        td { class: "mono", "{c.interface}" }
                        td { span { class: "kind", "{c.kind}" } }
                        td { if c.link { span { class: "up", "up" } } else { span { class: "muted", "down" } } }
                        td { class: "mono",
                            {c.addresses.join(", ")}
                            if let Some(g) = &c.gateway { span { class: "muted", " via {g}" } }
                        }
                        td { if c.gateway_ok { span { class: "up", "ok" } } else { span { class: "muted", "silent" } } }
                        td { if c.internet_ok { span { class: "up", "ok" } } else { span { class: "muted", "no" } } }
                        td { class: "num", "{ms(c.latency_ms)}" }
                        td { class: "num", "{fmt_rate(c.throughput_bps)}" }
                        td { class: "num", {format!("{:.1}", c.score)} }
                        td {
                            if c.eligible {
                                span { class: "chip", "viable" }
                            } else {
                                span { class: "warn-text", "{c.reason}" }
                            }
                        }
                    }
                }
            }
        }

        h2 { "Wi-Fi associations (one supplicant per adapter)" }
        if w.wifi.is_empty() {
            div { class: "empty", "no wireless adapter discovered" }
        } else {
            table {
                thead {
                    tr {
                        th { "interface" }
                        th { "associated BSSID" }
                        th { "signal" }
                        th { "chosen BSSID" }
                        th { "error" }
                        th { "BSSes offering {w.ssid}" }
                    }
                }
                tbody {
                    for a in w.wifi.iter() {
                        tr {
                            td { class: "mono", "{a.interface}" }
                            td { class: "mono", {a.associated_bssid.clone().unwrap_or_else(|| "–".into())} }
                            td { class: "num", {a.signal_dbm.map(|s| format!("{s} dBm")).unwrap_or_else(|| "–".into())} }
                            td { class: "mono", {a.chosen_bssid.clone().unwrap_or_else(|| "–".into())} }
                            td { class: "muted", {a.error.clone().unwrap_or_default()} }
                            td { class: "mono", {a.aps.iter().map(|b| format!("{} {}dBm", b.bssid, b.signal_dbm.unwrap_or(0))).collect::<Vec<_>>().join("; ")} }
                        }
                    }
                }
            }
        }

        h2 { "Android / USB tether" }
        div { class: "{usb_class}", "{w.usb.diagnosis}" }
        if !w.usb.devices.is_empty() {
            table {
                thead {
                    tr {
                        th { "usb path" }
                        th { "vendor:product" }
                        th { "name" }
                        th { "driver" }
                        th { "netdev" }
                    }
                }
                tbody {
                    for d in w.usb.devices.iter() {
                        tr {
                            td { class: "mono", "{d.path}" }
                            td { class: "mono", "{d.vendor}:{d.product}" }
                            td { "{d.name}" }
                            td { class: "mono", {d.driver.clone().unwrap_or_else(|| "–".into())} }
                            td { class: "mono", {d.netdev.clone().unwrap_or_else(|| "–".into())} }
                        }
                    }
                }
            }
        }
        if !w.usb.kernel_failures.is_empty() {
            h2 { "Kernel USB failures (last 15 min)" }
            for line in w.usb.kernel_failures.iter() {
                div { class: "issue warn mono", "{line}" }
            }
        }

        h2 { "Repairs this tick" }
        div { class: "note", "{repairs}" }
        div { class: "note", "The supervisor re-measures every path every {w.interval_secs}s. Failover is immediate; return to a previous path needs a clear margin, because every switch rewrites the NAT source address of every open connection." }
    }
}
