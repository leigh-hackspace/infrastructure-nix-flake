//! `ups-watchdog` — the fleet half of a power cut on `gw`.
//!
//! NUT owns the UPS. It is the thing that speaks APC's HID dialect down a USB
//! cable, and [`plan`] exists to make sure nothing in here pretends otherwise.
//! This program owns the one thing NUT has no idea about: **`gw` is the router,
//! and a router that dies with its neighbours still drawing current is a hard
//! shutdown for all of them.** So when the battery has been the only supply for
//! longer than the grace period allows, this powers the other machines off over
//! ssh — in the order `machines/gw/ups.nix` gives it — and only then itself.
//!
//! Four modes, and the difference between them is the whole reason there is a
//! binary rather than a shell function in `upssched.conf`:
//!
//! * `run` — the sequence. Started by `upssched`'s CMDSCRIPT (or by `upsmon`'s
//!   `SHUTDOWNCMD`) as `ups-fleet-shutdown.service`, which is
//!   `RemainAfterExit`, so a second and third start are no-ops and `upsmon`
//!   polling its `SHUTDOWNCMD` every five seconds cannot queue a second attempt.
//! * `plan` — print the order and the exact argv, run nothing, touch nothing.
//!   This is what `just ups-drill` shows you before you trust `run`.
//! * `status` — ask `upsd` what the UPS says and exit with a code a script can
//!   branch on (0 discharging, 10 on mains, 20 unreadable).
//! * `broadcast` — `wall` one notice to every server and to this box's own
//!   terminals.  NUT's events start this unit directly for the two notices that
//!   are not part of a shutdown (`on battery, 300 s of grace`, `mains restored`),
//!   because a notice that only appears once the countdown has expired is a
//!   confession rather than a warning.
//!
//! The four modules are the four jobs: [`plan`] decides, [`exec`] runs programs
//! under a deadline, [`notice`] gets messages to the fleet, and this file is the
//! argv and the sequence.
//!
//! What this program deliberately does **not** do:
//!
//! * **It does not contain the timer.** "Discharging for N seconds, cancelled by
//!   `ONLINE`" is `upssched`'s documented idiom and it is configured next to the
//!   rest of NUT's config, where a powercut-time reader will find it. A second
//!   implementation here would be a second timer to disagree with.
//! * **It does not write NVRAM or the UPS's load.** The only power command sent
//!   to the UPS is the one NUT already sends at shutdown (`ups-killpower` →
//!   `upsdrvctl shutdown`); this program's contribution is the file that arms it
//!   — see `--killpower-flag`.
//! * **It does not read any secret.** The hop key is passed by *path* and handed
//!   to `ssh`, which is the only process that opens it. Nothing here prints a
//!   file's contents, and `--dry-run` prints argv, not keys.

mod exec;
mod notice;
mod plan;

use exec::{bounded_output, run_bounded, status_line, write_killpower_flag};
use plan::{
    compose_broadcast, decide, ssh_command, Decision, Mains, Reason, SshOpts, Status, Target,
};

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

/// Exit codes, because this runs as a oneshot and `systemctl show -p
/// ExecMainStatus` is often the only record it leaves when there was no battery
/// left to write a journal with.
const EXIT_DONE: i32 = 0;
/// The re-check said the mains are back: nothing was shut down.
const EXIT_ABORTED: i32 = 21;
/// A broadcast that did not reach everyone it was aimed at.
const EXIT_NOTICE_INCOMPLETE: i32 = 30;
const EXIT_USAGE: i32 = 2;

/// The unit that runs this program in `run` mode, named in the notices so that a
/// message on a terminal three boxes away says what to stop.
const UNIT: &str = "ups-fleet-shutdown";

const DEFAULT_REMOTE_COMMAND: &str = "sudo -n systemctl poweroff --no-block";
const DEFAULT_WALL_COMMAND: &str = "sudo -n wall -n";
const DEFAULT_SSH_TIMEOUT: u64 = 30;
const DEFAULT_CONNECT_TIMEOUT: u64 = 5;
/// The gap between "this is happening" and "it is happening", for the
/// grace-timer path.  It is what turns the shutdown notice into an offer rather
/// than an announcement: the only way to stop the sequence is to read the `wall`
/// and stop the unit, and that needs seconds to happen in.
const DEFAULT_NOTICE_DELAY: u64 = 30;
/// The same window when the reason is already late (`LOWBATT`, `upsmon`):
/// remaining battery is worth more than an intervention nobody asked for.
const DEFAULT_LATE_NOTICE_DELAY: u64 = 5;
/// How long to wait after the last hop before powering this box off — long
/// enough for an ssh'd `systemctl poweroff` to be through systemd's own teardown
/// on the far side, short enough not to matter at 4 % battery.
const DEFAULT_SETTLE: u64 = 10;
const DEFAULT_STATUS_TIMEOUT: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Run,
    Plan,
    Status,
    Broadcast,
}

