//! Getting one message to every machine the router is about to power off.
//!
//! Without these notices this crate would be a shutdown script that lies about
//! having warned people.  A fleet-wide power-off driven by a box three switches
//! away is *unfair* unless the humans on those boxes had a chance to say no, and
//! the alternative to a broadcast is the journal, which nobody reads until the
//! building is already dark.
//!
//! The message is composed in `plan::compose_broadcast` (so its shape is a tested
//! decision rather than a `printf`) and delivered over the same ssh hop the
//! poweroff uses.  Two deliberate choices live here:
//!
//! * **The text travels on stdin, never in the remote command line.** `wall` reads
//!   a stream, and interpolating a message into a string that a remote shell will
//!   parse is the kind of quoting bug that only shows up during an outage — which
//!   is also why `--wall-command` is fixed text from the unit, not a template.
//! * **A missed recipient is a non-zero exit, but never a stopped shutdown.** In
//!   `run` the notice is a courtesy owed to the living; in `broadcast` mode it is
//!   the whole job, so the exit code there is the finding (`systemctl is-failed
//!   ups-notice` is the check that the building was actually told).

use crate::exec::run_bounded_stdin;
use crate::plan::{ssh_command, SshOpts};
use crate::{targets, Opts};

use std::fs;
use std::time::Duration;

/// Where a broadcast mode message comes from: the file `ups-trigger` wrote (line
/// 1 is the headline), or the flags, which is how `run` carries its own.
pub(crate) fn notice_text(opts: &Opts) -> Result<(String, Vec<String>), String> {
    if let Some(path) = &opts.notice_file {
        match fs::read_to_string(path) {
            Ok(text) => {
                let mut lines = text.lines().map(|l| l.trim_end().to_string());
                let headline = lines
                    .next()
                    .filter(|l| !l.trim().is_empty())
                    .unwrap_or_else(|| "NOTICE".to_string());
                return Ok((headline, lines.filter(|l| !l.trim().is_empty()).collect()));
            }
            Err(why) => {
                return Err(format!("cannot read {}: {why}", path.display()));
            }
        }
    }
    let headline = opts
        .broadcast_headline
        .clone()
        .ok_or("broadcast needs --notice-file or --broadcast-headline")?;
    Ok((headline, opts.broadcast_messages.clone()))
}

/// Send `text` to every target and, when asked, to this box's own terminals.
///
/// The message travels on the child's **stdin**, never inside the remote command
/// line: `wall` reads a stream, and interpolating a message into a string that a
/// remote shell will parse is the kind of quoting bug that only appears during an
/// outage.  `--wall-command` is fixed text from the unit.
///
/// Returns how many recipients did not take it.  For a *notice*, ssh's 255 is a
/// miss — unlike the poweroff, where it is what a box going down under the hop
/// looks like, here nothing was supposed to close the connection.
pub(crate) fn send_notice(opts: &Opts, text: &str) -> usize {
    let mut missed = 0usize;
    let limit = Duration::from_secs(opts.broadcast_timeout);

    if opts.local_wall {
        let argv = vec![opts.local_wall_bin.clone(), "-n".to_string()];
        if opts.dry_run {
            println!("ups-watchdog: [dry-run] local wall — {}", argv.join(" "));
        } else {
            match run_bounded_stdin(&argv, text, limit) {
                Ok(0) => println!("ups-watchdog:   gw: notified"),
                Ok(code) => {
                    missed += 1;
                    eprintln!("ups-watchdog:   gw: local wall exited {code}");
                }
                Err(why) => {
                    missed += 1;
                    eprintln!("ups-watchdog:   gw: {why}");
                }
            }
        }
    }

    let (targets, _findings) = targets(opts);
    // Same hop as the poweroff, different remote verb.
    let wall = SshOpts {
        remote_command: opts.wall_command.clone(),
        ..opts.ssh.clone()
    };
    for t in &targets {
        let argv = ssh_command(t, &wall, opts.connect_timeout);
        if opts.dry_run {
            println!("ups-watchdog: [dry-run] {} — {}", t.name, argv.join(" "));
            continue;
        }
        match run_bounded_stdin(&argv, text, limit) {
            Ok(0) => println!("ups-watchdog:   {}: notified", t.name),
            Ok(code) => {
                missed += 1;
                eprintln!("ups-watchdog:   {}: ssh exited {code}", t.name);
            }
            Err(why) => {
                missed += 1;
                eprintln!("ups-watchdog:   {}: {why}", t.name);
            }
        }
    }
    missed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::compose_broadcast;

    #[test]
    fn a_notice_file_is_a_headline_and_a_body() {
        let dir = std::env::temp_dir().join(format!("ups-watchdog-notice-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("tmpdir");
        let file = dir.join("notice");
        fs::write(
            &file,
            "POWER OUTAGE DETECTED\nOn battery at 2026-10-28T20:32:26Z.\nThe servers power off in 300s unless mains return.\n\n",
        )
        .expect("write");
        let (headline, body) = notice_text(&Opts {
            notice_file: Some(file.clone()),
            ..Opts::default()
        })
        .expect("readable");
        assert_eq!(headline, "POWER OUTAGE DETECTED");
        assert_eq!(body.len(), 2, "{body:?}");
        let text = compose_broadcast(&headline, &body, None);
        assert!(
            text.starts_with("=== gw UPS: POWER OUTAGE DETECTED ==="),
            "{text}"
        );
        assert!(text.contains("unless mains return"), "{text}");

        // A one-line notice is legal, and the blank lines the dispatcher's `tr`
        // can leave behind must not become empty body lines.
        fs::write(&file, "MAINS RESTORED\n\n").expect("write");
        let (headline, body) = notice_text(&Opts {
            notice_file: Some(file),
            ..Opts::default()
        })
        .expect("readable");
        assert_eq!(headline, "MAINS RESTORED");
        assert!(body.is_empty(), "{body:?}");
        let _ = fs::remove_dir_all(&dir);
    }
}
