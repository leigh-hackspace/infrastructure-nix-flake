//! switch-gui — ports and VLANs on the network's ProCurve switches, in a browser.
//!
//! Three switches, one page each: `switch1` (10.3.1.2, J9280A 2510G-48, Y.11.44),
//! `switch2` (10.3.1.3, the same model) and `switch3` (10.3.1.4, a J4903A 2824 on
//! I.10.107). `/` lists them and `/switch/<name>` is one switch's dashboard.
//!
//! Why it exists: neither the switch's own web UI nor SNMP says "which VLAN is
//! port 17 on" without four commands, and the switch authenticates against
//! RADIUS so there is no useful shared login.  This reads the whole
//! configuration over Telnet, merges the four tables that split it up
//! ([`switch`]), and serves one page per switch.
//!
//! Three decisions worth knowing before changing anything:
//!
//! * **The switch state is cached, not fetched per request.** A full read is
//!   six-plus Telnet round-trips against a 1990s-era CPU that pages its own
//!   output; loading the page live would take seconds and would serialise
//!   behind every other reader.  So there is one snapshot **per switch** behind
//!   its own mutex, refreshed when it goes stale (`--ttl`) or when the Refresh
//!   button posts `/api/refresh`.  The page always says how old the numbers are,
//!   and a failed refresh keeps the last good snapshot rather than showing
//!   nothing.
//! * **One Telnet session per switch, behind its own mutex.** The CLI is a
//!   console: two concurrent command streams interleave their output.  Every
//!   read and write for a switch goes through that switch's `Slot::cli`.  The
//!   mutex is per switch, not global, so a slow read on the 2824 cannot hold up
//!   a page for a switch that answered normally, and a switch that is down or
//!   rebooting cannot take the other two with it.
//! * **Sign-in is authentik, gated on the `Infra` group** ([`oidc`]), because
//!   the switch itself authenticates against RADIUS and a local account here
//!   would be a second, weaker door to the same device.  `--dev-user` and
//!   `--no-auth` exist for testing and are refused on a non-loopback bind.
//!
//! Configuration comes from the environment (the systemd unit's
//! `EnvironmentFile`) with `--env-file` as a fallback, matching `dns-sync`.

mod api;
mod b64;
mod commands;
mod gui;
mod jsonw;
mod model;
mod oidc;
mod parse;
mod sha256;
mod switch;
mod telnet;

use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use common_web as web;

use model::{Privilege, Snapshot};
use oidc::Oidc;

type R<T> = std::result::Result<T, String>;

const DEFAULT_ENV: &str = "/var/lib/secrets/switch-gui.env";
const AUDIT_MAX: usize = 200;
/// The switches, as `name=address` pairs. The names are what the URLs use
/// (`/switch/switch2`), so they are the same as the switches' own `System Name`.
const DEFAULT_SWITCHES: &str = "switch1=10.3.1.2,switch2=10.3.1.3,switch3=10.3.1.4";
/// Cookie holding the OAuth `state` between `/auth/login` and `/auth/callback`.
const STATE_COOKIE: &str = "switch_login_state";

#[derive(Clone)]
struct Config {
    /// The switches this instance serves, in the order the index lists them.
    switches: Vec<switch::Config>,
    bind: String,
    ttl: Duration,
    read_only: bool,
    /// Sign in as this user without OIDC (loopback binds only).  `--no-auth`
    /// sets it to `dev`, so one option is enough to get a local session.
    dev_user: Option<String>,
}

struct AuditEntry {
    at: u64,
    /// Which switch the action went to. With three switches an audit line that
    /// does not name one is not evidence of anything.
    switch: String,
    user: String,
    action: String,
    result: String,
}

/// One switch's state: its own console and its own cache.
///
/// Per switch rather than shared, for two reasons. The CLI is one console per
/// switch, so a mutex shared across switches would make a slow read on the 2824
/// (which pages every table) hold up a page for a switch that answered normally.
/// And a switch that is down must not take the other two with it: each slot
/// keeps its own last good snapshot, so the index still shows what it knew.
struct Slot {
    cfg: switch::Config,
    ttl: Duration,
    /// `None` until the first connect; any failure puts `None` back so the next
    /// request reconnects — the switch ages idle consoles out, and another
    /// operator opening the console can drop us.
    cli: Mutex<Option<switch::Switch>>,
    cache: Mutex<Option<Snapshot>>,
    /// When the cached snapshot was taken.  Kept beside the snapshot rather
    /// than inside it so a failed refresh leaves the old one usable.
    cached_at: Mutex<Option<Instant>>,
    /// The last read failure, kept so the index can show a switch as down rather
    /// than as a switch with no ports.
    error: Mutex<Option<String>>,
}

