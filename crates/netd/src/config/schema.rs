//! The schema's value types, and the runtime questions the rest of the daemon
//! asks of a validated config.
//!
//! Reading the document lives in `parse.rs` and cross-field rules in
//! `validate.rs`; this file is only *what the fields are*.

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// small typed values
// ---------------------------------------------------------------------------

/// An IPv4 network, derived from an address and a mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct V4Net {
    pub addr: Ipv4Addr,
    pub prefix: u8,
}

impl V4Net {
    /// The contiguous-mask test: a mask with a hole in it is not a mask, and
    /// dnsmasq would have accepted it as something else entirely.
    pub fn from_mask(addr: Ipv4Addr, mask: Ipv4Addr) -> Option<V4Net> {
        let m = u32::from(mask);
        let prefix = m.leading_ones() as u8;
        let expect = if prefix == 0 {
            0
        } else {
            u32::MAX << (32 - prefix)
        };
        if m != expect {
            return None;
        }
        Some(V4Net {
            addr: Ipv4Addr::from(u32::from(addr) & m),
            prefix,
        })
    }

    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        u32::from(ip) & u32::from(self.mask()) == u32::from(self.addr)
    }

    pub fn mask(&self) -> Ipv4Addr {
        let m = if self.prefix == 0 {
            0
        } else {
            u32::MAX << (32 - self.prefix)
        };
        Ipv4Addr::from(m)
    }

    pub fn broadcast(&self) -> Ipv4Addr {
        Ipv4Addr::from(u32::from(self.addr) | !u32::from(self.mask()))
    }

    /// `10.3.1.0/24`, the spelling every table and doc in this repo uses.
    pub fn cidr(&self) -> String {
        format!("{}/{}", self.addr, self.prefix)
    }
}

/// An IPv6 prefix. Only `/64` is representable: this daemon advertises
/// stateless autoconfiguration, and a client handed a prefix of any other
/// length cannot form an address from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RaNet {
    pub addr: Ipv6Addr,
    pub prefix: u8,
}

impl RaNet {
    pub fn parse(s: &str) -> Result<RaNet, String> {
        let (a, p) = s
            .split_once('/')
            .ok_or_else(|| format!("{s}: an IPv6 prefix needs a length, e.g. 2001:db8::/64"))?;
        let addr: Ipv6Addr = a
            .parse()
            .map_err(|_| format!("{s}: {a} is not an IPv6 address"))?;
        let prefix: u8 = p
            .parse()
            .map_err(|_| format!("{s}: {p} is not a prefix length"))?;
        if prefix != 64 {
            return Err(format!(
                "{s}: only /64 prefixes are advertised — this is stateless autoconfiguration, and a non-64 length would give clients an address they cannot use"
            ));
        }
        if u128::from(addr) & ((1u128 << (128 - prefix)) - 1) != 0 {
            return Err(format!(
                "{s}: not a network address — the bits below /{prefix} must be zero"
            ));
        }
        Ok(RaNet { addr, prefix })
    }
}

/// A MAC as it appears on the wire.
pub type Mac = [u8; 6];

pub fn mac_to_string(m: &Mac) -> String {
    m.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

pub fn mac_parse(s: &str) -> Result<Mac, String> {
    let parts: Vec<&str> = s.split([':', '-']).collect();
    if parts.len() != 6 {
        return Err(format!("{s}: a MAC needs six octets"));
    }
    let mut out = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        out[i] = u8::from_str_radix(p, 16).map_err(|_| format!("{s}: {p} is not a hex octet"))?;
    }
    Ok(out)
}

/// A DHCP client identifier, kept as bytes because that is what the wire
/// carries: `common/dns-records.nix` names them as hex, and the first byte is a
/// hardware type rather than text.
pub type ClientId = Vec<u8>;

