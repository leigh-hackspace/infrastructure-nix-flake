//! The status files the dashboard and Prometheus read.
//!
//! Two shapes, one source: `status.json` for the page (`/api/wan`) and
//! `metrics.prom` for the scrape that already exists. Both are written every
//! tick, world-readable (they contain interface names and probe results, never
//! the passphrase), and they are the only way an operator can see *why* the
//! daemon chose what it chose without reading the journal.

use crate::config::Config;
use crate::net::Kind;
use crate::probe::{Bss, UsbDevice};
use crate::rank::{Candidate, Selection};
use crate::util::{jopt, jopt_f, quoted};
use crate::wifi::WifiReport;
use std::time::Instant;

pub struct Report<'a> {
    pub cfg: &'a Config,
    pub candidates: &'a [Candidate],
    pub sel: &'a Selection,
    pub wifi: &'a [WifiReport],
    pub aps: &'a [Bss],
    pub usb: &'a [UsbDevice],
    pub usb_failures: &'a [String],
    pub actions: &'a [String],
    pub started: Instant,
    pub ticks: u64,
    pub last_error: Option<&'a str>,
}

fn usb_diagnosis(usb: &[UsbDevice], failures: &[String]) -> (String, bool) {
    let phones: Vec<&UsbDevice> = usb
        .iter()
        .filter(|d| crate::probe::looks_like_phone(&d.vendor, &d.product, &d.product_name))
        .collect();
    let phone_netdev = phones.iter().find_map(|d| d.netdev.clone());
    if let Some(n) = phone_netdev {
        return (
            format!("phone-class USB device is enumerated and present as netdev {n}; DHCP is the next step"),
            true,
        );
    }
    if !failures.is_empty() {
        let last = failures.last().cloned().unwrap_or_default();
        return (
            format!(
                "a USB device is electrically visible but never enumerates (last kernel line: {last}). \
                 This is the `-71`/EPROTO case: the phone sees power but no data link, so Android greys out \
                 USB tethering. Use a known data-capable cable, a different port (the internal USB header \
                 is EHCI and may not source enough current), or a powered hub."
            ),
            false,
        );
    }
    if phones.is_empty() {
        (
            "no phone-class USB device is enumerated. Check the cable, the port and the phone's \
             USB mode (File transfer / USB tethering, not 'charging only')."
                .into(),
            false,
        )
    } else {
        (
            "the phone is enumerated but no Ethernet netdev has appeared: the RNDIS/CDC driver may not \
             have bound, or USB tethering is still off on the phone."
                .into(),
            false,
        )
    }
}

