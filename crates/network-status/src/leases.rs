//! The DHCP lease table: dnsmasq's lease file rendered as "which client holds
//! which address, and how much of that lease is left".
//!
//! The conntrack table ([`crate::flows`]) says an address is *in use*.  The lease
//! file says dnsmasq promised to keep answering this client at this address until
//! an expiry, which is why a client's connections look the way they do: when the
//! lease lapses the client renews, and the address can move.  That is why the page
//! shows a countdown rather than a date.
//!
//! ## Why a lease's length is recovered rather than read
//!
//! dnsmasq's file stores an **absolute expiry and no TTL**.  A lease's length is
//! therefore not in the file, and a "how much is left" bar needs the length.  It
//! is recovered from the block the address falls in: every `dhcp-range`
//! `machines/gw/lib/network-tables.nix` generates is generated at `leaseTtl`, the
//! flake's one knob for how long a lease lives, which reaches this crate as
//! `--lease-ttl` beside the blocks themselves (`--vlan vlan226=10.3.2.0/24`, via
//! [`crate::nets`]).  So a lease inside a block we were told about is `known:
//! true` — its bar is a true fraction of a real lease.  A lease in no block (a
//! tailnet address, a hand-written range this flake does not describe) gets the
//! same arithmetic with `known: false`, because what it actually ran at was
//! dnsmasq's own default, which this crate does not know.  The page dims those
//! bars rather than drawing full-length lies.
//!
//! ## The 2038 rows
//!
//! dnsmasq's `ra-only`/`ra-param` lines hand a whole VLAN block a fixed release
//! time with no client attached, and they land in the same file as leases.  Their
//! expiry is the release time and the start was never recorded anywhere, so they
//! have no `issued` and no bar: a countdown from "when this file was last written"
//! would be a countdown whose length changes with every write, and would prove
//! nothing.  They *are* leases as far as "this address is spoken for" goes, which
//! is why they are rows and not skipped.
//!
//! ## A table nobody is writing must not read like a quiet network
//!
//! `expired` rows are a real state and `max_left=-1` deliberately shows them, so
//! the table can be *full* and *worthless* at the same time.  That is not
//! hypothetical: from 2026-10-28 20:51 to 2026-10-31 14:28 this box served 56
//! rows, every one of them already lapsed, while dnsmasq's live database held 92
//! leases — the file had been replaced under the daemon's open fd and nothing
//! could reach the disk copy again (`machines/gw/lease-reader.nix` has the
//! mechanism and the receipts).  A page that says "56 leases (56 lapsed)" is a
//! page that says "nobody is renewing", which is a different lie from "I have no
//! data".  So `read` stats the file and [`stale`] judges the pair, and `/api/leases`
//! answers `stale` with a reason: `"all-expired"` when every row has lapsed (a
//! serving DHCP server always has at least one that has not), and `"unwritten"`
//! when the file is older than two lease lengths (dnsmasq rewrites it on every
//! lease event, so a lease length is the most a live one can go quiet).  The page
//! turns either into a banner.  Neither rule can fire on an empty table: zero
//! leases with a dead writer and zero leases with a healthy one look the same
//! from here, and only the first is a fault.
//!
//! ## Reading the file is a decision, not an accident
//!
//! On `gw` the file is `0640 root:root`.  `machines/gw/lease-reader.nix` is what
//! makes it group-readable to the dashboard's account — the same shape as
//! `conntrack-reader.nix`, and for the same reason: supplementary groups on a
//! `DynamicUser` are not reliable for reading a root-owned file, so the service
//! has a real system account with a primary group.  An unreadable file is
//! therefore reported as an error by `/api/leases` and never as an empty table —
//! an empty lease table reads as "no DHCP clients", which is the wrong thing for
//! an operator to conclude.

use crate::json::{jopt_num, opt_str};
use crate::names::{lease_line, LeaseLine};
use crate::nets::Localness;
use common_json::quoted;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// The lease, and reading the file
// ---------------------------------------------------------------------------

