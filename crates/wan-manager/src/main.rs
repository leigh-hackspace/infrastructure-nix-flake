//! wan-manager: the router's relentless WAN supervisor.
//!
//! It does not take a WAN interface, address, gateway or SSID as an argument.
//! Every tick it:
//!
//!   1. brings up an association on every wireless interface that advertises
//!      the organisation's backup SSID (all BSSIDs, ranked);
//!   2. discovers every other interface that could be an uplink and asks
//!      networkd for DHCP on it — the gateway is whatever the server offers,
//!      never a number in this repo;
//!   3. probes each path's own gateway and the internet through it, and
//!      periodically measures throughput;
//!   4. ranks the viable paths, with hysteresis in both directions;
//!   5. installs the main default, the hub-mark table, the tunnel table and the
//!      L2TP endpoint route, and fills the firewall's `wan_ifaces` NAT set;
//!   6. resets a selected backup whose own gateway has been silent for 12
//!      ticks, because a route is not a link;
//!   7. writes `/run/wan-manager/status.json` and `metrics.prom`, and pets the
//!      systemd watchdog.
//!
//! A tick that fails does not stop the loop: the failure is recorded and the
//! next tick tries again. That is the *whole* design. There is no one-shot mode
//! and no notion of "the WAN is configured"; only the current measurement.

mod apply;
mod config;
mod dhcp;
mod net;
mod probe;
mod rank;
mod sdnotify;
mod status;
mod util;
mod wifi;

use config::Config;
use net::{IfaceInfo, Kind};
use rank::{Candidate, Selection};
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

struct Manager {
    cfg: Config,
    candidates: Vec<Candidate>,
    wifi_rt: wifi::WifiMap,
    dhcp_rt: dhcp::DhcpRuntime,
    wifi_reports: Vec<wifi::WifiReport>,
    usb: Vec<probe::UsbDevice>,
    usb_failures: Vec<String>,
    usb_checked: Option<Instant>,
    sel: Selection,
    prev_best: Option<String>,
    prev_tunnel: Option<String>,
    ticks: u64,
    started: Instant,
    last_error: Option<String>,
}

impl Manager {
    fn new(cfg: Config) -> Self {
        let _ = std::fs::create_dir_all(&cfg.state_dir);
        let _ = std::fs::create_dir_all(cfg.networkd_dir());
        Manager {
            cfg,
            candidates: Vec::new(),
            wifi_rt: HashMap::new(),
            dhcp_rt: Default::default(),
            wifi_reports: Vec::new(),
            usb: Vec::new(),
            usb_failures: Vec::new(),
            usb_checked: None,
            sel: Selection {
                best: None,
                tunnel: None,
                state: "none".into(),
            },
            prev_best: None,
            prev_tunnel: None,
            ticks: 0,
            started: Instant::now(),
            last_error: None,
        }
    }

    fn merge_candidates(&mut self, infos: Vec<IfaceInfo>) {
        let old = std::mem::take(&mut self.candidates);
        let mut merged = Vec::with_capacity(infos.len());
        for info in infos {
            if let Some(mut c) = old.iter().find(|c| c.info.name == info.name).cloned() {
                c.info = info;
                merged.push(c);
            } else {
                merged.push(Candidate::new(info));
            }
        }
        merged.sort_by(|a, b| a.info.name.cmp(&b.info.name));
        self.candidates = merged;
    }

