//! dns-sync — keep the LAN router's dnsmasq and DigitalOcean DNS in step
//! with the nginx reverse proxy on services1.
//!
//! The set of names that *should* exist is generated from the nginx config
//! at build time (see machines/services1/dns-sync.nix) and written to
//! `/etc/dns-sync/expected-int-names` — one `*.int.leighhack.org` FQDN per
//! line (vhost names plus their serverAliases). This tool compares that
//! list against two DNS sources:
//!
//!   * the router's dnsmasq (`/var/etc/dnsmasq-hosts` on 10.3.1.1, an
//!     OPNsense box, ssh as root with the machine-hop key), and
//!   * the `leighhack.org` zone on DigitalOcean — the public DNS that DoH
//!     users see (token in `/var/lib/secrets/.env`).
//!
//! Run it on services1 (it needs the DO token and ssh access to the
//! router):
//!
//!   sudo dns-sync check            # report whether every vhost is in DNS
//!   sudo dns-sync sync             # add missing records (router + DO)
//!
//! The tool is strictly additive: it only ever *adds* missing names (router
//! host-override aliases / DO CNAMEs) and never deletes or rewrites existing
//! records. Lots of `*.int.leighhack.org` names legitimately point at other
//! hosts (cameras, Pis, APs, switches...), so anything not in the expected
//! list is out of scope and is never touched.
//!
//! Zero external crates (house style): HTTP goes through `curl`, the
//! router edit goes through `ssh` + python3 (present on OPNsense), and
//! JSON is parsed with the tiny hand-rolled parser in `json.rs`.

mod json;

use std::collections::BTreeMap;
use std::env;
use std::io::Write;
use std::process::{Command, Stdio};

const DEFAULT_EXPECTED: &str = "/etc/dns-sync/expected-int-names";
const DEFAULT_SSH_KEY: &str = "/home/leigh-admin/.ssh/agent-hop-key";
const DEFAULT_ROUTER: &str = "root@10.3.1.1";
const DEFAULT_ENV_FILE: &str = "/var/lib/secrets/.env";
/// Names DNS was last brought in line with (written by `prune`). Used to
/// tell rename leftovers (were expected, no longer are — prune them) apart
/// from records that predate dns-sync and were never expected (left as-is).
const DEFAULT_LAST_EXPECTED: &str = "/var/lib/dns-sync/last-expected";

const ZONE: &str = "leighhack.org";
/// IPv4 of services1 — what every *.int.leighhack.org record must answer.
const SERVICES1_IP: &str = "10.3.1.20";
/// Target of the DO CNAMEs for *.int names (nginx.int.leighhack.org).
/// The DO API requires the trailing dot on the data field.
const DO_CNAME_TARGET: &str = "nginx.int.leighhack.org.";
const DO_TTL: u64 = 60;

const DO_RECORDS_API: &str = "https://api.digitalocean.com/v2/domains/leighhack.org/records";

/// python3 script run on the router: append names to the `services1` host
/// override's `<aliases>` in /conf/config.xml (surgical, backed up,
/// validated), leaving everything else byte-for-byte intact.
const PY_ADD_ALIASES: &str = r#"import re, sys, shutil, time, xml.etree.ElementTree as ET

missing = [a for a in sys.argv[1:] if a]
if not missing:
    print("no names to add")
    sys.exit(0)

CONF = "/conf/config.xml"
s = open(CONF, "r").read()
# The element carries attributes (<dnsmasq version=...>), so match the prefix.
start = s.index("<dnsmasq")
end = s.index("</dnsmasq>", start)
section = s[start:end]
m = re.search(r"(<host>services1</host>.*?<aliases>)([^<]*)(</aliases>)", section, re.S)
if not m:
    print("ERROR: services1 host override not found under <dnsmasq>", file=sys.stderr)
    sys.exit(1)
cur = [x for x in m.group(2).split(",") if x]
want = sorted(set(cur) | set(missing))
if want == sorted(cur):
    print("aliases already contain all requested names; nothing to do")
    sys.exit(0)
new_xml = s[: start + m.start()] + m.group(1) + ",".join(want) + m.group(3) + s[start + m.end() :]
try:
    ET.fromstring(new_xml)
except ET.ParseError as e:
    print("ERROR: edited config.xml failed XML validation: %s" % e, file=sys.stderr)
    sys.exit(1)
