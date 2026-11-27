//! The rules that are about *relationships* between parts of the document.
//!
//! Collected rather than short-circuited: a generator with three mistakes
//! fixed one rebuild at a time is a generator nobody iterates on. Every finding
//! names the field and says what the consequence is, because the reader is
//! someone editing a Nix table at 23:00, not someone reading this file.

use super::schema::{Config, Pool};
use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        let mut bad: Vec<String> = Vec::new();
        self.check_names(&mut bad);
        self.check_tags(&mut bad);
        self.check_pools(&mut bad);
        self.check_reservations(&mut bad);
        self.check_dns(&mut bad);
        self.check_ra(&mut bad);
        if bad.is_empty() {
            Ok(())
        } else {
            Err(bad.join("\n"))
        }
    }

    fn check_names(&self, bad: &mut Vec<String>) {
        if !self.zone.contains('.') {
            bad.push(format!(
                "config.zone: `{}` is not a dotted domain — the zone has to be a name, not a label",
                self.zone
            ));
        }
        if self.api.port == 0 {
            bad.push("api.port: 0 is not a port".into());
        }
    }

    /// A tag that matches nothing is a typo, and the failure it causes is
    /// *silent*: an option nobody matches is a lease with no DNS server.
    fn check_tags(&self, bad: &mut Vec<String>) {
        let mut known: BTreeSet<&str> = BTreeSet::new();
        known.extend(self.dhcp.interfaces.iter().map(String::as_str));
        known.extend(self.dhcp.pools.iter().map(|p| p.interface.as_str()));
        known.extend(self.ra.interfaces.iter().map(|r| r.interface.as_str()));
        for (i, o) in self.dhcp.options.iter().enumerate() {
            if let Some(t) = &o.tag {
                if !known.contains(t.as_str()) {
                    bad.push(format!(
                        "dhcp.options[{i}]: tag `{t}` matches nothing — an option tag is one of the DHCP interfaces ({})",
                        self.dhcp.interfaces.join(", ")
                    ));
                }
            }
        }
        for (i, n) in self.dhcp.netboot.iter().enumerate() {
            for t in &n.when_tags {
                if !known.contains(t.as_str()) {
                    bad.push(format!(
                        "dhcp.netboot[{i}].when_tags: `{t}` matches no interface"
                    ));
                }
            }
        }
    }

    fn check_pools(&self, bad: &mut Vec<String>) {
        for (i, p) in self.dhcp.pools.iter().enumerate() {
            if !self.dhcp.interfaces.contains(&p.interface) {
                bad.push(format!(
                    "dhcp.pools[{i}].interface: `{}` is not in dhcp.interfaces ({})",
                    p.interface,
                    self.dhcp.interfaces.join(", ")
                ));
            }
            if p.from == p.net.addr || p.to == p.net.broadcast() {
                bad.push(format!(
                    "dhcp.pools[{i}]: the range includes the network or broadcast address of {}",
                    p.net.cidr()
                ));
            }
            if p.from == p.server_ip {
                bad.push(format!(
                    "dhcp.pools[{i}]: the range starts at the server's own address ({})",
                    p.server_ip
                ));
            }
            if let Some(other) = self.dhcp.pools.iter().skip(i + 1).find(|o| overlap(p, o)) {
                bad.push(format!(
                    "dhcp.pools: {} and {} overlap — one address cannot be in two pools",
                    p.net.cidr(),
                    other.net.cidr()
                ));
            }
        }
        for a in &self.dhcp.ignored_addresses {
            if !self.dhcp.pools.iter().any(|p| in_range(p, *a)) {
                bad.push(format!(
                    "dhcp.ignored_addresses: {a} is not inside any dynamic range, so nothing would hand it out anyway"
                ));
            }
            if self.reservation_at(*a).is_some() {
                bad.push(format!(
                    "dhcp.ignored_addresses: {a} is both ignored and reserved — one of the two is wrong"
                ));
            }
        }
    }

    fn check_reservations(&self, bad: &mut Vec<String>) {
        let mut seen_addr: BTreeMap<Ipv4Addr, &str> = BTreeMap::new();
        let mut seen_ident: BTreeMap<String, &str> = BTreeMap::new();
        for (i, r) in self.dhcp.reservations.iter().enumerate() {
            if r.macs.is_empty() && r.client_ids.is_empty() {
                bad.push(format!(
                    "dhcp.reservations[{i}] ({}): no MAC and no client-id — a reservation that matches nothing is a dead line",
                    r.name
                ));
            }
            if let Some(prev) = seen_addr.insert(r.address, &r.name) {
                if prev != r.name {
                    bad.push(format!(
                        "dhcp.reservations: {} is claimed by both `{prev}` and `{}`",
                        r.address, r.name
                    ));
                }
            }
            for m in &r.macs {
                let k = format!("mac:{}", super::schema::mac_to_string(m));
                if let Some(prev) = seen_ident.insert(k.clone(), &r.name) {
                    if prev != r.name {
                        bad.push(format!(
                            "dhcp.reservations: {k} is claimed by both `{prev}` and `{}` — the two would race for one address",
                            r.name
                        ));
                    }
                }
            }
            for c in &r.client_ids {
                let k = format!("cid:{}", super::schema::client_id_hex(c));
                if let Some(prev) = seen_ident.insert(k.clone(), &r.name) {
                    if prev != r.name {
                        bad.push(format!(
                            "dhcp.reservations: {k} is claimed by both `{prev}` and `{}`",
                            r.name
                        ));
                    }
                }
            }
            for p in &self.dhcp.pools {
                if in_range(p, r.address) {
                    bad.push(format!(
                        "dhcp.reservations ({}): {} is inside the dynamic range {}–{} of {} — the pool could hand it out; move the reservation out of the range or add it to ignored_addresses",
                        r.name, r.address, p.from, p.to, p.interface
                    ));
                }
            }
        }
    }

    fn check_dns(&self, bad: &mut Vec<String>) {
        if self.dns.listen.is_empty() {
            bad.push("dns.listen: empty — a resolver that binds nothing answers nothing".into());
        }
        if self.dns.acl.is_empty() && self.dns.acl6.is_empty() {
            bad.push(
                "dns.acl: empty — every query would be refused, including this box's own".into(),
            );
        }
        for (i, s) in self.dns.listen.iter().enumerate() {
            if s.parse::<std::net::SocketAddr>().is_err() {
                bad.push(format!(
                    "dns.listen[{i}]: `{s}` is not an address:port (e.g. 0.0.0.0:53 or [::]:53)"
                ));
            }
        }
        let mut seen: BTreeMap<&str, ()> = BTreeMap::new();
        for r in &self.dns.records {
            // A name in the zone, or a **bare label** — dnsmasq's hosts file
            // answered both forms on one line (`10.3.1.20 services1.int…
            // services1`) and the generator emits one entry per form. A dotted
            // name outside the zone is a name this box would forward, which is
            // never what a record means.
            let in_zone = r.name == self.zone || r.name.ends_with(&format!(".{}", self.zone));
            let bare = !r.name.contains('.');
            if !(in_zone || bare) {
                bad.push(format!(
                    "dns.records: `{}` is outside the zone `{}` — a record is either a name in the zone or a bare label; anything else would be forwarded rather than answered here",
                    r.name, self.zone
                ));
            }
            if seen.insert(&r.name, ()).is_some() {
                bad.push(format!(
                    "dns.records: `{}` is defined twice — the later entry would win by accident",
                    r.name
                ));
            }
        }
        for c in &self.dns.conditional {
            if c.servers.is_empty() {
                bad.push(format!(
                    "dns.conditional: `{}` names no server, so the suffix would be a black hole",
                    c.zone
                ));
            }
            if self.in_zone(&format!("x.{}", c.zone)) || c.zone == self.zone {
                bad.push(format!(
                    "dns.conditional: `{}` is inside (or is) the authoritative zone `{}` — this box answers that itself and would never forward it",
                    c.zone, self.zone
                ));
            }
        }
        if self.dns.forwarders.is_empty() && self.dns.conditional.is_empty() {
            bad.push("dns.forwarders: empty — nothing outside the zone would resolve".into());
        }
        if self.dns.cache_size > 1_000_000 {
            bad.push(format!(
                "dns.cache_size: {} entries is more than this crate will ever hold (the cache is a map in a 7 GiB router's RAM)",
                self.dns.cache_size
            ));
        }
    }

    fn check_ra(&self, bad: &mut Vec<String>) {
        if self.ra.interval == 0 {
            bad.push("ra.interval: 0 would advertise in a busy loop".into());
        }
        for (i, r) in self.ra.interfaces.iter().enumerate() {
            if r.prefixes.is_empty() {
                bad.push(format!("ra.interfaces[{i}]: no prefixes to advertise"));
            }
            if r.preferred_lifetime > r.valid_lifetime {
                bad.push(format!(
                    "ra.interfaces[{i}]: preferred lifetime ({}) is longer than the valid one ({}) — clients would deprecate an address they are still allowed to use",
                    r.preferred_lifetime, r.valid_lifetime
                ));
            }
            if r.router_lifetime != 0 && u64::from(r.router_lifetime) < self.ra.interval * 3 {
                bad.push(format!(
                    "ra.interfaces[{i}]: router lifetime {}s is under three advertisement intervals ({}s) — the default route would expire between advertisements",
                    r.router_lifetime,
                    self.ra.interval * 3
                ));
            }
            if self.ra.interfaces[..i]
                .iter()
                .any(|p| p.interface == r.interface)
            {
                bad.push(format!(
                    "ra.interfaces: `{}` appears twice — two advertisements for one VLAN",
                    r.interface
                ));
            }
        }
    }
}

fn in_range(p: &Pool, ip: Ipv4Addr) -> bool {
    u32::from(ip) >= u32::from(p.from) && u32::from(ip) <= u32::from(p.to)
}

fn overlap(a: &Pool, b: &Pool) -> bool {
    u32::from(a.from) <= u32::from(b.to) && u32::from(b.from) <= u32::from(a.to)
}
