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
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Tunnel => "tunnel",
            Kind::Wired => "wired",
            Kind::UsbTether => "usb-tether",
            Kind::Wifi => "wifi",
        }
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
        for line in out_text.lines() {
            let mut it = line.split_whitespace();
            // `2: eth0    inet 10.0.0.2/24 brd ... scope global eth0`
            while let Some(tok) = it.next() {
                if tok == "inet" {
                    if let Some(cidr) = it.next() {
                        if let Some((a, m)) = cidr.split_once('/') {
                            if let (Ok(ip), Ok(mask)) = (a.parse(), m.parse()) {
                                out.push((ip, mask));
                            }
                        }
                    }
                }
            }
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
    out
}
