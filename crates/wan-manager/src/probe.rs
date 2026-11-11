//! Viability probes and the two discovery sources that are not a netdev's
//! address: the 802.11 scan and the USB tree.
//!
//! The probe policy is copied from the 2026-11-07 outage, where "the link is up"
//! and "the DHCP lease exists" were both true while the radio lost beacons:
//!
//!   * a path is viable only when its **own gateway** answers, not merely when
//!     the interface has an address;
//!   * the internet is a second, independent question, asked of more than one
//!     anchor so one blocked ICMP destination is not an outage;
//!   * throughput is measured over HTTP through the interface, not inferred
//!     from the negotiated rate, because the 2026-10-31 camera measurement
//!     showed two identical links differing 22×.

use crate::util::{now_ts, run, run_ok};
use std::process::Command;
use std::time::Instant;

#[derive(Debug, Clone, Default)]
pub struct Probe {
    pub gateway_ok: bool,
    pub internet_ok: bool,
    /// **How** the internet verdict was reached. `icmp` is three echoes through
    /// this path; `tcp` is an HTTP GET, used when echo is filtered or lost under
    /// load. The distinction is not decoration: on 2026-11-09 the Uplink tab read
    /// `170.1 KB/s` beside `no internet through this path` for the Wi-Fi path —
    /// because the throughput *sample* had completed over TCP while the *verdict*
    /// came only from ICMP, which this phone's hotspot drops when the cell is
    /// busy. The daemon then kept the network on a 10 Mbit/s USB Ethernet dongle
    /// at 16 KB/s, which is the mis-selection this field exists to make visible.
    pub internet_via: Option<&'static str>,
    pub latency_ms: Option<f64>,
    pub throughput_bps: Option<f64>,
    /// When `throughput_bps` was measured. Kept across a failed probe on purpose
    /// (a momentary loss must not erase the ranking evidence), so it needs its own
    /// timestamp or the table shows an old number as if it were this tick's.
    pub throughput_ts: i64,
    pub checked_ts: i64,
    pub error: Option<String>,
}

pub fn ping(iface: &str, target: &str) -> Option<f64> {
    let args = vec![
        "-c".to_string(),
        // Three echoes, not one: a single lost packet on a 2.4 GHz cell is
        // normal and was demoting a working path for a tick.
        "3".to_string(),
        "-i".to_string(),
        "0.2".to_string(),
        "-W".to_string(),
        // Two seconds, not one: this network's backup paths are a phone's cellular
        // uplink behind a hotspot, where 300-470 ms round trips are normal and a
        // 1 s wait reported a working path as silent whenever the cell queued.
        "2".to_string(),
        "-n".to_string(),
        "-q".to_string(),
        "-I".to_string(),
        iface.to_string(),
        target.to_string(),
    ];
    let t0 = Instant::now();
    let (code, out, _) = run("ping", &args, 4)?;
    if code != 0 {
        return None;
    }
    // `rtt min/avg/max/mdev = 1.234/1.234/1.234/0.000 ms`
    let avg = out
        .lines()
        .find(|l| l.contains("min/avg/max"))
        .and_then(|l| l.split('=').nth(1))
        .and_then(|s| s.trim().split('/').nth(1))
        .and_then(|s| s.trim().parse::<f64>().ok());
    Some(avg.unwrap_or_else(|| t0.elapsed().as_secs_f64() * 1000.0))
}

pub fn gateway_latency(iface: &str, gateway: &str) -> Option<f64> {
    // `ping -I <iface> <gw>` uses SO_BINDTODEVICE, which is what makes this a
    // test of *this* path: without it a route change elsewhere could answer.
    ping(iface, gateway)
}

pub fn internet_latency(iface: &str, targets: &[String]) -> Option<f64> {
    targets.iter().find_map(|t| ping(iface, t))
}

/// One transfer-phase sample: bytes delivered divided by the time spent
/// delivering them, i.e. total time minus time-to-first-byte.
fn throughput_once(iface: &str, url: &str) -> Option<f64> {
    let args = vec![
        "--interface".to_string(),
        iface.to_string(),
        "--max-time".to_string(),
        "5".to_string(),
        "--connect-timeout".to_string(),
        "5".to_string(),
        "--silent".to_string(),
        "--show-error".to_string(),
        "--output".to_string(),
        "/dev/null".to_string(),
        "--write-out".to_string(),
        "%{size_download} %{time_total} %{time_starttransfer}".to_string(),
        url.to_string(),
    ];
    // 28 is `--max-time`: the write-out is still emitted, and a partial transfer
    // measured over the time it actually took is a valid (low-bound) rate.
    let (code, out, _) = run("curl", &args, 10)?;
    if code != 0 && code != 28 {
        return None;
    }
    let mut it = out.split_whitespace();
    let size: f64 = it.next()?.parse().ok()?;
    let total: f64 = it.next()?.parse().ok()?;
    let ttfb: f64 = it.next()?.parse().ok()?;
    // Pure divide-by-zero guard, and nothing more. A *meaningful* floor is how
    // this went wrong once already: with a 56 KB page and a 20 ms floor every
    // measurement on a fast path clamped to `56614 / 0.02`, i.e. the constant
    // 2 830 700 B/s for every path at once — a number that looked plausible and
    // measured nothing. That is why the probe URL is now a megabyte.
    let transfer = (total - ttfb).max(0.001);
    (size > 0.0).then_some(size / transfer)
}

