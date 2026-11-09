//! The Wi-Fi backup: association is a session, so it cannot be declared.
//!
//! Every wireless interface is a candidate, not just the one this box happened
//! to have on 2026-11-07. For each we scan for the organisation's SSID, keep
//! every BSSID advertising it, and rank them by signal before association. A
//! single radio can only hold one BSS, so "try all of them" means: pin the best
//! one, measure it, and rotate to the next on repeated failure — the ranking is
//! a record of what was measured, not a claim that a 1×1 radio is several.
//!
//! The passphrase is read from `/run/secrets` at use time and turned into the
//! PMK wpa_supplicant keys with by `wpa_passphrase` on **stdin**. It never
//! appears in a store path, an argv, or the journal.

use crate::config::Config;
use crate::probe::{self, Bss};
use crate::util::{read_trim, run, write_if_changed};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default)]
pub struct WifiRuntime {
    pub last_scan: Option<Instant>,
    pub last_start: Option<Instant>,
    pub chosen_bssid: Option<String>,
    pub rotation: usize,
    pub last_error: Option<String>,
    /// Set by a kick. A restart without a pinned BSSID makes wpa_supplicant
    /// scan and roam to whichever AP it can hear best; pinning one BSSID is
    /// why the old daemon stayed on a failing hotspot while another was up.
    pub force_restart: bool,
    /// True when this daemon found an association it did not start (the
    /// break-glass supplicant, or a previous process). Adopting it is what
    /// makes a handback seamless: the daemon manages routes and monitors the
    /// link without killing the WPA session that is carrying traffic.
    pub adopted: bool,
}

#[derive(Debug, Clone)]
pub struct WifiReport {
    pub iface: String,
    pub associated_bssid: Option<String>,
    pub signal_dbm: Option<i32>,
    pub aps: Vec<Bss>,
    pub chosen_bssid: Option<String>,
    pub error: Option<String>,
}

fn psk(cfg: &Config) -> Option<String> {
    let s = std::fs::read_to_string(&cfg.psk_file).ok()?;
    let s = s.trim().to_string();
    if s.len() >= 8 {
        Some(s)
    } else {
        None
    }
}

fn pmk(ssid: &str, pass: &str) -> Option<String> {
    let mut child = Command::new("wpa_passphrase")
        .arg(ssid)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    {
        use std::io::Write;
        let mut stdin = child.stdin.take()?;
        stdin.write_all(pass.as_bytes()).ok()?;
        stdin.write_all(b"\n").ok()?;
    }
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let pmk = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("psk="))
        .map(|s| s.trim().to_string())?;
    if pmk.len() == 64 && pmk.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(pmk)
    } else {
        None
    }
}

fn safe(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect()
}

fn conf_path(cfg: &Config, iface: &str) -> PathBuf {
    cfg.state_dir.join(format!("wpa-{}.conf", safe(iface)))
}

fn ctrl_dir(cfg: &Config, iface: &str) -> PathBuf {
    cfg.state_dir.join(format!("wpa-{}", safe(iface)))
}

fn associated(iface: &str) -> Option<(String, Option<i32>)> {
    probe::wifi_link(iface)
}

/// Start (or restart) the supplicant for one interface, pinned to `bssid` when
/// there is one. `-B` is safe here because this daemon is the supervisor: it
/// checks the association every tick and restarts a dead process itself, which
/// is the one thing systemd cannot do from outside a `wpa_supplicant` session.
fn start(cfg: &Config, iface: &str, pass: &str, _bssid: Option<&str>, rt: &mut WifiRuntime) -> Option<()> {
    let pmk = pmk(&cfg.wifi_ssid, pass)?;
    let ctrl = ctrl_dir(cfg, iface);
    let _ = std::fs::create_dir_all(&ctrl);
    let mut conf = format!(
        "country=GB\nctrl_interface=DIR={} GROUP=root\nupdate_config=0\nnetwork={{\n  ssid=\"{}\"\n  psk={}\n  key_mgmt=WPA-PSK\n  scan_ssid=1\n",
        ctrl.display(),
        cfg.wifi_ssid.replace('"', ""),
        pmk
    );
    // Deliberately no `bssid=`: one radio, one SSID, and wpa_supplicant must be
    // free to roam to the other BSSes carrying it. The scan above still ranks
    // them for the dashboard; the association is not pinned to the winner.
    conf.push_str("}\n");
    let path = conf_path(cfg, iface);
    let changed = write_if_changed(&path, &conf, 0o600);
    // A stale supplicant holding the interface makes association impossible.
    // This is the same takeover the old `wan-backup-wifi` preStart did, and it
    // is also how a hand-started supplicant from break-glass is cleared.
    let _ = run(
        "pkill",
        &["-f".to_string(), format!("wpa_supplicant.*{iface}")],
        3,
    );
    // A config that has not changed and a process that survived pkill? The
    // pkill above is unconditional, so this always starts. The `changed` flag is
    // kept for the status/log line only.
    let _ = changed;
    let (_, out, err) = run(
        "wpa_supplicant",
        &[
            "-B".to_string(),
            "-i".to_string(),
            iface.to_string(),
            "-c".to_string(),
            path.display().to_string(),
            "-D".to_string(),
            "nl80211,wext".to_string(),
            "-C".to_string(),
            ctrl.display().to_string(),
        ],
        20,
    )?;
    if !err.trim().is_empty() {
        rt.last_error = Some(err.trim().to_string());
    } else if !out.trim().is_empty() {
        rt.last_error = Some(out.trim().to_string());
    }
    rt.last_start = Some(Instant::now());
    Some(())
}