/// The lease file's contents, plus what the read could not make sense of.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Leases {
    pub(crate) rows: Vec<LeaseLine>,
    /// Lines that were not leases.  Not an error: dnsmasq's own `duid <hex>`
    /// header is one of these, and so is any half-written renewal caught
    /// mid-write.  Counted so the page can say "49 leases, 2 lines ignored"
    /// rather than quietly losing them.
    pub(crate) skipped: usize,
    /// When the file was last written, as the filesystem says, in unix seconds.
    /// `None` when the file would not say (no mtime on this platform, or the
    /// stat failed under a read that somehow succeeded).
    ///
    /// This is a fact about the *channel*, not about any lease, and it is here
    /// because the channel dies silently: dnsmasq writes this file in place on
    /// the descriptor it opened when it started, so anything that replaces the
    /// path — `install`, `mv`, `rsync`, an editor's save-replace — leaves the
    /// daemon writing to an orphan and the page serving whatever was on disk at
    /// the moment of the swap, forever (module header).
    pub(crate) source_mtime: Option<i64>,
}

/// Read the lease file.  `Err` is the file being unopenable, which the endpoint
/// reports rather than answering with an empty table (module header).
pub fn read(path: &str) -> Result<Leases, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    // Stat it too, and do not fail the read if that fails: the table is worth
    // serving without the freshness fact, and an error here would replace data
    // with a 503 over a filesystem that will not report an mtime.
    let source_mtime = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    let mut out = Leases {
        source_mtime,
        ..Default::default()
    };
    for line in text.lines() {
        match lease_line(line) {
            Some(lease) => out.rows.push(lease),
            // A blank line is not a finding; anything else is.
            None if line.trim().is_empty() => {}
            None => out.skipped += 1,
        }
    }
    Ok(out)
}

/// Why this table cannot be the truth right now, if it cannot be.
///
/// `expired` and `mtime_ago` are what the caller already counted (`expired` as
/// the endpoint reports it, so the verdict and the number beside it cannot
/// disagree).  `ttl` is the lease length the endpoint was handed, which is the
/// longest a live dnsmasq can go without rewriting the file while it still has
/// clients.  See the module header for what each reason means and for why an
/// empty table is never called stale.
pub fn stale(
    leases: usize,
    expired: usize,
    mtime_ago: Option<i64>,
    ttl: i64,
) -> Option<&'static str> {
    if leases == 0 {
        return None;
    }
    if expired == leases {
        return Some("all-expired");
    }
    // Two lease lengths, not one: a single length is reachable honestly on a
    // network whose clients all went quiet at the top of a lease, and a reason
    // this box prints wrongly is a reason it stops being believed.
    if ttl > 0 && mtime_ago.is_some_and(|age| age > 2 * ttl) {
        return Some("unwritten");
    }
    None
}

// ---------------------------------------------------------------------------
// The query grammar `/api/leases` accepts
// ---------------------------------------------------------------------------

/// Which lease to show first.  `parse`/`as_str` are the pair
/// [`crate::flows::query::Sort`] has, and for the same reason: the value comes
/// back out in the JSON so the SPA can echo it without a second table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    /// Soonest to be handed out again: the row a client is about to lose.
    #[default]
    Expires,
    /// Most recently handed out — the same list read from the other end when
    /// every lease has the same length, and a different one when they do not.
    Issued,
    /// By the name the client chose.
    Name,
    /// By VLAN, so one VLAN's leases sit together.
    Vlan,
    /// By address, so one network's leases sit together.
    Addr,
}

impl Sort {
    pub fn parse(s: &str) -> Sort {
        match s.trim().trim_start_matches('^') {
            "issued" => Sort::Issued,
            "name" => Sort::Name,
            "vlan" => Sort::Vlan,
            "addr" => Sort::Addr,
            _ => Sort::Expires,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Sort::Expires => "expires",
            Sort::Issued => "issued",
            Sort::Name => "name",
            Sort::Vlan => "vlan",
            Sort::Addr => "addr",
        }
    }
}

/// What to show, after defaults and clamping.
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    /// Keep leases expiring no later than `now + max_left` seconds.  Negative
    /// keeps every lease, expired ones included — which is the useful default,
    /// because an expired-but-still-listed lease is exactly the row an operator
    /// looking at a stale address needs to see.
    pub max_left: i64,
    /// Case-insensitive substring of name, address, VLAN, network, MAC and
    /// client-id.  Lower-cased by `parse`, because the filter narrows the same
    /// haystack twice per row.
    pub needle: Option<String>,
    /// Keep leases on this VLAN label (`vlan226`).
    pub vlan: Option<String>,
    /// Keep leases inside this network, as the prefix was written
    /// (`10.3.2.0/24`).
    pub net: Option<String>,
    pub limit: usize,
    pub sort: Sort,
    /// `^`-prefixed sort names, the way the connection table spells "backwards".
    pub reverse: bool,
}

