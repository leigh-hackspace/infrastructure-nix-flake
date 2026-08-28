//! network-status — real-time web dashboard for the OPNsense router (10.3.1.1).
//!
//! Every few seconds it ssh's to the router (root, machine-hop key) and runs a
//! small `sh -s` script that emits a machine-readable snapshot:
//!
//!   * per-interface byte/error counters (`netstat -ibn`, link rows only),
//!   * load / uptime / process count / CPU / memory (`top -b`),
//!   * firewall state-table size (`pfctl -s info`),
//!   * cumulative TCP retransmissions (`netstat -s`),
//!   * per-interface link status (`ifconfig -a`),
//!   * the router's own TCP/UDP socket count (`netstat -b`).
//!
//! Byte counters are differenced between samples to give up/down bandwidth
//! per interface; a rolling in-memory history (default 1 hour) powers the
//! realtime charts.  It also raises "potential issues": WAN link down,
//! new interface errors, high TCP retransmit rate, load above the CPU count,
//! low free memory, and router unreachable.
//!
//! Endpoints (served behind nginx at network-info.int.leighhack.org):
//!
//!   GET  /               HTML dashboard (hand-rolled canvas charts, no CDN)
//!   GET  /api/snapshot   latest snapshot as JSON
//!   GET  /api/history    full rolling history as JSON
//!
//! Zero external crates (house style, see status-dashboard/): ssh goes
//! through the `ssh` binary, JSON is hand-rolled, the frontend uses no
//! external JavaScript.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SSH_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_HISTORY: usize = 1440; // ~1 hour at the default 5s interval

/// Collection script, fed to `ssh ... 'sh -s'` on the router (the root shell
/// is tcsh, so everything POSIX-sh goes through `sh -s` on stdin — no
/// redirections, no quote hell).
const REMOTE_SCRIPT: &str = r#"
echo ==IFACES==
netstat -ibn
echo ==TOP==
top -b | sed -n '1,4p'
echo ==PF==
pfctl -s info | grep 'current entries'
echo ==RETRANS==
netstat -s | grep -m1 'retransmitted$'
echo ==LINKS==
ifconfig -a | awk '/^[a-z]/{name=$1} /status:/{print name, $2}'
echo ==CONNS==
netstat -b | tail -n +3 | grep -c '^tcp'
echo ==NCPU==
sysctl -n hw.ncpu
"#;

/// Pseudo/loopback interfaces that are never interesting.
const IGNORED_IFACES: [&str; 3] = ["lo0", "pflog0", "pfsync0"];

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Iface {
    name: String,
    active: bool,
    ibytes: u64,
    obytes: u64,
    ierrors: u64,
    oerrors: u64,
    /// Bytes/sec since the previous sample (None on the first sample).
    down_bps: Option<f64>,
    up_bps: Option<f64>,
    /// New interface errors (in+out) during the last interval.
    err_delta: u64,
}

#[derive(Clone)]
struct Issue {
    level: String, // "bad" | "warn"
    message: String,
}

#[derive(Clone, Default)]
struct Snapshot {
    ts: i64,
    /// Whether the most recent ssh probe succeeded.
    ok: bool,
    error: Option<String>,
    uptime_secs: Option<u64>,
    load: Option<[f64; 3]>,
    nprocs: Option<u32>,
    cpu_user: Option<f64>,
    cpu_sys: Option<f64>,
    cpu_idle: Option<f64>,
    mem_free: Option<u64>,
    mem_active: Option<u64>,
    mem_wired: Option<u64>,
    pf_states: Option<u64>,
    own_tcp: Option<u64>,
    retrans_rate: Option<f64>, // retransmissions per second
    retrans_total: Option<u64>, // cumulative, for diffing between samples
    ifaces: Vec<Iface>,
    issues: Vec<Issue>,
}

struct HistoryPoint {
    ts: i64,
    /// interface -> (up_bps, down_bps)
    rates: BTreeMap<String, (Option<f64>, Option<f64>)>,
    pf: Option<u64>,
    load1: Option<f64>,
}

struct State {
    last: Mutex<Option<Snapshot>>,
    history: Mutex<VecDeque<HistoryPoint>>,
}

// ---------------------------------------------------------------------------
// Router probe
// ---------------------------------------------------------------------------

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Run the collection script on the router; returns its stdout.
fn fetch_router(ssh_key: &str, router: &str) -> Result<String, String> {
    let mut child = Command::new("ssh")
        .arg("-i")
        .arg(ssh_key)
        .args(["-o", "BatchMode=yes"])
        .args(["-o", "ConnectTimeout=10"])
        .args(["-o", "StrictHostKeyChecking=accept-new"])
        .arg(router)
        .arg("sh -s")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn ssh: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(REMOTE_SCRIPT.as_bytes()) {
            let _ = child.kill();
            return Err(format!("failed to write probe script to ssh: {e}"));
        }
    }
    // Output is small (~10 KB, well under the pipe buffer), so draining
    // stdout after the child exits is safe.
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().ok().flatten() {
            let mut out = String::new();
            if let Some(mut stdout) = child.stdout.take() {
                let _ = std::io::Read::read_to_string(&mut stdout, &mut out);
            }
            if status.success() && !out.trim().is_empty() {
                return Ok(out);
            }
            if status.success() {
                return Err("empty output from router probe".into());
            }
            return Err("ssh to router failed (check key/ssh access)".into());
        }
        if start.elapsed() > SSH_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err("router probe timed out".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Parsed {
    /// name -> (ibytes, obytes, ierrors, oerrors)
    ifaces: BTreeMap<String, (u64, u64, u64, u64)>,
    links: BTreeMap<String, bool>,
    uptime_secs: Option<u64>,
    load: Option<[f64; 3]>,
    nprocs: Option<u32>,
    cpu_user: Option<f64>,
    cpu_sys: Option<f64>,
    cpu_idle: Option<f64>,
    mem_free: Option<u64>,
    mem_active: Option<u64>,
    mem_wired: Option<u64>,
    pf_states: Option<u64>,
    own_tcp: Option<u64>,
    retrans_total: Option<u64>,
}

/// Parse a "5+00:39:58" or "00:39:58" uptime string.
fn parse_uptime(s: &str) -> Option<u64> {
    let (days, rest) = match s.split_once('+') {
        Some((d, r)) => (d, r),
        None => ("0", s),
    };
    let days: u64 = days.parse().ok()?;
    let mut parts = rest.split(':');
    let h: u64 = parts.next()?.parse().ok()?;
    let m: u64 = parts.next()?.parse().ok()?;
    let sec: u64 = parts.next()?.parse().ok()?;
    Some(days * 86400 + h * 3600 + m * 60 + sec)
}

/// Parse a "41M" / "56K" / "1.2G" memory value into bytes.
fn parse_mem(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mult) = match s.chars().next_back() {
        Some('K') => (s[..s.len() - 1].trim().parse::<f64>().ok()?, 1024u64),
        Some('M') => (s[..s.len() - 1].trim().parse::<f64>().ok()?, 1024 * 1024),
        Some('G') => (s[..s.len() - 1].trim().parse::<f64>().ok()?, 1024u64 * 1024 * 1024),
        _ => (s.parse::<f64>().ok()?, 1),
    };
    Some((num * mult as f64) as u64)
}