#[derive(Debug, Clone)]
struct Opts {
    mode: Mode,
    dry_run: bool,
    /// `NAME=ADDRESS` pairs, in shutdown order.
    hosts: Vec<String>,
    /// Addresses this box answers on.  A target answering one of them is refused
    /// by name *and* by address — the reason is in `plan::plan`.
    self_addresses: Vec<String>,
    reason: Reason,
    reason_file: Option<PathBuf>,
    ssh: SshOpts,
    ssh_timeout: u64,
    connect_timeout: u64,
    notice_delay: u64,
    late_notice_delay: u64,
    settle: u64,
    poweroff_bin: String,
    no_poweroff: bool,
    killpower_flag: Option<PathBuf>,
    upsc_bin: String,
    ups: Option<String>,
    status_timeout: u64,
    // --- the broadcast: `wall` on every server, and on this box's terminals ---
    /// `run` only: announce the shutdown before starting it.  `broadcast` mode
    /// ignores this flag, because announcing is the whole job there.
    broadcast: bool,
    broadcast_headline: Option<String>,
    broadcast_messages: Vec<String>,
    /// For `broadcast` mode: the notice file `ups-trigger` wrote.  Line 1 is the
    /// headline, the rest is the body.
    notice_file: Option<PathBuf>,
    wall_command: String,
    local_wall: bool,
    local_wall_bin: String,
    broadcast_timeout: u64,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts {
            mode: Mode::Run,
            dry_run: false,
            hosts: Vec::new(),
            self_addresses: Vec::new(),
            reason: Reason::Unknown,
            reason_file: None,
            ssh: SshOpts {
                bin: "ssh".to_string(),
                user: "leigh-admin".to_string(),
                identity_file: String::new(),
                remote_command: DEFAULT_REMOTE_COMMAND.to_string(),
            },
            ssh_timeout: DEFAULT_SSH_TIMEOUT,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            notice_delay: DEFAULT_NOTICE_DELAY,
            late_notice_delay: DEFAULT_LATE_NOTICE_DELAY,
            settle: DEFAULT_SETTLE,
            poweroff_bin: "systemctl".to_string(),
            no_poweroff: false,
            killpower_flag: None,
            upsc_bin: "upsc".to_string(),
            ups: None,
            status_timeout: DEFAULT_STATUS_TIMEOUT,
            broadcast: false,
            broadcast_headline: None,
            broadcast_messages: Vec::new(),
            notice_file: None,
            wall_command: DEFAULT_WALL_COMMAND.to_string(),
            local_wall: false,
            local_wall_bin: "wall".to_string(),
            broadcast_timeout: 15,
        }
    }
}

fn usage() -> String {
    let remote = DEFAULT_REMOTE_COMMAND;
    let wall = DEFAULT_WALL_COMMAND;
    format!(
        "ups-watchdog <run|plan|status|broadcast> [options]\n\
         \n\
         Powers the fleet off before the UPS runs out, then this box.\n\
         \n\
         Every flag below is passed by machines/gw/ups.nix. The defaults are the safe\n\
         ones, not a working configuration: there is no default identity file and no\n\
         default host list, on purpose.\n\
         \x20 --host NAME=ADDRESS        machine to power off, in order (repeatable)\n\
         \x20 --self-address ADDRESS     an address this box owns; refuse to ssh to it\n\
         \x20 --identity-file PATH       ssh private key, by path (never opened by us)\n\
         \x20 --ssh-user USER            default: leigh-admin\n\
         \x20 --ssh-bin PATH             default: ssh\n\
         \x20 --remote-command CMD       default: {remote}\n\
         \x20 --ssh-timeout SECS         wall-clock limit per hop (default {DEFAULT_SSH_TIMEOUT})\n\
         \x20 --connect-timeout SECS     ssh ConnectTimeout (default {DEFAULT_CONNECT_TIMEOUT})\n\
         \x20 --notice-delay-seconds N   gap between the notice and the first hop (default {DEFAULT_NOTICE_DELAY})\n\
         \x20 --late-notice-delay-seconds N  the same, when the reason is already late (default {DEFAULT_LATE_NOTICE_DELAY})\n\
         \x20 --settle-seconds N         gap between the last hop and this box going down (default {DEFAULT_SETTLE})\n\
         \x20 --poweroff-bin PATH        default: systemctl, run as `systemctl poweroff`\n\
         \x20 --no-poweroff              stop before powering off this box\n\
         \x20 --killpower-flag PATH      create PATH (NUT's POWERDOWNFLAG) so the UPS drops its\n\
         \x20                            output at shutdown and returns it with mains\n\
         \x20 --reason NAME              why we were started (see --reason-file)\n\
         \x20 --reason-file PATH         first line wins over --reason\n\
         \x20 --ups NAME@HOST            the best-effort status read (default: no read)\n\
         \x20 --upsc-bin PATH            default: upsc\n\
         \x20 --status-timeout SECS      default {DEFAULT_STATUS_TIMEOUT}\n\
         \x20 --broadcast                run only: wall the shutdown before starting it\n\
         \x20 --broadcast-headline TEXT  banner line for that message\n\
         \x20 --broadcast-message TEXT   body line of that message (repeatable)\n\
         \x20 --notice-file PATH         broadcast mode: line 1 is the headline, rest is body\n\
         \x20 --wall-command CMD         remote command (default: {wall})\n\
         \x20 --local-wall               also wall this box's own terminals\n\
         \x20 --local-wall-bin PATH      default: wall\n\
         \x20 --broadcast-timeout SECS   limit per recipient (default 15)\n\
         \x20 --dry-run                  print, do not run, do not sleep, do not touch\n\
         \n\
         exit: run 0 done | {EXIT_ABORTED} aborted (mains back) | {EXIT_USAGE} bad flags;\n\
         status 0 discharging | 10 on mains | 20 unreadable;\n\
         broadcast 0 everyone reached | {EXIT_NOTICE_INCOMPLETE} somebody was missed\n"
    )
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match parse_opts(&argv) {
        Ok(opts) => std::process::exit(dispatch(&opts)),
        Err(why) => {
            eprintln!("ups-watchdog: {why}\n\n{}", usage());
            std::process::exit(EXIT_USAGE);
        }
    }
}