impl Default for Query {
    fn default() -> Self {
        Query {
            max_left: -1,
            needle: None,
            vlan: None,
            net: None,
            limit: 100,
            sort: Sort::Expires,
            reverse: false,
        }
    }
}

/// Pull the filters out of a request's query string, falling back to the
/// operator's defaults and clamping `limit` to what `--lease-max-rows` allowed.
///
/// Deliberately its own small grammar rather than a shared one with
/// [`crate::flows::query`]: the two tables filter on different things, and a
/// grammar both have to share is a bigger guess than the handful of names each
/// of them actually has.
pub fn parse(params: &HashMap<String, String>, defaults: &Query) -> Query {
    let num = |key: &str| -> Option<i64> { params.get(key).and_then(|v| v.trim().parse().ok()) };
    let text = |key: &str| -> Option<String> {
        params
            .get(key)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let sort_raw = params.get("sort").map(|s| s.trim()).unwrap_or("");
    Query {
        max_left: num("max_left").unwrap_or(defaults.max_left),
        needle: text("q").map(|v| v.to_lowercase()),
        vlan: text("vlan"),
        net: text("net"),
        limit: params
            .get("limit")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(defaults.limit)
            .clamp(1, defaults.limit.max(1)),
        sort: Sort::parse(sort_raw),
        reverse: sort_raw.starts_with('^'),
    }
}

// ---------------------------------------------------------------------------
// A row: the lease, plus what its address's block says
// ---------------------------------------------------------------------------

/// One lease as the renderer needs it: dnsmasq's fields plus the labels the
/// address's block carries.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub lease: LeaseLine,
    /// The VLAN the block belongs to, when the prefix was labelled.
    pub vlan: Option<String>,
    /// The prefix the address falls in, as it was written; empty when no prefix
    /// covers it.
    pub net: String,
    /// The lease length the renderer was given (`--lease-ttl`).
    pub ttl: i64,
    /// Whether `ttl` describes *this* lease's block.  False for an address no
    /// labelled block covers, where the number is the closest thing we have
    /// rather than the truth (module header).
    pub known: bool,
}

impl Row {
    pub(crate) fn new(lease: LeaseLine, local: &Localness, ttl: i64) -> Row {
        let vlan = local.vlan_of(lease.ip);
        let known = vlan.is_some();
        Row {
            net: local.prefix_of(lease.ip).unwrap_or_default(),
            vlan,
            lease,
            ttl,
            known,
        }
    }

    /// When this lease was issued, as far as it can be recovered: the expiry
    /// minus the block's lease length, and `None` when the block is unknown.
    pub(crate) fn issued(&self) -> Option<i64> {
        if self.known {
            Some(self.lease.expires - self.ttl)
        } else {
            None
        }
    }

    /// Seconds until the address is handed out again.  Negative once it has
    /// already passed, which is a real state: the row is still in the file until
    /// the client renews or dnsmasq rewrites it.
    pub(crate) fn left(&self, now: i64) -> i64 {
        self.lease.expires.saturating_sub(now)
    }

    /// How much of the lease is left, as a percentage — `None` when the length
    /// is unknown or the lease has already expired, so the page can draw "no
    /// bar" rather than "an empty bar", which would mean "one second left".
    pub(crate) fn pct(&self, now: i64) -> Option<f64> {
        let left = self.left(now);
        if !self.known || self.ttl <= 0 || left < 0 {
            return None;
        }
        Some((left as f64) * 100.0 / (self.ttl as f64))
    }

    /// dnsmasq's own row rather than a client's: `ra-*` writes `*` where a MAC
    /// would be.
    pub(crate) fn generated(&self) -> bool {
        self.lease.mac == "*"
    }

    /// Does this row match a needle that `parse` has already lower-cased?
    pub(crate) fn matches(&self, needle: &str) -> bool {
        let hay = [
            self.lease.ip.to_string(),
            self.lease.name.clone(),
            self.lease.mac.clone(),
            self.lease.cid.clone(),
            self.net.clone(),
            self.vlan.clone().unwrap_or_default(),
        ];
        hay.iter().any(|s| s.to_lowercase().contains(needle))
    }