/// A small HTTP GET through one interface, as **bytes per second**, measured over
/// the transfer phase only. `curl --interface` is the only portable way (with no
/// netlink crate) to say "this request must leave by this device".
///
/// `%{speed_download}` is deliberately *not* used. It is bytes over the whole
/// request, so on a small page it measures the server answering, not the link:
/// measured on `gw` 2026-11-11, a 56 KB page with 96-145 ms time-to-first-byte
/// reported 476 418 B/s while a 10 MB transfer over the same interface did
/// 4 141 702 B/s — 8.7x wrong, and it moved with the server's latency.
///
/// Subtracting the time-to-first-byte removes that bias, but the payload then
/// has to be big enough for the transfer phase to be measurable at all, which is
/// why `--probe-url` is a megabyte and not a homepage. Two samples, max: sharing
/// or congestion can only make a sample slower, so the larger one is the better
/// estimate of what the path can do.
pub fn throughput(iface: &str, url: &str) -> Option<f64> {
    let a = throughput_once(iface, url);
    let b = throughput_once(iface, url);
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

pub fn internet_http(iface: &str, url: &str) -> bool {
    let args = vec![
        "--interface".to_string(),
        iface.to_string(),
        "--max-time".to_string(),
        "10".to_string(),
        "--silent".to_string(),
        "--output".to_string(),
        "/dev/null".to_string(),
        "--write-out".to_string(),
        "%{http_code}".to_string(),
        url.to_string(),
    ];
    run("curl", &args, 15)
        .map(|(code, out, _)| {
            code == 0
                && out
                    .trim()
                    .parse::<u32>()
                    .map(|n| (200..500).contains(&n))
                    .unwrap_or(false)
        })
        .unwrap_or(false)
}

#[derive(Debug, Clone)]
pub struct Bss {
    pub bssid: String,
    pub ssid: String,
    pub signal_dbm: Option<i32>,
    pub freq_mhz: Option<u32>,
}

pub fn wifi_scan(iface: &str) -> Vec<Bss> {
    let args = vec![
        "dev".to_string(),
        iface.to_string(),
        "scan".to_string(),
    ];
    let Some(out) = run_ok("iw", &args, 8) else {
        return Vec::new();
    };
    let mut list = Vec::new();
    let mut cur: Option<Bss> = None;
    for line in out.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("BSS ") {
            if let Some(b) = cur.take() {
                if !b.ssid.is_empty() {
                    list.push(b);
                }
            }
            let bssid = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches("(on")
                .trim_end_matches(')')
                .to_string();
            cur = Some(Bss {
                bssid,
                ssid: String::new(),
                signal_dbm: None,
                freq_mhz: None,
            });
        } else if let Some(c) = cur.as_mut() {
            if let Some(v) = t.strip_prefix("SSID:") {
                c.ssid = v.trim().to_string();
            } else if let Some(v) = t.strip_prefix("signal:") {
                c.signal_dbm = v
                    .trim()
                    .trim_end_matches(" dBm")
                    .parse::<f64>()
                    .ok()
                    .map(|f| f as i32);
            } else if let Some(v) = t.strip_prefix("freq:") {
                c.freq_mhz = v.trim().parse::<f64>().ok().map(|f| f as u32);
            }
        }
    }
    if let Some(b) = cur {
        if !b.ssid.is_empty() {
            list.push(b);
        }
    }
    list
}

/// The current association, as `iw` reports it. `bssid` is what the ranking
/// records; the signal is a sanity check on the scan's estimate.
pub fn wifi_link(iface: &str) -> Option<(String, Option<i32>)> {
    let args = vec!["dev".to_string(), iface.to_string(), "link".to_string()];
    let out = run_ok("iw", &args, 5)?;
    if out.contains("Not connected") {
        return None;
    }
    let bssid = out
        .lines()
        .find_map(|l| l.trim().strip_prefix("Connected to "))
        .map(|s| s.split_whitespace().next().unwrap_or("").to_string())?;
    let signal = out
        .lines()
        .find_map(|l| l.trim().strip_prefix("signal:"))
        .and_then(|s| s.trim().trim_end_matches(" dBm").parse::<f64>().ok())
        .map(|f| f as i32);
    Some((bssid, signal))
}