fn dispatch(opts: &Opts) -> i32 {
    match opts.mode {
        Mode::Plan => mode_plan(opts),
        Mode::Status => mode_status(opts),
        Mode::Broadcast => mode_broadcast(opts),
        Mode::Run => mode_run(opts),
    }
}

// --- mode: plan ---------------------------------------------------------------

/// Print the sequence without running any of it.  This is the output a reviewer
/// compares against what they meant to deploy, so it prints argv verbatim.
fn mode_plan(opts: &Opts) -> i32 {
    let (targets, findings) = targets(opts);
    report_findings(&findings);
    println!(
        "ups-watchdog: order of operations (reason {}, {} target(s))",
        opts.reason,
        targets.len()
    );
    for (i, t) in targets.iter().enumerate() {
        println!(
            "  {}. {} — {}",
            i + 1,
            t.name,
            ssh_command(t, &opts.ssh, opts.connect_timeout).join(" ")
        );
    }
    // The router is last and is not in the list: it powers off locally, after
    // the notice window, never over ssh.
    println!("  {}. gw — {}", targets.len() + 1, poweroff_argv(opts).join(" "));
    if opts.no_poweroff {
        println!("       (this one is suppressed: --no-poweroff)");
    }
    if targets.is_empty() {
        eprintln!("ups-watchdog: a plan with no targets in it is not a plan");
        return EXIT_USAGE;
    }
    EXIT_DONE
}

// --- mode: status -------------------------------------------------------------

fn mode_status(opts: &Opts) -> i32 {
    let ups = match &opts.ups {
        Some(ups) => ups.clone(),
        None => {
            eprintln!("ups-watchdog: status needs --ups NAME@HOST");
            return EXIT_USAGE;
        }
    };
    let argv = vec![opts.upsc_bin.clone(), ups.clone()];
    match bounded_output(&argv, Duration::from_secs(opts.status_timeout)) {
        Ok(text) => {
            print!("{text}");
            let parsed = status_line(&text).map(Status::parse);
            match &parsed {
                Some(status) => println!("-- parsed: {status}"),
                None => println!("-- parsed: no ups.status line in that output"),
            }
            // The exit codes are the interface `just ups-status` and any future
            // monitoring endpoint branch on, and they are three-valued because the
            // question is: 0 on battery, 10 mains fine, 20 cannot tell.  "Cannot
            // tell" is never reported as "fine".
            match parsed.as_ref() {
                Some(s) if s.discharging() => 0,
                Some(s) if s.mains == Mains::On => 10,
                _ => 20,
            }
        }
        Err(why) => {
            eprintln!("ups-watchdog: {ups}: {why}");
            20
        }
    }
}

// --- mode: broadcast ----------------------------------------------------------

