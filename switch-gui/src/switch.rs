//! A logged-in CLI session on the switch.
//!
//! Three behaviours of this firmware shape everything in here, and all three
//! were established against 10.3.1.2 rather than assumed:
//!
//! 1. **The banner swallows input.** The CLI discards anything typed before it
//!    prints `Please Enter Login Name:`, and the discarded text is then read as
//!    the *password*.  So `connect` waits for each prompt instead of typing on
//!    sight, and a wrong-password attempt is reported as exactly what the
//!    switch says (see [`check_auth`]).
//! 2. **It pages.** Long tables stop with `-- MORE --, next page: Space, next
//!    line: Enter, quit: Control-C` and wait for a space.  `NAWS` is ignored, so
//!    claiming a 100-row window changes nothing; [`Switch::command`] answers the
//!    marker itself.  The line the marker interrupted *continues* after it, so
//!    the marker is deleted rather than the line.
//! 3. **It echoes.** The command comes back on the prompt line, prefixed with
//!    the prompt and terminated by `ESC E`; [`strip_echo`] removes it so callers
//!    parse only the response.
//!
//! The session is not thread-safe by design (a Telnet session is one console)
//! and the server keeps exactly one behind a mutex.

use std::collections::BTreeMap;
use std::thread;
use std::time::Duration;

use crate::model::{Counters, Port, Privilege, Snapshot, TrunkMember};
use crate::parse;
use crate::telnet::Telnet;
use crate::R;

/// How long to let the console settle between configuration commands.
///
/// The CLI **ignores keystrokes while it repaints** — the same reason it
/// swallows input after the login banner — so a command typed the instant its
/// prompt appears can be dropped on the floor, and the read then waits the full
/// CLI timeout for a response that was never taken. Measured: sending
/// `interface 5 name "…"` straight after `config` timed out with the buffer
/// still ending on the *previous* command's prompt, while the same line typed
/// with a pause in front of it applied cleanly.
///
/// Reads do not need it: each one is separated by the table it prints.
const SETTLE: Duration = Duration::from_millis(400);

const LOGIN_PROMPT: &str = "Please Enter Login Name";
const PASSWORD_PROMPT: &str = "Please Enter Password";
const MORE_MARK: &str = "-- MORE --";
const MORE_END: &str = "Control-C";
/// A 48-port table is ~4 pages at the CLI's 22-row page; this is headroom for
/// `show mac-addresses`-sized output, and a guard against a marker we fail to
/// clear turning into a hang.
const MAX_PAGES: usize = 200;

#[derive(Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub login: String,
    pub password: String,
    /// Manager credentials for `enable`.  Without them the session stays in
    /// the Operator context and every write is refused.
    pub enable_login: Option<String>,
    pub enable_password: Option<String>,
    /// How long to wait for any one prompt.  RADIUS is on the other end of the
    /// login and authentik is not always quick, so this is generous.
    pub timeout: Duration,
}

pub struct Switch {
    tn: Telnet,
    cfg: Config,
    pub privilege: Privilege,
}

impl Switch {
    /// Connect, log in, and `enable` if manager credentials were supplied.
    pub fn connect(cfg: &Config) -> R<Switch> {
        let mut tn = Telnet::connect(&cfg.host, cfg.port, cfg.timeout)?;
        let intro = strip_ansi(&tn.read_until(
            |raw| {
                let t = strip_ansi(raw);
                t.contains(LOGIN_PROMPT) || ends_at_prompt(&t)
            },
            cfg.timeout,
        )?);

        let mut sw = Switch {
            tn,
            cfg: cfg.clone(),
            privilege: Privilege::Operator,
        };
        if intro.contains(LOGIN_PROMPT) {
            sw.login()?;
        } else {
            // A switch configured without a login banner drops the console
            // straight to a prompt; take the privilege from what we see.
            sw.privilege = prompt_privilege(&intro);
        }

        let wants_enable =
            sw.cfg.enable_login.is_some() || sw.cfg.enable_password.is_some();
        if wants_enable && !sw.privilege.is_manager() {
            // A failed `enable` must not kill the session.  The dashboard is
            // still useful read-only, and letting a stale Manager password turn
            // a write outage into a total outage is the worse failure.  The
            // page already explains why the controls are disabled.
            if let Err(e) = sw.enable() {
                eprintln!("switch-gui: `enable` refused, staying read-only: {e}");
            }
        }
        Ok(sw)
    }