bak = CONF + ".dns-sync-" + time.strftime("%Y%m%d-%H%M%S")
shutil.copy2(CONF, bak)
open(CONF, "w").write(new_xml)
print("backup: " + bak)
print("added: " + ", ".join(sorted(set(missing) - set(cur))))
"#;

/// python3 script run on the router: print the services1 host override's
/// current aliases (FQDNs), one per line.
const PY_GET_ALIASES: &str = r#"import re, sys

CONF = "/conf/config.xml"
s = open(CONF, "r").read()
start = s.index("<dnsmasq")
end = s.index("</dnsmasq>", start)
section = s[start:end]
m = re.search(r"(<host>services1</host>.*?<aliases>)([^<]*)(</aliases>)", section, re.S)
if not m:
    print("ERROR: services1 host override not found under <dnsmasq>", file=sys.stderr)
    sys.exit(1)
print("\n".join(x.strip() for x in m.group(2).split(",") if x.strip()))
"#;

/// python3 script run on the router: remove the given FQDNs from the
/// services1 host override's `<aliases>` (backed up, XML-validated), leaving
/// everything else byte-for-byte intact.
const PY_REMOVE_ALIASES: &str = r#"import re, sys, shutil, time, xml.etree.ElementTree as ET

remove = [a for a in sys.argv[1:] if a]
if not remove:
    print("no names to remove")
    sys.exit(0)

CONF = "/conf/config.xml"
s = open(CONF, "r").read()
start = s.index("<dnsmasq")
end = s.index("</dnsmasq>", start)
section = s[start:end]
m = re.search(r"(<host>services1</host>.*?<aliases>)([^<]*)(</aliases>)", section, re.S)
if not m:
    print("ERROR: services1 host override not found under <dnsmasq>", file=sys.stderr)
    sys.exit(1)
cur = [x.strip() for x in m.group(2).split(",") if x.strip()]
removed = [x for x in cur if x in remove]
if not removed:
    print("aliases do not contain any requested names; nothing to do")
    sys.exit(0)
want = [x for x in cur if x not in remove]
new_xml = s[: start + m.start()] + m.group(1) + ",".join(want) + m.group(3) + s[start + m.end() :]
try:
    ET.fromstring(new_xml)
except ET.ParseError as e:
    print("ERROR: edited config.xml failed XML validation: %s" % e, file=sys.stderr)
    sys.exit(1)
bak = CONF + ".dns-sync-remove-" + time.strftime("%Y%m%d-%H%M%S")
shutil.copy2(CONF, bak)
open(CONF, "w").write(new_xml)
print("backup: " + bak)
print("removed: " + ", ".join(sorted(removed)))
"#;

// ---------------------------------------------------------------------------
// Process helpers
// ---------------------------------------------------------------------------

fn run_checked(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("failed to run {}: {}", cmd, e))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.success() {
        Ok(stdout)
    } else {
        let detail = if stderr.trim().is_empty() { stdout } else { stderr };
        Err(format!(
            "`{} {}` failed ({}){}",
            cmd,
            args.join(" "),
            out.status,
            if detail.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", detail.trim())
            }
        ))
    }
}

/// Run a command feeding `input` to its stdin; returns (success, stdout, stderr).
fn run_input(cmd: &str, args: &[&str], input: &str) -> Result<(bool, String, String), String> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn {}: {}", cmd, e))?;
    child
        .stdin
        .as_mut()
        .expect("stdin piped")
        .write_all(input.as_bytes())
        .map_err(|e| format!("failed to write stdin to {}: {}", cmd, e))?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("failed to wait for {}: {}", cmd, e))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    ))
}

fn ssh(ssh_key: &str, router: &str, remote_cmd: &str) -> Result<String, String> {
    run_checked(
        "ssh",
        &[
            "-i",
            ssh_key,
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            // TOFU on first connect (same policy as the documented dev flow).
            "-o",
            "StrictHostKeyChecking=accept-new",
            router,
            remote_cmd,
        ],
    )
}

// ---------------------------------------------------------------------------
// Expected names
// ---------------------------------------------------------------------------

fn read_expected(path: &str) -> Result<Vec<String>, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read expected names from {}: {}", path, e))?;
    let mut names: Vec<String> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        return Err(format!("no names in {}", path));
    }
    Ok(names)
}

