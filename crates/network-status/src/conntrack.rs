//! `/proc/net/nf_conntrack`'s format, and nothing else — the parsing half of the
//! connection table.  The tracker built on top of it is `flows.rs`, which owns
//! the *why* of the four properties below.
//!
//! Why conntrack at all: the router sees both directions of every flow it
//! forwards and the kernel already counts the bytes, so this is the one place
//! the answer lives.  `nft` has no per-flow counters, `ss` only sees this box's
//! own sockets, and a packet capture is a far more privileged thing to run on
//! the network's router than reading a file.
//!
//! Four properties of the source shape everything in `flows.rs`:
//!
//!   * **The counters are opt-in.** The file prints `packets=`/`bytes=` only
//!     while `net.netfilter.nf_conntrack_acct = 1`, and counts only for
//!     connections created *after* it was switched on.  A row without counters
//!     is not an error, so the byte fields are `Option` and
//!     [`crate::flows::Flows::metrics`] exports how many are missing — the
//!     answer to "why is nothing flowing".  `machines/gw/network-status.nix`
//!     turns the sysctl on.
//!   * **The original tuple is pre-SNAT.** For a laptop pulling a file through
//!     this box the entry reads `src=10.3.14.x … src=<server> dst=<tunnel ip>`:
//!     the initiator's tuple keeps the *private* address and only the reply
//!     tuple shows the masqueraded one.  So `src` answers "who" and `dst` "what"
//!     without undoing any NAT — measured on gw, 2026-10-25, against a 2 GB
//!     download from services1.
//!   * **An entry has no start time.** The kernel prints the remaining `timeout`
//!     — which it resets with every packet — and nothing else.  A real age needs
//!     `nf_conntrack_timestamp` *and* netlink, neither of which is available to a
//!     process reading a proc file.  Age in this crate is therefore "how long
//!     the dashboard has been seeing this flow": restart the service and every
//!     age restarts, and a flow's age is off by up to one sample interval.  That
//!     is enough for the question being asked — is this connection sustained, or
//!     is it churn — and the SPA labels it as such.
//!   * **A tuple can be reused.** Ports are ephemeral, so an entry can die and a
//!     new connection arrive with identical five fields.  Counters going
//!     backwards therefore restarts the flow's history
//!     ([`crate::flows::Flows::observe`]), exactly as [`crate::model::rate`]
//!     treats an interface counter that goes backwards after a reboot.

use std::net::IpAddr;

// ---------------------------------------------------------------------------
// One line of /proc/net/nf_conntrack
// ---------------------------------------------------------------------------

/// One direction of a connection, as the kernel printed it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tuple {
    pub(crate) src: Option<IpAddr>,
    pub(crate) dst: Option<IpAddr>,
    pub(crate) sport: Option<u16>,
    pub(crate) dport: Option<u16>,
    pub(crate) packets: Option<u64>,
    pub(crate) bytes: Option<u64>,
    /// ICMP's `type=` (an ICMP tuple has no ports; `id`/`code` are ignored).
    pub(crate) icmp_type: Option<u16>,
}

/// A connection as the kernel's table describes it at one instant.
#[derive(Clone, Debug, PartialEq)]
pub struct ConnRow {
    /// The kernel's protocol name: `tcp`, `udp`, `icmp`, `sctp`, …
    pub(crate) proto: String,
    /// TCP's state word.  UDP and ICMP have none, hence `None` — which is also
    /// what an unrecognised tracker prints, and why the SPA falls back to the
    /// protocol rather than showing an empty cell.
    pub(crate) state: Option<String>,
    /// Seconds of timeout left.  **Not an age** (see the module header).
    pub(crate) timeout: u64,
    /// `[ASSURED]`: both directions have been seen.
    pub(crate) assured: bool,
    /// `[OFFLOAD]`: a flow-table entry, whose conntrack counters only catch up
    /// when the flow is refreshed.  gw runs no flowtable, so today this is a
    /// label nobody has had to use — but if offload ever appears it is the
    /// reason a saturating flow can look idle.
    pub(crate) offloaded: bool,
    /// The initiating direction.  Pre-SNAT, so for a LAN client this is the
    /// private address (module header).
    pub(crate) orig: Tuple,
    pub(crate) reply: Tuple,
}

