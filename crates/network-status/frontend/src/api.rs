//! Types mirroring the JSON produced by the network-status backend, plus the fetch
//! helpers.
//!
//! Field names are the wire contract, read off the emitters in
//! `crates/network-status/src/render.rs`, `flows.rs` and `leases.rs` — not off the old
//! `api.ts`, which carried a `panels` field the backend has never produced. Every field
//! is `#[serde(default)]` because one endpoint answering a `null` must not turn into a
//! failed fetch: a failed fetch keeps the last data and shows an error, and the two are
//! different things.

use serde::Deserialize;
use std::collections::BTreeMap;

use crate::state::{ConnControls, LeaseControls};

/// The error shape every endpoint answers with (`common_json::quoted`). The message is
/// the useful half — which endpoint refused, and why — so it is kept verbatim rather
/// than reduced to a status code.
#[derive(Deserialize, Clone, PartialEq, Debug)]
pub struct ApiError {
    pub error: String,
}

#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Snapshot {
    pub ts: i64,
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub wan: String,
    #[serde(default)]
    pub router: Router,
    #[serde(default)]
    pub ifaces: Vec<Iface>,
    #[serde(default)]
    pub issues: Vec<Issue>,
}

#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Iface {
    #[serde(default)]
    pub name: String,
    pub active: bool,
    pub down_bps: Option<f64>,
    pub up_bps: Option<f64>,
    /// Cumulative received bytes (from the router's perspective).
    pub down_total: u64,
    /// Cumulative transmitted bytes.
    pub up_total: u64,
    pub errors: i64,
}

#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Issue {
    /// `bad` or `warn`.
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub message: String,
}

#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Router {
    #[serde(default)]
    pub hostname: String,
    pub uptime_secs: Option<i64>,
    pub load: Option<[f64; 3]>,
    pub ncpu: Option<i64>,
    #[serde(default)]
    pub cpu: Cpu,
    #[serde(default)]
    pub mem: Mem,
    pub conntrack: Option<u64>,
    pub own_tcp: Option<u64>,
    pub retrans_rate: Option<f64>,
}

#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Cpu {
    pub user: Option<f64>,
    pub system: Option<f64>,
    pub idle: Option<f64>,
}

#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Mem {
    pub free: Option<u64>,
    pub total: Option<u64>,
}

/// One point per sample. The same struct serves the `/api/history` response *and* the
/// store's rolling window, because `push` appends to it in place — see `state.rs`.
#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct History {
    #[serde(default)]
    pub ts: Vec<i64>,
    #[serde(default)]
    pub ifaces: BTreeMap<String, IfaceSeries>,
    #[serde(default)]
    pub conntrack: Vec<Option<u64>>,
    #[serde(default)]
    pub load: Vec<Option<f64>>,
}

#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct IfaceSeries {
    #[serde(default)]
    pub up: Vec<Option<f64>>,
    #[serde(default)]
    pub down: Vec<Option<f64>>,
}

#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Config {
    /// Interface name treated as the WAN for labels.
    #[serde(default)]
    pub wan: String,
    #[serde(default)]
    pub title: String,
    /// Seconds between the backend's samples; the page's rates are per interval.
    #[serde(default)]
    pub interval: Option<u64>,
    #[serde(default)]
    pub connections: Option<ConnConfig>,
    #[serde(default)]
    pub leases: Option<LeaseConfig>,
}

#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct ConnConfig {
    pub enabled: bool,
    pub min_age: i64,
    pub min_bytes: i64,
    pub max_rows: usize,
    /// Stale window, in seconds — the age past which a connection is no longer
    /// counted as active by the backend.
    pub stale: i64,
    #[serde(default)]
    pub local_prefixes: Vec<String>,
}

#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct LeaseConfig {
    /// false when the router was started with `--no-leases` (no lease file).
    pub enabled: bool,
    /// The lease length dnsmasq hands out (`--lease-ttl`), in seconds.
    pub ttl: i64,
    pub max_rows: usize,
    /// Every VLAN the router knows about, whether or not it has leases.
    #[serde(default)]
    pub vlans: Vec<String>,
}

// --- the connection table (/api/connections) ------------------------------

/// Which side of the connection the network is on, as the backend labels it.
/// A `String` rather than an enum because the page's "every direction" option is
/// the empty value, and the wire writes `null` for it.
#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct ConnFilter {
    pub min_age: i64,
    pub min_bytes: i64,
    pub min_bps: f64,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub sort: String,
    #[serde(default)]
    pub q: Option<String>,
    pub limit: usize,
}