/// `wall` one message to every server and, if asked, to this box's own
/// terminals.
///
/// This is a mode rather than a few lines inside `run`, because NUT's notices are
/// worth sending when nothing is being powered off: "on battery, 300 s of grace"
/// has to be said the moment it becomes true, and a notice that first appears
/// once the countdown has already expired is a confession rather than a warning.
fn mode_broadcast(opts: &Opts) -> i32 {
    let (headline, body) = match notice::notice_text(opts) {
        Ok(pair) => pair,
        Err(why) => {
            eprintln!("ups-watchdog: {why}");
            return EXIT_USAGE;
        }
    };
    // Best effort, like every other use of it: a notice that cannot read the UPS
    // still goes out, because "we cannot see the UPS" is not "the mains are fine".
    let status = read_status(opts);
    let text = compose_broadcast(&headline, &body, status.as_ref());
    let (recipients, findings) = targets(opts);
    report_findings(&findings);
    println!(
        "ups-watchdog: broadcasting `{headline}` to {} server(s){}",
        recipients.len(),
        if opts.local_wall {
            " and this box's terminals"
        } else {
            ""
        }
    );
    let missed = notice::send_notice(opts, &text);
    if missed > 0 {
        eprintln!("ups-watchdog: {missed} recipient(s) were not reached");
        return EXIT_NOTICE_INCOMPLETE;
    }
    EXIT_DONE
}


// --- mode: run ----------------------------------------------------------------

