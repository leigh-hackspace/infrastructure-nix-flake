//! The parts of starting a service that are not the service.
//!
//! Every program here is a systemd unit that is handed its configuration the same
//! three ways — `--flag value` arguments, an `EnvironmentFile` with the secrets,
//! and a `--bind` address — and each of those has a right way to parse that is not
//! the obvious one.  They were written three times over (filestore,
//! gocardless-dashboard, switch-gui) and had already started to disagree, which is
//! how a `--no-auth` escape hatch ends up trusting a hostname that only *looks*
//! like a loopback address.  They live here so there is one behaviour to audit.
//!
//! ```
//! use common_service as svc;
//!
//! let args: Vec<String> = vec!["prog".into(), "--bind".into(), "127.0.0.1:8096".into()];
//! let argv: Vec<String> = args.iter().skip(1).cloned().collect();
//! assert_eq!(svc::arg(&argv, "--bind", Some("0.0.0.0")).as_str(), "127.0.0.1:8096");
//! assert!(svc::is_loopback("127.0.0.1:8096"));
//! ```

use std::net::IpAddr;

/// One parsed `EnvironmentFile`: names to values, in name order.
pub type Env = std::collections::BTreeMap<String, String>;

/// Read a systemd `EnvironmentFile` (`KEY=VALUE` per line).
///
/// Blank lines and `#` comments are skipped, and whitespace around the name, the
/// value and the `=` is not part of the value — systemd's own generator is
/// whitespace-insensitive there, and a secret with a trailing space is a secret
/// that does not authenticate.
///
/// One pair of *matching* surrounding double quotes is removed (`KEY="value"`
/// yields `value`), because that is how these files are written by hand.  Only a
/// matching pair: stripping a leading and a trailing quote independently would eat
/// one half of `KEY="a, 'b"'`, and a value that really begins and ends with a
/// quote character is worth more than the convenience.
///
/// A missing file is an error here; [`env_file_if_present`] is the version for a
/// service that can run without one.
pub fn env_file(path: &str) -> Result<Env, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("cannot read env file {path}: {e}"))?;
    Ok(parse_env(&text))
}

/// [`env_file`], with a missing or unreadable file reading as no entries.
///
/// For the case where the file is optional — `--no-auth` on a loopback bind needs
/// no OIDC credentials, so their absence is a configuration, not a fault.
pub fn env_file_if_present(path: &str) -> Env {
    env_file(path).unwrap_or_default()
}

fn parse_env(text: &str) -> Env {
    let mut env = Env::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            env.insert(k.trim().to_string(), unquote(v.trim()).to_string());
        }
    }
    env
}

fn unquote(v: &str) -> &str {
    v.strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(v)
}

/// The value of `--name <value>` in a program's arguments (everything after
/// argv[0]), or `default`.
///
/// Exits with status 2 when the flag is required and absent, which is what both
/// callers did by hand: a missing required flag is a usage error, and the process
/// has nothing useful left to do.  Use [`arg_opt`] where absence is a state the
/// program handles.
pub fn arg(args: &[String], name: &str, default: Option<&str>) -> String {
    arg_opt(args, name).or_else(|| default.map(str::to_string)).unwrap_or_else(|| {
        eprintln!("missing required argument {name}");
        std::process::exit(2);
    })
}

