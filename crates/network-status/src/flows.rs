//! The tracker: successive [`crate::conntrack`] samples folded into flows with
//! ages, rates and per-host totals, then filtered, sorted and rendered for the
//! page.
//!
//! Rates are differenced between *this service's* samples, so they mean "bytes/s
//! over the last sample interval" — the same number, from the same tick, that
//! the bandwidth chart is made of.  A flow needs two readings with counters
//! before it has a rate at all, which is why the default `min_age` is one
//! interval: an age filter below the sample interval asks for rows that cannot
//! have a rate.
//!
//! The query string's contract — what a caller may ask for and what it gets when
//! it asks badly — is in [`query`], and the tests of the rendered page are in
//! [`tests`].  They are submodules because they are about this one type, and the
//! alternative is one file holding the tracker, its parser, its query grammar and
//! the rendering of both — which is how `main.rs` got to 1,308 lines before this
//! feature added four more modules to the crate (AGENTS.md rule 26).

pub mod query;
#[cfg(test)]
mod tests;

pub(crate) use query::{parse_query, Query, Sort};

use crate::conntrack::{flow_key, parse_table, ConnRow};
use crate::json::{jnum, jopt_num, jopt_u16, jopt_u64, opt_str};
use crate::model::rate;
use crate::names::{icmp_name, service_name, Names};
use crate::nets::Localness;
use common_json::quoted;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

// Tracking across samples
// ---------------------------------------------------------------------------

/// Which side of the connection the network is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An internal address initiated it: a laptop pulling a file.
    Out,
    /// An external address initiated it: something reaching in through a port
    /// forward, the tailnet, or the public aliases.
    In,
    /// Both ends are internal: NFS to the NAS, frigate pulling a camera.
    Lan,
    /// Neither end is internal: the router's own traffic out of the uplink, or
    /// a tailnet transit.
    Wan,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Out => "out",
            Kind::In => "in",
            Kind::Lan => "lan",
            Kind::Wan => "wan",
        }
    }

    fn parse(s: &str) -> Option<Kind> {
        match s {
            "out" => Some(Kind::Out),
            "in" => Some(Kind::In),
            "lan" => Some(Kind::Lan),
            "wan" => Some(Kind::Wan),
            _ => None,
        }
    }
}

/// A connection as the dashboard has seen it over one or more samples.
#[derive(Clone, Debug)]
pub(crate) struct Flow {
    pub(crate) proto: String,
    pub(crate) state: Option<String>,
    pub(crate) assured: bool,
    pub(crate) offloaded: bool,
    pub(crate) kind: Kind,
    /// The network's end and the other end.  For [`Kind::Lan`] and [`Kind::Wan`]
    /// that is just the initiator and its peer.
    pub(crate) host: IpAddr,
    pub(crate) peer: IpAddr,
    pub(crate) host_port: Option<u16>,
    pub(crate) peer_port: Option<u16>,
    pub(crate) icmp_type: Option<u16>,
    pub(crate) v6: bool,
    /// `true` when "down" is the kernel's reply tuple — which it is for every
    /// kind except [`Kind::In`], where the original tuple *is* the traffic
    /// arriving from the peer.
    pub(crate) down_is_reply: bool,
    /// Kernel counters at the latest reading.
    pub(crate) orig_bytes: Option<u64>,
    pub(crate) reply_bytes: Option<u64>,
    /// Bytes/s since the previous reading, in the kernel's own directions.
    pub(crate) orig_bps: Option<f64>,
    pub(crate) reply_bps: Option<f64>,
    pub(crate) first_seen: i64,
    pub(crate) last_seen: i64,
    pub(crate) samples: u32,
    pub(crate) timeout: u64,
}