pub fn client_id_hex(c: &ClientId) -> String {
    c.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

pub fn client_id_parse(s: &str) -> Result<ClientId, String> {
    let parts: Vec<&str> = s.split(':').collect();
    let mut out = Vec::with_capacity(parts.len());
    for p in parts {
        out.push(u8::from_str_radix(p, 16).map_err(|_| format!("{s}: {p} is not a hex octet"))?);
    }
    if out.is_empty() {
        return Err(format!("{s}: empty client id"));
    }
    Ok(out)
}

pub fn hex_parse(s: &str) -> Result<Vec<u8>, String> {
    let clean: String = s
        .chars()
        .filter(|c| *c != ':' && *c != '-' && !c.is_whitespace())
        .collect();
    if !clean.len().is_multiple_of(2) {
        return Err(format!("{s}: hex needs an even number of digits"));
    }
    let mut out = Vec::with_capacity(clean.len() / 2);
    for i in (0..clean.len()).step_by(2) {
        out.push(
            u8::from_str_radix(&clean[i..i + 2], 16)
                .map_err(|_| format!("{s}: not hex at digit {}", i + 1))?,
        );
    }
    Ok(out)
}

/// Lower-case and strip the trailing dot: the canonical form every name in this
/// daemon is stored and looked up in, whether it came from the table or off the
/// wire.
pub fn canonical_name(s: &str) -> String {
    s.trim().trim_end_matches('.').to_ascii_lowercase()
}

// ---------------------------------------------------------------------------
// the document
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Config {
    pub version: u64,
    pub zone: String,
    pub state_dir: PathBuf,
    pub api: Api,
    pub dns: Dns,
    pub dhcp: Dhcp,
    pub ra: Ra,
}

#[derive(Clone, Debug)]
pub struct Api {
    pub bind: IpAddr,
    pub port: u16,
}

#[derive(Clone, Debug)]
pub struct Dns {
    /// `0.0.0.0:53` / `[::]:53` — sockets, not interface names. This box has no
    /// business answering on the uplink, and `acl` below is a *source* rule,
    /// which is the check that also holds for a spoofed source arriving on a
    /// VLAN.
    pub listen: Vec<String>,
    /// The sources a query is answered from: loopback plus every VLAN block,
    /// i.e. the same "local network" `common/config.nix` gates the front door
    /// with.
    pub acl: Vec<V4Net>,
    pub acl6: Vec<(Ipv6Addr, u8)>,
    /// Every name this box is authoritative for, fully qualified. The bare-label
    /// form is a second entry generated from the same record, because dnsmasq's
    /// hosts files answered both and monitoring asks for the FQDN.
    pub records: Vec<Named>,
    pub forwarders: Vec<IpAddr>,
    /// `ts.leighhack.org` goes to the tailnet's resolver and nowhere else.
    pub conditional: Vec<Conditional>,
    pub cache_size: usize,
    pub ttl: u32,
    pub forward_max: usize,
    pub domain_needed: bool,
    pub stop_rebind: bool,
    pub rebind_ok: Vec<String>,
    pub rebind_localhost_ok: bool,
    pub soa: Soa,
}

#[derive(Clone, Debug)]
pub struct Named {
    /// Canonical: lower-case, no trailing dot.
    pub name: String,
    pub addresses: Vec<IpAddr>,
}

#[derive(Clone, Debug)]
pub struct Conditional {
    pub zone: String,
    pub servers: Vec<IpAddr>,
}

#[derive(Clone, Debug)]
pub struct Soa {
    pub mname: String,
    pub rname: String,
    pub refresh: u32,
    pub retry: u32,
    pub expire: u32,
    pub minimum: u32,
}

#[derive(Clone, Debug)]
pub struct Dhcp {
    pub interfaces: Vec<String>,
    pub lease_ttl: u32,
    /// How long an address a client DECLINEd is held back. A window rather than
    /// dnsmasq's "until the lease file is rewritten": it cannot hold a good
    /// address hostage for a day because one device lied.
    pub decline_quarantine: u32,
    /// Addresses inside a pool that must never be handed out although no
    /// reservation can name them (`10.3.1.166`, the intercom — the story is in
    /// `machines/gw/dns.nix`).
    pub ignored_addresses: BTreeSet<Ipv4Addr>,
    pub ignored_names: BTreeSet<String>,
    pub pools: Vec<Pool>,
    pub reservations: Vec<Reservation>,
    pub options: Vec<DhcpOption>,
    pub netboot: Vec<Netboot>,
}

#[derive(Clone, Debug)]
pub struct Pool {
    pub interface: String,
    /// The tag a pool's leases carry and `options` are matched against. Equal to
    /// the interface name for every VLAN here, but kept separate so the two
    /// cannot be silently conflated.
    pub tag: String,
    pub from: Ipv4Addr,
    pub to: Ipv4Addr,
    pub net: V4Net,
    pub server_ip: Ipv4Addr,
    pub ttl: u32,
}

#[derive(Clone, Debug)]
pub struct Reservation {
    /// The device's name, which is also what `dhcp-fqdn` makes answer as
    /// `<name>.<zone>` from the lease.
    pub name: String,
    pub address: Ipv4Addr,
    pub macs: Vec<Mac>,
    pub client_ids: Vec<ClientId>,
}

#[derive(Clone, Debug)]
pub struct DhcpOption {
    /// `None` = every request. `Some(tag)` = requests whose tag set contains it.
    pub tag: Option<String>,
    pub code: u8,
    pub value: OptionValue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OptionValue {
    Ipv4(Vec<Ipv4Addr>),
    U32(u32),
    Text(String),
    Hex(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct Netboot {
    pub tag: String,
    /// Option 93 (client architecture) carries numbers, option 77 (user class)
    /// carries text; the Pi Room's menu needs both.
    pub match_option: u8,
    pub match_values: Vec<u16>,
    pub match_contains: Option<String>,
    /// The interfaces the variant is offered on — dnsmasq's `tag:vlan227` half
    /// of a `dhcp-boot` line.
    pub when_tags: Vec<String>,
    pub boot: String,
    pub server: Ipv4Addr,
}

#[derive(Clone, Debug)]
pub struct Ra {
    /// Seconds between unsolicited advertisements. One knob for every interface:
    /// the network has always used 60 (`ra-param=…,60,1200`), and a per-VLAN
    /// interval is a fact nobody has ever wanted to differ.
    pub interval: u64,
    pub interfaces: Vec<RaIface>,
}

#[derive(Clone, Debug)]
pub struct RaIface {
    pub interface: String,
    pub prefixes: Vec<RaNet>,
    pub router_lifetime: u16,
    pub valid_lifetime: u32,
    pub preferred_lifetime: u32,
    pub mtu: Option<u32>,
}

// ---------------------------------------------------------------------------
// the questions the rest of the daemon asks
// ---------------------------------------------------------------------------

impl Config {
    /// The pool that hands out leases on this interface, if any.
    pub fn pool_for(&self, interface: &str) -> Option<&Pool> {
        self.dhcp.pools.iter().find(|p| p.interface == interface)
    }

    #[allow(clippy::needless_lifetimes)]
    pub fn reservation_for<'a>(
        &'a self,
        mac: &Mac,
        cid: Option<&ClientId>,
    ) -> Option<&'a Reservation> {
        // The client-id wins, because it is the identifier the *client* chose: a
        // device that presents both and whose MAC changed (a replaced NIC) still
        // keeps its reservation.
        if let Some(cid) = cid {
            if let Some(r) = self
                .dhcp
                .reservations
                .iter()
                .find(|r| r.client_ids.iter().any(|c| c == cid))
            {
                return Some(r);
            }
        }
        self.dhcp
            .reservations
            .iter()
            .find(|r| r.macs.iter().any(|m| m == mac))
    }

    pub fn reservation_at(&self, ip: Ipv4Addr) -> Option<&Reservation> {
        self.dhcp.reservations.iter().find(|r| r.address == ip)
    }

    /// Is this name one no lease may be answered for (`wpad`)? Every Windows
    /// device asks for it, and the old config suppressed it by name.
    pub fn ignored_name(&self, name: &str) -> bool {
        let full = canonical_name(name);
        let label = full.split('.').next().unwrap_or("").to_string();
        self.dhcp.ignored_names.contains(&label)
    }

    /// Every tag a request on `interface` carries before option matching: the
    /// interface itself, which is dnsmasq's per-device tag and the model
    /// `machines/gw/lib/network-tables.nix` renders as `tag:vlan225`.
    pub fn tags_for(&self, interface: &str) -> BTreeSet<String> {
        let mut s = BTreeSet::new();
        s.insert(interface.to_string());
        if let Some(p) = self.pool_for(interface) {
            s.insert(p.tag.clone());
        }
        s
    }

    pub fn in_zone(&self, name: &str) -> bool {
        let n = canonical_name(name);
        n == self.zone || n.ends_with(&format!(".{}", self.zone))
    }
}