#[derive(Debug, Clone)]
pub struct UsbDevice {
    pub path: String,
    pub vendor: String,
    pub product: String,
    pub product_name: String,
    pub driver: Option<String>,
    pub netdev: Option<String>,
}

/// Every enumerated USB device. A phone that is *not* here never reached the
/// point where a driver could bind; that is the distinction the diagnosis below
/// is built on.
pub fn usb_devices() -> Vec<UsbDevice> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/sys/bus/usb/devices") else {
        return out;
    };
    for e in rd.flatten() {
        let dir = e.path();
        let path = e.file_name().to_string_lossy().into_owned();
        if path.contains(':') || path.starts_with("usb") {
            continue;
        }
        let rd_file = |n: &str| {
            std::fs::read_to_string(dir.join(n))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let Some(vendor) = rd_file("idVendor") else {
            continue;
        };
        let product = rd_file("idProduct").unwrap_or_default();
        let product_name = rd_file("product").unwrap_or_default();
        let driver = std::fs::read_link(dir.join("driver"))
            .ok()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()));
        // A USB netdev's device path is `<usb-path>:<config>.<iface>`.
        let netdev = crate::net::list_interfaces().into_iter().find(|n| {
            crate::net::usb_path(n)
                .map(|u| path.starts_with(&u) || u.starts_with(&path))
                .unwrap_or(false)
        });
        out.push(UsbDevice {
            path,
            vendor,
            product,
            product_name,
            driver,
            netdev,
        });
    }
    out
}

/// The kernel's recent view of USB enumeration failures.
///
/// Read with `journalctl -k` because the failure this reports (`error -71`,
/// `unable to enumerate USB device`) is emitted by `usbcore` and nothing in
/// sysfs records it. The line count is bounded; this is a diagnosis, not a log
/// mirror.
pub fn usb_kernel_failures() -> Vec<String> {
    let args = vec![
        "-k".to_string(),
        "--since".to_string(),
        "-15 min".to_string(),
        "--no-pager".to_string(),
        "-n".to_string(),
        "400".to_string(),
    ];
    let Some((_, out, _)) = run("journalctl", &args, 5) else {
        return Vec::new();
    };
    out.lines()
        .filter(|l| {
            l.contains("error -71")
                || l.contains("unable to enumerate USB device")
                || l.contains("device descriptor read")
                || l.contains("device not accepting address")
        })
        .map(|s| s.trim().to_string())
        .collect()
}

/// Android-class vendors. This is a *hint* for the diagnosis, not a filter: a
/// phone whose vendor is not in this list still becomes a candidate as soon as
/// it presents an Ethernet netdev, because the USB bus and the netdev type are
/// facts while the vendor list is a convenience.
pub fn looks_like_phone(vendor: &str, product: &str, name: &str) -> bool {
    const VENDORS: &[&str] = &[
        "18d1", // Google
        "04e8", // Samsung
        "2717", "2716", // Xiaomi
        "12d1", // Huawei
        "0fce", // Sony
        "22b8", // Motorola
        "2a70", "2a96", // OnePlus
        "0b05", // Asus
        "1004", // LG
        "0e8d", // MediaTek
        "05c6", // Qualcomm
        "1d6b", // Linux gadget (this phone in RNDIS/ECM mode)
    ];
    let n = name.to_ascii_lowercase();
    VENDORS.contains(&vendor.to_ascii_lowercase().as_str())
        || n.contains("android")
        || n.contains("phone")
        || n.contains("tether")
        || n.contains("rndis")
        || product.eq_ignore_ascii_case("0000")
}

pub fn now() -> i64 {
    now_ts()
}

/// A tiny helper for the emergency path: is this box online at all, through
/// any route? Used by the manager's own "is the current selection still true"
/// check, so a probe and the break-glass script cannot disagree about what
/// "online" means.
pub fn any_internet(url: &str) -> bool {
    let args = vec![
        "--max-time".to_string(),
        "8".to_string(),
        "--silent".to_string(),
        "--output".to_string(),
        "/dev/null".to_string(),
        "--write-out".to_string(),
        "%{http_code}".to_string(),
        url.to_string(),
    ];
    Command::new("curl")
        .args(&args)
        .output()
        .map(|o| {
            o.status.success()
                && String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse::<u32>()
                    .map(|n| (200..500).contains(&n))
                    .unwrap_or(false)
        })
        .unwrap_or(false)
}