/// Names DNS was last brought in line with. A missing file means no history
/// yet (baseline) — nothing is considered stale until `prune` runs once.
fn read_last_expected(path: &str) -> Result<Vec<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let mut names: Vec<String> = raw
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(str::to_string)
                .collect();
            names.sort();
            names.dedup();
            Ok(names)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("cannot read {}: {}", path, e)),
    }
}

fn write_last_expected(path: &str, names: &[String]) -> Result<(), String> {
    let dir = std::path::Path::new(path)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("/"));
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
    std::fs::write(path, names.join("\n") + "\n")
        .map_err(|e| format!("cannot write {}: {}", path, e))
}

// ---------------------------------------------------------------------------
// Router (OPNsense dnsmasq)
// ---------------------------------------------------------------------------

/// name -> IPv4 it resolves to (first hit in /var/etc/dnsmasq-hosts).
fn router_hosts(ssh_key: &str, router: &str) -> Result<BTreeMap<String, String>, String> {
    let out = ssh(ssh_key, router, "cat /var/etc/dnsmasq-hosts")?;
    let mut map = BTreeMap::new();
    for line in out.lines() {
        let mut it = line.split_whitespace();
        let ip = it.next().unwrap_or("").trim();
        if ip.is_empty() || ip.starts_with('#') {
            continue;
        }
        for name in it {
            map.entry(name.trim().to_string()).or_insert_with(|| ip.to_string());
        }
    }
    Ok(map)
}

/// Base ssh args to reach the router (same policy as the documented dev flow).
fn router_ssh_args(ssh_key: &str, router: &str) -> Vec<String> {
    vec![
        "-i".into(),
        ssh_key.into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ConnectTimeout=10".into(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        router.into(),
    ]
}

/// Run `python3 - <script>` on the router; `script_args` arrive as argv.
fn router_python(
    ssh_key: &str,
    router: &str,
    script: &str,
    script_args: &[String],
) -> Result<(bool, String, String), String> {
    let mut args = router_ssh_args(ssh_key, router);
    args.push("python3".into());
    // Read the script from stdin; the requested names arrive as argv.
    args.push("-".into());
    args.extend(script_args.iter().cloned());
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    run_input("ssh", &refs, script)
}

fn router_restart_dnsmasq(ssh_key: &str, router: &str) -> Result<(), String> {
    let restart = ssh(ssh_key, router, "configctl dnsmasq restart")
        .map_err(|e| format!("dnsmasq restart failed: {}", e))?;
    if !restart.trim().is_empty() {
        for line in restart.lines() {
            println!("    {}", line);
        }
    }
    Ok(())
}

/// Append names to the services1 host override's aliases and restart dnsmasq.
fn router_add_aliases(ssh_key: &str, router: &str, missing: &[String]) -> Result<(), String> {
    println!("  editing /conf/config.xml (backup taken) and restarting dnsmasq...");
    let (ok, stdout, stderr) = router_python(ssh_key, router, PY_ADD_ALIASES, missing)?;
    if !stdout.trim().is_empty() {
        for line in stdout.lines() {
            println!("    {}", line);
        }
    }
    if !stderr.trim().is_empty() {
        eprintln!("  (router stderr) {}", stderr.trim());
    }
    if !ok {
        return Err("router edit failed (see above)".into());
    }
    router_restart_dnsmasq(ssh_key, router)
}