impl ConnRow {
    pub(crate) fn v6(&self) -> bool {
        self.orig.src.is_some_and(|ip| ip.is_ipv6())
    }
}

/// Parse one `/proc/net/nf_conntrack` line.
///
/// A scan of `key=value` tokens rather than a positional split, because how many
/// fields come before the first tuple is not fixed: TCP prints a state word and
/// UDP does not, and protocol trackers keep adding fields of their own.  What is
/// fixed is that the original tuple comes first and that each tuple starts with
/// its own `src=`.
pub(crate) fn parse_row(line: &str) -> Option<ConnRow> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    // "ipv4 2 tcp 6 119 TIME_WAIT src=…"
    if toks.len() < 6 || (toks[0] != "ipv4" && toks[0] != "ipv6") {
        return None;
    }
    let first = toks.iter().position(|t| t.starts_with("src="))?;
    if first < 4 {
        return None;
    }
    let proto = toks[2].to_string();

    // Between the protocol number and the first tuple: the timeout, and for TCP
    // a state word.  Which is which is by "does it parse as a number", because
    // that is the only thing the kernel's own printers agree on.
    let mut timeout = 0u64;
    let mut state = None;
    for t in &toks[4..first] {
        if let Ok(n) = t.parse::<u64>() {
            timeout = n;
        } else if state.is_none() {
            state = Some(t.to_string());
        }
    }

    let mut row = ConnRow {
        proto,
        state,
        timeout,
        assured: false,
        offloaded: false,
        orig: Tuple::default(),
        reply: Tuple::default(),
    };
    let mut tuple = 0usize;
    for t in &toks[first..] {
        // Status flags are bare `[NAME]`s.
        if let Some(flag) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            match flag {
                "ASSURED" => row.assured = true,
                "OFFLOAD" => row.offloaded = true,
                _ => {}
            }
            continue;
        }
        let Some((k, v)) = t.split_once('=') else { continue };
        if k == "src" {
            tuple += 1;
            if tuple > 2 {
                return None;
            }
        }
        let t = if tuple == 2 { &mut row.reply } else { &mut row.orig };
        match k {
            "src" => t.src = v.parse().ok(),
            "dst" => t.dst = v.parse().ok(),
            "sport" => t.sport = v.parse().ok(),
            "dport" => t.dport = v.parse().ok(),
            "packets" => t.packets = v.parse().ok(),
            "bytes" => t.bytes = v.parse().ok(),
            "type" => t.icmp_type = v.parse().ok(),
            // mark, zone, use, label, helpers, expires, seqflags, ip=ID, …
            _ => {}
        }
    }
    if tuple < 2 || row.orig.src.is_none() || row.orig.dst.is_none() {
        return None;
    }
    Some(row)
}

/// The whole table, and the count of lines that would not parse.  An unparsed
/// line is reported rather than assumed to be a header: the header lines are
/// `cat`'d by nothing here (this is not `/proc/net/dev`), so every line that
/// reaches [`parse_row`] should be a connection.
pub fn parse_table(raw: &str) -> (Vec<ConnRow>, usize) {
    let mut rows = Vec::new();
    let mut skipped = 0usize;
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match parse_row(line) {
            Some(row) => rows.push(row),
            None => skipped += 1,
        }
    }
    (rows, skipped)
}

