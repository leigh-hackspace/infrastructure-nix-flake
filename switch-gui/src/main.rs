//! switch-gui — ports and VLANs on the network's ProCurve switches, in a browser.
//!
//! Three switches, one page each: `switch1` (10.3.1.2, J9280A 2510G-48, Y.11.44),
//! `switch2` (10.3.1.3, the same model) and `switch3` (10.3.1.4, a J4903A 2824 on
//! I.10.107).  `/` lists them and `/switch/<name>` is one switch's dashboard.
//!
//! Why it exists: neither the switch's own web UI nor SNMP says "which VLAN is
//! port 17 on" without four commands, and the switch authenticates against RADIUS
//! so there is no useful shared login.  This reads the whole configuration over
//! Telnet, merges the five tables that split it up ([`switch`]), and shows one
//! page per switch.
//!
//! ## Where the data comes from now
//!
//! The page used to read the switches.  It does not any more, and the shape of
//! the program follows from that:
//!
//! ```text
//!   browser ──HTTP──▶ routes ──▶ Postgres ──▶ sync ──Telnet──▶ switches
//!                       │                        ▲
//!                       └──────queue row─────────┘
//! ```
//!
//! * **Reads** ([`routes`]) answer from the **shadow copy** in Postgres
//!   ([`shadow`]) that [`sync`] keeps in step.  The page is a handful of indexed
//!   queries, so it loads in milliseconds, every tab sees the same state, and a
//!   switch that is unreachable costs a warning on the page instead of a spinner.
//! * **Writes** ([`intent`]) are validated against the shadow copy and appended
//!   to a **queue** ([`changes`]).  The request returns as soon as the row is in:
//!   the page says *pending sync*, and the row says so too — with who asked, what
//!   CLI lines it became, what the switch said, and how many times the sync tried.
//!   That row is also the audit trail, and the thing a Revert is built from.
//!
//! Four decisions worth knowing before changing anything:
//!
//! * **One console session per switch, and only the sync may hold it.** The CLI
//!   is a console: two command streams interleave their output.  Each switch has
//!   its own session behind its own mutex, so a slow read on the 2824 cannot hold
//!   up a page for a switch that answered normally, and a switch that is down or
//!   rebooting cannot take the other two with it.
//! * **Nothing writes to NVRAM.** `write` is this firmware's save command and this
//!   tool never sends it, so a reboot undoes every change.  The page says so.
//! * **Sign-in is authentik, gated on the `Infra` group** ([`common_oidc`]),
//!   because the switch itself authenticates against RADIUS and a local account
//!   here would be a second, weaker door to the same device.  `--dev-user` and
//!   `--no-auth` exist for testing and are refused on a non-loopback bind.
//! * **The Telnet client and the CLI parsers stay dependency-free.** They are the
//!   reason the tool exists; a crate that buffers a line the way the 2824 pages
//!   its own output would put the switches back behind a wall.
//!
//! Configuration comes from the environment (the systemd unit's
//! `EnvironmentFile`) with `--env-file` as a fallback, matching `dns-sync`.

mod changes;
mod commands;
mod intent;
mod model;
mod parse;
mod routes;
mod shadow;
mod switch;
mod sync;
mod telnet;

/// The wire types, shared with the SPA (`dto/`, the same split as
/// gocardless-dashboard's `dto`).  A re-export rather than a module so every
/// handler can write `dto::Page` and the crate name stays out of the way.
pub use switch_gui_dto as dto;

use std::collections::BTreeMap;
use std::io::Write;
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

// The asset table build.rs generates into $OUT_DIR (`find_asset`).
include!(concat!(env!("OUT_DIR"), "/assets_gen.rs"));

type R<T> = std::result::Result<T, String>;

const DEFAULT_ENV: &str = "/var/lib/secrets/.switch-gui-env";

/// What a `--read-only` instance says when something asks it to write.
///
/// One sentence, three callers: `POST /api/intent`, the change-row actions, and the
/// sync failing a drained batch.  They used to be three wordings of the same
/// refusal, which meant an operator could press two buttons and read two different
/// explanations of one flag.
pub const READ_ONLY: &str =
    "this instance runs with --read-only: it reads the switches and will not write to them";

fn main() {
    if let Err(e) = run() {
        eprintln!("switch-gui: {e}");
        std::process::exit(1);
    }
}