/// Parse "0.5% user" style segments (value word) from a comma-separated line
/// after stripping its leading "CPU:"/"Mem:" label.
fn parse_kv_line(line: &str, label: &str) -> Vec<(f64, String)> {
    let rest = match line.split_once(label) {
        Some((_, r)) => r,
        None => return Vec::new(),
    };
    rest.split(',')
        .filter_map(|seg| {
            let mut it = seg.split_whitespace();
            let value = it.next()?;
            let key = it.next()?.to_string();
            let num = value.trim_end_matches('%').parse::<f64>().ok()?;
            Some((num, key))
        })
        .collect()
}

fn parse_output(raw: &str) -> Parsed {
    let mut p = Parsed::default();
    let mut section = "";
    for line in raw.lines() {
        let t = line.trim_start();
        if let Some(name) = t.strip_prefix("==") {
            if let Some(end) = name.find("==") {
                section = &name[..end];
                continue;
            }
        }
        if t.is_empty() {
            continue;
        }
        match section {
            "IFACES" => {
                // Name Mtu Network Address Ipkts Ierrs Idrop Ibytes Opkts Oerrs Obytes Coll
                let toks: Vec<&str> = t.split_whitespace().collect();
                if toks.len() < 12 || !toks[2].starts_with('<') {
                    continue; // header row or per-address row
                }
                let name = toks[0].trim_end_matches('*').to_string();
                if IGNORED_IFACES.contains(&name.as_str()) {
                    continue;
                }
                let ib: u64 = toks[7].parse().unwrap_or(0);
                let ob: u64 = toks[10].parse().unwrap_or(0);
                let ie: u64 = toks[5].parse().unwrap_or(0);
                let oe: u64 = toks[9].parse().unwrap_or(0);
                p.ifaces.entry(name).or_insert((ib, ob, ie, oe));
            }
            "TOP" => {
                // line 1: ... load averages: L1, L2, L3  up 5+00:39:58  HH:MM:SS
                if t.contains("load averages:") {
                    let rest = t.split_once("load averages:").map(|(_, r)| r).unwrap_or("");
                    let loads: Vec<f64> = rest
                        .split_whitespace()
                        .take(3)
                        .filter_map(|v| v.trim_end_matches(',').parse().ok())
                        .collect();
                    if loads.len() == 3 {
                        p.load = Some([loads[0], loads[1], loads[2]]);
                    }
                    if let Some((_, after)) = t.split_once(" up ") {
                        if let Some(up) = after.split_whitespace().next() {
                            p.uptime_secs = parse_uptime(up);
                        }
                    }
                // line 2: "70 processes:  1 running, 69 sleeping"
                } else if t.contains("processes:") {
                    let n: u32 = t.split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0);
                    p.nprocs = Some(n);
                } else if let Some(kvs) = t.strip_prefix("CPU:") {
                    for (v, k) in parse_kv_line(kvs, "") {
                        match k.as_str() {
                            "user" => p.cpu_user = Some(v),
                            "system" => p.cpu_sys = Some(v),
                            "idle" => p.cpu_idle = Some(v),
                            _ => {}
                        }
                    }
                } else if let Some(rest) = t.strip_prefix("Mem:") {
                    // "Mem: 41M Active, 445M Inact, 2213M Wired, 56K Buf, 5131M Free"
                    for seg in rest.split(',') {
                        let mut it = seg.split_whitespace();
                        let value = it.next();
                        let key = it.next();
                        if let (Some(value), Some(key)) = (value, key) {
                            if let Some(b) = parse_mem(value) {
                                match key {
                                    "Active" => p.mem_active = Some(b),
                                    "Wired" => p.mem_wired = Some(b),
                                    "Free" => p.mem_free = Some(b),
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
            "PF" => {
                if t.contains("current entries") {
                    if let Some(last) = t.split_whitespace().last() {
                        p.pf_states = last.parse().ok();
                    }
                }
            }
            "RETRANS" => {
                if let Some(first) = t.split_whitespace().next() {
                    if p.retrans_total.is_none() {
                        p.retrans_total = first.parse().ok();
                    }
                }
            }
            "LINKS" => {
                if let Some((name, status)) = t.split_once(':') {
                    p.links.insert(name.trim().to_string(), status.trim() == "active");
                }
            }
            "CONNS" => {
                if let Ok(n) = t.parse::<u64>() {
                    p.own_tcp = Some(n);
                }
            }
            "NCPU" => {
                if let Ok(n) = t.parse::<u32>() {
                    p.nprocs = Some(n);
                }
            }
            _ => {}
        }
    }

    p
}

// ---------------------------------------------------------------------------
// Sampling
// ---------------------------------------------------------------------------

/// Bytes/sec between two counter readings; a counter going backwards
/// (router reboot) is treated as traffic since zero.
fn rate(cur: u64, prev: u64, dt: f64) -> Option<f64> {
    if dt <= 0.0 {
        return None;
    }
    let bytes = if cur < prev { cur } else { cur - prev };
    Some(bytes as f64 / dt)
}

fn compute_issues(args: &Args, snap: &mut Snapshot) {
    snap.issues.clear();

    // WAN link down is the big one.
    if let Some(wan) = snap.ifaces.iter().find(|i| i.name == args.wan) {
        if !wan.active {
            snap.issues.push(Issue {
                level: "bad".into(),
                message: format!("WAN interface {} has no link", wan.name),
            });
        }
    }

    // New interface errors on an active link.
    for i in &snap.ifaces {
        if i.active && i.err_delta > 0 {
            snap.issues.push(Issue {
                level: "warn".into(),
                message: format!(
                    "{}: {} new interface error(s) in the last interval",
                    i.name, i.err_delta
                ),
            });
        }
    }

    // Sustained TCP retransmissions.
    if let Some(r) = snap.retrans_rate {
        if r > 0.5 {
            snap.issues.push(Issue {
                level: "warn".into(),
                message: format!("high TCP retransmit rate on router: {r:.2}/s"),
            });
        }
    }

    // Load above CPU count.
    if let (Some(load), Some(n)) = (snap.load, snap.nprocs) {
        if load[0] > f64::from(n) {
            snap.issues.push(Issue {
                level: "warn".into(),
                message: format!("load {:.2} above {} CPUs (1 min)", load[0], n),
            });
        }
    }

    // Memory pressure: < 10% free.
    if let (Some(free), Some(active), Some(wired)) = (snap.mem_free, snap.mem_active, snap.mem_wired) {
        let total = free + active + wired;
        if total > 0 && free * 10 < total {
            snap.issues.push(Issue {
                level: "warn".into(),
                message: format!(
                    "router memory low: {} free of {} total",
                    fmt_bytes(free),
                    fmt_bytes(total)
                ),
            });
        }
    }

    // Router unreachable (set by the sampler on ssh failure).
    if !snap.ok {
        let msg = snap.error.clone().unwrap_or_else(|| "cannot reach router".into());
        if !snap.issues.iter().any(|i| i.message == msg) {
            snap.issues.insert(0, Issue {
                level: "bad".into(),
                message: format!("cannot reach router: {msg}"),
            });
        }
    }

}

/// Human byte formatting for issue messages (server-side, only for warnings).
fn fmt_bytes(b: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let v = b as f64;
    if v >= GB {
        format!("{:.1} GiB", v / GB)
    } else if v >= MB {
        format!("{:.0} MiB", v / MB)
    } else if v >= KB {
        format!("{:.0} KiB", v / KB)
    } else {
        format!("{b} B")
    }
}

fn sample_once(args: &Args, prev: &Option<Snapshot>) -> Snapshot {
    let ts = now_ts();
    match fetch_router(&args.ssh_key, &args.router) {
        Ok(raw) => {
            let parsed = parse_output(&raw);
            let dt = prev.as_ref().map(|p| (ts - p.ts) as f64).unwrap_or(0.0);

            let mut ifaces = Vec::new();
            for (name, (ib, ob, ie, oe)) in &parsed.ifaces {
                let (down_bps, up_bps, err_delta) = match prev {
                    Some(p) if dt > 0.0 => {
                        let prev_c = p
                            .ifaces
                            .iter()
                            .find(|i| &i.name == name)
                            .map(|i| (i.ibytes, i.obytes, i.ierrors + i.oerrors));
                        match prev_c {
                            Some((pib, pob, perr)) => (
                                rate(*ib, pib, dt),
                                rate(*ob, pob, dt),
                                (ie + oe).saturating_sub(perr),
                            ),
                            None => (None, None, 0),
                        }
                    }
                    _ => (None, None, 0),
                };
                let active = parsed.links.get(name).copied().unwrap_or(false);
                ifaces.push(Iface {
                    name: name.clone(),
                    active,
                    ibytes: *ib,
                    obytes: *ob,
                    ierrors: *ie,
                    oerrors: *oe,
                    down_bps,
                    up_bps,
                    err_delta,
                });
            }
            ifaces.sort_by(|a, b| a.name.cmp(&b.name));

            let retrans_rate = match (parsed.retrans_total, prev) {
                (Some(t), Some(p)) if dt > 0.0 => match p.retrans_total {
                    Some(pt) if t >= pt => Some((t - pt) as f64 / dt),
                    _ => None,
                },
                _ => None,
            };

            let mut snap = Snapshot {
                ts,
                ok: true,
                error: None,
                uptime_secs: parsed.uptime_secs,
                load: parsed.load,
                nprocs: parsed.nprocs,
                cpu_user: parsed.cpu_user,
                cpu_sys: parsed.cpu_sys,
                cpu_idle: parsed.cpu_idle,
                mem_free: parsed.mem_free,
                mem_active: parsed.mem_active,
                mem_wired: parsed.mem_wired,
                pf_states: parsed.pf_states,
                own_tcp: parsed.own_tcp,
                retrans_rate,
                retrans_total: parsed.retrans_total,
                ifaces,
                issues: Vec::new(),
            };
            compute_issues(args, &mut snap);
            snap
        }
        Err(e) => {
            // Keep the last snapshot, mark it stale.
            let mut snap = match prev {
                Some(p) => p.clone(),
                None => Snapshot {
                    ts,
                    ..Default::default()
                },
            };
            snap.ok = false;
            snap.error = Some(e);
            compute_issues(args, &mut snap);
            snap
        }
    }
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn jnum(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "null".into()
    }
}

fn jopt_num(v: Option<f64>) -> String {
    v.map(jnum).unwrap_or_else(|| "null".into())
}

fn jopt_u64(v: Option<u64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "null".into())
}

fn opt_series(vals: &[Option<f64>]) -> String {
    format!(
        "[{}]",
        vals.iter().map(|v| jopt_num(*v)).collect::<Vec<_>>().join(",")
    )
}

fn snapshot_json(s: &Snapshot, args: &Args) -> String {
    let ifaces: Vec<String> = s
        .ifaces
        .iter()
        .map(|i| {
            format!(
                r#"{{"name":{},"active":{},"down_bps":{},"up_bps":{},"down_total":{},"up_total":{},"errors":{}}}"#,
                json_str(&i.name),
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
        .map(|i| format!(r#"{{"level":{},"message":{}}}"#, json_str(&i.level), json_str(&i.message)))
        .collect();
    let load = match s.load {
        Some([a, b, c]) => format!("[{a},{b},{c}]"),
        None => "null".into(),
    };
    format!(
        r#"{{"ts":{},"ok":{},"error":{},"router":{{"hostname":{},"uptime_secs":{},"load":{},"nprocs":{},"cpu":{{"user":{},"system":{},"idle":{}}},"mem":{{"free":{},"active":{},"wired":{}}},"pf_states":{},"own_tcp":{},"retrans_rate":{}}},"ifaces":[{}],"issues":[{}]}}"#,
        s.ts,
        s.ok,
        s.error.as_deref().map(json_str).unwrap_or_else(|| "null".into()),
        json_str(&router_host(&args.router)),
        jopt_u64(s.uptime_secs),
        load,
        jopt_u64(s.nprocs.map(|n| n as u64)),
        jopt_num(s.cpu_user),
        jopt_num(s.cpu_sys),
        jopt_num(s.cpu_idle),
        jopt_u64(s.mem_free),
        jopt_u64(s.mem_active),
        jopt_u64(s.mem_wired),
        jopt_u64(s.pf_states),
        jopt_u64(s.own_tcp),
        jopt_num(s.retrans_rate),
        ifaces.join(","),
        issues.join(","),
    )
}

/// `root@10.3.1.1` -> `10.3.1.1`.
fn router_host(router: &str) -> &str {
    router.rsplit('@').next().unwrap_or(router)
}

fn history_json(history: &[&HistoryPoint]) -> String {
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
            json_str(n),
            up.join(","),
            down.join(",")
        ));
    }
    let pf: Vec<String> = history.iter().map(|h| jopt_u64(h.pf)).collect();
    let load: Vec<String> = history.iter().map(|h| jopt_num(h.load1)).collect();
    format!(
        r#"{{"ts":[{}],"ifaces":{{{}}},"pf":[{}],"load":[{}]}}"#,
        ts.join(","),
        ifaces_json.join(","),
        pf.join(","),
        load.join(",")
    )
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Args {
    bind: String,
    port: u16,
    router: String,
    ssh_key: String,
    interval: Duration,
    wan: String,
    title: String,
}

fn route(method: &str, path: &str, state: &State, args: &Args) -> (u16, String, String) {
    match (method, path) {
        ("GET", "/") | ("GET", "/index.html") => (
            200,
            "text/html; charset=utf-8".to_string(),
            PAGE.replace("__TITLE__", &args.title),
        ),
        ("GET", "/api/snapshot") => {
            let g = state.last.lock().unwrap();
            match g.as_ref() {
                Some(s) => (200, "application/json".to_string(), snapshot_json(s, args)),
                None => (
                    503,
                    "application/json".to_string(),
                    r#"{"error":"no data yet (first router probe in progress)"}"#.to_string(),
                ),
            }
        }
        ("GET", "/api/history") => {
            let g = state.history.lock().unwrap();
            let points: Vec<&HistoryPoint> = g.iter().collect();
            (200, "application/json".to_string(), history_json(&points))
        }
        _ => (404, "application/json".to_string(), r#"{"error":"not found"}"#.to_string()),
    }
}

fn handle_client(mut stream: TcpStream, state: &State, args: &Args) {
    use std::io::Read;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        match stream.read(&mut tmp) {
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => return,
        }
        if buf.len() > 64 * 1024 {
            return;
        }
    }

    let head = String::from_utf8_lossy(&buf);
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    let (status, ctype, body) = route(method, path, state, args);
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "OK",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

// ---------------------------------------------------------------------------
// Frontend
// ---------------------------------------------------------------------------

const PAGE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>__TITLE__</title>
<style>
  :root {
    --bg: #0d1117; --panel: #161d26; --panel2: #1c2530; --border: #2a3542;
    --text: #d7e0e8; --muted: #8b98a5;
    --good: #2ecc71; --bad: #e74c3c; --warn: #f1c40f; --inactive: #7f8c8d;
    --up: #2dd4bf; --down: #60a5fa;
  }
  * { box-sizing: border-box; }
  body { margin: 0; background: var(--bg); color: var(--text);
         font: 14px/1.5 ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif; }
  main { max-width: 1240px; margin: 0 auto; padding: 20px; }
  header { display: flex; align-items: baseline; gap: 12px; flex-wrap: wrap; margin-bottom: 14px; }
  h1 { font-size: 20px; margin: 0; }
  h2 { font-size: 12px; text-transform: uppercase; letter-spacing: .05em; color: var(--muted); margin: 0 0 8px; }
  .sub, .muted { color: var(--muted); }
  .mono { font-variant-numeric: tabular-nums; font-family: ui-monospace, "SF Mono", Menlo, monospace; }
  .cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(360px, 1fr)); gap: 12px; margin-bottom: 16px; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: 10px; padding: 12px 14px; min-width: 0; }
  .card.wide { grid-column: 1 / -1; }
  .pill { display: inline-block; padding: 2px 10px; border-radius: 999px; font-size: 12px; font-weight: 600; }
  .pill.good { background: rgba(46,204,113,.15); color: var(--good); }
  .pill.bad { background: rgba(231,76,60,.15); color: var(--bad); }
  .pill.warn { background: rgba(241,196,15,.15); color: var(--warn); }
  .banner { display: none; background: rgba(231,76,60,.12); border: 1px solid rgba(231,76,60,.4);
            color: #fca5a5; border-radius: 8px; padding: 10px 14px; margin-bottom: 14px; }
  canvas { display: block; width: 100%; }
  .chart-head { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; margin-bottom: 8px; }
  .chart-head h2 { margin: 0; }
  .chart-head .now { margin-left: auto; font-size: 13px; }
  .legend { display: flex; gap: 14px; font-size: 12px; color: var(--muted); margin-top: 6px; flex-wrap: wrap; }
  .legend .sw { display: inline-block; width: 10px; height: 10px; border-radius: 3px; margin-right: 5px; vertical-align: -1px; }
  select { background: var(--panel2); color: var(--text); border: 1px solid var(--border);
           border-radius: 6px; padding: 3px 8px; font-size: 13px; }
  .iface-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(230px, 1fr)); gap: 10px; }
  .iface { background: var(--panel); border: 1px solid var(--border); border-radius: 10px; padding: 10px 12px; }
  .iface .head { display: flex; align-items: center; gap: 8px; }
  .iface .name { font-weight: 600; font-size: 13px; }
  .iface .rates { margin-top: 4px; font-size: 12px; display: flex; justify-content: space-between; }
  .dot { width: 8px; height: 8px; border-radius: 50%; display: inline-block; }
  .dot.up { background: var(--good); box-shadow: 0 0 6px rgba(46,204,113,.7); }
  .dot.down { background: var(--bad); }
  .issue { padding: 6px 10px; border-radius: 6px; margin-bottom: 6px; font-size: 13px; }
  .issue.bad { background: rgba(231,76,60,.12); border: 1px solid rgba(231,76,60,.35); color: #fca5a5; }
  .issue.warn { background: rgba(241,196,15,.10); border: 1px solid rgba(241,196,15,.3); color: #fde68a; }
  .upc { color: var(--up); } .downc { color: var(--down); }
</style>
</head>
<body>
<main>
  <header>
    <h1>&#x1F310; <span id="title"></span></h1>
    <span class="sub mono" id="meta"></span>
    <span class="pill good" id="statusPill" style="margin-left:auto">waiting&hellip;</span>
  </header>

  <div class="banner" id="errBanner"></div>

  <section class="cards">
    <div class="card wide">
      <div class="chart-head">
        <h2>Bandwidth</h2>
        <select id="ifaceSelect"></select>
        <span class="now mono" id="bwNow"></span>
      </div>
      <canvas id="bwChart" height="220"></canvas>
      <div class="legend">
        <span><span class="sw" style="background:var(--up)"></span>up (to internet)</span>
        <span><span class="sw" style="background:var(--down)"></span>down (from internet)</span>
        <span class="muted" id="bwTotal"></span>
      </div>
    </div>
    <div class="card">
      <h2>Connections (firewall state table)</h2>
      <canvas id="connChart" height="150"></canvas>
      <div class="legend"><span class="mono" id="connNow"></span></div>
    </div>
    <div class="card">
      <h2>Load average</h2>
      <canvas id="loadChart" height="150"></canvas>
      <div class="legend"><span class="mono" id="loadNow"></span></div>
    </div>
  </section>

  <section class="card" style="margin-bottom:16px">
    <h2>Interfaces</h2>
    <div class="iface-grid" id="ifaceGrid"></div>
  </section>

  <section class="card">
    <h2>Issues</h2>
    <div id="issues"></div>
  </section>
</main>
<script>
const MAXPTS = 720;          // keep 1 hour of 5s samples on the client
const BW_WINDOW = 120;       // points shown in the big bandwidth chart (10 min)
const CONN_WINDOW = 240;     // 20 min
const SPARK_WINDOW = 60;     // 5 min sparklines

// history: { ts: [], rates: { name: { up: [], down: [] } }, pf: [], load: [] }
const hist = { ts: [], rates: {}, pf: [], load: [] };
let lastTs = null;
let selectedIface = null;   // 'total' (default) or an interface name
let lastSnap = null;        // last snapshot, for re-render on dropdown change

// Sum of up/down across all interfaces, aligned to hist.ts (right-aligned,
// so an interface that only appears later doesn't shift the sums).
function totalSeries() {
  const n = hist.ts.length;
  const up = new Array(n).fill(null);
  const down = new Array(n).fill(null);
  for (const name in hist.rates) {
    const s = hist.rates[name];
    const len = s.up.length;
    const off = n - len;
    for (let i = 0; i < len; i++) {
      const j = off + i;
      if (s.up[i] != null) up[j] = (up[j] == null ? 0 : up[j]) + s.up[i];
      if (s.down[i] != null) down[j] = (down[j] == null ? 0 : down[j]) + s.down[i];
    }
  }
  return { up: up, down: down };
}

function pushPoint(p) {
  if (lastTs !== null && p.ts <= lastTs) return;
  lastTs = p.ts;
  hist.ts.push(p.ts);
  hist.pf.push(p.pf_states == null ? null : p.pf_states);
  hist.load.push(p.load ? p.load[0] : null);
  for (const i of p.ifaces) {
    if (!hist.rates[i.name]) hist.rates[i.name] = { up: [], down: [] };
    hist.rates[i.name].up.push(i.up_bps == null ? null : i.up_bps);
    hist.rates[i.name].down.push(i.down_bps == null ? null : i.down_bps);
  }
  while (hist.ts.length > MAXPTS) {
    hist.ts.shift();
    hist.pf.shift();
    hist.load.shift();
    for (const k in hist.rates) { hist.rates[k].up.shift(); hist.rates[k].down.shift(); }
  }
}

function seedFromHistory(h) {
  for (let i = 0; i < h.ts.length; i++) pushPoint({
    ts: h.ts[i],
    ifaces: Object.keys(h.ifaces).map(function (name) {
      return {
        name: name,
        up_bps: h.ifaces[name].up[i] == null ? null : h.ifaces[name].up[i],
        down_bps: h.ifaces[name].down[i] == null ? null : h.ifaces[name].down[i]
      };
    }),
    pf_states: h.pf[i] == null ? null : h.pf[i],
    load: h.load[i] == null ? null : [h.load[i]]
  });
}

// --- formatting ------------------------------------------------------------

function fmtRate(bps) {
  if (bps == null) return '–';
  if (bps >= 1e9) return (bps / 1e9).toFixed(2) + ' GB/s';
  if (bps >= 1e6) return (bps / 1e6).toFixed(2) + ' MB/s';
  if (bps >= 1e3) return (bps / 1e3).toFixed(1) + ' KB/s';
  return Math.round(bps) + ' B/s';
}
function fmtTotal(b) {
  if (b == null) return '–';
  if (b >= 1e12) return (b / 1e12).toFixed(2) + ' TB';
  if (b >= 1e9) return (b / 1e9).toFixed(2) + ' GB';
  if (b >= 1e6) return (b / 1e6).toFixed(1) + ' MB';
  if (b >= 1e3) return (b / 1e3).toFixed(1) + ' KB';
  return b + ' B';
}
function fmtDur(s) {
  if (s == null) return '–';
  const d = Math.floor(s / 86400), h = Math.floor(s % 86400 / 3600), m = Math.floor(s % 3600 / 60);
  if (d) return d + 'd ' + h + 'h ' + m + 'm';
  if (h) return h + 'h ' + m + 'm';
  return m + 'm ' + Math.floor(s % 60) + 's';
}
function fmtMem(b) {
  if (b == null) return '–';
  if (b >= 1e9) return (b / 1e9).toFixed(2) + ' GB';
  if (b >= 1e6) return (b / 1e6).toFixed(0) + ' MB';
  return (b / 1e3).toFixed(0) + ' KB';
}
function fmtTime(ts) {
  const d = new Date(ts * 1000);
  function p(n) { return n < 10 ? '0' + n : '' + n; }
  return p(d.getHours()) + ':' + p(d.getMinutes()) + ':' + p(d.getSeconds());
}

// --- canvas charting (hand-rolled, no dependencies) ------------------------

function setupCanvas(canvas, cssHeight) {
  const dpr = window.devicePixelRatio || 1;
  const w = canvas.clientWidth || 300;
  canvas.width = Math.max(1, Math.round(w * dpr));
  canvas.height = Math.max(1, Math.round(cssHeight * dpr));
  const ctx = canvas.getContext('2d');
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return { ctx, w, h: cssHeight };
}

function niceMax(v) {
  if (v <= 0) return 1;
  const exp = Math.pow(10, Math.floor(Math.log10(v)));
  const m = v / exp;
  if (m <= 1) return exp;
  if (m <= 2) return 2 * exp;
  if (m <= 5) return 5 * exp;
  return 10 * exp;
}

// series: [{data: (number|null)[], color, fill, window}]
function drawChart(canvas, series, opts) {
  opts = opts || {};
  const fmt = opts.fmt || (v => v.toFixed(1));
  const { ctx, w, h } = setupCanvas(canvas, opts.height || 200);
  const padL = 58, padR = 8, padT = 8, padB = 20;
  const pw = w - padL - padR, ph = h - padT - padB;

  let maxv = 0;
  for (const s of series) {
    const start = Math.max(0, s.data.length - s.window);
    for (let i = start; i < s.data.length; i++) {
      const v = s.data[i];
      if (v != null && v > maxv) maxv = v;
    }
  }
  const ymax = niceMax(maxv * 1.05);
  const n = series[0] ? series[0].data.length : 0;
  const win = series[0] ? series[0].window : n;

  ctx.clearRect(0, 0, w, h);
  ctx.font = '11px ui-monospace, monospace';

  // grid + y labels
  ctx.strokeStyle = '#232d38';
  ctx.fillStyle = '#8b98a5';
  ctx.lineWidth = 1;
  for (let g = 0; g <= 4; g++) {
    const y = padT + ph - (ph * g) / 4;
    ctx.beginPath();
    ctx.moveTo(padL, y);
    ctx.lineTo(w - padR, y);
    ctx.stroke();
    ctx.fillText(fmt((ymax * g) / 4), 4, y + 4);
  }
  // x labels (3). Series can be shorter than hist.ts (an interface that
  // appeared later in the history), so offset into the timestamp array.
  if (n > 1) {
    const ts = hist.ts;
    const off = ts.length - n;
    [0, Math.floor((n - 1) / 2), n - 1].forEach(function (i) {
      const x = padL + (pw * i) / (n - 1);
      const right = i === n - 1, mid = i === Math.floor((n - 1) / 2);
      ctx.textAlign = right ? 'right' : (mid ? 'center' : 'left');
      ctx.fillText(fmtTime(ts[off + i] || 0), x, h - 5);
    });
    ctx.textAlign = 'left';
  }

  if (n === 0) {
    ctx.fillStyle = '#8b98a5';
    ctx.fillText('no data yet', padL + 8, padT + ph / 2);
    return;
  }

  for (const s of series) {
    const start = Math.max(0, s.data.length - s.window);
    const count = s.data.length - start;
    if (count < 2) continue;
    const xAt = i => padL + (pw * (i - start)) / (count - 1);
    const yAt = v => padT + ph - (ph * Math.min(v, ymax)) / ymax;

    if (s.fill) {
      ctx.beginPath();
      let started = false;
      for (let i = start; i < s.data.length; i++) {
        const v = s.data[i];
        if (v == null) { started = false; continue; }
        const x = xAt(i), y = yAt(v);
        if (!started) { ctx.moveTo(x, y); started = true; }
        else ctx.lineTo(x, y);
      }
      // close the fill path down to the axis
      let lastX = null, firstX = null;
      for (let i = start; i < s.data.length; i++) {
        if (s.data[i] == null) continue;
        if (firstX === null) firstX = xAt(i);
        lastX = xAt(i);
      }
      if (firstX !== null && lastX !== null) {
        ctx.lineTo(lastX, padT + ph);
        ctx.lineTo(firstX, padT + ph);
        ctx.closePath();
        const grad = ctx.createLinearGradient(0, padT, 0, padT + ph);
        grad.addColorStop(0, s.color + '55');
        grad.addColorStop(1, s.color + '05');
        ctx.fillStyle = grad;
        ctx.fill();
      }
    }

    ctx.beginPath();
    let started = false;
    for (let i = start; i < s.data.length; i++) {
      const v = s.data[i];
      if (v == null) { started = false; continue; }
      const x = xAt(i), y = yAt(v);
      if (!started) { ctx.moveTo(x, y); started = true; }
      else ctx.lineTo(x, y);
    }
    ctx.strokeStyle = s.color;
    ctx.lineWidth = s.width || 1.8;
    ctx.lineJoin = 'round';
    ctx.stroke();
  }
}

// series: [{data: (number|null)[], color}]
function sparkline(canvas, series) {
  const { ctx, w, h } = setupCanvas(canvas, 46);
  ctx.clearRect(0, 0, w, h);
  const n = series[0] ? series[0].data.length : 0;
  const start = Math.max(0, n - SPARK_WINDOW);
  const count = n - start;
  if (count < 2) {
    ctx.fillStyle = '#4a5560';
    ctx.font = '10px ui-monospace, monospace';
    ctx.fillText('…', 4, h / 2 + 3);
    return;
  }
  let maxv = 0;
  for (const s of series)
    for (let i = start; i < n; i++) {
      const v = s.data[i];
      if (v != null && v > maxv) maxv = v;
    }
  const ymax = niceMax(maxv * 1.1) || 1;
  const xAt = i => (w * (i - start)) / (count - 1);
  const yAt = v => h - 2 - ((h - 6) * Math.min(v, ymax)) / ymax;
  for (const s of series) {
    ctx.beginPath();
    let started = false;
    for (let i = start; i < n; i++) {
      const v = s.data[i];
      if (v == null) { started = false; continue; }
      const x = xAt(i), y = yAt(v);
      if (!started) { ctx.moveTo(x, y); started = true; }
      else ctx.lineTo(x, y);
    }
    ctx.strokeStyle = s.color;
    ctx.lineWidth = 1.5;
    ctx.stroke();
  }
}

// --- rendering --------------------------------------------------------------

function interestingIfaces(snap) {
  return snap.ifaces.filter(function (i) {
    return i.active || i.down_total > 0 || i.up_total > 0;
  });
}

function render(snap) {
  document.getElementById('title').textContent = document.title;

  const r = snap.router;
  // textContent (no HTML entities) — use the literal middle dot.
  document.getElementById('meta').textContent =
    'router up ' + fmtDur(r.uptime_secs) +
    (r.nprocs != null ? ' · ' + r.nprocs + ' CPUs' : '') +
    (r.cpu.user != null ? ' · cpu ' + (100 - r.cpu.idle).toFixed(1) + '%' : '') +
    (r.mem.free != null ? ' · ' + fmtMem(r.mem.free) + ' mem free' : '') +
    ' · ' + fmtTime(snap.ts);

  const pill = document.getElementById('statusPill');
  if (!snap.ok) {
    pill.textContent = 'router unreachable';
    pill.className = 'pill bad';
  } else if (snap.issues.some(i => i.level === 'bad')) {
    pill.textContent = 'problem detected';
    pill.className = 'pill bad';
  } else if (snap.issues.length) {
    pill.textContent = snap.issues.length + ' warning' + (snap.issues.length > 1 ? 's' : '');
    pill.className = 'pill warn';
  } else {
    pill.textContent = 'all good';
    pill.className = 'pill good';
  }

  const banner = document.getElementById('errBanner');
  if (!snap.ok) {
    banner.style.display = 'block';
    banner.textContent = 'Cannot reach the router (10.3.1.1) — ' + (snap.error || 'unknown error') +
      '. Showing last known data; will retry automatically.';
  } else {
    banner.style.display = 'none';
  }

  // interface selector: a synthetic 'total' (all interfaces, the default)
  // plus every interface that is active or has traffic.
  const ifaces = interestingIfaces(snap);
  const sel = document.getElementById('ifaceSelect');
  if (!selectedIface) selectedIface = 'total';
  if (selectedIface !== 'total' && !ifaces.some(i => i.name === selectedIface)) {
    selectedIface = 'total'; // previously selected interface went away
  }
  const options = [{ name: 'total', label: 'total (all interfaces)' }].concat(
    ifaces.map(function (i) {
      return { name: i.name, label: i.name === (snap.wan || 'em0') ? i.name + ' (WAN)' : i.name };
    }));
  const sig = options.map(o => o.name).join(',');
  if (sel.dataset.sig !== sig) {
    sel.dataset.sig = sig;
    sel.innerHTML = options.map(function (o) {
      return '<option value="' + o.name + '"' + (o.name === selectedIface ? ' selected' : '') + '>' +
        o.label + '</option>';
    }).join('');
  } else if (sel.value !== selectedIface) {
    sel.value = selectedIface;
  }

  // Bandwidth chart: either the synthetic total or one interface.
  let upSeries, downSeries, curUp, curDown, totUp, totDown;
  if (selectedIface === 'total') {
    const t = totalSeries();
    upSeries = t.up; downSeries = t.down;
    curUp = 0; curDown = 0; totUp = 0; totDown = 0;
    for (const i of snap.ifaces) {
      if (i.up_bps != null) curUp += i.up_bps;
      if (i.down_bps != null) curDown += i.down_bps;
      totUp += i.up_total || 0;
      totDown += i.down_total || 0;
    }
  } else {
    const h = hist.rates[selectedIface];
    if (h) {
      upSeries = h.up; downSeries = h.down;
      const cur = snap.ifaces.find(i => i.name === selectedIface);
      curUp = cur ? cur.up_bps : null;
      curDown = cur ? cur.down_bps : null;
      totUp = cur ? cur.up_total : null;
      totDown = cur ? cur.down_total : null;
    }
  }
  if (upSeries) {
    drawChart(document.getElementById('bwChart'), [
      { data: upSeries, color: '#2dd4bf', fill: true, window: BW_WINDOW },
      { data: downSeries, color: '#60a5fa', fill: true, window: BW_WINDOW }
    ], { fmt: fmtRate, height: 220 });
    document.getElementById('bwNow').innerHTML =
      '<span class="upc">▲ ' + fmtRate(curUp) + '</span> &nbsp; <span class="downc">▼ ' +
      fmtRate(curDown) + '</span>';
    document.getElementById('bwTotal').textContent =
      (selectedIface === 'total' ? 'all: ' : '') + '▼ ' + fmtTotal(totDown) + ' · ▲ ' + fmtTotal(totUp);
  }

  drawChart(document.getElementById('connChart'),
    [{ data: hist.pf, color: '#a78bfa', fill: true, window: CONN_WINDOW }],
    { fmt: v => Math.round(v) + '', height: 150 });
  document.getElementById('connNow').textContent =
    (r.pf_states != null ? r.pf_states + ' active states' : '–') +
    (r.own_tcp != null ? '  ·  ' + r.own_tcp + ' router sockets' : '') +
    (r.retrans_rate != null ? '  ·  retrans ' + r.retrans_rate.toFixed(2) + '/s' : '');

  drawChart(document.getElementById('loadChart'),
    [{ data: hist.load, color: '#f1c40f', fill: true, window: CONN_WINDOW }],
    { fmt: v => v.toFixed(1), height: 150 });
  document.getElementById('loadNow').textContent = r.load
    ? 'load ' + r.load.map(l => l.toFixed(2)).join(' / ') + (r.nprocs ? ' on ' + r.nprocs + ' CPUs' : '')
    : '–';

  // per-interface cards
  const grid = document.getElementById('ifaceGrid');
  const cards = ifaces.map(function (i) {
    const up = hist.rates[i.name] ? hist.rates[i.name].up : [];
    const down = hist.rates[i.name] ? hist.rates[i.name].down : [];
    return { i: i, up: up, down: down };
  }).sort(function (a, b) {
    return (b.i.down_total + b.i.up_total) - (a.i.down_total + a.i.up_total);
  });
  const existing = grid.querySelectorAll('.iface canvas');
  grid.innerHTML = cards.map(function (c, idx) {
    return '<div class="iface" data-idx="' + idx + '">' +
      '<div class="head"><span class="dot ' + (c.i.active ? 'up' : 'down') + '"></span>' +
      '<span class="name">' + c.i.name + (c.i.name === (snap.wan || 'em0') ? ' <span class="muted">(WAN)</span>' : '') +
      '</span><span class="muted mono" style="margin-left:auto">' +
      (c.i.active ? 'linked' : 'no carrier') + '</span></div>' +
      '<canvas></canvas>' +
      '<div class="rates mono">' +
      '<span class="upc">▲ ' + fmtRate(c.i.up_bps) + '</span>' +
      '<span class="downc">▼ ' + fmtRate(c.i.down_bps) + '</span></div>' +
      '</div>';
  }).join('');
  grid.querySelectorAll('.iface').forEach(function (el, idx) {
    const c = cards[idx];
    sparkline(el.querySelector('canvas'), [
      { data: c.down, color: '#60a5fa' },
      { data: c.up, color: '#2dd4bf' }
    ]);
  });

  // issues
  const issuesEl = document.getElementById('issues');
  if (!snap.issues.length) {
    issuesEl.innerHTML = '<span class="muted">✔ no issues detected</span>';
  } else {
    issuesEl.innerHTML = snap.issues.map(function (i) {
      return '<div class="issue ' + i.level + '">' + (i.level === 'bad' ? '&#x1F534; ' : '&#x26A0; ') +
        i.message + '</div>';
    }).join('');
  }
}

// --- main loop ---------------------------------------------------------------

function poll() {
  fetch('/api/snapshot')
    .then(function (r) { return r.json().then(function (b) { return { ok: r.ok, b: b }; }); })
    .then(function (res) {
      if (!res.ok || res.b.error) return;
      pushPoint({
        ts: res.b.ts,
        ifaces: res.b.ifaces,
        pf_states: res.b.router.pf_states,
        load: res.b.router.load
      });
      res.b.wan = WAN;
      lastSnap = res.b;
      render(res.b);
    })
    .catch(function () {});
}

const WAN = '__WAN__';
document.getElementById('title').textContent = document.title;
document.title = '__TITLE__';

fetch('/api/history')
  .then(function (r) { return r.json(); })
  .then(seedFromHistory)
  .catch(function () {})
  .finally(function () {
    // Re-render immediately when the user picks a different interface.
    document.getElementById('ifaceSelect').addEventListener('change', function () {
      selectedIface = this.value;
      if (lastSnap) render(lastSnap);
    });
    poll();
    setInterval(poll, 5000);
    window.addEventListener('resize', function () { poll(); });
  });
</script>
</body>
</html>
"##;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut bind = "127.0.0.1".to_string();
    let mut port: u16 = 8091;
    let mut router = "root@10.3.1.1".to_string();
    let mut ssh_key = "/home/leigh-admin/.ssh/agent-hop-key".to_string();
    let mut interval_secs: u64 = 5;
    let mut wan = "em0".to_string();
    let mut title = "network-info".to_string();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--bind" => {
                if let Some(v) = argv.get(i + 1) {
                    bind = v.clone();
                    i += 1;
                }
            }
            "--port" => {
                if let Some(v) = argv.get(i + 1) {
                    port = v.parse().unwrap_or(port);
                    i += 1;
                }
            }
            "--router" => {
                if let Some(v) = argv.get(i + 1) {
                    router = v.clone();
                    i += 1;
                }
            }
            "--ssh-key" => {
                if let Some(v) = argv.get(i + 1) {
                    ssh_key = v.clone();
                    i += 1;
                }
            }
            "--interval" => {
                if let Some(v) = argv.get(i + 1) {
                    interval_secs = v.parse().unwrap_or(interval_secs);
                    i += 1;
                }
            }
            "--wan" => {
                if let Some(v) = argv.get(i + 1) {
                    wan = v.clone();
                    i += 1;
                }
            }
            "--title" => {
                if let Some(v) = argv.get(i + 1) {
                    title = v.clone();
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }

    let args = Args {
        bind,
        port,
        router,
        ssh_key,
        interval: Duration::from_secs(interval_secs),
        wan,
        title,
    };
    let state = Arc::new(State {
        last: Mutex::new(None),
        history: Mutex::new(VecDeque::with_capacity(MAX_HISTORY)),
    });

    let listener = match TcpListener::bind((args.bind.as_str(), args.port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("network-status: failed to bind {}:{}", args.bind, args.port);
            eprintln!("  ({e})");
            std::process::exit(1);
        }
    };
    eprintln!(
        "network-status listening on {}:{} (router: {}, wan: {}, interval: {}s)",
        args.bind, args.port, args.router, args.wan, interval_secs
    );

    // Sampler thread
    {
        let state = state.clone();
        let args = args.clone();
        thread::spawn(move || {
            let mut prev: Option<Snapshot> = None;
            let mut next = Instant::now();
            loop {
                let snap = sample_once(&args, &prev);
                let mut h = state.history.lock().unwrap();
                let mut rates = BTreeMap::new();
                for i in &snap.ifaces {
                    rates.insert(i.name.clone(), (i.up_bps, i.down_bps));
                }
                h.push_back(HistoryPoint {
                    ts: snap.ts,
                    rates,
                    pf: snap.pf_states,
                    load1: snap.load.map(|l| l[0]),
                });
                while h.len() > MAX_HISTORY {
                    h.pop_front();
                }
                drop(h);
                state.last.lock().unwrap().replace(snap.clone());
                prev = Some(snap);

                next += args.interval;
                let now = Instant::now();
                if now < next {
                    thread::sleep(next - now);
                } else {
                    next = now;
                }
            }
        });
    }

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state = state.clone();
                let args = args.clone();
                thread::spawn(move || handle_client(stream, &state, &args));
            }
            Err(_) => continue,
        }
    }
}
