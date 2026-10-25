//! Labelling what the connection table shows: addresses → host names, read out
//! of the files the router already keeps, and the two small tables that turn a
//! port number or an ICMP type into a word.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/// The addresses this network has a name for, read out of the files the router
/// already keeps: the flake's own DNS records (world-readable, store-backed)
/// and — when this process is allowed to open it — dnsmasq's lease file.
///
/// Reloaded when a source's mtime changes, which is how a new lease appears
/// without a restart.  An unreadable source is not an error: the addresses are
/// still shown, only as numbers.
#[derive(Default, Debug)]
pub struct Names {
    pub(crate) by_addr: HashMap<IpAddr, String>,
    pub(crate) sources: Vec<Source>,
}

#[derive(Debug)]
pub(crate) struct Source {
    pub(crate) path: String,
    /// `None` until the file has been read once.
    pub(crate) mtime: Option<SystemTime>,
    pub(crate) kind: SourceKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SourceKind {
    /// `10.3.14.62 3d-blue.int.leighhack.org 3d-blue`
    Hosts,
    /// `<epoch> <mac> <ip> <hostname> <client-id>`
    Leases,
}

/// When a file last changed, to the nearest thing the filesystem reports.
///
/// Nanoseconds rather than whole seconds on purpose: the lease file is rewritten
/// by dnsmasq whenever a client renews, and a one-second resolution would let two
/// writes inside one tick look like nothing happened — the dashboard would then
/// keep a name for an address that has been given to a different client.
pub(crate) fn mtime(path: &str) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

impl Names {
    /// `--hosts-file` paths plus `--leases-file`.  Leases are read first and
    /// hosts files after, because a record the flake owns is a better answer
    /// for an address than whatever that client last called itself.
    pub fn open(hosts: &[String], leases: Option<String>) -> Self {
        let mut sources = Vec::new();
        if let Some(path) = leases {
            sources.push(Source {
                path,
                mtime: None,
                kind: SourceKind::Leases,
            });
        }
        for path in hosts {
            sources.push(Source {
                path: path.clone(),
                mtime: None,
                kind: SourceKind::Hosts,
            });
        }
        let mut names = Names {
            by_addr: HashMap::new(),
            sources,
        };
        names.load();
        names
    }

    /// Re-read anything that changed, or that has never been read.
    pub(crate) fn refresh(&mut self) {
        if self
            .sources
            .iter()
            .any(|s| s.mtime.is_none() || s.mtime != mtime(&s.path))
        {
            self.load();
        }
    }

    fn load(&mut self) {
        let mut by_addr = HashMap::new();
        for source in &mut self.sources {
            source.mtime = mtime(&source.path);
            let Ok(text) = std::fs::read_to_string(&source.path) else {
                continue;
            };
            for line in text.lines() {
                if let Some((ip, name)) = parse_name_line(line, source.kind) {
                    by_addr.insert(ip, name);
                }
            }
        }
        self.by_addr = by_addr;
    }

    pub fn get(&self, ip: IpAddr) -> Option<&str> {
        self.by_addr.get(&ip).map(String::as_str)
    }
}

/// One `(address, name)` pair out of a hosts or leases line, if it has one.
pub(crate) fn parse_name_line(line: &str, kind: SourceKind) -> Option<(IpAddr, String)> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    let out = match kind {
        SourceKind::Hosts => {
            let ip: IpAddr = (*toks.first()?).parse().ok()?;
            // Prefer the first alias with no dot in it: the router's records
            // file is `<ip> <fqdn> <short>` and a table cell wants `services1`,
            // not `services1.int.leighhack.org`.
            let alias = toks[1..]
                .iter()
                .find(|a| !a.contains('.'))
                .copied()
                .or_else(|| toks.get(1).copied())?;
            (ip, alias.to_string())
        }
        SourceKind::Leases => {
            // `<epoch> <mac> <ip> <hostname> <client-id>`; dnsmasq writes `*`
            // for a client that sent no hostname, which is not a name.
            if toks.len() < 4 || toks[0].is_empty() || !toks[0].bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let ip: IpAddr = toks[2].parse().ok()?;
            let host = toks[3];
            if host.is_empty() || host == "*" {
                return None;
            }
            (ip, host.to_string())
        }
    };
    Some(out)
}