    fn login(&mut self) -> R<()> {
        self.tn.send_line(&self.cfg.login)?;
        self.tn.read_until(
            |raw| strip_ansi(raw).contains(PASSWORD_PROMPT),
            self.cfg.timeout,
        )?;
        self.tn.send_line(&self.cfg.password)?;
        let out = strip_ansi(&self.tn.read_until(
            |raw| ends_at_prompt(&strip_ansi(raw)),
            self.cfg.timeout,
        )?);
        check_auth(&out)?;
        self.privilege = prompt_privilege(&out);
        Ok(())
    }

    /// `enable` — the switch re-asks for a *login name* here (it is a second
    /// login, not a password-only unlock), so this is the same two-prompt
    /// exchange as [`Switch::login`].
    pub fn enable(&mut self) -> R<()> {
        let login = self
            .cfg
            .enable_login
            .clone()
            .unwrap_or_else(|| "manager".to_string());
        let password = self.cfg.enable_password.clone().unwrap_or_default();

        self.tn.send_line("enable")?;
        let out = strip_ansi(&self.tn.read_until(
            |raw| {
                let t = strip_ansi(raw);
                t.contains(LOGIN_PROMPT) || t.contains(PASSWORD_PROMPT) || ends_at_prompt(&t)
            },
            self.cfg.timeout,
        )?);
        if out.contains(LOGIN_PROMPT) {
            self.tn.send_line(&login)?;
            self.tn.read_until(
                |raw| strip_ansi(raw).contains(PASSWORD_PROMPT),
                self.cfg.timeout,
            )?;
        }
        self.tn.send_line(&password)?;
        let out = strip_ansi(&self.tn.read_until(
            |raw| ends_at_prompt(&strip_ansi(raw)),
            self.cfg.timeout,
        )?);
        check_auth(&out)?;
        self.privilege = prompt_privilege(&out);
        if !self.privilege.is_manager() {
            return Err(
                "`enable` did not reach the Manager context (prompt is still an operator prompt)"
                    .to_string(),
            );
        }
        Ok(())
    }

    /// Run one CLI command and return its response, paging and cleaning.
    pub fn command(&mut self, cmd: &str) -> R<String> {
        self.tn.send_line(cmd)?;
        let mut acc = String::new();
        let mut pages = 0usize;
        loop {
            let chunk = self
                .tn
                .read_until(
                    |raw| {
                        let t = strip_ansi(raw);
                        waits_for_key(&t) || ends_at_prompt(&t)
                    },
                    self.cfg.timeout,
                )
                // Name the command in every failure: `apply` sends several in a
                // row and a bare timeout from the socket does not say which one
                // the console stopped answering.
                .map_err(|e| format!("while sending `{cmd}`: {e}"))?;
            let chunk = strip_ansi(&chunk);
            let paged = waits_for_key(&chunk);
            acc.push_str(&strip_more(&chunk));
            if !paged {
                break;
            }
            pages += 1;
            if pages > MAX_PAGES {
                return Err(format!(
                    "gave up after {MAX_PAGES} pages of `{cmd}` — the CLI kept printing \
                     `-- MORE --`"
                ));
            }
            self.tn.send_raw(" ")?;
        }
        let out = strip_echo(&acc, cmd);
        // Check the raw response as well as the cleaned one.  This CLI paints
        // the prompt, the echoed command and the refusal on a single line, and
        // anything the cleaner drops would take an `Invalid input:` with it —
        // which for a write means reporting "applied" for a command the switch
        // never ran.
        check_cli_error(&acc, cmd)?;
        check_cli_error(&out, cmd)?;
        Ok(out)
    }

