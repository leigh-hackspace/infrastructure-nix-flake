//! How the statistics are read: one POSIX sh script, fed to `sh -s` locally or
//! over ssh, and its output parsed back into [`Parsed`].
//!
//! The script is a string rather than Rust because the router is the box being
//! read and `cat`ping `/proc` is what needs no dependency, no privilege and no
//! per-kernel-version parser — with the one exception being `/proc/net/nf_conntrack`,
//! which is kept as text here and parsed over in `conntrack.rs`.

use crate::model::{compute_issues, rate, Iface, Snapshot};
use crate::Args;
use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long a probe may run before it is killed.  Generous because the ssh mode
/// pays a connection setup as well as the script, and mean because the local one
/// has never in living memory taken a tenth of it.
pub(crate) const SSH_TIMEOUT: Duration = Duration::from_secs(20);

/// Collection script, fed to `ssh ... 'sh -s'` on the router.  gw is NixOS, so
/// this is POSIX sh reading `/proc` and `/sys` — no platform commands, no root,
/// and no sudo: every file here is world-readable.
pub(crate) const REMOTE_SCRIPT: &str = r#"
echo ==IFACES==
cat /proc/net/dev
echo ==STAT==
head -1 /proc/stat
echo ==MEM==
grep -E '^(MemTotal|MemFree|MemAvailable):' /proc/meminfo
echo ==LOAD==
cat /proc/loadavg
echo ==UPTIME==
cat /proc/uptime
echo ==CONNTRACK==
cat /proc/sys/net/netfilter/nf_conntrack_count 2>/dev/null || true
echo ==RETRANS==
awk '/^Tcp:/ {n++; if (n==2) {print $13; exit}}' /proc/net/snmp
echo ==LINKS==
for d in /sys/class/net/*; do
  n=${d##*/}
  c=$(cat "$d/carrier" 2>/dev/null || echo 1)
  echo "$n $c"
done
echo ==SOCKETS==
awk 'FNR>1 {n++} END {print n+0}' /proc/net/tcp /proc/net/tcp6
echo ==NCPU==
nproc
"#;

/// Loopback is never interesting.  ppp0 must not be filtered out: its counters
/// only exist while the tunnel does, and its disappearance is the WAN-down
/// signal.
pub(crate) const IGNORED_IFACES: [&str; 1] = ["lo"];

// ---------------------------------------------------------------------------
// Router probe
// ---------------------------------------------------------------------------

pub(crate) fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Feed `script` to a command's stdin and collect its stdout.  Used for both
/// probe modes: `ssh ... sh -s` to another box, or `sh -s` on this one.
///
/// stdout is drained on a thread for the child's whole life, which is not
/// paranoia: the conntrack section alone is ~140 KB on a quiet gw (measured
/// 2026-10-25) and over 500 KB when the table is busy, against a 64 KB pipe
/// buffer.  A parent that waited for the child to exit before reading would
/// watch `cat` block on a full pipe forever and then report a probe timeout —
/// and a timeout, unlike this bug, looks like the router's fault.
pub(crate) fn run_probe(
    mut command: Command,
    script: &str,
    what: &str,
    hint: &str,
) -> Result<String, String> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn {what}: {e}"))?;
    // Written and then closed (`stdin` is dropped here): the script reads its
    // own text from stdin, and leaving it open would keep `sh` waiting.
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("{what}: no stdin to write the probe script to"))?;
        if let Err(e) = stdin.write_all(script.as_bytes()) {
            let _ = child.kill();
            return Err(format!("failed to write probe script to {what}: {e}"));
        }
    }
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{what}: no stdout to read"))?;
    let reader = thread::spawn(move || {
        let mut out = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut out);
        out
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().ok().flatten() {
            break status;
        }
        if start.elapsed() > SSH_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            // The reader may still be blocked on a pipe nothing is writing to
            // any more; it is woken by the kill, and its result is discarded.
            let _ = reader.join();
            return Err(format!("{what} probe timed out"));
        }
        thread::sleep(Duration::from_millis(20));
    };
    let out = reader
        .join()
        .map_err(|_| format!("{what}: the stdout reader panicked"))?;
    if status.success() && !out.trim().is_empty() {
        Ok(out)
    } else if status.success() {
        Err(format!("empty output from {what} probe"))
    } else {
        Err(format!("{what} failed{hint}"))
    }
}

