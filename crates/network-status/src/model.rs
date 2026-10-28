//! The snapshot's data model, the "potential issues" list, and the one way this
//! program turns a counter into a rate.
//!
//! [`rate`] is deliberately the only implementation of "bytes/sec since the last
//! sample": interface counters and per-connection byte counters have the same
//! failure mode (a reboot or a reused tuple takes the counter backwards) and
//! the two call sites must not disagree about what that means.


use crate::flows::Flows;
use crate::Args;
use crate::MAX_HISTORY;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) struct Iface {
    pub(crate) name: String,
    pub(crate) active: bool,
    pub(crate) ibytes: u64,
    pub(crate) obytes: u64,
    pub(crate) ierrors: u64,
    pub(crate) oerrors: u64,
    /// Bytes/sec since the previous sample (None on the first sample).
    pub(crate) down_bps: Option<f64>,
    pub(crate) up_bps: Option<f64>,
    /// New interface errors (in+out) during the last interval.
    pub(crate) err_delta: u64,
}

#[derive(Clone)]
pub(crate) struct Issue {
    pub(crate) level: String, // "bad" | "warn"
    pub(crate) message: String,
}

#[derive(Clone, Default)]
pub(crate) struct Snapshot {
    pub(crate) ts: i64,
    /// Whether the most recent ssh probe succeeded.
    pub(crate) ok: bool,
    pub(crate) error: Option<String>,
    pub(crate) uptime_secs: Option<u64>,
    pub(crate) load: Option<[f64; 3]>,
    /// Number of CPUs (`nproc`), used for the load-per-CPU issue.  The field
    /// was called `nprocs` when OPNsense's `top` supplied a process count.
    pub(crate) ncpu: Option<u32>,
    pub(crate) cpu_user: Option<f64>,
    pub(crate) cpu_sys: Option<f64>,
    pub(crate) cpu_idle: Option<f64>,
    /// Free memory as the kernel's `MemAvailable`; `total` is `MemTotal`.
    pub(crate) mem_free: Option<u64>,
    pub(crate) mem_total: Option<u64>,
    /// Entries in the nftables conntrack table (OPNsense called this the pf
    /// state table; the number means the same thing).
    pub(crate) conntrack: Option<u64>,
    pub(crate) own_tcp: Option<u64>,
    pub(crate) retrans_rate: Option<f64>, // retransmissions per second
    pub(crate) retrans_total: Option<u64>, // cumulative, for diffing between samples
    pub(crate) ifaces: Vec<Iface>,
    pub(crate) issues: Vec<Issue>,
}

pub(crate) struct HistoryPoint {
    pub(crate) ts: i64,
    /// interface -> (up_bps, down_bps)
    pub(crate) rates: BTreeMap<String, (Option<f64>, Option<f64>)>,
    /// conntrack table size (OPNsense's pf state count).
    pub(crate) conntrack: Option<u64>,
    pub(crate) load1: Option<f64>,
}

pub(crate) struct State {
    pub(crate) last: Mutex<Option<Snapshot>>,
    pub(crate) history: Mutex<VecDeque<HistoryPoint>>,
    /// The connection tracker (see `flows.rs`): the conntrack table of the last
    /// good sample, with the byte counters differenced against the sample before
    /// it.  Only the sampler writes it, so a handler only ever reads a completed
    /// sample.
    pub(crate) flows: Mutex<Flows>,
}

impl State {
    /// `hosts_files` / `leases_file` are the name sources the tracker reads
    /// (see [`crate::names::Names`]).
    pub(crate) fn new(hosts_files: &[String], leases_file: Option<String>) -> State {
        State {
            last: Mutex::new(None),
            history: Mutex::new(VecDeque::with_capacity(MAX_HISTORY)),
            flows: Mutex::new(Flows::new(hosts_files, leases_file)),
        }
    }
}

// ---------------------------------------------------------------------------
// Sampling
// ---------------------------------------------------------------------------

/// Bytes/sec between two counter readings; a counter going backwards
/// (router reboot) is treated as traffic since zero.
pub(crate) fn rate(cur: u64, prev: u64, dt: f64) -> Option<f64> {
    if dt <= 0.0 {
        return None;
    }
    let bytes = if cur < prev { cur } else { cur - prev };
    Some(bytes as f64 / dt)
}

