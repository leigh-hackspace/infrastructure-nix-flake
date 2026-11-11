//! Ranking, in one place.
//!
//! The score is deliberately explainable: a measured throughput term, a
//! latency term, a small class bonus, and a hysteresis bonus for the path that
//! is already selected. A candidate is only *eligible* when its own gateway
//! answers and an internet anchor answers through it; everything else is shown
//! in the dashboard with the reason it was rejected.
//!
//! The asymmetry is the same one the 2026-11-07 reconciler learned: fail over
//! quickly, fail back only after a challenger beats the incumbent by a clear
//! margin. Every switch rewrites the NAT source address of every open
//! connection, so a marginal difference is not a reason to move the network.
//!
//! **"Immediately" was the bug.** Until 2026-11-09 the incumbent lost the seat
//! on the first tick its probe failed, and a challenger took it back as soon as
//! it measured 20 % better — so two marginal backups flapped the network every
//! few minutes, and (measured) each move stranded the AAISP L2TP session for
//! 40-90 s while pppd waited out six missed echoes. `History` therefore gives
//! both directions a *duration*: an incumbent that is still link-up but has
//! stopped answering keeps the seat for `Policy::failover_ticks`, and a
//! challenger must stay `margin` times better for `dwell` before it wins. A
//! link whose carrier is gone is not a probe blip and moves at once.

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

/// What the ranking remembers between ticks. This is the only state that makes
/// "do not move the network for a marginal difference" measurable rather than a
/// hope, and it is carried by the caller so `update` stays a pure decision.
#[derive(Debug, Default, Clone)]
pub struct History {
    /// Consecutive ticks the incumbent has been selected but ineligible.
    pub bad_ticks: u32,
    /// The path currently trying to earn the seat, and since when.
    pub challenger: Option<String>,
    pub challenger_since: Option<Instant>,
}

/// How long a decision has to keep looking the same before it is taken.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    /// Consecutive ineligible ticks before the network moves. `1` is the old
    /// behaviour (fail over on the first failed probe); the default is 3 ticks,
    /// i.e. 15 s at the 5 s interval.
    pub failover_ticks: u32,
    /// How long a challenger must stay `margin` times better before it takes a
    /// healthy incumbent's seat.
    pub dwell: Duration,
    /// How much better, as a factor of the incumbent's score.
    pub margin: f64,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            failover_ticks: 3,
            dwell: Duration::from_secs(600),
            margin: 1.5,
        }
    }
}