#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct ConnResponse {
    pub ts: i64,
    pub interval: u64,
    pub tracked: usize,
    pub active: usize,
    pub matched: usize,
    pub returned: usize,
    pub unaccounted: usize,
    pub unparsed: usize,
    #[serde(default)]
    pub filter: ConnFilter,
    #[serde(default)]
    pub defaults: ConnDefaults,
    #[serde(default)]
    pub totals: HostAgg,
    #[serde(default)]
    pub hosts: Vec<HostAgg>,
    #[serde(default)]
    pub connections: Vec<ConnRow>,
}

#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct ConnDefaults {
    pub min_age: i64,
    pub min_bytes: i64,
    pub limit: usize,
}

/// A roll-up: one device's totals, or (as `totals`) the whole network's. Computed by
/// the backend over *every* active connection, not over the filtered page — so the
/// age slider cannot hide the answer it was moved to find.
#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct HostAgg {
    #[serde(default)]
    pub addr: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    pub conns: usize,
    pub down_bps: f64,
    pub up_bps: f64,
    pub bytes_down: u64,
    pub bytes_up: u64,
    pub unaccounted: usize,
}

#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct ConnRow {
    #[serde(default)]
    pub proto: String,
    /// TCP's state word; null for UDP/ICMP, which have none.
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub kind: String,
    pub assured: bool,
    pub offloaded: bool,
    /// The network's end: the device, not the direction.
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub host_name: Option<String>,
    #[serde(default)]
    pub host_port: Option<u16>,
    /// The other end.
    #[serde(default)]
    pub peer: String,
    #[serde(default)]
    pub peer_name: Option<String>,
    #[serde(default)]
    pub peer_port: Option<u16>,
    /// A word for the port or ICMP type, when one is known.
    #[serde(default)]
    pub service: Option<String>,
    /// Seconds this dashboard has been seeing the flow — not the connection's age,
    /// because conntrack records no start time.
    pub age: i64,
    pub first_seen: i64,
    pub samples: usize,
    /// Peer→device and device→peer since the connection began. Null when the kernel
    /// was not counting it (created before the acct sysctl was on).
    #[serde(default)]
    pub bytes_down: Option<u64>,
    #[serde(default)]
    pub bytes_up: Option<u64>,
    #[serde(default)]
    pub down_bps: Option<f64>,
    #[serde(default)]
    pub up_bps: Option<f64>,
    pub accounted: bool,
    pub timeout: i64,
}

// --- the DHCP lease table (/api/leases) -----------------------------------

#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct LeaseRow {
    #[serde(default)]
    pub ip: String,
    /// What the client called itself; "" for dnsmasq's own `ra` rows.
    #[serde(default)]
    pub name: String,
    /// The MAC, or `*` for a row dnsmasq generated rather than one a client took.
    #[serde(default)]
    pub mac: String,
    /// dnsmasq's client-id: how the client was matched, and may be empty.
    #[serde(default)]
    pub cid: String,
    /// Wall-clock epoch seconds when this address is handed out again.
    pub expires: i64,
    /// The lease length the router hands out.
    pub ttl: i64,
    /// Whether `ttl` describes *this* lease's block rather than the closest guess.
    pub known: bool,
    #[serde(default)]
    pub vlan: Option<String>,
    /// The network the address is in (`10.3.2.0/24`), or "".
    #[serde(default)]
    pub net: String,
    /// Seconds until it is handed out again; negative once it has passed.
    pub left: i64,
    /// Percent of the lease still to run; null when the length is unknown.
    #[serde(default)]
    pub pct: Option<f64>,
    /// When it was issued, recovered from the expiry; null when unknown.
    #[serde(default)]
    pub issued: Option<i64>,
    /// dnsmasq's `ra-*` row rather than a client's lease.
    pub generated: bool,
}