/// The value of `--name <value>`, or `None`.  A flag present with no value after
/// it reads as absent rather than panicking: the usage text is the place that
/// explains why.
pub fn arg_opt(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Does `--bind <addr>` name a loopback address?
///
/// The answer gates the testing-only escape hatches (`--no-auth`), so it has to be
/// the address that is loopback, not a string that looks like one: `127.example.com`
/// is a name anyone can point at a routable interface, and `::1234` is not `::1`.
/// Anything that does not parse as an address is therefore only accepted as
/// `localhost` — a name the resolver controls, which is the same trade-off `bind`
/// itself makes later.
///
/// Accepts every form a unit might pass: `127.0.0.1`, `127.0.0.1:8096`, `::1`,
/// `[::1]:8096`, `localhost`, `localhost:8096`.
pub fn is_loopback(bind: &str) -> bool {
    let bind = bind.trim();
    let host = if let Some(rest) = bind.strip_prefix('[') {
        // A bracketed host is taken whole: the brackets are there precisely so an
        // IPv6 address's own colons cannot read as a port separator.
        rest.split(']').next().unwrap_or(rest)
    } else {
        // Unbracketed, the host ends at the *first* colon that has a numeric port
        // after it (`127.0.0.1:8096`).  Splitting at the *last* colon would cut
        // `::1` down to `:` and refuse a bind that works, and splitting at all
        // would refuse the bare IPv6 forms entirely.
        match bind.split_once(':') {
            Some((h, port))
                if !h.is_empty()
                    && !port.is_empty()
                    && port.bytes().all(|b| b.is_ascii_digit()) =>
            {
                h
            }
            _ => bind,
        }
    };
    match host.parse::<IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => host.eq_ignore_ascii_case("localhost"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    fn write(name: &str, body: &str) -> String {
        let path = std::env::temp_dir().join(format!(
            "common-service-env-{}-{name}.env",
            std::process::id()
        ));
        std::fs::write(&path, body).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn env_file_reads_assignments_and_skips_the_rest() {
        let path = write(
            "read",
            "# the switch login\n\n  SWITCH_GUI_LOGIN = agent\nSWITCH_GUI_PASSWORD=\"hunter2\"\n\
             # SWITCH_GUI_PASSWORD=old\n",
        );
        let env = env_file(&path).unwrap();
        assert_eq!(env.get("SWITCH_GUI_LOGIN").map(String::as_str), Some("agent"));
        assert_eq!(env.get("SWITCH_GUI_PASSWORD").map(String::as_str), Some("hunter2"));
        assert_eq!(env.len(), 2, "the commented-out line must not come back");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn env_file_strips_one_matching_pair_of_quotes_and_nothing_else() {
        let path = write(
            "quotes",
            "A=\"quoted\"\nB=\"half\nC=half\"\nD=\"\"a\"b\"\"\nE=a=b\n",
        );
        let env = env_file(&path).unwrap();
        assert_eq!(env.get("A").map(String::as_str), Some("quoted"));
        // An unpaired quote is part of the value: eating it would silently change
        // a secret that happens to start with one.
        assert_eq!(env.get("B").map(String::as_str), Some("\"half"));
        assert_eq!(env.get("C").map(String::as_str), Some("half\""));
        // Only the outer pair goes, so an embedded quote survives.
        assert_eq!(env.get("D").map(String::as_str), Some("\"a\"b\""));
        // A value may contain `=`; the first one is the separator.
        assert_eq!(env.get("E").map(String::as_str), Some("a=b"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_missing_env_file_is_an_error_unless_asked_otherwise() {
        let path = "/nonexistent/common-service.env";
        assert!(env_file(path).is_err(), "a unit whose secret vanished must say so");
        assert!(env_file(path).unwrap_err().contains(path));
        assert!(env_file_if_present(path).is_empty());
    }

    #[test]
    fn args_read_the_value_after_the_flag() {
        let args = argv(&["--bind", "127.0.0.1:8096", "--no-auth"]);
        assert_eq!(arg_opt(&args, "--bind").as_deref(), Some("127.0.0.1:8096"));
        assert_eq!(arg_opt(&args, "--no-auth"), None, "a flag is not a value");
        assert_eq!(arg_opt(&args, "--root"), None);
        assert_eq!(arg(&args, "--bind", Some("0.0.0.0")), "127.0.0.1:8096");
        assert_eq!(arg(&args, "--root", Some("/srv")), "/srv");
    }

    #[test]
    fn a_flag_as_the_last_argument_is_absent_not_a_panic() {
        let args = argv(&["--bind"]);
        assert_eq!(arg_opt(&args, "--bind"), None);
    }

    #[test]
    fn loopback_covers_every_form_a_unit_hands_over() {
        for bind in [
            "127.0.0.1",
            "127.0.0.1:8096",
            "127.42.1.9:1",
            "::1",
            "[::1]",
            "[::1]:8097",
            "localhost",
            "localhost:8096",
            "LOCALHOST",
            " 127.0.0.1:8096 ",
        ] {
            assert!(is_loopback(bind), "{bind} is loopback");
        }
    }

    #[test]
    fn loopback_refuses_what_only_looks_like_loopback() {
        for bind in [
            "0.0.0.0",
            "0.0.0.0:8096",
            "10.3.1.20:8096",
            // A name anyone can point at a routable interface.
            "127.example.com",
            "127.0.0.1.evil.com",
            // `::1` with trailing digits is a different address entirely.
            "::1234",
            "[::1234]:8096",
            // The unspecified address: every interface, so every client.
            "[::]:8096",
            // An empty host binds everything too, colon and port notwithstanding.
            ":8097",
            "",
        ] {
            assert!(!is_loopback(bind), "{bind} is not loopback");
        }
    }

    #[test]
    fn an_unparseable_host_is_only_accepted_as_localhost() {
        // A name is not an address, so the only one trusted by name is the one the
        // resolver is guaranteed to point at this machine.  Anything else is not
        // loopback, port or no port.
        assert!(!is_loopback("switch-box"));
        assert!(!is_loopback("switch-box:8097"));
        assert!(!is_loopback("localhost.evil.com"));
    }
}
