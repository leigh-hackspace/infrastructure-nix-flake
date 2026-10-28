//! Running programs under a deadline, and reading the UPS once.
//!
//! Every wait in `ups-watchdog` goes through here, and the deadline on each one
//! is the point.  This program's last step is `systemctl poweroff`; anything that
//! can hang before that step — an ssh to a box that has already lost its router,
//! an `upsc` aimed at an `upsd` that is wedged — does not fail harmlessly.  It
//! spends battery, and if it hangs long enough systemd's own `TimeoutStartSec`
//! kills the unit *before* the step that saves the machine.  So there is no
//! unbounded wait in this crate: these four functions are the only places a child
//! process is spawned, and all four return inside the limit they are given.
//!
//! The limit is enforced by polling `try_wait` rather than by trusting the child:
//! `ssh` honours `ConnectTimeout` but nothing guarantees it notices a server that
//! accepts the connection and then stops talking, and `wall` on the far side of an
//! outage may be writing to a console that is no longer there.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Pull the `ups.status` value out of `upsc` output, which is either one bare
/// value (`upsc <ups> ups.status`) or a whole `key: value` dump (`upsc <ups>`).
///
/// The prefixed form wins even when something else comes first, and a bare line
/// only counts if every token in it looks like a NUT status flag: `upsc` can put
/// prose on stdout, and reading a sentence as a status would be worse than
/// reading nothing — `plan::decide` treats a missing status as "act on the
/// timer's authority", and it must never get there on a mis-read.
pub(crate) fn status_line(text: &str) -> Option<&str> {
    let mut bare: Option<&str> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('[') || line.starts_with("Trying") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("ups.status:") {
            return Some(rest.trim());
        }
        if bare.is_none() && looks_like_status_flags(line) {
            bare = Some(line);
        }
    }
    bare
}

/// `OB LB`, `OL CHRG`, `--`: NUT's status flags are short, upper-case and
/// space-separated.  Everything else — including a line with a colon in it, which
/// is a `key: value` pair whose value we have not found yet — is not a status.
fn looks_like_status_flags(line: &str) -> bool {
    !line.is_empty()
        && !line.contains(':')
        && line.split_whitespace().all(|token| {
            token == "--"
                || (token.len() <= 8
                    && token
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()))
        })
}

/// Run a command with inherited stdio — so journald gets the child's own
/// complaint — under a wall-clock limit.
///
/// The limit is not courtesy: `TimeoutStartSec` would take the whole unit, and
/// with it the local poweroff at the end of the sequence, the moment one box
/// stopped answering.  A unit killed by its own timeout never reaches the step
/// that saves its own battery.  `Err` is only "we gave up" or "we could not start
/// it"; a non-zero exit is `Ok(code)`, because the caller decides what a code
/// means (255 does, and does not, depending on which step it is).
pub(crate) fn run_bounded(argv: &[String], limit: Duration) -> Result<i32, String> {
    let (bin, args) = argv.split_first().ok_or("empty command")?;
    let mut child = Command::new(bin)
        .args(args)
        .spawn()
        .map_err(|e| format!("could not run `{bin}`: {e}"))?;
    wait_bounded(&mut child, bin, limit)
}

/// The same, with `input` on the child's stdin — the only safe way to get a
/// message to `wall`.
///
/// stdin is written on a thread and then dropped, because a message larger than
/// the pipe buffer would otherwise deadlock the writer against the reader, and
/// the reader is the process we are timing.
pub(crate) fn run_bounded_stdin(
    argv: &[String],
    input: &str,
    limit: Duration,
) -> Result<i32, String> {
    let (bin, args) = argv.split_first().ok_or("empty command")?;
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run `{bin}`: {e}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("`{bin}` gave us no stdin to write to"))?;
    let text = input.to_string();
    thread::spawn(move || {
        let _ = stdin.write_all(text.as_bytes());
        let _ = stdin.flush();
        drop(stdin);
    });
    wait_bounded(&mut child, bin, limit)
}

/// Poll `child` until it is gone or the limit is spent.
fn wait_bounded(
    child: &mut std::process::Child,
    bin: &str,
    limit: Duration,
) -> Result<i32, String> {
    let started = Instant::now();
    loop {
        match child
            .try_wait()
            .map_err(|e| format!("could not wait for `{bin}`: {e}"))?
        {
            Some(status) => return Ok(status.code().unwrap_or(-1)),
            None if started.elapsed() >= limit => {
                // SIGKILL to the child; `KillMode=control-group` on the unit is
                // the backstop for anything it left behind.
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("gave up on `{bin}` after {}s", limit.as_secs()));
            }
            None => thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// The same, capturing stdout, for the one read worth waiting on.  stdout is
/// drained on a thread so a chatty child cannot fill a pipe and stall the exit
/// we are timing.
pub(crate) fn bounded_output(argv: &[String], limit: Duration) -> Result<String, String> {
    let (bin, args) = argv.split_first().ok_or("empty command")?;
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run `{bin}`: {e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("`{bin}` gave us no stdout to capture"))?;
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let mut stdout = stdout;
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
    });
    match rx.recv_timeout(limit) {
        Ok(text) => {
            let _ = child.wait();
            Ok(text)
        }
        Err(_) => {
            let _ = child.kill();
            Err(format!("nothing from `{bin}` after {}s", limit.as_secs()))
        }
    }
}

