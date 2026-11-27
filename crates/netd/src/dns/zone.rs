//! The zone this box is authoritative for: `int.leighhack.org` plus the
//! reverse names of the addresses it knows.
//!
//! Two sources feed it, and the second is the point of the replacement:
//!
//!   * `dns.records` — the device table and the front-door names, generated from
//!     `common/dns-records.nix` and `common/int-dns.nix`;
//!   * **the DHCP lease table, in memory** — `dhcp-fqdn` in the old config meant
//!     a client's own hostname answered as `<hostname>.<zone>`, and dnsmasq
//!     implemented that by rewriting its hosts file. Here the same answer comes
//!     from the table the server already holds, so there is no file to be stale
//!     and no rewrite to miss: a device that gets a lease is resolvable in the
//!     same microsecond.

use super::wire::*;
use crate::config::Config;
use crate::lease::LeaseDb;
use std::net::IpAddr;

pub struct ZoneAnswer {
    /// `true` when this box owns the name, whether or not it had an answer.
    /// `false` means "forward it" and nothing else in this struct is used.
    pub authoritative: bool,
    pub rcode: u8,
    pub answers: Vec<Record>,
    pub authority: Vec<Record>,
}

impl ZoneAnswer {
    fn not_ours() -> ZoneAnswer {
        ZoneAnswer {
            authoritative: false,
            rcode: RCODE_NOERROR,
            answers: Vec::new(),
            authority: Vec::new(),
        }
    }
}

/// Answer one question from the zone, or say it is not ours.
pub fn answer(cfg: &Config, db: &LeaseDb, q: &Question, now: i64) -> ZoneAnswer {
    let name = q.name.trim_end_matches('.').to_ascii_lowercase();
    let ttl = cfg.dns.ttl;
    let mut answers = Vec::new();

    // --- reverse names ---------------------------------------------------
    if let Some(ip) = reverse_to_ip(&name) {
        let mut names: Vec<String> = cfg
            .dns
            .records
            .iter()
            .filter(|r| r.addresses.contains(&ip))
            .map(|r| r.name.clone())
            .collect();
        // A live lease's name is an answer too; a lapsed one is not (the
        // address may already belong to somebody else).
        for l in db.all() {
            if IpAddr::V4(l.ip) == ip && l.expires > now {
                if let Some(h) = &l.hostname {
                    names.push(format!("{h}.{}", cfg.zone));
                }
            }
        }
        names.sort();
        names.dedup();
        if names.is_empty() {
            // An address this network hands out but nothing claims. Answering
            // NXDOMAIN would be a lie; answering NOERROR with no records is the
            // honest "no name for it".
            return ZoneAnswer {
                authoritative: true,
                rcode: RCODE_NOERROR,
                answers,
                authority: soa(cfg, ttl),
            };
        }
        if matches!(q.qtype, TYPE_PTR | TYPE_ANY) {
            for n in &names {
                answers.push(ptr_record(&name, n, ttl));
            }
        }
        return ZoneAnswer {
            authoritative: true,
            rcode: RCODE_NOERROR,
            answers,
            authority: Vec::new(),
        };
    }

    // --- a name the table owns, whichever form it was written in ----------
    //
    // This comes **before** the zone test on purpose: the generator emits a bare
    // label as its own entry (dnsmasq's hosts lines carried
    // `services1.int… services1`), and a bare name is not "in the zone" by a
    // suffix test. Answering it here is what makes `ping services1` work without
    // making a bare name resolvable by forwarding.
    answers.extend(addresses(cfg, &name, q.qtype, ttl));
    answers.extend(lease_addresses(cfg, db, &name, q.qtype, ttl, now));
    if !answers.is_empty() {
        return ZoneAnswer {
            authoritative: true,
            rcode: RCODE_NOERROR,
            answers,
            authority: Vec::new(),
        };
    }

    if !cfg.in_zone(&name) {
        return ZoneAnswer::not_ours();
    }

    // --- the apex ---------------------------------------------------------
    if name == cfg.zone {
        match q.qtype {
            TYPE_SOA => answers.push(soa_record(
                &name,
                &cfg.dns.soa.mname,
                &cfg.dns.soa.rname,
                serial(now),
                cfg.dns.soa.refresh,
                cfg.dns.soa.retry,
                cfg.dns.soa.expire,
                cfg.dns.soa.minimum,
                ttl,
            )),
            TYPE_NS => answers.push(ns_record(&name, &cfg.dns.soa.mname, ttl)),
            TYPE_A | TYPE_AAAA | TYPE_ANY => {
                // The apex is a name like any other when the table gives it an
                // address; nothing currently does.
                answers.extend(addresses(cfg, &name, q.qtype, ttl));
                if q.qtype == TYPE_ANY {
                    answers.push(soa_record(
                        &name,
                        &cfg.dns.soa.mname,
                        &cfg.dns.soa.rname,
                        serial(now),
                        cfg.dns.soa.refresh,
                        cfg.dns.soa.retry,
                        cfg.dns.soa.expire,
                        cfg.dns.soa.minimum,
                        ttl,
                    ));
                    answers.push(ns_record(&name, &cfg.dns.soa.mname, ttl));
                }
            }
            _ => {}
        }
        let empty = answers.is_empty();
        return ZoneAnswer {
            authoritative: true,
            rcode: RCODE_NOERROR,
            answers,
            authority: if empty { soa(cfg, ttl) } else { Vec::new() },
        };
    }

    // In the zone, past the apex, and nothing matched above. NXDOMAIN with the
    // SOA in the authority section, which is what RFC 2308 says a negative
    // answer carries so a downstream cache knows how long to believe it.
    // (dnsmasq answered this case with an empty authority; including the SOA is
    // strictly more correct and is the one deliberate difference, noted in
    // docs/netd-2026-11-26.md.)
    let exists = cfg.dns.records.iter().any(|r| r.name == name)
        || db
            .all()
            .iter()
            .any(|l| lease_name_matches(cfg, l, &name) && l.expires > now);
    ZoneAnswer {
        authoritative: true,
        rcode: if exists {
            RCODE_NOERROR
        } else {
            RCODE_NXDOMAIN
        },
        answers,
        authority: soa(cfg, ttl),
    }
}

