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
//! 3. **It echoes, on the prompt's own row.** The command comes back prefixed
//!    with the prompt and terminated by `ESC E`, and the prompt for the *next*
//!    command is painted on the end of that same row — a command that prints
//!    nothing answers `interface 23 name "x"switch1(config)#`.  [`strip_echo`]
//!    removes the echo; [`is_prompt`] has to read the prompt as a suffix of the
//!    line or no write is ever seen to finish.
//!
//! The 2824 at 10.3.1.4 adds a fourth, and it is the one that bites a client
//! written against the 2510G: its banner ends with `Press any key to continue`
//! and the CLI will not ask for a login name until it gets one (see [`KEY_GATE`]).
//!
//! The session is not thread-safe by design (a Telnet session is one console)
//! and the server keeps exactly one per switch behind a mutex.

use std::collections::BTreeMap;
use std::thread;
use std::time::Duration;

use crate::model::{Counters, Port, Privilege, Snapshot, TrunkMember};
use crate::parse;
use crate::telnet::{ReadFail, Telnet};
use crate::R;

/// A short gap before each configuration command.
///
/// This is **not** what makes a write work.  The note here used to claim the
/// console drops keystrokes while it repaints, which was a guess made while the
/// real bug — a prompt matcher that could not see a prompt painted on the same
/// row as the echoed command — was still hiding (see [`is_prompt`]).  Measured
/// again on 10.3.1.2 on 2026-10-13 with the matcher fixed: `config` and the
/// command after it go back-to-back with no gap at all and both apply.  It
/// stays at 400 ms because this is a 1990s CPU that pages its own output and one
/// write per button click is not worth tuning.
///
/// Reads do not need it: each one is separated by the table it prints.
const SETTLE: Duration = Duration::from_millis(400);

const LOGIN_PROMPT: &str = "Please Enter Login Name";
const PASSWORD_PROMPT: &str = "Please Enter Password";
const MORE_MARK: &str = "-- MORE --";
const MORE_END: &str = "Control-C";
/// The 2824's banner ends with this, and the CLI will not ask for a login name
/// until it gets a keypress. The 2510G (Y.11.44) does not print it at all.
///
/// It matters because the keypress is taken from whatever is already typed: a
/// login name sent before the gate loses its first character, so `agent` arrives
/// as `gent`, RADIUS refuses it, and the switch reports its generic
/// authentication failure. The client has to answer the gate, not assume the
/// login prompt is next.
const KEY_GATE: &str = "Press any key to continue";
/// A 48-port table is ~4 pages at the CLI's 22-row page; this is headroom for
/// `show mac-addresses`-sized output, and a guard against a marker we fail to
/// clear turning into a hang.
const MAX_PAGES: usize = 200;

#[derive(Clone)]
pub struct Config {
    /// What the GUI calls this switch (`switch1`, `switch2`, ...). Every error
    /// names it: with three switches a failure that does not say which one is
    /// not actionable.
    pub name: String,
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
    /// How long a login may take, which has to be longer than `timeout`: the
    /// switch tries RADIUS first and its retry ladder is ~5 s per attempt, so a
    /// refused login — which is every `agent` login here, see [`check_auth`] — can
    /// hold the console for 15-25 s before it falls back to its local account
    /// database and prints the prompt. At 20 s that read times out on `switch3`.
    pub login_timeout: Duration,
}

pub struct Switch {
    tn: Telnet,
    cfg: Config,
    pub privilege: Privilege,
    /// What the login said that is worth showing on the page without failing the
    /// session — the RADIUS noise, see [`check_auth`].
    pub note: Option<String>,
    /// The context the console is in, read off the last prompt: `Some("config")`
    /// at `switch1(config)#`, `Some("eth-23")` at `switch1(eth-23)#`, `None` at
    /// an Exec prompt.  [`Switch::leave_config`] needs it because `exit` at an
    /// Exec prompt does not go up a level, it *closes the session*.
    context: Option<String>,
}