#[derive(Debug, Clone)]
struct Opts {
    bind: String,
    env_file: String,
    switches: String,
    /// How the Postgres shadow copy is reached.  Empty means the local socket,
    /// which is what the unit uses: the database is on this box, and a DSN with a
    /// password in the unit's environment would be a secret in the journal on
    /// every `systemctl show`.
    db_url: String,
    /// How often the shadow copy is re-read.  The queue's own latency is
    /// [`sync::TICK`], not this: a change you just made is applied in about two
    /// seconds whatever this is set to.
    interval: u64,
    /// How many times the sync tries a change before marking it failed.
    max_attempts: i32,
    dev_user: Option<String>,
    read_only: bool,
    no_auth: bool,
}

/// The defaults are in one place so `usage()` cannot drift away from what `run()`
/// actually does.
impl Default for Opts {
    fn default() -> Opts {
        Opts {
            bind: "127.0.0.1:8097".to_string(),
            env_file: DEFAULT_ENV.to_string(),
            switches: String::new(),
            db_url: String::new(),
            interval: 30,
            max_attempts: 5,
            dev_user: None,
            read_only: false,
            no_auth: false,
        }
    }
}

fn usage() -> String {
    let o = Opts::default();
    format!(
        "switch-gui — manage the ProCurve switches from a browser\n\n\
         \x20 usage: switch-gui [flags]\n\n\
         \x20  --bind <a:p>        listen address (default {bind})\n\
         \x20  --env-file <path>   KEY=VALUE file (default {env})\n\
         \x20  --db-url <dsn>      Postgres DSN (default: local socket, database\n\
         \x20                      switch_gui, as the service's own role)\n\
         \x20  --interval <secs>   re-read each switch this often (default {interval};\n\
         \x20                      queued changes are applied on a {tick}s tick)\n\
         \x20  --max-attempts <n>  tries per queued change before it is failed\n\
         \x20  --read-only         never write to a switch (queue refusals are logged)\n\
         \x20  --dev-user <name>   serve as this user without signing in (testing only)\n\
         \x20  --no-auth           no sign-in at all (testing only, loopback bind only)\n\n\
         \x20 Environment (or the env file): SWITCH_GUI_SWITCHES, SWITCH_GUI_LOGIN,\n\
         \x20 SWITCH_GUI_PASSWORD, SWITCH_GUI_OIDC_CLIENT_ID, SWITCH_GUI_OIDC_CLIENT_SECRET,\n\
         \x20 SWITCH_GUI_DB_URL; optional SWITCH_GUI_ENABLE_LOGIN/_PASSWORD, which\n\
         \x20 default to the login pair because `enable` is a second login for the\n\
         \x20 same account on this firmware.\n",
        bind = o.bind,
        env = o.env_file,
        interval = o.interval,
        tick = sync::TICK.as_secs(),
    )
}