/// The best eligible non-tunnel egress, optionally ignoring one candidate.
fn best_egress(candidates: &[Candidate], skip: Option<usize>) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .filter(|(i, c)| Some(*i) != skip && c.info.kind != Kind::Tunnel && c.eligible())
        .max_by(|a, b| a.1.score.total_cmp(&b.1.score))
        .map(|(i, _)| i)
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
        // **Bytes** per second, not bits: `probe::throughput` returns curl's
        // byte count over the transfer phase. The name here used to read `mbps`,
        // which is how the unit survived unchecked. Dividing by 1e6 therefore
        // yields MB/s; the thresholds below are tuned against that.
        let mbytes = self.throughput_bps.unwrap_or(0.0) / 1_000_000.0;
        let latency = self.probe.latency_ms.unwrap_or(500.0);
        // Throughput is the dominant term and the latency term breaks ties among
        // paths of similar speed. The class bonus is small on purpose: it decides
        // between two paths that measured the same, not between a working cable
        // and a faster one.
        let mut s = mbytes * 100.0
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
        // A tunnel or relay with no IPv4 address is a link that is up and cannot
        // carry a packet: nothing binds. Reporting "no internet" for that hides
        // the only actionable fact — and that was the AAISP tunnel's real state
        // after a networkd restart flushed its negotiated address on 2026-11-09.
        if self.info.kind.is_gatewayless() && self.info.addresses.is_empty() {
            return "no IPv4 address (the link is up but IPCP has given it none)".into();
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
    history: &mut History,
    previous_best: Option<&str>,
    previous_tunnel: Option<&str>,
    policy: Policy,
) -> Selection {
    // Throughput is measured by the caller before this runs: the measurement
    // needs `&mut candidates`, and borrowing that twice would make the ranking
    // function own the probe schedule as well as the decision.

    for c in candidates.iter_mut() {
        c.score = c.score();
    }

    let incumbent = previous_best
        .and_then(|name| candidates.iter().position(|c| c.info.name == name))
        .filter(|i| candidates[*i].info.kind != Kind::Tunnel);

    // Non-tunnel egress, in three cases. The incumbent keeps the seat while it
    // is eligible; while it is merely *unreachable* it keeps it for a few ticks
    // (a lost probe is not an outage, and a move costs more than the tick it
    // saves); and a challenger only takes a healthy incumbent's seat after it
    // has stayed clearly better for the whole dwell.
    let best: Option<usize> = match incumbent {
        Some(i) if candidates[i].eligible() => {
            history.bad_ticks = 0;
            match best_egress(candidates, Some(i))
                .filter(|j| candidates[*j].score > candidates[i].score * policy.margin)
            {
                Some(j) => {
                    let name = candidates[j].info.name.clone();
                    if history.challenger.as_deref() != Some(name.as_str()) {
                        history.challenger = Some(name);
                        history.challenger_since = Some(Instant::now());
                    }
                    let ready = history
                        .challenger_since
                        .map(|t| t.elapsed() >= policy.dwell)
                        .unwrap_or(false);
                    if ready {
                        history.challenger = None;
                        history.challenger_since = None;
                        Some(j)
                    } else {
                        Some(i)
                    }
                }
                None => {
                    history.challenger = None;
                    history.challenger_since = None;
                    Some(i)
                }
            }
        }
        Some(i) => {
            history.challenger = None;
            history.challenger_since = None;
            if !candidates[i].info.link_up {
                // The link itself is gone. That is a fact, not a blip.
                history.bad_ticks = 0;
                best_egress(candidates, None)
            } else {
                history.bad_ticks = history.bad_ticks.saturating_add(1);
                if history.bad_ticks >= policy.failover_ticks.max(1) {
                    history.bad_ticks = 0;
                    best_egress(candidates, None)
                } else {
                    Some(i)
                }
            }
        }
        None => {
            history.bad_ticks = 0;
            history.challenger = None;
            history.challenger_since = None;
            best_egress(candidates, None)
        }
    };
    // A tunnel alone is still a way out for the router's own traffic; it is not
    // a non-tunnel egress (no gateway to NAT to), but the caller may use it for
    // the main table when nothing else exists.

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn path(name: &str, link_up: bool, answering: bool, bps: f64) -> Candidate {
        let mut c = Candidate::new(IfaceInfo {
            name: name.into(),
            kind: Kind::UsbTether,
            link_up,
            carrier: link_up,
            addresses: vec![(Ipv4Addr::new(10, 0, 0, 1), 24)],
            gateway: Some("10.0.0.254".into()),
            dhcp_metric: Some(2000),
            wireless: false,
            usb: true,
            usb_path: None,
            driver: None,
        });
        c.probe = Probe {
            gateway_ok: answering,
            internet_ok: answering,
            ..Default::default()
        };
        c.throughput_bps = Some(bps);
        c
    }

    fn select(
        candidates: &mut [Candidate],
        history: &mut History,
        previous: &str,
        policy: Policy,
    ) -> Option<String> {
        let sel = update(candidates, history, Some(previous), None, policy);
        sel.best.map(|i| candidates[i].info.name.clone())
    }

    /// A tick where the incumbent loses a probe must not move the network: the
    /// flap that kills every session is a *voluntary* move, not a dead link.
    #[test]
    fn a_transient_probe_failure_keeps_the_incumbent() {
        let policy = Policy::default();
        let mut history = History::default();
        let mut c = vec![path("a", true, false, 500_000.0), path("b", true, true, 300_000.0)];
        assert_eq!(select(&mut c, &mut history, "a", policy).as_deref(), Some("a"));
        assert_eq!(history.bad_ticks, 1);

        // It answers again next tick: no move happened at all.
        c[0].probe.internet_ok = true;
        c[0].probe.gateway_ok = true;
        assert_eq!(select(&mut c, &mut history, "a", policy).as_deref(), Some("a"));
        assert_eq!(history.bad_ticks, 0);
    }

    /// …but a path that stays unreachable does not hold the seat for ever.
    #[test]
    fn an_outage_fails_over_after_the_grace() {
        let policy = Policy::default();
        let mut history = History::default();
        let mut c = vec![path("a", true, false, 500_000.0), path("b", true, true, 300_000.0)];
        assert_eq!(select(&mut c, &mut history, "a", policy).as_deref(), Some("a"));
        assert_eq!(select(&mut c, &mut history, "a", policy).as_deref(), Some("a"));
        assert_eq!(select(&mut c, &mut history, "a", policy).as_deref(), Some("b"));
    }

    /// Losing the carrier is a fact, not a blip, and moves on the first tick.
    #[test]
    fn a_link_that_drops_fails_over_at_once() {
        let policy = Policy::default();
        let mut history = History::default();
        let mut c = vec![path("a", false, false, 500_000.0), path("b", true, true, 300_000.0)];
        assert_eq!(select(&mut c, &mut history, "a", policy).as_deref(), Some("b"));
    }

    /// A healthy incumbent is not swapped for a faster challenger until the
    /// challenger has stayed faster for the whole dwell — this is the direction
    /// that costs a tunnel redial and a NAT rewrite for no outage at all.
    #[test]
    fn a_challenger_waits_out_the_dwell() {
        let policy = Policy {
            dwell: Duration::from_secs(600),
            ..Policy::default()
        };
        let mut history = History::default();
        let mut c = vec![path("a", true, true, 500_000.0), path("b", true, true, 3_000_000.0)];
        assert_eq!(select(&mut c, &mut history, "a", policy).as_deref(), Some("a"));
        assert_eq!(history.challenger.as_deref(), Some("b"));

        // The same call with the dwell already satisfied (0) takes the seat.
        let now = Policy {
            dwell: Duration::ZERO,
            ..policy
        };
        let mut fresh = History::default();
        assert_eq!(select(&mut c, &mut fresh, "a", now).as_deref(), Some("b"));
    }

    /// A challenger inside the margin is not a challenger at all.
    #[test]
    fn a_marginal_difference_is_not_a_reason_to_move() {
        let policy = Policy {
            dwell: Duration::ZERO,
            margin: 1.5,
            ..Policy::default()
        };
        let mut history = History::default();
        // b scores ~`path`'s throughput term only 20 % higher: inside the margin.
        let mut c = vec![path("a", true, true, 500_000.0), path("b", true, true, 600_000.0)];
        assert_eq!(select(&mut c, &mut history, "a", policy).as_deref(), Some("a"));
        assert!(history.challenger.is_none());
    }
}