/// Current services1 host-override aliases (FQDNs) from /conf/config.xml —
/// the names dns-sync manages on the router.
fn router_managed_aliases(ssh_key: &str, router: &str) -> Result<Vec<String>, String> {
    let (ok, stdout, stderr) = router_python(ssh_key, router, PY_GET_ALIASES, &[])?;
    if !ok {
        return Err(format!("router alias read failed: {}", stderr.trim()));
    }
    let mut names: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// Remove names from the services1 host override's aliases and restart dnsmasq.
fn router_remove_aliases(ssh_key: &str, router: &str, stale: &[String]) -> Result<(), String> {
    println!("  editing /conf/config.xml (backup taken) and restarting dnsmasq...");
    let (ok, stdout, stderr) = router_python(ssh_key, router, PY_REMOVE_ALIASES, stale)?;
    if !stdout.trim().is_empty() {
        for line in stdout.lines() {
            println!("    {}", line);
        }
    }
    if !stderr.trim().is_empty() {
        eprintln!("  (router stderr) {}", stderr.trim());
    }
    if !ok {
        return Err("router edit failed (see above)".into());
    }
    router_restart_dnsmasq(ssh_key, router)
}

// ---------------------------------------------------------------------------
// DigitalOcean
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct DoRecord {
    id: u64,
    typ: String,
    name: String,
    data: String,
}

/// Read DO_AUTH_TOKEN from the env file (directly, or via passwordless sudo
/// when the tool runs unprivileged). Never prints the token.
fn do_token(env_file: &str) -> Result<String, String> {
    let raw = match std::fs::read_to_string(env_file) {
        Ok(s) => s,
        Err(_) => run_checked("sudo", &["-n", "cat", env_file])?,
    };
    for line in raw.lines() {
        if let Some(v) = line.strip_prefix("DO_AUTH_TOKEN=") {
            let t = v.trim().trim_matches('"').trim_matches('\'').to_string();
            if t.is_empty() {
                return Err("DO_AUTH_TOKEN is empty in env file".into());
            }
            return Ok(t);
        }
    }
    Err(format!("DO_AUTH_TOKEN not found in {}", env_file))
}

/// One curl round-trip to the DO API. Returns (http_code, response body).
fn do_api(token: &str, method: &str, url: &str, body: Option<&str>) -> Result<(u64, String), String> {
    let tmp = env::temp_dir().join(format!("dns-sync-{}-{}.json", std::process::id(), method));
    let tmp_str = tmp.to_string_lossy().to_string();
    let mut args: Vec<String> = vec![
        "-sS".into(),
        "--max-time".into(),
        "30".into(),
        "-o".into(),
        tmp_str.clone(),
        "-w".into(),
        "%{http_code}".into(),
        "-H".into(),
        format!("Authorization: Bearer {}", token),
    ];
    if method != "GET" {
        args.push("-X".into());
        args.push(method.into());
    }
    if let Some(b) = body {
        args.push("-H".into());
        args.push("Content-Type: application/json".into());
        args.push("-d".into());
        args.push(b.into());
    }
    args.push(url.into());
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let code_str = run_checked("curl", &refs)?;
    let body = std::fs::read_to_string(&tmp).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp);
    let code: u64 = code_str.trim().parse().unwrap_or(0);
    Ok((code, body))
}

fn do_list_records(token: &str) -> Result<Vec<DoRecord>, String> {
    let mut records = Vec::new();
    let mut page: u64 = 1;
    loop {
        let url = format!("{}?per_page=200&page={}", DO_RECORDS_API, page);
        let (code, resp) = do_api(token, "GET", &url, None)?;
        if code != 200 {
            return Err(format!("DO list failed with HTTP {}: {}", code, truncate(&resp, 300)));
        }
        let j = json::parse(&resp).map_err(|e| format!("bad JSON from DO: {}", e))?;
        let recs = j
            .get("domain_records")
            .and_then(|a| a.as_array())
            .ok_or_else(|| "DO response has no domain_records".to_string())?;
        for r in recs {
            records.push(DoRecord {
                id: r.get("id").and_then(|v| v.as_u64()).unwrap_or(0),
                typ: r.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                name: r.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                data: r.get("data").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            });
        }
        let total = j
            .get("meta")
            .and_then(|m| m.get("total"))
            .and_then(|t| t.as_u64())
            .unwrap_or(records.len() as u64);
        if page * 200 >= total {
            break;
        }
        page += 1;
    }
    Ok(records)
}

fn do_create_cname(token: &str, label: &str) -> Result<u64, String> {
    let body = format!(
        r#"{{"type":"CNAME","name":"{}","data":"{}","ttl":{}}}"#,
        label, DO_CNAME_TARGET, DO_TTL
    );
    let (code, resp) = do_api(token, "POST", DO_RECORDS_API, Some(&body))?;
    if code != 201 {
        return Err(format!(
            "DO create {} failed with HTTP {}: {}",
            label,
            code,
            truncate(&resp, 300)
        ));
    }
    let j = json::parse(&resp).map_err(|e| format!("bad JSON from DO create: {}", e))?;
    j.get("domain_record")
        .and_then(|r| r.get("id"))
        .and_then(|v| v.as_u64())
        .ok_or_else(|| format!("no domain_record.id in DO create response for {}", label))
}

