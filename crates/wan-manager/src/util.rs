//! Small process, time and text helpers.
//!
//! Everything here is deliberately boring. The daemon's whole contract is "keep
//! running and keep looking", so a failed command is a *result*, not an error
//! that unwinds: `run` returns `None` and the caller records why. Nothing in this
//! module panics on a missing binary, a dead interface or a command that hangs.

use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// Run a command, bounded by `timeout` seconds via coreutils' `timeout(1)`.
///
/// `timeout` is used rather than a thread + `try_wait` because `iw scan`,
/// `networkctl reconfigure` and `nft` can all block on a wedged kernel, and a
/// daemon that hangs doing a probe is indistinguishable from one that has nothing
/// to do. `timeout` is in the daemon's PATH by construction (coreutils is the
/// first package in the unit's `path`).
///
/// `--kill-after` is not decoration: `timeout` alone sends one TERM and then
/// **waits for the child to exit**, so a child that ignores it (or whose signal
/// handling is stuck behind a blocking syscall) holds this daemon forever. That
/// is a hang in `probe`, not a slow probe — the distinction the watchdog is
/// there to catch — and this is what stops it from being one.
pub fn run(prog: &str, args: &[String], timeout_secs: u64) -> Option<(i32, String, String)> {
    let mut cmd = Command::new("timeout");
    cmd.arg("--signal=TERM")
        .arg("--kill-after=5s")
        .arg(format!("{timeout_secs}s"))
        .arg(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = cmd.output().ok()?;
    let code = out.status.code().unwrap_or(-1);
    Some((
        code,
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// Run a command and return only stdout when it exited 0.
pub fn run_ok(prog: &str, args: &[String], timeout_secs: u64) -> Option<String> {
    let (code, stdout, _) = run(prog, args, timeout_secs)?;
    if code == 0 {
        Some(stdout)
    } else {
        None
    }
}

/// A single trimmed line from a file, or `None` if it is absent/unreadable.
/// Sysfs is the daemon's primary source and a device disappearing between a
/// directory listing and a read is normal, not exceptional.
pub fn read_trim(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

pub fn read_trim_path(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

pub fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn now_mono() -> std::time::Instant {
    std::time::Instant::now()
}

/// Write a file only when its content would change. Returns true when it wrote.
///
/// The daemon runs every few seconds and writes several `/run` files; rewriting
/// an unchanged file would churn networkd's reload path and make every tick look
/// like a configuration change in the journal.
pub fn write_if_changed(path: &std::path::Path, content: &str, mode: u32) -> bool {
    if let Ok(old) = std::fs::read_to_string(path) {
        if old == content {
            return false;
        }
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::write(path, content).is_err() {
        return false;
    }
    let _ = std::fs::set_permissions(
        path,
        std::os::unix::fs::PermissionsExt::from_mode(mode),
    );
    true
}

/// JSON string quoting. The status file is the only JSON this crate writes and
/// its strings are interface names, error text and BSSIDs; hand-rolling the
/// escape is the same choice `crates/network-status` makes (zero deps).
pub fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn jopt<T: ToString>(v: Option<T>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "null".into())
}

pub fn jopt_f(v: Option<f64>) -> String {
    match v {
        Some(x) if x.is_finite() => format!("{x:.3}"),
        _ => "null".into(),
    }
}

/// Parse the `metric N` field of an `ip route` line.
pub fn route_metric(line: &str) -> Option<u32> {
    line.split_whitespace()
        .position(|w| w == "metric")
        .and_then(|i| line.split_whitespace().nth(i + 1))
        .and_then(|s| s.parse().ok())
}

/// Parse the `via A.B.C.D` field of an `ip route` line.
pub fn route_via(line: &str) -> Option<String> {
    line.split_whitespace()
        .position(|w| w == "via")
        .and_then(|i| line.split_whitespace().nth(i + 1))
        .map(|s| s.to_string())
}

/// Parse the `dev NAME` field of an `ip route` line.
pub fn route_dev(line: &str) -> Option<String> {
    line.split_whitespace()
        .position(|w| w == "dev")
        .and_then(|i| line.split_whitespace().nth(i + 1))
        .map(|s| s.to_string())
}
