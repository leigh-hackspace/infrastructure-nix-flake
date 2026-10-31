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
//!   GET  /               SPA shell (the embedded index.html), or a minimal
//!                        "not built yet" page if there is no bundle
//!   GET  /assets/{name}  the embedded SPA bundle (index.html, the wasm, its glue
//!                        module), with SPA fallback to index.html for unknown
//!                        client-side routes
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
//! through the `ssh` binary, JSON is hand-rolled, and the frontend is a Dioxus SPA
//! compiled to wasm and **embedded in this binary** by `build.rs`
//! (`common-rs/build-spa`, `common/frontend-build-spa.sh`) — served as bytes, so the
//! `.wasm` is not decoded as text. It used to be a prebuilt SolidJS SPA served from
//! `--static-dir`, which is why the response path here took a `String` body; that flag is
//! gone. The request/response plumbing is the shared in-repo `common-rs/web`.
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
//!   * `serve` — the embedded SPA bundle, its MIME types, and the page shown when
//!     nothing has been built
//!   * `json` — the handful of JSON output primitives this crate hand-rolls
//!
//! `main.rs` is left with the argument contract, the routing table and the
//! sampler loop, which is the part that only makes sense next to each other.

mod conntrack;
mod flows;
mod json;
mod leases;
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
use serve::{page, serve_asset};
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
    /// Serve `/api/leases` at all.  Off when there is no `--leases-file`, and
    /// with `--no-leases` for a probe target whose lease file is not this
    /// dashboard's to read — the same switch `--no-conns` is, and for the same
    /// reason: an endpoint that answers an unreadable source with an empty table
    /// looks like a network with no DHCP clients, which is a conclusion an
    /// operator would act on.
    leases: bool,
    /// How long a lease this router hands out: `leaseTtl` in
    /// `machines/gw/lib/network-tables.nix`.  dnsmasq's lease file records an
    /// expiry and no length, so this is what turns one into "how much is left" —
    /// and a lease in a block we were not told about is marked `known: false`
    /// rather than divided by a number that does not describe it.
    lease_ttl: i64,
    /// Most rows `/api/leases` will return.
    lease_max_rows: usize,
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
) -> common_web::Reply {
    match (method, path) {
        ("GET", "/api/config") => common_web::Reply::json(
            200,
            &format!(
                r#"{{"wan":{},"title":{},"interval":{},"connections":{{"enabled":{},"min_age":{},"min_bytes":{},"max_rows":{},"stale":{},"local_prefixes":{}}},"leases":{{"enabled":{},"ttl":{},"max_rows":{},"vlans":{}}}}}"#,
                quoted(&args.wan),
                quoted(&args.title),
                args.interval.as_secs(),
                args.conns,
                args.conn_min_age,
                args.conn_min_bytes,
                args.conn_max_rows,
                args.conn_stale,
                local_prefixes_json(args),
                args.leases,
                args.lease_ttl,
                args.lease_max_rows,
                json_strings(&args.local_nets.vlan_labels()),
            ),
        ),
        ("GET", "/api/snapshot") => {
            let g = state.last.lock().unwrap();
            match g.as_ref() {
                Some(s) => common_web::Reply::json(200, &snapshot_json(s, args)),
                None => common_web::Reply::json(
                    503,
                    r#"{"error":"no data yet (first router probe in progress)"}"#,
                ),
            }
        }
        ("GET", "/metrics") => {
            let g = state.last.lock().unwrap();
            let ctype = "text/plain; version=0.0.4; charset=utf-8";
            match g.as_ref() {
                Some(s) => common_web::Reply::new(
                    200,
                    ctype,
                    metrics_with_conns(s, state, args).into_bytes(),
                ),
                None => common_web::Reply::new(
                    200,
                    ctype,
                    "no data yet (first router probe in progress)"
                        .as_bytes()
                        .to_vec(),
                ),
            }
        }
        ("GET", "/api/history") => {
            let g = state.history.lock().unwrap();
            let points: Vec<&HistoryPoint> = g.iter().collect();
            common_web::Reply::json(200, &history_json(&points))
        }
        ("GET", "/api/connections") => {
            if !args.conns {
                return common_web::Reply::json(
                    404,
                    r#"{"error":"connection table disabled (started with --no-conns)"}"#,
                );
            }
            let defaults = conn_query(args);
            let q = flows::parse_query(query, &defaults);
            let g = state.flows.lock().unwrap();
            common_web::Reply::json(200, &g.json(&q, &defaults, args.interval.as_secs()))
        }
        ("GET", "/api/leases") => {
            // Three ways this can be off, each said out loud rather than answered
            // with an empty table: the operator disabled it, no lease file was
            // configured, or the file exists and this process may not read it.
            if !args.leases {
                return common_web::Reply::json(
                    404,
                    r#"{"error":"lease table disabled (started with --no-leases)"}"#,
                );
            }
            let Some(path) = args.leases_file.as_deref() else {
                return common_web::Reply::json(
                    404,
                    r#"{"error":"lease table disabled (no --leases-file)"}"#,
                );
            };
            let defaults = lease_query(args);
            let q = leases::parse(query, &defaults);
            match leases::json(
                path,
                &q,
                &defaults,
                probe::now_ts(),
                &args.local_nets,
                args.lease_ttl,
                args.lease_max_rows,
            ) {
                Ok(body) => common_web::Reply::json(200, &body),
                // The file is root-only unless `machines/gw/lease-reader.nix`
                // has put this account in its group; a 503 with the OS's own
                // words is the receipt for that, not a table that reads empty.
                Err(e) => common_web::Reply::json(503, &format!(r#"{{"error":{}}}"#, quoted(&e))),
            }
        }
        // The SPA. `/assets/…` is the embedded bundle; anything else is a page route,
        // which a single-page app answers with its own index.html — that is why a bookmark
        // of `/#leases` or a stray `/favicon.ico` still opens the page instead of 404ing.
        ("GET", "/") => page(&args.title),
        ("GET", p) if p.starts_with("/assets/") => match serve_asset(&p[8..]) {
            Some(r) => r,
            None => page(&args.title),
        },
        ("GET", p) => match serve_asset(p.trim_start_matches('/')) {
            Some(r) => r,
            None => page(&args.title),
        },
        _ => common_web::Reply::json(404, r#"{"error":"not found"}"#),
    }
}

/// `["vlan225","vlan226"]` — small helper the config's VLAN list needs.
fn json_strings(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| quoted(s)).collect();
    format!("[{}]", inner.join(","))
}

/// The filter `/api/leases` uses when the request asks for none.
fn lease_query(args: &Args) -> leases::Query {
    leases::Query {
        max_left: -1,
        needle: None,
        vlan: None,
        net: None,
        limit: args.lease_max_rows,
        sort: leases::Sort::Expires,
        reverse: false,
    }
}

/// `args.local` as a JSON array of the prefix strings as given.
fn local_prefixes_json(args: &Args) -> String {
    let inner: Vec<String> = args.local_nets.specs().iter().map(|s| quoted(s)).collect();
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

    // request.path() drops any query string so asset routes resolve by path;
    // /api/connections is the one route that reads the query string, and it
    // reads it off the raw path through the shared parser.
    let query = common_web::query_params(&request.raw_path);
    let reply = route(&request.method, request.path(), &query, state, args);
    // `send`, not `respond`: an asset needs a `Cache-Control` of its own, and the body is
    // bytes because one of them is a wasm binary.
    let _ = reply.send(&mut stream);
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Which prefixes count as "ours" when the operator named none.  Private
/// addressing is the best available guess, and the only reason it is not simply
/// empty is that empty means every connection is classified as neither-end-
/// internal, which looks like a router with no LAN rather than a misconfigured
/// dashboard.
const DEFAULT_LOCAL_PREFIXES: [&str; 4] =
    ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "fc00::/7"];

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
    let mut conns = true;
    let mut local_prefixes: Vec<String> = Vec::new();
    let mut conn_min_age: u64 = 5;
    let mut conn_min_bytes: u64 = 0;
    let mut conn_max_rows: usize = 200;
    let mut conn_stale: i64 = 15;
    let mut hosts_files: Vec<String> = Vec::new();
    let mut leases_file: Option<String> = None;
    let mut no_leases = false;
    let mut lease_ttl: i64 = 86_400;
    let mut lease_max_rows: usize = 500;
    let mut vlans: Vec<String> = Vec::new();
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
            "--no-leases" => no_leases = true,
            // `--vlan vlan226=10.3.2.0/24`: the same block a `--local-prefix`
            // would give, plus the VLAN's name so the lease table can group by
            // it.  Kept in its own list because it is parsed by the same
            // `Localness` and must land in the *same* list, not a second one.
            "--vlan" => {
                if let Some(v) = argv.get(i + 1) {
                    vlans.push(v.clone());
                    i += 1;
                }
            }
            "--lease-ttl" => {
                if let Some(v) = argv.get(i + 1) {
                    lease_ttl = arg_num("--lease-ttl", v, lease_ttl as u64) as i64;
                    i += 1;
                }
            }
            "--lease-max-rows" => {
                if let Some(v) = argv.get(i + 1) {
                    lease_max_rows = arg_num("--lease-max-rows", v, lease_max_rows as u64) as usize;
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
    let local_nets = if local_prefixes.is_empty() && vlans.is_empty() {
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
    // The `--vlan` blocks land in the *same* prefix list as `--local-prefix`: a
    // VLAN is inside the network by definition, so the connection table's
    // direction rules do not change, and `vlan_of` has one list to search
    // rather than a second one that could disagree with it.
    let local_nets = {
        let mut nets = local_nets;
        nets.add_named(&vlans);
        nets
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
        conns,
        local_nets,
        conn_min_age,
        conn_min_bytes,
        conn_max_rows,
        conn_stale,
        hosts_files,
        leases: !no_leases && leases_file.is_some(),
        lease_ttl,
        lease_max_rows,
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
    // The count the binary was built with, not a directory it will go and look at: 1 means
    // build.rs found no bundle, which is what a missed `just build-frontend` looks like from
    // the outside — so say it rather than let the page say it.
    let assets = format!("{} embedded", serve::embedded_count());
    eprintln!(
        "network-status listening on {}:{} (probe: {}, wan: {}, interval: {}s, assets: {}, conns: {})\n  local prefixes: {}\n  names from: {}{}",
        args.bind,
        args.port,
        if args.local {
            "local"
        } else {
            args.router.as_str()
        },
        args.wan,
        interval_secs,
        assets,
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
    eprintln!(
        "  lease table: {} (ttl {}s, max {} rows, vlans: {})",
        match (&args.leases, args.leases_file.as_deref()) {
            (false, _) => "disabled".to_string(),
            (true, Some(p)) => p.to_string(),
            (true, None) => "disabled (no --leases-file)".to_string(),
        },
        args.lease_ttl,
        args.lease_max_rows,
        match args.local_nets.vlan_labels().is_empty() {
            true => "none (--vlan vlan226=10.3.2.0/24)".to_string(),
            false => args.local_nets.vlan_labels().join(" "),
        },
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

    /// A `Reply` body is bytes, because one of the things this server answers is a wasm
    /// binary; the tests read it as text because the assertions are about sentences.
    fn body_of(r: &common_web::Reply) -> String {
        String::from_utf8_lossy(&r.body).to_string()
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

        let r = route("GET", "/api/connections", &empty, &state, &off);
        assert_eq!(r.status, 404, "{}", body_of(&r));
        let r = route("GET", "/api/connections", &empty, &state, &on);
        assert_eq!(r.status, 200);
        let r = route("GET", "/api/snapshot", &empty, &state, &off);
        assert_eq!(
            r.status,
            503,
            "no snapshot before the first probe: {}",
            body_of(&r)
        );

        // The config is what the SPA reads its min-age default out of, so it has
        // to say the same thing the filter will.
        let cfg = body_of(&route("GET", "/api/config", &empty, &state, &on));
        assert!(
            cfg.contains(&format!("\"min_age\":{}", on.conn_min_age)),
            "{cfg}"
        );
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

    #[test]
    fn the_lease_endpoint_follows_the_switch_and_says_which_reason() {
        // Three ways `/api/leases` can be off, and each has a different sentence:
        // the operator disabled it, no file was configured, or the file is there
        // and this process may not read it.  None of them may answer with an empty
        // table, because "no DHCP clients" is a conclusion an operator acts on.
        let state = Arc::new(State::new(&[], None));
        let empty = HashMap::new();

        let mut off = args_with(true);
        off.leases = false;
        off.leases_file = Some("/tmp/does-not-matter".into());
        let r = route("GET", "/api/leases", &empty, &state, &off);
        assert_eq!(r.status, 404, "{}", body_of(&r));
        assert!(body_of(&r).contains("--no-leases"), "{}", body_of(&r));

        let no_file = args_with(true);
        assert!(no_file.leases_file.is_none());
        let r = route("GET", "/api/leases", &empty, &state, &no_file);
        assert_eq!(r.status, 404, "{}", body_of(&r));
        assert!(body_of(&r).contains("no --leases-file"), "{}", body_of(&r));

        let unreadable = {
            let mut a = args_with(true);
            a.leases_file = Some("/nonexistent/dnsmasq.leases".into());
            a
        };
        let r = route("GET", "/api/leases", &empty, &state, &unreadable);
        assert_eq!(
            r.status,
            503,
            "an unreadable file is an error, not a table: {}",
            body_of(&r)
        );
        assert!(
            body_of(&r).contains("dnsmasq.leases"),
            "the OS's own reason is the receipt: {}",
            body_of(&r)
        );
    }

    #[test]
    fn the_lease_table_renders_rows_and_ignores_dnsmasqs_header() {
        // The live file was measured with 49 lines of which one was dnsmasq's own
        // `duid` header; that header must be counted as skipped and not become a
        // row, and a lease inside a labelled block must come back `known` so the
        // page can draw a bar it can justify.
        let path = std::env::temp_dir().join("network-status-main-leases-test");
        std::fs::write(
            &path,
            concat!(
                "duid 00:01:00:01:32:6b:9c:78:00:25:90:d3:f0:ea\n",
                "1900000000 aa:bb:cc:dd:ee:ff 10.3.2.50 laptop *\n",
                "1900000000 * 10.3.2.0 * *\n",
            ),
        )
        .expect("temp lease file");

        let mut args = args_with(true);
        args.leases_file = Some(path.to_str().expect("utf-8 path").into());
        args.lease_ttl = 86_400;
        // A labelled block, so "this lease's length is the one the router hands
        // out" is a fact the response can state rather than a guess.
        let mut local_nets = Localness::new(&[]);
        local_nets.add_named(&["vlan226=10.3.2.0/24".to_string()]);
        args.local_nets = local_nets;
        let state = Arc::new(State::new(&[], None));

        let mut q = HashMap::new();
        q.insert("sort".to_string(), "name".to_string());
        let r = route("GET", "/api/leases", &q, &state, &args);
        let body = body_of(&r);
        assert_eq!(r.status, 200, "{body}");
        assert_eq!(r.content_type, "application/json");
        assert!(body.contains("\"skipped\":1"), "{body}");
        assert!(body.contains("\"total\":2"), "{body}");
        assert!(body.contains("\"name\":\"laptop\""), "{body}");
        assert!(body.contains("\"known\":true"), "{body}");
        // The `*` hostname is dnsmasq's "client sent none", not the name `*`.
        assert!(!body.contains("\"name\":\"*\""), "{body}");
        assert!(body.contains("\"generated\":true"), "{body}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_asset_name_cannot_escape_and_the_shell_is_not_cacheable() {
        // The response path returns `Reply` rather than a (status, content-type, String)
        // triple for one reason: a `String` cannot hold a wasm binary, and the asset this page
        // loads is one. What is testable without a bundle in this build is the two rules that
        // do not depend on which bundle was embedded.
        let args = args_with(true);
        let state = Arc::new(State::new(&[], None));
        let empty = HashMap::new();

        // An embedded asset has no directory to escape into, so `..` can only fail to
        // match — which is the SPA fallback, not a file outside the bundle.
        assert!(serve_asset("../index.html").is_none());
        let r = route("GET", "/assets/../../etc/passwd", &empty, &state, &args);
        assert_eq!(r.status, 200);
        assert_eq!(
            r.content_type,
            "text/html; charset=utf-8",
            "{}",
            body_of(&r)
        );

        // index.html is the file that names the bundle, so caching it pins a page to a bundle
        // it no longer refers to. The assets are cacheable and the shell is not.
        let shell = serve_asset("index.html").expect("build.rs always writes an index.html entry");
        assert_eq!(shell.content_type, "text/html; charset=utf-8");
        assert!(
            !shell
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("cache-control")),
            "the shell must not be served with a Cache-Control"
        );
    }
}