/// Delete a DO record by id (HTTP 204 on success).
fn do_delete_record(token: &str, id: u64) -> Result<(), String> {
    let url = format!("{}/{}", DO_RECORDS_API, id);
    let (code, resp) = do_api(token, "DELETE", &url, None)?;
    if code != 204 {
        return Err(format!(
            "DO delete id {} failed with HTTP {}: {}",
            id,
            code,
            truncate(&resp, 300)
        ));
    }
    Ok(())
}

/// Normalise a DO name (relative label or FQDN) to the bare label:
/// `firewall.int.leighhack.org` and `firewall.int` both -> `firewall.int`.
fn do_label(name: &str) -> String {
    name.trim_end_matches('.')
        .strip_suffix(ZONE)
        .map(|p| p.trim_end_matches('.').to_string())
        .unwrap_or_else(|| name.trim_end_matches('.').to_string())
}

/// Compare a CNAME target to `nginx.int` ignoring the zone suffix / trailing dot.
fn cname_target_eq(data: &str, target: &str) -> bool {
    let norm = |s: &str| s.trim_end_matches('.').trim_end_matches(".leighhack.org").to_string();
    norm(data) == norm(target)
}

fn truncate(s: &str, n: usize) -> String {
    let s = s.trim();
    let chars: String = s.chars().take(n).collect();
    if chars.len() == s.chars().count() {
        s.to_string()
    } else {
        format!("{}...", chars)
    }
}

// ---------------------------------------------------------------------------
// Diffing
// ---------------------------------------------------------------------------

fn router_status(
    expected: &[String],
    hosts: &BTreeMap<String, String>,
) -> Vec<(String, String, String)> {
    expected
        .iter()
        .map(|name| match hosts.get(name) {
            Some(ip) if ip == SERVICES1_IP => ("OK".into(), ip.clone(), name.clone()),
            Some(ip) => (
                "ELSEWHERE".into(),
                format!("{} (not {})", ip, SERVICES1_IP),
                name.clone(),
            ),
            None => ("MISSING".into(), String::new(), name.clone()),
        })
        .collect()
}