pub fn json(r: &Report) -> String {
    let (usb_text, usb_ok) = usb_diagnosis(r.usb, r.usb_failures);
    let candidates: Vec<String> = r
        .candidates
        .iter()
        .map(|c| {
            let addrs: Vec<String> = c
                .info
                .addresses
                .iter()
                .map(|(a, m)| quoted(&format!("{a}/{m}")))
                .collect();
            format!(
                r#"{{"interface":{},"kind":{},"link":{},"carrier":{},"addresses":[{}],"gateway":{},"dhcp_metric":{},"driver":{},"usb_path":{},"eligible":{},"reason":{},"score":{},"gateway_ok":{},"internet_ok":{},"latency_ms":{},"throughput_bps":{},"failures":{},"checked_ts":{}}}"#,
                quoted(&c.info.name),
                quoted(c.info.kind.as_str()),
                c.info.link_up,
                c.info.carrier,
                addrs.join(","),
                c.info.gateway.as_deref().map(quoted).unwrap_or_else(|| "null".into()),
                jopt(c.info.dhcp_metric),
                c.info.driver.as_deref().map(quoted).unwrap_or_else(|| "null".into()),
                c.info.usb_path.as_deref().map(quoted).unwrap_or_else(|| "null".into()),
                c.eligible(),
                quoted(&c.reason()),
                format!("{:.2}", c.score),
                c.probe.gateway_ok,
                c.probe.internet_ok,
                jopt_f(c.probe.latency_ms),
                jopt_f(c.throughput_bps),
                c.failures,
                c.probe.checked_ts,
            )
        })
        .collect();
    let selected = match r.sel.best {
        Some(i) => {
            let c = &r.candidates[i];
            format!(
                r#"{{"role":"egress","interface":{},"kind":{},"gateway":{},"score":{},"latency_ms":{},"throughput_bps":{}}}"#,
                quoted(&c.info.name),
                quoted(c.info.kind.as_str()),
                c.info.gateway.as_deref().map(quoted).unwrap_or_else(|| "null".into()),
                format!("{:.2}", c.score),
                jopt_f(c.probe.latency_ms),
                jopt_f(c.throughput_bps),
            )
        }
        None => "null".into(),
    };
    let tunnel = match r.sel.tunnel {
        Some(i) => {
            let c = &r.candidates[i];
            format!(
                r#"{{"role":"tunnel","interface":{},"kind":{},"score":{},"latency_ms":{},"throughput_bps":{}}}"#,
                quoted(&c.info.name),
                quoted(c.info.kind.as_str()),
                format!("{:.2}", c.score),
                jopt_f(c.probe.latency_ms),
                jopt_f(c.throughput_bps),
            )
        }
        None => "null".into(),
    };
    let wifi: Vec<String> = r
        .wifi
        .iter()
        .map(|w| {
            let aps: Vec<String> = w
                .aps
                .iter()
                .map(|b| {
                    format!(
                        r#"{{"bssid":{},"ssid":{},"signal_dbm":{},"freq_mhz":{}}}"#,
                        quoted(&b.bssid),
                        quoted(&b.ssid),
                        jopt(b.signal_dbm),
                        jopt(b.freq_mhz),
                    )
                })
                .collect();
            format!(
                r#"{{"interface":{},"associated_bssid":{},"signal_dbm":{},"chosen_bssid":{},"error":{},"aps":[{}]}}"#,
                quoted(&w.iface),
                w.associated_bssid.as_deref().map(quoted).unwrap_or_else(|| "null".into()),
                jopt(w.signal_dbm),
                w.chosen_bssid.as_deref().map(quoted).unwrap_or_else(|| "null".into()),
                w.error.as_deref().map(quoted).unwrap_or_else(|| "null".into()),
                aps.join(","),
            )
        })
        .collect();
    let all_aps: Vec<String> = r
        .aps
        .iter()
        .map(|b| {
            format!(
                r#"{{"bssid":{},"ssid":{},"signal_dbm":{},"freq_mhz":{}}}"#,
                quoted(&b.bssid),
                quoted(&b.ssid),
                jopt(b.signal_dbm),
                jopt(b.freq_mhz),
            )
        })
        .collect();
    let usb: Vec<String> = r
        .usb
        .iter()
        .map(|d| {
            format!(
                r#"{{"path":{},"vendor":{},"product":{},"name":{},"driver":{},"netdev":{}}}"#,
                quoted(&d.path),
                quoted(&d.vendor),
                quoted(&d.product),
                quoted(&d.product_name),
                d.driver.as_deref().map(quoted).unwrap_or_else(|| "null".into()),
                d.netdev.as_deref().map(quoted).unwrap_or_else(|| "null".into()),
            )
        })
        .collect();
    let repairs: Vec<String> = r.actions.iter().map(|s| quoted(s)).collect();
    let kernel: Vec<String> = r.usb_failures.iter().map(|s| quoted(s)).collect();
    format!(
        r#"{{"ts":{},"uptime_secs":{},"ticks":{},"state":{},"selected":{},"tunnel":{},"candidates":[{}],"wifi":[{}],"all_aps":[{}],"usb":{{"ok":{},"diagnosis":{},"devices":[{}],"kernel_failures":[{}]}},"repairs":[{}],"last_error":{},"ssid":{},"interval_secs":{},"fallback_metric":{},"hub_table":{},"tunnel_table":{}}}"#,
        crate::util::now_ts(),
        r.started.elapsed().as_secs(),
        r.ticks,
        quoted(&r.sel.state),
        selected,
        tunnel,
        candidates.join(","),
        wifi.join(","),
        all_aps.join(","),
        usb_ok,
        quoted(&usb_text),
        usb.join(","),
        kernel.join(","),
        repairs.join(","),
        r.last_error.map(quoted).unwrap_or_else(|| "null".into()),
        quoted(&r.cfg.wifi_ssid),
        r.cfg.interval.as_secs(),
        r.cfg.fallback_metric,
        r.cfg.hub_table,
        r.cfg.tunnel_table,
    )
}

