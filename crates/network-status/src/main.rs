//! network-status — real-time web dashboard for the router `gw` (10.3.1.1).
//!
//! It runs on gw itself (`--local`), where every few seconds it runs a small
//! `sh -s` script that reads the Linux interfaces the router's statistics
//! actually live in:
//!
//!   * per-interface byte/error counters (`/proc/net/dev`),
//!   * load / uptime / CPU (`/proc/loadavg`, `/proc/uptime`, `/proc/stat`),
//!   * memory (`/proc/meminfo`),
//!   * the nftables conntrack table size
//!     (`/proc/sys/net/netfilter/nf_conntrack_count`),
//!   * cumulative TCP retransmissions (`/proc/net/snmp`),
//!   * per-interface link status (`/sys/class/net/*/carrier`),
//!   * the router's own TCP socket count (`/proc/net/tcp`, `/proc/net/tcp6`),
//!   * the CPU count (`nproc`).
//!
//! It used to speak OPNsense (`netstat -ibn`, `pfctl`, `ifconfig`, `top -b` over
//! a tcsh root shell) and it can still probe another box over ssh (`--router
//! leigh-admin@host --ssh-key … --known-hosts …`, the mode services1 used until
//! 2026-10-21). The local mode is the production one: `machines/gw/network-status.nix`
//! runs it as a DynamicUser on gw, so there is no login, key or host-key pin.
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
//!   GET  /metrics        latest snapshot in Prometheus text format, scraped
//!                        by services1's Prometheus so Grafana can chart the
//!                        same data as the SPA (see the "router" dashboard in
//!                        machines/services1/services/monitoring-dashboards.nix)
//!
//! Zero external crates (house style, see status-dashboard/): ssh goes
//! through the `ssh` binary, JSON is hand-rolled, and the frontend is a
//! prebuilt SolidJS + TypeScript SPA served as static files (--static-dir).
//! The request/response plumbing is the shared in-repo `common-rs/web`.

use common_json::quoted;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SSH_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_HISTORY: usize = 1440; // ~1 hour at the default 5s interval

/// Collection script, fed to `ssh ... 'sh -s'` on the router.  gw is NixOS, so
/// this is POSIX sh reading `/proc` and `/sys` — no platform commands, no root,
/// and no sudo: every file here is world-readable.
const REMOTE_SCRIPT: &str = r#"
echo ==IFACES==
cat /proc/net/dev
echo ==STAT==
head -1 /proc/stat
echo ==MEM==
grep -E '^(MemTotal|MemFree|MemAvailable):' /proc/meminfo
echo ==LOAD==
cat /proc/loadavg
echo ==UPTIME==
cat /proc/uptime
echo ==CONNTRACK==
cat /proc/sys/net/netfilter/nf_conntrack_count 2>/dev/null || true
echo ==RETRANS==
awk '/^Tcp:/ {n++; if (n==2) {print $13; exit}}' /proc/net/snmp
echo ==LINKS==
for d in /sys/class/net/*; do
  n=${d##*/}
  c=$(cat "$d/carrier" 2>/dev/null || echo 1)
  echo "$n $c"
done
echo ==SOCKETS==
awk 'FNR>1 {n++} END {print n+0}' /proc/net/tcp /proc/net/tcp6
echo ==NCPU==
nproc
"#;

/// Loopback is never interesting.  ppp0 must not be filtered out: its counters
/// only exist while the tunnel does, and its disappearance is the WAN-down
/// signal.
const IGNORED_IFACES: [&str; 1] = ["lo"];

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
    /// Number of CPUs (`nproc`), used for the load-per-CPU issue.  The field
    /// was called `nprocs` when OPNsense's `top` supplied a process count.
    ncpu: Option<u32>,
    cpu_user: Option<f64>,
    cpu_sys: Option<f64>,
    cpu_idle: Option<f64>,
    /// Free memory as the kernel's `MemAvailable`; `total` is `MemTotal`.
    mem_free: Option<u64>,
    mem_total: Option<u64>,
    /// Entries in the nftables conntrack table (OPNsense called this the pf
    /// state table; the number means the same thing).
    conntrack: Option<u64>,
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
    /// conntrack table size (OPNsense's pf state count).
    conntrack: Option<u64>,
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