    fn probe_all(&mut self) {
        let targets = self.cfg.ping_targets.clone();
        let http_url = self.cfg.probe_url.clone();
        for c in self.candidates.iter_mut() {
            let ts = util::now_ts();
            if !c.info.link_up {
                c.probe = probe::Probe {
                    checked_ts: ts,
                    error: Some("link down".into()),
                    ..Default::default()
                };
                continue;
            }
            let bind = c.info.probe_bind();
            let mut p = probe::Probe {
                checked_ts: ts,
                // Deliberately not this tick's: a throughput sample is only due
                // every `throughput_interval`, and the value is kept across a
                // failed probe so a momentary loss does not erase the evidence.
                throughput_ts: c.probe.throughput_ts,
                ..Default::default()
            };
            // The two questions, asked in the order the outage taught: does this
            // path's own next hop answer, and is there internet beyond it. The
            // second one is asked over **two** transports, because they fail
            // independently: a phone's hotspot drops ICMP under load while the
            // TCP path is fine, and judging it by echo alone demoted a working
            // 197 KB/s path to "no internet" on 2026-11-09 while a 3 MB HTTPS
            // download through it completed.
            let gateway_ok = match c.info.kind {
                // A gatewayless path has no next hop to ask; the far end answers
                // the internet question or nothing does.
                _ if c.info.kind.is_gatewayless() => {
                    p.gateway_ok = false;
                    true
                }
                _ => match c.info.gateway.as_deref() {
                    Some(gw) => match probe::gateway_latency(&bind, gw) {
                        Some(ms) => {
                            p.gateway_ok = true;
                            p.latency_ms = Some(ms);
                            true
                        }
                        None => {
                            p.error = Some("gateway silent".into());
                            false
                        }
                    },
                    None => {
                        p.error = Some("no DHCP gateway offered".into());
                        false
                    }
                },
            };
            if gateway_ok {
                if let Some(ms) = probe::internet_latency(&bind, &targets) {
                    p.internet_ok = true;
                    p.internet_via = Some("icmp");
                    p.latency_ms = Some(ms);
                } else if probe::internet_http(&bind, &http_url) {
                    // Echo was filtered or lost; the network does not care which
                    // protocol answered, only that something did. The latency
                    // figure stays the gateway's, so a `tcp` verdict is not also
                    // a claim about round-trip time.
                    p.internet_ok = true;
                    p.internet_via = Some("tcp");
                } else {
                    p.error = Some(
                        if c.info.kind.is_gatewayless() {
                            "no internet through this path".into()
                        } else {
                            "gateway answers, no internet beyond it".into()
                        },
                    );
                }
            }
            if c.info.kind.is_gatewayless() {
                if p.internet_ok {
                    c.failures = 0;
                } else {
                    c.failures = c.failures.saturating_add(1);
                }
            }
            c.probe = p;
        }
    }

    fn measure_throughput(&mut self) {
        let interval = self.cfg.throughput_interval;
        let url = self.cfg.probe_url.clone();
        for c in self.candidates.iter_mut() {
            if !c.eligible() {
                continue;
            }
            let due = c
                .last_measured
                .map(|t| t.elapsed() >= interval)
                .unwrap_or(true);
            if !due {
                continue;
            }
            if let Some(v) = probe::throughput(&c.info.probe_bind(), &url) {
                c.throughput_bps = Some(v);
                c.probe.throughput_bps = Some(v);
                c.probe.throughput_ts = util::now_ts();
            }
            c.last_measured = Some(Instant::now());
        }
    }

    fn kick_selected_fallbacks(&mut self, actions: &mut Vec<String>) {
        let selected = self.sel.best;
        // `tunnel-only` also means the backup is needed: the tunnel may be up
        // *over* the failing Wi-Fi path, and pinning that path was why the
        // daemon never roamed to the second hotspot.
        let needs_backup = self.sel.best.is_none()
            || matches!(self.sel.state.as_str(), "backup" | "none");
        for (i, c) in self.candidates.iter_mut().enumerate() {
            if !matches!(c.info.kind, Kind::UsbTether | Kind::Wifi) {
                continue;
            }
            let needed = selected == Some(i) || needs_backup;
            if needed && !c.probe.gateway_ok {
                c.failures = c.failures.saturating_add(1);
            } else {
                c.failures = 0;
            }
            if c.failures >= self.cfg.kick_ticks {
                actions.push(format!(
                    "resetting {} after {} ticks of a silent gateway",
                    c.info.name, c.failures
                ));
                match c.info.kind {
                    Kind::Wifi => {
                        if let Some(rt) = self.wifi_rt.get_mut(&c.info.name) {
                            rt.last_scan = None;
                            rt.last_start = None;
                            rt.adopted = false;
                            rt.force_restart = true;
                            rt.rotation = rt.rotation.wrapping_add(1);
                        }
                    }
                    _ => {
                        let _ = util::run(
                            "ip",
                            &[
                                "link".to_string(),
                                "set".to_string(),
                                "dev".to_string(),
                                c.info.name.clone(),
                                "down".to_string(),
                            ],
                            5,
                        );
                        std::thread::sleep(Duration::from_millis(500));
                        let _ = util::run(
                            "ip",
                            &[
                                "link".to_string(),
                                "set".to_string(),
                                "dev".to_string(),
                                c.info.name.clone(),
                                "up".to_string(),
                            ],
                            5,
                        );
                        let _ = util::run(
                            "networkctl",
                            &["reconfigure".to_string(), c.info.name.clone()],
                            10,
                        );
                    }
                }
                c.failures = 0;
            }
        }
    }

    fn refresh_usb(&mut self) {
        let due = self
            .usb_checked
            .map(|t| t.elapsed() >= Duration::from_secs(30))
            .unwrap_or(true);
        if !due {
            return;
        }
        self.usb = probe::usb_devices();
        self.usb_failures = probe::usb_kernel_failures();
        self.usb_checked = Some(Instant::now());
    }