fn run() -> R<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut opt = Opts::default();
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        // Every flag that takes a value consumes the next argument; a missing one
        // is a usage error rather than a panic.
        let mut val = |what: &str| -> R<String> {
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("{what} needs a value\n\n{}", usage()))
        };
        match flag {
            "--help" | "-h" => {
                print!("{}", usage());
                return Ok(());
            }
            "--bind" => opt.bind = val("--bind")?,
            "--env-file" => opt.env_file = val("--env-file")?,
            "--db-url" => opt.db_url = val("--db-url")?,
            "--interval" => {
                opt.interval = val("--interval")?
                    .parse()
                    .map_err(|_| "--interval wants a number of seconds".to_string())?
            }
            "--max-attempts" => {
                opt.max_attempts = val("--max-attempts")?
                    .parse()
                    .map_err(|_| "--max-attempts wants a number".to_string())?
            }
            "--dev-user" => opt.dev_user = Some(val("--dev-user")?),
            "--read-only" => opt.read_only = true,
            "--no-auth" => opt.no_auth = true,
            other => return Err(format!("unknown flag {other}\n\n{}", usage())),
        }
        i += 1;
    }

    let env = load_env(&opt.env_file);
    let env_get = |k: &str| {
        std::env::var(k)
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| env.get(k).cloned())
    };

    if opt.switches.is_empty() {
        opt.switches = env_get("SWITCH_GUI_SWITCHES").unwrap_or_default();
    }
    if opt.db_url.is_empty() {
        opt.db_url = env_get("SWITCH_GUI_DB_URL").unwrap_or_default();
    }
    let switches = parse_switches(&opt.switches)?;

    // The switch credentials are required, and required means *fail at startup*
    // rather than authenticate badly forever: an empty password produces a login
    // failure on every pass, which reads like a wrong password in the journal and
    // not at all like the missing secret it is.
    let need = |k: &str| -> R<String> {
        env_get(k).ok_or_else(|| format!("{k} is not set (in the environment or in {})", opt.env_file))
    };
    let login = need("SWITCH_GUI_LOGIN")?;
    let password = need("SWITCH_GUI_PASSWORD")?;
    let (enable_login, enable_password) = enable_creds(
        env_get("SWITCH_GUI_ENABLE_LOGIN"),
        env_get("SWITCH_GUI_ENABLE_PASSWORD"),
        &login,
        &password,
    );

    // The testing escape hatches.  `--no-auth` treats every request as a local
    // session, which on a bind anyone else can reach is a switch with no login;
    // `--dev-user` is the same door with a name on it.  Both are refused unless
    // the bind is loopback, and this is checked before the listener opens so a
    // misconfigured unit fails at start rather than after serving one request.
    if (opt.no_auth || opt.dev_user.is_some()) && !is_loopback_bind(&opt.bind) {
        return Err(format!(
            "{} needs a loopback --bind (got {})",
            if opt.no_auth {
                "--no-auth"
            } else {
                "--dev-user"
            },
            opt.bind
        ));
    }

    let oidc = match (
        env_get("SWITCH_GUI_OIDC_CLIENT_ID"),
        env_get("SWITCH_GUI_OIDC_CLIENT_SECRET"),
    ) {
        (Some(id), Some(secret)) => Some(common_oidc::Oidc::new(
            common_oidc::OidcConfig {
                // The authentik endpoints and the group gate are the same for
                // every app on this network (see docs/authentik-oidc-notes.md);
                // only the client pair and the redirect URI differ.
                authorize_url: "https://id.leighhack.org/application/o/authorize/".to_string(),
                token_url: "https://id.leighhack.org/application/o/token/".to_string(),
                introspect_url: "https://id.leighhack.org/application/o/introspect/".to_string(),
                client_id: id,
                client_secret: secret,
                redirect_uri: "https://switch.int.leighhack.org/auth/callback".to_string(),
                required_group: "Infra".to_string(),
                cookie_name: "switch_session".to_string(),
            },
            reqwest::Client::new(),
        )),
        (None, None) => None,
        _ => {
            return Err(
                "SWITCH_GUI_OIDC_CLIENT_ID and SWITCH_GUI_OIDC_CLIENT_SECRET come as a pair"
                    .to_string(),
            )
        }
    };
    if oidc.is_none() && opt.dev_user.is_none() && !opt.no_auth {
        return Err(
            "no sign-in configured: set the OIDC client pair, or --dev-user on a loopback bind"
                .to_string(),
        );
    }

    // The async half of the program: the pool and the sync loop.  The HTTP side
    // stays on plain threads and `block_on`s into here, which is how
    // gocardless-dashboard is built and what keeps the Telnet code synchronous.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio: {e}"))?;
    let rt_handle = rt.handle().clone();

    let dsn = if opt.db_url.is_empty() {
        default_dsn()
    } else {
        opt.db_url.clone()
    };
    let pool = rt_handle.block_on(common_pg::connect(&dsn, 8))?;
    rt_handle.block_on(shadow::migrate(
        &pool,
        &switches
            .iter()
            .map(|(n, a)| (n.clone(), a.clone()))
            .collect::<Vec<_>>(),
    ))?;
    rt_handle.block_on(changes::migrate(&pool))?;
    // A previous instance may have died mid-write; those rows would otherwise sit
    // "applying" forever, showing a pending notice nobody is working on.
    match rt_handle.block_on(changes::recover_stale(&pool, sync::STALE)) {
        Ok(0) => {}
        Ok(n) => eprintln!("switch-gui: put {n} interrupted change(s) back in the queue"),
        Err(e) => eprintln!("switch-gui: {e}"),
    }

    let devices: Vec<Arc<sync::Device>> = switches
        .iter()
        .map(|(name, address)| {
            Arc::new(sync::Device {
                cfg: switch::Config {
                    name: name.clone(),
                    host: address.clone(),
                    port: 23,
                    login: login.clone(),
                    password: password.clone(),
                    timeout: Duration::from_secs(20),
                    login_timeout: Duration::from_secs(45),
                    read_only: opt.read_only,
                    enable_login: enable_login.clone(),
                    enable_password: enable_password.clone(),
                },
                cli: std::sync::Mutex::new(None),
            })
        })
        .collect();
    if opt.read_only {
        eprintln!("switch-gui: --read-only, the queue will not be written to switches");
    }

    let shared = Arc::new(routes::Shared {
        rt: rt_handle.clone(),
        pool: pool.clone(),
        oidc,
        dev_user: opt.dev_user.clone(),
        no_auth: opt.no_auth,
        read_only: opt.read_only,
        max_attempts: opt.max_attempts,
    });

    sync::spawn(
        &rt_handle,
        pool.clone(),
        devices.clone(),
        Duration::from_secs(opt.interval),
    );

    let listener = TcpListener::bind(&opt.bind).map_err(|e| format!("bind {}: {e}", opt.bind))?;
    println!(
        "switch-gui: http://{} — {} switch(es), shadow copy in postgres, reading every {}s",
        opt.bind,
        switches.len(),
        opt.interval
    );
    for d in &devices {
        println!("  {} = {}", d.cfg.name, d.cfg.host);
    }

    // One thread per connection, as it always was: the handlers are short (a few
    // indexed queries) and the accept loop must never wait behind a slow client.
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let shared = shared.clone();
                thread::spawn(move || {
                    if let Err(e) = serve(stream, &shared) {
                        eprintln!("switch-gui: connection: {e}");
                    }
                });
            }
            Err(e) => eprintln!("switch-gui: accept: {e}"),
        }
    }
    Ok(())
}