/// The probe with the conntrack section added or dropped, which is what
/// `--no-conns` does.
///
/// The section is read with `2>/dev/null || true` for two separate reasons:
/// `/proc/net/nf_conntrack` does not exist on a box without `nf_conntrack`
/// loaded (this used to be pointed at an OPNsense box with pf states instead),
/// and on Linux it is `0440 root:root`, so a process the router's
/// `conntrack-readers` group has not been added to reads nothing rather than
/// dying.
pub(crate) fn probe_script(conns: bool) -> String {
    if conns {
        format!("{REMOTE_SCRIPT}{CONNTRACK_SCRIPT}")
    } else {
        REMOTE_SCRIPT.to_string()
    }
}

/// The conntrack table, appended to [`REMOTE_SCRIPT`] unless `--no-conns`.
///
/// Requires `net.netfilter.nf_conntrack_acct = 1` for the `bytes=`/`packets=`
/// fields this is read for — the counters, not the addresses, are the whole
/// point of the section — and requires the reading user's supplementary group
/// membership, which `machines/gw/network-status.nix` arranges for the deployed
/// unit.  See `connections.rs` for what is done with it.
pub(crate) const CONNTRACK_SCRIPT: &str = r#"
echo ==CONNS==
cat /proc/net/nf_conntrack 2>/dev/null || true
"#;

/// Run the collection script on another box over ssh; returns its stdout.
pub(crate) fn fetch_router(
    ssh_key: &str,
    known_hosts: &str,
    router: &str,
    script: &str,
) -> Result<String, String> {
    let mut command = Command::new("ssh");
    command
        .arg("-i")
        .arg(ssh_key)
        .args(["-o", "BatchMode=yes"])
        .args(["-o", "ConnectTimeout=10"])
        // Host identity is pinned, not TOFU: the probe ssh's to the router as
        // leigh-admin, and `accept-new` would silently re-trust a key after a
        // router reinstall (or an on-path impersonation).  Nothing ships the pin
        // any more — it used to be /etc/dns-sync/known_hosts, written by a tool
        // that edited DNS over ssh and is gone (see common/int-dns.nix) — so the
        // path below is generated by `just router-known-hosts` and kept by hand.
        // A missing file is no ssh rather than an unverified connection, which is
        // the right default for a mode nobody runs in production.
        .args([
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            &format!("UserKnownHostsFile={known_hosts}"),
        ])
        .arg(router)
        .arg("sh -s");
    run_probe(command, script, "ssh to router", " (check key/ssh access)")
}