impl Switch {
    /// Connect, log in, and `enable` if manager credentials were supplied.
    pub fn connect(cfg: &Config) -> R<Switch> {
        let mut tn = Telnet::connect(&cfg.host, cfg.port, cfg.timeout)?;
        let intro = strip_ansi(
            &tn.read_until(
                |raw| {
                    let t = strip_ansi(raw);
                    t.contains(LOGIN_PROMPT) || ends_at_prompt(&t) || ends_at_key_gate(&t)
                },
                cfg.timeout,
            )
            .map_err(ReadFail::into_message)?,
        );

        // Answer the 2824's key gate, then read the prompt it was holding up.
        let intro = if ends_at_key_gate(&intro) {
            tn.send_raw(" ")?;
            strip_ansi(
                &tn.read_until(
                    |raw| {
                        let t = strip_ansi(raw);
                        t.contains(LOGIN_PROMPT) || ends_at_prompt(&t)
                    },
                    cfg.timeout,
                )
                .map_err(ReadFail::into_message)?,
            )
        } else {
            intro
        };

        let mut sw = Switch {
            tn,
            cfg: cfg.clone(),
            privilege: Privilege::Operator,
            note: None,
            context: None,
        };
        if intro.contains(LOGIN_PROMPT) {
            sw.login()?;
        } else {
            // A switch configured without a login banner drops the console
            // straight to a prompt; take the privilege from what we see.
            sw.privilege = prompt_privilege(&intro);
        }

        let wants_enable = sw.cfg.enable_login.is_some() || sw.cfg.enable_password.is_some();
        if wants_enable && !sw.privilege.is_manager() {
            // A failed `enable` must not kill the session.  The dashboard is
            // still useful read-only, and letting a stale Manager password turn
            // a write outage into a total outage is the worse failure.  The
            // page already explains why the controls are disabled.
            if let Err(e) = sw.enable() {
                eprintln!(
                    "switch-gui: {} refused `enable`, staying read-only: {e}",
                    sw.cfg.name
                );
            }
        }
        Ok(sw)
    }

    fn login(&mut self) -> R<()> {
        self.tn.send_line(&self.cfg.login)?;
        self.tn
            .read_until(
                |raw| strip_ansi(raw).contains(PASSWORD_PROMPT),
                self.cfg.login_timeout,
            )
            .map_err(ReadFail::into_message)?;
        self.tn.send_line(&self.cfg.password)?;
        let out = strip_ansi(
            &self
                .tn
                .read_until(
                    |raw| ends_at_prompt(&strip_ansi(raw)),
                    self.cfg.login_timeout,
                )
                .map_err(ReadFail::into_message)?,
        );
        self.note_auth(&out)?;
        self.privilege = prompt_privilege(&out);
        self.context = console_context(&out);
        Ok(())
    }

    /// Run the auth check and keep whatever the switch said as a note, so the
    /// page can show the RADIUS noise on a session that is actually working.
    fn note_auth(&mut self, out: &str) -> R<()> {
        if let Some(n) = check_auth(out)? {
            self.note = Some(n);
        }
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
        let out = strip_ansi(
            &self
                .tn
                .read_until(
                    |raw| {
                        let t = strip_ansi(raw);
                        t.contains(LOGIN_PROMPT) || t.contains(PASSWORD_PROMPT) || ends_at_prompt(&t)
                    },
                    self.cfg.login_timeout,
                )
                .map_err(ReadFail::into_message)?,
        );
        if out.contains(LOGIN_PROMPT) {
            self.tn.send_line(&login)?;
            self.tn
                .read_until(
                    |raw| strip_ansi(raw).contains(PASSWORD_PROMPT),
                    self.cfg.login_timeout,
                )
                .map_err(ReadFail::into_message)?;
        }
        self.tn.send_line(&password)?;
        let out = strip_ansi(
            &self
                .tn
                .read_until(
                    |raw| ends_at_prompt(&strip_ansi(raw)),
                    self.cfg.login_timeout,
                )
                .map_err(ReadFail::into_message)?,
        );
        self.note_auth(&out)?;
        self.privilege = prompt_privilege(&out);
        self.context = console_context(&out);
        if !self.privilege.is_manager() {
            return Err(
                "`enable` did not reach the Manager context (prompt is still an operator prompt)"
                    .to_string(),
            );
        }
        Ok(())
    }

    /// Like [`Switch::command`], for output that must never be quoted back.
    ///
    /// `show running-config` is the only command this tool reads whose output
    /// carries secrets — the RADIUS shared secret, the local user password
    /// hashes, the SNMP community — and every failure path in `command` quotes
    /// what it was reading: a timeout appends the tail of the buffer, a refusal
    /// appends the CLI's own line.  Those errors end up in `Snapshot::warnings`,
    /// on the page and in the journal, and this repo has already leaked the
    /// RADIUS secret once by reading this very command into a transcript.
    ///
    /// So the failure is replaced wholesale rather than filtered: a masking rule
    /// is one syntax change away from leaking again — the filter that missed
    /// `radius-server key` is the cautionary tale — and an operator loses nothing
    /// they could have acted on, because the command is a read and the warning
    /// still says which one failed.
    pub fn secret_command(&mut self, cmd: &str) -> R<String> {
        self.command(cmd).map_err(|_| {
            format!(
                "{}: `{cmd}` failed; its output is withheld because it carries the \
                 switch's secrets, which this tool does not put in errors",
                self.cfg.name
            )
        })
    }