fn mode_run(opts: &Opts) -> i32 {
    let (reason, provenance) = reason(opts);
    println!(
        "ups-watchdog: starting the fleet shutdown (reason {reason} — {provenance}; dry-run: {})",
        opts.dry_run
    );

    // Step 1 — the re-check.  The timer belongs to `upssched`; this is the last
    // look before anything is switched off, and it is advisory in both
    // directions for the reasons in `plan::decide`.
    let status = read_status(opts);
    match &status {
        Some(s) => println!("ups-watchdog: the UPS says {s}"),
        None => println!(
            "ups-watchdog: ups.status could not be re-read; acting on the timer's authority \
             (upssched cancels that same timer on COMMBAD/TIMEDOUT, so a UPS that went away \
             mid-outage is already handled — a broken `upsc` is not a reason to ride the \
             battery to zero)"
        ),
    }
    let ups_label = opts.ups.clone().unwrap_or_else(|| "<ups>".to_string());
    if let Some(s) = &status {
        if s.mains == Mains::On && reason.is_already_late() {
            eprintln!(
                "ups-watchdog: WARNING the UPS reports mains, but {reason} is already past \
                 waiting, so this continues. If that is wrong: `systemctl stop \
                 ups-fleet-shutdown` and read `upsc {ups_label} ups.status`"
            );
        }
    }
    match decide(status.as_ref(), reason) {
        Decision::Abort(why) => {
            eprintln!("ups-watchdog: ABORT, nothing was shut down: {why}");
            return EXIT_ABORTED;
        }
        Decision::Act { late } if late => {
            println!(
                "ups-watchdog: this reason is already late — short notice window, no further \
                 waits"
            );
        }
        Decision::Act { .. } => {}
    }

    // Step 1.5 — say so before doing it.  The notice goes out *ahead* of the
    // hops, and the wait in step 2 is the window it buys: the only way to stop
    // this is to read the message and `systemctl stop ups-fleet-shutdown` inside
    // it.  A failed notice never stops the shutdown: a box that cannot be told is
    // still going to be told that it went down.
    if opts.broadcast {
        let headline = opts
            .broadcast_headline
            .clone()
            .unwrap_or_else(|| "FLEET SHUTDOWN BEGINNING".to_string());
        let text = compose_broadcast(&headline, &opts.broadcast_messages, status.as_ref());
        println!("ups-watchdog: broadcasting the shutdown notice before touching anything");
        let missed = notice::send_notice(opts, &text);
        if missed > 0 {
            eprintln!(
                "ups-watchdog: {missed} notice recipient(s) were not reached; continuing — \
                 their absence is exactly why the outage cannot wait on them"
            );
        }
    }

    // Step 2 — the window the notice just bought.  Skipping it would make the
    // notice a courtesy rather than an intervention: this is the only point in the
    // sequence where a human who reads the wall can still stop everything.
    let window = if reason.is_already_late() {
        opts.late_notice_delay
    } else {
        opts.notice_delay
    };
    if opts.dry_run {
        println!("ups-watchdog: [dry-run] would wait {window}s for someone to stop this");
    } else if window > 0 {
        println!("ups-watchdog: {window}s before the first hop — `systemctl stop {UNIT}` cancels here");
        thread::sleep(Duration::from_secs(window));
    }

    // Step 3 — the fleet.  Findings here are printed and *not* fatal: one
    // misconfigured row must not cost every other machine an orderly shutdown,
    // and this box must not be the one left running until its battery dies.
    let (targets, findings) = targets(opts);
    report_findings(&findings);
    if targets.is_empty() {
        eprintln!(
            "ups-watchdog: no usable target, so this box powers off alone — which is still \
             better than dying mid-write with the network already down"
        );
    }

    let mut uncertain = 0usize;
    for t in &targets {
        let argv = ssh_command(t, &opts.ssh, opts.connect_timeout);
        if opts.dry_run {
            println!("ups-watchdog: [dry-run] {} — {}", t.name, argv.join(" "));
            continue;
        }
        println!("ups-watchdog: {} ({}): powering off", t.name, t.address);
        match run_bounded(&argv, Duration::from_secs(opts.ssh_timeout)) {
            Ok(0) => println!("ups-watchdog:   {}: shutdown accepted", t.name),
            // ssh's own error exit.  A box that is going down closes the
            // connection under us and that also arrives as 255, so 255 means
            // *"probably"*.  It is counted as uncertain rather than as a clean
            // stop, because the outage journal is the only record there will be.
            Ok(255) => {
                uncertain += 1;
                eprintln!(
                    "ups-watchdog:   {}: ssh exited 255 — already going down, or never \
                     answered; continuing either way",
                    t.name
                );
            }
            Ok(code) => {
                uncertain += 1;
                eprintln!(
                    "ups-watchdog:   {}: remote command exited {code}; continuing (the outage \
                     does not get to wait for a machine that will not obey)",
                    t.name
                );
            }
            Err(why) => {
                uncertain += 1;
                eprintln!("ups-watchdog:   {name}: {why}; continuing", name = t.name);
            }
        }
    }
    if uncertain > 0 {
        eprintln!(
            "ups-watchdog: {uncertain} of {} hop(s) are unconfirmed. From here the NAS and \
             everything still running loses its router; `journalctl -u ups-fleet-shutdown` is \
             the record of which ones.",
            targets.len()
        );
    }

    // Step 4 — let the ones that accepted it get through their own teardown
    // before their router, the NAS's gateway and everyone's DNS server go too.
    if opts.dry_run {
        println!("ups-watchdog: [dry-run] would wait {}s, then power off gw", opts.settle);
    } else if opts.settle > 0 {
        println!(
            "ups-watchdog: waiting {}s for the fleet to settle",
            opts.settle
        );
        thread::sleep(Duration::from_secs(opts.settle));
    }

    // Step 5 — arm NUT's killpower, then follow it.  POWERDOWNFLAG is what
    // `ups-killpower.service` waits for: with it the UPS is told to drop its
    // output and to bring it back when mains return, which is the only way this
    // box comes back by itself after an outage that outlasted the battery.
    // Without it the outlets stay live, the box sits at the power button, and
    // "the network came back on its own" was a rumour.
    if opts.no_poweroff {
        println!("ups-watchdog: not powering off this box (--no-poweroff); the fleet has been told");
        return EXIT_DONE;
    }
    if !opts.dry_run {
        if let Some(flag) = &opts.killpower_flag {
            match write_killpower_flag(flag) {
                Ok(()) => println!(
                    "ups-watchdog: wrote {} — the UPS will be told to drop its load at \
                     shutdown and return it with mains",
                    flag.display()
                ),
                Err(why) => eprintln!(
                    "ups-watchdog: could not write {}: {why} — the UPS will keep its outlets \
                     energised and this box will not power on itself when mains return",
                    flag.display()
                ),
            }
        }
    }

    let argv = poweroff_argv(opts);
    if opts.dry_run {
        println!("ups-watchdog: [dry-run] would run: {}", argv.join(" "));
        return EXIT_DONE;
    }
    println!(
        "ups-watchdog: powering off gw — the network is dark until mains return and this box does"
    );
    match run_bounded(&argv, Duration::from_secs(15)) {
        Ok(0) => println!("ups-watchdog: shutdown accepted"),
        Ok(code) => eprintln!("ups-watchdog: poweroff exited {code}; systemd is halting anyway"),
        Err(why) => eprintln!("ups-watchdog: {why}; systemd is halting anyway"),
    }
    EXIT_DONE
}

// --- the effectful helpers ----------------------------------------------------

/// Resolve the shutdown list, and report everything wrong with it.
///
/// [`plan::plan`] is asked for the whole list first, so a finding that only
/// exists *between* two rows — one address under two names, a row that is this
/// box — is still reported. If it refused, the entries that are individually
/// sound are kept: in `run` a finding must never be fatal, and a typo in the
/// third row must not cost the first two their orderly shutdown.
fn targets(opts: &Opts) -> (Vec<Target>, Vec<String>) {
    let findings = match plan::plan(&opts.hosts, &opts.self_addresses) {
        Ok(list) => return (list, Vec::new()),
        Err(findings) => findings,
    };
    let mut usable: Vec<Target> = Vec::new();
    for spec in &opts.hosts {
        let Ok(target) = Target::parse(spec) else { continue };
        if opts
            .self_addresses
            .iter()
            .any(|me| me.eq_ignore_ascii_case(&target.address))
        {
            continue;
        }
        if usable.iter().any(|t| t.address == target.address) {
            continue;
        }
        usable.push(target);
    }
    (usable, findings)
}