/// One connection: read the request, route it, write the reply.
fn serve(mut stream: TcpStream, shared: &routes::Shared) -> R<()> {
    // A client that stops mid-request must not pin a thread forever; 60 s is
    // longer than a browser needs for a request this small.
    stream.set_read_timeout(Some(Duration::from_secs(60))).ok();
    stream.set_nodelay(true).ok();

    let Some(req) = common_web::read_request(&mut stream) else {
        return Ok(());
    };
    let reply = routes::handle(&req, shared).unwrap_or_else(|e| {
        // A handler that fails should say so rather than close the socket: the
        // SPA shows the message, and a bare hang is what sends someone to the
        // journal for a message that could have been on screen.  `json!` rather
        // than a hand-built object because these messages quote the switch, and a
        // quote in one of them would otherwise break the JSON around it.
        common_web::Reply::json(
            500,
            &serde_json::json!({ "error": format!("{e:?}") }).to_string(),
        )
    });
    // The peer is allowed to hang up mid-write (a browser that gave up on a
    // request); that is not an error worth a line in the journal.
    let _ = reply.send(&mut stream);
    let _ = stream.flush();
    Ok(())
}

/// The credentials for `enable`: what was configured, or the login pair.
///
/// `enable` on this firmware is a **second login** — it re-asks for a login name,
/// not just a password — and the same `agent` account answers it, taking the prompt
/// from `switch1>` to `switch1#` (verified on 10.3.1.2, and
/// `aaa authentication telnet enable radius local` says the enable path is
/// authenticated exactly like the login path). So one secret in the env file is
/// enough and writes work; naming a genuinely different Manager account is still
/// possible with `SWITCH_GUI_ENABLE_LOGIN`/`_PASSWORD`.
///
/// This default is **not** tied to `--read-only`, which is how it used to be gated,
/// and that gating is why it broke twice: on this firmware the *read* needs Manager
/// as well, because port names come only from `show running-config` and that command
/// is refused in the Operator context. Staying in Operator to be safe about writes
/// would cost the port names and put every page in the "read-only session" state.
/// `--read-only` is enforced where it belongs — the queue refuses the row
/// (`routes::post_intent`) and the sync fails anything already queued
/// (`sync::pass`) — not by crippling the session that reads.
pub fn enable_creds(
    login_override: Option<String>,
    password_override: Option<String>,
    login: &str,
    password: &str,
) -> (Option<String>, Option<String>) {
    (
        Some(login_override.unwrap_or_else(|| login.to_string())),
        Some(password_override.unwrap_or_else(|| password.to_string())),
    )
}

