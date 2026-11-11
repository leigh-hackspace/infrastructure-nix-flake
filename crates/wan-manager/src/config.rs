//! The daemon's runtime contract: the handful of *facts* it needs, and nothing
//! about a WAN.
//!
//! The rule this file exists to enforce is in the task that created the module:
//! **no WAN interface, address, gateway or SSID may be hardcoded here.** The
//! only names below are things that are not a WAN — the LAN interface (so the
//! discovery loop knows what to exclude), the network's own address blocks (so a
//! framed packet is not mistaken for an uplink), and the two policy-routing
//! tables the firewall's marks already read. Which NIC is the hub, which SSID is
//! the hotspot, which USB port the phone is on: all discovered at runtime.

use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    /// The LAN-side NIC whose VLAN devices are *not* WAN candidates. This is not
    /// a WAN fact; it is the one interface the daemon must never route out of.
    pub lan_interface: String,

    /// The network's own blocks. An interface holding an address inside one of
    /// these is LAN-side, not an uplink candidate. Derived from
    /// `common/vlans.nix` by the module, so a new VLAN cannot become a WAN.
    pub lan_prefixes: Vec<(std::net::Ipv4Addr, u8)>,

    /// AAISP's L2TP endpoint. Not a WAN interface: it is the address the tunnel
    /// *dials*, and the daemon keeps its `/32` on the best working egress so a
    /// dead hub cannot take the public addresses down with it.
    pub l2tp_endpoint: String,

    /// The SSID the organisation's backup hotspot advertises. Any BSSID carrying
    /// it is attempted; there can be more than one.
    pub wifi_ssid: String,
    pub psk_file: PathBuf,

    /// `/run/wan-manager` — status, heartbeat, wpa configs, last decisions.
    pub state_dir: PathBuf,

    /// The interface pppd creates for the AAISP session. It is discovered, not
    /// assumed to exist: if it is not there, there is no tunnel candidate.
    pub tunnel_interface: String,

    /// Policy tables the marks select (`machines/gw/networking.nix`).
    pub tunnel_table: u32,
    pub hub_table: u32,

    /// The metric the daemon's own promoted default gets. The DHCP client's
    /// routes stay well below it, so the daemon's route is the one selected only
    /// when it wants to be, and `ip route del` can target it precisely.
    pub fallback_metric: u32,
    pub l2tp_metric: u32,

    /// Internet anchors, tried in order. More than one because a single anchor
    /// blocking ICMP would otherwise demote a perfectly good uplink.
    pub ping_targets: Vec<String>,

    /// Source blocks that must be NATed when a candidate is the egress. These
    /// are the network's own blocks, not a WAN fact; the firewall's `wan_ifaces`
    /// set is the declarative form of the same rule, and this list is what the
    /// daemon uses if that set or the nft table is missing.
    pub nat_sources: Vec<String>,

    /// Interfaces that are **declared** WAN paths rather than discovered ones:
    /// a point-to-point tunnel to another box of ours that owns the real uplink
    /// (`machines/gw/wan-relay.nix`). A tunnel netdev has no upstream gateway to
    /// probe and no `/sys/class/net/*/device` symlink, so discovery cannot find
    /// it and ranking must not demand a gateway of it; both are facts about the
    /// declaration, not about the interface. Empty on any box that has no such
    /// path, which is why nothing about a relay appears in the defaults.
    pub relay_candidates: Vec<String>,

    /// A **large** HTTP object used to measure throughput. Two lessons are
    /// attached. `https://1.1.1.1` redirects and times out on a slow link, so
    /// this defaults to Cloudflare's speed endpoint; and a *small* page cannot
    /// measure a link at all — the old default (`https://one.one.one.one/`, a
    /// 56 KB homepage) spent 90 % of the measured time waiting for the server's
    /// first byte and under-reported `gw`'s path by 8.7x. One megabyte is enough
    /// that the transfer phase dominates even at 5 MB/s (~0.2 s) and still fits
    /// inside `--max-time` on a path that manages a tenth of that.
    pub probe_url: String,

    pub interval: Duration,
    pub throughput_interval: Duration,
    pub wifi_scan_interval: Duration,

    /// Consecutive failed gateway probes before a path is reset. 12 ticks at 5 s
    /// is a minute: long enough that a re-association or a DHCP renew is not a
    /// fault, short enough that a wedged radio is repaired while someone watches.
    pub kick_ticks: u32,

    /// Consecutive ticks the *incumbent* egress may be selected but ineligible
    /// before the network moves. `1` is the first version's behaviour, and it
    /// moved the network on a single failed probe: measured 2026-11-09, two
    /// marginal backups swapped every few minutes and each swap stranded the
    /// AAISP session for 40-90 s. A link that loses carrier is not a probe
    /// failure and still fails over on the first tick.
    pub failover_ticks: u32,

    /// How long a challenger must stay `failback_margin` times better than a
    /// healthy incumbent before it takes the seat. Failing *back* is the only
    /// voluntary move, and it costs every open connection its NAT source and
    /// the tunnel its socket, so it needs to be obviously worth it.
    pub failback_dwell: Duration,
    pub failback_margin: f64,

    /// The AAISP session unit. When the selected egress changes, pppd's L2TP
    /// socket is still bound to the old path's source address, so the session
    /// is stranded even though it looks up; `pppd` only notices after six
    /// missed LCP echoes (60 s). Restarting the unit redials it immediately,
    /// after `tunnel_redial` (the new route has to be installed first) and no
    /// more often than `tunnel_redial_cooldown`.
    pub tunnel_unit: String,
    pub tunnel_redial: Duration,
    pub tunnel_redial_cooldown: Duration,

    /// Do not touch an interface whose name matches these. VLANs and the LAN are
    /// excluded structurally; this is for the other things on a router that are
    /// not an uplink and must not be reconfigured (tailscale, OpenVPN, bridges).
    pub ignore_prefixes: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        let v4 = |s: &str| -> (std::net::Ipv4Addr, u8) {
            let (a, m) = s.split_once('/').unwrap_or((s, "32"));
            (
                a.parse().unwrap_or(std::net::Ipv4Addr::UNSPECIFIED),
                m.parse().unwrap_or(32),
            )
        };
        Config {
            lan_interface: "router-lan".into(),
            lan_prefixes: ["10.3.0.0/16", "192.168.2.0/24"]
                .iter()
                .map(|s| v4(s))
                .collect(),
            l2tp_endpoint: "194.4.172.12".into(),
            wifi_ssid: "Leighhack WAN Backup".into(),
            psk_file: PathBuf::from("/run/secrets/wan_backup_wifi_psk"),
            state_dir: PathBuf::from("/run/wan-manager"),
            tunnel_interface: "ppp0".into(),
            tunnel_table: 100,
            hub_table: 200,
            fallback_metric: 100,
            l2tp_metric: 5,
            ping_targets: vec!["1.1.1.1".into(), "8.8.8.8".into()],
            nat_sources: vec![
                "10.3.0.0/16".into(),
                "192.168.2.0/24".into(),
                "100.64.0.0/10".into(),
            ],
            probe_url: "https://speed.cloudflare.com/__down?bytes=1000000".into(),
            interval: Duration::from_secs(5),
            throughput_interval: Duration::from_secs(60),
            wifi_scan_interval: Duration::from_secs(300),
            kick_ticks: 12,
            failover_ticks: 3,
            failback_dwell: Duration::from_secs(600),
            failback_margin: 1.5,
            tunnel_unit: "xl2tpd-aaisp".into(),
            tunnel_redial: Duration::from_secs(10),
            tunnel_redial_cooldown: Duration::from_secs(300),
            ignore_prefixes: vec![
                "vlan".into(),
                "tailscale".into(),
                "docker".into(),
                "podman".into(),
                "br-".into(),
                "veth".into(),
                "tun".into(),
                "tap".into(),
                "wg".into(),
                "lo".into(),
            ],
            relay_candidates: Vec::new(),
        }
    }
}