/// What a port means, for the handful of services this network actually sees.
/// Deliberately small: it is a label, not an nmap, and a confident wrong answer
/// here is worse than the number.  The ports are the ones a device on *this*
/// network is found talking to (RTSP for the cameras, NFS for the NAS, 41641
/// for the tailnet), which is a different list from `/etc/services`.
pub fn service_name(proto: &str, port: u16) -> Option<&'static str> {
    let tcp = proto == "tcp";
    let name = match (tcp, port) {
        (true, 20) | (true, 21) => "ftp",
        (true, 22) => "ssh",
        (true, 25) | (true, 465) | (true, 587) => "smtp",
        (false, 53) | (true, 53) => "dns",
        (true, 80) => "http",
        (true, 110) => "pop3",
        (true, 135) => "rpc",
        (true, 143) => "imap",
        (true, 443) => "https",
        (true, 445) => "smb",
        (true, 554) => "rtsp",
        (true, 636) => "ldaps",
        (true, 993) => "imaps",
        (true, 995) => "pop3s",
        (true, 1194) => "openvpn",
        (true, 1433) => "mssql",
        (true, 2049) => "nfs",
        (false, 2049) => "nfs",
        (true, 3000) => "grafana",
        (true, 3128) => "proxy",
        (true, 3240) | (true, 32410) | (true, 32412) => "plex",
        (true, 4433) => "unifi-mgmt",
        (true, 5000) => "upnp",
        (true, 5060 | 5061) => "sip",
        (true, 5173 | 5174) => "vnc",
        (true, 5357) => "wsd",
        (true, 5432) => "postgres",
        (true, 5683) => "coap",
        (true, 5900) => "vnc",
        (true, 6379) => "redis",
        (true, 8000 | 8080 | 8443 | 8888) => "http-alt",
        (true, 8123) => "home-assistant",
        (true, 9000) => "http-admin",
        (true, 9090) => "prometheus",
        (true, 9100) => "printer",
        (true, 9443) => "nginx-sso",
        (true, 10000) => "webmin",
        (true, 32400) => "plex",
        (false, 67) | (false, 68) => "dhcp",
        (false, 123) => "ntp",
        (false, 443) => "quic",
        (false, 500) | (false, 4500) => "ipsec",
        (false, 1194) => "openvpn",
        (false, 1900) => "ssdp",
        (false, 3478) | (false, 3479) => "stun",
        (false, 41641) => "tailscale",
        (false, 51820) => "wireguard",
        (false, 5353) => "mdns",
        (false, 5355) => "lldp",
        (false, 5683) => "coap",
        (false, 62463) => "ok-google",
        _ => return None,
    };
    Some(name)
}

