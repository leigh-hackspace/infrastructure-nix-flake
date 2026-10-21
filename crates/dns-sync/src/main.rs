//! dns-sync — keep the LAN router's dnsmasq and DigitalOcean DNS in step
//! with the nginx reverse proxy on services1.
//!
//! The set of names that *should* exist is generated from the nginx config
//! at build time (see machines/services1/dns-sync.nix) and written to
//! `/etc/dns-sync/expected-int-names` — one `*.int.leighhack.org` FQDN per
//! line (vhost names plus their serverAliases). This tool compares that
//! list against two DNS sources:
//!
//!   * the router's dnsmasq — `machines/gw/`, at 10.3.1.1, whose
//!     `/var/lib/dnsmasq/names.hosts` is one of the two `addn-hosts` files it
//!     reads (the other, `/etc/dnsmasq-records.hosts`, is generated from the
//!     network's tables and is *not* dns-sync's to write). Reached over ssh as
//!     `leigh-admin` with the machine-hop key; the router has no root login,
//!     so every command on it goes through `sudo -n` (its address, its user:
//!     `common/hosts.nix` and AGENTS.md), and
//!   * the `leighhack.org` zone on DigitalOcean — the public DNS that DoH
//!     users see (token in `CONFIG.ENV_FILE`).
//!
//! Run it on services1 (it needs the DO token and ssh access to the
//! router):
//!
//!   sudo dns-sync check            # report whether every vhost is in DNS
//!   sudo dns-sync sync             # add missing records (router + DO)
//!
//! The tool is strictly additive: it only ever *adds* missing names (router
//! hosts lines / DO CNAMEs) and never deletes or rewrites existing records.
//! Lots of `*.int.leighhack.org` names legitimately point at other hosts
//! (cameras, Pis, APs, switches, the authentik appliance...), so anything not
//! in the expected list is out of scope and is never touched.  `prune` is the
//! one deliberate exception and it is scoped by `last-expected` — see there.
//!
//! A vhost is one record *per address*, not one per name: the old box's
//! `dnsmasq-hosts` carried `*.int` names against all three of services1's
//! addresses (IPv4, public IPv6, ULA IPv6) and a name missing its IPv6 lines is
//! the failure this network has already been bitten by — a dual-stack client
//! tries IPv6, hangs, and only then falls back (`machines/gw/README.md`, "The
//! third way dnsmasq breaks the LAN silently").  The addresses come from
//! `/etc/dns-sync/services1-addresses`, generated from `common/dns-records.nix`
//! by machines/services1/dns-sync.nix, so this file is the only place they live.
//!
//! Writing the router's file is a whole-file replace: read it, edit the copy in
//! memory, verify that nothing outside the intended (address, name) pairs moved,
//! then `cat > tmp && mv -f` as root with mode 0644, then `systemctl reload
//! dnsmasq`.  Nothing in the middle reorders, reformats or re-emits a line it
//! did not intend to change, because the file also carries the records
//! dns-sync does not own and those have to survive byte-for-byte.  `reload`
//! (SIGHUP) is enough — dnsmasq re-reads `addn-hosts` on it — and a `restart`
//! would take the network's DNS down for a second to no purpose.
//!
//! Zero external crates (house style): HTTP goes through `curl`, the router
//! edit goes through `ssh` + `sudo -n sh -c` (the router has no python3, and
//! the edit is a replace-and-rename, not a program), and JSON is parsed with
//! the shared hand-rolled parser in `common-rs/json`.

use common_json as json;

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::io::Write;
use std::process::{Command, Stdio};

const DEFAULT_EXPECTED: &str = "/etc/dns-sync/expected-int-names";
/// The machine-hop key, installed by common/users.nix (the same private key on
/// every flake-managed machine).  Override with --ssh-key; the flake passes the
/// path explicitly, so this default is only the local-dev convenience.
const DEFAULT_SSH_KEY: &str = "/home/leigh-admin/.ssh/agent-hop-key";
/// Pinned router host keys.  dns-sync sshes to the router as **root** and edits
/// its DNS config, so host identity is verified against this list rather than
/// accepted on first contact (`accept-new` would silently re-trust a key after
/// a router reinstall, or an on-path impersonation).  Regenerate with
/// `just router-known-hosts`; missing file = no ssh, not an unverified one.
const DEFAULT_KNOWN_HOSTS: &str = "/etc/dns-sync/known_hosts";
/// `leigh-admin`, never root: the router's ssh is the fleet's (`prohibit-
/// password` for root), and `common/users.nix` gives this account passwordless
/// sudo, which is how the tool edits a root-owned file without a root login.
const DEFAULT_ROUTER: &str = "leigh-admin@10.3.1.1";
const DEFAULT_ENV_FILE: &str = "/var/lib/secrets/.env";
/// Names DNS was last brought in line with (written by `sync` and `prune`).
/// Used to tell rename leftovers (were expected, no longer are — prune them)
/// apart from records that predate dns-sync and were never expected (left
/// as-is).
const DEFAULT_LAST_EXPECTED: &str = "/var/lib/dns-sync/last-expected";
/// The mutable `addn-hosts` file on the router.  Created empty by
/// `machines/gw/dns.nix`'s tmpfiles rule and never rewritten by the flake, so
/// it is dns-sync's to own — but it is *shared* with every record added to it
/// by hand or by an older tool, which is the reason `sync` is additive.
const DEFAULT_NAMES_FILE: &str = "/var/lib/dnsmasq/names.hosts";
/// The addresses every *.int vhost must answer with, one per line (IPv4
/// first).  Generated from `common/dns-records.nix` — read it, do not hardcode
/// a second copy here (AGENTS.md rule 2, one fact one place).
const DEFAULT_ADDRESSES: &str = "/etc/dns-sync/services1-addresses";

const ZONE: &str = "leighhack.org";
/// Target of the DO CNAMEs for *.int names (nginx.int.leighhack.org).
/// The DO API requires the trailing dot on the data field.
const DO_CNAME_TARGET: &str = "nginx.int.leighhack.org.";
const DO_TTL: u64 = 60;

