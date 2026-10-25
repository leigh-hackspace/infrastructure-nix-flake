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
//!   * the conntrack table itself (`/proc/net/nf_conntrack`), which is what the
//!     per-connection view is built from and by far the largest thing this reads
//!     — a quiet gw prints 140 KB of it, which is why the probe's stdout is
//!     drained on its own thread (see `probe::run_probe`),
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
//!   GET  /api/config     the knobs the SPA reads: wan, title, sample interval,
//!                        and the connection view's defaults
//!   GET  /api/snapshot   latest snapshot as JSON
//!   GET  /api/history    full rolling history as JSON
//!   GET  /api/connections  the tracked connection table, filtered, sorted and
//!                        rolled up per host:
//!                        `?min_age=&min_bytes=&min_bps=&q=&kind=&sort=&limit=`
//!                        (see `flows::Query`)
//!   GET  /metrics        latest snapshot plus the connection table in
//!                        Prometheus text format, scraped by services1's
//!                        Prometheus so Grafana can chart the same data as the
//!                        SPA (see the "router" dashboard in
//!                        machines/services1/services/monitoring-dashboards.nix)
//!
//! Zero external crates (house style, see status-dashboard/): ssh goes
//! through the `ssh` binary, JSON is hand-rolled, and the frontend is a
//! prebuilt SolidJS + TypeScript SPA served as static files (--static-dir).
//! The request/response plumbing is the shared in-repo `common-rs/web`.
//!
//! # Where things are
//!
//! One file per job, none of them 1000 lines (AGENTS.md, "no 1000+ line source
//! files"): the crate used to be one 1300-line `main.rs`, and the connection
//! table would have made it 2000.
//!
//!   * `model` — the snapshot's data, the "potential issues", and the counter
//!     differencing every rate in the program is made from
//!   * `probe` — the shell script, how it is run, and how its output is parsed
//!   * `conntrack` — `/proc/net/nf_conntrack`'s format, and nothing else
//!   * `flows` — the tracker built out of successive conntrack samples, which is
//!     what `/api/connections` serves
//!   * `nets` — the prefixes that count as "ours" (CIDR matching)
//!   * `names` — addresses → names, from the router's DNS records and leases,
//!     plus the port and ICMP-type labels
//!   * `render` — the snapshot/history/metrics text formats
//!   * `serve` — the static SPA files and their path containment
//!   * `json` — the handful of JSON output primitives this crate hand-rolls
//!
//! `main.rs` is left with the argument contract, the routing table and the
//! sampler loop, which is the part that only makes sense next to each other.

mod conntrack;
mod flows;
mod json;
mod model;
mod names;
mod nets;
mod probe;
mod render;
mod serve;

use common_json::quoted;
use flows::Query as FlowQuery;
use model::{HistoryPoint, Snapshot, State};
use nets::Localness;
use probe::{local_hostname, router_host, sample_once, Sample};
use render::{history_json, metrics_text, snapshot_json};
use serve::{serve_static, FALLBACK_HTML};
use std::collections::{BTreeMap, HashMap};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const MAX_HISTORY: usize = 1440; // ~1 hour at the default 5s interval

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
    /// Read `/proc/net/nf_conntrack` as well as the rest, and serve
    /// `/api/connections`.  Off with `--no-conns` for a probe target whose
    /// conntrack table is not this box's (or not there at all).
    conns: bool,
    /// The `--local-prefix` list, parsed: an address outside every one of these
    /// is not one of ours, which is what decides the direction of a flow.
    /// (`local`, beside it, is the *probe* mode — read this box's /proc rather
    /// than ssh'ing to another — and the two are unrelated.)
    local_nets: Localness,
    /// The age filter `/api/connections` defaults to — the page's "show me
    /// things that have lasted more than N seconds", and the knob the SPA can
    /// move without a redeploy.
    conn_min_age: u64,
    /// Hide connections that have moved less than this in total.
    conn_min_bytes: u64,
    /// Most rows `/api/connections` will return, and most it will serialise.
    conn_max_rows: usize,
    /// How long a connection may be absent from the kernel's table and still be
    /// remembered with its original age.
    conn_stale: i64,
    /// Files of `<ip> <name>` pairs, for labelling the addresses.
    hosts_files: Vec<String>,
    /// dnsmasq's lease file, read if this process may open it (on gw it may
    /// not: it is `0640 root:root`), which is what names the DHCP clients that
    /// have no record of their own.
    leases_file: Option<String>,
}

/// The filter `/api/connections` uses when the request asks for none.
fn conn_query(args: &Args) -> FlowQuery {
    FlowQuery {
        min_age: args.conn_min_age,
        min_bytes: args.conn_min_bytes,
        min_bps: 0.0,
        needle: None,
        kind: None,
        sort: flows::Sort::Rate,
        limit: args.conn_max_rows,
    }
}