    /// Send a sequence of configuration commands.
    ///
    /// This firmware only accepts configuration **inside config mode**: at the
    /// Manager Exec prompt `port 1 name x` answers `Invalid input: port`.  Both
    /// `config` and `configure terminal` get there (measured on 10.3.1.2: the
    /// prompt goes `switch1#` → `switch1(config)#`), and `exit` comes back out.
    ///
    /// The session always leaves config mode, including when a command is
    /// refused — a cached console left sitting in `(config)#` would make the
    /// next *read* command fail in a way that looks like a parsing bug.
    pub fn apply(&mut self, cmds: &[String]) -> R<()> {
        if !self.privilege.is_manager() {
            return Err("the session is not in the Manager context".to_string());
        }
        self.command("config")?;
        let mut failed: Option<String> = None;
        for cmd in cmds {
            thread::sleep(SETTLE);
            if let Err(e) = self.command(cmd) {
                failed = Some(e);
                break;
            }
        }
        thread::sleep(SETTLE);
        let left = self.command("exit");
        match (failed, left) {
            (None, Ok(_)) => Ok(()),
            (None, Err(e)) => Err(format!(
                "applied, but leaving config mode failed ({e}) — the console was dropped"
            )),
            (Some(f), Ok(_)) => Err(f),
            (Some(f), Err(e)) => Err(format!("{f}; and leaving config mode failed: {e}")),
        }
    }
}

/// Read the whole configuration into a [`Snapshot`].
///
/// One command per table, and a failure of any single one is recorded as a
/// warning rather than aborting the refresh: `show trunks` not existing on
/// some firmware should cost the trunk column, not the whole page.
pub fn read_snapshot(sw: &mut Switch) -> R<Snapshot> {
    let mut snap = Snapshot::default();
    let warn = |w: &str, e: String, warnings: &mut Vec<String>| {
        eprintln!("switch: {w} failed: {e}");
        warnings.push(format!("{w}: {e}"));
    };

    match sw.command("show system") {
        Ok(out) => snap.system = parse::system_info(&out),
        Err(e) => warn("show system", e, &mut snap.warnings),
    }

    let vlans = match sw.command("show vlan") {
        Ok(out) => parse::vlans(&out),
        Err(e) => {
            warn("show vlan", e, &mut snap.warnings);
            Vec::new()
        }
    };
    snap.vlans = vlans.clone();

    // Membership needs one command per VLAN; the switch has room for 64.
    for v in &vlans {
        let cmd = format!("show vlan {}", v.id);
        match sw.command(&cmd) {
            Ok(out) => {
                for m in parse::members(&out) {
                    let port = snap
                        .ports
                        .entry(m.port.clone())
                        .or_insert_with(|| Port::new(&m.port));
                    match m.mode.to_ascii_lowercase().as_str() {
                        "untagged" => port.untagged_vlan = Some(v.id),
                        "tagged" => port.tagged_vlans.push(v.id),
                        _ => {}
                    }
                    // Every VLAN a port belongs to reports the same link state,
                    // so the last row seen is as good as the first.
                    port.link_up = Some(m.link.eq_ignore_ascii_case("up"));
                }
            }
            Err(e) => warn(&format!("show vlan {}", v.id), e, &mut snap.warnings),
        }
    }

    match sw.command("show spanning-tree") {
        Ok(out) => {
            for s in parse::stp_ports(&out) {
                let port = snap
                    .ports
                    .entry(s.port.clone())
                    .or_insert_with(|| Port::new(&s.port));
                port.media = s.media;
                port.stp_state = s.state;
            }
        }
        Err(e) => warn("show spanning-tree", e, &mut snap.warnings),
    }

    match sw.command("show trunks") {
        Ok(out) => merge_trunks(&mut snap, &parse::trunks(&out)),
        Err(e) => warn("show trunks", e, &mut snap.warnings),
    }

    match sw.command("show interfaces") {
        Ok(out) => merge_counters(&mut snap, &parse::counters(&out)),
        Err(e) => warn("show interfaces", e, &mut snap.warnings),
    }

    inherit_trunk_states(&mut snap);

    Ok(snap)
}