/// Feed `REMOTE_SCRIPT` to a command's stdin and collect its stdout.  Used for
/// both probe modes: `ssh ... sh -s` to another box, or `sh -s` on this one.
fn run_probe(mut command: Command, what: &str, hint: &str) -> Result<String, String> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn {what}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(REMOTE_SCRIPT.as_bytes()) {
            let _ = child.kill();
            return Err(format!("failed to write probe script to {what}: {e}"));
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
                return Err(format!("empty output from {what} probe"));
            }
            return Err(format!("{what} failed{hint}"));
        }
        if start.elapsed() > SSH_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{what} probe timed out"));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Run the collection script on another box over ssh; returns its stdout.
fn fetch_router(ssh_key: &str, known_hosts: &str, router: &str) -> Result<String, String> {
    let mut command = Command::new("ssh");
    command
        .arg("-i")
        .arg(ssh_key)
        .args(["-o", "BatchMode=yes"])
        .args(["-o", "ConnectTimeout=10"])
        // Host identity is pinned, not TOFU: the probe ssh's to the router as
        // leigh-admin, and `accept-new` would silently re-trust a key after a
        // router reinstall (or an on-path impersonation).  Nothing ships the pin
        // any more — it used to be /etc/dns-sync/known_hosts, written by a tool
        // that edited DNS over ssh and is gone (see common/int-dns.nix) — so the
        // path below is generated by `just router-known-hosts` and kept by hand.
        // A missing file is no ssh rather than an unverified connection, which is
        // the right default for a mode nobody runs in production.
        .args([
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            &format!("UserKnownHostsFile={known_hosts}"),
        ])
        .arg(router)
        .arg("sh -s");
    run_probe(command, "ssh to router", " (check key/ssh access)")
}

/// Run the collection script on this machine.  This is the mode gw uses: the
/// dashboard runs on the router, so the statistics are local and there is no
/// ssh key, no host-key pin and no service account to keep.
fn fetch_local() -> Result<String, String> {
    let mut command = Command::new("sh");
    command.arg("-s");
    run_probe(command, "local", "")
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
    ncpu: Option<u32>,
    cpu_user: Option<f64>,
    cpu_sys: Option<f64>,
    cpu_idle: Option<f64>,
    mem_free: Option<u64>,
    mem_total: Option<u64>,
    conntrack: Option<u64>,
    own_tcp: Option<u64>,
    retrans_total: Option<u64>,
}

