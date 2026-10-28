//! The deciding half of `ups-watchdog`, with no side effects, so that the part
//! which powers off a network can be argued about in a test rather than in a
//! journal.
//!
//! Everything here answers one of two questions:
//!
//! * **Is this an outage?** ([`Status`], [`decide`]) — from one line of NUT's
//!   `ups.status`, and from *why we were woken up*.
//! * **Who goes, in what order, and with what argv?** ([`Target`],
//!   [`plan`], [`ssh_command`]) — from the list `machines/gw/ups.nix` hands us.
//!
//! The timer is deliberately **not** here.  "Discharging, wait 300 s, and if it
//! is still discharging then act" is NUT's own `upssched` idiom
//! (`AT ONBATT * START-TIMER … 300` / `AT ONLINE * CANCEL-TIMER …`), and it is
//! the half of this that has to keep working while nothing else does — a second
//! implementation of it would be a second timer to disagree with.  What is here
//! is the check *at the moment of action* (a lost `ONLINE` notification must not
//! power off a building with mains) and the part NUT has no idea about: the
//! other machines.

use std::fmt;

/// Whether the UPS thinks it is being fed by mains right now.
///
/// `Unknown` is a real third state, not a failure: `upsc` cannot reach `upsd`,
/// or the driver has lost the UPS, and the two directions of wrong have very
/// different costs.  See [`decide`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mains {
    /// `OL` — utility power present, battery not in use.
    On,
    /// `OB` — on battery: the outage.
    Off,
    /// Nothing readable, or a status line with neither flag in it.
    Unknown,
}

/// One parsed `ups.status` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub mains: Mains,
    /// `LB` — the battery is nearly empty; the remaining minutes are not the
    /// ones the grace period was sized for.
    pub low_battery: bool,
    /// `OFF` — the UPS has already taken its outlets away from us.
    pub turned_off: bool,
    /// The line as it arrived, for the log.
    pub raw: String,
}

impl Status {
    /// Parse a NUT status line: `OB LB`, `OL CHRG`, `ups.status: OL`, `--`,
    /// `""`.
    ///
    /// The flag names are NUT's own (see `upsserv.h`: `OL`, `OB`, `LB`, `RB`,
    /// `OFF`, `OV`, `CHRG`, `DISCHRG`, `BYPASS`, `ESC`, `TESTN`, `TESTW`), and
    /// the two that matter are the mutually exclusive `OL`/`OB` pair — which is
    /// why "neither present" is [`Mains::Unknown`] rather than a guess at `OL`.
    /// NUT also spells "cannot read the UPS" as a bare `--` or `NOCOMM`, both of
    /// which land in `Unknown` too.
    pub fn parse(line: &str) -> Status {
        let trimmed = line.trim();
        // `upsc <ups> ups.status` printed the value alone in NUT 2.8 and
        // `ups.status: <value>` in 2.7; accept either so an upgrade cannot turn
        // "we are on mains" into "we could not tell".  Only that one key is
        // stripped, so a value that happens to contain a colon is not eaten.
        let value = match trimmed.strip_prefix("ups.status:") {
            Some(rest) => rest.trim(),
            None => trimmed,
        };
        let flags: Vec<String> = value
            .split_whitespace()
            .map(|f| f.to_ascii_uppercase())
            .collect();
        let has = |f: &str| flags.iter().any(|x| x == f);

        let mains = if has("OB") {
            Mains::Off
        } else if has("OL") {
            Mains::On
        } else {
            Mains::Unknown
        };

        Status {
            mains,
            low_battery: has("LB"),
            turned_off: has("OFF"),
            raw: trimmed.to_string(),
        }
    }

    /// `true` when the battery is what is running the box.
    ///
    /// Not used by `decide`, and that is deliberate: "is it an outage?" and "may I
    /// stop waiting?" are different questions, and the second one depends on the
    /// reason as well (`plan::decide`).  It is here because every caller that logs
    /// or reports wants the first one, and `mains == Mains::Off` spelled at four
    /// call sites is how those two questions get confused.
    pub fn discharging(&self) -> bool {
        self.mains == Mains::Off
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mains = match self.mains {
            Mains::On => "mains",
            Mains::Off => "on battery",
            Mains::Unknown => "unknown",
        };
        write!(
            f,
            "{mains} (status {})",
            if self.raw.is_empty() {
                "<empty>"
            } else {
                &self.raw
            }
        )?;
        if self.low_battery {
            write!(f, ", LOW BATTERY")?;
        }
        if self.turned_off {
            write!(f, ", UPS OUTPUT OFF")?;
        }
        Ok(())
    }
}