/// Give a trunk member the state of its group.
///
/// `show spanning-tree` prints a row for `Trk1`/`Trk3` and none for ports 5, 6,
/// 47 and 48, and `show vlan <id>` likewise lists the group — so a member port
/// has no state of its own to show.  Leaving it blank reads as "this link is
/// dead"; copying the group's state without saying where it came from would
/// claim a measurement that was never taken.  So it is copied *and* tagged,
/// and the page renders it with a tooltip naming the group.
pub fn inherit_trunk_states(snap: &mut Snapshot) {
    let groups: BTreeMap<String, (String, Option<bool>)> = snap
        .ports
        .values()
        .filter(|p| p.id.starts_with("Trk"))
        .map(|p| (p.id.clone(), (p.stp_state.clone(), p.link_up)))
        .collect();
    for p in snap.ports.values_mut() {
        if p.trunk_group.is_empty() || p.id.starts_with("Trk") {
            continue;
        }
        let Some((state, link)) = groups.get(&p.trunk_group) else {
            continue;
        };
        if p.stp_state.is_empty() && !state.is_empty() {
            p.stp_state = state.clone();
            p.stp_state_from = p.trunk_group.clone();
        }
        if p.link_up.is_none() {
            p.link_up = *link;
        }
    }
}

/// Fold `show trunks` into the snapshot.
///
/// This is what puts ports 5, 6, 47 and 48 on the page at all: the VLAN tables
/// list their **group** (`Trk3`, `Trk1`) rather than the members, and
/// `show spanning-tree` does the same.  Reading the rows into entries that
/// already exist — `get_mut` — silently dropped exactly the uplink and NAS
/// ports an operator most wants to look at.
pub fn merge_trunks(snap: &mut Snapshot, rows: &[TrunkMember]) {
    for t in rows {
        let port = snap
            .ports
            .entry(t.port.clone())
            .or_insert_with(|| Port::new(&t.port));
        port.name = t.name.clone();
        port.trunk_group = t.group.clone();
        port.trunk_type = t.group_type.clone();
        if port.media.is_empty() {
            port.media = t.media.clone();
        }
    }
}

/// Fold `show interfaces` (the counters table) into the snapshot.  A port with
/// counters but no other row still deserves to be listed.
pub fn merge_counters(snap: &mut Snapshot, rows: &[(String, Counters)]) {
    for (id, c) in rows {
        snap.ports
            .entry(id.clone())
            .or_insert_with(|| Port::new(id))
            .counters = Some(c.clone());
    }
}

/// Remove ANSI/VT sequences the CLI paints its screen with.
///
/// The switch runs a full-screen CLI over a serial-line emulation: it clears
/// the screen, positions the cursor, and hides it, so almost every line of
/// real output arrives wrapped in escapes.  `ESC [` is consumed through its
/// final byte (0x40–0x7E); any other `ESC x` consumes one character (the CLI's
/// `ESC E` "next line" is the common one).
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// Drop the `-- MORE --…Control-C` marker from each page break, keeping the
/// text that continues on the same line.
pub fn strip_more(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(MORE_MARK) {
        out.push_str(&rest[..i]);
        let after = &rest[i + MORE_MARK.len()..];
        rest = match after.find(MORE_END) {
            Some(j) => &after[j + MORE_END.len()..],
            None => after,
        };
    }
    out.push_str(rest);
    out
}

/// True when the CLI has stopped at a page break and is waiting for a space.
fn waits_for_key(s: &str) -> bool {
    s.trim_end().ends_with(MORE_END) && s.contains(MORE_MARK)
}

/// The CLI prompt, as a line: `switch1>`, `switch1#`, `switch1(config)#`.
fn is_prompt(line: &str) -> bool {
    let t = line.trim();
    let mut chars = t.chars();
    let Some(last) = chars.next_back() else {
        return false;
    };
    if !matches!(last, '>' | '#') {
        return false;
    }
    let name = chars.as_str();
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.()".contains(c))
}