    /// Run one CLI command and return its response, paging and cleaning.
    pub fn command(&mut self, cmd: &str) -> R<String> {
        // The echo is the only proof the console took the line.  When it is
        // missing the command is resent once — but see the `Err` arm: a command
        // that *was* echoed is never sent again, because for a configuration
        // command a resend is a second application of the change.
        let mut acc = String::new();
        let mut last_err: Option<String> = None;
        for attempt in 0..2 {
            if attempt > 0 {
                thread::sleep(SETTLE);
            }
            self.tn.send_line(cmd)?;
            match self.read_response(cmd) {
                // Echoed back: the console read it, so its response is real.
                Ok(text) if text.contains(cmd) => {
                    acc = text;
                    last_err = None;
                    break;
                }
                Ok(text) => {
                    acc = text;
                    if attempt == 1 {
                        break;
                    }
                }
                Err(fail) => {
                    // The line reached the console and then the prompt never
                    // came back.  The command may well have been applied, so
                    // resending it is the one thing not to do: it would apply a
                    // configuration change twice, and `no vlan 225 tagged 7`
                    // after a half-read `vlan 225 tagged 7` is not a thing to
                    // leave to chance.  Say what is known and let the caller
                    // re-read the switch, which is the only real confirmation.
                    if fail.text.contains(cmd) {
                        return Err(format!(
                            "{}: the console echoed `{cmd}` and then stopped answering, so \
                             the command reached it and may have been applied — it was not \
                             sent again. Re-read the switch before retrying. ({})",
                            self.cfg.name, fail.message
                        ));
                    }
                    // No echo at all is the swallowed-input case: the line never
                    // reached the parser, so sending it once more is safe.
                    if attempt == 1 {
                        return Err(fail.message);
                    }
                    last_err = Some(fail.message);
                }
            }
        }
        if let Some(e) = last_err {
            return Err(e);
        }
        self.context = console_context(&acc);
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

    /// Read one command's response, paging through `-- MORE --` as needed.
    ///
    /// The failure keeps the partial response (see [`ReadFail`]) so
    /// [`Switch::command`] can tell "never sent" from "sent, then quiet".
    fn read_response(&mut self, cmd: &str) -> Result<String, ReadFail> {
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
                // the console stopped answering — and with three switches it does
                // not say which switch either.
                .map_err(|mut f| {
                    f.message = format!("{}: while sending `{cmd}`: {}", self.cfg.name, f.message);
                    f
                })?;
            let chunk = strip_ansi(&chunk);
            let paged = waits_for_key(&chunk);
            acc.push_str(&strip_more(&chunk));
            if !paged {
                break;
            }
            pages += 1;
            if pages > MAX_PAGES {
                return Err(ReadFail {
                    message: format!(
                        "gave up after {MAX_PAGES} pages of `{cmd}` — the CLI kept printing the \
                         `-- MORE --` marker"
                    ),
                    text: acc,
                });
            }
            self.tn.send_raw(" ").map_err(|e| ReadFail {
                message: e,
                text: acc.clone(),
            })?;
        }
        Ok(acc)
    }

    /// Send a sequence of configuration commands.
    ///
    /// This firmware only accepts configuration **inside config mode**: at the
    /// Manager Exec prompt `port 1 name x` answers `Invalid input: port`.  Both
    /// `config` and `configure terminal` get there (measured on 10.3.1.2: the
    /// prompt goes `switch1#` → `switch1(config)#`), and `exit` comes back out.
    ///
    /// The session always leaves config mode, including when a command is
    /// refused — see [`Switch::leave_config`].
    pub fn apply(&mut self, cmds: &[String]) -> R<()> {
        if !self.privilege.is_manager() {
            return Err("the session is not in the Manager context".to_string());
        }
        self.command("config")?;
        // `config` was accepted, so the console is inside config mode whatever
        // the prompt parse made of the reply — and `leave_config` must not
        // conclude from an empty context that there is nothing to leave.
        if self.context.is_none() {
            self.context = Some("config".to_string());
        }
        let mut failed: Option<String> = None;
        for cmd in cmds {
            thread::sleep(SETTLE);
            if let Err(e) = self.command(cmd) {
                failed = Some(e);
                break;
            }
        }
        let left = self.leave_config();
        match (failed, left) {
            (None, Ok(())) => Ok(()),
            (None, Err(e)) => Err(format!(
                "applied, but the console never came back out of config mode: {e}"
            )),
            (Some(f), Ok(())) => Err(f),
            (Some(f), Err(e)) => Err(format!(
                "{f}; and the console never came back out of config mode: {e}"
            )),
        }
    }