    fn tick(&mut self) {
        let mut actions = Vec::new();

        // 1. Association first: a wireless interface that has never associated
        //    has no address for discovery to find.
        let wireless = wifi::wireless_interfaces();
        self.wifi_reports.clear();
        for iface in wireless {
            let rt = self.wifi_rt.entry(iface.clone()).or_default();
            let report = wifi::ensure(&self.cfg, &iface, rt);
            self.wifi_reports.push(report);
        }

        // 2. Discovery and DHCP.
        let infos = net::discover(&self.cfg);
        self.merge_candidates(infos);
        actions.extend(dhcp::ensure(
            &self.cfg,
            &self.candidates.iter().map(|c| c.info.clone()).collect::<Vec<_>>(),
            &mut self.dhcp_rt,
        ));

        // 3. Probes, then the slow throughput sample.
        self.probe_all();
        self.measure_throughput();

        // 4. Rank.
        self.sel = rank::update(
            &mut self.candidates,
            self.prev_best.as_deref(),
            self.prev_tunnel.as_deref(),
            self.cfg.throughput_interval,
        );

        // 5. Repair a selected backup whose own gateway is silent.
        self.kick_selected_fallbacks(&mut actions);

        // 6. Routing and NAT.
        actions.extend(apply::apply_routes(&self.cfg, &self.candidates, &self.sel));
        actions.extend(apply::apply_nat(&self.cfg, &self.candidates, &self.sel));

        // 7. USB diagnostics are expensive (journalctl); refresh them slowly.
        self.refresh_usb();

        // 8. Status, heartbeat and a transition line in the journal.
        let new_best = self.sel.best.map(|i| self.candidates[i].info.name.clone());
        let new_tunnel = self.sel.tunnel.map(|i| self.candidates[i].info.name.clone());
        if new_best != self.prev_best || new_tunnel != self.prev_tunnel || self.ticks == 0 {
            let detail = new_best
                .as_deref()
                .map(|n| format!("egress {n}"))
                .unwrap_or_else(|| "no non-tunnel egress".into());
            let t = new_tunnel
                .as_deref()
                .map(|n| format!(", tunnel {n}"))
                .unwrap_or_else(|| ", tunnel down".into());
            eprintln!("wan-manager: {} ({detail}{t})", self.sel.state);
            let decision = format!(
                "{} {} {}\n",
                self.sel.state,
                new_best.as_deref().unwrap_or("-"),
                new_tunnel.as_deref().unwrap_or("-")
            );
            let _ = std::fs::write(self.cfg.decision(), decision);
        }
        self.prev_best = new_best;
        self.prev_tunnel = new_tunnel;
        self.ticks += 1;

        let all_aps: Vec<probe::Bss> = self
            .wifi_reports
            .iter()
            .flat_map(|w| w.aps.iter().cloned())
            .collect();
        let report = status::Report {
            cfg: &self.cfg,
            candidates: &self.candidates,
            sel: &self.sel,
            wifi: &self.wifi_reports,
            aps: &all_aps,
            usb: &self.usb,
            usb_failures: &self.usb_failures,
            actions: &actions,
            started: self.started,
            ticks: self.ticks,
            last_error: self.last_error.as_deref(),
        };
        status::write(&report);
        let _ = std::fs::write(self.cfg.heartbeat(), format!("{}\n", util::now_ts()));
        if self.ticks == 1 {
            sdnotify::ready();
        }
        sdnotify::status(&format!(
            "{}: {} candidate(s), egress {}",
            self.sel.state,
            self.candidates.len(),
            self.prev_best.as_deref().unwrap_or("none")
        ));
    }
}

fn main() {
    let cfg = Config::parse();
    eprintln!(
        "wan-manager: starting (lan {}, tunnel {}, ssid {:?}, interval {}s)",
        cfg.lan_interface,
        cfg.tunnel_interface,
        cfg.wifi_ssid,
        cfg.interval.as_secs()
    );
    let mut manager = Manager::new(cfg.clone());
    let interval = cfg.interval;
    let mut ever_ready = false;
    loop {
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| manager.tick()));
        match result {
            Ok(()) => {
                manager.last_error = None;
                if !ever_ready {
                    sdnotify::ready();
                    ever_ready = true;
                }
                sdnotify::watchdog();
            }
            Err(e) => {
                let msg = format!("tick panicked: {e:?}");
                eprintln!("wan-manager: {msg}");
                manager.last_error = Some(msg);
                // Deliberately do NOT pet the watchdog after a panic. If every
                // tick panics, systemd kills this process and restarts it with
                // a clean heap — the only recovery a supervisor can offer.
            }
        }
        std::thread::sleep(interval);
    }
}