/// Run the collection script on this machine.  This is the mode gw uses: the
/// dashboard runs on the router, so the statistics are local and there is no
/// ssh key, no host-key pin and no service account to keep.
pub(crate) fn fetch_local(script: &str) -> Result<String, String> {
    let mut command = Command::new("sh");
    command.arg("-s");
    run_probe(command, script, "local", "")
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct Parsed {
    /// name -> (ibytes, obytes, ierrors, oerrors)
    pub(crate) ifaces: BTreeMap<String, (u64, u64, u64, u64)>,
    pub(crate) links: BTreeMap<String, bool>,
    pub(crate) uptime_secs: Option<u64>,
    pub(crate) load: Option<[f64; 3]>,
    pub(crate) ncpu: Option<u32>,
    pub(crate) cpu_user: Option<f64>,
    pub(crate) cpu_sys: Option<f64>,
    pub(crate) cpu_idle: Option<f64>,
    pub(crate) mem_free: Option<u64>,
    pub(crate) mem_total: Option<u64>,
    pub(crate) conntrack: Option<u64>,
    pub(crate) own_tcp: Option<u64>,
    pub(crate) retrans_total: Option<u64>,
    /// The `==CONNS==` section verbatim, for `conntrack::parse_table`.
    /// Deliberately kept as text rather than parsed here: the tracker owns that,
    /// and its own tests own the samples it is fed.
    pub(crate) conns: String,
}

/// Parse a `/proc/meminfo` value (kB) into bytes.
pub(crate) fn parse_meminfo_kb(s: &str) -> Option<u64> {
    let kb: u64 = s.split_whitespace().next()?.parse().ok()?;
    Some(kb * 1024)
}

pub(crate) fn parse_output(raw: &str) -> Parsed {
    let mut p = Parsed::default();
    let mut section = "";
    for line in raw.lines() {
        let t = line.trim_start();
        if let Some(name) = t.strip_prefix("==") {
            if let Some(end) = name.find("==") {
                section = &name[..end];
                continue;
            }
        }
        if t.is_empty() {
            continue;
        }
        match section {
            "IFACES" => {
                // "name: rx_bytes rx_packets rx_errs rx_drop ... tx_bytes tx_packets tx_errs ..."
                let Some((name, rest)) = t.split_once(':') else {
                    continue; // the two header lines
                };
                let name = name.trim();
                if name.is_empty() || IGNORED_IFACES.contains(&name) {
                    continue;
                }
                let toks: Vec<u64> = rest
                    .split_whitespace()
                    .filter_map(|v| v.parse().ok())
                    .collect();
                if toks.len() < 16 {
                    continue;
                }
                // rest index 0 = rx bytes: rx errs 2, tx bytes 8, tx errs 10.
                p.ifaces
                    .entry(name.to_string())
                    .or_insert((toks[0], toks[8], toks[2], toks[10]));
            }
            "STAT" => {
                // "cpu user nice system idle iowait irq softirq steal ..." —
                // cumulative since boot, so these percentages are the since-boot
                // averages `top -b`'s CPU line used to report on OPNsense.
                let toks: Vec<u64> = t
                    .split_whitespace()
                    .skip(1)
                    .filter_map(|v| v.parse().ok())
                    .collect();
                if toks.len() >= 7 {
                    let (user, nice, system) = (toks[0], toks[1], toks[2]);
                    let (idle, iowait, irq, softirq) = (toks[3], toks[4], toks[5], toks[6]);
                    let steal = toks.get(7).copied().unwrap_or(0);
                    let total_f =
                        (user + nice + system + idle + iowait + irq + softirq + steal) as f64;
                    if total_f > 0.0 {
                        p.cpu_user = Some((user + nice) as f64 * 100.0 / total_f);
                        p.cpu_sys = Some((system + irq + softirq + steal) as f64 * 100.0 / total_f);
                        p.cpu_idle = Some((idle + iowait) as f64 * 100.0 / total_f);
                    }
                }
            }
            "MEM" => {
                // "MemTotal: 8104932 kB" / "MemAvailable: ..."
                let (key, value) = match t.split_once(':') {
                    Some((k, v)) => (k.trim(), v),
                    None => continue,
                };
                match key {
                    "MemTotal" => p.mem_total = parse_meminfo_kb(value),
                    // `MemAvailable` is the kernel's own estimate of what a new
                    // workload could claim (it accounts for reclaimable cache);
                    // `MemFree` is the fallback on a kernel without it.
                    "MemAvailable" => p.mem_free = parse_meminfo_kb(value),
                    "MemFree" if p.mem_free.is_none() => p.mem_free = parse_meminfo_kb(value),
                    _ => {}
                }
            }
            "LOAD" => {
                // "1.04 0.39 0.22 1/233 33386"
                let loads: Vec<f64> = t
                    .split_whitespace()
                    .take(3)
                    .filter_map(|v| v.parse().ok())
                    .collect();
                if loads.len() == 3 {
                    p.load = Some([loads[0], loads[1], loads[2]]);
                }
            }
            "UPTIME" => {
                // "11736.64 41971.75" — seconds since boot, then idle.
                if let Some(first) = t.split_whitespace().next() {
                    p.uptime_secs = first.parse::<f64>().ok().map(|v| v as u64);
                }
            }
            "CONNTRACK" => {
                if let Some(first) = t.split_whitespace().next() {
                    p.conntrack = first.parse().ok();
                }
            }
            "RETRANS" => {
                if let Some(first) = t.split_whitespace().next() {
                    if p.retrans_total.is_none() {
                        p.retrans_total = first.parse().ok();
                    }
                }
            }
            "LINKS" => {
                // "<iface> <carrier>" from /sys/class/net/*/carrier.  A carrier
                // file that does not exist (ppp0, tailscale0) means the device
                // exists and is up, so the script prints 1 for it.
                let mut it = t.split_whitespace();
                if let (Some(name), Some(carrier)) = (it.next(), it.next()) {
                    p.links.insert(name.to_string(), carrier != "0");
                }
            }
            "SOCKETS" => {
                if let Ok(n) = t.parse::<u64>() {
                    p.own_tcp = Some(n);
                }
            }
            "CONNS" => {
                // Verbatim; `conntrack::parse_table` owns the format.  This
                // is the only section kept as text rather than parsed here, and
                // the only one that can be hundreds of kilobytes.
                if !p.conns.is_empty() {
                    p.conns.push('\n');
                }
                p.conns.push_str(t);
            }
            "NCPU" => {
                if let Ok(n) = t.parse::<u32>() {
                    p.ncpu = Some(n);
                }
            }
            _ => {}
        }
    }

    p
}

/// `leigh-admin@10.3.1.1` -> `10.3.1.1`.
pub(crate) fn router_host(router: &str) -> &str {
    router.rsplit('@').next().unwrap_or(router)
}

/// The hostname reported in local mode (`gw`).
pub(crate) fn local_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "router".into())
}
pub(crate) fn sample_once(args: &Args, prev: &Option<Snapshot>) -> Sample {
    let ts = now_ts();
    let script = probe_script(args.conns);
    let probe = if args.local {
        fetch_local(&script)
    } else {
        fetch_router(&args.ssh_key, &args.known_hosts, &args.router, &script)
    };
    match probe {
        Ok(raw) => {
            let parsed = parse_output(&raw);
            let dt = prev.as_ref().map(|p| (ts - p.ts) as f64).unwrap_or(0.0);

            let mut ifaces = Vec::new();
            for (name, (ib, ob, ie, oe)) in &parsed.ifaces {
                let (down_bps, up_bps, err_delta) = match prev {
                    Some(p) if dt > 0.0 => {
                        let prev_c = p
                            .ifaces
                            .iter()
                            .find(|i| &i.name == name)
                            .map(|i| (i.ibytes, i.obytes, i.ierrors + i.oerrors));
                        match prev_c {
                            Some((pib, pob, perr)) => (
                                rate(*ib, pib, dt),
                                rate(*ob, pob, dt),
                                (ie + oe).saturating_sub(perr),
                            ),
                            None => (None, None, 0),
                        }
                    }
                    _ => (None, None, 0),
                };
                let active = parsed.links.get(name).copied().unwrap_or(false);
                ifaces.push(Iface {
                    name: name.clone(),
                    active,
                    ibytes: *ib,
                    obytes: *ob,
                    ierrors: *ie,
                    oerrors: *oe,
                    down_bps,
                    up_bps,
                    err_delta,
                });
            }
            ifaces.sort_by(|a, b| a.name.cmp(&b.name));

            let retrans_rate = match (parsed.retrans_total, prev) {
                (Some(t), Some(p)) if dt > 0.0 => match p.retrans_total {
                    Some(pt) if t >= pt => Some((t - pt) as f64 / dt),
                    _ => None,
                },
                _ => None,
            };

            let mut snap = Snapshot {
                ts,
                ok: true,
                error: None,
                uptime_secs: parsed.uptime_secs,
                load: parsed.load,
                ncpu: parsed.ncpu,
                cpu_user: parsed.cpu_user,
                cpu_sys: parsed.cpu_sys,
                cpu_idle: parsed.cpu_idle,
                mem_free: parsed.mem_free,
                mem_total: parsed.mem_total,
                conntrack: parsed.conntrack,
                own_tcp: parsed.own_tcp,
                retrans_rate,
                retrans_total: parsed.retrans_total,
                ifaces,
                issues: Vec::new(),
            };
            compute_issues(args, &mut snap);
            Sample {
                snap,
                // Only `Some` when the probe was asked for the table: a probe of
                // a box without conntrack must not look like a router whose
                // connections have all died.
                conns: if args.conns { Some(parsed.conns) } else { None },
            }
        }
        Err(e) => {
            // Keep the last snapshot, mark it stale.
            let mut snap = match prev {
                Some(p) => p.clone(),
                None => Snapshot {
                    ts,
                    ..Default::default()
                },
            };
            snap.ok = false;
            snap.error = Some(e);
            compute_issues(args, &mut snap);
            Sample {
                snap,
                // A failed probe says nothing about the connection table, so it
                // must not be read as "every connection died": the tracker
                // keeps whatever the last good sample said, and `/api/connections`
                // reports the sample it is standing on.
                conns: None,
            }
        }
    }
}