/// The key a connection is tracked by: protocol plus the original tuple, which
/// is what makes it unique in the kernel's own table.
pub(crate) fn flow_key(row: &ConnRow) -> String {
    let t = &row.orig;
    let mut key = String::with_capacity(48);
    key.push_str(&row.proto);
    key.push('|');
    key.push_str(&t.src.map(|i| i.to_string()).unwrap_or_default());
    key.push('|');
    key.push_str(&t.dst.map(|i| i.to_string()).unwrap_or_default());
    key.push('|');
    match (t.sport, t.dport) {
        (Some(s), Some(d)) => {
            key.push_str(&s.to_string());
            key.push('/');
            key.push_str(&d.to_string());
        }
        // ICMP (and anything else with no ports): the type distinguishes a
        // ping's request entry from its reply entry.
        _ => key.push_str(&t.icmp_type.map(|n| n.to_string()).unwrap_or_default()),
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row below was read out of gw's `/proc/net/nf_conntrack` on
    /// 2026-10-25 (the `[OFFLOAD]` row is the one exception: gw runs no
    /// flowtable, so that flag is constructed from the kernel's own syntax).
    const DOWNLOAD: &str = "ipv4     2 tcp      6 431999 ESTABLISHED src=10.3.1.20 dst=104.16.246.239 sport=58506 dport=443 packets=535 bytes=51878 src=104.16.246.239 dst=81.187.223.123 sport=443 dport=58506 packets=1828 bytes=2382711 [ASSURED] mark=0 zone=0 use=2";
    const DNS: &str = "ipv4     2 udp      17 4 src=10.3.1.30 dst=10.3.1.1 sport=51522 dport=53 packets=2 bytes=128 src=10.3.1.1 dst=10.3.1.30 sport=53 dport=51522 packets=2 bytes=251 mark=0 zone=0 use=2";
    const UNACCOUNTED: &str = "ipv4     2 tcp      6 75890 ESTABLISHED src=10.3.1.30 dst=10.3.14.13 sport=60486 dport=554 src=10.3.14.13 dst=10.3.1.30 sport=554 dport=60486 [ASSURED] mark=0 zone=0 use=2";
    const PING: &str = "ipv4     2 icmp     1 28 src=10.3.1.20 dst=1.0.0.1 type=8 code=0 id=17139 packets=2 bytes=168 src=1.0.0.1 dst=81.187.223.123 type=0 code=0 id=17139 packets=2 bytes=168 [ASSURED] mark=0 zone=0 use=2";
    const PING6: &str = "ipv6     10 icmpv6   58 29 src=2001:08b0:1d14:0225:0000:0000:0000:0001 dst=2606:4700:4700:0000:0000:0000:0000:1111 type=128 code=0 id=37128 packets=2 bytes=208 src=2606:4700:4700:0000:0000:0000:0000:1111 dst=2001:08b0:1d14:0225:0000:0000:0000:0001 type=129 code=0 id=37128 packets=2 bytes=208 [ASSURED] mark=0 zone=0 use=2";
    const INBOUND: &str = "ipv4     2 udp      17 8 src=192.168.49.1 dst=10.3.1.1 sport=40087 dport=53 packets=1 bytes=81 src=10.3.1.1 dst=192.168.49.1 sport=53 dport=40087 packets=1 bytes=137 mark=0 zone=0 use=2";

    fn row(line: &str) -> ConnRow {
        parse_row(line).unwrap_or_else(|| panic!("row should parse: {line}"))
    }

    #[test]
    fn the_two_tuples_split_on_their_second_src() {
        let r = row(DOWNLOAD);
        assert_eq!(r.proto, "tcp");
        assert_eq!(r.orig.src.map(|i| i.to_string()), Some("10.3.1.20".into()));
        assert_eq!(r.orig.dst.map(|i| i.to_string()), Some("104.16.246.239".into()));
        assert_eq!(r.orig.sport, Some(58506));
        assert_eq!(r.orig.dport, Some(443));
        // The reply tuple shows the masqueraded address, which is why the
        // *original* tuple is the one that says who the connection belongs to.
        assert_eq!(r.reply.src.map(|i| i.to_string()), Some("104.16.246.239".into()));
        assert_eq!(r.reply.dst.map(|i| i.to_string()), Some("81.187.223.123".into()));
        assert_eq!(r.orig.bytes, Some(51_878));
        assert_eq!(r.reply.bytes, Some(2_382_711));
        assert_eq!(r.reply.packets, Some(1828));
        assert!(r.assured);
        assert_eq!(r.timeout, 431_999, "the timeout left, not an age");
    }

    #[test]
    fn counters_are_per_row_optional_not_missing() {
        // A connection that began before `net.netfilter.nf_conntrack_acct` was
        // on has no counters and must not be reported as a zero-byte one.
        let r = row(UNACCOUNTED);
        assert_eq!(r.orig.bytes, None);
        assert_eq!(r.reply.bytes, None);
        assert_eq!(r.orig.packets, None);
        assert!(r.assured, "everything else about the row is fine");
    }

    #[test]
    fn only_tcp_has_a_state_word() {
        assert_eq!(row(DOWNLOAD).state.as_deref(), Some("ESTABLISHED"));
        assert_eq!(row(DNS).state, None, "udp: the number is the timeout");
        assert_eq!(row(DNS).timeout, 4);
        assert_eq!(row(PING).state, None);
        assert_eq!(row(PING).timeout, 28);
    }

    #[test]
    fn icmp_has_types_where_other_protocols_have_ports() {
        let r = row(PING);
        assert_eq!(r.orig.sport, None);
        assert_eq!(r.orig.dport, None);
        assert_eq!(r.orig.icmp_type, Some(8), "echo-request");
        assert_eq!(r.reply.icmp_type, Some(0), "echo-reply");
        assert!(!r.v6());

        let r6 = row(PING6);
        assert_eq!(r6.proto, "icmpv6");
        assert_eq!(r6.orig.icmp_type, Some(128), "the v6 echo-request number");
        assert!(r6.v6());
    }

    #[test]
    fn flags_are_read_and_strangers_ignored() {
        assert!(row(DOWNLOAD).assured);
        assert!(!row(DNS).assured, "one-way so far: no [ASSURED]");
        let offload = DNS.replace("mark=0", "[OFFLOAD] mark=0");
        let r = row(&offload);
        assert!(r.offloaded);
        assert_eq!(r.orig.bytes, Some(128), "a flag must not shift the fields");
    }

    #[test]
    fn addresses_normalise_so_two_spellings_are_one_connection() {
        // The kernel pads IPv6 to `2001:08b0:…`; `/etc/dnsmasq-records.hosts`
        // and the API both write `2001:8b0:…`.  Parsing to `IpAddr` and
        // formatting with `Display` is what makes them agree.
        let r = row(PING6);
        assert_eq!(
            r.orig.src.map(|i| i.to_string()),
            Some("2001:8b0:1d14:225::1".into())
        );
        let short = PING6.replace(
            "src=2001:08b0:1d14:0225:0000:0000:0000:0001",
            "src=2001:8b0:1d14:225::1",
        );
        assert_eq!(flow_key(&r), flow_key(&row(&short)));
    }

    #[test]
    fn flow_keys_separate_what_the_kernel_separates() {
        let base = row(DOWNLOAD);
        assert_eq!(flow_key(&base), flow_key(&row(DOWNLOAD)));
        let other_port = DOWNLOAD.replace("dport=443", "dport=8443");
        assert_ne!(flow_key(&base), flow_key(&row(&other_port)));
        let other_host = DOWNLOAD.replace("src=10.3.1.20", "src=10.3.1.21");
        assert_ne!(flow_key(&base), flow_key(&row(&other_host)));
        // A ping's request and reply entries must not collide.
        assert_ne!(flow_key(&row(PING)), flow_key(&row(&PING.replace("type=8", "type=4"))));
        assert_ne!(flow_key(&row(PING)), flow_key(&row(DNS)));
    }

    #[test]
    fn rows_that_are_not_rows_are_counted() {
        let table = format!(
            "{DOWNLOAD}\n{DNS}\nnonsense\nipv4 2 tcp 6 1 ESTABLISHED src=10.3.1.1 dst=\n\n{INBOUND}\n"
        );
        let (rows, unparsed) = parse_table(&table);
        assert_eq!(rows.len(), 3);
        assert_eq!(unparsed, 2, "the junk line and the half-written tuple");
        // The truncated line has one complete tuple and no reply tuple, so it is
        // refused rather than shown as a connection with no reply.
        assert_eq!(row(INBOUND).orig.src.map(|i| i.to_string()), Some("192.168.49.1".into()));
    }

    #[test]
    fn an_empty_table_is_a_table() {
        // `cat /proc/net/nf_conntrack 2>/dev/null || true` prints nothing when
        // conntrack is not loaded, and `--no-conns` prints no section at all.
        let (rows, unparsed) = parse_table("");
        assert!(rows.is_empty());
        assert_eq!(unparsed, 0);
    }
}