pub(crate) fn compute_issues(args: &Args, snap: &mut Snapshot) {
    snap.issues.clear();

    // WAN link down is the big one.  On gw the WAN is `ppp0`, and pppd deletes
    // the interface when the tunnel drops — so "no entry" is the down signal,
    // not just "entry with carrier 0".
    if snap.ok {
        match snap.ifaces.iter().find(|i| i.name == args.wan) {
            Some(wan) if !wan.active => snap.issues.push(Issue {
                level: "bad".into(),
                message: format!("WAN interface {} has no link", wan.name),
            }),
            None => snap.issues.push(Issue {
                level: "bad".into(),
                message: format!("WAN interface {} is not present", args.wan),
            }),
            _ => {}
        }
    }

    // New interface errors on an active link.
    for i in &snap.ifaces {
        if i.active && i.err_delta > 0 {
            snap.issues.push(Issue {
                level: "warn".into(),
                message: format!(
                    "{}: {} new interface error(s) in the last interval",
                    i.name, i.err_delta
                ),
            });
        }
    }

    // Sustained TCP retransmissions.
    if let Some(r) = snap.retrans_rate {
        if r > 0.5 {
            snap.issues.push(Issue {
                level: "warn".into(),
                message: format!("high TCP retransmit rate on router: {r:.2}/s"),
            });
        }
    }

    // Load above CPU count.
    if let (Some(load), Some(n)) = (snap.load, snap.ncpu) {
        if load[0] > f64::from(n) {
            snap.issues.push(Issue {
                level: "warn".into(),
                message: format!("load {:.2} above {} CPUs (1 min)", load[0], n),
            });
        }
    }

    // Memory pressure: < 10% free.
    if let (Some(free), Some(total)) = (snap.mem_free, snap.mem_total) {
        if total > 0 && free * 10 < total {
            snap.issues.push(Issue {
                level: "warn".into(),
                message: format!(
                    "router memory low: {} free of {} total",
                    fmt_bytes(free),
                    fmt_bytes(total)
                ),
            });
        }
    }

    // Router unreachable (set by the sampler on ssh failure).
    if !snap.ok {
        let msg = snap.error.clone().unwrap_or_else(|| "cannot reach router".into());
        if !snap.issues.iter().any(|i| i.message == msg) {
            snap.issues.insert(0, Issue {
                level: "bad".into(),
                message: format!("cannot reach router: {msg}"),
            });
        }
    }

}

/// Human byte formatting for issue messages (server-side, only for warnings).
pub(crate) fn fmt_bytes(b: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let v = b as f64;
    if v >= GB {
        format!("{:.1} GiB", v / GB)
    } else if v >= MB {
        format!("{:.0} MiB", v / MB)
    } else if v >= KB {
        format!("{:.0} KiB", v / KB)
    } else {
        format!("{b} B")
    }
}


#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::nets::Localness;
    use std::time::Duration;

    pub(crate) fn test_args() -> Args {
        Args {
            bind: "127.0.0.1".into(),
            port: 0,
            router: "leigh-admin@10.3.1.1".into(),
            ssh_key: String::new(),
            known_hosts: String::new(),
            local: true,
            hostname: "gw".into(),
            interval: Duration::from_secs(5),
            wan: "ppp0".into(),
            title: "network-info".into(),
            static_dir: None,
            conns: true,
            local_nets: Localness::new(
                &"10.3.1.0/24 10.3.2.0/24 10.3.14.0/24"
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect::<Vec<String>>(),
            ),
            conn_min_age: 5,
            conn_min_bytes: 0,
            conn_max_rows: 200,
            conn_stale: 15,
            hosts_files: Vec::new(),
            leases: true,
            lease_ttl: 86_400,
            lease_max_rows: 500,
            leases_file: None,
        }
    }

    pub(crate) fn iface(name: &str, active: bool) -> Iface {
        Iface {
            name: name.into(),
            active,
            ibytes: 0,
            obytes: 0,
            ierrors: 0,
            oerrors: 0,
            down_bps: None,
            up_bps: None,
            err_delta: 0,
        }
    }

    #[test]
    fn missing_wan_interface_is_reported() {
        let args = test_args();

        // ppp0 gone: pppd deletes the interface when the tunnel drops.
        let mut snap = Snapshot {
            ok: true,
            ifaces: vec![iface("vlan225", true)],
            ..Default::default()
        };
        compute_issues(&args, &mut snap);
        assert!(
            snap.issues.iter().any(|i| i.level == "bad" && i.message.contains("not present")),
            "missing WAN should be an issue: {:?}",
            snap.issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );

        // ppp0 present but no carrier.
        let mut snap = Snapshot {
            ok: true,
            ifaces: vec![iface("ppp0", false)],
            ..Default::default()
        };
        compute_issues(&args, &mut snap);
        assert!(snap.issues.iter().any(|i| i.level == "bad" && i.message.contains("no link")));

        // A failed probe already reports "cannot reach router"; it must not
        // also claim the WAN is missing.
        let mut snap = Snapshot {
            ok: false,
            error: Some("ssh failed".into()),
            ..Default::default()
        };
        compute_issues(&args, &mut snap);
        assert!(!snap.issues.iter().any(|i| i.message.contains("not present")));
    }
}
