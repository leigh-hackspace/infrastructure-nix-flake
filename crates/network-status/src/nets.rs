//! Which addresses count as "ours" — the CIDR matching that decides the
//! direction of every connection the table shows, and which VLAN an address was
//! handed out on, which is what the lease table groups by.  One list of prefixes
//! answers both questions, so it is labelled once (`--vlan vlan225=10.3.1.0/24`)
//! rather than kept twice in two syntaxes.

use std::net::IpAddr;

// ---------------------------------------------------------------------------
// "Is this address one of ours?"
// ---------------------------------------------------------------------------

/// An IPv4 or IPv6 prefix, and the VLAN it belongs to when it came in labelled.
/// The family is part of the value: an IPv6 `/0` must not match an IPv4 address
/// however convenient the shared bit pattern looks.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    pub(crate) net: Net,
    /// The prefix as it was given on the command line, for `/api/config` and
    /// the startup line.  For a `--vlan` spec this is the CIDR half, so the
    /// config keeps reading as addresses rather than as VLAN names.
    pub(crate) label: String,
    /// `Some` only for a `--vlan vlan225=10.3.1.0/24` entry; a plain
    /// `--local-prefix` knows no VLAN, which is every other box's case.
    pub(crate) vlan: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Net {
    pub(crate) base: u128,
    pub(crate) mask: u128,
    pub(crate) v6: bool,
}

pub(crate) fn bits_of(ip: IpAddr) -> u128 {
    match ip {
        // IPv4 in the low 32 bits, so `10.3.1.0/24` reads as itself in a
        // debugger rather than as a v4-mapped address.
        IpAddr::V4(v4) => u128::from(u32::from(v4)),
        IpAddr::V6(v6) => u128::from(v6),
    }
}

pub(crate) fn mask_for(bits: u32, width: u32) -> u128 {
    if bits == 0 {
        0
    } else if bits >= width {
        u128::MAX
    } else {
        u128::MAX << (width - bits)
    }
}

impl Net {
    /// `10.3.1.0/24` or `2001:8b0:1d14:225::/64`.  Host bits are ignored, so
    /// `10.3.1.1/24` names the same prefix as `10.3.1.0/24`.
    fn parse(s: &str) -> Option<Net> {
        let (addr, bits) = s.trim().split_once('/')?;
        let addr: IpAddr = addr.trim().parse().ok()?;
        let bits: u32 = bits.trim().parse().ok()?;
        let width = if addr.is_ipv6() { 128 } else { 32 };
        if bits > width {
            return None;
        }
        let mask = mask_for(bits, width);
        Some(Net {
            base: bits_of(addr) & mask,
            mask,
            v6: addr.is_ipv6(),
        })
    }

    fn matches(&self, ip: IpAddr) -> bool {
        self.v6 == ip.is_ipv6() && bits_of(ip) & self.mask == self.base
    }
}

/// One command-line prefix spec into an [`Entry`]: `named` is the `--vlan` form,
/// whose name half the lease table groups by.  An unparsable spec is `None`, and
/// the caller names the flag it came from, because the flag is the advice.
pub(crate) fn parse_spec(spec: &str, named: bool) -> Option<Entry> {
    if !named {
        return Net::parse(spec).map(|net| Entry {
            net,
            label: spec.to_string(),
            vlan: None,
        });
    }

    // The VLAN name is everything before the first `=`; a spec without one is
    // the operator's typo and refused rather than guessed at.
    let (name, cidr) = spec.split_once("=")?;
    if name.is_empty() || name.contains(" ") {
        return None;
    }
    Net::parse(cidr.trim()).map(|net| Entry {
        net,
        // The CIDR half only: `specs()` feeds `/api/config`'s `local_prefixes`,
        // which has always been a list of addresses and stays one.
        label: cidr.trim().to_string(),
        vlan: Some(name.to_string()),
    })
}

/// The prefixes that count as "inside the network" — the VLANs, which is
/// `common/vlans.nix` rendered into repeated `--local-prefix` (or `--vlan`,
/// labelled) flags by `machines/gw/network-status.nix`.
#[derive(Clone, Debug, Default)]
pub struct Localness {
    pub(crate) entries: Vec<Entry>,
}

impl Localness {
    /// Parses every `--local-prefix` and keeps the ones that parsed.  A prefix
    /// that did not parse is the operator's mistake and says so once, on stderr,
    /// at startup rather than by silently classifying half the network as
    /// external.
    pub fn new(specs: &[String]) -> Self {
        let mut local = Self {
            entries: Vec::new(),
        };
        for spec in specs {
            match parse_spec(spec, false) {
                Some(e) => local.entries.push(e),
                None => eprintln!("network-status: ignoring --local-prefix {spec:?}: expected CIDR like 10.3.1.0/24"),
            }
        }
        local
    }