/// One turn of the sampler: the snapshot the API serves, plus the conntrack
/// table to fold into the connection tracker (`None` when the probe did not
/// read one, which must leave the tracker alone rather than empty it).
pub(crate) struct Sample {
    pub(crate) snap: Snapshot,
    pub(crate) conns: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    const SAMPLE: &str = r#"
==IFACES==
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 1735563   13584    0    0    0     0          0         0  1735563   13584    0    0    0     0       0          0
router-lan: 108335552750 106929170    0 5128    0     0          0     60252 285073487745 203019379    0    3    0     0       0          0
vlan225: 78197913655 14990952    0   60    0     0          0     30766 31484006958 23653606    0   20    0     0       0          0
  ppp0: 41152642209 29457831    0    0    0     0          0         0 927347146 7107417    0    0    0     0       0          0
==STAT==
cpu  275198 5 32126 4197173 4648 20764 154326 0 0 0
==MEM==
MemTotal:        8104932 kB
MemAvailable:    6843124 kB
==LOAD==
1.04 0.39 0.22 1/233 33386
==UPTIME==
11736.64 41971.75
==CONNTRACK==
2372
==RETRANS==
3098
==LINKS==
lo 1
ppp0 1
router-lan 1
vlan225 1
==SOCKETS==
32
==NCPU==
4
"#;