pub fn prom(r: &Report) -> String {
    let mut out = String::new();
    out.push_str("# HELP wan_manager_up The WAN manager loop is running\n");
    out.push_str("# TYPE wan_manager_up gauge\nwan_manager_up 1\n");
    out.push_str("# HELP wan_manager_ticks Completed reconciliation ticks\n");
    out.push_str("# TYPE wan_manager_ticks counter\n");
    out.push_str(&format!("wan_manager_ticks {}\n", r.ticks));
    out.push_str("# HELP wan_manager_state Selected egress class (1 for the current state)\n");
    out.push_str("# TYPE wan_manager_state gauge\n");
    for s in ["primary", "backup", "tunnel-only", "none"] {
        out.push_str(&format!(
            "wan_manager_state{{state=\"{s}\"}} {}\n",
            if r.sel.state == s { 1 } else { 0 }
        ));
    }
    let selected = r.sel.best.map(|i| r.candidates[i].info.name.clone());
    let tunnel = r.sel.tunnel.map(|i| r.candidates[i].info.name.clone());
    out.push_str("# HELP wan_manager_selected Whether this interface is the selected egress/tunnel\n");
    out.push_str("# TYPE wan_manager_selected gauge\n");
    for c in r.candidates {
        let sel = if selected.as_deref() == Some(c.info.name.as_str()) || tunnel.as_deref() == Some(c.info.name.as_str()) {
            1
        } else {
            0
        };
        out.push_str(&format!(
            "wan_manager_selected{{interface=\"{}\",kind=\"{}\"}} {sel}\n",
            c.info.name,
            c.info.kind.as_str()
        ));
    }
    out.push_str("# HELP wan_manager_candidate_eligible Whether the path is viable right now\n");
    out.push_str("# TYPE wan_manager_candidate_eligible gauge\n");
    out.push_str("# HELP wan_manager_candidate_latency_ms Last probe latency\n");
    out.push_str("# TYPE wan_manager_candidate_latency_ms gauge\n");
    out.push_str("# HELP wan_manager_candidate_throughput_bps Last measured HTTP throughput\n");
    out.push_str("# TYPE wan_manager_candidate_throughput_bps gauge\n");
    out.push_str("# HELP wan_manager_candidate_score Ranking score\n");
    out.push_str("# TYPE wan_manager_candidate_score gauge\n");
    for c in r.candidates {
        let labels = format!(
            "interface=\"{}\",kind=\"{}\"",
            c.info.name,
            c.info.kind.as_str()
        );
        out.push_str(&format!(
            "wan_manager_candidate_eligible{{{labels}}} {}\n",
            if c.eligible() { 1 } else { 0 }
        ));
        if let Some(v) = c.probe.latency_ms {
            if v.is_finite() {
                out.push_str(&format!("wan_manager_candidate_latency_ms{{{labels}}} {v}\n"));
            }
        }
        if let Some(v) = c.throughput_bps {
            if v.is_finite() {
                out.push_str(&format!(
                    "wan_manager_candidate_throughput_bps{{{labels}}} {v}\n"
                ));
            }
        }
        out.push_str(&format!(
            "wan_manager_candidate_score{{{labels}}} {:.2}\n",
            c.score
        ));
    }
    out.push_str("# HELP wan_manager_wifi_signal_dbm Current association signal\n");
    out.push_str("# TYPE wan_manager_wifi_signal_dbm gauge\n");
    for w in r.wifi {
        if let Some(s) = w.signal_dbm {
            out.push_str(&format!(
                "wan_manager_wifi_signal_dbm{{interface=\"{}\"}} {s}\n",
                w.iface
            ));
        }
    }
    let (_, usb_ok) = usb_diagnosis(r.usb, r.usb_failures);
    out.push_str("# HELP wan_manager_usb_tether_ok Whether a phone-class USB netdev is present\n");
    out.push_str("# TYPE wan_manager_usb_tether_ok gauge\n");
    out.push_str(&format!(
        "wan_manager_usb_tether_ok {}\n",
        if usb_ok { 1 } else { 0 }
    ));
    out
}

pub fn write(r: &Report) -> bool {
    let j = json(r);
    let p = prom(r);
    let a = crate::util::write_if_changed(&r.cfg.status_json(), &j, 0o644);
    let b = crate::util::write_if_changed(&r.cfg.status_prom(), &p, 0o644);
    a || b
}

pub fn kind_label(k: &Kind) -> &'static str {
    k.as_str()
}