/// NUT's POWERDOWNFLAG.  Written here rather than left to `upsmon`, because the
/// grace-timer path never asks `upsmon` to power down — it asks *us* to.
pub(crate) fn write_killpower_flag(path: &Path) -> std::io::Result<()> {
    fs::write(
        path,
        "written by ups-watchdog: the fleet has been told to power off. NUT's \
         ups-killpower.service reads this file and runs `upsdrvctl shutdown`, which drops the \
         UPS output and returns it when mains come back.\n",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `sh` for the tests that need a shell.  `/bin/sh` is a symlink NixOS does
    /// create, but a test must not depend on that.
    fn sh() -> String {
        if Path::new("/bin/sh").exists() {
            "/bin/sh".to_string()
        } else {
            "sh".to_string()
        }
    }

    #[test]
    fn the_status_line_survives_both_upsc_output_shapes() {
        // `upsc <ups> ups.status` → the bare value, in both the NUT 2.8 (`OB`) and
        // the NUT 2.7 (`ups.status: OL`) shapes.
        assert_eq!(status_line("OB LB\n"), Some("OB LB"));
        assert_eq!(status_line("ups.status: OL CHRG\n"), Some("OL CHRG"));
        // `upsc <ups>` → a `key: value` dump, found by the prefix whatever order it
        // arrives in, with prose lines (colon-free but not flags) never mistaken
        // for the status.
        assert_eq!(
            status_line("Server apc  Upssd 1.0\nups.status: OB\n"),
            Some("OB")
        );
        assert_eq!(
            status_line("Server: apc\nAgent: NUT 2.8.4\nUps mode: Standalone\nups.status: OB\n"),
            Some("OB")
        );
        assert_eq!(status_line("Server apc\nbattery.charge: 90\n"), None);
        assert_eq!(status_line("OB\n"), Some("OB"));
        assert_eq!(status_line(""), None);
    }

    #[test]
    fn the_killpower_flag_says_who_wrote_it() {
        let path = std::env::temp_dir().join(format!("ups-watchdog-kp-{}", std::process::id()));
        write_killpower_flag(&path).expect("write");
        let text = fs::read_to_string(&path).expect("read");
        assert!(text.contains("ups-watchdog"), "{text}");
        assert!(text.contains("upsdrvctl"), "{text}");
        let _ = fs::remove_file(&path);
    }
    #[test]
    fn stdin_reaches_a_child_that_reads_it() {
        // `wall` reads a stream, not an argument.  If this ever stops working the
        // notices go somewhere nobody reads them.
        let code = run_bounded_stdin(
            &[sh(), "-c".to_string(), "cat > /dev/null".to_string()],
            "broadcast\n",
            Duration::from_secs(10),
        )
        .expect("ran");
        assert_eq!(code, 0);
    }

    #[test]
    fn a_command_that_cannot_start_is_an_error_not_a_zero() {
        let err = run_bounded(
            &["definitely-not-a-binary".to_string()],
            Duration::from_secs(1),
        )
        .expect_err("must fail");
        assert!(err.contains("definitely-not-a-binary"), "{err}");
    }

    #[test]
    fn the_wall_clock_limit_ends_a_hop_that_never_answers() {
        let started = Instant::now();
        let outcome = run_bounded(
            &["sleep".to_string(), "30".to_string()],
            Duration::from_millis(300),
        );
        let elapsed = started.elapsed();
        assert!(outcome.is_err(), "a 30s sleep must not finish inside 300ms");
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    }

    #[test]
    fn bounded_output_reads_a_child_that_writes_at_once() {
        let out = bounded_output(
            &[sh(), "-c".to_string(), "echo hi".to_string()],
            Duration::from_secs(10),
        )
        .expect("ran");
        assert_eq!(out.trim(), "hi");
    }

    #[test]
    fn bounded_output_gives_up_on_a_child_that_never_writes() {
        let started = Instant::now();
        let err = bounded_output(
            &[sh(), "-c".to_string(), "sleep 30".to_string()],
            Duration::from_millis(300),
        )
        .expect_err("must time out");
        assert!(err.contains("after"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }
}