struct Shared {
    cfg: Config,
    slots: Vec<Slot>,
    oidc: Option<Oidc>,
    audit: Mutex<VecDeque<AuditEntry>>,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("switch-gui: {e}");
        std::process::exit(1);
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn run() -> R<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut opt = Opts::default();
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        // Every flag that takes a value consumes the next argument; a missing
        // one is a usage error rather than a panic.
        let mut val = |what: &str| -> R<String> {
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("{what} needs a value"))
        };
        match flag {
            "--bind" => opt.bind = val(flag)?,
            "--env-file" => opt.env_file = val(flag)?,
            "--ttl" => {
                let v = val(flag)?;
                opt.ttl = v
                    .parse()
                    .map_err(|_| format!("--ttl needs a number of seconds, got `{v}`"))?
            }
            "--switches" => opt.switches = val(flag)?,
            "--dev-user" => opt.dev_user = Some(val(flag)?),
            "--read-only" => opt.read_only = true,
            "--no-auth" => opt.no_auth = true,
            "-h" | "--help" => {
                print!("{}", usage());
                return Ok(());
            }
            other => return Err(format!("unknown argument `{other}`\n\n{}", usage())),
        }
        i += 1;
    }

    let env = load_env(&opt.env_file);
    let get = |k: &str| -> Option<String> {
        std::env::var(k)
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| env.get(k).cloned())
    };
    let need = |k: &str| -> R<String> {
        get(k)
            .ok_or_else(|| format!("{k} is not set (in the environment or in {})", opt.env_file))
    };

    let spec = if opt.switches.is_empty() {
        get("SWITCH_GUI_SWITCHES").unwrap_or_else(|| DEFAULT_SWITCHES.to_string())
    } else {
        opt.switches.clone()
    };
    let pairs = parse_switches(&spec)?;
    // One Telnet port for all of them: every ProCurve CLI here answers on 23.
    let port: u16 = match get("SWITCH_GUI_PORT") {
        Some(v) => v
            .parse()
            .map_err(|_| format!("SWITCH_GUI_PORT is not a number: `{v}`"))?,
        None => 23,
    };
    let cli_timeout = Duration::from_secs(
        get("SWITCH_GUI_CLI_TIMEOUT")
            .and_then(|v| v.parse().ok())
            .unwrap_or(20),
    );
    // Longer than the command timeout, because a login goes out to RADIUS first
    // and the retry ladder can hold the console for 15-25 s before the switch
    // falls back to its local account database and prints the prompt. At 20 s this
    // read times out on `switch3`.
    let login_timeout = Duration::from_secs(
        get("SWITCH_GUI_LOGIN_TIMEOUT")
            .and_then(|v| v.parse().ok())
            .unwrap_or(45),
    );

    let login = need("SWITCH_GUI_LOGIN")?;
    let password = need("SWITCH_GUI_PASSWORD")?;

    // `enable` on this firmware is a second login, and the **same account**
    // works: `agent`/`agent` takes the prompt from `switch1>` to `switch1#`
    // (verified on 10.3.1.2, and the switch's own `aaa authentication telnet
    // enable radius local` says the enable path is authenticated the same way
    // as the login path).  So the enable pair defaults to the login pair and
    // writes work without a second secret; naming a genuinely different Manager
    // account is still possible with SWITCH_GUI_ENABLE_LOGIN/_PASSWORD.
    //
    // `--read-only` asks for the Manager context not at all, so the session
    // cannot write even if the switch would let it.
    let escalate = !opt.read_only;
    let enable_login = escalate.then(|| {
        get("SWITCH_GUI_ENABLE_LOGIN").unwrap_or_else(|| login.clone())
    });
    let enable_password = escalate.then(|| {
        get("SWITCH_GUI_ENABLE_PASSWORD").unwrap_or_else(|| password.clone())
    });

    let switches: Vec<switch::Config> = pairs
        .into_iter()
        .map(|(name, host)| switch::Config {
            name,
            host,
            port,
            login: login.clone(),
            password: password.clone(),
            enable_login: enable_login.clone(),
            enable_password: enable_password.clone(),
            timeout: cli_timeout,
            login_timeout,
        })
        .collect();

    // OIDC is on as soon as a client id and secret are present.
    let oidc_cfg = match (
        get("SWITCH_GUI_OIDC_CLIENT_ID"),
        get("SWITCH_GUI_OIDC_CLIENT_SECRET"),
    ) {
        (Some(client_id), Some(client_secret)) => Some(oidc::Config {
            client_id,
            client_secret,
            authorize_url: get("SWITCH_GUI_OIDC_AUTHORIZE")
                .unwrap_or_else(|| "https://id.leighhack.org/application/o/authorize/".into()),
            token_url: get("SWITCH_GUI_OIDC_TOKEN")
                .unwrap_or_else(|| "https://id.leighhack.org/application/o/token/".into()),
            introspect_url: get("SWITCH_GUI_OIDC_INTROSPECT")
                .unwrap_or_else(|| "https://id.leighhack.org/application/o/introspect/".into()),
            redirect_uri: get("SWITCH_GUI_OIDC_REDIRECT")
                .unwrap_or_else(|| "https://switch.int.leighhack.org/auth/callback".into()),
            required_group: get("SWITCH_GUI_REQUIRED_GROUP")
                .unwrap_or_else(|| "Infra".to_string()),
            cookie_name: get("SWITCH_GUI_COOKIE_NAME")
                .unwrap_or_else(|| "switch_session".to_string()),
            curl: get("SWITCH_GUI_CURL")
                .unwrap_or_else(|| "/run/current-system/sw/bin/curl".into()),
        }),
        _ => None,
    };

    let loopback = opt.bind.starts_with("127.")
        || opt.bind.starts_with("localhost")
        || opt.bind.starts_with("[::1]");
    if !loopback && (opt.dev_user.is_some() || opt.no_auth) {
        return Err(format!(
            "--dev-user/--no-auth treat every request as signed in, so they are refused on a \
             non-loopback bind (this one is {})",
            opt.bind
        ));
    }
    if oidc_cfg.is_none() && !loopback {
        return Err(format!(
            "no OIDC client configured (SWITCH_GUI_OIDC_CLIENT_ID/_SECRET) and {} is not \
             loopback: refusing to serve an unauthenticated switch console.",
            opt.bind
        ));
    }

    let cfg = Config {
        switches,
        bind: opt.bind.clone(),
        ttl: Duration::from_secs(opt.ttl),
        read_only: opt.read_only,
        dev_user: opt
            .dev_user
            .clone()
            .or_else(|| opt.no_auth.then(|| "dev".to_string())),
    };

    let slots: Vec<Slot> = cfg
        .switches
        .iter()
        .map(|s| Slot::new(s.clone(), cfg.ttl))
        .collect();

    let shared = Arc::new(Shared {
        cfg,
        slots,
        oidc: oidc_cfg.map(Oidc::new),
        audit: Mutex::new(VecDeque::new()),
    });

    // Fail fast if a switch is unreachable, but do not die: a switch that is
    // rebooting must not stop the service, and the page reports the error. One
    // switch that does not answer must not stop the others being read here.
    for slot in &shared.slots {
        match slot.snapshot(true) {
            Ok(s) => eprintln!(
                "switch-gui: {} read {} VLANs and {} ports ({})",
                slot.cfg.name,
                s.vlans.len(),
                s.ports.len(),
                slot.privilege().label()
            ),
            Err(e) => eprintln!(
                "switch-gui: initial read failed for {} (will keep retrying): {e}",
                slot.cfg.name
            ),
        }
    }

    let listener = TcpListener::bind(shared.cfg.bind.as_str())
        .map_err(|e| format!("cannot bind {}: {e}", shared.cfg.bind))?;
    println!("switch-gui: listening on {}", shared.cfg.bind);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let shared = Arc::clone(&shared);
                thread::spawn(move || {
                    if let Err(e) = serve(stream, &shared) {
                        eprintln!("switch-gui: {e}");
                    }
                });
            }
            Err(e) => eprintln!("switch-gui: accept failed: {e}"),
        }
    }
    Ok(())
}