const DO_RECORDS_API: &str = "https://api.digitalocean.com/v2/domains/leighhack.org/records";

/// The remote half of a write: take the new file on stdin, then put it in
/// place.  Runs under `sudo -n sh -c` on the router, so: POSIX sh, and the
/// router has no python and no assumptions beyond cat/chown/chmod/mv.
///
/// Four things are deliberate:
///
/// * the temp file lives in the **target's own directory**, so `mv -f` is a
///   rename: dnsmasq sees either the old file or the new one, never a partial
///   one, and a dropped ssh connection mid-`cat` leaves the old file in place;
/// * `umask 022` and an explicit `chmod`/`chown`, because a rename carries the
///   temp file's mode and owner, not the target's, and the file has to stay
///   `0644 root:root` — dnsmasq answers the network by reading it as a
///   non-privileged user, and an unreadable addn-hosts is a router that exits
///   at startup (machines/gw/dns.nix);
/// * the `trap` deletes the temp file if the script dies, because a stray
///   `.names.hosts.dns-sync.1234` in `/var/lib/dnsmasq` is the kind of thing a
///   future operator has to stop and read before trusting the directory;
/// * `set -eu` so a failed chown aborts before the rename rather than shipping
///   a file dnsmasq cannot open.
fn write_script(path: &str) -> String {
    let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("/");
    format!(
        "set -eu\numask 022\ntmp={dir}/.names.hosts.dns-sync.$$\ntrap 'rm -f \"$tmp\"' EXIT\ncat > \"$tmp\"\nchown root:root \"$tmp\"\nchmod 0644 \"$tmp\"\nmv -f \"$tmp\" {path}\n"
    )
}

/// Quote a string for the remote shell as a single word (ssh hands its
/// argument to the login shell, so anything with `$`, quotes or spaces in it
/// has to be quoted here rather than hoped over).
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

// ---------------------------------------------------------------------------
// The router's addn-hosts file
// ---------------------------------------------------------------------------

/// One address/name pair — the unit of everything dns-sync does to the
/// router's file.  A *name* is never enough on its own: `authentik.int` exists
/// pointing at the appliance and a `*.int` vhost exists pointing at services1,
/// and the only safe edit is the one that names which of the two it means.
type Pair = (String, String);

/// The router's `/var/lib/dnsmasq/names.hosts`: `addr<TAB>name [name...]`
/// lines, dnsmasq's hosts-file format.  Blank and `#` lines are carried
/// through as opaque text — dnsmasq honours `#` only at the *start* of a line,
/// which is also why this tool writes no trailing `# managed by dns-sync`
/// marker: the `#` would be parsed as a hostname.
#[derive(Debug, Clone, Default)]
struct HostsFile {
    /// Raw lines, in file order.  Lines that are not edited are re-emitted
    /// from here verbatim, which is the whole of the "preserve everything we
    /// do not own" guarantee.
    lines: Vec<String>,
}

/// Split one line into (address, names); `None` for blank and comment lines.
fn host_line_fields(line: &str) -> Option<(String, Vec<String>)> {
    let t = line.trim();
    if t.is_empty() || t.starts_with('#') {
        return None;
    }
    let mut it = t.split_whitespace();
    let addr = it.next().unwrap_or("").to_string();
    let names: Vec<String> = it.map(str::to_string).collect();
    if addr.is_empty() || names.is_empty() {
        return None;
    }
    Some((addr, names))
}

impl HostsFile {
    fn parse(text: &str) -> Self {
        HostsFile {
            lines: text.lines().map(str::to_string).collect(),
        }
    }

    fn render(&self) -> String {
        let mut out = self.lines.join("\n");
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out
    }

    /// Every (address, name) pair in the file.
    fn pairs(&self) -> BTreeSet<Pair> {
        let mut set = BTreeSet::new();
        for line in &self.lines {
            if let Some((addr, names)) = host_line_fields(line) {
                for n in names {
                    set.insert((addr.clone(), n));
                }
            }
        }
        set
    }

    /// name -> every address it answers with (a name with three `*.int` lines
    /// resolves to all three; dnsmasq returns them all).
    fn addresses(&self) -> BTreeMap<String, BTreeSet<String>> {
        let mut map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (addr, name) in self.pairs() {
            map.entry(name).or_default().insert(addr);
        }
        map
    }

    /// The names dns-sync *could* be managing: those answering with one of
    /// services1's addresses.  Whether a name is *stale* or merely legacy is a
    /// question about `last-expected`, not about this file, so it is asked
    /// elsewhere (`router_managed_split`).
    fn managed_names(&self, addrs: &[String]) -> Vec<String> {
        let mine: BTreeSet<String> = addrs.iter().cloned().collect();
        let mut names: Vec<String> = self
            .addresses()
            .into_iter()
            .filter(|(_, a)| a.iter().any(|x| mine.contains(x)))
            .map(|(n, _)| n)
            .collect();
        names.sort();
        names
    }

    /// Used by the tests to say which line survived; the production paths all
    /// work on pair *sets* (`pairs`, `managed_names`), so this would be dead
    /// code outside them.
    #[cfg(test)]
    fn has_pair(&self, addr: &str, name: &str) -> bool {
        self.pairs().contains(&(addr.to_string(), name.to_string()))
    }

    /// Append `names` under each of `addrs`, one line per pair, at the end of
    /// the file in (name, address) order.  Never touches an existing line, even
    /// one that already carries a sibling name — dnsmasq merges duplicate names
    /// across lines, and rewriting a line this file's owner added by hand is not
    /// dns-sync's business.  Returns the pairs added.
    fn add(&mut self, addrs: &[String], names: &[String]) -> Vec<Pair> {
        // `render` always terminates the last line, and `parse` never invents
        // one, so a file that came off the router without a final newline is
        // whole again here and the first appended line cannot be glued to it.
        let mut have = self.pairs();
        let mut added = Vec::new();
        for name in names {
            for addr in addrs {
                let pair = (addr.clone(), name.clone());
                if !have.insert(pair.clone()) {
                    continue; // already answers there; nothing to do
                }
                self.lines.push(format!("{}\t{}", addr, name));
                added.push(pair);
            }
        }
        added
    }

