//! The argument table: what `machines/gw/ups.nix` passes, and the shape it becomes.
//!
//! This module knows nothing about power, ssh, or the fleet.  It turns argv into
//! an [`Opts`] and refuses — at parse time, where a mistake costs a failed unit
//! start rather than a machine that stayed up or one that did not — the shapes
//! that must never reach the run modes: no mode, no identity file, a `broadcast`
//! with nobody in it, a `run` that would power this box off without arming NUT's
//! POWERDOWNFLAG.  What to *do* with the result is `main.rs`; the split is by job,
//! and each half is one job (AGENTS.md rule 26).
//!
//! Every default here is the conservative one.  There is deliberately no default
//! host list and no default identity file: a program that powers machines off must
//! not be able to guess whom to power off, and a guessed ssh identity during a
//! power cut is an identity nobody audited.

use crate::plan::{Reason, SshOpts};
use std::path::PathBuf;

/// Exit codes, because this runs as a oneshot and `systemctl show -p
/// ExecMainStatus` is often the only record it leaves when there was no battery
/// left to write a journal with.
pub const EXIT_DONE: i32 = 0;
/// The re-check said the mains are back: nothing was shut down.
pub const EXIT_ABORTED: i32 = 21;
/// A broadcast that did not reach everyone it was aimed at.
pub const EXIT_NOTICE_INCOMPLETE: i32 = 30;
pub const EXIT_USAGE: i32 = 2;

/// The unit that runs this program in `run` mode, named in the notices so that a
/// message on a terminal three boxes away says what to stop.
pub const UNIT: &str = "ups-fleet-shutdown";

pub const DEFAULT_REMOTE_COMMAND: &str = "sudo -n systemctl poweroff --no-block";
pub const DEFAULT_WALL_COMMAND: &str = "sudo -n wall -n";
pub const DEFAULT_SSH_TIMEOUT: u64 = 30;
pub const DEFAULT_CONNECT_TIMEOUT: u64 = 5;
/// The gap between "this is happening" and "it is happening", for the
/// grace-timer path.  It is what turns the shutdown notice into an offer rather
/// than an announcement: the only way to stop the sequence is to read the `wall`
/// and stop the unit, and that needs seconds to happen in.
pub const DEFAULT_NOTICE_DELAY: u64 = 30;
/// The same window when the reason is already late (`LOWBATT`, `upsmon`):
/// remaining battery is worth more than an intervention nobody asked for.
pub const DEFAULT_LATE_NOTICE_DELAY: u64 = 5;
/// How long to wait after the last hop before powering this box off — long
/// enough for an ssh'd `systemctl poweroff` to be through systemd's own teardown
/// on the far side, short enough not to matter at 4 % battery.
pub const DEFAULT_SETTLE: u64 = 10;
pub const DEFAULT_STATUS_TIMEOUT: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Run,
    Plan,
    Status,
    Broadcast,
}

#[derive(Debug, Clone)]
pub struct Opts {
    pub mode: Mode,
    pub dry_run: bool,
    /// `NAME=ADDRESS` pairs, in shutdown order.
    pub hosts: Vec<String>,
    /// Addresses this box answers on.  A target answering one of them is refused
    /// by name *and* by address — the reason is in `plan::plan`.
    pub self_addresses: Vec<String>,
    pub reason: Reason,
    pub reason_file: Option<PathBuf>,
    pub ssh: SshOpts,
    pub ssh_timeout: u64,
    pub connect_timeout: u64,
    pub notice_delay: u64,
    pub late_notice_delay: u64,
    pub settle: u64,
    pub poweroff_bin: String,
    pub no_poweroff: bool,
    pub killpower_flag: Option<PathBuf>,
    pub upsc_bin: String,
    pub ups: Option<String>,
    pub status_timeout: u64,
    // --- the broadcast: `wall` on every server, and on this box's terminals ---
    /// `run` only: announce the shutdown before starting it.  `broadcast` mode
    /// ignores this flag, because announcing is the whole job there.
    pub broadcast: bool,
    pub broadcast_headline: Option<String>,
    pub broadcast_messages: Vec<String>,
    /// For `broadcast` mode: the notice file `ups-trigger` wrote.  Line 1 is the
    /// headline, the rest is the body.
    pub notice_file: Option<PathBuf>,
    pub wall_command: String,
    pub local_wall: bool,
    pub local_wall_bin: String,
    pub broadcast_timeout: u64,
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

pub fn usage() -> String {
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
/// Parse argv by hand: the crate has no dependencies (Cargo.toml says why), and
/// this is a flag list, not a language.  One value per flag, `--flag=value`
/// accepted, anything unknown fatal.
pub fn parse_opts(argv: &[String]) -> Result<Opts, String> {
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
                    argv.get(*i)
                        .cloned()
                        .ok_or_else(|| format!("{flag} needs a value"))
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
            "--reason-file" => {
                opts.reason_file = Some(PathBuf::from(value(&mut i, "--reason-file")?))
            }
            "--ups" => opts.ups = Some(value(&mut i, "--ups")?),
            "--upsc-bin" => opts.upsc_bin = value(&mut i, "--upsc-bin")?,
            "--killpower-flag" => {
                opts.killpower_flag = Some(PathBuf::from(value(&mut i, "--killpower-flag")?))
            }
            "--ssh-timeout" => {
                opts.ssh_timeout = seconds(&value(&mut i, "--ssh-timeout")?, "--ssh-timeout")?
            }
            "--connect-timeout" => {
                opts.connect_timeout =
                    seconds(&value(&mut i, "--connect-timeout")?, "--connect-timeout")?
            }
            "--notice-delay-seconds" => {
                opts.notice_delay = seconds(
                    &value(&mut i, "--notice-delay-seconds")?,
                    "--notice-delay-seconds",
                )?
            }
            "--late-notice-delay-seconds" => {
                opts.late_notice_delay = seconds(
                    &value(&mut i, "--late-notice-delay-seconds")?,
                    "--late-notice-delay-seconds",
                )?
            }
            "--settle-seconds" => {
                opts.settle = seconds(&value(&mut i, "--settle-seconds")?, "--settle-seconds")?
            }
            "--status-timeout" => {
                opts.status_timeout =
                    seconds(&value(&mut i, "--status-timeout")?, "--status-timeout")?
            }
            "--broadcast" => opts.broadcast = true,
            "--broadcast-headline" => {
                opts.broadcast_headline = Some(value(&mut i, "--broadcast-headline")?)
            }
            "--broadcast-message" => opts
                .broadcast_messages
                .push(value(&mut i, "--broadcast-message")?),
            "--notice-file" => {
                opts.notice_file = Some(PathBuf::from(value(&mut i, "--notice-file")?))
            }
            "--wall-command" => opts.wall_command = value(&mut i, "--wall-command")?,
            "--local-wall" => opts.local_wall = true,
            "--local-wall-bin" => opts.local_wall_bin = value(&mut i, "--local-wall-bin")?,
            "--broadcast-timeout" => {
                opts.broadcast_timeout = seconds(
                    &value(&mut i, "--broadcast-timeout")?,
                    "--broadcast-timeout",
                )?
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
        return Err(
            "broadcast has no recipient: pass --host NAME=ADDRESS and/or --local-wall".to_string(),
        );
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