    /// One row as JSON.  Every string here is device-chosen (a hostname, a MAC,
    /// a client-id), so it goes through `quoted` — and the SPA escapes it again
    /// before it reaches the DOM.
    pub(crate) fn json(&self, now: i64) -> String {
        format!(
            concat!(
                r#"{{"ip":{},"name":{},"mac":{},"cid":{},"expires":{},"ttl":{},"known":{},"#,
                r#""vlan":{},"net":{},"left":{},"pct":{},"issued":{},"generated":{}}}"#
            ),
            quoted(&self.lease.ip.to_string()),
            quoted(&self.lease.name),
            quoted(&self.lease.mac),
            quoted(&self.lease.cid),
            self.lease.expires,
            self.ttl,
            self.known,
            opt_str(self.vlan.as_deref()),
            quoted(&self.net),
            self.left(now),
            jopt_num(self.pct(now)),
            match self.issued() {
                Some(t) => t.to_string(),
                None => "null".to_string(),
            },
            self.generated(),
        )
    }

    /// Ascending order for `sort` — the caller reverses for `^`.
    ///
    /// `IpAddr` already orders by family then address, which is the order an
    /// operator reading a table expects, so there is no hand-rolled key here — the
    /// earlier version of this compared a `u128` cast and could not express IPv6
    /// at all.
    pub(crate) fn cmp(&self, other: &Row, sort: Sort) -> Ordering {
        let primary = match sort {
            Sort::Expires => self.lease.expires.cmp(&other.lease.expires),
            // `issued()` is `None` for an unknown-length lease; the expiry is the
            // honest fallback and keeps the order total.
            Sort::Issued => self
                .issued()
                .unwrap_or(self.lease.expires)
                .cmp(&other.issued().unwrap_or(other.lease.expires)),
            Sort::Name => self
                .lease
                .name
                .to_lowercase()
                .cmp(&other.lease.name.to_lowercase()),
            Sort::Vlan => self.vlan.cmp(&other.vlan),
            Sort::Addr => self.lease.ip.cmp(&other.lease.ip),
        };
        primary
            // A stable order under every sort: two rows with the same name are
            // told apart by their address, so the page does not reshuffle between
            // polls and a row can be read while it is being watched.
            .then_with(|| self.lease.ip.cmp(&other.lease.ip))
            .then_with(|| self.lease.name.cmp(&other.lease.name))
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// `{"vlan226":12,"vlan225":3}` — the shape the connection table's per-host
/// totals use, and a `BTreeMap` so the keys come out in a stable order.
fn counts(map: &BTreeMap<String, usize>) -> String {
    map.iter()
        .map(|(k, n)| format!("{}:{}", quoted(k), n))
        .collect::<Vec<_>>()
        .join(",")
}

/// Render `/api/leases`.
///
/// `Err` is the lease file being unopenable.  The summary counts are computed
/// over **every** row that was read, not over the filtered page: they answer
/// "what does this router hold", and a count that moved with the filter would
/// answer "how many rows am I looking at", which `returned` already says.
pub fn json(
    lease_file: &str,
    q: &Query,
    defaults: &Query,
    now: i64,
    local: &Localness,
    ttl: i64,
    max_rows: usize,
) -> Result<String, String> {
    let file = read(lease_file)?;
    let rows: Vec<Row> = file
        .rows
        .iter()
        .map(|l| Row::new(l.clone(), local, ttl))
        .collect();

    let mut by_vlan: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_net: BTreeMap<String, usize> = BTreeMap::new();
    let mut expired = 0usize;
    let mut unknown = 0usize;
    let mut generated = 0usize;
    let mut farthest: i64 = 0;
    for r in &rows {
        if let Some(v) = &r.vlan {
            *by_vlan.entry(v.clone()).or_insert(0) += 1;
        }
        if !r.net.is_empty() {
            *by_net.entry(r.net.clone()).or_insert(0) += 1;
        }
        if r.left(now) < 0 {
            expired += 1;
        }
        if !r.known {
            unknown += 1;
        }
        if r.generated() {
            generated += 1;
        }
        farthest = farthest.max(r.left(now));
    }

    let mut kept: Vec<Row> = rows
        .iter()
        .filter(|r| q.max_left < 0 || r.left(now) <= q.max_left)
        .filter(|r| match &q.vlan {
            None => true,
            Some(v) => r.vlan.as_deref() == Some(v.as_str()),
        })
        .filter(|r| match &q.net {
            None => true,
            Some(n) => &r.net == n,
        })
        .filter(|r| match &q.needle {
            None => true,
            Some(needle) => r.matches(needle),
        })
        .cloned()
        .collect();

    kept.sort_by(|a, b| a.cmp(b, q.sort));
    if q.reverse {
        kept.reverse();
    }
    let matched = kept.len();
    kept.truncate(max_rows);
    kept.truncate(q.limit);
    let returned = kept.len();

    let body: Vec<String> = kept.iter().map(|r| r.json(now)).collect();
    let vlans: Vec<String> = local.vlan_labels();
    let age = file.source_mtime.map(|m| now.saturating_sub(m));
    let stale = stale(rows.len(), expired, age, ttl);

    Ok(format!(
        concat!(
            r#"{{"now":{},"lease_ttl":{},"file":{},"total":{},"skipped":{},"matched":{},"#,
            r#""returned":{},"expired":{},"unknown_ttl":{},"generated":{},"max_left":{},"#,
            // The channel, not the leases: when the file was written, and what
            // that plus the rows say about whether either can still be true
            // (module header).  `stale` is a reason string or null, and the page
            // banners on it rather than comparing timestamps itself.
            r#""source_mtime":{},"source_age":{},"stale":{},"#,
            // The two counts are JSON *objects* (`{"vlan226":12}`) rather than
            // arrays of pairs: the key is the name, and an array here would make
            // the SPA look the name up in a list for every cell.
            r#""vlans":{{{}}},"vlan_labels":[{}],"networks":{{{}}},"filter":{{"max_left":{},"#,
            r#""vlan":{},"net":{},"sort":{},"reverse":{},"q":{},"limit":{}}},"#,
            r#""defaults":{{"max_left":{},"limit":{}}},"leases":[{}]}}"#
        ),
        now,
        ttl,
        quoted(lease_file),
        rows.len(),
        file.skipped,
        matched,
        returned,
        expired,
        unknown,
        generated,
        farthest,
        // The channel, not the leases, and deliberately last: the `concat!` above
        // puts these three keys after `max_left`, and a placeholder list that
        // drifts from the key list shifts every value beside it — which is exactly
        // what `the_json_keys_carry_the_values_they_are` is there to notice.
        file.source_mtime
            .map(|m| m.to_string())
            .unwrap_or_else(|| "null".into()),
        age.map(|a| a.to_string()).unwrap_or_else(|| "null".into()),
        opt_str(stale),
        counts(&by_vlan),
        vlans
            .iter()
            .map(|v| quoted(v))
            .collect::<Vec<_>>()
            .join(","),
        counts(&by_net),
        q.max_left,
        opt_str(q.vlan.as_deref()),
        opt_str(q.net.as_deref()),
        quoted(q.sort.as_str()),
        q.reverse,
        opt_str(q.needle.as_deref()),
        q.limit,
        defaults.max_left,
        defaults.limit,
        body.join(","),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> std::net::IpAddr {
        s.parse().expect("a test address parses")
    }

    fn specs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn local() -> Localness {
        let mut l = Localness::new(&[]);
        l.add_named(&specs(&["vlan226=10.3.2.0/24", "vlan225=10.3.1.0/24"]));
        l
    }

    fn row(expires: i64, ip_s: &str, name: &str, mac: &str) -> Row {
        Row::new(
            LeaseLine {
                expires,
                mac: mac.to_string(),
                ip: ip(ip_s),
                name: name.to_string(),
                cid: String::new(),
            },
            &local(),
            86_400,
        )
    }

    #[test]
    fn a_lease_inside_a_labelled_block_is_known_and_has_a_bar() {
        let r = row(
            1_000_000 + 43_200,
            "10.3.2.50",
            "laptop",
            "aa:bb:cc:dd:ee:ff",
        );
        assert_eq!(r.vlan.as_deref(), Some("vlan226"));
        assert_eq!(r.net, "10.3.2.0/24");
        assert!(r.known);
        assert_eq!(r.issued(), Some(1_043_200 - 86_400));
        // Half a 24-hour lease left is half a bar: `now` is the lease's own start
        // plus half its length.
        assert_eq!(r.pct(1_043_200 - 43_200), Some(50.0));
        assert_eq!(r.pct(1_043_200 + 1), None, "an expired lease has no bar");
    }

    #[test]
    fn a_lease_outside_every_block_is_a_row_with_no_bar() {
        // A tailnet address: inside the network's world, outside every VLAN.
        let r = row(1_000_000, "100.64.0.9", "phone", "aa:bb:cc:dd:ee:01");
        assert_eq!(r.vlan, None);
        assert_eq!(r.net, "");
        assert!(!r.known);
        assert_eq!(r.issued(), None);
        assert_eq!(r.pct(900_000), None, "no bar rather than a full one");
    }

    #[test]
    fn dnsmasqs_own_rows_are_marked_generated() {
        let r = row(1_000_000, "10.3.2.0", "", "*");
        assert!(r.generated());
        assert_eq!(r.lease.name, "");
    }

    #[test]
    fn a_blank_line_is_not_a_finding_but_a_bad_one_is() {
        let path = std::env::temp_dir().join("network-status-leases-test");
        let text = concat!(
            "duid 00:01:00:01:32:6b:9c:78:00:25:90:d3:f0:ea\n",
            "\n",
            "1800000000 aa:bb:cc:dd:ee:ff 10.3.2.50 laptop *\n",
            "not-a-lease\n"
        );
        std::fs::write(&path, text).expect("temp file");
        let file = read(path.to_str().expect("utf-8 path")).expect("readable");
        assert_eq!(file.rows.len(), 1);
        assert_eq!(file.skipped, 2, "the duid header and the garbage line");
        assert_eq!(file.rows[0].name, "laptop");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_unreadable_file_is_an_error_not_an_empty_table() {
        let err = read("/nonexistent/dnsmasq.leases").expect_err("no such file");
        assert!(err.contains("dnsmasq.leases"), "{err}");
    }

    /// Write `text` to a temp path and read it back, returning what was read and
    /// how old the file thinks it is.
    fn read_back(name: &str, text: &str, now: i64) -> (Leases, Option<i64>) {
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, text).expect("temp file");
        let file = read(path.to_str().expect("utf-8 path")).expect("readable");
        let age = file.source_mtime.map(|m| now - m);
        let _ = std::fs::remove_file(&path);
        (file, age)
    }

    #[test]
    fn reading_a_file_also_learns_when_it_was_written() {
        // A file written this second is not "unwritten", and its age is this
        // second — which is the whole of what the freshness verdict rests on.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is past 1970")
            .as_secs() as i64;
        let (file, age) = read_back(
            "network-status-leases-fresh",
            "1800000000 aa:bb:cc:dd:ee:ff 10.3.2.50 laptop *\n",
            now,
        );
        assert_eq!(file.rows.len(), 1);
        assert!(file.source_mtime.is_some(), "the stat has to work here");
        assert!(age.is_some_and(|a| a.abs() <= 5), "just written: {age:?}");
        assert_eq!(
            stale(file.rows.len(), 0, age, 86_400),
            None,
            "a live table is not stale"
        );
    }

    #[test]
    fn a_table_of_nothing_but_lapsed_leases_is_stale() {
        // The real thing: the frozen file `/api/leases` served on 2026-10-31, at
        // the clock it served it.  56 rows, every one past its expiry, the file
        // 2 d 16 h unwritten, while dnsmasq's live database held 92 leases — see
        // the module header.
        assert_eq!(
            stale(56, 56, Some(232_200), 86_400),
            Some("all-expired"),
            "a serving DHCP server always has one lease that has not lapsed"
        );
    }

    #[test]
    fn a_file_older_than_two_lease_lengths_is_stale_even_with_live_rows() {
        let age = |days: i64| days * 86_400;
        // Some leases still have most of their length left, but the file has not
        // been written in three days: the rows cannot be the whole truth.
        assert_eq!(stale(92, 3, Some(age(3)), 86_400), Some("unwritten"));
        // One lease length is a network that went quietly, not a broken channel.
        assert_eq!(stale(92, 3, Some(age(1)), 86_400), None);
        // And an unknown mtime is evidence of nothing.
        assert_eq!(stale(92, 3, None, 86_400), None);
    }

    #[test]
    fn an_empty_table_is_never_called_stale() {
        // Zero leases with a dead writer and zero with a healthy one are the same
        // number from here, and only one of them is a fault (module header).
        assert_eq!(stale(0, 0, Some(1_000_000), 86_400), None);
        assert_eq!(stale(0, 0, None, 86_400), None);
    }

    #[test]
    fn the_json_keys_carry_the_values_they_are() {
        // `concat!` matches keys to arguments **by position**: a key list and an
        // argument list that drift apart still compile, and every value after the
        // insertion is simply somebody else's.  That is what happened on this
        // endpoint on 2026-10-31, when `stale` answered `86389` — a `max_left`
        // — and the leases page bannered a reason out of a countdown.  So the
        // adjacency is pinned, with the fixture's own numbers, rather than the
        // payload: a shift by one breaks it, a reordering of keys has to be said
        // here out loud.
        let path = std::env::temp_dir().join("network-status-leases-json");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is past 1970")
            .as_secs() as i64;
        // Two live leases, one inside a labelled block and one outside every
        // block, plus dnsmasq's own header line: `total` 2, `skipped` 1,
        // `unknown_ttl` 1, nothing lapsed, nothing generated.
        std::fs::write(
            &path,
            format!(
                "duid 00:01:00:01:32:6b:9c:78:00:25:90:d3:f0:ea\n\
                 {} aa:bb:cc:dd:ee:ff 10.3.2.50 laptop *\n\
                 {} aa:bb:cc:dd:ee:01 100.64.0.9 tailnet *\n",
                now + 1_000,
                now + 200
            ),
        )
        .expect("temp file");
        let q = Query::default();
        let body = json(
            path.to_str().expect("utf-8 path"),
            &q,
            &q,
            now,
            &local(),
            86_400,
            500,
        )
        .expect("the fixture reads");
        let _ = std::fs::remove_file(&path);

        assert!(
            body.contains(r#""total":2,"skipped":1,"matched":2,"returned":2,"expired":0,"unknown_ttl":1,"generated":0,"#),
            "{body}"
        );
        // Fresh by construction, so the three channel keys are these values and
        // no one else's.
        assert!(body.contains(r#""stale":null"#), "{body}");
        assert!(body.contains(r#""source_age":0"#), "{body}");
    }

    #[test]
    fn the_query_defaults_and_clamps() {
        let defaults = Query::default();
        let q = parse(&HashMap::new(), &defaults);
        assert_eq!(q, defaults);
        assert_eq!(q.max_left, -1, "every lease, expired ones included");
        assert_eq!(q.sort, Sort::Expires);

        let mut p = HashMap::new();
        p.insert("limit".to_string(), "100000".to_string());
        p.insert("sort".to_string(), "^name".to_string());
        p.insert("q".to_string(), "  Laptop ".to_string());
        p.insert("vlan".to_string(), "vlan226".to_string());
        let q = parse(&p, &defaults);
        assert_eq!(q.limit, defaults.limit, "clamped to what was allowed");
        assert_eq!(q.sort, Sort::Name);
        assert!(
            q.reverse,
            "^ is the connection table's spelling of backwards"
        );
        assert_eq!(q.needle.as_deref(), Some("laptop"));
        assert_eq!(q.vlan.as_deref(), Some("vlan226"));
    }

    #[test]
    fn the_sort_names_round_trip() {
        for s in [
            Sort::Expires,
            Sort::Issued,
            Sort::Name,
            Sort::Vlan,
            Sort::Addr,
        ] {
            assert_eq!(Sort::parse(s.as_str()), s);
        }
        assert_eq!(Sort::parse("^issued"), Sort::Issued);
        assert_eq!(
            Sort::parse("nonsense"),
            Sort::Expires,
            "an unknown sort is the default, not an error"
        );
    }

    #[test]
    fn sorting_is_total_and_breaks_ties_on_address() {
        let a = row(1_000_000, "10.3.2.9", "same", "aa");
        let b = row(1_000_000, "10.3.2.8", "same", "bb");
        let mut rows = [a.clone(), b.clone()];
        rows.sort_by(|x, y| x.cmp(y, Sort::Name));
        assert_eq!(
            rows[0].lease.ip, b.lease.ip,
            "same name: the lower address wins"
        );
        let mut rows = [a.clone(), b.clone()];
        rows.sort_by(|x, y| x.cmp(y, Sort::Addr));
        assert_eq!(rows[0].lease.ip, b.lease.ip);
    }

    #[test]
    fn a_needle_matches_any_field_except_the_bar() {
        let r = row(1_000_000, "10.3.2.50", "laptop", "aa:bb:cc:dd:ee:ff");
        assert!(r.matches("laptop"));
        assert!(r.matches("10.3.2"));
        assert!(r.matches("aa:bb"));
        assert!(r.matches("vlan226"));
        assert!(!r.matches("vlan225"));
    }
}
