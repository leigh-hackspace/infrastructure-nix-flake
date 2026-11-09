//! Reading the kernel's view of links, addresses and routes.
//!
//! Everything here goes through sysfs and `ip`, not a netlink crate: the daemon
//! is a router daemon, and the `ip` binary is the same one an operator uses when
//! they ssh in to disagree with it. The parse functions are deliberately total —
//! a route line this parser does not understand is `None`, never a panic.

use crate::config::Config;
use crate::util::{read_trim, route_dev, route_metric, route_via, run_ok};
use std::net::Ipv4Addr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Tunnel,
    Wired,
    UsbTether,
    Wifi,
    /// A **declared** point-to-point path to another box of ours that owns a
    /// real uplink — `--relay-candidate`, and the reason it exists is measured:
    /// the phone enumerates on `aibox`'s xHCI and never on this box's EHCI, so
    /// the only way this router can reach that tether is through a box that can
    /// (`docs/wan-manager-2026-11-09.md`, "The phone that enumerates somewhere
    /// else"). Like the tunnel it has no gateway to probe and no `.device`
    /// symlink for discovery to find, so it is *declared* rather than found.
    Relay,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Tunnel => "tunnel",
            Kind::Wired => "wired",
            Kind::UsbTether => "usb-tether",
            Kind::Wifi => "wifi",
            Kind::Relay => "relay",
        }
    }

    /// A path with no gateway and no `.device` symlink: it carries traffic on
    /// its own terms, so probes ask only "is there internet at the far end".
    pub fn is_gatewayless(&self) -> bool {
        matches!(self, Kind::Tunnel | Kind::Relay)
    }

    /// A stable, *small* preference bonus. Performance is the primary ranking
    /// signal (throughput, then latency); this only breaks ties, because a cable
    /// at the same measured rate is less likely to be the thing that fails next.
    pub fn preference(&self) -> f64 {
        match self {
            Kind::Tunnel => 60.0,
            Kind::Wired => 40.0,
            Kind::UsbTether => 30.0,
            Kind::Wifi => 20.0,
            // A relay is someone else's tether: it is that path plus a hop, so
            // it must not outrank the same link measured directly on this box
            // at the same rate. Throughput still dominates, which is what makes
            // a USB tether beat a 2.4 GHz hotspot through the same phone.
            Kind::Relay => 15.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IfaceInfo {
    pub name: String,
    pub kind: Kind,
    pub link_up: bool,
    pub carrier: bool,
    pub addresses: Vec<(Ipv4Addr, u8)>,
    /// The gateway the DHCP client installed, if any. This is the *offer* the
    /// task asks the daemon to listen for; nothing here assumes a subnet.
    pub gateway: Option<String>,
    pub dhcp_metric: Option<u32>,
    /// True when the interface looks like a wireless device, even if it has no
    /// association yet.
    pub wireless: bool,
    pub usb: bool,
    /// The USB path (e.g. `2-1.1`) when `usb` is true, for diagnostics.
    pub usb_path: Option<String>,
    pub driver: Option<String>,
}

impl IfaceInfo {
    /// What a probe must bind to so it leaves by *this* path.
    ///
    /// For a device that owns a route that is the device itself. A relay has no
    /// route of its own until it is selected, so `-I <device>` would fail the
    /// lookup and report "network unreachable" for a path that is fine; its own
    /// address is the thing that is bound, and `machines/gw/wan-relay.nix`
    /// declares the rule that sends that source into the relay's own table.
    /// `ping -I` and `curl --interface` both accept an address for this reason.
    pub fn probe_bind(&self) -> String {
        match self.kind {
            Kind::Relay => self
                .addresses
                .first()
                .map(|(a, _)| a.to_string())
                .unwrap_or_else(|| self.name.clone()),
            _ => self.name.clone(),
        }
    }
}

pub fn list_interfaces() -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir("/sys/class/net") {
        for e in rd.flatten() {
            v.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    v.sort();
    v
}

pub fn is_wireless(name: &str) -> bool {
    let base = format!("/sys/class/net/{name}");
    std::path::Path::new(&format!("{base}/wireless")).exists()
        || std::path::Path::new(&format!("{base}/phy80211")).exists()
}

/// The USB path behind a netdev, read from the device symlink. Returns `None`
/// for a PCIe NIC. This is how a USB-tethered phone is distinguished from the
/// built-in e1000e without naming either: the USB bus is a property of the
/// device, and the port is a fact that changes when a cable moves.
pub fn usb_path(name: &str) -> Option<String> {
    let dev = std::fs::read_link(format!("/sys/class/net/{name}/device")).ok()?;
    // `read_link` is relative (`../../../2-1/...`) and may not mention `usb` at
    // all; the canonical path does. Resolve first, then ask the bus question.
    let canon = std::fs::canonicalize(format!("/sys/class/net/{name}/device"))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| dev.to_string_lossy().into_owned());
    if !canon.contains("usb") {
        return None;
    }
    for part in canon.split('/').rev() {
        if part.starts_with(|c: char| c.is_ascii_digit()) && part.contains('-') {
            return Some(part.split(':').next().unwrap_or(part).to_string());
        }
    }
    None
}

pub fn driver(name: &str) -> Option<String> {
    std::fs::read_link(format!("/sys/class/net/{name}/device/driver"))
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
}

pub fn link_up(name: &str) -> bool {
    read_trim(&format!("/sys/class/net/{name}/operstate"))
        .map(|s| s == "up" || s == "unknown")
        .unwrap_or(false)
}

pub fn carrier(name: &str) -> bool {
    read_trim(&format!("/sys/class/net/{name}/carrier"))
        .map(|s| s == "1")
        .unwrap_or(false)
}

pub fn ipv4_addresses(name: &str) -> Vec<(Ipv4Addr, u8)> {
    let mut out = Vec::new();
    let proc_path = format!("/proc/net/fib_trie");
    if let Ok(text) = std::fs::read_to_string(proc_path) {
        // /proc/net/fib_trie has no device names, so it is only a fallback for
        // the `ip` path below; the real read is `ip -4 -o addr show dev`.
        let _ = text;
    }
    let args = vec![
        "-4".to_string(),
        "-o".to_string(),
        "addr".to_string(),
        "show".to_string(),
        "dev".to_string(),
        name.to_string(),
    ];
    if let Some(out_text) = run_ok("ip", &args, 3) {
        out = parse_ipv4_addrs(&out_text);
    }
    out
}

/// Parse `ip -4 -o addr show` output into (address, prefix length) pairs.
///
/// Two shapes, and reading only the first is a bug that hid for as long as this
/// crate has existed (measured 2026-11-09):
///
///   `2: eth0    inet 10.0.0.2/24 brd 10.0.0.255 scope global eth0`
///   `29: ppp0   inet 81.187.223.123 peer 81.187.81.187/32 scope global ppp0`
///
/// The second is a point-to-point link: the local half carries **no** prefix, so
/// a `split_once('/')` on it found nothing and `ppp0` was reported as having no
/// IPv4 address at all — including while the AAISP tunnel was up and carrying
/// the network. It cost only the dashboard's address column until 2026-11-09,
/// when the daemon started using "addressless tunnel" as a reason to stop
/// probing, which would have left a working tunnel unselected. The local address
/// is the first field; the peer's mask is the link's mask.
///
/// It is a pure function so it can be tested against the exact strings: `ip` is
/// not installed in a test sandbox and the two shapes differ by one token.
pub fn parse_ipv4_addrs(out_text: &str) -> Vec<(Ipv4Addr, u8)> {
    let mut out = Vec::new();
    for line in out_text.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        let Some(i) = toks.iter().position(|t| *t == "inet") else {
            continue;
        };
        let Some(local) = toks.get(i + 1) else {
            continue;
        };
        let (addr, mask): (&str, String) = match local.split_once('/') {
            Some((a, m)) => (a, m.to_string()),
            None => {
                let m = toks
                    .iter()
                    .position(|t| *t == "peer")
                    .and_then(|p| toks.get(p + 1))
                    .and_then(|cidr| cidr.split_once('/'))
                    .map(|(_, m)| m.to_string())
                    .unwrap_or_else(|| "32".to_string());
                (*local, m)
            }
        };
        if let (Ok(ip), Ok(mask)) = (addr.parse(), mask.parse()) {
            out.push((ip, mask));
        }
    }
    out
}

/// The default route a DHCP client installed on this device. The daemon's own
/// promoted routes use `fallback_metric`, and are excluded so a second tick
/// never mistakes its own output for a fresh DHCP offer.
pub fn dhcp_gateway(name: &str, fallback_metric: u32) -> Option<(String, Option<u32>)> {
    let args = vec![
        "-4".to_string(),
        "route".to_string(),
        "show".to_string(),
        "default".to_string(),
        "dev".to_string(),
        name.to_string(),
    ];
    let text = run_ok("ip", &args, 3)?;
    let mut best: Option<(String, Option<u32>, bool)> = None;
    for line in text.lines() {
        let Some(gw) = route_via(line) else { continue };
        let metric = route_metric(line);
        if metric == Some(fallback_metric) {
            continue;
        }
        let dhcp = line.contains(" proto dhcp")
            || line.contains(" proto dhcp ")
            || line.split_whitespace().any(|w| w == "dhcp");
        let better = match &best {
            None => true,
            Some((_, m, d)) => (dhcp, metric.unwrap_or(u32::MAX))
                > (*d, (*m).unwrap_or(u32::MAX)),
        };
        if better {
            best = Some((gw, metric, dhcp));
        }
    }
    best.map(|(gw, m, _)| (gw, m))
}

pub fn has_default_table(table: u32) -> bool {
    let args = vec![
        "-4".to_string(),
        "route".to_string(),
        "show".to_string(),
        "table".to_string(),
        table.to_string(),
        "default".to_string(),
    ];
    run_ok("ip", &args, 3)
        .map(|s| s.lines().any(|l| l.trim_start().starts_with("default")))
        .unwrap_or(false)
}

pub fn default_dev_in_table(table: u32) -> Option<String> {
    let args = vec![
        "-4".to_string(),
        "route".to_string(),
        "show".to_string(),
        "default".to_string(),
        "table".to_string(),
        table.to_string(),
    ];
    let text = run_ok("ip", &args, 3)?;
    text.lines().find_map(route_dev)
}

pub fn any_default_route() -> Option<(String, String, Option<u32>)> {
    let text = run_ok("ip", &vec!["-4".to_string(), "route".to_string(), "show".to_string(), "default".to_string()], 3)?;
    for line in text.lines() {
        if let (Some(gw), Some(dev)) = (route_via(line), route_dev(line)) {
            return Some((gw, dev, route_metric(line)));
        }
        if let Some(dev) = route_dev(line) {
            return Some((String::new(), dev, route_metric(line)));
        }
    }
    None
}

pub fn is_lan_ip(cfg: &Config, ip: Ipv4Addr) -> bool {
    cfg.lan_prefixes.iter().any(|(net, mask)| {
        let m = if *mask == 0 {
            0
        } else {
            u32::MAX << (32 - u32::from(*mask))
        };
        (u32::from(ip) & m) == (u32::from(*net) & m)
    })
}

pub fn ignored(cfg: &Config, name: &str) -> bool {
    if name == cfg.lan_interface {
        return true;
    }
    cfg.ignore_prefixes.iter().any(|p| name.starts_with(p))
}

/// Classify and describe every interface that could plausibly carry the
/// network's egress. The tunnel is included only when pppd has made it.
pub fn discover(cfg: &Config) -> Vec<IfaceInfo> {
    let mut out = Vec::new();
    for name in list_interfaces() {
        if ignored(cfg, &name) {
            continue;
        }
        let wireless = is_wireless(&name);
        let usb = usb_path(&name).is_some();
        let addresses = ipv4_addresses(&name);
        let is_tunnel = name == cfg.tunnel_interface;
        // Every physical Ethernet NIC is an uplink candidate even before it has
        // an address. A USB adapter that appears later must get a DHCP file on
        // the tick it appears; skipping addressless interfaces meant a newly
        // plugged one was never attempted at all. Virtual devices (bridges,
        // dummies, bonds) have no `device` symlink and are not WANs.
        let ether_like = read_trim(&format!("/sys/class/net/{name}/type"))
            .as_deref()
            == Some("1");
        let physical = std::path::Path::new(&format!("/sys/class/net/{name}/device")).exists();
        if !is_tunnel && !wireless && !(ether_like && physical) {
            continue;
        }
        let all_lan = !addresses.is_empty() && addresses.iter().all(|(a, _)| is_lan_ip(cfg, *a));
        // A non-LAN interface whose only addresses are the LAN's (a bridge, a
        // veth) is not an uplink. The tunnel is exempt: its address is public.
        if all_lan && name != cfg.tunnel_interface {
            continue;
        }
        let kind = if name == cfg.tunnel_interface {
            Kind::Tunnel
        } else if wireless {
            Kind::Wifi
        } else if usb {
            Kind::UsbTether
        } else {
            Kind::Wired
        };
        let gateway = if kind == Kind::Tunnel {
            None
        } else {
            dhcp_gateway(&name, cfg.fallback_metric).map(|(g, _)| g)
        };
        let (_, dhcp_metric) = if kind == Kind::Tunnel {
            (None, None)
        } else {
            dhcp_gateway(&name, cfg.fallback_metric)
                .map(|(g, m)| (Some(g), m))
                .unwrap_or((None, None))
        };
        out.push(IfaceInfo {
            name: name.clone(),
            kind,
            link_up: link_up(&name),
            carrier: carrier(&name),
            addresses,
            gateway,
            dhcp_metric,
            wireless,
            usb,
            usb_path: usb_path(&name),
            driver: driver(&name),
        });
    }
    // Declared relays last, so a name that discovery also found keeps the kind
    // discovery gave it. A tunnel netdev has no `device` symlink and offers no
    // DHCP gateway, so neither of discovery's two questions can see it: that it
    // is a WAN at all is the declaration, and everything else is measured.
    for name in &cfg.relay_candidates {
        if ignored(cfg, name) || out.iter().any(|i| &i.name == name) {
            continue;
        }
        if !std::path::Path::new(&format!("/sys/class/net/{name}")).exists() {
            continue;
        }
        out.push(IfaceInfo {
            name: name.clone(),
            kind: Kind::Relay,
            link_up: link_up(name),
            carrier: carrier(name),
            addresses: ipv4_addresses(name),
            gateway: None,
            dhcp_metric: None,
            wireless: false,
            usb: false,
            usb_path: None,
            driver: driver(name),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two shapes `ip -4 -o addr show` emits, captured from this box on
    /// 2026-11-09. The point-to-point one is why `ppp0` looked addressless for
    /// the life of the crate; the peer's `/32` is the link's mask, and the local
    /// address is what the daemon must see.
    #[test]
    fn parses_connected_and_point_to_point_addresses() {
        let eth = "23: enp0s29u1u2u1    inet 192.168.1.101/24 metric 2000 brd 192.168.1.255 scope global dynamic enp0s29u1u2u1\\       valid_lft 86348sec preferred_lft 86348sec";
        assert_eq!(
            parse_ipv4_addrs(eth),
            vec![(std::net::Ipv4Addr::new(192, 168, 1, 101), 24)]
        );

        let ppp = "29: ppp0    inet 81.187.223.123 peer 81.187.81.187/32 scope global ppp0\\       valid_lft forever preferred_lft forever";
        assert_eq!(
            parse_ipv4_addrs(ppp),
            vec![(std::net::Ipv4Addr::new(81, 187, 223, 123), 32)]
        );

        // A /30 point-to-point link keeps the shared mask rather than assuming
        // /32, because `ip` prints the peer's prefix for exactly that reason.
        let p2p30 = "7: tun0    inet 10.9.0.1 peer 10.9.0.2/30 scope global tun0";
        assert_eq!(
            parse_ipv4_addrs(p2p30),
            vec![(std::net::Ipv4Addr::new(10, 9, 0, 1), 30)]
        );

        // An interface with no IPv4 address at all yields nothing, which is the
        // answer the tunnel branch of `probe_all` acts on.
        assert!(parse_ipv4_addrs("29: ppp0    inet6 fe80::1/64 scope link").is_empty());
        assert!(parse_ipv4_addrs("").is_empty());
    }
}