    #[test]
    fn parses_gw_probe() {
        let p = parse_output(SAMPLE);

        // lo is dropped; rx bytes / tx bytes / rx errs / tx errs are at the
        // right offsets in /proc/net/dev.
        assert!(!p.ifaces.contains_key("lo"));
        assert_eq!(p.ifaces.get("ppp0"), Some(&(41152642209, 927347146, 0, 0)));
        assert_eq!(
            p.ifaces.get("vlan225"),
            Some(&(78197913655, 31484006958, 0, 0))
        );

        assert_eq!(p.uptime_secs, Some(11736));
        assert_eq!(p.load, Some([1.04, 0.39, 0.22]));
        assert_eq!(p.ncpu, Some(4));
        assert_eq!(p.mem_total, Some(8104932 * 1024));
        assert_eq!(p.mem_free, Some(6843124 * 1024));
        assert_eq!(p.conntrack, Some(2372));
        assert_eq!(p.retrans_total, Some(3098));
        assert_eq!(p.own_tcp, Some(32));
        assert_eq!(p.links.get("ppp0"), Some(&true));

        // The three CPU shares come from the same cumulative counters and add
        // up to 100% (steal is folded into system).
        let sum = p.cpu_user.unwrap() + p.cpu_sys.unwrap() + p.cpu_idle.unwrap();
        assert!((sum - 100.0).abs() < 1e-9, "cpu shares sum to {sum}");
    }