struct Opts {
    bind: String,
    env_file: String,
    ttl: u64,
    switches: String,
    dev_user: Option<String>,
    read_only: bool,
    no_auth: bool,
}

/// The defaults are in one place so `usage()` cannot drift away from what
/// `run()` actually does — it had already drifted once (the help text printed
/// an empty bind).
impl Default for Opts {
    fn default() -> Opts {
        Opts {
            bind: "127.0.0.1:8097".to_string(),
            env_file: DEFAULT_ENV.to_string(),
            ttl: 30,
            switches: String::new(),
            dev_user: None,
            read_only: false,
            no_auth: false,
        }
    }
}

/// Where the "Switch UI" button points.
///
/// **http, not https.** The 2510G's web management serves plain HTTP — measured
/// on 10.3.1.2, `http://` answers 200 while `https://` does not complete a TLS
/// handshake at all — and this is only ever a link out of the page, so there is
/// no reason to pay for a redirect that the device cannot give us.
fn default_switch_url(host: &str) -> String {
    format!("http://{host}")
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

/// The switch list as the sign-in page shows it.
fn switch_list(shared: &Shared) -> String {
    shared
        .slots
        .iter()
        .map(|s| format!("{} ({})", s.cfg.name, s.cfg.host))
        .collect::<Vec<_>>()
        .join(", ")
}

fn usage() -> String {
    let d = Opts::default();
    format!(
        "\
switch-gui — ports and VLANs on the network switches, in a browser

  --bind <addr:port>     listen address            (default {default_bind})
  --env-file <path>      KEY=value secret file     (default {default_env})
  --ttl <secs>           re-read a switch after this long (default {default_ttl})
  --switches <list>      name=address pairs, comma separated
                         (default {default_switches})
  --read-only            never send a configuration command
  --dev-user <name>      skip sign-in (loopback binds only)
  --no-auth              accept any request (loopback binds only)

Required environment: SWITCH_GUI_LOGIN, SWITCH_GUI_PASSWORD
Optional:             SWITCH_GUI_SWITCHES ({default_switches}), SWITCH_GUI_PORT (23),
                      SWITCH_GUI_ENABLE_LOGIN, SWITCH_GUI_ENABLE_PASSWORD,
                      SWITCH_GUI_CLI_TIMEOUT (20), SWITCH_GUI_LOGIN_TIMEOUT (45),
                      SWITCH_GUI_REQUIRED_GROUP (Infra),
                      SWITCH_GUI_OIDC_CLIENT_ID, SWITCH_GUI_OIDC_CLIENT_SECRET,
                      SWITCH_GUI_OIDC_AUTHORIZE, SWITCH_GUI_OIDC_TOKEN,
                      SWITCH_GUI_OIDC_INTROSPECT, SWITCH_GUI_OIDC_REDIRECT,
                      SWITCH_GUI_CURL, SWITCH_GUI_COOKIE_NAME
",
        default_bind = d.bind,
        default_env = DEFAULT_ENV,
        default_ttl = d.ttl,
        default_switches = DEFAULT_SWITCHES,
    )
}

/// `KEY=value` lines, ignoring blanks and comments.  A missing file is not an
/// error: the environment may be enough, which is how the systemd unit works.
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

// ---------------------------------------------------------------- HTTP ----

fn serve(mut stream: TcpStream, shared: &Shared) -> R<()> {
    stream.set_read_timeout(Some(Duration::from_secs(60))).ok();
    stream.set_nodelay(true).ok();

    let Some(req) = web::read_request(&mut stream) else {
        return Ok(());
    };
    // Everything after the request head only writes, so a shared reference is
    // enough (and `read_request` is the only caller needing `&mut`).
    let stream = &stream;
    let path = req.path().to_string();
    let query = parse_query(&req.raw_path);
    let cookie_header = req.header("cookie").to_string();

    // Endpoints reachable signed out: the OIDC dance and the health check.
    if path.starts_with("/auth/") || path == "/healthz" {
        return route_auth(stream, shared, &path, &query, &cookie_header);
    }

    let session = match (&shared.oidc, &shared.cfg.dev_user) {
        // A dev user wins over a configured provider: on a loopback bind that
        // combination only happens because someone is testing without authentik.
        (_, Some(user)) => Some(oidc::Session {
            username: user.clone(),
            email: None,
            expires: i64::MAX,
        }),
        (Some(oidc), None) => cookie_value(&cookie_header, oidc.cookie_name())
            .as_deref()
            .and_then(|c| oidc.session(c)),
        (None, None) => None,
    };

    let Some(session) = session else {
        if shared.oidc.is_some() {
            // There is one page, so there is no destination worth preserving
            // across sign-in — and refusing a `next` parameter is also what
            // stops this becoming an open redirect.
            return redirect(stream, 302, "/auth/login", &[]);
        }
        return text(stream, 401, "sign-in is not configured on this instance");
    };
    let user = session.username.clone();

    if path == "/logout" {
        if let (Some(oidc), Some(c)) = (&shared.oidc, cookie_value(&cookie_header, shared.cookie_name())) {
            oidc.logout(&c);
        }
        return redirect(
            stream,
            302,
            "/",
            &[format!("{}=\"\"; Path=/; Max-Age=0", shared.cookie_name())],
        );
    }

    // Which switch this request is about: the path for a page, the `switch`
    // query for an action. An unknown name is a 404 rather than a quiet fallback
    // to the first switch — a mistyped URL that acts on the wrong device is the
    // one mistake this cannot walk back. An empty name (the index page, or a
    // request that names nothing) means the first configured switch.
    let name = match path.strip_prefix("/switch/") {
        Some(n) => n.to_string(),
        None => query.get("switch").cloned().unwrap_or_default(),
    };
    let Some(slot) = shared.slot(&name) else {
        return text(stream, 404, &format!("no switch named `{name}`"));
    };

    match (req.method.as_str(), path.as_str()) {
        ("GET", "/") | ("GET", "/index.html") => index_page(stream, shared, &session),
        ("GET", p) if p.starts_with("/switch/") => page(stream, shared, slot, &session),
        ("GET", "/api/snapshot") => {
            let snap = slot.snapshot(false)?;
            json(stream, 200, &api::snapshot_json(&snap))
        }
        ("POST", "/api/refresh") => match slot.snapshot(true) {
            Ok(s) => {
                shared.audit(&slot.cfg.name, &user, "refresh", "ok");
                text(stream, 200, &format!("refreshed: {} ports", s.ports.len()))
            }
            Err(e) => {
                shared.audit(&slot.cfg.name, &user, "refresh", &format!("error: {e}"));
                text(stream, 502, &format!("refresh failed: {e}"))
            }
        },
        ("POST", p) if p.starts_with("/api/port/") => {
            let rest = &p["/api/port/".len()..];
            let (port, verb) = match rest.split_once('/') {
                Some((a, b)) => (a, b),
                None => return text(stream, 404, "no such action"),
            };
            let v = query.get("v").map(|s| s.as_str()).unwrap_or("");
            let cmds = match verb {
                "name" => commands::port_name(port, v),
                "vlan" => match v.parse::<u32>() {
                    Ok(id) => commands::port_vlan(port, id),
                    Err(_) => Err(format!("`{v}` is not a VLAN id")),
                },
                "enabled" => match v {
                    "1" | "true" => commands::port_state(port, true),
                    "0" | "false" => commands::port_state(port, false),
                    _ => Err("expected ?v=1 or ?v=0".to_string()),
                },
                _ => return text(stream, 404, "no such action"),
            };
            apply(stream, shared, slot, &user, &format!("port {port} {verb}"), cmds)
        }
        ("POST", p) if p.starts_with("/api/vlan/") => {
            let rest = &p["/api/vlan/".len()..];
            let (id, verb) = match rest.split_once('/') {
                Some((a, b)) => (a, b),
                None => return text(stream, 404, "no such action"),
            };
            let id_num: u32 = match id.parse() {
                Ok(v) => v,
                Err(_) => return text(stream, 400, "the VLAN id in the URL must be a number"),
            };
            let cmds = match verb {
                "rename" => commands::vlan_rename(id_num, query.get("v").map(|s| s.as_str()).unwrap_or("")),
                "delete" => slot.check_delete(id_num),
                "tagged" => {
                    let ports = query.get("ports").cloned().unwrap_or_default();
                    let add = match query.get("v").map(|s| s.as_str()) {
                        Some("0") | Some("false") => false,
                        _ => true,
                    };
                    commands::vlan_tagged(id_num, &ports, add)
                }
                _ => return text(stream, 404, "no such action"),
            };
            apply(stream, shared, slot, &user, &format!("vlan {id_num} {verb}"), cmds)
        }
        ("POST", "/api/command") => {
            let cmd = query.get("cmd").cloned().unwrap_or_default();
            run_read_command(stream, shared, slot, &user, &cmd)
        }
        ("GET", "/api/audit") => json(stream, 200, &shared.audit_json()),
        _ => text(stream, 404, "not found"),
    }
}

/// The landing page: the switch list, from the cache only.
///
/// It does not read. A page that read all three switches on every hit would pay
/// six-plus Telnet round-trips each against CPUs that page their own output, and
/// the numbers it shows are the ones the last refresh produced — the page says
/// how old they are.
fn index_page(stream: &TcpStream, shared: &Shared, session: &oidc::Session) -> R<()> {
    let rows = shared.rows("");
    html(
        stream,
        200,
        &gui::index(&rows, &session.username, session.email.as_deref()),
    )
}

/// The dashboard: that switch's cache, refreshed first if it has gone stale.
fn page(stream: &TcpStream, shared: &Shared, slot: &Slot, session: &oidc::Session) -> R<()> {
    let snap = slot.snapshot(false)?;
    let privilege = slot.privilege();
    let rows = shared.rows(&slot.cfg.name);
    let url = default_switch_url(&slot.cfg.host);
    let page = gui::Page {
        name: &slot.cfg.name,
        snap: &snap,
        rows: &rows,
        privilege: privilege.label(),
        is_manager: privilege.is_manager() && !shared.cfg.read_only,
        user: &session.username,
        email: session.email.as_deref(),
        read_only: shared.cfg.read_only,
        cache_age_secs: slot.cache_age().as_secs(),
        switch_url: &url,
    };
    html(stream, 200, &gui::dashboard(&page))
}

/// Validate, refuse, send, audit.  Every write path ends here.
fn apply(
    stream: &TcpStream,
    shared: &Shared,
    slot: &Slot,
    user: &str,
    what: &str,
    cmds: R<Vec<String>>,
) -> R<()> {
    let cmds = match cmds {
        Ok(c) => c,
        // Rejected before reaching the switch: the caller's input was wrong,
        // so 400 with the reason rather than a 500.
        Err(e) => {
            shared.audit(&slot.cfg.name, user, what, &format!("refused: {e}"));
            return text(stream, 400, &e);
        }
    };
    if shared.cfg.read_only {
        let msg = "this instance runs with --read-only".to_string();
        shared.audit(&slot.cfg.name, user, what, &msg);
        return text(stream, 403, &msg);
    }
    if !slot.privilege().is_manager() {
        let msg = "the switch refused `enable`, so this session is in the Operator context and cannot configure it. The enable credentials default to the login account, which is a Manager here, so this means they were refused outright.";
        shared.audit(&slot.cfg.name, user, what, msg);
        return text(stream, 403, msg);
    }

    let body = match slot.run(&cmds) {
        Ok(()) => (200, format!("applied: {}", cmds.join(" ; "))),
        Err(e) => (502, format!("switch refused: {e}")),
    };
    shared.audit(&slot.cfg.name, user, what, &body.1);
    // The configuration may have changed either way — a failure halfway
    // through a plan is still a change — so force a re-read.
    slot.invalidate();
    text(stream, body.0, &body.1)
}

fn run_read_command(
    stream: &TcpStream,
    shared: &Shared,
    slot: &Slot,
    user: &str,
    raw: &str,
) -> R<()> {
    let cmd = match commands::read_command(raw) {
        Ok(c) => c,
        Err(e) => {
            shared.audit(
                &slot.cfg.name,
                user,
                &format!("console: {raw}"),
                &format!("refused: {e}"),
            );
            return text(stream, 400, &e);
        }
    };
    match slot.command(&cmd) {
        Ok(out) => {
            shared.audit(&slot.cfg.name, user, &format!("console: {cmd}"), "ok");
            text(stream, 200, &out)
        }
        Err(e) => {
            shared.audit(
                &slot.cfg.name,
                user,
                &format!("console: {cmd}"),
                &format!("error: {e}"),
            );
            text(stream, 502, &e)
        }
    }
}

fn route_auth(
    stream: &TcpStream,
    shared: &Shared,
    path: &str,
    query: &BTreeMap<String, String>,
    cookie_header: &str,
) -> R<()> {
    if path == "/healthz" {
        // The check is about this app, not about the switches: each switch
        // reports its own state, and one switch being rebooted should not make
        // the GUI read as down. `ok` means at least one switch has a snapshot.
        let items: Vec<String> = shared
            .slots
            .iter()
            .map(|s| {
                format!(
                    "{{\"name\":\"{}\",\"host\":\"{}\",\"ok\":{},\"cacheAge\":{},\"privilege\":\"{}\"}}",
                    crate::jsonw::escape(&s.cfg.name),
                    crate::jsonw::escape(&s.cfg.host),
                    s.has_cache(),
                    s.cache_age().as_secs(),
                    s.privilege().label()
                )
            })
            .collect();
        let ok = shared.slots.iter().any(|s| s.has_cache());
        return json(
            stream,
            if ok { 200 } else { 503 },
            &format!("{{\"ok\":{ok},\"switches\":[{}]}}", items.join(",")),
        );
    }

    let Some(oidc) = &shared.oidc else {
        return text(stream, 404, "sign-in is not configured on this instance");
    };

    match path {
        "/auth/login" => {
            let (state, url) = oidc.begin_login()?;
            redirect(
                stream,
                302,
                &url,
                &[format!("{STATE_COOKIE}=\"{state}\"; Path=/; Max-Age=600; SameSite=Lax")],
            )
        }
        "/auth/callback" => {
            let Some(code) = query.get("code") else {
                let err = query
                    .get("error_description")
                    .or_else(|| query.get("error"))
                    .cloned()
                    .unwrap_or_else(|| "authentik returned no code".to_string());
                return html(
                    stream,
                    401,
                    &gui::login("/auth/login", oidc.required_group(), &switch_list(shared), Some(&err)),
                );
            };
            let state = cookie_value(cookie_header, STATE_COOKIE).unwrap_or_default();
            // The state cookie is single-use: clear it whatever happens so a
            // replay of the callback URL cannot be reused.
            let clear = format!("{STATE_COOKIE}=\"\"; Path=/; Max-Age=0");
            match oidc.finish_login(code, &state) {
                Ok((session_cookie, session)) => {
                    shared.audit("", &session.username, "login", "ok");
                    redirect(
                        stream,
                        302,
                        "/",
                        &[
                            clear,
                            format!(
                                "{}=\"{session_cookie}\"; Path=/; HttpOnly; SameSite=Lax; Max-Age=43200",
                                oidc.cookie_name()
                            ),
                        ],
                    )
                }
                Err(e) => {
                    shared.audit("", "?", "login", &format!("denied: {}", e.0));
                    // The reason (e.g. "not a member of Infra") is worth
                    // showing, and comes from authentik or from our own gate.
                    html(
                        stream,
                        401,
                        &gui::login("/auth/login", oidc.required_group(), &switch_list(shared), Some(&e.0)),
                    )
                }
            }
        }
        _ => text(stream, 404, "not found"),
    }
}

// ------------------------------------------------------------ responses ----

fn send(stream: &TcpStream, status: u16, ctype: &str, body: &[u8]) -> R<()> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {len}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        reason = web::reason_phrase(status),
        len = body.len(),
    );
    let mut s = stream;
    s.write_all(head.as_bytes())
        .and_then(|_| s.write_all(body))
        .map_err(|e| format!("write response: {e}"))
}