impl Flow {
    fn new(sample_ts: i64, row: &ConnRow, local: &Localness) -> Flow {
        let unspecified = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
        let (src, dst) = (
            row.orig.src.unwrap_or(unspecified),
            row.orig.dst.unwrap_or(unspecified),
        );
        let kind = match (local.is_local(src), local.is_local(dst)) {
            (true, false) => Kind::Out,
            (false, true) => Kind::In,
            (true, true) => Kind::Lan,
            (false, false) => Kind::Wan,
        };
        let in_ = kind == Kind::In;
        Flow {
            proto: row.proto.clone(),
            state: row.state.clone(),
            assured: row.assured,
            offloaded: row.offloaded,
            kind,
            host: if in_ { dst } else { src },
            peer: if in_ { src } else { dst },
            host_port: if in_ { row.orig.dport } else { row.orig.sport },
            peer_port: if in_ { row.orig.sport } else { row.orig.dport },
            icmp_type: row.orig.icmp_type,
            v6: row.v6(),
            down_is_reply: !in_,
            orig_bytes: row.orig.bytes,
            reply_bytes: row.reply.bytes,
            orig_bps: None,
            reply_bps: None,
            first_seen: sample_ts,
            last_seen: sample_ts,
            samples: 1,
            timeout: row.timeout,
        }
    }

    /// Fold a newer reading of the same connection in.
    fn observe(&mut self, sample_ts: i64, row: &ConnRow) {
        let dt = (sample_ts - self.last_seen).max(0) as f64;
        // Either counter going backwards means the tuple now belongs to a
        // different connection (module header): the history is not comparable,
        // so it is dropped rather than reported as a negative rate.
        let reused = is_downgrade(row.orig.bytes, self.orig_bytes)
            || is_downgrade(row.reply.bytes, self.reply_bytes);
        if reused {
            self.first_seen = sample_ts;
            self.samples = 1;
            self.orig_bps = None;
            self.reply_bps = None;
        } else {
            self.orig_bps = delta(row.orig.bytes, self.orig_bytes, dt);
            self.reply_bps = delta(row.reply.bytes, self.reply_bytes, dt);
            self.samples += 1;
        }
        self.state = row.state.clone();
        self.assured = row.assured;
        self.offloaded = row.offloaded;
        self.orig_bytes = row.orig.bytes;
        self.reply_bytes = row.reply.bytes;
        self.timeout = row.timeout;
        self.last_seen = sample_ts;
    }

    fn bytes_down(&self) -> Option<u64> {
        if self.down_is_reply {
            self.reply_bytes
        } else {
            self.orig_bytes
        }
    }

    fn bytes_up(&self) -> Option<u64> {
        if self.down_is_reply {
            self.orig_bytes
        } else {
            self.reply_bytes
        }
    }

    fn down_bps(&self) -> Option<f64> {
        if self.down_is_reply {
            self.reply_bps
        } else {
            self.orig_bps
        }
    }

    fn up_bps(&self) -> Option<f64> {
        if self.down_is_reply {
            self.orig_bps
        } else {
            self.reply_bps
        }
    }

    /// The kernel counts a flow in both directions only if `acct` was on when it
    /// began.  A flow missing either half has no rate and no honest total, so
    /// `bytes_*` for it are reported as `null` rather than as zero.
    fn accounted(&self) -> bool {
        self.orig_bytes.is_some() && self.reply_bytes.is_some()
    }

    fn age_at(&self, now: i64) -> i64 {
        (now - self.first_seen).max(0)
    }

    /// What the "what" column shows: the service on the far port, or this
    /// protocol's name when there is no port to name (ICMP, UDP).
    fn service(&self) -> Option<&'static str> {
        if let Some(t) = self.icmp_type {
            return icmp_name(t, self.v6);
        }
        service_name(&self.proto, self.peer_port.unwrap_or(u16::MAX))
            .or_else(|| service_name(&self.proto, self.host_port.unwrap_or(u16::MAX)))
    }

    fn json(&self, names: &Names, now: i64) -> String {
        format!(
            r#"{{"proto":{},"state":{},"kind":{},"assured":{},"offloaded":{},"host":{},"host_name":{},"host_port":{},"peer":{},"peer_name":{},"peer_port":{},"service":{},"age":{},"first_seen":{},"samples":{},"bytes_down":{},"bytes_up":{},"down_bps":{},"up_bps":{},"accounted":{},"timeout":{}}}"#,
            quoted(&self.proto),
            self.state
                .as_deref()
                .map(quoted)
                .unwrap_or_else(|| "null".into()),
            quoted(self.kind.as_str()),
            self.assured,
            self.offloaded,
            quoted(&self.host.to_string()),
            names
                .get(self.host)
                .map(quoted)
                .unwrap_or_else(|| "null".into()),
            jopt_u16(self.host_port),
            quoted(&self.peer.to_string()),
            names
                .get(self.peer)
                .map(quoted)
                .unwrap_or_else(|| "null".into()),
            jopt_u16(self.peer_port),
            self.service().map(quoted).unwrap_or_else(|| "null".into()),
            self.age_at(now),
            self.first_seen,
            self.samples,
            jopt_u64(self.bytes_down()),
            jopt_u64(self.bytes_up()),
            jopt_num(self.down_bps()),
            jopt_num(self.up_bps()),
            self.accounted(),
            self.timeout,
        )
    }
}