    #[test]
    fn missing_conntrack_file_is_not_an_error() {
        // `cat ... 2>/dev/null || true` leaves the section empty when the
        // nf_conntrack module is not loaded; the rest of the snapshot is fine.
        let p = parse_output("==CONNTRACK==\n\n==NCPU==\n4\n");
        assert_eq!(p.conntrack, None);
        assert_eq!(p.ncpu, Some(4));
    }

    /// The conntrack section is the only one kept as text, and the only one big
    /// enough to matter.  Every row below was read off gw on 2026-10-25: a UDP
    /// flow with `acct` on, a TCP flow created before `acct` was (no counters,
    /// and `Option::None` rather than zero), and ICMP in both families — which
    /// is where `type=`/`code=`/`id=` come from instead of ports, and where the
    /// kernel's protocol name changes from `icmp` to `icmpv6`.
    #[test]
    fn conntrack_section_is_kept_verbatim() {
        let raw = "==CONNS==\n\
ipv4     2 udp      17 4 src=10.3.14.215 dst=10.3.1.166 sport=43593 dport=5060 packets=3 bytes=2310 [UNREPLIED] src=10.3.1.166 dst=10.3.14.215 sport=5060 dport=43593 packets=0 bytes=0 mark=0 zone=0 use=2\n\
ipv4     2 tcp      6 431999 ESTABLISHED src=10.3.1.20 dst=10.3.2.6 sport=37148 dport=554 src=10.3.2.6 dst=10.3.1.20 sport=554 dport=37148 [ASSURED] mark=0 zone=0 use=2\n\
ipv4     2 icmp     1 28 src=10.3.1.20 dst=1.0.0.1 type=8 code=0 id=17139 packets=2 bytes=168 src=1.0.0.1 dst=81.187.223.123 type=0 code=0 id=17139 packets=2 bytes=168 [ASSURED] mark=0 zone=0 use=2\n\
ipv6     10 icmpv6   58 29 src=2001:08b0:1d14:0225:0000:0000:0000:0001 dst=2606:4700:4700:0000:0000:0000:0000:1111 type=128 code=0 id=37128 packets=2 bytes=208 src=2606:4700:4700:0000:0000:0000:0000:1111 dst=2001:08b0:1d14:0225:0000:0000:0000:0001 type=129 code=0 id=37128 packets=2 bytes=208 [ASSURED] mark=0 zone=0 use=2\n\
==NCPU==\n4\n";
        let p = parse_output(raw);
        let (rows, unparsed) = crate::conntrack::parse_table(&p.conns);
        assert_eq!(unparsed, 0, "every row parsed: {:?}", p.conns);
        assert_eq!(rows.len(), 4);
        assert_eq!(p.ncpu, Some(4));
        assert!(
            !p.conns.contains("NCPU"),
            "the section ends at the next marker"
        );

        assert_eq!(rows[0].proto, "udp");
        assert_eq!(rows[0].state, None, "udp has no state word");
        assert_eq!(rows[0].orig.bytes, Some(2310));
        assert_eq!(rows[0].reply.bytes, Some(0), "[UNREPLIED]: counted, zero");

        assert_eq!(rows[1].state.as_deref(), Some("ESTABLISHED"));
        assert_eq!(rows[1].orig.bytes, None, "created before acct: no counters");
        assert_eq!(rows[1].timeout, 431999);

        assert_eq!(rows[2].proto, "icmp");
        assert_eq!(rows[2].orig.icmp_type, Some(8));
        assert_eq!(rows[2].reply.icmp_type, Some(0));
        assert_eq!(rows[2].orig.sport, None, "ICMP has no ports");

        assert_eq!(rows[3].proto, "icmpv6");
        assert_eq!(rows[3].orig.icmp_type, Some(128), "echo-request");
        assert_eq!(rows[3].orig.src.map(|i| i.is_ipv6()), Some(true));
        assert_eq!(rows[3].orig.bytes, Some(208));
    }
}
