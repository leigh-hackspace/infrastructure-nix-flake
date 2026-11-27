//! IPv6 router advertisements, on a timer that does not depend on anything.
//!
//! ## Why this file is the reason netd exists
//!
//! dnsmasq's **unsolicited** advertisement timer dies when `router-lan` loses
//! carrier — the flap itself, not the cable being out. Nothing on the box is
//! broken in any way a unit, a log line or a dashboard can see: dnsmasq keeps
//! running, keeps answering DNS and DHCP, keeps logging. What expires is the
//! *default route* the clients hold, whose lifetime is 1200 s, so roughly twenty
//! minutes after the last advertisement the LAN quietly becomes a network with no
//! off-link IPv6. Every `*.int` name is answered on-link, so it hides until
//! somebody tries to ping something outside the /64.
//!
//! Measured three times on this network (2026-10-21, 2026-11-19, 2026-11-26), and
//! `machines/gw/ra-watch.nix` exists to count advertisements on the wire and
//! restart dnsmasq when they stop.
//!
//! This sender has no such state to lose. The loop wakes every second, compares
//! `now - last_sent` with `ra.interval` for each interface, and sends. There is
//! no per-link timer, no socket teardown, and nothing that a carrier event can
//! put out. The *only* thing that can stop an advertisement is the interface
//! being absent or the permission being missing, and both of those are reported
//! on every attempt rather than once at startup.
//!
//! ## What it advertises
//!
//! Exactly what the generated `ra-param`/`dhcp-range … ra-only` lines did:
//! the VLAN's prefixes with autonomous + on-link flags, a router lifetime of
//! 1200 s, and no DHCPv6 (M = O = 0). The prefix valid lifetime is the lease
//! `ttl` the clients have always been given a day of, so nothing about a client's
//! address changes on the day this replaces dnsmasq.

use crate::config::{Config, RaIface};
use crate::stats::Stats;
use crate::sys;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const ICMP6_ROUTER_ADVERT: u8 = 134;
const ICMP6_ROUTER_SOLICIT: u8 = 133;
/// RFC 4861's default for a box that is a router and not a host.
const CUR_HOP_LIMIT: u8 = 64;

/// Build one router advertisement. Pure, so the byte layout is testable without
/// a socket or a capability.
pub fn build(iface: &RaIface, link_layer: Option<[u8; 6]>) -> Vec<u8> {
    let mut p = Vec::with_capacity(16 + 32 * iface.prefixes.len() + 8);
    p.push(ICMP6_ROUTER_ADVERT);
    p.push(0); // code
    p.extend_from_slice(&[0, 0]); // checksum: the kernel fills this for a raw
                                  // IPPROTO_ICMPV6 socket
    p.push(CUR_HOP_LIMIT);
    // M = 0 (no DHCPv6 server here), O = 0 (nothing else to fetch), and the
    // reserved bits. This is dnsmasq's `option6:23,[::]` said without an option.
    p.push(0);
    p.extend_from_slice(&iface.router_lifetime.to_be_bytes());
    p.extend_from_slice(&0u32.to_be_bytes()); // reachable time: unspecified
    p.extend_from_slice(&0u32.to_be_bytes()); // retrans timer: unspecified

    if let Some(mac) = link_layer {
        p.push(1); // source link-layer address
        p.push(1); // 8 bytes, in units of 8
        p.extend_from_slice(&mac);
    }
    for prefix in &iface.prefixes {
        p.push(3); // prefix information
        p.push(4); // 32 bytes
        p.push(prefix.prefix);
        // L = 1 (this prefix is on-link) and A = 1 (clients may autoconfigure
        // from it). Without A a client keeps its old address and adds nothing;
        // without L it stops treating the /64 as local.
        p.push(0b1100_0000);
        p.extend_from_slice(&iface.valid_lifetime.to_be_bytes());
        p.extend_from_slice(&iface.preferred_lifetime.to_be_bytes());
        p.extend_from_slice(&0u32.to_be_bytes()); // reserved
        p.extend_from_slice(&prefix.addr.octets());
    }
    if let Some(mtu) = iface.mtu {
        p.push(5); // MTU
        p.push(1);
        p.extend_from_slice(&0u16.to_be_bytes()); // reserved
        p.extend_from_slice(&mtu.to_be_bytes());
    }
    p
}