pub(crate) fn is_downgrade(cur: Option<u64>, prev: Option<u64>) -> bool {
    matches!((cur, prev), (Some(c), Some(p)) if c < p)
}

/// [`rate`] over `Option`al counters: `None` if either half is missing, which is
/// a flow the kernel is not counting for us, or a first reading.
pub(crate) fn delta(cur: Option<u64>, prev: Option<u64>, dt: f64) -> Option<f64> {
    match (cur, prev) {
        (Some(c), Some(p)) => rate(c, p, dt),
        _ => None,
    }
}

/// The tracker: every connection the table held at the last sample, plus what
/// this service has watched it do since.
#[derive(Default)]
pub struct Flows {
    pub(crate) flows: HashMap<String, Flow>,
    pub(crate) names: Names,
    pub(crate) sample_ts: i64,
    /// Lines of the last sample that would not parse.
    pub(crate) unparsed: usize,
    /// Active connections with no kernel byte counters — created before
    /// `net.netfilter.nf_conntrack_acct` was switched on, so this decays to zero
    /// as those connections die out.
    pub(crate) unaccounted: usize,
}

impl Flows {
    pub fn new(hosts_files: &[String], leases_file: Option<String>) -> Self {
        Flows {
            flows: HashMap::new(),
            names: Names::open(hosts_files, leases_file),
            sample_ts: 0,
            unparsed: 0,
            unaccounted: 0,
        }
    }

    /// Fold one sample of the kernel's table into the tracker.
    ///
    /// `stale` is how long a flow may be missing from the table and still be
    /// remembered: a UDP entry expires between packets, and a flow that blinks
    /// out and back should not have its age reset under the operator.
    pub fn update(&mut self, sample_ts: i64, raw: &str, local: &Localness, stale: i64) {
        self.names.refresh();
        let (rows, unparsed) = parse_table(raw);
        self.unparsed = unparsed;
        self.sample_ts = sample_ts;
        for row in rows {
            let key = flow_key(&row);
            match self.flows.get_mut(&key) {
                Some(flow) => flow.observe(sample_ts, &row),
                None => {
                    self.flows.insert(key, Flow::new(sample_ts, &row, local));
                }
            }
        }
        self.flows.retain(|_, f| sample_ts - f.last_seen <= stale);
        self.unaccounted = self.active().filter(|f| !f.accounted()).count();
    }

    /// Flows present at the newest sample — what "active" means here, and
    /// what the kernel's own entry count would be if it were asked now.
    fn active(&self) -> impl Iterator<Item = &Flow> {
        let ts = self.sample_ts;
        self.flows.values().filter(move |f| f.last_seen == ts)
    }