    /// Drop `names` from `addrs` only.  A name that also answers elsewhere
    /// keeps those lines (`authentik.int` at the appliance), and a line shared
    /// with a name that is not being removed keeps that name.  Returns the
    /// pairs removed.
    fn remove(&mut self, addrs: &[String], names: &[String]) -> Vec<Pair> {
        let drop_addrs: BTreeSet<&str> = addrs.iter().map(String::as_str).collect();
        let drop_names: BTreeSet<&str> = names.iter().map(String::as_str).collect();
        let mut removed = Vec::new();
        let mut out: Vec<String> = Vec::with_capacity(self.lines.len());
        for line in self.lines.iter() {
            match host_line_fields(line) {
                Some((addr, line_names)) if drop_addrs.contains(addr.as_str()) => {
                    let gone: Vec<String> = line_names
                        .iter()
                        .filter(|n| drop_names.contains(n.as_str()))
                        .cloned()
                        .collect();
                    for n in gone {
                        removed.push((addr.clone(), n));
                    }
                    let kept: Vec<&String> = line_names
                        .iter()
                        .filter(|n| !drop_names.contains(n.as_str()))
                        .collect();
                    if kept.is_empty() {
                        continue; // the whole line was dns-sync's
                    }
                    let mut rebuilt = addr;
                    rebuilt.push('\t');
                    rebuilt.push_str(&kept.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" "));
                    out.push(rebuilt);
                }
                _ => out.push(line.clone()),
            }
        }
        self.lines = out;
        removed
    }
}