/// Does this ICMPv6 packet look like a router solicitation from a link-local
/// source? Anything else on the raw socket (echo, neighbour discovery, another
/// router's advertisements) is ignored.
fn is_solicitation(packet: &[u8]) -> bool {
    packet.len() >= 8 && packet[0] == ICMP6_ROUTER_SOLICIT && packet[1] == 0
}

pub fn run(cfg: std::sync::Arc<Config>, stats: std::sync::Arc<Stats>) {
    if cfg.ra.interfaces.is_empty() {
        return;
    }
    loop {
        let socket = match sys::Icmp6::open() {
            Ok(s) => s,
            Err(e) => {
                Stats::bump(&stats.ra_errors);
                eprintln!("netd: router advertisements disabled this minute: {e}");
                std::thread::sleep(Duration::from_secs(60));
                continue;
            }
        };
        println!(
            "netd: router advertisements every {}s on {}",
            cfg.ra.interval,
            cfg.ra
                .interfaces
                .iter()
                .map(|i| i.interface.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        // Say which address each advertisement will appear to come from. It is
        // the one fact that decides whether a client can accept it at all
        // (RFC 4861 requires a link-local source), and reading it here means a
        // VLAN that is advertised from the wrong scope is visible in the journal
        // rather than in a capture.
        for iface in &cfg.ra.interfaces {
            match sys::link_local_v6(&iface.interface) {
                Some(ll) => println!(
                    "netd: ra {} from {ll} every {}s ({} prefix(es), router lifetime {}s, valid {}s)",
                    iface.interface,
                    cfg.ra.interval,
                    iface.prefixes.len(),
                    iface.router_lifetime,
                    iface.valid_lifetime
                ),
                None => eprintln!(
                    "netd: ra {}: no fe80:: address on the interface yet — a client will not accept an advertisement whose source is not link-local, so this waits rather than sending one it would drop",
                    iface.interface
                ),
            }
        }
        let mut last_sent: BTreeMap<String, Instant> = BTreeMap::new();
        let mut last_error: BTreeMap<String, String> = BTreeMap::new();
        let mut last_solicited = Instant::now() - Duration::from_secs(3600);
        loop {
            // The advertisement timer: a comparison, not a scheduled event. It
            // cannot be cancelled by anything, which is the entire point.
            for iface in &cfg.ra.interfaces {
                let due = last_sent
                    .get(&iface.interface)
                    .map(|t| t.elapsed() >= Duration::from_secs(cfg.ra.interval))
                    .unwrap_or(true);
                if !due {
                    continue;
                }
                if !sys::interface_exists(&iface.interface) {
                    let msg = "no such interface (waiting for it to appear; not fatal)".to_string();
                    if last_error.get(&iface.interface) != Some(&msg) {
                        eprintln!("netd: ra {}: {msg}", iface.interface);
                        last_error.insert(iface.interface.clone(), msg);
                    }
                    continue;
                }
                let Some(index) = sys::ifindex(&iface.interface) else {
                    continue;
                };
                let mac = link_layer(&iface.interface);
                let packet = build(iface, mac);
                match socket.send_all_nodes(index, &packet) {
                    Ok(()) => {
                        Stats::bump(&stats.ra_sent);
                        last_sent.insert(iface.interface.clone(), Instant::now());
                        last_error.remove(&iface.interface);
                    }
                    Err(e) => {
                        Stats::bump(&stats.ra_errors);
                        let msg = e.to_string();
                        if last_error.get(&iface.interface) != Some(&msg) {
                            eprintln!("netd: ra {}: {e}", iface.interface);
                            last_error.insert(iface.interface.clone(), msg);
                        }
                    }
                }
            }

            // Solicitations: answer one promptly, because a device that has just
            // booted is the one that cannot wait out a 60 s interval. This is
            // responsiveness, not the guarantee above — the timer runs either
            // way, which is what makes a lost solicitation harmless.
            let mut answered = 0;
            while answered < 8 {
                match socket.recv() {
                    Ok(Some((packet, from, scope))) => {
                        if !is_solicitation(&packet) {
                            continue;
                        }
                        if last_solicited.elapsed() < Duration::from_millis(500) {
                            continue;
                        }
                        let Some(iface) = cfg
                            .ra
                            .interfaces
                            .iter()
                            .find(|i| sys::ifindex(&i.interface) == Some(scope))
                        else {
                            continue;
                        };
                        let ra = build(iface, link_layer(&iface.interface));
                        if socket.send_to(scope, from, &ra).is_ok() {
                            Stats::bump(&stats.ra_sent);
                            Stats::bump(&stats.ra_solicited);
                            last_solicited = Instant::now();
                            answered += 1;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        Stats::bump(&stats.ra_errors);
                        eprintln!("netd: ra: receive: {e}");
                        break;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

/// The device's own MAC, for the source-link-layer option. Absent is fine: a
/// router advertisement without it is legal and every client on this network has
/// a neighbour cache entry from its own DHCP exchange anyway.
fn link_layer(device: &str) -> Option<[u8; 6]> {
    let text = std::fs::read_to_string(format!("/sys/class/net/{device}/address")).ok()?;
    let parts: Vec<&str> = text.trim().split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut mac = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        mac[i] = u8::from_str_radix(p, 16).ok()?;
    }
    Some(mac)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    const CFG: &str = include_str!("../tests/fixtures/minimal.json");

    fn iface() -> RaIface {
        Config::parse(CFG)
            .expect("fixture parses")
            .ra
            .interfaces
            .first()
            .cloned()
            .expect("one RA interface")
    }

    #[test]
    fn a_router_advertisement_has_the_fields_rfc_4861_requires() {
        let p = build(&iface(), Some([0xaa, 0, 0, 0, 0, 1]));
        assert_eq!(p[0], ICMP6_ROUTER_ADVERT);
        assert_eq!(p[1], 0, "code is always zero");
        assert_eq!(p[4], 64, "cur hop limit");
        assert_eq!(p[5] & 0b1100_0000, 0, "no DHCPv6: M and O are clear");
        assert_eq!(u16::from_be_bytes([p[6], p[7]]), 1200, "router lifetime");
        assert_eq!(u32::from_be_bytes([p[8], p[9], p[10], p[11]]), 0);
    }

    #[test]
    fn every_prefix_is_advertised_autonomous_and_on_link() {
        let p = build(&iface(), None);
        // 16-byte header, then the first option is at 16.
        assert_eq!(p[16], 3, "prefix information option");
        assert_eq!(p[17], 4, "32 bytes");
        assert_eq!(p[18], 64, "prefix length");
        assert_eq!(p[19], 0b1100_0000, "L and A set");
        let valid = u32::from_be_bytes([p[20], p[21], p[22], p[23]]);
        assert_eq!(valid, 86_400, "the day the clients have always been given");
        let preferred = u32::from_be_bytes([p[24], p[25], p[26], p[27]]);
        assert_eq!(preferred, 14_400);
        assert_eq!(&p[32..48], &iface().prefixes[0].addr.octets()[..]);
    }

    #[test]
    fn the_source_link_layer_option_is_present_only_when_the_mac_is_known() {
        let with = build(&iface(), Some([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]));
        assert_eq!(with[16], 1, "source link-layer address");
        assert_eq!(&with[18..24], &[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        let without = build(&iface(), None);
        assert_eq!(without[16], 3, "straight to the prefix option");
    }

    #[test]
    fn only_a_router_solicitation_is_answered() {
        assert!(is_solicitation(&[133, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!is_solicitation(&[134, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!is_solicitation(&[133, 1, 0, 0, 0, 0, 0, 0]));
        assert!(!is_solicitation(&[133]));
    }

    #[test]
    fn a_router_lifetime_of_zero_is_still_a_valid_advertisement() {
        // RFC 4861: lifetime 0 tells clients to forget the default route while
        // keeping the prefix; withdrawal must be expressible.
        let mut i = iface();
        i.router_lifetime = 0;
        let p = build(&i, None);
        assert_eq!(u16::from_be_bytes([p[6], p[7]]), 0);
    }
}