    /// Render `/api/connections`.
    pub fn json(&self, q: &Query, defaults: &Query, interval: u64) -> String {
        // Totals and the per-host roll-up are deliberately computed over *every*
        // active connection, not over the filtered page: "who is the biggest
        // talker" must not depend on the age filter or on how far the table was
        // scrolled, or turning the filter up hides the very answer it was
        // turned up to find.
        let mut totals = Agg::default();
        let mut hosts: HashMap<IpAddr, Agg> = HashMap::new();
        for f in self.active() {
            totals.add(f);
            hosts.entry(f.host).or_default().add(f);
        }

        // Folded once per request rather than per row: the haystack below is
        // folded, and a needle that arrives already lower-cased by `parse_query`
        // is an invariant nobody should have to remember to keep.
        let needle = q
            .needle
            .as_deref()
            .map(|n| n.trim().to_lowercase())
            .filter(|n| !n.is_empty());

        let mut rows: Vec<&Flow> = self
            .active()
            .filter(|f| f.age_at(self.sample_ts) >= q.min_age as i64)
            .filter(|f| {
                q.min_bytes == 0
                    || f.bytes_down().unwrap_or(0) + f.bytes_up().unwrap_or(0) >= q.min_bytes
            })
            .filter(|f| {
                q.min_bps <= 0.0
                    || f.down_bps().unwrap_or(0.0) + f.up_bps().unwrap_or(0.0) >= q.min_bps
            })
            .filter(|f| q.kind.is_none_or(|k| k == f.kind))
            .filter(|f| match &needle {
                None => true,
                Some(needle) => row_matches(f, &self.names, needle),
            })
            .collect();
        rows.sort_by_key(|f| std::cmp::Reverse(sort_key(f, q.sort)));
        let matched = rows.len();
        rows.truncate(q.limit);

        let conns: Vec<String> = rows
            .iter()
            .map(|f| f.json(&self.names, self.sample_ts))
            .collect();
        let mut host_rows: Vec<(IpAddr, Agg)> = hosts.into_iter().collect();
        host_rows.sort_by(|a, b| {
            b.1.rate()
                .total_cmp(&a.1.rate())
                .then_with(|| a.0.cmp(&b.0))
        });
        let host_rows: Vec<String> = host_rows
            .iter()
            .map(|(addr, agg)| agg.json(Some(*addr), &self.names))
            .collect();

        format!(
            r#"{{"ts":{},"interval":{},"tracked":{},"active":{},"matched":{},"returned":{},"unaccounted":{},"unparsed":{},"filter":{{"min_age":{},"min_bytes":{},"min_bps":{},"kind":{},"sort":{},"q":{},"limit":{}}},"defaults":{{"min_age":{},"min_bytes":{},"limit":{}}},"totals":{},"hosts":[{}],"connections":[{}]}}"#,
            self.sample_ts,
            interval,
            self.flows.len(),
            totals.conns,
            matched,
            rows.len(),
            self.unaccounted,
            self.unparsed,
            q.min_age,
            q.min_bytes,
            jnum(q.min_bps),
            opt_str(q.kind.map(|k| k.as_str())),
            quoted(q.sort.as_str()),
            q.needle
                .as_deref()
                .map(quoted)
                .unwrap_or_else(|| "null".into()),
            q.limit,
            defaults.min_age,
            defaults.min_bytes,
            defaults.limit,
            totals.json(None, &self.names),
            host_rows.join(","),
            conns.join(","),
        )
    }

    /// The numbers worth charting about the table itself.  Nothing per-flow: a
    /// per-address label would be a new cardinality limit to manage every busy
    /// hour, and detail that size belongs to the page rather than to Prometheus.
    pub fn metrics(&self, min_age: u64) -> String {
        let min_age = min_age as i64;
        let mut long_running = 0usize;
        let mut active = 0usize;
        for f in self.active() {
            active += 1;
            if f.age_at(self.sample_ts) >= min_age {
                long_running += 1;
            }
        }
        format!(
            "# HELP router_connections_tracked Flows tracked: in the table now, or gone within --conn-stale\n\
             # TYPE router_connections_tracked gauge\n\
             router_connections_tracked {}\n\
             # HELP router_connections_active Flows in the kernel's table at the last sample\n\
             # TYPE router_connections_active gauge\n\
             router_connections_active {}\n\
             # HELP router_connections_long_running Active connections older than --conn-min-age\n\
             # TYPE router_connections_long_running gauge\n\
             router_connections_long_running {}\n\
             # HELP router_connections_unaccounted Active connections with no kernel byte counters, i.e. created before net.netfilter.nf_conntrack_acct was on\n\
             # TYPE router_connections_unaccounted gauge\n\
             router_connections_unaccounted {}\n",
            self.flows.len(),
            active,
            long_running,
            self.unaccounted,
        )
    }
}

