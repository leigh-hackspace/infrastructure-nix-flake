//! Ranking, in one place.
//!
//! The score is deliberately explainable: a measured throughput term, a
//! latency term, a small class bonus, and a hysteresis bonus for the path that
//! is already selected. A candidate is only *eligible* when its own gateway
//! answers and an internet anchor answers through it; everything else is shown
//! in the dashboard with the reason it was rejected.
//!
//! The asymmetry is the same one the 2026-11-07 reconciler learned: fail over
//! immediately, fail back only after a challenger beats the incumbent by a
//! clear margin. Every switch rewrites the NAT source address of every open
//! connection, so a marginal difference is not a reason to move the network.

use crate::net::{IfaceInfo, Kind};
use crate::probe::Probe;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Candidate {
    pub info: IfaceInfo,
    pub probe: Probe,
    pub score: f64,
    pub last_measured: Option<Instant>,
    /// Consecutive ticks where this path was needed but its gateway was silent.
    pub failures: u32,
    /// The measured throughput from the last successful sample, kept across a
    /// failed probe so a momentary loss does not erase the ranking evidence.
    pub throughput_bps: Option<f64>,
}

impl Candidate {
    pub fn new(info: IfaceInfo) -> Self {
        Candidate {
            info,
            probe: Probe::default(),
            score: 0.0,
            last_measured: None,
            failures: 0,
            throughput_bps: None,
        }
    }

    pub fn eligible(&self) -> bool {
        match self.info.kind {
            Kind::Tunnel | Kind::Relay => self.info.link_up && self.probe.internet_ok,
            _ => {
                self.info.link_up
                    && self.info.gateway.is_some()
                    && self.probe.gateway_ok
                    && self.probe.internet_ok
            }
        }
    }

    pub fn score(&self) -> f64 {
        if !self.eligible() {
            return 0.0;
        }
        let mbps = self.throughput_bps.unwrap_or(0.0) / 1_000_000.0;
        let latency = self.probe.latency_ms.unwrap_or(500.0);
        // Throughput is the dominant term below ~10 Mbit/s (the hotspot's
        // measured ceiling) and the latency term breaks ties among slow paths.
        // The class bonus is small on purpose: it decides between two paths
        // that measured the same, not between a working cable and a faster one.
        let mut s = mbps * 100.0
            + 1000.0 / (latency + 20.0)
            + self.info.kind.preference();
        // A path with a live tunnel session is proof the path carries traffic;
        // `ppp0` itself has no gateway to probe, so this is its equivalent.
        if self.info.kind == Kind::Tunnel && self.probe.internet_ok {
            s += 80.0;
        }
        s
    }

    pub fn reason(&self) -> String {
        if !self.info.link_up {
            return "link down".into();
        }
        if !self.info.kind.is_gatewayless() {
            if self.info.gateway.is_none() {
                return "no DHCP gateway offered".into();
            }
            if !self.probe.gateway_ok {
                return "gateway silent".into();
            }
        }
        if !self.probe.internet_ok {
            return "no internet through this path".into();
        }
        "viable".into()
    }
}

#[derive(Debug, Clone)]
pub struct Selection {
    /// The best non-tunnel egress: the default in the main table and table 200.
    pub best: Option<usize>,
    /// The AAISP tunnel, when it is carrying packets. It is selected separately
    /// because it is the egress for VLAN 225 (mark 1) and for public inbound.
    pub tunnel: Option<usize>,
    /// "primary" (a wired path or the tunnel is carrying the network),
    /// "backup" (only a USB/Wi-Fi path is), "none".
    pub state: String,
}

pub fn update(
    candidates: &mut [Candidate],
    previous_best: Option<&str>,
    previous_tunnel: Option<&str>,
    _throughput_interval: Duration,
) -> Selection {
    // Throughput is measured by the caller before this runs: the measurement
    // needs `&mut candidates`, and borrowing that twice would make the ranking
    // function own the probe schedule as well as the decision.

    for c in candidates.iter_mut() {
        c.score = c.score();
    }

    let incumbent = previous_best.and_then(|name| candidates.iter().position(|c| c.info.name == name));
    let incumbent_score = incumbent
        .map(|i| candidates[i].score)
        .unwrap_or(0.0);

    // Non-tunnel egress: highest score, with the incumbent keeping the seat
    // unless a challenger is at least 20 % better. The bonus is applied only
    // when the incumbent is still eligible, so a dead path cannot hold on.
    let mut best: Option<usize> = None;
    for (i, c) in candidates.iter().enumerate() {
        if c.info.kind == Kind::Tunnel || !c.eligible() {
            continue;
        }
        let s = if Some(i) == incumbent && c.eligible() {
            c.score * 1.20
        } else {
            c.score
        };
        let better = match best {
            None => true,
            Some(b) => s > {
                let bs = if Some(b) == incumbent {
                    candidates[b].score * 1.20
                } else {
                    candidates[b].score
                };
                bs
            },
        };
        if better {
            best = Some(i);
        }
    }
    // A tunnel alone is still a way out for the router's own traffic; it is not
    // a non-tunnel egress (no gateway to NAT to), but the caller may use it for
    // the main table when nothing else exists.
    let _ = incumbent_score;

    let tunnel_incumbent = previous_tunnel.and_then(|name| candidates.iter().position(|c| c.info.name == name));
    let mut tunnel: Option<usize> = None;
    for (i, c) in candidates.iter().enumerate() {
        if c.info.kind != Kind::Tunnel || !c.eligible() {
            continue;
        }
        let better = match tunnel {
            None => true,
            Some(t) => {
                let bonus = if Some(t) == tunnel_incumbent { 1.10 } else { 1.0 };
                c.score > candidates[t].score * bonus
            }
        };
        if better {
            tunnel = Some(i);
        }
    }

    let state = match best {
        Some(i) if matches!(candidates[i].info.kind, Kind::Wired) => "primary".into(),
        Some(_) => "backup".into(),
        None if tunnel.is_some() => "tunnel-only".into(),
        None => "none".into(),
    };
    Selection {
        best,
        tunnel,
        state,
    }
}