fn report_findings(findings: &[String]) {
    for finding in findings {
        eprintln!("ups-watchdog: {finding}");
    }
}

/// The reason we were woken up, and where that came from.
///
/// The file wins over the flag because the file is written by the event:
/// `upssched`'s CMDSCRIPT records `low-battery` against the grace timer's
/// `outage-grace-expired`, and `--reason` is the fallback for whoever started
/// the unit by hand. `/run` is wiped by the reboot, so a stale marker cannot
/// outlive the event it described — which matters, because a stale
/// `low-battery` would be read as "already late" and skip the notice window.
fn reason(opts: &Opts) -> (Reason, String) {
    match &opts.reason_file {
        Some(path) => match fs::read_to_string(path) {
            Ok(text) => {
                let first = text.lines().next().unwrap_or("").trim().to_string();
                if first.is_empty() {
                    (opts.reason, format!("{} was empty", path.display()))
                } else {
                    (
                        Reason::parse(&first),
                        format!("{} says `{first}`", path.display()),
                    )
                }
            }
            Err(_) => (
                opts.reason,
                format!("no {} — the unit's own --reason", path.display()),
            ),
        },
        None => (opts.reason, "--reason".to_string()),
    }
}

/// One best-effort read of `ups.status`.  `None` means "could not ask" and is
/// never "not an outage" — see [`plan::decide`].
fn read_status(opts: &Opts) -> Option<Status> {
    let ups = opts.ups.as_ref()?;
    let argv = vec![opts.upsc_bin.clone(), ups.clone(), "ups.status".to_string()];
    match bounded_output(&argv, Duration::from_secs(opts.status_timeout)) {
        Ok(text) => status_line(&text).map(Status::parse),
        Err(why) => {
            eprintln!("ups-watchdog: {ups} ups.status: {why}");
            None
        }
    }
}

fn poweroff_argv(opts: &Opts) -> Vec<String> {
    vec![opts.poweroff_bin.clone(), "poweroff".to_string()]
}

// --- argv --------------------------------------------------------------------