/// Why this process was started, which decides what a contradictory UPS reading
/// means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The `upssched` timer expired: the outage has already lasted the grace
    /// period, and NUT's notifications are the only evidence.
    OutageGraceExpired,
    /// `LOWBATT`: there are minutes, not the grace period.
    LowBattery,
    /// `upsmon` reached its own power-off decision and ran its `SHUTDOWNCMD`.
    UpsmonShutdown,
    /// A person started it (`just ups-drill`, or by hand).
    Manual,
    /// Nothing was written and nothing was passed.
    Unknown,
}

impl Reason {
    /// Read the flag names `machines/gw/ups.nix` writes.  Unrecognised text is
    /// [`Reason::Unknown`] rather than an error: a reason that cannot be parsed
    /// must not be the thing that stops the shutdown.
    pub fn parse(text: &str) -> Reason {
        match text.trim().to_ascii_lowercase().as_str() {
            "outage-grace-expired" | "ups-outage" => Reason::OutageGraceExpired,
            "low-battery" | "ups-low-battery" => Reason::LowBattery,
            "upsmon-shutdown" => Reason::UpsmonShutdown,
            "manual" | "manual-drill" | "ups-drill" => Reason::Manual,
            _ => Reason::Unknown,
        }
    }

    /// `true` when waiting any longer costs more than it can save.
    ///
    /// `UpsmonShutdown` and `LowBattery` are in this set because `upsmon` only
    /// reaches them when the battery is already gone or the master has said
    /// `FSD`: at that point a `OL` reading is far more likely to be a lying
    /// driver than a restored supply.
    pub fn is_already_late(&self) -> bool {
        matches!(
            self,
            Reason::LowBattery | Reason::UpsmonShutdown | Reason::Manual
        )
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Reason::OutageGraceExpired => "outage-grace-expired",
            Reason::LowBattery => "low-battery",
            Reason::UpsmonShutdown => "upsmon-shutdown",
            Reason::Manual => "manual",
            Reason::Unknown => "unknown",
        }
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Run the sequence.  `late` selects the short drain (`upsmon` has already
    /// decided, so the remaining seconds belong to the shutdown, not to waiting).
    Act { late: bool },
    /// Do nothing at all: no ssh, no poweroff.  The string is the reason, and it
    /// is written to be read in a journal at 03:00.
    Abort(&'static str),
}

/// The one place that reads "is it still an outage?" against "why were we
/// woken up?".
///
/// The asymmetry is the whole point.  `status` is a *re-check* through a route
/// (`upsc` → `upsd`) that can fail for reasons that have nothing to do with
/// power, so:
///
/// * `OL` **during the grace timer** → abort.  The timer and a live mains
///   reading cannot both be true, and the recoverable mistake is the one that
///   leaves the network up.
/// * `OL` **when `upsmon` itself has decided** → act anyway, because at that
///   point `upsmon` has the battery telemetry and we do not.
/// * no reading at all → act on the timer's authority.  `upssched` is
///   instructed to cancel the very same timer on `COMMBAD`/`TIMEDOUT`, so a
///   driver that went away mid-outage is already handled, and a broken `upsc`
///   must not be the reason a router dies from an empty battery instead of
///   shutting down cleanly.
pub fn decide(status: Option<&Status>, reason: Reason) -> Decision {
    let late = reason.is_already_late();
    match status {
        None => Decision::Act { late },
        Some(s) if s.mains == Mains::On && !late => Decision::Abort(
            "the UPS reports OL (mains present); a grace-period timer and a live mains reading \
             cannot both be true, and the recoverable mistake is to leave the network running — \
             nothing was shut down. Check `journalctl -u upsmon -u upsd` for a lost ONLINE \
             notification",
        ),
        // Includes `discharging()`, `Unknown`, and every late reason: see the two
        // comments above on what a re-check can and cannot say.
        Some(_) => Decision::Act { late },
    }
}

/// Compose a broadcast (a `wall` on every server, and on this box's own
/// terminals) out of a headline, body lines and whatever the UPS last said.
///
/// Pure, and shaped the way it is for one reason: **a message nobody can act on
/// is worse than no message.** `AGENTS.md`'s "a UI that refuses an edit must say
/// so" applies to a terminal bell too — so the outage notice has to name the
/// countdown ("300 s"), the cancel condition ("mains return"), and where to look
/// (`journalctl -u ups-fleet-shutdown`), because `wall` is the only warning a
/// box has before something ssh's in and powers it off. The shutdown notice goes
/// out *before* the hops for the same reason: it is an offer to intervene, not an
/// apology.
///
/// No timestamp: the caller is a shell script and `date -u` costs it nothing,
/// while a clock in Rust would mean either `chrono` (this crate has no
/// dependencies) or a hand-rolled civil-calendar converter in the middle of a
/// power cut.
pub fn compose_broadcast(headline: &str, body: &[String], status: Option<&Status>) -> String {
    let mut out = String::new();
    out.push_str("=== gw UPS: ");
    out.push_str(headline);
    out.push_str(" ===\n");
    for line in body {
        out.push_str(line);
        out.push('\n');
    }
    if let Some(s) = status {
        out.push_str("ups: ");
        out.push_str(&s.to_string());
        out.push('\n');
    }
    out
}

/// What a [`Decision`] means once you know whether this is a drill.
///
/// The distinction is not cosmetic and it was found by deploying: a drill is a
/// review of the *plan*, and it happens while the mains are fine — so a `--dry-run`
/// that obeys the abort arm exits 21 every time it is started and shows nothing.
/// The dry unit in `machines/gw/ups.nix` is the thing a person runs to decide
/// whether to trust the real one, which makes "it always fails" a bug rather than
/// a curiosity.  A real run still aborts, and still aborts for the same reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proceed {
    /// Go on.  `note` is what the journal should say about the decision.
    Yes { note: Option<String> },
    /// Stop here: nothing has been powered off and nothing will be.
    No { why: &'static str },
}

pub fn proceed(decision: Decision, dry_run: bool) -> Proceed {
    match decision {
        Decision::Abort(why) if dry_run => Proceed::Yes {
            note: Some(format!(
                "[dry-run] would ABORT and shut nothing down — {why}. continuing, to show the plan it stopped at"
            )),
        },
        Decision::Abort(why) => Proceed::No { why },
        Decision::Act { late } if late => Proceed::Yes {
            note: Some("this reason is already late — short notice window, no further waits".into()),
        },
        Decision::Act { .. } => Proceed::Yes { note: None },
    }
}

/// One machine to power off over ssh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    pub address: String,
}

impl Target {
    /// Parse a `name=address` pair.
    ///
    /// The name is carried for the log and must not be empty: an outage journal
    /// that says `10.3.1.20: done` is a journal that cannot be matched to a
    /// machine at 03:00.
    pub fn parse(spec: &str) -> Result<Target, String> {
        let (name, address) = spec.split_once('=').ok_or_else(|| {
            format!("`{spec}` is not NAME=ADDRESS — a target needs both, in that order")
        })?;
        let name = name.trim();
        let address = address.trim();
        if name.is_empty() {
            return Err(format!("`{spec}` has no name"));
        }
        if address.is_empty() {
            return Err(format!("`{spec}` has no address"));
        }
        if address.contains('=') || address.split_whitespace().count() != 1 {
            return Err(format!("`{spec}` has something after the address"));
        }
        if address.eq_ignore_ascii_case("localhost") {
            return Err(format!(
                "`{spec}` points at `localhost` — this program powers off *this* box by itself, \
                 last, and never over ssh"
            ));
        }
        Ok(Target {
            name: name.to_string(),
            address: address.to_string(),
        })
    }
}

/// The ssh options every hop shares.
#[derive(Debug, Clone)]
pub struct SshOpts {
    pub bin: String,
    pub user: String,
    pub identity_file: String,
    pub remote_command: String,
}

/// The argv for one hop.
///
/// * `BatchMode=yes` — the process has no tty and must never sit waiting for a
///   password that nobody will type during a power cut.
/// * `IdentitiesOnly=yes` — gw holds one key but sshd refuses a client that
///   offers several identities before the right one (`Too many authentication
///   failures`, and `just deploy` carries the same note for the same reason).
/// * **no `KnownHostsFile`** — OpenSSH 10.5 dropped it as a `-o` option, and
///   `programs.ssh.knownHosts` already writes `/etc/ssh/ssh_known_hosts` from
///   `common/ssh-known-hosts.nix`, which every ssh reads through
///   `GlobalKnownHostsFile`.  So the host keys stay pinned and the pin stays in
///   one file.
/// * `ConnectTimeout` + `ServerAlive*` — the deadline is what keeps one dead box
///   from spending the whole battery.
pub fn ssh_command(target: &Target, opts: &SshOpts, connect_timeout: u64) -> Vec<String> {
    vec![
        opts.bin.clone(),
        "-i".to_string(),
        opts.identity_file.clone(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "IdentitiesOnly=yes".to_string(),
        "-o".to_string(),
        format!("ConnectTimeout={connect_timeout}"),
        "-o".to_string(),
        "ServerAliveInterval=5".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=2".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=yes".to_string(),
        "-o".to_string(),
        "LogLevel=ERROR".to_string(),
        format!("{}@{}", opts.user, target.address),
        opts.remote_command.clone(),
    ]
}

/// Order, check and refuse the shutdown list.
///
/// Errors describe every problem at once rather than the first: this list is
/// generated from `common/hosts.nix` by `machines/gw/ups.nix`, so anything wrong
/// here is a table or an edit, and a single-finding message would send somebody
/// back to the table three times.
///
/// A target that answers with one of `self_addresses` is a **hard error**, and
/// that is the guard rather than a nicety: `gw` holds this repo, the hop key and
/// an sshd that accepts it, so a hop to 10.3.1.1 from gw *succeeds* — and on the
/// box that powers off last, the first thing that goes down would be the
/// machine still holding the lock on the rest of the sequence.
pub fn plan(specs: &[String], self_addresses: &[String]) -> Result<Vec<Target>, Vec<String>> {
    let mut findings: Vec<String> = Vec::new();
    let mut targets: Vec<Target> = Vec::new();

    for spec in specs {
        match Target::parse(spec) {
            Err(why) => findings.push(format!("--host `{spec}`: {why}")),
            Ok(target) => {
                if let Some(dup) = targets.iter().find(|t| t.address == target.address) {
                    findings.push(format!(
                        "--host `{spec}`: {addr} is already `{dup}` — one address, one machine",
                        addr = target.address,
                        dup = dup.name
                    ));
                }
                if let Some(me) = self_addresses
                    .iter()
                    .find(|me| me.eq_ignore_ascii_case(&target.address))
                {
                    findings.push(format!(
                        "--host `{spec}`: {me} is this box (the router). It is powered off \
                         locally, last, and never over ssh"
                    ));
                }
                if !targets.iter().any(|t| t.address == target.address) {
                    targets.push(target);
                }
            }
        }
    }

    if specs.is_empty() {
        findings.push(
            "no --host was given: nothing will be told to power off, and this box will power \
             off alone while the rest of the network runs on to a dead switch"
                .to_string(),
        );
    }

    if findings.is_empty() {
        Ok(targets)
    } else {
        Err(findings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|s| s.to_string()).collect()
    }

    fn status(line: &str) -> Status {
        Status::parse(line)
    }

    #[test]
    fn ob_is_an_outage_and_ol_is_not() {
        assert_eq!(status("OB").mains, Mains::Off);
        assert!(status("OB").discharging());
        assert_eq!(status("OL CHRG").mains, Mains::On);
        assert!(!status("OL CHRG").discharging());
        assert_eq!(status("OB CHRG").mains, Mains::Off);
    }

    #[test]
    fn low_battery_and_output_off_are_read_out_of_the_same_line() {
        let s = status("OB LB");
        assert!(s.discharging());
        assert!(s.low_battery);
        assert!(!s.turned_off);
        let s = status("OL LB OFF");
        assert_eq!(s.mains, Mains::On);
        assert!(s.turned_off);
    }

    #[test]
    fn a_status_that_is_not_ours_is_unknown_not_mains() {
        // NUT's own "I cannot see the UPS", and a driver that has not published
        // anything yet.  Neither may be read as "mains is fine" *or* "outage".
        assert_eq!(status("--").mains, Mains::Unknown);
        assert_eq!(status("").mains, Mains::Unknown);
        assert_eq!(status("NOCOMM").mains, Mains::Unknown);
        assert_eq!(status("   ").mains, Mains::Unknown);
    }

    #[test]
    fn both_upsc_output_shapes_are_accepted() {
        assert_eq!(status("ups.status: OB").mains, Mains::Off);
        assert_eq!(status("ups.status: OL").mains, Mains::On);
        // A *variable* named with a colon in its value must not be mistaken for
        // the 2.7 form; only a leading `ups.status` is stripped.
        assert_eq!(status("UPS: on battery").raw, "UPS: on battery");
    }

    #[test]
    fn the_grace_timer_defers_to_a_mains_reading() {
        let s = status("OL CHRG");
        assert!(matches!(
            decide(Some(&s), Reason::OutageGraceExpired),
            Decision::Abort(_)
        ));
    }

    #[test]
    fn a_late_reason_acts_through_a_mains_reading() {
        let s = status("OL CHRG");
        for reason in [Reason::LowBattery, Reason::UpsmonShutdown, Reason::Manual] {
            assert_eq!(
                decide(Some(&s), reason),
                Decision::Act { late: true },
                "{reason}"
            );
        }
    }

    #[test]
    fn no_reading_acts_on_the_timers_authority() {
        // upssched is told to cancel the timer on COMMBAD/TIMEDOUT, so this is
        // "the re-check is broken", not "the UPS is gone".
        assert_eq!(
            decide(None, Reason::OutageGraceExpired),
            Decision::Act { late: false }
        );
        assert_eq!(
            decide(Some(&status("--")), Reason::OutageGraceExpired),
            Decision::Act { late: false }
        );
    }

    #[test]
    fn an_outage_reading_acts_however_it_arrived() {
        let s = status("OB LB");
        assert_eq!(
            decide(Some(&s), Reason::OutageGraceExpired),
            Decision::Act { late: false }
        );
        assert_eq!(
            decide(Some(&s), Reason::LowBattery),
            Decision::Act { late: true }
        );
    }

    #[test]
    fn reasons_are_parsed_from_what_the_module_writes() {
        assert_eq!(
            Reason::parse("outage-grace-expired"),
            Reason::OutageGraceExpired
        );
        assert_eq!(Reason::parse("LOW-BATTERY\n"), Reason::LowBattery);
        assert_eq!(Reason::parse("upsmon-shutdown"), Reason::UpsmonShutdown);
        assert_eq!(Reason::parse("manual-drill"), Reason::Manual);
        assert_eq!(Reason::parse(""), Reason::Unknown);
        assert_eq!(Reason::parse("nonsense"), Reason::Unknown);
    }

    #[test]
    fn a_target_needs_a_name_and_an_address() {
        assert!(Target::parse("services1").is_err());
        assert!(Target::parse("services1=").is_err());
        assert!(Target::parse("=10.3.1.20").is_err());
        assert!(Target::parse("a=10.3.1.20 extra").is_err());
        assert!(Target::parse("a=localhost").is_err());
        let t = Target::parse("services1=10.3.1.20").expect("well formed");
        assert_eq!(t.name, "services1");
        assert_eq!(t.address, "10.3.1.20");
    }

    #[test]
    fn the_plan_keeps_the_given_order() {
        // Order is the point of the list: postgres before the build box.
        let specs = [
            "services1=10.3.1.20",
            "aibox=10.3.1.32",
            "services2=10.3.1.21",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
        let got = plan(&specs, &[]).expect("fine");
        assert_eq!(
            got.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["services1", "aibox", "services2"]
        );
    }

    #[test]
    fn the_router_is_refused_by_address_not_by_name() {
        // `gw` is `router` in common/hosts.nix, and the list could name it either
        // way; what is refused is the address it answers on.
        let specs = ["gw=10.3.1.1", "services1=10.3.1.20"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let err = plan(&specs, &["10.3.1.1".to_string()]).expect_err("must refuse");
        assert!(err.iter().any(|f| f.contains("this box")), "{err:?}");
    }

    #[test]
    fn one_address_twice_is_one_machine_claiming_two_names() {
        let specs = ["services1=10.3.1.20", "old-services1=10.3.1.20"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let err = plan(&specs, &[]).expect_err("must refuse");
        assert!(err.iter().any(|f| f.contains("already")), "{err:?}");
    }

    #[test]
    fn an_empty_list_is_a_finding_not_a_silence() {
        let err = plan(&[], &[]).expect_err("must refuse");
        assert!(err.iter().any(|f| f.contains("no --host")), "{err:?}");
    }

    #[test]
    fn every_bad_entry_is_reported_in_one_pass() {
        let specs = ["broken", "x=", "y=10.3.1.1"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let err = plan(&specs, &["10.3.1.1".to_string()]).expect_err("must refuse");
        assert_eq!(err.len(), 3, "{err:?}");
    }

    #[test]
    fn the_ssh_hop_is_batch_pinned_and_has_no_known_hosts_file() {
        let target = Target::parse("services1=10.3.1.20").expect("well formed");
        let opts = SshOpts {
            bin: "ssh".to_string(),
            user: "leigh-admin".to_string(),
            identity_file: "/home/leigh-admin/.ssh/agent-hop-key".to_string(),
            remote_command: "sudo -n systemctl poweroff --no-block".to_string(),
        };
        let argv = ssh_command(&target, &opts, 5);
        let joined = argv.join(" ");
        assert!(joined.contains("BatchMode=yes"), "{joined}");
        assert!(joined.contains("IdentitiesOnly=yes"), "{joined}");
        assert!(joined.contains("ConnectTimeout=5"), "{joined}");
        assert!(joined.contains("StrictHostKeyChecking=yes"), "{joined}");
        assert!(
            joined.contains("-i /home/leigh-admin/.ssh/agent-hop-key"),
            "{joined}"
        );
        assert!(joined.contains("leigh-admin@10.3.1.20"), "{joined}");
        // OpenSSH 10.5 rejects `-o KnownHostsFile=` outright, and the pin lives in
        // /etc/ssh/ssh_known_hosts (common/ssh-known-hosts.nix).
        assert!(!joined.contains("KnownHostsFile"), "{joined}");
        assert!(!joined.contains("accept-new"), "{joined}");
        assert_eq!(
            argv.last().map(String::as_str),
            Some(opts.remote_command.as_str())
        );
    }

    #[test]
    fn a_broadcast_names_the_countdown_and_the_way_out() {
        let body = lines(&[
            "On battery: servers power off in 300 s unless mains return.",
            "Nothing is off yet. `systemctl status ups-fleet-shutdown` on gw.",
        ]);
        let text = compose_broadcast("POWER OUTAGE", &body, Some(&status("OB DISCHRG")));
        assert!(text.starts_with("=== gw UPS: POWER OUTAGE ==="), "{text}");
        assert!(text.contains("300 s"), "{text}");
        assert!(text.contains("unless mains return"), "{text}");
        assert!(text.contains("ups: on battery"), "{text}");
    }

    #[test]
    fn a_broadcast_survives_no_status() {
        // The notice has to go out even when `upsc` is the thing that is broken;
        // that is the difference between a broadcast and a status page.
        let text = compose_broadcast("POWER RESTORED", &lines(&["Mains are back."]), None);
        assert_eq!(text, "=== gw UPS: POWER RESTORED ===\nMains are back.\n");
    }

    #[test]
    fn a_drill_reports_an_abort_instead_of_obeying_it() {
        let s = status("OL CHRG");
        let real = proceed(decide(Some(&s), Reason::OutageGraceExpired), false);
        assert!(matches!(real, Proceed::No { .. }), "{real:?}");

        // …and the same decision, drilled, is a note and a plan.
        let drill = proceed(decide(Some(&s), Reason::OutageGraceExpired), true);
        match drill {
            Proceed::Yes { note: Some(note) } => {
                assert!(note.contains("would ABORT"), "{note}");
                assert!(note.contains("continuing"), "{note}");
            }
            other => panic!("a drill must continue: {other:?}"),
        }
    }

    #[test]
    fn a_late_reason_still_notes_that_it_is_late() {
        let s = status("OB LB");
        match proceed(decide(Some(&s), Reason::LowBattery), false) {
            Proceed::Yes { note: Some(note) } => assert!(note.contains("already late"), "{note}"),
            other => panic!("{other:?}"),
        }
        match proceed(decide(Some(&s), Reason::OutageGraceExpired), false) {
            Proceed::Yes { note: None } => {}
            other => panic!("in time, and nothing to say: {other:?}"),
        }
    }

    #[test]
    fn display_says_what_a_journal_needs() {
        let s = status("OB LB");
        let text = s.to_string();
        assert!(text.contains("on battery"), "{text}");
        assert!(text.contains("LOW BATTERY"), "{text}");
    }
}
