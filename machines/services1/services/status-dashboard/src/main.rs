//! Simple systemd status dashboard for the services1 box.
//!
//! Runs on 127.0.0.1 behind nginx (see ../status.nix).  Provides:
//!
//!   GET  /                    HTML dashboard (auto-refreshing)
//!   GET  /api/status          JSON snapshot of watched unit states
//!   POST /api/restart/<unit>  restart a unit (requires X-WEBAUTH-USER header)
//!
//! The restart endpoint is only reachable through the SSO-protected nginx
//! vhost, which injects X-WEBAUTH-USER after a successful auth_request.
//!
//! Deliberately zero external dependencies: it talks to systemd purely by
//! shelling out to `systemctl`, so the flake build needs no crates.io access.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

const NAS_IP: &str = "10.3.1.6";
const NAS_MOUNTS: [&str; 3] = ["/mnt/cameras", "/mnt/filestore", "/mnt/backups"];

/// Units we surface on the dashboard: containers, NAS mounts, boot gates.
const WATCH_PREFIXES: [&str; 3] = ["podman-", "mnt-", "wait-for-"];

const CMD_TIMEOUT: Duration = Duration::from_secs(15);
const PING_TIMEOUT: Duration = Duration::from_secs(5);
const RESTART_TIMEOUT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Process helpers
// ---------------------------------------------------------------------------