/// The DSN used when `--db-url` is not given: the local socket, this service's
/// own role, whose name is its database.
///
/// No password, because the socket is `trust` for local connections on this box
/// and a DSN with a secret in it ends up in the journal on every `systemctl show`
/// (and in the unit's environment, which is world-readable).  A remote database
/// is what `--db-url`/`SWITCH_GUI_DB_URL` is for, and then the caller decides how
/// the password is passed.
///
/// **The socket path is percent-encoded as the host**, which is sqlx's convention
/// and not libpq's: the psql-style `postgresql://user@/db?host=/run/postgresql`
/// parses to an empty host and sqlx refuses it with `error with configuration:
/// empty host` (measured against the live server, 2026-10-13 — the form this
/// function used to return, which meant the unit could not start at all).  `%2F` is
/// `/`, so this reads as `…@/run/postgresql/switch_gui` once decoded.
fn default_dsn() -> String {
    "postgresql://switch_gui@%2Frun%2Fpostgresql/switch_gui".to_string()
}

/// `switch1=10.3.1.2,switch2=10.3.1.3` into `(name, address)` pairs.
///
/// The name is what the URLs use (`/switch/switch2`), so it is checked for the
/// characters that would make a path ambiguous, and duplicates are refused: two
/// switches sharing a name would share one page, and the audit log would not say
/// which one a command went to.
fn parse_switches(spec: &str) -> R<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();
    for item in spec.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let (name, host) = item
            .split_once('=')
            .ok_or_else(|| format!("`{item}` is not `name=address`"))?;
        let name = name.trim();
        let host = host.trim();
        if name.is_empty() || host.is_empty() {
            return Err(format!("`{item}` has an empty name or address"));
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(format!("switch name `{name}` cannot be used in a URL"));
        }
        if out.iter().any(|(n, _)| n == name) {
            return Err(format!("switch name `{name}` is used twice"));
        }
        out.push((name.to_string(), host.to_string()));
    }
    if out.is_empty() {
        return Err("no switches given (SWITCH_GUI_SWITCHES)".to_string());
    }
    Ok(out)
}

/// Whether a bind address is loopback, which is what the testing escape hatches
/// require.  `0.0.0.0` and a bare `:8097` are not loopback and are refused.
fn is_loopback_bind(bind: &str) -> bool {
    let host = if let Some(rest) = bind.strip_prefix('[') {
        // A bracketed host is taken whole: the brackets are there precisely so an
        // IPv6 address's own colons cannot read as a port separator.
        rest.split(']').next().unwrap_or(rest)
    } else {
        // Unbracketed, the host ends at the *first* colon that has a port after it
        // (`127.0.0.1:8097`).  Splitting at the *last* colon would cut `::1` down to
        // `:` and refuse a bind that works, and splitting at all would refuse the
        // bare IPv6 forms entirely.
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
        // A hostname is resolved by `bind` later; refusing `localhost` by name
        // would be a footgun, so it is accepted, and so is anything else — the
        // alternative is refusing a working configuration.
        Err(_) => host == "localhost",
    }
}