/// The sortable projection of a flow: rates and byte counts as integers, so the
/// tuple is `Ord` and one comparator serves every column.  A rate is bytes per
/// second, so the saturation at `i64::MAX` is 8 exabytes a second; quantising it
/// to whole bytes per second cannot reorder two flows that differ by more than
/// half a byte per second, which is not a distinction the table makes.
pub(crate) fn sort_key(a: &Flow, sort: Sort) -> (i64, i64, i64, String) {
    let rate = |o: Option<f64>| o.unwrap_or(0.0).clamp(0.0, i64::MAX as f64).round() as i64;
    let bytes = |o: Option<u64>| o.unwrap_or(0).min(i64::MAX as u64) as i64;
    let total_rate = |f: &Flow| rate(f.down_bps()) + rate(f.up_bps());
    let total_bytes = |f: &Flow| bytes(f.bytes_down()) + bytes(f.bytes_up());
    match sort {
        Sort::Rate => (total_rate(a), 0, 0, String::new()),
        Sort::Down => (rate(a.down_bps()), 0, 0, String::new()),
        Sort::Up => (rate(a.up_bps()), 0, 0, String::new()),
        Sort::Bytes => (total_bytes(a), 0, 0, String::new()),
        // `-first_seen`, because the comparator below inverts its arguments to
        // sort descending and the oldest connection must come out on top.
        Sort::Age => (0, 0, -a.first_seen, String::new()),
        Sort::Host => (0, 0, 0, format!("{} {}", a.host, a.peer)),
    }
}

pub(crate) fn row_matches(f: &Flow, names: &Names, needle: &str) -> bool {
    let mut hay = format!(
        "{} {} {} {} {} {}",
        f.proto,
        f.kind.as_str(),
        f.host,
        f.peer,
        names.get(f.host).unwrap_or(""),
        names.get(f.peer).unwrap_or(""),
    );
    if let Some(state) = &f.state {
        hay.push(' ');
        hay.push_str(state);
    }
    for port in [f.host_port, f.peer_port].into_iter().flatten() {
        hay.push(' ');
        hay.push_str(&port.to_string());
    }
    if let Some(svc) = f.service() {
        hay.push(' ');
        hay.push_str(svc);
    }
    hay.make_ascii_lowercase();
    hay.contains(needle)
}

/// A roll-up over a set of connections: one host's totals, or the network's.
#[derive(Default, Clone, Copy)]
pub(crate) struct Agg {
    pub(crate) conns: usize,
    pub(crate) down_bps: f64,
    pub(crate) up_bps: f64,
    pub(crate) bytes_down: u64,
    pub(crate) bytes_up: u64,
    /// Flows with no kernel counters, which is why `bytes_*` under-reports
    /// and the page has to say so instead of showing a confident zero.
    pub(crate) unaccounted: usize,
}

impl Agg {
    fn add(&mut self, f: &Flow) {
        self.conns += 1;
        self.down_bps += f.down_bps().unwrap_or(0.0);
        self.up_bps += f.up_bps().unwrap_or(0.0);
        self.bytes_down += f.bytes_down().unwrap_or(0);
        self.bytes_up += f.bytes_up().unwrap_or(0);
        if !f.accounted() {
            self.unaccounted += 1;
        }
    }

    /// The ranking key: current rate, because that is the question the page asks.
    fn rate(&self) -> f64 {
        self.down_bps + self.up_bps
    }

    /// `addr` is `None` for the network-wide roll-up.
    fn json(&self, addr: Option<IpAddr>, names: &Names) -> String {
        let name = match addr {
            Some(ip) => names.get(ip).map(quoted).unwrap_or_else(|| "null".into()),
            None => "null".into(),
        };
        let addr = match addr {
            Some(ip) => quoted(&ip.to_string()),
            None => "null".into(),
        };
        format!(
            r#"{{"addr":{},"name":{},"conns":{},"down_bps":{},"up_bps":{},"bytes_down":{},"bytes_up":{},"unaccounted":{}}}"#,
            addr,
            name,
            self.conns,
            jnum(self.down_bps),
            jnum(self.up_bps),
            self.bytes_down,
            self.bytes_up,
            self.unaccounted,
        )
    }
}