    /// `--vlan vlan225=10.3.1.0/24`, which is how the router hands the same
    /// blocks over with their VLAN's name so `/api/leases` can say which VLAN a
    /// lease was handed out on.  A VLAN block is inside the network by
    /// definition, so these land in the same list as every plain prefix and the
    /// connection table's direction rules do not change shape; how many parsed
    /// is returned so the startup line can say how many did not.
    pub fn add_named(&mut self, specs: &[String]) -> usize {
        let mut ok = 0;
        for spec in specs {
            match parse_spec(spec, true) {
                Some(e) => {
                    self.entries.push(e);
                    ok += 1;
                }
                None => {
                    eprintln!("network-status: ignoring --vlan {spec:?}: expected VLAN=10.3.1.0/24")
                }
            }
        }
        ok
    }

    pub fn is_local(&self, ip: IpAddr) -> bool {
        self.entries.iter().any(|e| e.net.matches(ip))
    }

    /// The VLAN an address belongs to, from the labelled prefixes: the **narrowest**
    /// match, because a VLAN's own block and a more specific prefix can both
    /// contain one address and the specific one is the better answer.  A tie (two
    /// VLANs claiming one prefix) takes the first given — `check-network-tables`
    /// refuses that state rather than leaving it to this tie-break.
    pub fn vlan_of(&self, ip: IpAddr) -> Option<String> {
        let best = self
            .entries
            .iter()
            .filter(|e| e.vlan.is_some() && e.net.matches(ip))
            .max_by(|a, b| a.net.mask.cmp(&b.net.mask));
        match best {
            Some(e) => e.vlan.clone(),
            None => None,
        }
    }

    /// The network an address falls in, as the prefix was written on the command
    /// line (`10.3.2.0/24`) — the **narrowest** match, for the same reason
    /// [`Self::vlan_of`] takes the narrowest: a VLAN's block and a more specific
    /// prefix can both contain one address, and the specific one is the better
    /// label.  `None` for an address no prefix describes, which the lease table
    /// renders as an empty network rather than as a guess.
    pub fn prefix_of(&self, ip: IpAddr) -> Option<String> {
        let best = self
            .entries
            .iter()
            .filter(|e| e.net.matches(ip))
            .max_by(|a, b| a.net.mask.cmp(&b.net.mask));
        best.map(|e| e.label.clone())
    }

    /// Every VLAN name the router was told about, deduped and sorted — the
    /// options the lease table's VLAN filter offers, including a VLAN whose
    /// leases have all aged out and so appears in no row.
    pub fn vlan_labels(&self) -> Vec<String> {
        let mut names: Vec<String> = self.entries.iter().flat_map(|e| e.vlan.clone()).collect();
        names.sort();
        names.dedup();
        names
    }