/// `A`/`AAAA` for a name in the table, filtered by the question's type.
fn addresses(cfg: &Config, name: &str, qtype: u16, ttl: u32) -> Vec<Record> {
    let Some(rec) = cfg.dns.records.iter().find(|r| r.name == name) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for a in &rec.addresses {
        match (qtype, a) {
            (TYPE_ANY, IpAddr::V4(v4)) => out.push(a_record(name, *v4, ttl)),
            (TYPE_ANY, IpAddr::V6(v6)) => out.push(aaaa_record(name, *v6, ttl)),
            (TYPE_A, IpAddr::V4(v4)) => out.push(a_record(name, *v4, ttl)),
            (TYPE_AAAA, IpAddr::V6(v6)) => out.push(aaaa_record(name, *v6, ttl)),
            _ => {}
        }
    }
    out
}

/// A live lease's address, by the name `dhcp-fqdn` used to give: the client's
/// hostname, with and without the zone suffix.
fn lease_addresses(
    cfg: &Config,
    db: &LeaseDb,
    name: &str,
    qtype: u16,
    ttl: u32,
    now: i64,
) -> Vec<Record> {
    if !matches!(qtype, TYPE_A | TYPE_ANY) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for l in db.all() {
        if l.expires <= now || !lease_name_matches(cfg, l, name) {
            continue;
        }
        out.push(a_record(name, l.ip, ttl));
    }
    out
}

fn lease_name_matches(cfg: &Config, l: &crate::lease::Lease, name: &str) -> bool {
    let Some(h) = &l.hostname else {
        return false;
    };
    name == h || name == format!("{h}.{}", cfg.zone)
}

fn soa(cfg: &Config, ttl: u32) -> Vec<Record> {
    vec![soa_record(
        &cfg.zone,
        &cfg.dns.soa.mname,
        &cfg.dns.soa.rname,
        serial(crate::dhcp_server::now()),
        cfg.dns.soa.refresh,
        cfg.dns.soa.retry,
        cfg.dns.soa.expire,
        cfg.dns.soa.minimum,
        ttl,
    )]
}

/// The SOA serial. The zone's content is generated by a deploy, and clients are
/// told a TTL of one second, so the serial only has to be a number that moves
/// when the daemon starts — a fixed value would tell a downstream cache the zone
/// had not changed since it last asked.
fn serial(now: i64) -> u32 {
    (now.max(0) / 60) as u32
}

/// `20.1.3.10.in-addr.arpa` or a 32-nibble `ip6.arpa` name back to an address.
/// The name is the address **reversed**, which is the one thing easy to get
/// backwards and impossible to notice while both directions are being written.
fn reverse_to_ip(name: &str) -> Option<IpAddr> {
    if let Some(rest) = name.strip_suffix(".in-addr.arpa") {
        let parts: Vec<&str> = rest.split('.').collect();
        if parts.len() != 4 {
            return None;
        }
        let mut o = [0u8; 4];
        for (i, p) in parts.iter().enumerate() {
            o[3 - i] = p.parse().ok()?;
        }
        return Some(IpAddr::V4(std::net::Ipv4Addr::new(o[0], o[1], o[2], o[3])));
    }
    if let Some(rest) = name.strip_suffix(".ip6.arpa") {
        let nibbles: Vec<&str> = rest.split('.').collect();
        if nibbles.len() != 32 {
            return None;
        }
        let mut bytes = [0u8; 16];
        for (i, nibble) in nibbles.iter().enumerate() {
            let v = u8::from_str_radix(nibble, 16).ok()?;
            // `i` counts from the address's least significant nibble.
            let address_nibble = 31 - i;
            let byte = address_nibble / 2;
            if address_nibble % 2 == 0 {
                bytes[byte] |= v << 4;
            } else {
                bytes[byte] |= v;
            }
        }
        return Some(IpAddr::V6(std::net::Ipv6Addr::from(bytes)));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn reverse_names_round_trip() {
        let ip = Ipv4Addr::new(10, 3, 1, 20);
        assert_eq!(reverse_v4(ip), "20.1.3.10.in-addr.arpa");
        assert_eq!(reverse_to_ip(&reverse_v4(ip)), Some(IpAddr::V4(ip)));
        let v6: std::net::Ipv6Addr = "2001:db8:225::32".parse().unwrap();
        assert_eq!(reverse_to_ip(&reverse_v6(v6)), Some(IpAddr::V6(v6)));
        assert_eq!(reverse_to_ip("example.com"), None);
    }
}