/// Render up to three pairs for an error message — enough to see what the tool
/// was about to do to the network's DNS, without dumping 183 lines.
fn fmt_pairs<'a, I: Iterator<Item = &'a Pair>>(it: I) -> String {
    it.take(3)
        .map(|p| format!("{} {}", p.0, p.1))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The guard between "edit the copy in memory" and "overwrite the file the
/// whole network resolves through": nothing may have moved except the pairs
/// that were asked to move.  Every failure here is a bug in this file rather
/// than a bad state on the router, which is exactly when it is cheapest to
/// catch — the router's file is left untouched and the diff is printed instead.
fn verify_edit(
    before: &HostsFile,
    after: &HostsFile,
    added: &[Pair],
    removed: &[Pair],
) -> Result<(), String> {
    let b = before.pairs();
    let a = after.pairs();
    let want_added: BTreeSet<Pair> = added.iter().cloned().collect();
    let want_removed: BTreeSet<Pair> = removed.iter().cloned().collect();

    let unexpected_lost: Vec<Pair> = b
        .difference(&a)
        .filter(|p| !want_removed.contains(*p))
        .cloned()
        .collect();
    if !unexpected_lost.is_empty() {
        return Err(format!(
            "refusing to write: {} record(s) would vanish that were not asked to be removed (e.g. {})",
            unexpected_lost.len(),
            fmt_pairs(unexpected_lost.iter())
        ));
    }
    let unexpected_gained: Vec<Pair> = a
        .difference(&b)
        .filter(|p| !want_added.contains(*p))
        .cloned()
        .collect();
    if !unexpected_gained.is_empty() {
        return Err(format!(
            "refusing to write: {} record(s) would appear that were not asked for (e.g. {})",
            unexpected_gained.len(),
            fmt_pairs(unexpected_gained.iter())
        ));
    }
    for p in want_removed.iter() {
        if a.contains(p) {
            return Err(format!(
                "refusing to write: {} {} should have been removed and was not",
                p.0, p.1
            ));
        }
    }
    for p in want_added.iter() {
        if !a.contains(p) {
            return Err(format!(
                "refusing to write: {} {} should have been added and was not",
                p.0, p.1
            ));
        }
    }
    // Opaque lines (comments, blanks) are carried, not parsed: losing one is
    // still a change to a file dns-sync does not fully own.
    let opaque = |h: &HostsFile| {
        h.lines
            .iter()
            .filter(|l| host_line_fields(l).is_none())
            .count()
    };
    if opaque(before) != opaque(after) {
        return Err(format!(
            "refusing to write: {} comment/blank line(s) in the router's file became {}",
            opaque(before),
            opaque(after)
        ));
    }
    Ok(())
}


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

fn ssh(
    ssh_key_file: &str,
    known_hosts: &str,
    dest: &str,
    remote_cmd: &str,
) -> Result<String, String> {
    run_checked(
        "ssh",
        &[
            "-i",
            ssh_key_file,
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            &format!("UserKnownHostsFile={known_hosts}"),
            dest,
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
// Router (gw: /var/lib/dnsmasq/names.hosts, over ssh + sudo)
// ---------------------------------------------------------------------------

/// How to reach the router and edit its hosts file.  Host identity is pinned
/// (see DEFAULT_KNOWN_HOSTS): this edits the DNS the whole network resolves
/// through, so `accept-new` — which silently trusts a new key after a router
/// reinstall, or an on-path impersonation — is not good enough.
struct Router {
    ssh_key_file: String,
    known_hosts: String,
    dest: String,
    names_file: String,
}

impl Router {
    fn new(opts: &Opts) -> Self {
        Router {
            ssh_key_file: opts.ssh_key_file.clone(),
            known_hosts: opts.known_hosts.clone(),
            dest: opts.router.clone(),
            names_file: opts.names_file.clone(),
        }
    }

    /// One command on the router, as root.  `leigh-admin` has passwordless
    /// sudo and the router has no root login (`machines/gw/README.md`), so
    /// every command — reads included, the file is root-owned 0644 — goes
    /// through `sudo -n`, which fails loudly rather than prompting if the
    /// grant is ever removed.
    fn sudo(&self, cmd: &str) -> Result<String, String> {
        ssh(
            &self.ssh_key_file,
            &self.known_hosts,
            &self.dest,
            &format!("sudo -n {}", cmd),
        )
    }

    /// Read the router's `addn-hosts` file.
    ///
    /// A read failure is fatal and is never treated as "empty": this file also
    /// carries records dns-sync does not own (cameras, APs, Pi Room boards,
    /// `authentik.int` at the appliance), and a tool that mistook "cannot read"
    /// for "nothing there" would write back a file containing only the vhosts
    /// and take the rest of the network's DNS with it.
    fn read_hosts(&self) -> Result<HostsFile, String> {
        match self.sudo(&format!("cat {}", sh_quote(&self.names_file))) {
            Ok(out) => Ok(HostsFile::parse(&out)),
            Err(e) => Err(format!(
                "{}: {} — refusing to continue: dns-sync edits this file in place, \
                 so it has to be readable (it is created by machines/gw/dns.nix's \
                 tmpfiles rule, and dnsmasq exits if its addn-hosts path is gone)",
                self.names_file, e
            )),
        }
    }

    /// Replace the file with `text`: write a sibling, set its ownership, rename
    /// over the target.  Same directory, so `mv -f` is a rename and dnsmasq
    /// either sees the old file or the new one, never a half-written one; if
    /// `cat` dies mid-stream the rename never happens.  `umask 022` plus the
    /// explicit `chmod`/`chown` because tmpfiles' rule is 0644 root:root and a
    /// dnsmasq that cannot read its own addn-hosts file is a network with no DNS.
    fn write_hosts(&self, text: &str) -> Result<(), String> {
        let script = write_script(&self.names_file);
        let (ok, stdout, stderr) = run_input(
            "ssh",
            &self
                .ssh_cmd(&format!("sudo -n sh -c {}", sh_quote(&script)))
                .iter()
                .map(String::as_str)
                .collect::<Vec<&str>>(),
            text,
        )?;
        if !ok {
            return Err(format!(
                "could not replace {} on the router: {}",
                self.names_file,
                if stderr.trim().is_empty() {
                    stdout.trim().to_string()
                } else {
                    stderr.trim().to_string()
                }
            ));
        }
        Ok(())
    }

    /// Ask dnsmasq to re-read its config and `addn-hosts` files.  SIGHUP, not a
    /// restart: `systemctl reload dnsmasq` is the unit's own `ExecReload`, and
    /// this is the process answering port 53 for every VLAN — a restart is a
    /// second of silence on the network's most-queried service for no gain.
    fn reload_dnsmasq(&self) -> Result<(), String> {
        self.sudo("systemctl reload dnsmasq")
            .map_err(|e| format!("dnsmasq reload failed: {}", e))?;
        Ok(())
    }

    /// ssh argv for one remote command.  Owned strings, because the caller
    /// needs `&[&str]` and the pieces (`UserKnownHostsFile=…`, the quoted
    /// command) are built here.
    fn ssh_cmd(&self, remote_cmd: &str) -> Vec<String> {
        vec![
            "-i".into(),
            self.ssh_key_file.clone(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ConnectTimeout=10".into(),
            "-o".into(),
            "StrictHostKeyChecking=yes".into(),
            "-o".into(),
            format!("UserKnownHostsFile={}", self.known_hosts),
            self.dest.clone(),
            remote_cmd.into(),
        ]
    }


    /// The whole edit, in the order that makes a mistake unimportant:
    /// read, edit a copy, prove the copy only moved what was asked, write,
    /// reload, then read the router's file again and check the intent landed.
    /// Nothing is written when there is nothing to write, so `sync` on a
    /// network that is already in line never touches the router at all.
    fn apply(&self, addrs: &[String], names: &[String], remove: bool) -> Result<usize, String> {
        if names.is_empty() {
            return Ok(0);
        }
        let before = self.read_hosts()?;
        let mut after = before.clone();
        let (added, removed): (Vec<Pair>, Vec<Pair>) = if remove {
            (Vec::new(), after.remove(addrs, names))
        } else {
            (after.add(addrs, names), Vec::new())
        };
        if added.is_empty() && removed.is_empty() {
            return Ok(0);
        }
        verify_edit(&before, &after, &added, &removed)?;
        for p in &added {
            println!("    + {} {}", p.0, p.1);
        }
        for p in &removed {
            println!("    - {} {}", p.0, p.1);
        }
        self.write_hosts(&after.render())?;
        self.reload_dnsmasq()?;

        // Proof, not hope: re-read the router's file and check the intent is in
        // it.  Anything else the file gained or lost is reported, not fatal —
        // it is somebody else's record, and dns-sync has no business rolling
        // back a change it did not make.
        let now = self.read_hosts()?;
        let now_pairs = now.pairs();
        let missing_now: Vec<Pair> = added
            .iter()
            .filter(|p| !now_pairs.contains(*p))
            .cloned()
            .collect();
        if !missing_now.is_empty() {
            return Err(format!(
                "wrote {} but these records are not in it: {}",
                self.names_file,
                fmt_pairs(missing_now.iter())
            ));
        }
        let still_there: Vec<Pair> = removed
            .iter()
            .filter(|p| now_pairs.contains(*p))
            .cloned()
            .collect();
        if !still_there.is_empty() {
            return Err(format!(
                "asked to remove {} but they are still in {}: {}",
                removed.len(),
                self.names_file,
                fmt_pairs(still_there.iter())
            ));
        }
        let intended: BTreeSet<Pair> = added
            .iter()
            .chain(removed.iter())
            .chain(before.pairs().iter())
            .cloned()
            .collect();
        let other: Vec<Pair> = now_pairs
            .difference(&intended)
            .cloned()
            .collect();
        if !other.is_empty() {
            eprintln!(
                "  note: {} gained records dns-sync did not write: {}",
                self.names_file,
                fmt_pairs(other.iter())
            );
        }
        Ok(added.len() + removed.len())
    }
}

/// The addresses every `*.int` vhost must answer with, in the order the lines
/// are written (IPv4 first, as the file is laid out today).
fn read_addresses(path: &str) -> Result<Vec<String>, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read services1 addresses from {}: {}", path, e))?;
    let mut addrs: Vec<String> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect();
    addrs.dedup();
    if addrs.is_empty() {
        return Err(format!("no addresses in {}", path));
    }
    if !addrs.iter().any(|a| a.contains('.') && !a.contains(':')) {
        return Err(format!(
            "{} lists no IPv4 for services1 — refusing to write AAAA-only records",
            path
        ));
    }
    Ok(addrs)
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

/// How each expected name stands against the router's file.
///
/// `PARTIAL` is the state that used to be invisible: a name answering on IPv4
/// but not IPv6.  It is a real record and `sync` completes it, because a
/// dual-stack client that gets no AAAA tries IPv6, hangs, and only then falls
/// back — which reads as "the internet is slow" (`machines/gw/README.md`).
fn router_status(
    expected: &[String],
    hosts: &BTreeMap<String, BTreeSet<String>>,
    addrs: &[String],
) -> Vec<(String, String, String)> {
    let mine: BTreeSet<&String> = addrs.iter().collect();
    expected
        .iter()
        .map(|name| match hosts.get(name) {
            Some(found) => {
                let hits: Vec<&str> = found
                    .iter()
                    .filter(|a| mine.contains(a))
                    .map(|s| s.as_str())
                    .collect();
                if hits.is_empty() {
                    (
                        "ELSEWHERE".into(),
                        format!(
                            "{} (not services1)",
                            found.iter().next().cloned().unwrap_or_default()
                        ),
                        name.clone(),
                    )
                } else {
                    let absent: Vec<&str> = addrs
                        .iter()
                        .filter(|a| !found.contains(*a))
                        .map(|s| s.as_str())
                        .collect();
                    if absent.is_empty() {
                        ("OK".into(), hits.join(" "), name.clone())
                    } else {
                        (
                            "PARTIAL".into(),
                            format!("no address line for {}", absent.join(", ")),
                            name.clone(),
                        )
                    }
                }
            }
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

/// Which of the names dns-sync manages on the router are stale (were expected,
/// no longer are) and which are legacy (predate dns-sync — the hand-restored
/// records and anything another tool put in `names.hosts`).  `managed` comes
/// from the router's file (`HostsFile::managed_names`), not from a second list
/// on the router: there is no `config.xml` alias block any more, and inventing
/// one would be a fact with nothing behind it.
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
    ssh_key_file: String,
    known_hosts: String,
    router: String,
    names_file: String,
    addresses: String,
    env_file: String,
    router_only: bool,
    do_only: bool,
}

fn usage() -> ! {
    eprintln!(
        "usage: dns-sync <check|sync|prune> [--expected FILE] [--last-expected FILE] \
         [--ssh-key PATH] [--known-hosts PATH] [--router HOST] [--names-file PATH] \
         [--addresses FILE] [--env-file PATH] [--router-only|--do-only]\n\n  check  report expected vs present DNS (exit 1 if missing, partial or stale)\n  sync   add missing records (strictly additive) and rebase last-expected\n  prune  remove rename leftovers — records dns-sync manages that were\n         expected previously but are no longer (never touches records that\n         predate dns-sync)\n\n  router defaults: {} editing {} with the addresses in {}; override with\n  --router/--names-file/--addresses when testing against something else.\n  Nothing is written unless something actually needs to change.",
        DEFAULT_ROUTER, DEFAULT_NAMES_FILE, DEFAULT_ADDRESSES
    );
    std::process::exit(2);
}

fn parse_args() -> (Opts, String) {
    let mut opts = Opts {
        expected: DEFAULT_EXPECTED.into(),
        last_expected: DEFAULT_LAST_EXPECTED.into(),
        ssh_key_file: DEFAULT_SSH_KEY.into(),
        known_hosts: DEFAULT_KNOWN_HOSTS.into(),
        router: DEFAULT_ROUTER.into(),
        names_file: DEFAULT_NAMES_FILE.into(),
        addresses: DEFAULT_ADDRESSES.into(),
        env_file: DEFAULT_ENV_FILE.into(),
        router_only: false,
        do_only: false,
    };
    let mut subcmd: Option<String> = None;
    let mut args = env::args().skip(1).peekable();
    while let Some(a) = args.next() {
        match a.as_str() {
            "check" | "sync" | "prune" if subcmd.is_none() => subcmd = Some(a),
            "--expected" | "--last-expected" | "--ssh-key" | "--known-hosts" | "--router"
            | "--names-file" | "--addresses" | "--env-file" => {
                let v = args.next().unwrap_or_else(|| usage());
                match a.as_str() {
                    "--expected" => opts.expected = v,
                    "--last-expected" => opts.last_expected = v,
                    "--ssh-key" => opts.ssh_key_file = v,
                    "--known-hosts" => opts.known_hosts = v,
                    "--router" => opts.router = v,
                    "--names-file" => opts.names_file = v,
                    "--addresses" => opts.addresses = v,
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
    let mut needs_sync = 0usize;
    let mut stale: Vec<String> = Vec::new();
    let mut legacy: Vec<String> = Vec::new();

    if !opts.do_only {
        let addrs = match read_addresses(&opts.addresses) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("router: {}", e);
                return 1;
            }
        };
        let router = Router::new(opts);
        let hosts = match router.read_hosts() {
            Ok(h) => h,
            Err(e) => {
                eprintln!("router: {}", e);
                return 1;
            }
        };
        let status = router_status(&expected, &hosts.addresses(), &addrs);
        print_table(
            &format!(
                "router dnsmasq @ {} ({})",
                opts.router, opts.names_file
            ),
            &status,
        );
        needs_sync += status
            .iter()
            .filter(|(s, _, _)| s == "MISSING" || s == "PARTIAL")
            .count();
        let managed = hosts.managed_names(&addrs);
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
        needs_sync += do_status(&expected, &records)
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

    if needs_sync > 0 {
        println!(
            "\n{} record(s) missing or incomplete — run `sudo dns-sync sync` to apply",
            needs_sync
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
        let addrs = match read_addresses(&opts.addresses) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("router: {}", e);
                return 1;
            }
        };
        let router = Router::new(opts);
        let hosts = match router.read_hosts() {
            Ok(h) => h,
            Err(e) => {
                eprintln!("router: {}", e);
                return 1;
            }
        };
        let status = router_status(&expected, &hosts.addresses(), &addrs);
        // MISSING and PARTIAL both mean "write the lines that are not there",
        // and both are one call: completing a name's IPv6 is the same additive
        // edit as adding the name, and neither rewrites a line that exists.
        let need: Vec<String> = status
            .iter()
            .filter(|(s, _, _)| s == "MISSING" || s == "PARTIAL")
            .map(|(_, _, name)| name.clone())
            .collect();
        let elsewhere: Vec<String> = status
            .iter()
            .filter(|(s, _, _)| s == "ELSEWHERE")
            .map(|(_, d, n)| format!("{} ({})", n, d))
            .collect();
        if !need.is_empty() {
            println!(
                "\nrouter: adding {} names to {} ({} address line(s) each)",
                need.len(),
                opts.names_file,
                addrs.len()
            );
            for n in &need {
                println!("  + {}", n);
            }
            // `apply` writes, reloads dnsmasq and re-reads the file, so it
            // returns Err unless the router actually answers the new records;
            // there is nothing left to verify here.
            if let Err(e) = router.apply(&addrs, &need, false) {
                eprintln!("router: {}", e);
                return 1;
            }
            println!("  verified: all {} names now answer in {}", need.len(), opts.names_file);
        } else {
            println!(
                "\nrouter: up to date ({} names in {})",
                hosts.addresses().len(),
                opts.names_file
            );
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

    // Rebase the prune baseline.  A name dns-sync has been asked to create is a
    // name a later rename can leave behind, and `prune` can only tell that
    // apart from a record that predates dns-sync (never removed) if the name is
    // in this list — so `sync` records it.  As a *union*: replacing the list
    // with `expected` here would destroy the evidence for the very name `prune`
    // exists to clean up after a rename, because sync-then-prune is the order
    // the justfile documents.
    let last = read_last_expected(&opts.last_expected).unwrap_or_default();
    if let Err(e) = write_last_expected(
        &opts.last_expected,
        &merge_baseline(&last, &expected, &[]),
    ) {
        eprintln!("warning: could not update {}: {}", opts.last_expected, e);
    }

    println!("\nsync complete");
    0
}

/// The new baseline: everything expected now, plus what was expected before and
/// has *not* just been removed.  `sync` passes no removals (so: the union), and
/// `prune` passes the names it actually deleted (so: the list, compacted).  The
/// difference matters for a scoped run — `prune --router-only` must not make the
/// DigitalOcean half unrecognisable as stale.
fn merge_baseline(last: &[String], expected: &[String], removed: &[String]) -> Vec<String> {
    let mut out: Vec<String> = last.iter().filter(|n| !removed.contains(n)).cloned().collect();
    out.extend_from_slice(expected);
    out.sort();
    out.dedup();
    out
}

/// Remove rename leftovers — records dns-sync manages that were expected
/// previously but are no longer. Explicitly *not* part of `sync`: removal is
/// never automatic. Never touches records that predate dns-sync. Rebases the
/// baseline on success (see `merge_baseline`).
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
    // Names this run actually deleted, in either half.  They leave the baseline;
    // everything else in it stays, so a scoped prune cannot blind a later one.
    let mut acted: Vec<String> = Vec::new();

    // --- router ---
    if !opts.do_only {
        let addrs = match read_addresses(&opts.addresses) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("router: {}", e);
                return 1;
            }
        };
        let router = Router::new(opts);
        let hosts = match router.read_hosts() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("router: {}", e);
                return 1;
            }
        };
        let managed = hosts.managed_names(&addrs);
        let (stale, legacy) = router_managed_split(&last, &expected, &managed);
        if !stale.is_empty() {
            println!(
                "\nrouter: removing {} stale names from {} (only their services1 lines)",
                stale.len(),
                opts.names_file
            );
            for n in &stale {
                println!("  - {}", n);
            }
            if let Err(e) = router.apply(&addrs, &stale, true) {
                eprintln!("router: {}", e);
                return 1;
            }
            println!("  verified: all {} names gone from {}", stale.len(), opts.names_file);
            acted.extend(stale.iter().cloned());
        } else {
            println!(
                "\nrouter: no stale names ({} managed, {} legacy left as-is)",
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
            // `do_label` first, because DO stores these as relative labels
            // (`filestore.int`) and the baseline is FQDNs — the two only meet
            // after normalising.
            acted.extend(
                stale
                    .iter()
                    .map(|r| format!("{}.{}", do_label(&r.name), ZONE)),
            );
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

    // Success: DNS is now in line with the current expected list, so that is the
    // new baseline — minus exactly what this run deleted.  Anything else the old
    // baseline knew about stays in it: a `prune --router-only` must not leave
    // the DigitalOcean half unable to recognise its own stale records.
    if let Err(e) = write_last_expected(
        &opts.last_expected,
        &merge_baseline(&last, &expected, &acted),
    ) {
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

// ---------------------------------------------------------------------------
// Tests
//
// The whole point of these is the 183 lines that are *not* dns-sync's: the
// router's `names.hosts` answers the cameras, the APs, the Pi Room boards and
// the authentik appliance too, and the only undo for getting its edit wrong is
// the backup taken before the first write.  So the tests are about what does
// not change.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// services1's three addresses, in the order the router's file groups them.
    fn addrs() -> Vec<String> {
        [
            "10.3.1.20",
            "2001:8b0:1d14:225::20",
            "fd99:dead:beef:225d::20",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn v(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// The shape read off the live file: tab after the address, more than one
    /// name on a line, three lines per vhost (one per address), records that
    /// belong to other hosts, and a comment.
    fn sample() -> String {
        [
            "# everything the router answers beyond the DHCP reservations",
            "10.3.1.21\tservices2.int.leighhack.org services2",
            "10.3.1.20\tservices1.int.leighhack.org services1",
            "10.3.1.20\tfilestore.int.leighhack.org",
            "10.3.1.36\tauthentik.int.leighhack.org",
            "10.3.1.15\tyunohost.int.leighhack.org",
            "2001:8b0:1d14:225::20\tfilestore.int.leighhack.org",
            "fd99:dead:beef:225d::20\tfilestore.int.leighhack.org",
            "",
        ]
        .join("\n")
    }

    #[test]
    fn a_file_survives_parse_and_render_unchanged() {
        let text = sample();
        let parsed = HostsFile::parse(&text);
        assert_eq!(parsed.render(), text, "round-trip changed the file");
    }

    #[test]
    fn add_writes_one_line_per_address_and_keeps_every_other_line() {
        let mut f = HostsFile::parse(&sample());
        let before = f.clone();
        let added = f.add(&addrs(), &v(&["strata.int.leighhack.org"]));
        assert_eq!(added.len(), 3, "one line per services1 address");
        assert_eq!(f.lines.len(), before.lines.len() + 3);
        // Everything that was there is still there, in the same order, at the
        // start of the file: the new records are appended, never interleaved.
        assert_eq!(&f.lines[..before.lines.len()], before.lines.as_slice());
        assert!(f.has_pair("10.3.1.36", "authentik.int.leighhack.org"));
        assert!(f.has_pair("10.3.1.15", "yunohost.int.leighhack.org"));
        verify_edit(&before, &f, &added, &[]).expect("add must verify");
    }

    #[test]
    fn add_twice_writes_nothing_the_second_time() {
        let names = v(&["strata.int.leighhack.org"]);
        let mut f = HostsFile::parse(&sample());
        assert_eq!(f.add(&addrs(), &names).len(), 3);
        assert!(f.add(&addrs(), &names).is_empty(), "re-adding is not a change");
    }

    #[test]
    fn remove_drops_only_the_services1_lines() {
        let mut f = HostsFile::parse(&sample());
        let before = f.clone();
        // `authentik.int` answers twice: at the appliance, which is not dns-sync's,
        // and at services1, left over from when the vhost moved.  Pruning the name
        // must take the second and leave the first alone.
        f.lines.push("10.3.1.20\tauthentik.int.leighhack.org".into());
        let ours = f.clone();

        let removed = f.remove(
            &addrs(),
            &v(&[
                "authentik.int.leighhack.org",
                "filestore.int.leighhack.org",
            ]),
        );
        assert_eq!(
            removed.len(),
            4,
            "authentik's services1 line and filestore's three"
        );
        assert_eq!(
            removed.iter().filter(|p| p.0 == "10.3.1.36").count(),
            0,
            "the appliance's record is not dns-sync's to remove"
        );
        assert!(f.has_pair("10.3.1.36", "authentik.int.leighhack.org"));
        assert!(
            !f.addresses().contains_key("filestore.int.leighhack.org"),
            "all three filestore lines answer at services1, so all three go"
        );
        assert_eq!(before.lines.len(), 8, "the sample's own line count");
        assert_eq!(ours.lines.len(), before.lines.len() + 1);
        verify_edit(&ours, &f, &[], &removed).expect("remove must verify");
    }

    #[test]
    fn remove_keeps_a_sibling_on_the_same_line() {
        let mut f = HostsFile::parse(&sample());
        let before = f.clone();
        let removed = f.remove(&addrs(), &v(&["services1.int.leighhack.org"]));
        assert_eq!(removed.len(), 1);
        assert!(
            f.lines
                .iter()
                .any(|l| l == "10.3.1.20\tservices1"),
            "the bare `services1` name on that line must survive: {:?}",
            f.lines
        );
        verify_edit(&before, &f, &[], &removed).expect("remove must verify");
    }

    #[test]
    fn remove_of_an_unmanaged_name_changes_nothing_and_verifies() {
        let mut f = HostsFile::parse(&sample());
        let before = f.clone();
        let removed = f.remove(&addrs(), &v(&["camera-front.int.leighhack.org"]));
        assert!(removed.is_empty());
        assert_eq!(f.lines, before.lines);
    }

    #[test]
    fn verify_edit_refuses_to_lose_a_line_it_did_not_mean_to() {
        let before = HostsFile::parse(&sample());
        let mut after = before.clone();
        // A bug that ate the appliance's record: the edit asked for nothing of
        // the sort, so nothing gets written to the router.
        after.lines.retain(|l| !l.contains("10.3.1.36"));
        let err = verify_edit(&before, &after, &[], &[]).unwrap_err();
        assert!(err.contains("would vanish"), "{}", err);
    }

    #[test]
    fn verify_edit_refuses_a_record_it_did_not_mean_to_write() {
        let before = HostsFile::parse(&sample());
        let mut after = before.clone();
        after
            .lines
            .push("10.3.1.20\tsomething.int.leighhack.org".into());
        let err = verify_edit(&before, &after, &[], &[]).unwrap_err();
        assert!(err.contains("not asked for"), "{}", err);
    }

    #[test]
    fn verify_edit_refuses_to_swallow_a_comment() {
        let before = HostsFile::parse(&sample());
        let mut after = before.clone();
        after.lines.retain(|l| !l.starts_with('#'));
        let err = verify_edit(&before, &after, &[], &[]).unwrap_err();
        assert!(err.contains("comment"), "{}", err);
    }

    #[test]
    fn a_file_with_no_trailing_newline_is_terminated_before_appending() {
        let text = "10.3.1.20\tfilestore.int.leighhack.org";
        let mut f = HostsFile::parse(text);
        let added = f.add(&addrs()[..1], &v(&["strata.int.leighhack.org"]));
        assert_eq!(added.len(), 1);
        let out = f.render();
        assert!(
            out.lines().any(|l| l == "10.3.1.20\tstrata.int.leighhack.org"),
            "the new record was glued to the last line: {:?}",
            out
        );
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn managed_names_are_the_ones_answering_at_services1() {
        let f = HostsFile::parse(&sample());
        let managed = f.managed_names(&addrs());
        assert!(managed.contains(&"filestore.int.leighhack.org".to_string()));
        assert!(!managed.contains(&"authentik.int.leighhack.org".to_string()));
        assert!(!managed.contains(&"services2.int.leighhack.org".to_string()));
    }

    #[test]
    fn router_status_names_what_sync_would_do() {
        let f = HostsFile::parse(&sample());
        let hosts = f.addresses();
        let expected = v(&[
            "filestore.int.leighhack.org",
            "authentik.int.leighhack.org",
            "strata.int.leighhack.org",
            "half.int.leighhack.org",
        ]);
        // `half` answers on IPv4 only: the state that used to be invisible.
        let mut with_half = f.clone();
        with_half
            .lines
            .push("10.3.1.20\thalf.int.leighhack.org".into());
        let rows = router_status(&expected, &with_half.addresses(), &addrs());
        let of = |n: &str| -> String {
            rows.iter()
                .find(|r| r.2 == n)
                .map(|r| r.0.clone())
                .unwrap_or_default()
        };
        assert_eq!(of("filestore.int.leighhack.org"), "OK");
        assert_eq!(of("authentik.int.leighhack.org"), "ELSEWHERE");
        assert_eq!(of("strata.int.leighhack.org"), "MISSING");
        assert_eq!(of("half.int.leighhack.org"), "PARTIAL");
        assert!(!hosts.is_empty());
    }

    #[test]
    fn merge_baseline_unions_for_sync_and_drops_only_what_was_removed() {
        let last = v(&["old.int.leighhack.org", "kept.int.leighhack.org"]);
        let expected = v(&["kept.int.leighhack.org", "new.int.leighhack.org"]);

        // sync: a union, because `old` is still in DNS and `prune` has to be
        // able to recognise it as a rename leftover afterwards.
        let after_sync = merge_baseline(&last, &expected, &[]);
        assert!(after_sync.contains(&"old.int.leighhack.org".to_string()));
        assert!(after_sync.contains(&"new.int.leighhack.org".to_string()));

        // prune: the name it actually deleted leaves the list, and a scoped run
        // that did not touch the other half must not erase it either.
        let after_prune = merge_baseline(&last, &expected, &v(&["old.int.leighhack.org"]));
        assert!(!after_prune.contains(&"old.int.leighhack.org".to_string()));
        assert_eq!(after_prune, v(&["kept.int.leighhack.org", "new.int.leighhack.org"]));
    }

    #[test]
    fn the_write_script_is_shell_and_names_the_right_file() {
        let s = write_script("/var/lib/dnsmasq/names.hosts");
        // Same directory as the target: the rename has to be a rename.
        assert!(
            s.contains("tmp=/var/lib/dnsmasq/.names.hosts.dns-sync.$$"),
            "{}",
            s
        );
        assert!(s.contains("mv -f \"$tmp\" /var/lib/dnsmasq/names.hosts"), "{}", s);
        assert!(s.contains("chmod 0644"), "{}", s);
        // `sh -n` is a syntax check, no execution: what the router is about to
        // run has to at least parse there, and a typo here is a write that
        // silently never happens.
        let check = Command::new("sh")
            .args(["-n", "-c", &s])
            .output()
            .expect("sh");
        assert!(
            check.status.success(),
            "write script does not parse: {}",
            String::from_utf8_lossy(&check.stderr)
        );
    }

    #[test]
    fn sh_quote_survives_a_real_shell() {
        // The contract dns-sync's whole write path rests on: the script, put in
        // the ssh command line as one word, must reach the remote shell byte for
        // byte.  `$$` must still be `$$` (it is the temp file's uniqueness) and
        // the `trap`'s own quotes must not end the word.  So ask a shell.
        let script = "trap 'rm -f \"$tmp\"' EXIT\ncat > \"$tmp\"\necho $$\n";
        let quoted = sh_quote(script);
        assert!(quoted.starts_with('\'') && quoted.ends_with('\''), "{}", quoted);
        let out = Command::new("sh")
            .arg("-c")
            .arg(format!("printf '%s' {}", quoted))
            .output()
            .expect("sh");
        assert!(out.status.success(), "sh rejected {:?}", quoted);
        assert_eq!(String::from_utf8_lossy(&out.stdout), script);

        // and the actual script, quoted the way write_hosts sends it
        let real = write_script("/var/lib/dnsmasq/names.hosts");
        let out = Command::new("sh")
            .arg("-c")
            .arg(format!("printf '%s' {}", sh_quote(&real)))
            .output()
            .expect("sh");
        assert_eq!(String::from_utf8_lossy(&out.stdout), real);
    }

    #[test]
    fn addresses_file_needs_an_ipv4() {
        let dir = std::env::temp_dir().join(format!("dns-sync-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let v6only = dir.join("v6only");
        std::fs::write(&v6only, "2001:8b0:1d14:225::20\n").unwrap();
        let err = read_addresses(v6only.to_str().unwrap()).unwrap_err();
        assert!(err.contains("no IPv4"), "{}", err);

        let empty = dir.join("empty");
        std::fs::write(&empty, "\n").unwrap();
        assert!(read_addresses(empty.to_str().unwrap()).is_err());

        let ok = dir.join("ok");
        std::fs::write(&ok, "10.3.1.20\n# comment\n2001:8b0:1d14:225::20\n").unwrap();
        assert_eq!(read_addresses(ok.to_str().unwrap()).unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