    /// Come back to an Exec prompt, however deep the console went.
    ///
    /// One `exit` is enough for the one-shot `interface <n> …` form
    /// `commands.rs` emits, but not if the console went a level deeper —
    /// `interface 5` answers `switch1(eth-5)#`, and one `exit` from there lands
    /// on `switch1(config)#`.  A session left inside config mode makes the next
    /// *read* fail in a way that looks like a parser bug, so keep exiting while
    /// the prompt still names a context.
    ///
    /// The prompt is consulted before each `exit` rather than a fixed number
    /// being sent, because at an Exec prompt `exit` does not go up a level — its
    /// own help says it will "terminate current console/telnet session".
    fn leave_config(&mut self) -> R<()> {
        let mut dropped = None;
        for _ in 0..4 {
            if self.context.is_none() {
                return Ok(());
            }
            match self.command("exit") {
                Ok(_) => {}
                Err(e) => {
                    dropped = Some(e);
                    break;
                }
            }
        }
        Err(match dropped {
            Some(e) => format!("the console was dropped ({e})"),
            None => "it is still inside a configuration context after four `exit`s".to_string(),
        })
    }
}

/// Read the whole configuration into a [`Snapshot`].
///
/// One command per table, and a failure of any single one is recorded as a
/// warning rather than aborting the refresh: `show trunks` not existing on
/// some firmware should cost the trunk column, not the whole page.
pub fn read_snapshot(sw: &mut Switch) -> R<Snapshot> {
    // Which switch this snapshot came from, in the initializer so the rest of the
    // build can still start from `Default::default()`.
    let mut snap = Snapshot {
        switch: sw.cfg.name.clone(),
        ..Default::default()
    };
    // The login's RADIUS noise, if there was any: not a failure, but the page
    // should show it rather than imply the login went through authentik.
    if let Some(w) = &sw.note {
        snap.warnings.push(w.clone());
    }
    // Copied out before the closure: capturing `sw.cfg.name` would hold an
    // immutable borrow of the session across every `sw.command(...)` call.
    let name = sw.cfg.name.clone();
    let warn = |w: &str, e: String, warnings: &mut Vec<String>| {
        eprintln!("switch-gui: {name} {w} failed: {e}");
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

    // Port names.  `show vlan <id>` has a Name column and leaves it blank for
    // every dynamically-learned member, which on switch1 is every port, and
    // `show port name` does not exist on this firmware (`Ambiguous input: port`)
    // — the running config is the only source there is.  It is also the only
    // read here that carries secrets, hence `secret_command`.
    //
    // One caveat worth knowing before changing the prompt matcher: this is the
    // one response whose body could in principle end a line the way a prompt
    // does, since `is_prompt` reads a prompt as a line's suffix.  No line of the
    // real config does (checked against the capture from all three switches);
    // a port or VLAN name that ended in `word>` would.
    match sw.secret_command("show running-config") {
        Ok(out) => merge_names(&mut snap, &parse::port_names(&out)),
        Err(e) => warn("show running-config", e, &mut snap.warnings),
    }

    inherit_trunk_states(&mut snap);

    Ok(snap)
}

/// Fold the running config's port names into the snapshot.
///
/// Only ports the snapshot already knows about are named: a stray
/// `interface` block is not evidence a port belongs on the page — the
/// membership and spanning-tree tables decide that, and inventing a row from a
/// config fragment is how a snapshot starts describing a switch it never read.
fn merge_names(snap: &mut Snapshot, names: &BTreeMap<String, String>) {
    for (id, name) in names {
        if let Some(p) = snap.ports.get_mut(id) {
            p.name = name.clone();
        }
    }
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
///
/// A prompt is a **suffix** of the line, not the whole line.  The CLI paints the
/// echoed command and the prompt it prints next on the same row, so a command
/// that prints nothing answers:
///
/// ```text
/// interface 23 name "TEST WRITE"switch1(config)#
/// ```
///
/// Matching only a line that is nothing but a prompt meant that form was never
/// recognised: the read waited out its 20 s, the retry sent the command again,
/// and the operator was told the console had never echoed a configuration
/// command — when in fact it had echoed it, and applied it.  Measured on
/// 10.3.1.2 on 2026-10-13, where `interface 23 name "TEST WRITE"` turned up in
/// `show running-config` after a write the app had reported as a timeout.
///
/// The name is deliberately not anchored at its start: after `exit` the row is
/// `exitswitch1#`, the prompt running straight into the echo.  The cost of that
/// is that a response line ending in `word#` would read as a prompt; no table
/// this tool parses has one (checked against every fixture in `parse.rs`).
fn is_prompt(line: &str) -> bool {
    prompt_tail(line).is_some()
}

/// Split the prompt at the end of `line` into `(name, context, is_manager)`:
/// `switch1(vlan-225)#` is `("switch1", Some("vlan-225"), true)`.
fn prompt_tail(line: &str) -> Option<(&str, Option<&str>, bool)> {
    let t = line.trim_end();
    let manager = match *t.as_bytes().last()? {
        b'#' => true,
        b'>' => false,
        _ => return None,
    };
    let mut body = &t[..t.len() - 1];
    let mut context = None;
    if body.ends_with(')') {
        let open = body.rfind('(')?;
        let inner = &body[open + 1..body.len() - 1];
        if inner.is_empty() || !inner.chars().all(is_name_char) {
            return None;
        }
        context = Some(inner);
        body = &body[..open];
    }
    // The trailing run of name characters is the switch's own name.  They are
    // all ASCII, so walking bytes cannot split a code point.
    let bytes = body.as_bytes();
    let mut i = bytes.len();
    while i > 0 && is_name_char(bytes[i - 1] as char) {
        i -= 1;
    }
    if i == bytes.len() {
        return None;
    }
    Some((&body[i..], context, manager))
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'
}

/// Which context the console is sitting in, read off the last prompt in `text`:
/// `Some("config")` for `switch1(config)#`, `Some("eth-23")` for
/// `switch1(eth-23)#`, `None` at an Exec prompt or when no prompt was seen.
fn console_context(text: &str) -> Option<String> {
    let line = text.lines().rev().find(|l| !l.trim().is_empty())?;
    prompt_tail(line)
        .and_then(|(_, ctx, _)| ctx)
        .map(str::to_string)
}

/// True when the CLI is sitting on the 2824's `Press any key to continue` gate:
/// the banner is finished and the login prompt is behind it. Blank lines are
/// skipped for the same reason `ends_at_prompt` skips them.
fn ends_at_key_gate(s: &str) -> bool {
    s.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim_end().ends_with(KEY_GATE))
        .unwrap_or(false)
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
    if let Some(l) = s.lines().rev().find(|l| !l.trim().is_empty()) {
        if let Some((_, _, manager)) = prompt_tail(l) {
            return if manager {
                Privilege::Manager
            } else {
                Privilege::Operator
            };
        }
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
fn check_auth(out: &str) -> R<Option<String>> {
    if out.contains("User authentication failure") {
        return Err(
            "the switch refused the login (\"User authentication failure\") — wrong \
             credentials, or its RADIUS server rejected/unanswered them"
                .to_string(),
        );
    }
    if out.contains("Can't reach RADIUS server") {
        // It is this firmware's generic "authentication failed" text, not a
        // network timeout: it arrives in under two seconds. The switch then falls
        // back to its own local account database, so when a prompt follows the
        // session is usable and this is a note, not a failure — which is what
        // happens on every `agent` login here (the switch's local password is not
        // authentik's). Refusing the connect on it would make the GUI unusable on
        // a switch that is answering normally; that is what happened on `switch3`
        // before this was a note.
        if ends_at_prompt(out) {
            return Ok(Some(
                "switch says: Can't reach RADIUS server. — its generic 'authentication \
                 failed' text; it fell back to its local account database and let this \
                 session in"
                    .to_string(),
            ));
        }
        return Err(
            "the switch could not reach its RADIUS server — the login was never checked \
             against authentik"
                .to_string(),
        );
    }
    Ok(None)
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

    /// The 2824's banner ends on the key gate, and the login prompt only appears
    /// after a key arrives — on the same line as the gate, which is why the
    /// capture below reads as one run-on line.
    #[test]
    fn the_key_gate_is_a_gate_only_when_it_is_the_last_thing_printed() {
        let gated = "Please register your products now at:  www.ProCurve.com\n\n\nPress any key to continue";
        assert!(ends_at_key_gate(gated));
        // Once the key arrives the prompt is what the client is waiting for.
        let past = "Press any key to continuePlease Enter Login Name: agent";
        assert!(!ends_at_key_gate(past));
        assert!(!ends_at_key_gate("  1  DEFAULT\nswitch1>"));
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

    /// The write path, and the bug that made it look like the console never
    /// echoed a configuration command.  Captured from 10.3.1.2 on 2026-10-13:
    /// the CLI paints the echo and the next prompt on one row, so the prompt is
    /// a suffix of the line.  Reading it as the whole line meant the read timed
    /// out, the retry sent the command a second time, and the operator was told
    /// the switch had never seen it — while `show running-config` showed the
    /// name on port 23, applied twice over.
    #[test]
    fn a_prompt_painted_on_the_end_of_the_echo_line_ends_the_response() {
        let echoed = "interface 23 name \"TEST WRITE\"switch1(config)# ";
        assert!(
            is_prompt(echoed),
            "the CLI answers a silent command like this"
        );
        assert!(ends_at_prompt(echoed));
        // `exit` runs straight into the prompt it prints.
        assert!(is_prompt("exitswitch1#"));
        assert!(is_prompt("interface 23switch1(eth-23)#"));
        // And a table row still is not a prompt.
        assert!(!is_prompt("  23               Untagged Learn        Down      "));
    }

    /// The context in the prompt is what tells `leave_config` whether there is
    /// a level to leave — and `exit` at an Exec prompt closes the session, so
    /// it must not be sent on a guess.
    #[test]
    fn the_context_comes_from_the_prompt() {
        assert_eq!(console_context("configswitch1(config)# ").as_deref(), Some("config"));
        assert_eq!(
            console_context("interface 23switch1(eth-23)#").as_deref(),
            Some("eth-23")
        );
        assert_eq!(console_context("exitswitch1# ").as_deref(), None);
        assert_eq!(console_context("show vlan\n  1  DEFAULT\nswitch1>").as_deref(), None);
        // No prompt at all: nothing to go by.
        assert_eq!(console_context("  1  DEFAULT\n"), None);
    }

    /// A prompt is a suffix, so anything ending `word#` reads as one.  That is
    /// the accepted cost, and it is only safe because no table this tool parses
    /// has a row ending that way — checked against every fixture in `parse.rs`,
    /// and pinned here for the two tables that come closest.
    #[test]
    fn no_response_row_ends_like_a_prompt() {
        for line in crate::parse::tests::corpus().lines() {
            assert!(!is_prompt(line), "{:?} would end a response early", line);
        }
    }

    #[test]
    fn prompt_tail_splits_name_context_and_privilege() {
        assert_eq!(prompt_tail("switch1(vlan-225)#"), Some(("switch1", Some("vlan-225"), true)));
        assert_eq!(prompt_tail("switch3>"), Some(("switch3", None, false)));
        assert_eq!(
            prompt_tail("interface 23 name \"x\"switch1(config)# "),
            Some(("switch1", Some("config"), true))
        );
        assert_eq!(prompt_tail("  1  DEFAULT"), None);
        assert_eq!(prompt_tail("#"), None);
        assert_eq!(prompt_tail("()#"), None);
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
        // No prompt behind it: the login did not happen.
        assert!(check_auth("Can't reach RADIUS server.")
            .unwrap_err()
            .contains("RADIUS"));
        assert!(check_auth("switch1>").is_ok());
    }

    /// The case that happens on every `agent` login: RADIUS refuses, the switch
    /// prints its generic authentication-failed text, then its local account
    /// database lets the session in. The session works, so this is a note the page
    /// shows, not a dead connect.
    #[test]
    fn radius_noise_with_a_prompt_behind_it_is_a_note_not_a_failure() {
        let out = "Can't reach RADIUS server.\nCan't reach RADIUS server.\nswitch3>";
        let note = check_auth(out).unwrap().expect("a note");
        assert!(note.contains("Can't reach RADIUS server."), "quotes what it said");
        assert!(note.contains("local account database"));
    }
}