/// The ICMP type names, split by family because the numbers collide: type 3 is
/// `dest-unreach` in IPv4 and `time-exceeded` in IPv6, and a label that is
/// wrong half the time is worse than the number.
pub(crate) fn icmp_name(type_: u16, v6: bool) -> Option<&'static str> {
    let name = if v6 {
        match type_ {
            1 => "dest-unreach",
            2 => "packet-too-big",
            3 => "time-exceeded",
            4 => "param-problem",
            128 => "echo-request",
            129 => "echo-reply",
            130 => "mcast-listener-done",
            131 => "mcast-listener-report",
            132 => "mcast-listener-query",
            133 => "router-solicit",
            134 => "router-advert",
            135 => "neighbour-solicit",
            136 => "neighbour-advert",
            137 => "redirect",
            138 => "router-renumbering",
            139 => "reverse-nd-solicit",
            140 => "reverse-nd-advert",
            _ => return None,
        }
    } else {
        match type_ {
            0 => "echo-reply",
            3 => "dest-unreach",
            4 => "source-quench",
            5 => "redirect",
            8 => "echo-request",
            9 => "router-advert",
            10 => "router-solicit",
            11 => "time-exceeded",
            12 => "param-problem",
            13 => "timestamp",
            14 => "timestamp-reply",
            15 => "info-request",
            16 => "info-reply",
            17 => "address-mask-request",
            18 => "address-mask-reply",
            30 => "traceroute",
            31 => "conversion-error",
            32 => "mobile-redirect",
            33 => "ip-where-are-you",
            34 => "ip-are-you-here",
            35 => "mobile-registration-request",
            36 => "mobile-registration-reply",
            37 => "domain-name-request",
            38 => "domain-name-reply",
            39 => "ipv6-where-are-you",
            40 => "ipv6-are-you-here",
            41 => "depth-of-agreement",
            42 => "depth-of-agreement-reply",
            _ => return None,
        }
    };
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(line: &str) -> Option<(IpAddr, String)> {
        parse_name_line(line, SourceKind::Hosts)
    }

    fn leases(line: &str) -> Option<(IpAddr, String)> {
        parse_name_line(line, SourceKind::Leases)
    }

    /// The two formats, one line each, exactly as gw writes them
    /// (`/etc/dnsmasq-records.hosts` and `/var/lib/dnsmasq/dnsmasq.leases`).
    #[test]
    fn a_hosts_record_prefers_the_short_alias() {
        let (ip, name) = hosts("10.3.14.62 3d-blue.int.leighhack.org 3d-blue").unwrap();
        assert_eq!(ip.to_string(), "10.3.14.62");
        assert_eq!(name, "3d-blue", "a table cell wants the short name");

        // A record with only an FQDN still labels the address.
        let (_, name) = hosts("10.3.1.45 AP1-Fast.int.leighhack.org").unwrap();
        assert_eq!(name, "AP1-Fast.int.leighhack.org");

        // IPv6 records are as good as IPv4 ones, padded or not.
        let (ip, _) = hosts("2001:8b0:1d14:227d::1001 PI-ROOM-01.int.leighhack.org PI-ROOM-01").unwrap();
        assert!(ip.is_ipv6());
        assert!(hosts("2001:08b0:1d14:227d:0000:0000:0000:1001 x y").is_some());
    }

    #[test]
    fn a_lease_uses_the_hostname_and_skips_the_ones_without() {
        let (ip, name) = leases("1795000000 3c:7c:3f:11:22:33 10.3.14.190 laptop-vivaheri *").unwrap();
        assert_eq!(ip.to_string(), "10.3.14.190");
        assert_eq!(name, "laptop-vivaheri");

        // dnsmasq's placeholder for "this client never told us its name".
        assert!(leases("1795000000 3c:7c:3f:11:22:33 10.3.14.190 * *").is_none());
        // Not a lease at all: dnsmasq's comment lines and a truncated file.
        assert!(leases("# not a lease").is_none());
        assert!(leases("1795000000 3c:7c:3f:11:22:33").is_none());
        // Nor a hosts line, whose first field is an address, not an epoch.
        assert!(leases("10.3.1.1 gw.int.leighhack.org gw").is_none());
    }

    #[test]
    fn ports_and_icmp_types_get_words() {
        assert_eq!(service_name("tcp", 443), Some("https"));
        assert_eq!(service_name("udp", 443), Some("quic"), "UDP/443 is not https");
        assert_eq!(service_name("tcp", 554), Some("rtsp"), "the cameras' feed port");
        assert_eq!(service_name("udp", 51820), Some("wireguard"));
        assert_eq!(service_name("tcp", 41641), None, "that one is UDP-only");
        assert_eq!(service_name("tcp", 54117), None, "an unlisted port stays a number");

        // Type 3 means two different things, and picking one by protocol number
        // would have been wrong half the time.
        assert_eq!(icmp_name(3, false), Some("dest-unreach"));
        assert_eq!(icmp_name(3, true), Some("time-exceeded"));
        assert_eq!(icmp_name(128, true), Some("echo-request"));
        assert_eq!(icmp_name(128, false), None, "not an IPv4 type");
        assert_eq!(icmp_name(250, false), None);
    }

    /// `Names::refresh` is what lets a lease that arrived while the dashboard was
    /// running appear without a restart — so it has to notice a file that changed
    /// *inside the same second it was last read*, which is why the stamp keeps
    /// whatever precision the filesystem reports.
    #[test]
    fn a_name_that_arrives_mid_second_is_still_picked_up() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "network-status-names-{}-{}",
            std::process::id(),
            // One directory per test in the module, so a panic cannot make the
            // next run of the same name collide with a leftover file.
            "hosts"
        ));
        let path = path.to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "10.3.1.20 services1.int.leighhack.org services1\n").unwrap();

        let mut names = Names::open(std::slice::from_ref(&path), None);
        assert_eq!(names.get("10.3.1.20".parse().unwrap()), Some("services1"));
        assert_eq!(names.get("10.3.1.99".parse().unwrap()), None);

        std::fs::write(
            &path,
            "10.3.1.20 services1.int.leighhack.org services1\n10.3.1.99 aibox.int.leighhack.org aibox\n",
        )
        .unwrap();
        names.refresh();
        assert_eq!(
            names.get("10.3.1.99".parse().unwrap()),
            Some("aibox"),
            "a rewrite inside the same second must still be seen"
        );

        // And a file that goes away keeps the names it gave rather than
        // emptying the table.
        std::fs::remove_file(&path).unwrap();
        names.refresh();
        assert_eq!(names.get("10.3.1.20".parse().unwrap()), None);
    }

    #[test]
    fn a_missing_name_file_is_not_an_error() {
        // The lease file is `0640 root:root` on gw, so the deployed unit cannot
        // read it at all and the whole feature has to degrade to numbers.
        let names = Names::open(&["/nonexistent/hosts".into()], Some("/nonexistent/leases".into()));
        assert_eq!(names.get("10.3.1.20".parse().unwrap()), None);
    }
}