fn do_status(expected: &[String], records: &[DoRecord]) -> Vec<(String, String, String)> {
    expected
        .iter()
        .map(|fqdn| {
            let label = do_label(fqdn);
            match records.iter().find(|r| do_label(&r.name) == label) {
                Some(r) if r.typ == "CNAME" && cname_target_eq(&r.data, DO_CNAME_TARGET) => (
                    "OK".into(),
                    format!("{} -> {}", r.name, r.data),
                    fqdn.clone(),
                ),
                Some(r) => (
                    "OTHER".into(),
                    format!("{} {} -> {}", r.typ, r.name, r.data),
                    fqdn.clone(),
                ),
                None => ("MISSING".into(), String::new(), fqdn.clone()),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Managed-name triage: split the records dns-sync manages into
//   stale  — were expected previously, no longer are (rename leftovers)
//   legacy — managed but never expected (predate dns-sync; left as-is)
// ---------------------------------------------------------------------------

/// Router: services1 override aliases (FQDNs).
fn router_managed_split(
    last: &[String],
    expected: &[String],
    managed: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut stale = Vec::new();
    let mut legacy = Vec::new();
    for n in managed {
        if !expected.contains(n) {
            if last.contains(n) {
                stale.push(n.clone());
            } else {
                legacy.push(n.clone());
            }
        }
    }
    stale.sort();
    legacy.sort();
    (stale, legacy)
}

/// DO: CNAMEs whose data -> nginx.int.
fn do_managed_split(
    last: &[String],
    expected: &[String],
    records: &[DoRecord],
) -> (Vec<DoRecord>, Vec<DoRecord>) {
    let expected_labels: Vec<String> = expected.iter().map(|n| do_label(n)).collect();
    let last_labels: Vec<String> = last.iter().map(|n| do_label(n)).collect();
    let (mut stale, mut legacy) = (Vec::new(), Vec::new());
    for r in records {
        if r.typ == "CNAME" && cname_target_eq(&r.data, DO_CNAME_TARGET) {
            let label = do_label(&r.name);
            if !expected_labels.contains(&label) {
                if last_labels.contains(&label) {
                    stale.push(r.clone());
                } else {
                    legacy.push(r.clone());
                }
            }
        }
    }
    stale.sort_by(|a, b| a.name.cmp(&b.name));
    legacy.sort_by(|a, b| a.name.cmp(&b.name));
    (stale, legacy)
}

// ---------------------------------------------------------------------------
// check / sync / prune
// ---------------------------------------------------------------------------

struct Opts {
    expected: String,
    last_expected: String,
    ssh_key: String,
    router: String,
    env_file: String,
    router_only: bool,
    do_only: bool,
}

fn usage() -> ! {
    eprintln!(
        "usage: dns-sync <check|sync|prune> [--expected FILE] [--last-expected FILE] \
         [--ssh-key PATH] [--router HOST] [--env-file PATH] [--router-only|--do-only]\n\n  check  report expected vs present DNS (exit 1 if missing or stale)\n  sync   add missing records (strictly additive)\n  prune  remove rename leftovers — records dns-sync manages that were\n         expected previously but are no longer (never touches records that\n         predate dns-sync)"
    );
    std::process::exit(2);
}

fn parse_args() -> (Opts, String) {
    let mut opts = Opts {
        expected: DEFAULT_EXPECTED.into(),
        last_expected: DEFAULT_LAST_EXPECTED.into(),
        ssh_key: DEFAULT_SSH_KEY.into(),
        router: DEFAULT_ROUTER.into(),
        env_file: DEFAULT_ENV_FILE.into(),
        router_only: false,
        do_only: false,
    };
    let mut subcmd: Option<String> = None;
    let mut args = env::args().skip(1).peekable();
    while let Some(a) = args.next() {
        match a.as_str() {
            "check" | "sync" | "prune" if subcmd.is_none() => subcmd = Some(a),
            "--expected" | "--last-expected" | "--ssh-key" | "--router" | "--env-file" => {
                let v = args.next().unwrap_or_else(|| usage());
                match a.as_str() {
                    "--expected" => opts.expected = v,
                    "--last-expected" => opts.last_expected = v,
                    "--ssh-key" => opts.ssh_key = v,
                    "--router" => opts.router = v,
                    _ => opts.env_file = v,
                }
            }
            "--router-only" => opts.router_only = true,
            "--do-only" => opts.do_only = true,
            _ => usage(),
        }
    }
    let subcmd = subcmd.unwrap_or_else(|| usage());
    if opts.router_only && opts.do_only {
        eprintln!("--router-only and --do-only are mutually exclusive");
        usage();
    }
    (opts, subcmd)
}

fn print_table(title: &str, rows: &[(String, String, String)]) -> bool {
    println!("\n-- {} --", title);
    let mut drift = false;
    for (status, detail, name) in rows {
        let flag = match status.as_str() {
            "OK" => "  OK  ",
            "MISSING" => "MISS  ",
            _ => "WARN  ",
        };
        if status != "OK" {
            drift = true;
        }
        if detail.is_empty() {
            println!("{} {}", flag, name);
        } else {
            println!("{} {:<42} {}", flag, name, detail);
        }
    }
    drift
}

fn run_check(opts: &Opts) -> i32 {
    let expected = match read_expected(&opts.expected) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };
    let last = match read_last_expected(&opts.last_expected) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };
    println!("expected: {} names (from {})", expected.len(), opts.expected);
    let mut missing = 0usize;
    let mut stale: Vec<String> = Vec::new();
    let mut legacy: Vec<String> = Vec::new();

    if !opts.do_only {
        let hosts = match router_hosts(&opts.ssh_key, &opts.router) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("router: {}", e);
                return 1;
            }
        };
        print_table(
            &format!("router dnsmasq @ {} (/var/etc/dnsmasq-hosts)", opts.router),
            &router_status(&expected, &hosts),
        );
        missing += router_status(&expected, &hosts)
            .iter()
            .filter(|(s, _, _)| s == "MISSING")
            .count();
        let managed = match router_managed_aliases(&opts.ssh_key, &opts.router) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("router: {}", e);
                return 1;
            }
        };
        let (s, l) = router_managed_split(&last, &expected, &managed);
        stale.extend(s);
        legacy.extend(l);
    }

    if !opts.router_only {
        let token = match do_token(&opts.env_file) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("do: {}", e);
                return 1;
            }
        };
        let records = match do_list_records(&token) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("do: {}", e);
                return 1;
            }
        };
        print_table(
            "DigitalOcean (leighhack.org zone)",
            &do_status(&expected, &records),
        );
        missing += do_status(&expected, &records)
            .iter()
            .filter(|(s, _, _)| s == "MISSING")
            .count();
        let (s, l) = do_managed_split(&last, &expected, &records);
        stale.extend(s.iter().map(|r| r.name.clone()));
        legacy.extend(l.iter().map(|r| r.name.clone()));
    }

    stale.sort();
    stale.dedup();
    legacy.sort();
    legacy.dedup();
    if !stale.is_empty() {
        println!("\n-- stale (were expected previously, no longer are) --");
        for n in &stale {
            println!("  {}", n);
        }
    }
    if !legacy.is_empty() {
        println!(
            "\n-- note: managed by dns-sync but never expected (predates dns-sync, left as-is) --"
        );
        for n in &legacy {
            println!("  {}", n);
        }
    }

    if missing > 0 {
        println!(
            "\n{} record(s) missing — run `sudo dns-sync sync` to apply",
            missing
        );
        1
    } else if !stale.is_empty() {
        println!(
            "\n{} stale record(s) — run `sudo dns-sync prune` to remove",
            stale.len()
        );
        1
    } else {
        println!("\nno missing or stale records (notes above, if any, are informational)");
        0
    }
}