/// True when the response is finished: the last non-empty line is a prompt.
///
/// Blank lines are skipped rather than accepted — the CLI pads its output with
/// them, so treating one as a prompt would end every command early and leave
/// the rest of the table in the socket for the next caller to read.
fn ends_at_prompt(s: &str) -> bool {
    s.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(is_prompt)
        .unwrap_or(false)
}

fn prompt_privilege(s: &str) -> Privilege {
    for l in s.lines().rev() {
        if l.trim().is_empty() {
            continue;
        }
        return if l.trim().ends_with('#') {
            Privilege::Manager
        } else {
            Privilege::Operator
        };
    }
    Privilege::Operator
}

/// Remove the echoed `<prompt> <command>` line the CLI puts first.
/// Remove the echoed command from the response.
///
/// The CLI paints the prompt, the command and the **first line of the response**
/// on one line — `switch1# show version Image stamp: /sw/code/build` — so the
/// line cannot simply be dropped: that used to swallow the refusal on
/// `switch1# show foo Invalid input: foo`, and the caller then believed the
/// command had run.  Cut after the command and keep what follows it.
fn strip_echo(text: &str, cmd: &str) -> String {
    let mut lines = text.lines();
    let Some(first) = lines.next() else {
        return String::new();
    };
    if cmd.is_empty() {
        return text.to_string();
    }
    match first.rfind(cmd) {
        Some(i) => {
            // `cmd` is ASCII, so the byte just past the match is a char
            // boundary even when the prompt before it is not.
            let rest = first[i + cmd.len()..].trim_start();
            let tail = lines.collect::<Vec<_>>().join("\n");
            if rest.is_empty() {
                tail
            } else if tail.is_empty() {
                rest.to_string()
            } else {
                format!("{rest}\n{tail}")
            }
        }
        None => text.to_string(),
    }
}

/// Refuse a login that did not work, in the words the switch used.
fn check_auth(out: &str) -> R<()> {
    if out.contains("User authentication failure") {
        // This is what the firmware prints when RADIUS answers "no" as well as
        // when it does not answer: the same line covers a wrong password and a
        // dead authentik, so say both.
        return Err(
            "the switch refused the login (\"User authentication failure\") — wrong \
             credentials, or its RADIUS server rejected/unanswered them"
                .to_string(),
        );
    }
    if out.contains("Can't reach RADIUS server") {
        return Err(
            "the switch could not reach its RADIUS server — the login was never checked \
             against authentik"
                .to_string(),
        );
    }
    Ok(())
}