/// Parse argv by hand: the crate has no dependencies (Cargo.toml says why), and
/// this is a flag list, not a language.  One value per flag, `--flag=value`
/// accepted, anything unknown fatal.
fn parse_opts(argv: &[String]) -> Result<Opts, String> {
    let mut opts = Opts::default();
    let mut mode_set = false;
    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].as_str();
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n.to_string(), Some(v.to_string())),
            _ => (arg.to_string(), None),
        };
        // Consume the value, whether it came attached or as the next argument.
        let value = |i: &mut usize, flag: &str| -> Result<String, String> {
            match inline.clone() {
                Some(v) => Ok(v),
                None => {
                    *i += 1;
                    argv.get(*i).cloned().ok_or_else(|| format!("{flag} needs a value"))
                }
            }
        };
        match name.as_str() {
            "run" | "plan" | "status" | "broadcast" => {
                if mode_set {
                    return Err(format!("a second mode `{name}`"));
                }
                opts.mode = match name.as_str() {
                    "run" => Mode::Run,
                    "plan" => Mode::Plan,
                    "status" => Mode::Status,
                    _ => Mode::Broadcast,
                };
                mode_set = true;
            }
            "--host" => opts.hosts.push(value(&mut i, "--host")?),
            "--self-address" => opts.self_addresses.push(value(&mut i, "--self-address")?),
            "--identity-file" => opts.ssh.identity_file = value(&mut i, "--identity-file")?,
            "--ssh-user" => opts.ssh.user = value(&mut i, "--ssh-user")?,
            "--ssh-bin" => opts.ssh.bin = value(&mut i, "--ssh-bin")?,
            "--remote-command" => opts.ssh.remote_command = value(&mut i, "--remote-command")?,
            "--poweroff-bin" => opts.poweroff_bin = value(&mut i, "--poweroff-bin")?,
            "--reason" => opts.reason = Reason::parse(&value(&mut i, "--reason")?),
            "--reason-file" => opts.reason_file = Some(PathBuf::from(value(&mut i, "--reason-file")?)),
            "--ups" => opts.ups = Some(value(&mut i, "--ups")?),
            "--upsc-bin" => opts.upsc_bin = value(&mut i, "--upsc-bin")?,
            "--killpower-flag" => opts.killpower_flag = Some(PathBuf::from(value(&mut i, "--killpower-flag")?)),
            "--ssh-timeout" => opts.ssh_timeout = seconds(&value(&mut i, "--ssh-timeout")?, "--ssh-timeout")?,
            "--connect-timeout" => {
                opts.connect_timeout = seconds(&value(&mut i, "--connect-timeout")?, "--connect-timeout")?
            }
            "--notice-delay-seconds" => {
                opts.notice_delay = seconds(&value(&mut i, "--notice-delay-seconds")?, "--notice-delay-seconds")?
            }
            "--late-notice-delay-seconds" => {
                opts.late_notice_delay = seconds(
                    &value(&mut i, "--late-notice-delay-seconds")?,
                    "--late-notice-delay-seconds",
                )?
            }
            "--settle-seconds" => opts.settle = seconds(&value(&mut i, "--settle-seconds")?, "--settle-seconds")?,
            "--status-timeout" => {
                opts.status_timeout = seconds(&value(&mut i, "--status-timeout")?, "--status-timeout")?
            }
            "--broadcast" => opts.broadcast = true,
            "--broadcast-headline" => {
                opts.broadcast_headline = Some(value(&mut i, "--broadcast-headline")?)
            }
            "--broadcast-message" => opts.broadcast_messages.push(value(&mut i, "--broadcast-message")?),
            "--notice-file" => opts.notice_file = Some(PathBuf::from(value(&mut i, "--notice-file")?)),
            "--wall-command" => opts.wall_command = value(&mut i, "--wall-command")?,
            "--local-wall" => opts.local_wall = true,
            "--local-wall-bin" => opts.local_wall_bin = value(&mut i, "--local-wall-bin")?,
            "--broadcast-timeout" => {
                opts.broadcast_timeout = seconds(&value(&mut i, "--broadcast-timeout")?, "--broadcast-timeout")?
            }
            "--dry-run" => opts.dry_run = true,
            "--no-poweroff" => opts.no_poweroff = true,
            "-h" | "--help" => return Err(usage()),
            other if other.starts_with('-') => return Err(format!("unknown flag `{other}`")),
            other => return Err(format!("unexpected argument `{other}`")),
        }
        i += 1;
    }

    if !mode_set {
        return Err("no mode given (run|plan|status)".to_string());
    }
    // An empty identity file means ssh would go looking for whatever else is in
    // reach. A power cut is not the moment to improvise an identity.
    if opts.mode != Mode::Status && opts.ssh.identity_file.is_empty() {
        return Err("--identity-file is required (which key hops to the fleet?)".to_string());
    }
    // The notice and the shutdown must be aimed at the same machines, and a
    // broadcast with nobody in it would exit 0 having told no one — which is the
    // one failure mode an operator would never hear about.
    if opts.mode == Mode::Broadcast && !opts.local_wall && opts.hosts.is_empty() {
        return Err("broadcast has no recipient: pass --host NAME=ADDRESS and/or --local-wall"
            .to_string());
    }
    if opts.mode == Mode::Run && !opts.no_poweroff && !opts.dry_run && opts.killpower_flag.is_none()
    {
        return Err(
            "--killpower-flag is required unless --no-poweroff or --dry-run: an outage that \
             ends with the UPS still energised is an outage the network does not recover from \
             by itself"
                .to_string(),
        );
    }
    Ok(opts)
}