fn run_sync(opts: &Opts) -> i32 {
    let expected = match read_expected(&opts.expected) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };
    println!("== dns-sync sync: {} expected names ==", expected.len());

    // --- router ---
    if !opts.do_only {
        let hosts = match router_hosts(&opts.ssh_key, &opts.router) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("router: {}", e);
                return 1;
            }
        };
        let status = router_status(&expected, &hosts);
        let missing: Vec<String> = status
            .iter()
            .filter(|(s, _, _)| s == "MISSING")
            .map(|(_, _, name)| name.clone())
            .collect();
        let elsewhere: Vec<String> = status
            .iter()
            .filter(|(s, _, _)| s == "ELSEWHERE")
            .map(|(_, d, n)| format!("{} ({})", n, d))
            .collect();
        if !missing.is_empty() {
            println!("\nrouter: adding {} names -> services1 host override aliases", missing.len());
            for n in &missing {
                println!("  + {}", n);
            }
            if let Err(e) = router_add_aliases(&opts.ssh_key, &opts.router, &missing) {
                eprintln!("router: {}", e);
                return 1;
            }
            // verify
            match router_hosts(&opts.ssh_key, &opts.router) {
                Ok(after) => {
                    let still: Vec<&String> = missing.iter().filter(|n| !after.contains_key(*n)).collect();
                    if still.is_empty() {
                        println!("  verified: all {} names now answer in /var/etc/dnsmasq-hosts", missing.len());
                    } else {
                        eprintln!("  NOT verified, still missing: {:?}", still);
                        return 1;
                    }
                }
                Err(e) => {
                    eprintln!("router re-read failed: {}", e);
                    return 1;
                }
            }
        } else {
            println!("\nrouter: up to date ({} names)", hosts.len());
        }
        if !elsewhere.is_empty() {
            println!("  note: these expected names resolve elsewhere on the router (left as-is):");
            for n in &elsewhere {
                println!("    {}", n);
            }
        }
    }

    // --- DigitalOcean ---
    if !opts.router_only {
        let token = match do_token(&opts.env_file) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("do: {}", e);
                return 1;
            }
        };
        let records = match do_list_records(&token) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("do: {}", e);
                return 1;
            }
        };
        let missing: Vec<String> = do_status(&expected, &records)
            .into_iter()
            .filter(|(s, _, _)| s == "MISSING")
            .map(|(_, _, name)| name)
            .collect();
        let other: Vec<(String, String)> = do_status(&expected, &records)
            .into_iter()
            .filter(|(s, _, _)| s == "OTHER")
            .map(|(_, d, n)| (n, d))
            .collect();

        if !missing.is_empty() {
            println!("\nDO: creating {} CNAMEs -> {}", missing.len(), DO_CNAME_TARGET);
            for fqdn in &missing {
                let label = do_label(fqdn);
                match do_create_cname(&token, &label) {
                    Ok(id) => println!("  + {} -> {} (id {})", label, DO_CNAME_TARGET, id),
                    Err(e) => {
                        eprintln!("do: {}", e);
                        return 1;
                    }
                }
            }
        } else {
            println!("\nDO: up to date ({} records)", records.len());
        }
        if !other.is_empty() {
            println!("  note: these expected names exist with a different record (left as-is):");
            for (n, d) in &other {
                println!("    {}  ({})", n, d);
            }
        }
    }

    println!("\nsync complete");
    0
}