/// Refuse a command the CLI did not accept, rather than parsing its error text
/// as if it were a table.
fn check_cli_error(out: &str, cmd: &str) -> R<()> {
    // The exact strings this firmware prints.  `Ambiguous input` is easy to
    // miss and easy to hit — `show port name` answers with it — and a refusal
    // that is not recognised here is reported to the operator as a command that
    // worked, which for a write is the worst possible failure.
    for marker in [
        "Invalid input",
        "Incomplete input",
        "Ambiguous input",
        "Ambiguous command",
        "Unknown command",
    ] {
        if let Some(line) = out.lines().find(|l| l.contains(marker)) {
            return Err(format!(
                "`{cmd}` was refused by the switch: {}",
                line.trim()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CLI paints prompt, command and response on one line, so the refusal
    /// lives on the echoed line.  Stripping the echo must not eat it: a write
    /// whose refusal was discarded was reported to the operator as applied.
    #[test]
    fn a_refusal_on_the_echoed_line_survives_the_echo_strip() {
        let out = "switch1# show foo Invalid input: foo\nswitch1# ";
        let s = strip_echo(out, "show foo");
        assert!(s.contains("Invalid input"), "got {s:?}");
    }

    #[test]
    fn the_first_response_line_is_kept_after_the_command() {
        let out = "switch1# show version Image stamp: /sw/code/build\nboot: Primary\n";
        let s = strip_echo(out, "show version");
        assert!(s.starts_with("Image stamp: /sw/code/build"), "got {s:?}");
        assert!(s.contains("boot: Primary"), "got {s:?}");
    }

    #[test]
    fn a_bare_echo_leaves_just_the_following_lines() {
        let out = "switch1# config\nswitch1(config)# ";
        assert_eq!(strip_echo(out, "config"), "switch1(config)# ");
    }

    #[test]
    fn ansi_sequences_are_removed_but_text_survives() {
        let s = "\u{1b}[2J\u{1b}[?25lswitch1\u{1b}[24;1H show vlan\u{1b}E\n  1  DEFAULT\n";
        assert_eq!(strip_ansi(s), "switch1 show vlan\n  1  DEFAULT\n");
    }

    #[test]
    fn a_trailing_escape_does_not_panic() {
        assert_eq!(strip_ansi("abc\u{1b}"), "abc");
        assert_eq!(strip_ansi("\u{1b}["), "");
    }

    #[test]
    fn more_markers_are_cut_out_of_the_line_they_interrupt() {
        let s = "  16   Forwarding\n-- MORE --, next page: Space, next line: Enter, quit: Control-C  17   Disabled\n";
        assert_eq!(strip_more(s), "  16   Forwarding\n  17   Disabled\n");
    }

    #[test]
    fn page_break_is_detected_only_when_waiting() {
        let waiting = "  1  x\n-- MORE --, next page: Space, next line: Enter, quit: Control-C";
        assert!(waits_for_key(waiting));
        // A marker that has already been paged past is mid-text, not a stop.
        let paged = "-- MORE --, next page: Space, next line: Enter, quit: Control-C  17  y\nswitch1>";
        assert!(!waits_for_key(paged));
    }

    #[test]
    fn prompts() {
        assert!(is_prompt("switch1>"));
        assert!(is_prompt("  switch1>  "));
        assert!(is_prompt("switch1#"));
        assert!(is_prompt("switch1(config)#"));
        assert!(!is_prompt("switch1> show vlan"));
        assert!(!is_prompt("CST Root Port        : 46"));
        assert!(!is_prompt(""));
        assert!(ends_at_prompt("  1  DEFAULT\n\nswitch1>"));
        assert!(!ends_at_prompt("  1  DEFAULT\n"));
    }

    #[test]
    fn privilege_comes_from_the_prompt_character() {
        assert_eq!(prompt_privilege("x\nswitch1>"), Privilege::Operator);
        assert_eq!(prompt_privilege("x\nswitch1#"), Privilege::Manager);
        assert_eq!(
            prompt_privilege("x\nswitch1(config)#"),
            Privilege::Manager
        );
    }

    #[test]
    fn the_echo_line_goes_and_the_response_stays() {
        let text = "switch1> show vlan\n Status and Counters\n  1  DEFAULT\n";
        assert_eq!(
            strip_echo(text, "show vlan"),
            " Status and Counters\n  1  DEFAULT"
        );
        // If there is no echo (a resumed console), nothing is dropped.
        assert_eq!(strip_echo("  1  DEFAULT\n", "show vlan"), "  1  DEFAULT\n");
    }

    #[test]
    fn a_refused_command_is_an_error() {
        let out = "Invalid input: ports\nswitch1>";
        assert!(check_cli_error(out, "show ports")
            .unwrap_err()
            .contains("Invalid input: ports"));
        assert!(check_cli_error("  1  DEFAULT\nswitch1>", "show vlan").is_ok());
    }

    /// The bug this guards: trunk members appear only in `show trunks`, so a
    /// merge that requires the port to exist already leaves the two uplink
    /// ports and both NAS ports off the page entirely.
    #[test]
    fn trunk_rows_create_their_ports() {
        let rows = parse::trunks(TRUNKS);
        let mut snap = Snapshot::default();
        merge_trunks(&mut snap, &rows);
        for id in ["5", "6", "47", "48"] {
            assert!(snap.ports.contains_key(id), "port {id} is missing");
        }
        let p5 = &snap.ports["5"];
        assert_eq!(p5.name, "NAS2 Port 1");
        assert_eq!(p5.trunk_group, "Trk3");
        assert_eq!(p5.media, "100/1000T");
    }

    /// A trunk member that also appears in a VLAN table keeps both facts, and
    /// the media already learned from STP is not overwritten by the trunk's.
    #[test]
    fn merging_a_trunk_keeps_what_is_already_known() {
        let mut snap = Snapshot::default();
        let mut p = Port::new("47");
        p.untagged_vlan = Some(1);
        p.media = "1000X-SFP".into();
        snap.ports.insert("47".into(), p);
        merge_trunks(&mut snap, &parse::trunks(TRUNKS));
        let p47 = &snap.ports["47"];
        assert_eq!(p47.untagged_vlan, Some(1));
        assert_eq!(p47.media, "1000X-SFP");
        assert_eq!(p47.trunk_group, "Trk1");
    }

    /// Counters are keyed by the physical port even when the CLI printed the
    /// compound `5-Trk3` name.
    #[test]
    fn counters_attach_to_the_physical_port() {
        let rows = parse::counters(COUNTERS);
        let mut snap = Snapshot::default();
        merge_counters(&mut snap, &rows);
        assert!(snap.ports.contains_key("5"));
        assert_eq!(snap.ports["5"].counters.as_ref().unwrap().port, "5-Trk3");
        assert!(snap.ports.contains_key("47"));
    }

    /// A trunk member has no STP row of its own, so it takes the group's state
    /// — and records that it did, which is what lets the page say so.
    #[test]
    fn trunk_members_inherit_the_group_state() {
        let mut snap = Snapshot::default();
        let mut trk = Port::new("Trk3");
        trk.stp_state = "Forwarding".into();
        trk.link_up = Some(true);
        snap.ports.insert("Trk3".into(), trk);
        let mut p5 = Port::new("5");
        p5.trunk_group = "Trk3".into();
        snap.ports.insert("5".into(), p5);
        // A plain port with no group stays unknown rather than borrowing.
        snap.ports.insert("9".into(), Port::new("9"));

        inherit_trunk_states(&mut snap);

        let p5 = &snap.ports["5"];
        assert_eq!(p5.stp_state, "Forwarding");
        assert_eq!(p5.stp_state_from, "Trk3");
        assert_eq!(p5.link_up, Some(true));
        assert_eq!(snap.ports["9"].stp_state, "");
        assert_eq!(snap.ports["9"].link_up, None);
        // The group itself is never rewritten.
        assert_eq!(snap.ports["Trk3"].stp_state_from, "");
    }

    /// A member that already has its own state keeps it.
    #[test]
    fn an_inherited_state_never_overrides_a_real_one() {
        let mut snap = Snapshot::default();
        let mut trk = Port::new("Trk1");
        trk.stp_state = "Forwarding".into();
        snap.ports.insert("Trk1".into(), trk);
        let mut p47 = Port::new("47");
        p47.trunk_group = "Trk1".into();
        p47.stp_state = "Learning".into();
        snap.ports.insert("47".into(), p47);
        inherit_trunk_states(&mut snap);
        assert_eq!(snap.ports["47"].stp_state, "Learning");
        assert_eq!(snap.ports["47"].stp_state_from, "");
    }

    const TRUNKS: &str = r#"  Port | Name                             Type      | Group Type
  5    | NAS2 Port 1                      100/1000T | Trk3  LACP
  6    | NAS2 Port 2                      100/1000T | Trk3  LACP
  47   | Switch2 Port 47                  100/1000T | Trk1  LACP
  48   | Switch2 Port 48                  100/1000T | Trk1  LACP
"#;

    const COUNTERS: &str = r#"  1       2,993,652... 1,219,890... 0            0            off   0
  5-Trk3  2,995,318... 183,610,709  0            0            off   0
  47-Trk1 1,307,972... 1,307,972... 0            0            off   0
"#;

    #[test]
    fn auth_failures_are_named() {
        assert!(check_auth("User authentication failure.")
            .unwrap_err()
            .contains("refused the login"));
        assert!(check_auth("Can't reach RADIUS server.")
            .unwrap_err()
            .contains("RADIUS"));
        assert!(check_auth("switch1>").is_ok());
    }
}