/// Scan, rank and (re)associate one wireless interface. Called every tick;
/// scanning itself is rate-limited to `wifi_scan_interval`.
pub fn ensure(cfg: &Config, iface: &str, rt: &mut WifiRuntime) -> WifiReport {
    let mut report = WifiReport {
        iface: iface.to_string(),
        associated_bssid: None,
        signal_dbm: None,
        aps: Vec::new(),
        chosen_bssid: rt.chosen_bssid.clone(),
        error: None,
    };
    let Some(pass) = psk(cfg) else {
        report.error = Some(format!("no usable passphrase at {}", cfg.psk_file.display()));
        return report;
    };

    if rt.force_restart {
        rt.force_restart = false;
        rt.adopted = false;
        let _ = start(cfg, iface, &pass, None, rt);
    }

    let assoc_now = associated(iface);
    if assoc_now.is_some() && rt.last_start.is_none() {
        rt.adopted = true;
    }

    if rt
        .last_scan
        .map(|t| t.elapsed() >= cfg.wifi_scan_interval)
        .unwrap_or(true)
    {
        let all = probe::wifi_scan(iface);
        rt.last_scan = Some(Instant::now());
        let mut aps: Vec<Bss> = all
            .into_iter()
            .filter(|b| b.ssid == cfg.wifi_ssid)
            .collect();
        aps.sort_by_key(|b| std::cmp::Reverse(b.signal_dbm.unwrap_or(-127)));
        report.aps = aps.clone();
        if !aps.is_empty() {
            let idx = rt.rotation % aps.len();
            let chosen = aps[idx].bssid.clone();
            let changed = rt.chosen_bssid.as_deref() != Some(chosen.as_str());
            rt.chosen_bssid = Some(chosen);
            // An association that already exists is adopted, not killed: the
            // handback runs while the emergency supplicant is carrying the
            // network. Only start one when there is no association.
            if assoc_now.is_none() && (changed || rt.last_start.is_none()) {
                if let Some(b) = rt.chosen_bssid.clone() {
                    let _ = start(cfg, iface, &pass, Some(&b), rt);
                }
            }
        } else if assoc_now.is_none() {
            // No BSS of that name in the scan. Still run a supplicant with no
            // pinned BSSID: `scan_ssid=1` will associate the moment the AP
            // appears, instead of waiting for the next 5-minute scan. A radio
            // with no association also has no candidate for the ranker, so
            // this is the path that lets a hotspot switched on after boot be
            // found within seconds rather than minutes.
            let _ = start(cfg, iface, &pass, None, rt);
        }
    }

    if let Some((bssid, signal)) = associated(iface) {
        report.associated_bssid = Some(bssid);
        report.signal_dbm = signal;
        rt.last_error = None;
    } else {
        rt.adopted = false;
        // No association. Give a fresh supplicant a full minute to associate:
        // the 2.4 GHz cell is crowded and a 15 s retry was killing WPA
        // handshakes mid-flight, which is what made the handback flap.
        let retry = rt
            .last_start
            .map(|t| t.elapsed() >= Duration::from_secs(60))
            .unwrap_or(true);
        if retry {
            // Try the next ranked BSS on repeated failure.
            if !report.aps.is_empty() {
                rt.rotation = rt.rotation.wrapping_add(1);
                let idx = rt.rotation % report.aps.len();
                rt.chosen_bssid = Some(report.aps[idx].bssid.clone());
            }
            let b = rt.chosen_bssid.clone();
            let _ = start(cfg, iface, &pass, b.as_deref(), rt);
        }
        report.error = rt.last_error.clone();
    }
    report.chosen_bssid = rt.chosen_bssid.clone();
    report
}

/// A light per-interface map keyed by iface, kept by `main`.
pub type WifiMap = HashMap<String, WifiRuntime>;

pub fn wireless_interfaces() -> Vec<String> {
    crate::net::list_interfaces()
        .into_iter()
        .filter(|n| crate::net::is_wireless(n))
        .collect()
}

/// `wpa_cli` is used only for the human-readable diagnosis; the daemon's own
/// decisions come from `iw link` and the route table.
pub fn status_line(cfg: &Config, iface: &str) -> Option<String> {
    let ctrl = ctrl_dir(cfg, iface);
    let args = vec![
        "-p".to_string(),
        ctrl.display().to_string(),
        "-i".to_string(),
        iface.to_string(),
        "status".to_string(),
    ];
    run("wpa_cli", &args, 5).and_then(|(code, out, _)| if code == 0 { Some(out) } else { None })
}

pub fn read_pmk_hint(cfg: &Config, iface: &str) -> Option<String> {
    let path = conf_path(cfg, iface);
    // Never return the PMK; report only that the config exists and its mode.
    let meta = std::fs::metadata(&path).ok()?;
    let mode = std::os::unix::fs::PermissionsExt::mode(&meta.permissions());
    Some(format!("{} (mode {:o})", path.display(), mode))
}

pub fn current_ssid(iface: &str) -> Option<String> {
    let out = run("iw", &["dev".to_string(), iface.to_string(), "link".to_string()], 5)?;
    if out.0 != 0 {
        return None;
    }
    out.1
        .lines()
        .find_map(|l| l.trim().strip_prefix("SSID:"))
        .map(|s| s.trim().to_string())
}
