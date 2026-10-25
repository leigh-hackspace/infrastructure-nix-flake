//! The text formats the API serves: the snapshot and history JSON, and the
//! Prometheus exposition.  Pure renderings of a [`Snapshot`] — no state, no
//! locks — so they are testable without a sampler running.

use crate::json::{jopt_num, jopt_u64};
use crate::model::{HistoryPoint, Snapshot};
use crate::Args;
use common_json::quoted;
use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// Snapshot and history JSON (the metrics exposition is below; the JSON
// primitives these use live in `crate::json`)
// ---------------------------------------------------------------------------

pub(crate) fn snapshot_json(s: &Snapshot, args: &Args) -> String {
    let ifaces: Vec<String> = s
        .ifaces
        .iter()
        .map(|i| {
            format!(
                r#"{{"name":{},"active":{},"down_bps":{},"up_bps":{},"down_total":{},"up_total":{},"errors":{}}}"#,
                quoted(&i.name),
                i.active,
                jopt_num(i.down_bps),
                jopt_num(i.up_bps),
                i.ibytes,
                i.obytes,
                i.ierrors + i.oerrors,
            )
        })
        .collect();
    let issues: Vec<String> = s
        .issues
        .iter()
        .map(|i| format!(r#"{{"level":{},"message":{}}}"#, quoted(&i.level), quoted(&i.message)))
        .collect();
    let load = match s.load {
        Some([a, b, c]) => format!("[{a},{b},{c}]"),
        None => "null".into(),
    };
    format!(
        r#"{{"ts":{},"ok":{},"error":{},"router":{{"hostname":{},"uptime_secs":{},"load":{},"ncpu":{},"cpu":{{"user":{},"system":{},"idle":{}}},"mem":{{"free":{},"total":{}}},"conntrack":{},"own_tcp":{},"retrans_rate":{}}},"ifaces":[{}],"issues":[{}]}}"#,
        s.ts,
        s.ok,
        s.error.as_deref().map(quoted).unwrap_or_else(|| "null".into()),
        quoted(&args.hostname),
        jopt_u64(s.uptime_secs),
        load,
        jopt_u64(s.ncpu.map(|n| n as u64)),
        jopt_num(s.cpu_user),
        jopt_num(s.cpu_sys),
        jopt_num(s.cpu_idle),
        jopt_u64(s.mem_free),
        jopt_u64(s.mem_total),
        jopt_u64(s.conntrack),
        jopt_u64(s.own_tcp),
        jopt_num(s.retrans_rate),
        ifaces.join(","),
        issues.join(","),
    )
}
/// Latest snapshot in Prometheus text format. Cumulative counters (interface
/// bytes/errors, retransmissions) are exported as `_total` counters so
/// `rate()` works in PromQL; a router reboot resets them, which `rate()`
/// handles. The current per-interface byte rates are recomputed by
/// Prometheus from those counters, so they are not exported separately.
pub(crate) fn metrics_text(s: &Snapshot) -> String {
    let mut out = String::new();
    let gauge = |out: &mut String, name: &str, value: Option<f64>| {
        if let Some(v) = value {
            if v.is_finite() {
                out.push_str(&format!("{name} {v}\n"));
            }
        }
    };
    // `labels` is the full, already-quoted label set, e.g. `mode="user"`
    // or `interface="ppp0",direction="down"`.
    let labeled = |out: &mut String, name: &str, labels: &str, value: Option<f64>| {
        if let Some(v) = value {
            if v.is_finite() {
                out.push_str(&format!("{name}{{{labels}}} {v}\n"));
            }
        }
    };

    out.push_str("# HELP router_up Whether the most recent router probe succeeded (1/0)\n");
    out.push_str("# TYPE router_up gauge\n");
    out.push_str(&format!("router_up {}\n", if s.ok { 1 } else { 0 }));

    let uptime = s.uptime_secs.map(|u| u as f64);
    gauge(&mut out, "router_uptime_seconds", uptime);
    for (name, v) in [
        ("router_load1", s.load.map(|l| l[0])),
        ("router_load5", s.load.map(|l| l[1])),
        ("router_load15", s.load.map(|l| l[2])),
    ] {
        gauge(&mut out, name, v);
    }
    gauge(
        &mut out,
        "router_ncpu",
        s.ncpu.map(|n| n as f64),
    );
    for (mode, v) in [
        ("user", s.cpu_user),
        ("system", s.cpu_sys),
        ("idle", s.cpu_idle),
    ] {
        labeled(&mut out, "router_cpu_percent", &format!("mode=\"{mode}\""), v);
    }
    for (state, v) in [
        ("free", s.mem_free.map(|b| b as f64)),
        ("total", s.mem_total.map(|b| b as f64)),
    ] {
        labeled(&mut out, "router_mem_bytes", &format!("state=\"{state}\""), v);
    }
    gauge(&mut out, "router_conntrack_entries", s.conntrack.map(|n| n as f64));
    gauge(&mut out, "router_tcp_sockets", s.own_tcp.map(|n| n as f64));
    gauge(
        &mut out,
        "router_retransmissions_rate_per_second",
        s.retrans_rate,
    );
    gauge(
        &mut out,
        "router_retransmissions_total",
        s.retrans_total.map(|n| n as f64),
    );
    for i in &s.ifaces {
        // Linux interface names (alphanumerics, -, _, .) are safe inside a
        // label value as-is; /proc/net/dev does not include iproute's @parent.
        labeled(&mut out, "router_interface_up", &format!("interface=\"{}\"", i.name), Some(if i.active { 1.0 } else { 0.0 }));
        labeled(
            &mut out,
            "router_interface_bytes_total",
            &format!("interface=\"{}\",direction=\"down\"", i.name),
            Some(i.ibytes as f64),
        );
        labeled(
            &mut out,
            "router_interface_bytes_total",
            &format!("interface=\"{}\",direction=\"up\"", i.name),
            Some(i.obytes as f64),
        );
        labeled(
            &mut out,
            "router_interface_errors_total",
            &format!("interface=\"{}\"", i.name),
            Some((i.ierrors + i.oerrors) as f64),
        );
    }
    // Active "potential issues" as 0/1 gauges; level+message are labels so
    // a Grafana table can list them. Messages are short and stable-ish;
    // they never appear when an issue is clear.
    out.push_str("# HELP router_issue Active potential issue (1), labelled by level and message\n");
    out.push_str("# TYPE router_issue gauge\n");
    for issue in &s.issues {
        out.push_str(&format!(
            "router_issue{{level=\"{}\",message={}}} 1\n",
            issue.level,
            quoted(&issue.message)
        ));
    }
    out
}

pub(crate) fn history_json(history: &[&HistoryPoint]) -> String {
    let ts: Vec<String> = history.iter().map(|h| h.ts.to_string()).collect();
    let mut names: BTreeSet<String> = BTreeSet::new();
    for h in history {
        for n in h.rates.keys() {
            names.insert(n.clone());
        }
    }
    let mut ifaces_json = Vec::new();
    for n in &names {
        let up: Vec<String> = history.iter().map(|h| h.rates.get(n).map(|(u, _)| jopt_num(*u)).unwrap_or_else(|| "null".into())).collect();
        let down: Vec<String> = history.iter().map(|h| h.rates.get(n).map(|(_, d)| jopt_num(*d)).unwrap_or_else(|| "null".into())).collect();
        ifaces_json.push(format!(
            r#"{}:{{"up":[{}],"down":[{}]}}"#,
            quoted(n),
            up.join(","),
            down.join(",")
        ));
    }
    let conntrack: Vec<String> = history.iter().map(|h| jopt_u64(h.conntrack)).collect();
    let load: Vec<String> = history.iter().map(|h| jopt_num(h.load1)).collect();
    format!(
        r#"{{"ts":[{}],"ifaces":{{{}}},"conntrack":[{}],"load":[{}]}}"#,
        ts.join(","),
        ifaces_json.join(","),
        conntrack.join(","),
        load.join(",")
    )
}