/// Parse a `/proc/meminfo` value (kB) into bytes.
fn parse_meminfo_kb(s: &str) -> Option<u64> {
    let kb: u64 = s.split_whitespace().next()?.parse().ok()?;
    Some(kb * 1024)
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
                // "name: rx_bytes rx_packets rx_errs rx_drop ... tx_bytes tx_packets tx_errs ..."
                let Some((name, rest)) = t.split_once(':') else {
                    continue; // the two header lines
                };
                let name = name.trim();
                if name.is_empty() || IGNORED_IFACES.contains(&name) {
                    continue;
                }
                let toks: Vec<u64> = rest
                    .split_whitespace()
                    .filter_map(|v| v.parse().ok())
                    .collect();
                if toks.len() < 16 {
                    continue;
                }
                // rest index 0 = rx bytes: rx errs 2, tx bytes 8, tx errs 10.
                p.ifaces
                    .entry(name.to_string())
                    .or_insert((toks[0], toks[8], toks[2], toks[10]));
            }
            "STAT" => {
                // "cpu user nice system idle iowait irq softirq steal ..." —
                // cumulative since boot, so these percentages are the since-boot
                // averages `top -b`'s CPU line used to report on OPNsense.
                let toks: Vec<u64> = t
                    .split_whitespace()
                    .skip(1)
                    .filter_map(|v| v.parse().ok())
                    .collect();
                if toks.len() >= 7 {
                    let (user, nice, system) = (toks[0], toks[1], toks[2]);
                    let (idle, iowait, irq, softirq) = (toks[3], toks[4], toks[5], toks[6]);
                    let steal = toks.get(7).copied().unwrap_or(0);
                    let total_f = (user + nice + system + idle + iowait + irq + softirq + steal) as f64;
                    if total_f > 0.0 {
                        p.cpu_user = Some((user + nice) as f64 * 100.0 / total_f);
                        p.cpu_sys =
                            Some((system + irq + softirq + steal) as f64 * 100.0 / total_f);
                        p.cpu_idle = Some((idle + iowait) as f64 * 100.0 / total_f);
                    }
                }
            }
            "MEM" => {
                // "MemTotal: 8104932 kB" / "MemAvailable: ..."
                let (key, value) = match t.split_once(':') {
                    Some((k, v)) => (k.trim(), v),
                    None => continue,
                };
                match key {
                    "MemTotal" => p.mem_total = parse_meminfo_kb(value),
                    // `MemAvailable` is the kernel's own estimate of what a new
                    // workload could claim (it accounts for reclaimable cache);
                    // `MemFree` is the fallback on a kernel without it.
                    "MemAvailable" => p.mem_free = parse_meminfo_kb(value),
                    "MemFree" if p.mem_free.is_none() => p.mem_free = parse_meminfo_kb(value),
                    _ => {}
                }
            }
            "LOAD" => {
                // "1.04 0.39 0.22 1/233 33386"
                let loads: Vec<f64> = t
                    .split_whitespace()
                    .take(3)
                    .filter_map(|v| v.parse().ok())
                    .collect();
                if loads.len() == 3 {
                    p.load = Some([loads[0], loads[1], loads[2]]);
                }
            }
            "UPTIME" => {
                // "11736.64 41971.75" — seconds since boot, then idle.
                if let Some(first) = t.split_whitespace().next() {
                    p.uptime_secs = first.parse::<f64>().ok().map(|v| v as u64);
                }
            }
            "CONNTRACK" => {
                if let Some(first) = t.split_whitespace().next() {
                    p.conntrack = first.parse().ok();
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
                // "<iface> <carrier>" from /sys/class/net/*/carrier.  A carrier
                // file that does not exist (ppp0, tailscale0) means the device
                // exists and is up, so the script prints 1 for it.
                let mut it = t.split_whitespace();
                if let (Some(name), Some(carrier)) = (it.next(), it.next()) {
                    p.links.insert(name.to_string(), carrier != "0");
                }
            }
            "SOCKETS" => {
                if let Ok(n) = t.parse::<u64>() {
                    p.own_tcp = Some(n);
                }
            }
            "NCPU" => {
                if let Ok(n) = t.parse::<u32>() {
                    p.ncpu = Some(n);
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

    // WAN link down is the big one.  On gw the WAN is `ppp0`, and pppd deletes
    // the interface when the tunnel drops — so "no entry" is the down signal,
    // not just "entry with carrier 0".
    if snap.ok {
        match snap.ifaces.iter().find(|i| i.name == args.wan) {
            Some(wan) if !wan.active => snap.issues.push(Issue {
                level: "bad".into(),
                message: format!("WAN interface {} has no link", wan.name),
            }),
            None => snap.issues.push(Issue {
                level: "bad".into(),
                message: format!("WAN interface {} is not present", args.wan),
            }),
            _ => {}
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
    if let (Some(load), Some(n)) = (snap.load, snap.ncpu) {
        if load[0] > f64::from(n) {
            snap.issues.push(Issue {
                level: "warn".into(),
                message: format!("load {:.2} above {} CPUs (1 min)", load[0], n),
            });
        }
    }

    // Memory pressure: < 10% free.
    if let (Some(free), Some(total)) = (snap.mem_free, snap.mem_total) {
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
    let probe = if args.local {
        fetch_local()
    } else {
        fetch_router(&args.ssh_key, &args.known_hosts, &args.router)
    };
    match probe {
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
                ncpu: parsed.ncpu,
                cpu_user: parsed.cpu_user,
                cpu_sys: parsed.cpu_sys,
                cpu_idle: parsed.cpu_idle,
                mem_free: parsed.mem_free,
                mem_total: parsed.mem_total,
                conntrack: parsed.conntrack,
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

/// `leigh-admin@10.3.1.1` -> `10.3.1.1`.
fn router_host(router: &str) -> &str {
    router.rsplit('@').next().unwrap_or(router)
}

/// The hostname reported in local mode (`gw`).
fn local_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "router".into())
}

/// Latest snapshot in Prometheus text format. Cumulative counters (interface
/// bytes/errors, retransmissions) are exported as `_total` counters so
/// `rate()` works in PromQL; a router reboot resets them, which `rate()`
/// handles. The current per-interface byte rates are recomputed by
/// Prometheus from those counters, so they are not exported separately.
fn metrics_text(s: &Snapshot) -> String {
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

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Args {
    bind: String,
    port: u16,
    /// ssh target for the remote probe (`--router`); unused with `--local`.
    router: String,
    ssh_key: String,
    /// Pinned router host keys (`UserKnownHostsFile`). See fetch_router.
    known_hosts: String,
    /// Read the statistics on this machine instead of over ssh.  This is how gw
    /// hosts the dashboard.
    local: bool,
    /// What the JSON calls the router: the ssh target's host, or this box's
    /// hostname in local mode.
    hostname: String,
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
                quoted(&args.wan),
                quoted(&args.title)
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
        ("GET", "/metrics") => {
            let g = state.last.lock().unwrap();
            match g.as_ref() {
                Some(s) => (200, "text/plain; version=0.0.4; charset=utf-8".to_string(), metrics_text(s)),
                None => (
                    503,
                    "text/plain; version=0.0.4; charset=utf-8".to_string(),
                    "no data yet (first router probe in progress)".to_string(),
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
    let request = match common_web::read_request(&mut stream) {
        Some(request) => request,
        None => return,
    };

    // request.path() drops any query string so static routes resolve by path.
    let (status, ctype, body) = route(&request.method, request.path(), state, args);
    let _ = common_web::respond(&mut stream, status, &ctype, &body);
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut bind = "127.0.0.1".to_string();
    let mut port: u16 = 8091;
    let mut router = "leigh-admin@10.3.1.1".to_string();
    let mut ssh_key = "/home/leigh-admin/.ssh/agent-hop-key".to_string();
    let mut known_hosts = "/etc/network-status/known_hosts".to_string();
    let mut local = false;
    let mut interval_secs: u64 = 5;
    let mut wan = "ppp0".to_string();
    let mut title = "network-info".to_string();
    let mut static_dir: Option<String> = None;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--local" => local = true,
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
            "--known-hosts" => {
                if let Some(v) = argv.get(i + 1) {
                    known_hosts = v.clone();
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

    // The JSON's `router.hostname`: the box being charted.  In remote mode it
    // is the ssh target without its user; in local mode it is this machine.
    let hostname = if local {
        local_hostname()
    } else {
        router_host(&router).to_string()
    };

    let args = Args {
        bind,
        port,
        router,
        ssh_key,
        known_hosts,
        local,
        hostname,
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
        "network-status listening on {}:{} (probe: {}, wan: {}, interval: {}s, static_dir: {})",
        args.bind,
        args.port,
        if args.local {
            "local"
        } else {
            args.router.as_str()
        },
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
                    conntrack: snap.conntrack,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A real probe from gw (2026-10-21), including the two `/proc/net/dev`
    /// header lines and an interface (`lo`) the parser must drop.
    const SAMPLE: &str = r#"
==IFACES==
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 1735563   13584    0    0    0     0          0         0  1735563   13584    0    0    0     0       0          0
router-lan: 108335552750 106929170    0 5128    0     0          0     60252 285073487745 203019379    0    3    0     0       0          0
vlan225: 78197913655 14990952    0   60    0     0          0     30766 31484006958 23653606    0   20    0     0       0          0
  ppp0: 41152642209 29457831    0    0    0     0          0         0 927347146 7107417    0    0    0     0       0          0
==STAT==
cpu  275198 5 32126 4197173 4648 20764 154326 0 0 0
==MEM==
MemTotal:        8104932 kB
MemAvailable:    6843124 kB
==LOAD==
1.04 0.39 0.22 1/233 33386
==UPTIME==
11736.64 41971.75
==CONNTRACK==
2372
==RETRANS==
3098
==LINKS==
lo 1
ppp0 1
router-lan 1
vlan225 1
==SOCKETS==
32
==NCPU==
4
"#;

    #[test]
    fn parses_gw_probe() {
        let p = parse_output(SAMPLE);

        // lo is dropped; rx bytes / tx bytes / rx errs / tx errs are at the
        // right offsets in /proc/net/dev.
        assert!(!p.ifaces.contains_key("lo"));
        assert_eq!(p.ifaces.get("ppp0"), Some(&(41152642209, 927347146, 0, 0)));
        assert_eq!(p.ifaces.get("vlan225"), Some(&(78197913655, 31484006958, 0, 0)));

        assert_eq!(p.uptime_secs, Some(11736));
        assert_eq!(p.load, Some([1.04, 0.39, 0.22]));
        assert_eq!(p.ncpu, Some(4));
        assert_eq!(p.mem_total, Some(8104932 * 1024));
        assert_eq!(p.mem_free, Some(6843124 * 1024));
        assert_eq!(p.conntrack, Some(2372));
        assert_eq!(p.retrans_total, Some(3098));
        assert_eq!(p.own_tcp, Some(32));
        assert_eq!(p.links.get("ppp0"), Some(&true));

        // The three CPU shares come from the same cumulative counters and add
        // up to 100% (steal is folded into system).
        let sum = p.cpu_user.unwrap() + p.cpu_sys.unwrap() + p.cpu_idle.unwrap();
        assert!((sum - 100.0).abs() < 1e-9, "cpu shares sum to {sum}");
    }

    #[test]
    fn missing_conntrack_file_is_not_an_error() {
        // `cat ... 2>/dev/null || true` leaves the section empty when the
        // nf_conntrack module is not loaded; the rest of the snapshot is fine.
        let p = parse_output("==CONNTRACK==\n\n==NCPU==\n4\n");
        assert_eq!(p.conntrack, None);
        assert_eq!(p.ncpu, Some(4));
    }

    fn test_args() -> Args {
        Args {
            bind: "127.0.0.1".into(),
            port: 0,
            router: "leigh-admin@10.3.1.1".into(),
            ssh_key: String::new(),
            known_hosts: String::new(),
            local: true,
            hostname: "gw".into(),
            interval: Duration::from_secs(5),
            wan: "ppp0".into(),
            title: "network-info".into(),
            static_dir: None,
        }
    }

    fn iface(name: &str, active: bool) -> Iface {
        Iface {
            name: name.into(),
            active,
            ibytes: 0,
            obytes: 0,
            ierrors: 0,
            oerrors: 0,
            down_bps: None,
            up_bps: None,
            err_delta: 0,
        }
    }

    #[test]
    fn missing_wan_interface_is_reported() {
        let args = test_args();

        // ppp0 gone: pppd deletes the interface when the tunnel drops.
        let mut snap = Snapshot {
            ok: true,
            ifaces: vec![iface("vlan225", true)],
            ..Default::default()
        };
        compute_issues(&args, &mut snap);
        assert!(
            snap.issues.iter().any(|i| i.level == "bad" && i.message.contains("not present")),
            "missing WAN should be an issue: {:?}",
            snap.issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );

        // ppp0 present but no carrier.
        let mut snap = Snapshot {
            ok: true,
            ifaces: vec![iface("ppp0", false)],
            ..Default::default()
        };
        compute_issues(&args, &mut snap);
        assert!(snap.issues.iter().any(|i| i.level == "bad" && i.message.contains("no link")));

        // A failed probe already reports "cannot reach router"; it must not
        // also claim the WAN is missing.
        let mut snap = Snapshot {
            ok: false,
            error: Some("ssh failed".into()),
            ..Default::default()
        };
        compute_issues(&args, &mut snap);
        assert!(!snap.issues.iter().any(|i| i.message.contains("not present")));
    }
}