/// Remove rename leftovers — records dns-sync manages that were expected
/// previously but are no longer. Explicitly *not* part of `sync`: removal is
/// never automatic. Never touches records that predate dns-sync. Writes the
/// current expected list as the new baseline on success.
fn run_prune(opts: &Opts) -> i32 {
    let expected = match read_expected(&opts.expected) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };
    let last = match read_last_expected(&opts.last_expected) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };
    println!("== dns-sync prune: {} expected names ==", expected.len());

    // --- router ---
    if !opts.do_only {
        let managed = match router_managed_aliases(&opts.ssh_key, &opts.router) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("router: {}", e);
                return 1;
            }
        };
        let (stale, legacy) = router_managed_split(&last, &expected, &managed);
        if !stale.is_empty() {
            println!(
                "\nrouter: removing {} stale names from services1 host override aliases",
                stale.len()
            );
            for n in &stale {
                println!("  - {}", n);
            }
            if let Err(e) = router_remove_aliases(&opts.ssh_key, &opts.router, &stale) {
                eprintln!("router: {}", e);
                return 1;
            }
            match router_hosts(&opts.ssh_key, &opts.router) {
                Ok(after) => {
                    let still: Vec<&String> =
                        stale.iter().filter(|n| after.contains_key(*n)).collect();
                    if still.is_empty() {
                        println!(
                            "  verified: all {} names gone from /var/etc/dnsmasq-hosts",
                            stale.len()
                        );
                    } else {
                        eprintln!("  NOT verified, still present: {:?}", still);
                        return 1;
                    }
                }
                Err(e) => {
                    eprintln!("router re-read failed: {}", e);
                    return 1;
                }
            }
        } else {
            println!(
                "\nrouter: no stale aliases ({} managed, {} legacy left as-is)",
                managed.len(),
                legacy.len()
            );
        }
        if !legacy.is_empty() {
            println!("  note: left as-is (never expected): {}", legacy.join(", "));
        }
    }

    // --- DigitalOcean ---
    if !opts.router_only {
        let token = match do_token(&opts.env_file) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("do: {}", e);
                return 1;
            }
        };
        let records = match do_list_records(&token) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("do: {}", e);
                return 1;
            }
        };
        let (stale, legacy) = do_managed_split(&last, &expected, &records);
        if !stale.is_empty() {
            println!(
                "\nDO: deleting {} stale CNAMEs -> {}",
                stale.len(),
                DO_CNAME_TARGET
            );
            for r in &stale {
                println!("  - {} (id {})", r.name, r.id);
            }
            for r in &stale {
                if let Err(e) = do_delete_record(&token, r.id) {
                    eprintln!("do: {}", e);
                    return 1;
                }
            }
            match do_list_records(&token) {
                Ok(after) => {
                    let still: Vec<String> = stale
                        .iter()
                        .filter(|r| after.iter().any(|a| a.id == r.id))
                        .map(|r| r.name.clone())
                        .collect();
                    if still.is_empty() {
                        println!("  verified: all {} records deleted", stale.len());
                    } else {
                        eprintln!("  NOT verified, still present: {:?}", still);
                        return 1;
                    }
                }
                Err(e) => {
                    eprintln!("do re-read failed: {}", e);
                    return 1;
                }
            }
        } else {
            println!(
                "\nDO: no stale CNAMEs ({} managed, {} legacy left as-is)",
                records
                    .iter()
                    .filter(|r| r.typ == "CNAME" && cname_target_eq(&r.data, DO_CNAME_TARGET))
                    .count(),
                legacy.len()
            );
        }
        if !legacy.is_empty() {
            println!(
                "  note: left as-is (never expected): {}",
                legacy.iter().map(|r| r.name.clone()).collect::<Vec<_>>().join(", ")
            );
        }
    }

    // Success: DNS is now in line with the current expected list.
    if let Err(e) = write_last_expected(&opts.last_expected, &expected) {
        eprintln!("warning: could not update {}: {}", opts.last_expected, e);
    }
    println!("\nprune complete");
    0
}

fn main() {
    let (opts, subcmd) = parse_args();
    let code = match subcmd.as_str() {
        "check" => run_check(&opts),
        "sync" => run_sync(&opts),
        "prune" => run_prune(&opts),
        _ => usage(),
    };
    std::process::exit(code);
}