impl Config {
    pub fn parse() -> Config {
        let mut c = Config::default();
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut i = 0;
        let value = |i: &mut usize| -> Option<String> {
            *i += 1;
            args.get(*i).cloned()
        };
        while i < args.len() {
            let a = args[i].clone();
            match a.as_str() {
                "--lan-interface" => {
                    if let Some(v) = value(&mut i) {
                        c.lan_interface = v;
                    }
                }
                "--lan-prefix" => {
                    if let Some(v) = value(&mut i) {
                        if let Some((addr, mask)) = v.split_once('/') {
                            if let Ok(ip) = addr.parse() {
                                c.lan_prefixes
                                    .push((ip, mask.parse().unwrap_or(32)));
                            }
                        }
                    }
                }
                "--l2tp-endpoint" => {
                    if let Some(v) = value(&mut i) {
                        c.l2tp_endpoint = v;
                    }
                }
                "--wifi-ssid" => {
                    if let Some(v) = value(&mut i) {
                        c.wifi_ssid = v;
                    }
                }
                "--psk-file" => {
                    if let Some(v) = value(&mut i) {
                        c.psk_file = PathBuf::from(v);
                    }
                }
                "--state-dir" => {
                    if let Some(v) = value(&mut i) {
                        c.state_dir = PathBuf::from(v);
                    }
                }
                "--tunnel-interface" => {
                    if let Some(v) = value(&mut i) {
                        c.tunnel_interface = v;
                    }
                }
                "--tunnel-table" => {
                    if let Some(v) = value(&mut i) {
                        c.tunnel_table = v.parse().unwrap_or(c.tunnel_table);
                    }
                }
                "--hub-table" => {
                    if let Some(v) = value(&mut i) {
                        c.hub_table = v.parse().unwrap_or(c.hub_table);
                    }
                }
                "--fallback-metric" => {
                    if let Some(v) = value(&mut i) {
                        c.fallback_metric = v.parse().unwrap_or(c.fallback_metric);
                    }
                }
                "--l2tp-metric" => {
                    if let Some(v) = value(&mut i) {
                        c.l2tp_metric = v.parse().unwrap_or(c.l2tp_metric);
                    }
                }
                "--nat-source" => {
                    if let Some(v) = value(&mut i) {
                        c.nat_sources.push(v);
                    }
                }
                "--ping-target" => {
                    if let Some(v) = value(&mut i) {
                        c.ping_targets.push(v);
                    }
                }
                "--probe-url" => {
                    if let Some(v) = value(&mut i) {
                        c.probe_url = v;
                    }
                }
                "--interval" => {
                    if let Some(v) = value(&mut i) {
                        c.interval = Duration::from_secs(v.parse().unwrap_or(5));
                    }
                }
                "--throughput-interval" => {
                    if let Some(v) = value(&mut i) {
                        c.throughput_interval =
                            Duration::from_secs(v.parse().unwrap_or(60));
                    }
                }
                "--wifi-scan-interval" => {
                    if let Some(v) = value(&mut i) {
                        c.wifi_scan_interval =
                            Duration::from_secs(v.parse().unwrap_or(300));
                    }
                }
                "--kick-ticks" => {
                    if let Some(v) = value(&mut i) {
                        c.kick_ticks = v.parse().unwrap_or(c.kick_ticks);
                    }
                }
                "--failover-ticks" => {
                    if let Some(v) = value(&mut i) {
                        c.failover_ticks = v.parse().unwrap_or(c.failover_ticks);
                    }
                }
                "--failback-dwell" => {
                    if let Some(v) = value(&mut i) {
                        c.failback_dwell =
                            Duration::from_secs(v.parse().unwrap_or(600));
                    }
                }
                "--failback-margin" => {
                    if let Some(v) = value(&mut i) {
                        c.failback_margin = v.parse().unwrap_or(c.failback_margin);
                    }
                }
                "--tunnel-unit" => {
                    if let Some(v) = value(&mut i) {
                        c.tunnel_unit = v;
                    }
                }
                "--tunnel-redial" => {
                    if let Some(v) = value(&mut i) {
                        c.tunnel_redial = Duration::from_secs(v.parse().unwrap_or(10));
                    }
                }
                "--tunnel-redial-cooldown" => {
                    if let Some(v) = value(&mut i) {
                        c.tunnel_redial_cooldown =
                            Duration::from_secs(v.parse().unwrap_or(300));
                    }
                }
                "--ignore-prefix" => {
                    if let Some(v) = value(&mut i) {
                        c.ignore_prefixes.push(v);
                    }
                }
                "--relay-candidate" => {
                    if let Some(v) = value(&mut i) {
                        c.relay_candidates.push(v);
                    }
                }
                "--help" | "-h" => {
                    println!("{}", USAGE);
                    std::process::exit(0);
                }
                other => {
                    eprintln!("wan-manager: ignoring unknown argument {other:?}");
                }
            }
            i += 1;
        }
        c
    }