/// Run a command, returning `Some((exit_success, stdout))` if it ran, or
/// `None` if it could not be spawned or was killed by the timeout.
fn run_cmd(args: &[&str], timeout: Duration) -> Option<(bool, String)> {
    let mut child = Command::new(args[0])
        .args(&args[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().ok().flatten() {
            let mut out = String::new();
            if let Some(mut stdout) = child.stdout.take() {
                let _ = stdout.read_to_string(&mut out);
            }
            return Some((status.success(), out));
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn systemctl(args: &[&str], timeout: Duration) -> Option<(bool, String)> {
    let mut full = Vec::with_capacity(args.len() + 1);
    full.push("systemctl");
    full.extend_from_slice(args);
    run_cmd(&full, timeout)
}

// ---------------------------------------------------------------------------
// systemd data
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Meta {
    requires: Vec<String>,
    wants: Vec<String>,
    after: Vec<String>,
    description: String,
    since: String,
}

struct UnitRow {
    name: String,
    active: String,
    sub: String,
    description: String,
}

fn split_words(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

fn is_watched(name: &str) -> bool {
    WATCH_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Parse `systemctl list-units --plain` output.  Columns are whitespace
/// separated: UNIT LOAD ACTIVE SUB DESCRIPTION...
fn all_units() -> Vec<UnitRow> {
    let mut out = Vec::new();
    if let Some((_, raw)) = systemctl(
        &[
            "list-units",
            "--all",
            "--type=service,mount,automount,timer",
            "--no-legend",
            "--no-pager",
            "--plain",
        ],
        CMD_TIMEOUT,
    ) {
        for line in raw.lines() {
            let mut it = line.split_whitespace();
            let name = it.next();
            let _load = it.next();
            let active = it.next();
            let sub = it.next();
            if let (Some(name), Some(active), Some(sub)) = (name, active, sub) {
                out.push(UnitRow {
                    name: name.to_string(),
                    active: active.to_string(),
                    sub: sub.to_string(),
                    description: it.collect::<Vec<_>>().join(" "),
                });
            }
        }
    }
    out
}

/// Static-ish info for a unit (dependencies + description), parsed from
/// `systemctl show`.  Cached after the first request; this data never changes.
fn unit_meta(name: &str) -> Meta {
    let mut m = Meta {
        description: name.to_string(),
        ..Default::default()
    };
    if let Some((_, out)) = systemctl(
        &[
            "show",
            "-p",
            "Requires",
            "-p",
            "Wants",
            "-p",
            "After",
            "-p",
            "Description",
            "-p",
            "ActiveEnterTimestamp",
            "--no-pager",
            name,
        ],
        CMD_TIMEOUT,
    ) {
        for line in out.lines() {
            if let Some((key, value)) = line.split_once('=') {
                match key {
                    "Requires" => m.requires = split_words(value),
                    "Wants" => m.wants = split_words(value),
                    "After" => m.after = split_words(value),
                    "Description" => m.description = value.to_string(),
                    "ActiveEnterTimestamp" => m.since = value.to_string(),
                    _ => {}
                }
            }
        }
    }
    m
}

static META_CACHE: OnceLock<HashMap<String, Meta>> = OnceLock::new();

/// Classify a unit as good / bad / waiting / inactive.
fn classify(
    name: &str,
    active: &str,
    sub: &str,
    meta: &Meta,
    states: &HashMap<String, (String, String)>,
) -> (&'static str, String) {
    match active {
        "failed" => return ("bad", "failed".to_string()),
        "active" => return ("good", sub.to_string()),
        _ => {}
    }

    if sub == "auto-restart" {
        return ("waiting", "auto-restarting (will retry)".to_string());
    }
    if matches!(active, "activating" | "deactivating" | "reloading") {
        return ("waiting", active.to_string());
    }

    // Inactive: figure out why.
    if let Some(stripped) = name.strip_suffix(".mount") {
        let automount = format!("{stripped}.automount");
        if states.get(&automount).map(|(a, _)| a == "active").unwrap_or(false) {
            return ("waiting", "not mounted yet (automount idle)".to_string());
        }
    }

    let waiting: Vec<&String> = meta
        .requires
        .iter()
        .filter(|dep| {
            **dep != name
                && is_watched(dep)
                && states
                    .get(*dep)
                    .map(|(a, _)| a != "active")
                    .unwrap_or(false)
        })
        .collect();
    if !waiting.is_empty() {
        return (
            "waiting",
            format!("waiting for {}", waiting.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")),
        );
    }

    ("inactive", sub.to_string())
}

// ---------------------------------------------------------------------------
// Host / NAS status
// ---------------------------------------------------------------------------

fn host_status() -> (String, String, String) {
    let hostname = run_cmd(&["uname", "-n"], CMD_TIMEOUT)
        .map(|(_, o)| o.trim().to_string())
        .unwrap_or_else(|| "?".to_string());
    let uptime = run_cmd(&["uptime", "-p"], CMD_TIMEOUT)
        .map(|(_, o)| o.trim().to_string())
        .unwrap_or_default();
    let load = run_cmd(&["uptime"], CMD_TIMEOUT)
        .and_then(|(_, o)| o.split_once("load average:").map(|(_, l)| l.trim().to_string()))
        .unwrap_or_default();
    (hostname, uptime, load)
}

fn nas_status() -> (bool, Option<f64>, Vec<(String, String)>) {
    let (reachable, rtt) = run_cmd(&["ping", "-c", "1", "-W", "2", NAS_IP], PING_TIMEOUT)
        .map(|(ok, out)| {
            let rtt = out
                .split("time=")
                .nth(1)
                .and_then(|rest| {
                    let num: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
                    num.parse::<f64>().ok()
                });
            (ok, rtt)
        })
        .unwrap_or((false, None));

    let mut mounts = Vec::new();
    for path in NAS_MOUNTS {
        // findmnt prints the autofs entry and (once mounted) the real nfs*
        // entry on separate lines; the last one is the actual filesystem type.
        let fstype = run_cmd(&["findmnt", "-n", "-o", "FSTYPE", path], CMD_TIMEOUT)
            .map(|(_, o)| {
                o.lines()
                    .filter(|l| !l.trim().is_empty())
                    .last()
                    .map(|l| l.trim().to_string())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        mounts.push((path.to_string(), fstype));
    }
    (reachable, rtt, mounts)
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

fn json_str(s: &str) -> String {
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

fn json_error(msg: &str) -> String {
    format!(r#"{{"error":{}}}"#, json_str(msg))
}

fn status_payload() -> String {
    let units = all_units();

    let mut states: HashMap<String, (String, String)> = HashMap::new();
    for u in &units {
        states.insert(u.name.clone(), (u.active.clone(), u.sub.clone()));
    }

    let metas = META_CACHE.get_or_init(|| {
        let mut m = HashMap::new();
        for u in &units {
            if is_watched(&u.name) {
                m.insert(u.name.clone(), unit_meta(&u.name));
            }
        }
        m
    });

    let mut counts = (0usize, 0usize, 0usize, 0usize); // good, bad, waiting, inactive

    let mut group_json = |title: &str, filter: fn(&str) -> bool| -> String {
        let mut rows = Vec::new();
        let mut names: Vec<&UnitRow> = units.iter().filter(|u| filter(&u.name)).collect();
        names.sort_by(|a, b| a.name.cmp(&b.name));
        for u in names {
            let meta = metas.get(&u.name).cloned().unwrap_or_default();
            let (status, detail) = classify(&u.name, &u.active, &u.sub, &meta, &states);
            match status {
                "good" => counts.0 += 1,
                "bad" => counts.1 += 1,
                "waiting" => counts.2 += 1,
                _ => counts.3 += 1,
            }
            let restartable = u.name.ends_with(".service") && status != "good";
            rows.push(format!(
                r#"{{"unit":{},"description":{},"active":{},"sub":{},"status":{},"detail":{},"since":{},"restartable":{}}}"#,
                json_str(&u.name),
                json_str(if meta.description.is_empty() { &u.description } else { &meta.description }),
                json_str(&u.active),
                json_str(&u.sub),
                json_str(status),
                json_str(&detail),
                json_str(&meta.since),
                if restartable { "true" } else { "false" },
            ));
        }
        format!(r#"{{"title":{},"units":[{}]}}"#, json_str(title), rows.join(","))
    };

    let groups = [
        group_json("Containers", |n| n.starts_with("podman-")),
        group_json("NAS mounts", |n| n.starts_with("mnt-")),
        group_json("Boot gates", |n| n.starts_with("wait-for-")),
    ]
    .join(",");

    // Every failed unit on the box, watched or not.
    let mut failed = Vec::new();
    if let Some((_, raw)) = systemctl(
        &["list-units", "--state=failed", "--no-legend", "--no-pager", "--plain"],
        CMD_TIMEOUT,
    ) {
        for line in raw.lines() {
            let mut it = line.split_whitespace();
            if let Some(name) = it.next() {
                let desc = it.collect::<Vec<_>>().join(" ");
                failed.push(format!(
                    r#"{{"unit":{},"description":{}}}"#,
                    json_str(name),
                    json_str(&desc)
                ));
            }
        }
    }

    let (hostname, uptime, load) = host_status();
    let (nas_ok, rtt, mounts) = nas_status();
    let now = run_cmd(&["date", "+%Y-%m-%d %H:%M:%S"], CMD_TIMEOUT)
        .map(|(_, o)| o.trim().to_string())
        .unwrap_or_default();

    let mount_json: Vec<String> = mounts
        .iter()
        .map(|(p, f)| format!(r#"{{"path":{},"fstype":{}}}"#, json_str(p), json_str(f)))
        .collect();
    let rtt_json = match rtt {
        Some(v) => format!("{v}"),
        None => "null".to_string(),
    };

    format!(
        r#"{{"host":{{"hostname":{},"uptime":{},"load":{}}},"nas":{{"ip":{},"reachable":{},"rtt_ms":{},"mounts":[{}]}},"summary":{{"good":{},"bad":{},"waiting":{},"inactive":{}}},"groups":[{}],"failed":[{}],"now":{}}}"#,
        json_str(&hostname),
        json_str(&uptime),
        json_str(&load),
        json_str(NAS_IP),
        if nas_ok { "true" } else { "false" },
        rtt_json,
        mount_json.join(","),
        counts.0,
        counts.1,
        counts.2,
        counts.3,
        groups,
        failed.join(","),
        json_str(&now),
    )
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

fn is_safe_unit(name: &str) -> bool {
    let chars_ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@._:+-".contains(c));
    chars_ok
        && (name.ends_with(".service")
            || name.ends_with(".mount")
            || name.ends_with(".automount")
            || name.ends_with(".timer"))
}

fn percent_decode(s: &str) -> String {
    fn hex_val(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn route(method: &str, path: &str, headers: &HashMap<String, String>) -> (u16, String, String) {
    match (method, path) {
        ("GET", "/") | ("GET", "/index.html") => (200, "text/html; charset=utf-8".to_string(), PAGE.to_string()),
        ("GET", "/api/status") => (200, "application/json".to_string(), status_payload()),
        ("POST", _) if path.starts_with("/api/restart/") => {
            let unit = percent_decode(&path["/api/restart/".len()..]);
            if !is_safe_unit(&unit) {
                return (400, "application/json".to_string(), json_error("bad unit name"));
            }
            let user = headers
                .get("x-webauth-user")
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            if user.is_empty() {
                return (
                    403,
                    "application/json".to_string(),
                    json_error("not authenticated (SSO required)"),
                );
            }
            let _ = systemctl(&["reset-failed", &unit], CMD_TIMEOUT);
            let ok = systemctl(&["restart", &unit], RESTART_TIMEOUT)
                .map(|(ok, _)| ok)
                .unwrap_or(false);
            let body = if ok {
                format!(
                    r#"{{"ok":true,"unit":{},"user":{},"message":"restarted by {user}"}}"#,
                    json_str(&unit),
                    json_str(&user)
                )
            } else {
                json_error("restart failed")
            };
            (if ok { 200 } else { 500 }, "application/json".to_string(), body)
        }
        _ => (404, "application/json".to_string(), json_error("not found")),
    }
}

fn handle_client(mut stream: TcpStream) {
    // Read the request head up to the blank line.
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        match stream.read(&mut tmp) {
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => return,
        }
        if buf.len() > 64 * 1024 {
            return; // unreasonably large head
        }
    }

    let head = String::from_utf8_lossy(&buf);
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    let mut headers: HashMap<String, String> = HashMap::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let (status, ctype, body) = route(method, path, &headers);
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

// ---------------------------------------------------------------------------
// Frontend
// ---------------------------------------------------------------------------

const PAGE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>services1 status</title>
<style>
  :root {
    --bg: #101418; --panel: #1a2129; --border: #2a333d;
    --text: #d7e0e8; --muted: #8b98a5;
    --good: #2ecc71; --bad: #e74c3c; --wait: #f1c40f; --inactive: #7f8c8d;
  }
  * { box-sizing: border-box; }
  body { margin: 0; background: var(--bg); color: var(--text);
         font: 14px/1.5 ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif; }
  main { max-width: 1100px; margin: 0 auto; padding: 20px; }
  header { display: flex; align-items: baseline; gap: 12px; flex-wrap: wrap; margin-bottom: 16px; }
  h1 { font-size: 20px; margin: 0; }
  h2 { font-size: 13px; text-transform: uppercase; letter-spacing: .05em; color: var(--muted); margin: 0 0 8px; }
  .sub, .muted { color: var(--muted); }
  .cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 12px; margin-bottom: 20px; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: 8px; padding: 12px; }
  .pill { display: inline-block; padding: 2px 10px; border-radius: 999px; font-size: 12px; font-weight: 600; }
  .pill.good { background: rgba(46,204,113,.15); color: var(--good); }
  .pill.bad { background: rgba(231,76,60,.15); color: var(--bad); }
  .pill.waiting { background: rgba(241,196,15,.15); color: var(--wait); }
  .pill.inactive { background: rgba(127,140,141,.15); color: var(--inactive); }
  .badge.good { color: var(--good); } .badge.bad { color: var(--bad); }
  .badge.waiting { color: var(--wait); }
  table { width: 100%; border-collapse: collapse; background: var(--panel); border: 1px solid var(--border); border-radius: 8px; overflow: hidden; }
  th, td { text-align: left; padding: 8px 10px; border-bottom: 1px solid var(--border); vertical-align: top; }
  th { color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: .05em; }
  tr:last-child td { border-bottom: none; }
  code { background: rgba(255,255,255,.06); padding: 1px 6px; border-radius: 4px; word-break: break-all; }
  button { background: #2b6cb0; color: #fff; border: 0; border-radius: 6px; padding: 4px 10px; font-size: 12px; cursor: pointer; }
  button:hover { background: #2c5282; }
  button:disabled { opacity: .4; cursor: default; }
  .detail { color: var(--wait); font-size: 12px; }
  .failed-row { padding: 4px 0; }
  .failed-row button { margin-left: 8px; }
  ul { margin: 8px 0 0; padding-left: 18px; }
  #toast { position: fixed; bottom: 16px; right: 16px; background: var(--panel); border: 1px solid var(--border); border-radius: 8px; padding: 10px 14px; display: none; }
</style>
</head>
<body>
<main>
  <header>
    <h1>&#x1F5A5;&#xFE0F; <span id="hostname">&hellip;</span></h1>
    <span class="sub" id="meta"></span>
    <span class="pill good" id="counts"></span>
  </header>

  <section class="cards">
    <div class="card">
      <h2>NAS 10.3.1.6</h2>
      <div id="nas"></div>
    </div>
    <div class="card">
      <h2>Summary</h2>
      <div id="summary"></div>
    </div>
    <div class="card">
      <h2>Failed units</h2>
      <div id="failed"></div>
    </div>
  </section>

  <div id="groups"></div>
</main>
<div id="toast"></div>
<script>
function esc(s) {
  const d = document.createElement('div');
  d.textContent = s == null ? '' : String(s);
  return d.innerHTML;
}

async function getJSON(url, opts) {
  const r = await fetch(url, opts);
  const body = await r.json().catch(() => ({}));
  return { r, body };
}

function pill(status) {
  return '<span class="pill ' + status + '">' + status + '</span>';
}

function render(data) {
  document.getElementById('hostname').textContent = data.host.hostname;
  document.getElementById('meta').textContent =
    data.host.uptime + ' &middot; load ' + data.host.load + ' &middot; ' + data.now;

  const s = data.summary;
  const countsEl = document.getElementById('counts');
  countsEl.textContent = s.good + ' good &middot; ' + s.bad + ' bad &middot; ' + s.waiting + ' waiting';
  countsEl.className = 'pill ' + (s.bad ? 'bad' : s.waiting ? 'waiting' : 'good');

  // NAS card
  const nas = data.nas;
  let nasHtml = '<div>' + (nas.reachable ? '&#x1F7E2; up' : '&#x1F534; down')
    + (nas.rtt_ms != null ? ' (' + nas.rtt_ms + ' ms)' : '') + '</div><ul>';
  for (const m of nas.mounts) {
    nasHtml += '<li><code>' + esc(m.path) + '</code> &mdash; '
      + (m.fstype ? esc(m.fstype) : '<span class="muted">not mounted</span>') + '</li>';
  }
  nasHtml += '</ul>';
  document.getElementById('nas').innerHTML = nasHtml;

  // Summary card
  document.getElementById('summary').innerHTML =
    '<span class="badge good">' + s.good + ' good</span> &middot; '
    + '<span class="badge bad">' + s.bad + ' bad</span> &middot; '
    + '<span class="badge waiting">' + s.waiting + ' waiting</span>';

  // Failed units card
  const failedEl = document.getElementById('failed');
  if (!data.failed.length) {
    failedEl.innerHTML = '<span class="muted">none</span>';
  } else {
    failedEl.innerHTML = '';
    for (const u of data.failed) {
      const row = document.createElement('div');
      row.className = 'failed-row';
      row.innerHTML = '<code>' + esc(u.unit) + '</code>'
        + '<span class="muted"> ' + esc(u.description) + '</span>'
        + '<button onclick="restart(\'' + esc(u.unit) + '\')">restart</button>';
      failedEl.appendChild(row);
    }
  }

  // Watched groups
  const groupsEl = document.getElementById('groups');
  groupsEl.innerHTML = '';
  for (const group of data.groups) {
    let tbl = '<section style="margin-top:20px"><h2>' + esc(group.title) + '</h2><table>'
      + '<thead><tr><th>Unit</th><th>State</th><th>Detail</th><th>Since</th><th></th></tr></thead><tbody>';
    for (const u of group.units) {
      const btn = u.restartable
        ? '<button onclick="restart(\'' + esc(u.unit) + '\')">restart</button>' : '';
      tbl += '<tr><td><code>' + esc(u.unit) + '</code><br><span class="muted">'
        + esc(u.description) + '</span></td>'
        + '<td>' + pill(u.status) + '</td>'
        + '<td class="detail">' + (u.status !== 'good' && u.detail ? esc(u.detail) : '') + '</td>'
        + '<td class="muted">' + esc(u.since) + '</td>'
        + '<td>' + btn + '</td></tr>';
    }
    tbl += '</tbody></table></section>';
    groupsEl.insertAdjacentHTML('beforeend', tbl);
  }
}

async function refresh() {
  const { r, body } = await getJSON('/api/status');
  if (r.ok) render(body);
}

async function restart(unit) {
  const btn = event.target;
  btn.disabled = true;
  const { r, body } = await getJSON('/api/restart/' + encodeURIComponent(unit), { method: 'POST' });
  toast(body.message || body.error || (r.ok ? 'restarted' : 'failed'), r.ok ? 'good' : 'bad');
  setTimeout(refresh, 2000);
}

function toast(msg, kind) {
  const t = document.getElementById('toast');
  t.textContent = msg;
  t.style.display = 'block';
  t.style.borderColor = kind === 'good' ? 'var(--good)' : 'var(--bad)';
  clearTimeout(toast._t);
  toast._t = setTimeout(function () { t.style.display = 'none'; }, 4000);
}

refresh();
setInterval(refresh, 10000);
</script>
</body>
</html>
"##;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut bind = "127.0.0.1".to_string();
    let mut port: u16 = 8088;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--bind" => {
                if let Some(v) = args.get(i + 1) {
                    bind = v.clone();
                    i += 1;
                }
            }
            "--port" => {
                if let Some(v) = args.get(i + 1) {
                    port = v.parse().unwrap_or(port);
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }

    let listener = match TcpListener::bind((bind.as_str(), port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("status-dashboard: failed to bind {bind}:{port}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("status-dashboard listening on {bind}:{port}");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                thread::spawn(move || handle_client(stream));
            }
            Err(_) => continue,
        }
    }
}
