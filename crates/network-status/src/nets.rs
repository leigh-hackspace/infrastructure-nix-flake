//! Which addresses count as "ours" — the CIDR matching that decides the
//! direction of every connection the table shows.

use std::net::IpAddr;

// ---------------------------------------------------------------------------
// "Is this address one of ours?"
// ---------------------------------------------------------------------------

/// An IPv4 or IPv6 prefix.  The family is part of the value: an IPv6 `/0` must
/// not match an IPv4 address however convenient the shared bit pattern looks.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    pub(crate) net: Net,
    /// The prefix as it was given on the command line, for `/api/config`.
    pub(crate) label: String,
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

/// The prefixes that count as "inside the network" — the VLANs, which is
/// `common/vlans.nix` rendered into repeated `--local-prefix` flags by
/// `machines/gw/network-status.nix`.
#[derive(Clone, Debug, Default)]
pub struct Localness {
    pub(crate) entries: Vec<Entry>,
}

impl Localness {
    /// Parses every prefix and keeps the ones that parsed.  A prefix that did
    /// not parse is the operator's mistake and says so once, on stderr, at
    /// startup rather than by silently classifying half the network as external.
    pub fn new(specs: &[String]) -> Self {
        let mut entries = Vec::new();
        for spec in specs {
            match Net::parse(spec) {
                Some(net) => entries.push(Entry {
                    net,
                    label: spec.clone(),
                }),
                None => eprintln!("network-status: ignoring --local-prefix {spec:?}: expected CIDR like 10.3.1.0/24"),
            }
        }
        Self { entries }
    }

    pub fn is_local(&self, ip: IpAddr) -> bool {
        self.entries.iter().any(|e| e.net.matches(ip))
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
        s.parse().unwrap_or_else(|_| panic!("{s} should be an address"))
    }

    #[test]
    fn v4_prefixes_match_their_own_block_only() {
        let lan = net("10.3.1.0/24");
        assert!(lan.matches(ip("10.3.1.1")));
        assert!(lan.matches(ip("10.3.1.254")));
        assert!(!lan.matches(ip("10.3.2.1")), "the wireless VLAN is not the wired one");
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
        for bad in ["10.3.1.0", "10.3.1.0/33", "not-an-ip/8", "10.3.1.0/-1", "/24", "10.3.1.0/"] {
            assert!(Net::parse(bad).is_none(), "{bad} should be refused");
        }
    }

    #[test]
    fn unparsable_prefixes_are_dropped_and_said_so() {
        // A typo in `--local-prefix` must shrink the set rather than silently
        // match everything, and it must be reported (stderr, once, at startup).
        let specs: Vec<String> = ["10.3.1.0/24", "10.3.2.0/33"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let local = Localness::new(&specs);
        assert_eq!(local.specs(), vec!["10.3.1.0/24"]);
        assert!(local.is_local(ip("10.3.1.9")));
        assert!(!local.is_local(ip("10.3.2.9")));
    }
}