fn route(
    method: &str,
    path: &str,
    query: &HashMap<String, String>,
    state: &State,
    args: &Args,
) -> (u16, String, String) {
    match (method, path) {
        ("GET", "/api/config") => (
            200,
            "application/json".to_string(),
            format!(
                r#"{{"wan":{},"title":{},"interval":{},"connections":{{"enabled":{},"min_age":{},"min_bytes":{},"max_rows":{},"stale":{},"local_prefixes":{}}}}}"#,
                quoted(&args.wan),
                quoted(&args.title),
                args.interval.as_secs(),
                args.conns,
                args.conn_min_age,
                args.conn_min_bytes,
                args.conn_max_rows,
                args.conn_stale,
                local_prefixes_json(args),
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
                Some(s) => (
                    200,
                    "text/plain; version=0.0.4; charset=utf-8".to_string(),
                    metrics_with_conns(s, state, args),
                ),
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
        ("GET", "/api/connections") => {
            if !args.conns {
                return (
                    404,
                    "application/json".to_string(),
                    r#"{"error":"connection table disabled (started with --no-conns)"}"#.to_string(),
                );
            }
            let defaults = conn_query(args);
            let q = flows::parse_query(query, &defaults);
            let g = state.flows.lock().unwrap();
            (
                200,
                "application/json".to_string(),
                g.json(&q, &defaults, args.interval.as_secs()),
            )
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

/// `args.local` as a JSON array of the prefix strings as given.
fn local_prefixes_json(args: &Args) -> String {
    let inner: Vec<String> = args
        .local_nets
        .specs()
        .iter()
        .map(|s| quoted(s))
        .collect();
    format!("[{}]", inner.join(","))
}

/// The snapshot's metrics plus the connection table's.  Kept out of
/// `metrics_text` so that function stays a pure rendering of one snapshot and
/// its tests need no state; the two halves are exported under separate
/// `router_connection*` names for the same reason.
fn metrics_with_conns(s: &Snapshot, state: &State, args: &Args) -> String {
    let mut out = metrics_text(s);
    if args.conns {
        out.push_str(&state.flows.lock().unwrap().metrics(args.conn_min_age));
    }
    out
}

fn handle_client(mut stream: TcpStream, state: &State, args: &Args) {
    let request = match common_web::read_request(&mut stream) {
        Some(request) => request,
        None => return,
    };

    // request.path() drops any query string so static routes resolve by path;
    // /api/connections is the one route that reads the query string, and it
    // reads it off the raw path through the shared parser.
    let query = common_web::query_params(&request.raw_path);
    let (status, ctype, body) = route(&request.method, request.path(), &query, state, args);
    let _ = common_web::respond(&mut stream, status, &ctype, &body);
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Which prefixes count as "ours" when the operator named none.  Private
/// addressing is the best available guess, and the only reason it is not simply
/// empty is that empty means every connection is classified as neither-end-
/// internal, which looks like a router with no LAN rather than a misconfigured
/// dashboard.
const DEFAULT_LOCAL_PREFIXES: [&str; 4] = ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "fc00::/7"];

/// A numeric flag value, or the current default with a word on stderr.
///
/// The older flags use `str::parse().unwrap_or(default)`, which is silent; for
/// the connection view's knobs a typo is worth a line in the journal, because
/// their symptom is a dashboard that looks healthy and filters wrong.
fn arg_num<T: std::str::FromStr + std::fmt::Display>(name: &str, raw: &str, cur: T) -> T {
    match raw.trim().parse::<T>() {
        Ok(v) => v,
        Err(_) => {
            eprintln!("network-status: ignoring {name} {raw:?}, keeping {cur}");
            cur
        }
    }
}

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
    let mut conns = true;
    let mut local_prefixes: Vec<String> = Vec::new();
    let mut conn_min_age: u64 = 5;
    let mut conn_min_bytes: u64 = 0;
    let mut conn_max_rows: usize = 200;
    let mut conn_stale: i64 = 15;
    let mut hosts_files: Vec<String> = Vec::new();
    let mut leases_file: Option<String> = None;
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
            "--no-conns" => conns = false,
            "--local-prefix" => {
                if let Some(v) = argv.get(i + 1) {
                    local_prefixes.push(v.clone());
                    i += 1;
                }
            }
            "--hosts-file" => {
                if let Some(v) = argv.get(i + 1) {
                    hosts_files.push(v.clone());
                    i += 1;
                }
            }
            "--leases-file" => {
                if let Some(v) = argv.get(i + 1) {
                    leases_file = Some(v.clone());
                    i += 1;
                }
            }
            "--conn-min-age" => {
                if let Some(v) = argv.get(i + 1) {
                    conn_min_age = arg_num("--conn-min-age", v, conn_min_age);
                    i += 1;
                }
            }
            "--conn-min-bytes" => {
                if let Some(v) = argv.get(i + 1) {
                    conn_min_bytes = arg_num("--conn-min-bytes", v, conn_min_bytes);
                    i += 1;
                }
            }
            "--conn-max-rows" => {
                if let Some(v) = argv.get(i + 1) {
                    conn_max_rows = arg_num("--conn-max-rows", v, conn_max_rows as u64) as usize;
                    i += 1;
                }
            }
            "--conn-stale" => {
                if let Some(v) = argv.get(i + 1) {
                    conn_stale = arg_num("--conn-stale", v, conn_stale as u64) as i64;
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

    // Which addresses count as ours.  With no `--local-prefix` the connection
    // table would classify every flow as neither-end-internal, which is a quiet
    // way to be wrong, so the fallback is the private address space and the
    // startup line says which of the two is in force.
    let local_nets = if local_prefixes.is_empty() {
        eprintln!(
            "network-status: no --local-prefix given, guessing with the private ranges ({DEFAULT_LOCAL_PREFIXES:?})"
        );
        let guess: Vec<String> = DEFAULT_LOCAL_PREFIXES
            .iter()
            .map(|s| s.to_string())
            .collect();
        Localness::new(&guess)
    } else {
        Localness::new(&local_prefixes)
    };
    // `/etc/hosts` is the default name source because every Linux box has one;
    // the router adds its own DNS records file (see machines/gw/network-status.nix).
    if hosts_files.is_empty() {
        hosts_files.push("/etc/hosts".to_string());
    }

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
        conns,
        local_nets,
        conn_min_age,
        conn_min_bytes,
        conn_max_rows,
        conn_stale,
        hosts_files,
        leases_file,
    };
    let state = Arc::new(State::new(&args.hosts_files, args.leases_file.clone()));

    let listener = match TcpListener::bind((args.bind.as_str(), args.port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("network-status: failed to bind {}:{}", args.bind, args.port);
            eprintln!("  ({e})");
            std::process::exit(1);
        }
    };
    eprintln!(
        "network-status listening on {}:{} (probe: {}, wan: {}, interval: {}s, static_dir: {}, conns: {})\n  local prefixes: {}\n  names from: {}{}",
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
        if args.conns {
            format!(
                "/proc/net/nf_conntrack, min age {}s, max {} rows, stale {}s",
                args.conn_min_age, args.conn_max_rows, args.conn_stale
            )
        } else {
            "disabled (--no-conns)".to_string()
        },
        args.local_nets.specs().join(" "),
        args.hosts_files.join(" "),
        args.leases_file
            .as_deref()
            .map(|p| format!(" + {p}"))
            .unwrap_or_default(),
    );

    // Sampler thread
    {
        let state = state.clone();
        let args = args.clone();
        thread::spawn(move || {
            let mut prev: Option<Snapshot> = None;
            let mut next = Instant::now();
            loop {
                let Sample { snap, conns } = sample_once(&args, &prev);
                // The connection table is folded in before the snapshot is
                // published, so a client that reads /api/snapshot and then
                // /api/connections can never see a table from before the
                // snapshot it just read.
                if let Some(raw) = conns {
                    state.flows.lock().unwrap().update(
                        snap.ts,
                        &raw,
                        &args.local_nets,
                        args.conn_stale,
                    );
                }
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
    use model::tests::test_args;

    fn args_with(conns: bool) -> Args {
        let mut args = test_args();
        args.conns = conns;
        args
    }

    #[test]
    fn the_connection_endpoints_follow_the_switch() {
        // `--no-conns` is for a probe target with no conntrack table of its own;
        // it must not answer the endpoint with an empty table that looks like a
        // quiet network, and it must not lose the rest of the API either.
        let state = Arc::new(State::new(&[], None));
        let off = args_with(false);
        let on = args_with(true);
        let empty = HashMap::new();

        let (status, _, body) = route("GET", "/api/connections", &empty, &state, &off);
        assert_eq!(status, 404, "{body}");
        let (status, _, _) = route("GET", "/api/connections", &empty, &state, &on);
        assert_eq!(status, 200);
        let (status, _, body) = route("GET", "/api/snapshot", &empty, &state, &off);
        assert_eq!(status, 503, "no snapshot before the first probe: {body}");

        // The config is what the SPA reads its min-age default out of, so it has
        // to say the same thing the filter will.
        let (_, _, cfg) = route("GET", "/api/config", &empty, &state, &on);
        assert!(cfg.contains(&format!("\"min_age\":{}", on.conn_min_age)), "{cfg}");
        assert!(cfg.contains("\"enabled\":true"), "{cfg}");
        assert!(cfg.contains("\"interval\":5"), "{cfg}");
    }

    #[test]
    fn numeric_flags_say_when_they_are_ignored() {
        // A silent `unwrap_or(default)` here is a dashboard that filters wrong
        // and looks healthy; the old flags do it and the new knobs do not.
        assert_eq!(arg_num("--conn-min-age", "30", 5u64), 30);
        assert_eq!(arg_num("--conn-min-age", "soon", 5u64), 5);
        assert_eq!(arg_num("--conn-max-rows", " 40 ", 200usize), 40);
    }
}