fn seconds(text: &str, flag: &str) -> Result<u64, String> {
    text.trim()
        .parse::<u64>()
        .map_err(|_| format!("{flag} wants a whole number of seconds, got `{text}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }


    #[test]
    fn a_mode_is_mandatory_and_unique() {
        assert!(parse_opts(&args("--host a=1")).is_err());
        assert!(parse_opts(&args("run plan")).is_err());
        // `plan` needs the key too: the point of the mode is to print the argv
        // `run` would use, and an empty `-i` would print one that could never run.
        assert_eq!(
            parse_opts(&args("plan --identity-file /k"))
                .expect("plan is a mode")
                .mode,
            Mode::Plan
        );
        assert!(parse_opts(&args("plan")).is_err());
    }

    #[test]
    fn run_needs_a_key_and_a_killpower_flag() {
        let err = parse_opts(&args("run --host a=10.3.1.20")).expect_err("incomplete");
        assert!(err.contains("--identity-file"), "{err}");
        let err = parse_opts(&args("run --host a=10.3.1.20 --identity-file /k")).expect_err("incomplete");
        assert!(err.contains("--killpower-flag"), "{err}");
        // A drill is allowed to skip both promises, and so is a --no-poweroff.
        assert!(parse_opts(&args("run --host a=10.3.1.20 --identity-file /k --dry-run")).is_ok());
        assert!(
            parse_opts(&args("run --host a=10.3.1.20 --identity-file /k --no-poweroff")).is_ok()
        );
    }

    #[test]
    fn both_flag_shapes_take_their_value() {
        let opts = parse_opts(&args("plan --identity-file=/k --ssh-timeout 9")).expect("fine");
        assert_eq!(opts.ssh.identity_file, "/k");
        assert_eq!(opts.ssh_timeout, 9);
        assert!(parse_opts(&args("plan --identity-file")).is_err());
        assert!(parse_opts(&args("plan --ssh-timeout soon")).is_err());
        assert!(parse_opts(&args("plan --nope")).is_err());
        assert!(parse_opts(&args("plan -- --nope")).is_err());
        assert!(parse_opts(&args("plan --identity-file /k extra")).is_err());
    }

    #[test]
    fn hosts_keep_their_order_and_their_equals_signs() {
        let opts = parse_opts(&args(
            "plan --identity-file /k --host services1=10.3.1.20 --host aibox=10.3.1.32",
        ))
        .expect("fine");
        assert_eq!(opts.hosts, vec!["services1=10.3.1.20", "aibox=10.3.1.32"]);
    }

    #[test]
    fn one_bad_row_does_not_take_the_others_with_it() {
        let opts = parse_opts(&args(
            "plan --identity-file /k --host broken --host good=10.3.1.9",
        ))
        .expect("argv is fine; the row is not");
        let (targets, findings) = targets(&opts);
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert_eq!(targets[0].name, "good");
        assert_eq!(findings.len(), 1, "{findings:?}");
    }

    #[test]
    fn the_router_row_is_dropped_and_said_so() {
        let opts = parse_opts(&args(
            "plan --identity-file /k --self-address 10.3.1.1 --host gw=10.3.1.1 --host services1=10.3.1.20",
        ))
        .expect("fine");
        let (targets, findings) = targets(&opts);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "services1");
        assert!(findings.iter().any(|f| f.contains("this box")), "{findings:?}");
    }

    #[test]
    fn one_address_under_two_names_is_caught_across_rows() {
        let opts = parse_opts(&args(
            "plan --identity-file /k --host services1=10.3.1.20 --host oldbox=10.3.1.20",
        ))
        .expect("fine");
        let (targets, findings) = targets(&opts);
        assert_eq!(targets.len(), 1, "one address is one machine: {targets:?}");
        assert!(findings.iter().any(|f| f.contains("already")), "{findings:?}");
    }


    #[test]
    fn an_empty_or_absent_reason_file_falls_back_to_the_flag() {
        let dir = std::env::temp_dir().join(format!("ups-watchdog-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("tmpdir");
        let file = dir.join("reason");
        let opts = Opts {
            reason: Reason::UpsmonShutdown,
            reason_file: Some(file.clone()),
            ..Opts::default()
        };

        fs::write(&file, "\n").expect("write");
        assert_eq!(reason(&opts).0, Reason::UpsmonShutdown);

        fs::write(&file, "low-battery\nstale\n").expect("write");
        let (got, provenance) = reason(&opts);
        assert_eq!(got, Reason::LowBattery);
        assert!(provenance.contains("low-battery"), "{provenance}");

        let opts = Opts {
            reason: Reason::Manual,
            reason_file: Some(dir.join("absent")),
            ..Opts::default()
        };
        assert_eq!(reason(&opts).0, Reason::Manual);
        let _ = fs::remove_dir_all(&dir);
    }


    #[test]
    fn powering_off_is_the_local_systemctl_and_nothing_else() {
        let opts = Opts {
            poweroff_bin: "/run/booted-system/sw/bin/systemctl".to_string(),
            ..Opts::default()
        };
        assert_eq!(
            poweroff_argv(&opts),
            vec![
                "/run/booted-system/sw/bin/systemctl".to_string(),
                "poweroff".to_string()
            ]
        );
    }

    #[test]
    fn a_notice_needs_a_recipient_and_a_headline() {
        // A broadcast that reaches nobody must not exit 0: the outage journal
        // would read as "everyone was told", which is the one failure an operator
        // never hears about.
        let err = parse_opts(&args(
            "broadcast --identity-file /k --notice-file /run/ups-watchdog/notice",
        ))
        .expect_err("no recipient");
        assert!(err.contains("no recipient"), "{err}");
        assert!(parse_opts(&args(
            "broadcast --identity-file /k --local-wall --notice-file /run/ups-watchdog/notice"
        ))
        .is_ok());
        // …and with no notice file, a headline has to be named.
        let err = notice::notice_text(&Opts {
            local_wall: true,
            ..Opts::default()
        })
        .expect_err("no headline");
        assert!(err.contains("headline"), "{err}");
    }


}