#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct LeaseResponse {
    /// The router's clock when it answered: countdowns are relative to this.
    pub now: i64,
    pub lease_ttl: i64,
    #[serde(default)]
    pub file: String,
    pub total: usize,
    /// Lease-file lines that were not leases (dnsmasq's own header, a torn write).
    pub skipped: usize,
    pub matched: usize,
    pub returned: usize,
    pub expired: usize,
    /// Leases whose block is not one the router was told about, so no true length.
    pub unknown_ttl: usize,
    pub generated: usize,
    pub max_left: i64,
    /// Unix seconds when the lease file was last written, or null when the OS would not
    /// say. A fact about the *channel*, not about any lease: dnsmasq rewrites the file on
    /// the descriptor it opened at startup, so a path that is replaced leaves the daemon
    /// writing to an orphan and this table serving whatever was on disk at the swap.
    #[serde(default)]
    pub source_mtime: Option<i64>,
    /// Seconds between that write and `now`; null when the mtime is unknown.
    #[serde(default)]
    pub source_age: Option<i64>,
    /// Why this table cannot be the truth, if it cannot be: `"all-expired"` or
    /// `"unwritten"`. The verdict is the backend's (`crates/network-status/src/leases.rs::stale`),
    /// so the reason and the counts beside it cannot disagree — this page only translates it.
    #[serde(default)]
    pub stale: Option<String>,
    /// Lease count per VLAN device name.
    #[serde(default)]
    pub vlans: BTreeMap<String, usize>,
    /// Lease count per network prefix.
    #[serde(default)]
    pub networks: BTreeMap<String, usize>,
    #[serde(default)]
    pub vlan_labels: Vec<String>,
    #[serde(default)]
    pub filter: LeaseFilter,
    #[serde(default)]
    pub defaults: LeaseDefaults,
    #[serde(default)]
    pub leases: Vec<LeaseRow>,
}

#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct LeaseFilter {
    #[serde(default)]
    pub sort: String,
    #[serde(default)]
    pub max_left: i64,
    #[serde(default)]
    pub vlan: Option<String>,
    #[serde(default)]
    pub net: Option<String>,
    #[serde(default)]
    pub reverse: bool,
    #[serde(default)]
    pub q: Option<String>,
    pub limit: usize,
}

#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct LeaseDefaults {
    pub max_left: i64,
    pub limit: usize,
}

// ---------------------------------------------------------------- fetch helpers

pub async fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, String> {
    let res = gloo_net::http::Request::get(url)
        .send()
        .await
        .map_err(|e| format!("fetch: {e}"))?;
    if !res.ok() {
        let status = res.status();
        return Err(res
            .json::<ApiError>()
            .await
            .map(|e| e.error)
            .unwrap_or_else(|_| format!("HTTP {status}")));
    }
    res.json::<T>().await.map_err(|e| format!("decode: {e}"))
}

/// The connection table. The filter is applied by the backend — the table is thousands
/// of rows and the page shows a hundred — so changing the age or the search box costs
/// one request, not a re-download of everything.
pub fn conn_url(f: &ConnControls) -> String {
    let mut parts = vec![
        format!("min_age={}", f.min_age),
        format!("min_bytes={}", f.min_bytes),
        // "moving now" is a rate filter, so it is the backend's to apply: it runs before the
        // row limit, and filtering the rows this page already has would let the first hundred
        // idle connections hide the traffic that is actually moving. The control is the input
        // and this line is the whole of it — there is no stored `min_bps` to disagree with the
        // checkbox.
        format!("min_bps={}", if f.busy_only { 1.0 } else { 0.0 }),
        format!("sort={}", enc(&f.sort)),
        format!("limit={}", f.limit),
    ];
    if !f.kind.is_empty() {
        parts.push(format!("kind={}", enc(&f.kind)));
    }
    if !f.q.is_empty() {
        parts.push(format!("q={}", enc(&f.q)));
    }
    format!("/api/connections?{}", parts.join("&"))
}

pub fn lease_url(c: &LeaseControls) -> String {
    let mut parts = vec![format!("sort={}", enc(&c.sort))];
    if c.max_left >= 0 {
        parts.push(format!("max_left={}", c.max_left));
    }
    if !c.vlan.is_empty() {
        parts.push(format!("vlan={}", enc(&c.vlan)));
    }
    if !c.q.is_empty() {
        parts.push(format!("q={}", enc(&c.q)));
    }
    format!("/api/leases?{}", parts.join("&"))
}

/// `URLSearchParams` is not in `js_sys`; the encoding is one call, and the backend's
/// `common_web::query_params` percent-decodes, so the search box's spaces and slashes
/// have to arrive escaped.
fn enc(s: &str) -> String {
    js_sys::encode_uri_component(s).as_string().unwrap_or_else(|| s.to_string())
}
