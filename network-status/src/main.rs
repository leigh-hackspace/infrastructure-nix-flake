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
//!   GET  /               SPA shell (index.html) from --static-dir, or a
//!                        minimal embedded page if none is configured
//!   GET  /{asset}        static SPA assets (bundle.js, bundle.css, ...) from
//!                        --static-dir, with SPA fallback to index.html for
//!                        unknown client-side routes
//!   GET  /api/config     {"wan":...,"title":...} consumed by the SPA
//!   GET  /api/snapshot   latest snapshot as JSON
//!   GET  /api/history    full rolling history as JSON
//!
//! Zero external crates (house style, see status-dashboard/): ssh goes
//! through the `ssh` binary, JSON is hand-rolled, and the frontend is a
//! prebuilt SolidJS + TypeScript SPA served as static files (--static-dir).

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
    /// Directory of pre-built static SPA files (SolidJS/Vite build). When set,
    /// the binary serves those files; when None a minimal embedded page is
    /// returned so the backend still runs standalone (e.g. local dev).
    static_dir: Option<String>,
}

fn route(method: &str, path: &str, state: &State, args: &Args) -> (u16, String, String) {
    match (method, path) {
        ("GET", "/api/config") => (
            200,
            "application/json".to_string(),
            format!(
                r#"{{"wan":{},"title":{}}}"#,
                json_str(&args.wan),
                json_str(&args.title)
            ),
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
        // Anything else is a SPA route: serve the static asset if present,
        // otherwise fall back to index.html (client-side routing).
        ("GET", _) => match serve_static(&args.static_dir, path) {
            Some(r) => r,
            None => (200, "text/html; charset=utf-8".to_string(), FALLBACK_HTML.replace("__TITLE__", &args.title)),
        },
        _ => (404, "application/json".to_string(), r#"{"error":"not found"}"#.to_string()),
    }
}

const FALLBACK_HTML: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>__TITLE__</title></head>
<body style="font:16px system-ui;background:#0d1117;color:#d7e0e8;padding:40px">
<h1>__TITLE__</h1>
<p>The frontend has not been built yet. Build it and point <code>--static-dir</code> at the output:</p>
<pre>cd network-status/frontend &amp;&amp; npm install &amp;&amp; npm run build</pre>
<p>The dashboard will appear once the static files are served.</p>
</body>
</html>
"#;

/// MIME type for a URL path (best-effort, by extension).
fn static_content_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "map" | "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        _ => "application/octet-stream",
    }
}

/// Join `path` onto `dir`, refusing any `..` that would escape the directory.
fn within_dir(dir: &std::path::Path, path: &str) -> Option<std::path::PathBuf> {
    let mut out = std::path::PathBuf::from(dir);
    for comp in path.split(['/', '\\']) {
        match comp {
            "" | "." => {}
            ".." => return None,
            c => out.push(c),
        }
    }
    Some(out)
}

/// Serve a path from `static_dir`, with SPA fallback to index.html.
/// Returns `None` when no static dir is configured.
fn serve_static(static_dir: &Option<String>, path: &str) -> Option<(u16, String, String)> {
    let dir = std::path::Path::new(static_dir.as_ref()?);
    // Drop any query string.
    let clean = path.split('?').next().unwrap_or("/");
    let mut file = within_dir(dir, clean)?;
    if file.is_dir() {
        file.push("index.html");
    }
    match std::fs::read(&file) {
        Ok(bytes) => Some((
            200,
            static_content_type(&file.to_string_lossy()).to_string(),
            String::from_utf8_lossy(&bytes).into_owned(),
        )),
        Err(_) => {
            // SPA fallback: hand back index.html for unrecognised routes.
            match std::fs::read(dir.join("index.html")) {
                Ok(bytes) => Some((
                    200,
                    "text/html; charset=utf-8".to_string(),
                    String::from_utf8_lossy(&bytes).into_owned(),
                )),
                Err(_) => None,
            }
        }
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
    // Drop any query string so static routes resolve by path only.
    let path = parts.next().unwrap_or("").split('?').next().unwrap_or("");

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
    let mut static_dir: Option<String> = None;
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
            "--static-dir" => {
                if let Some(v) = argv.get(i + 1) {
                    static_dir = Some(v.clone());
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
        static_dir,
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
        "network-status listening on {}:{} (router: {}, wan: {}, interval: {}s, static_dir: {})",
        args.bind,
        args.port,
        args.router,
        args.wan,
        interval_secs,
        args.static_dir.as_deref().unwrap_or("<embedded>"),
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