    /// The prefixes as they were given, for `/api/config` and the startup line.
    /// A prefix that did not parse is not here, and `new` said so once.
    pub fn specs(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.label.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(spec: &str) -> Net {
        Net::parse(spec).unwrap_or_else(|| panic!("{spec} should parse"))
    }

    fn ip(s: &str) -> IpAddr {
        s.parse()
            .unwrap_or_else(|_| panic!("{s} should be an address"))
    }

    fn specs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn v4_prefixes_match_their_own_block_only() {
        let lan = net("10.3.1.0/24");
        assert!(lan.matches(ip("10.3.1.1")));
        assert!(lan.matches(ip("10.3.1.254")));
        assert!(
            !lan.matches(ip("10.3.2.1")),
            "the wireless VLAN is not the wired one"
        );
        assert!(!lan.matches(ip("10.3.0.1")));
        assert!(!lan.matches(ip("192.168.2.1")));
    }

    #[test]
    fn host_bits_in_a_prefix_are_ignored() {
        // `--local-prefix 10.3.1.1/24` is the same prefix as /24 and the same
        // one the flake would have written; refusing it would only be a trap.
        assert_eq!(net("10.3.1.1/24"), net("10.3.1.0/24"));
        assert!(net("10.3.1.1/24").matches(ip("10.3.1.77")));
    }

    #[test]
    fn v6_prefixes_match_on_their_own_family() {
        let lan = net("2001:8b0:1d14:225::/64");
        assert!(lan.matches(ip("2001:8b0:1d14:225::1")));
        assert!(lan.matches(ip("2001:08b0:1d14:0225:0000:0000:0000:1001")));
        assert!(!lan.matches(ip("2001:8b0:1d14:226::1")));
        assert!(!lan.matches(ip("fd99:dead:beef:225d::1")));
    }

    #[test]
    fn a_prefix_never_crosses_families() {
        // The reason `v6` is part of `Net` rather than a comment: `::/0` covers
        // every IPv6 address and must not be read as every IPv4 one.
        let all_v6 = net("::/0");
        let all_v4 = net("0.0.0.0/0");
        assert!(all_v6.matches(ip("2001:8b0::1")));
        assert!(!all_v6.matches(ip("10.3.1.1")));
        assert!(all_v4.matches(ip("10.3.1.1")));
        assert!(!all_v4.matches(ip("2001:8b0::1")));
    }

    #[test]
    fn single_addresses_and_bad_prefixes() {
        assert!(net("10.3.1.1/32").matches(ip("10.3.1.1")));
        assert!(!net("10.3.1.1/32").matches(ip("10.3.1.2")));
        for bad in [
            "10.3.1.0",
            "10.3.1.0/33",
            "not-an-ip/8",
            "10.3.1.0/-1",
            "/24",
            "10.3.1.0/",
        ] {
            assert!(Net::parse(bad).is_none(), "{bad} should be refused");
        }
    }

    #[test]
    fn unparsable_prefixes_are_dropped_and_said_so() {
        // A typo in `--local-prefix` must shrink the set rather than silently
        // match everything, and it must be reported (stderr, once, at startup).
        let local = Localness::new(&specs(&["10.3.1.0/24", "10.3.2.0/33"]));
        assert_eq!(local.specs(), vec!["10.3.1.0/24"]);
        assert!(local.is_local(ip("10.3.1.9")));
        assert!(!local.is_local(ip("10.3.2.9")));
    }

    #[test]
    fn a_named_prefix_is_the_same_prefix_and_a_vlan_name() {
        // The `--vlan` form, as gw is handed it: dnsmasq's own VLAN tag plus the
        // block that range is handed out of.
        let named: Vec<String> = [
            "vlan225=10.3.1.0/24",
            "vlan226=10.3.2.0/24",
            "vlan225=2001:8b0:1d14:225::/64",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut local = Localness::new(&[]);
        assert_eq!(local.add_named(&named), 3, "every one of those parses");
        assert!(
            local.is_local(ip("10.3.1.55")),
            "a VLAN block is inside the network whatever it is called by"
        );
        assert!(local.is_local(ip("2001:8b0:1d14:225::9")));
        assert_eq!(local.vlan_of(ip("10.3.2.200")), Some("vlan226".to_string()));
        assert_eq!(
            local.vlan_of(ip("2001:8b0:1d14:225::1")),
            Some("vlan225".to_string())
        );
        assert_eq!(
            local.vlan_of(ip("192.168.2.5")),
            None,
            "a tailnet or uplink address has no VLAN of its own"
        );
        assert_eq!(local.vlan_labels(), vec!["vlan225", "vlan226"]);
        assert_eq!(
            local.specs(),
            vec!["10.3.1.0/24", "10.3.2.0/24", "2001:8b0:1d14:225::/64"]
        );
    }

    #[test]
    fn a_bad_named_prefix_is_refused_not_guessed() {
        let named: Vec<String> = [
            "=10.3.1.0/24",
            "vlan 225=10.3.1.0/24",
            "vlan225=10.3.1.0",
            "vlan225",
            "vlan227=10.3.14.0/24",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut local = Localness::new(&[]);
        local.add_named(&named);
        assert_eq!(local.vlan_labels(), vec!["vlan227"]);
    }

    #[test]
    fn the_narrowest_label_wins() {
        // Synthetic on purpose (no real network puts a /28 and a /24 on two
        // VLANs) but the rule is the one a real overlap would hit: the more
        // specific prefix is the better answer, not whichever flag came first.
        let named: Vec<String> = ["wide=10.3.1.0/24", "narrow=10.3.1.16/28"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut local = Localness::new(&[]);
        local.add_named(&named);
        assert_eq!(local.vlan_of(ip("10.3.1.20")), Some("narrow".to_string()));
        assert_eq!(local.vlan_of(ip("10.3.1.4")), Some("wide".to_string()));
        // The plain-prefix view is the CIDR either way: `--vlan` adds a name,
        // it does not change what counts as inside the network.
        assert_eq!(
            local.prefix_of(ip("10.3.1.20")),
            Some("10.3.1.16/28".to_string())
        );
        assert_eq!(local.prefix_of(ip("192.168.2.5")), None);
    }
}