/// Read a `KEY=VALUE` env file.  Missing is not an error: the environment may
/// have everything (that is how the unit runs).
fn load_env(path: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return map;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(v);
            map.insert(k.trim().to_string(), v.to_string());
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The escape hatches are only ever safe on loopback, and the check is the
    /// only thing standing between a mistyped unit and an unauthenticated switch.
    #[test]
    fn only_a_loopback_bind_may_skip_sign_in() {
        assert!(is_loopback_bind("127.0.0.1:8097"));
        assert!(is_loopback_bind("127.0.0.1:0"));
        assert!(is_loopback_bind("[::1]:8097"));
        assert!(is_loopback_bind("localhost:8097"));
        assert!(!is_loopback_bind("0.0.0.0:8097"));
        assert!(!is_loopback_bind("10.3.1.20:8097"));
        assert!(!is_loopback_bind(":8097"), "an empty host binds everything");
    }

    /// The enable pair defaults to the login pair, because `enable` is a second
    /// login for the *same* account on this firmware and the deployment stores one
    /// secret. Losing this default is not a cosmetic regression: without `enable`
    /// the session is Operator, `show running-config` is refused, and the port
    /// names disappear from the page (seen live on 2026-10-13 after a rewrite).
    #[test]
    fn enable_defaults_to_the_login_account() {
        let (l, p) = enable_creds(None, None, "agent", "secret");
        assert_eq!(l.as_deref(), Some("agent"));
        assert_eq!(p.as_deref(), Some("secret"));

        // A named Manager account still wins, and half a pair stays half a pair:
        // `switch.rs` enables when either is set, so the other falls back.
        let (l, _) = enable_creds(Some("manager".into()), None, "agent", "secret");
        assert_eq!(l.as_deref(), Some("manager"));
        let (_, p) = enable_creds(Some("manager".into()), None, "agent", "secret");
        assert_eq!(p.as_deref(), Some("secret"));
    }

    /// The default DSN has to be one **sqlx** can parse, not one libpq can.
    ///
    /// The psql-style socket DSN (`postgresql://u@/db?host=/run/postgresql`) is what
    /// this function used to return, and sqlx rejects it with `error with
    /// configuration: empty host` — so the unit could not start, which is only
    /// discoverable by pointing it at a real server. Asserting both halves is the
    /// point: the shape we use, and the shape that looks right and is not.
    #[test]
    fn the_default_dsn_is_one_sqlx_can_parse() {
        use sqlx::postgres::PgConnectOptions;
        use std::str::FromStr;

        let dsn = default_dsn();
        let opts = PgConnectOptions::from_str(&dsn).expect("the default DSN must parse");
        // sqlx decodes the host and then normalises the socket directory to its own
        // default, `/var/run/postgresql`.  That is still the right socket on this
        // box: `/var/run` is a symlink to `/run` on NixOS, and this is the DSN a live
        // server accepted (the assertion is on the decoded host, not the text).
        assert!(
            opts.get_host().ends_with("run/postgresql"),
            "{} -> {}",
            dsn,
            opts.get_host()
        );
        assert_eq!(opts.get_database(), Some("switch_gui"), "{dsn}");

        assert!(
            PgConnectOptions::from_str("postgresql://switch_gui@/switch_gui?host=/run/postgresql")
                .is_err(),
            "if this ever parses, the test above is testing nothing"
        );
    }

    /// `[::1]` has a colon in the host, so the port split must be the *last* one.
    #[test]
    fn an_ipv6_bind_still_reads_as_loopback() {
        assert!(is_loopback_bind("[::1]:8097"));
        assert!(!is_loopback_bind("[::]:8097"));
    }

    #[test]
    fn the_switch_list_is_parsed_strictly() {
        let s = parse_switches("switch1=10.3.1.2, switch2=10.3.1.3").unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[1], ("switch2".to_string(), "10.3.1.3".to_string()));
        assert!(parse_switches("").is_err());
        assert!(parse_switches("switch1").is_err());
        assert!(parse_switches("switch1=").is_err());
        assert!(parse_switches("a=1,a=2").is_err(), "a name used twice");
        assert!(parse_switches("a b=1").is_err(), "a name with a space");
        assert!(
            parse_switches("a/../b=1").is_err(),
            "a name that escapes a path"
        );
    }

    /// The default DSN must carry no password: it is built from the local socket
    /// and this service's own role.  `postgresql://` has a colon in it, so the
    /// check is the one that actually means something — a userinfo of `user:pass`
    /// is the form that would leak, and `redact` is what rewrites it.
    #[test]
    fn the_default_dsn_holds_no_secret() {
        let dsn = default_dsn();
        assert!(!dsn.contains("password"), "{dsn}");
        // Percent-encoded, because that is how sqlx wants a socket path in the host
        // position (see `default_dsn`).
        assert!(dsn.contains("%2Frun%2Fpostgresql"), "{dsn}");
        assert!(
            common_pg::redact(&dsn) == dsn,
            "{dsn} should need no redaction"
        );
    }

    /// The help text is generated from the defaults, so it cannot name a flag
    /// that does not exist or omit one that does.
    #[test]
    fn usage_lists_every_flag() {
        let u = usage();
        for flag in [
            "--bind",
            "--env-file",
            "--db-url",
            "--interval",
            "--max-attempts",
            "--read-only",
            "--dev-user",
            "--no-auth",
        ] {
            assert!(u.contains(flag), "{flag} missing from usage()");
        }
    }

    #[test]
    fn the_env_file_is_optional_and_quoting_is_stripped() {
        let map = load_env("/nonexistent/path/writes/nothing");
        assert!(map.is_empty());
    }
}