    pub fn status_json(&self) -> PathBuf {
        self.state_dir.join("status.json")
    }
    pub fn status_prom(&self) -> PathBuf {
        self.state_dir.join("metrics.prom")
    }
    pub fn heartbeat(&self) -> PathBuf {
        self.state_dir.join("last-run")
    }
    pub fn decision(&self) -> PathBuf {
        self.state_dir.join("decision")
    }
    pub fn networkd_dir(&self) -> PathBuf {
        self.state_dir.join("networkd")
    }
}

pub const USAGE: &str = "\
wan-manager — discover, rank, select and continuously repair the router's WAN.

  wan-manager [options]

It takes no WAN interface, address or gateway as an argument on purpose: those
are discovered (DHCP offers, wireless scans, USB netdevs) and ranked.

Options:
  --lan-interface NAME       LAN NIC to exclude from discovery (router-lan)
  --lan-prefix CIDR          a network block that is LAN-side (repeatable)
  --l2tp-endpoint ADDR       AAISP LNS whose /32 follows the working egress
  --wifi-ssid NAME           SSID to attempt on every wireless interface
  --psk-file PATH            file containing that SSID's passphrase
  --state-dir PATH           /run/wan-manager
  --tunnel-interface NAME    ppp0
  --tunnel-table N           100
  --hub-table N              200
  --fallback-metric N        100
  --l2tp-metric N            5
  --ping-target ADDR         internet anchor (repeatable)
  --nat-source CIDR          a source block to masquerade (repeatable)
  --probe-url URL            large HTTP object for throughput ranking (1 MB)
  --interval SECS            5
  --throughput-interval SECS 60
  --wifi-scan-interval SECS  300
  --kick-ticks N             12
  --failover-ticks N         consecutive ineligible ticks before the network
                             moves (3 = 15 s at the 5 s interval; a lost
                             carrier moves at once)
  --failback-dwell SECS      how long a challenger must stay better before it
                             takes a healthy incumbent's seat (600)
  --failback-margin FACTOR   how much better, as a factor (1.5)
  --tunnel-unit NAME         the AAISP session unit to redial (xl2tpd-aaisp)
  --tunnel-redial SECS       redial after the egress moves, once the new route
                             has settled (10; 0 disables)
  --tunnel-redial-cooldown SECS  never redial more often than this (300)
  --ignore-prefix PREFIX     interface-name prefix never to touch (repeatable)
  --relay-candidate NAME     declared path to another box's uplink (repeatable)
";