fn text(stream: &TcpStream, status: u16, body: &str) -> R<()> {
    send(stream, status, "text/plain; charset=utf-8", body.as_bytes())
}

fn json(stream: &TcpStream, status: u16, body: &str) -> R<()> {
    send(stream, status, "application/json", body.as_bytes())
}

fn html(stream: &TcpStream, status: u16, body: &str) -> R<()> {
    send(stream, status, "text/html; charset=utf-8", body.as_bytes())
}

fn redirect(stream: &TcpStream, status: u16, location: &str, cookies: &[String]) -> R<()> {
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nLocation: {location}\r\nContent-Length: 0\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n",
        reason = web::reason_phrase(status)
    );
    for c in cookies {
        head.push_str(&format!("Set-Cookie: {c}\r\n"));
    }
    head.push_str("\r\n");
    let mut s = stream;
    s.write_all(head.as_bytes())
        .map_err(|e| format!("write redirect: {e}"))
}

/// `name=value` pairs from a `Cookie` header.
fn cookie_value(header: &str, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    for part in header.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix(prefix.as_str()) {
            let v = v.trim_matches('"');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// `a=1&b=2` into a map, percent-decoding both sides.
///
/// `+` is turned into a space **before** percent-decoding, which is the
/// form-urlencoded rule: it is what curl (`--data-urlencode -G`) and every HTML
/// form send for a space, and decoding first would turn a literal `%2B` into a
/// `+` and then wrongly into a space.  The GUI's own `encodeURIComponent` emits
/// `%20`, so both spellings have to work — a console command typed as
/// `show vlan 232` arrives as `show+vlan+232`, and reading it literally makes
/// the first word `show+vlan+232` and the command gets refused.
fn parse_query(raw_path: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let Some((_, q)) = raw_path.split_once('?') else {
        return map;
    };
    for pair in q.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(
            web::percent_decode(&k.replace('+', " ")),
            web::percent_decode(&v.replace('+', " ")),
        );
    }
    map
}

// --------------------------------------------------------------- state ----

impl Shared {
    fn cookie_name(&self) -> &str {
        match &self.oidc {
            Some(o) => o.cookie_name(),
            None => "switch_session",
        }
    }

    /// The slot for a switch name. An empty name means the first configured
    /// switch, which is what a request with no `switch` parameter gets.
    fn slot(&self, name: &str) -> Option<&Slot> {
        if name.is_empty() {
            return self.slots.first();
        }
        self.slots.iter().find(|s| s.cfg.name == name)
    }

    /// The switch list for the index page and the nav bar.
    ///
    /// Cache only, never a read: this is on every page, and reading three
    /// switches to draw a nav bar would cost more than the page it is on.
    fn rows(&self, current: &str) -> Vec<gui::SwitchRow> {
        self.slots
            .iter()
            .map(|s| {
                let snap = s
                    .cache
                    .lock()
                    .ok()
                    .and_then(|g| g.as_ref().cloned())
                    .unwrap_or_default();
                gui::SwitchRow {
                    name: s.cfg.name.clone(),
                    host: s.cfg.host.clone(),
                    current: s.cfg.name == current,
                    system_name: snap.system.name.clone(),
                    location: snap.system.location.clone(),
                    software: snap.system.software.clone(),
                    ports: snap.ports.len(),
                    vlans: snap.vlans.len(),
                    cache_age_secs: s.cache_age().as_secs(),
                    error: s.error.lock().ok().and_then(|g| g.clone()),
                }
            })
            .collect()
    }

    fn audit(&self, switch: &str, user: &str, action: &str, result: &str) {
        let at = now_secs();
        if let Ok(mut g) = self.audit.lock() {
            if g.len() >= AUDIT_MAX {
                g.pop_front();
            }
            g.push_back(AuditEntry {
                at,
                switch: switch.to_string(),
                user: user.to_string(),
                action: action.to_string(),
                result: result.to_string(),
            });
        }
    }

    fn audit_json(&self) -> String {
        let Ok(g) = self.audit.lock() else {
            return "[]".to_string();
        };
        let items: Vec<String> = g
            .iter()
            .map(|e| {
                format!(
                    "{{\"at\":{},\"switch\":\"{}\",\"user\":\"{}\",\"action\":\"{}\",\"result\":\"{}\"}}",
                    e.at,
                    crate::jsonw::escape(&e.switch),
                    crate::jsonw::escape(&e.user),
                    crate::jsonw::escape(&e.action),
                    crate::jsonw::escape(&e.result)
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }
}

impl Slot {
    fn new(cfg: switch::Config, ttl: Duration) -> Slot {
        Slot {
            cfg,
            ttl,
            cli: Mutex::new(None),
            cache: Mutex::new(None),
            cached_at: Mutex::new(None),
            error: Mutex::new(None),
        }
    }

    fn privilege(&self) -> Privilege {
        self.cli
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|s| s.privilege))
            .unwrap_or(Privilege::Operator)
    }

    fn cache_age(&self) -> Duration {
        self.cached_at
            .lock()
            .ok()
            .and_then(|g| g.map(|t| t.elapsed()))
            .unwrap_or(Duration::from_secs(u64::MAX))
    }

    fn has_cache(&self) -> bool {
        self.cache.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Forget the snapshot and the session, forcing a reconnect and a re-read.
    fn invalidate(&self) {
        if let Ok(mut g) = self.cached_at.lock() {
            *g = None;
        }
        if let Ok(mut g) = self.cli.lock() {
            *g = None;
        }
    }

    /// The cached snapshot, refreshed when older than the TTL (or on demand).
    fn snapshot(&self, force: bool) -> R<Snapshot> {
        if !force && self.cache_age() <= self.ttl {
            if let Some(snap) = self.cache.lock().ok().and_then(|g| g.as_ref().cloned()) {
                return Ok(snap);
            }
        }
        let snap = self.read()?;
        if let Ok(mut g) = self.cache.lock() {
            *g = Some(snap.clone());
        }
        if let Ok(mut g) = self.cached_at.lock() {
            *g = Some(Instant::now());
        }
        Ok(snap)
    }

    /// Read the whole configuration, reconnecting once if the console had
    /// died: every command of a dead session fails, which shows up as an empty
    /// snapshot with warnings rather than as an error.
    fn read(&self) -> R<Snapshot> {
        let mut last = String::new();
        for attempt in 0..2 {
            let mut guard = self
                .cli
                .lock()
                .map_err(|_| "the CLI lock was poisoned".to_string())?;
            if guard.is_none() {
                *guard = Some(switch::Switch::connect(&self.cfg)?);
            }
            let sw = guard.as_mut().unwrap();
            let mut snap = switch::read_snapshot(sw)?;
            if snap.ports.is_empty() && !snap.warnings.is_empty() {
                last = snap.warnings.join("; ");
                if attempt == 0 {
                    eprintln!(
                        "switch-gui: {} read failed on the cached session ({last}); reconnecting",
                        self.cfg.name
                    );
                    drop(guard);
                    self.invalidate();
                    continue;
                }
            }
            snap.taken_at_secs = now_secs();
            // A successful read clears the outage: the index must not keep
            // showing a switch as down after it came back.
            if let Ok(mut g) = self.error.lock() {
                *g = None;
            }
            return Ok(snap);
        }
        if let Ok(mut g) = self.error.lock() {
            *g = Some(last.clone());
        }
        Err(if last.is_empty() {
            format!("could not read {}", self.cfg.name)
        } else {
            last
        })
    }

    /// Run one command on this switch's session, reconnecting once if the switch
    /// had dropped the console out from under us.
    fn command(&self, cmd: &str) -> R<String> {
        let mut guard = self
            .cli
            .lock()
            .map_err(|_| "the CLI lock was poisoned".to_string())?;
        if let Some(sw) = guard.as_mut() {
            match sw.command(cmd) {
                Ok(out) => return Ok(out),
                Err(e) => {
                    eprintln!(
                        "switch-gui: {} `{cmd}` failed on the cached session ({e}); reconnecting",
                        self.cfg.name
                    );
                }
            }
        }
        *guard = None;
        let mut sw = switch::Switch::connect(&self.cfg)?;
        let out = sw.command(cmd)?;
        *guard = Some(sw);
        Ok(out)
    }

    /// Send a sequence of configuration commands, stopping at the first
    /// failure: a half-applied change is bad enough without also applying the
    /// rest of a plan that no longer makes sense.
    fn run(&self, cmds: &[String]) -> R<()> {
        let mut guard = self
            .cli
            .lock()
            .map_err(|_| "the CLI lock was poisoned".to_string())?;
        if guard.is_none() {
            *guard = Some(switch::Switch::connect(&self.cfg)?);
        }
        let sw = guard.as_mut().unwrap();
        let out = sw.apply(cmds);
        if out.is_err() {
            // A refused command can leave the console part-way through config
            // mode, and the next caller would read that as a broken parser.
            // Drop the session and let the next use reconnect.
            *guard = None;
        }
        out
    }

    /// The delete command for a VLAN, checked against the cached membership.
    /// With no cache yet, only the range is checked — the switch itself still
    /// refuses to delete a VLAN that has ports.
    fn check_delete(&self, id: u32) -> R<Vec<String>> {
        match self.cache.lock().ok().and_then(|g| g.as_ref().cloned()) {
            Some(snap) => commands::vlan_delete(id, &snap),
            None => commands::check_vlan(id).map(|_| vec![format!("no vlan {id}")]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_are_parsed_out_of_the_header() {
        let h = "a=1; switch_session=\"abc\"; other=x";
        assert_eq!(cookie_value(h, "switch_session").as_deref(), Some("abc"));
        assert_eq!(cookie_value(h, "missing"), None);
        assert_eq!(cookie_value(h, "a").as_deref(), Some("1"));
        // An empty value is no value: it must not look like a session.
        assert_eq!(cookie_value("switch_session=\"\"", "switch_session"), None);
    }

    #[test]
    fn query_strings_are_decoded() {
        let q = parse_query("/api/port/5/name?v=NAS2%20Port%201");
        assert_eq!(q.get("v").map(String::as_str), Some("NAS2 Port 1"));
        assert!(parse_query("/api/snapshot").is_empty());
        let q = parse_query("/x?a=1&b&c=2%2F3");
        assert_eq!(q.get("b").map(String::as_str), Some(""));
        assert_eq!(q.get("c").map(String::as_str), Some("2/3"));
    }

    /// A form-encoded space is a `+`, and curl's `-G --data-urlencode` sends
    /// one; treating it literally makes `show vlan 232` a command called
    /// `show+vlan+232`, which the console then refuses.
    #[test]
    fn query_strings_decode_plus_as_space() {
        let q = parse_query("/api/command?cmd=show+vlan+232");
        assert_eq!(q.get("cmd").map(String::as_str), Some("show vlan 232"));
        // ...while an encoded plus stays a plus.
        let q = parse_query("/api/port/1/name?v=a%2Bb");
        assert_eq!(q.get("v").map(String::as_str), Some("a+b"));
        let q = parse_query("/api/vlan/5/rename?v=AD+Lab");
        assert_eq!(q.get("v").map(String::as_str), Some("AD Lab"));
    }

    /// The switch has no HTTPS listener, so the link out of the page must not
    /// assume one.
    #[test]
    fn the_switch_ui_link_is_http() {
        assert_eq!(default_switch_url("10.3.1.2"), "http://10.3.1.2");
    }

    #[test]
    fn env_files_ignore_comments_and_quotes() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("switch-gui-env-{}", std::process::id()));
        std::fs::write(
            &path,
            "# a comment\n\nSWITCH_GUI_LOGIN=agent\nSWITCH_GUI_PASSWORD=\"p@ss word\"\nnope\n",
        )
        .unwrap();
        let env = load_env(path.to_str().unwrap());
        assert_eq!(env.get("SWITCH_GUI_LOGIN").map(String::as_str), Some("agent"));
        assert_eq!(
            env.get("SWITCH_GUI_PASSWORD").map(String::as_str),
            Some("p@ss word")
        );
        assert!(!env.contains_key("nope"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_missing_env_file_is_not_an_error() {
        assert!(load_env("/nonexistent/switch-gui.env").is_empty());
    }

    #[test]
    fn usage_lists_the_flags() {
        let u = usage();
        assert!(u.contains("--read-only"));
        assert!(u.contains("SWITCH_GUI_ENABLE_PASSWORD"));
        assert!(u.contains("127.0.0.1:8097"));
        assert!(u.contains("--switches"));
        assert!(u.contains(DEFAULT_SWITCHES));
    }

    #[test]
    fn the_switch_list_is_name_address_pairs() {
        let p = parse_switches(DEFAULT_SWITCHES).unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(p[0], ("switch1".to_string(), "10.3.1.2".to_string()));
        assert_eq!(p[2], ("switch3".to_string(), "10.3.1.4".to_string()));
    }

    /// The name is the URL, so a name that cannot be put in one is refused at
    /// startup rather than halfway through a request.
    #[test]
    fn a_switch_name_that_cannot_be_a_url_is_refused() {
        assert!(parse_switches("switch 2=10.3.1.3").is_err());
        assert!(parse_switches("switch2=").is_err());
        assert!(parse_switches("switch2").is_err());
        assert!(parse_switches("").is_err());
        // A duplicate would give two switches one page, and the audit log would
        // not say which one a command went to.
        assert!(parse_switches("switch1=10.3.1.2,switch1=10.3.1.3").is_err());
    }

    #[test]
    fn the_switch_list_is_the_default_when_nothing_is_given() {
        // Whitespace and empty items are tolerated: this is a shell-supplied
        // string, and `"switch1=10.3.1.2, switch2=10.3.1.3"` is what someone
        // will actually write.
        let p = parse_switches(" switch1=10.3.1.2 , switch2=10.3.1.3 , ").unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[1].1, "10.3.1.3");
    }
}
