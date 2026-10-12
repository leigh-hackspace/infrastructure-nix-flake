//! switch-gui — ports and VLANs on the ProCurve at 10.3.1.2, in a browser.
//!
//! Why it exists: neither the switch's own web UI nor SNMP says "which VLAN is
//! port 17 on" without four commands, and the switch authenticates against
//! RADIUS so there is no useful shared login.  This reads the whole
//! configuration over Telnet, merges the four tables that split it up
//! ([`switch`]), and serves one page.
//!
//! Three decisions worth knowing before changing anything:
//!
//! * **The switch state is cached, not fetched per request.** A full read is
//!   six-plus Telnet round-trips against a 1990s-era CPU that pages its own
//!   output; loading the page live would take seconds and would serialise
//!   behind every other reader.  So there is one snapshot behind a mutex,
//!   refreshed when it goes stale (`--ttl`) or when the Refresh button posts
//!   `/api/refresh`.  The page always says how old the numbers are, and a
//!   failed refresh keeps the last good snapshot rather than showing nothing.
//! * **One Telnet session, behind a mutex.** The CLI is a console: two
//!   concurrent command streams interleave their output.  Every read and write
//!   goes through `Shared::cli`, which is why the cache buys latency.
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
/// Cookie holding the OAuth `state` between `/auth/login` and `/auth/callback`.
const STATE_COOKIE: &str = "switch_login_state";

#[derive(Clone)]
struct Config {
    switch: switch::Config,
    bind: String,
    ttl: Duration,
    read_only: bool,
    /// Where the switch's own management UI lives, linked from the page.
    switch_url: String,
    /// Sign in as this user without OIDC (loopback binds only).  `--no-auth`
    /// sets it to `dev`, so one option is enough to get a local session.
    dev_user: Option<String>,
}

struct AuditEntry {
    at: u64,
    user: String,
    action: String,
    result: String,
}

struct Shared {
    cfg: Config,
    /// The one CLI session.  `None` until the first connect; any failure puts
    /// `None` back so the next request reconnects — the switch ages idle
    /// consoles out, and another operator opening the console can drop us.
    cli: Mutex<Option<switch::Switch>>,
    cache: Mutex<Option<Snapshot>>,
    /// When the cached snapshot was taken.  Kept beside the snapshot rather
    /// than inside it so a failed refresh leaves the old one usable.
    cached_at: Mutex<Option<Instant>>,
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
            "--switch-url" => opt.switch_url = val(flag)?,
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

    let host = get("SWITCH_GUI_HOST").unwrap_or_else(|| "10.3.1.2".to_string());
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

    let switch_cfg = switch::Config {
        host: host.clone(),
        port,
        login: need("SWITCH_GUI_LOGIN")?,
        password: need("SWITCH_GUI_PASSWORD")?,
        enable_login: get("SWITCH_GUI_ENABLE_LOGIN"),
        enable_password: get("SWITCH_GUI_ENABLE_PASSWORD"),
        timeout: cli_timeout,
    };

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
        switch: switch_cfg,
        bind: opt.bind.clone(),
        ttl: Duration::from_secs(opt.ttl),
        read_only: opt.read_only,
        switch_url: if opt.switch_url.is_empty() {
            default_switch_url(&host)
        } else {
            opt.switch_url.clone()
        },
        dev_user: opt
            .dev_user
            .clone()
            .or_else(|| opt.no_auth.then(|| "dev".to_string())),
    };

    let shared = Arc::new(Shared {
        cfg,
        cli: Mutex::new(None),
        cache: Mutex::new(None),
        cached_at: Mutex::new(None),
        oidc: oidc_cfg.map(Oidc::new),
        audit: Mutex::new(VecDeque::new()),
    });

    // Fail fast if the switch is unreachable, but do not die: a switch that is
    // rebooting must not stop the service, and the page reports the error.
    match shared.snapshot(true) {
        Ok(s) => eprintln!(
            "switch-gui: read {} VLANs and {} ports from {} ({})",
            s.vlans.len(),
            s.ports.len(),
            host,
            shared.privilege().label()
        ),
        Err(e) => eprintln!("switch-gui: initial read failed (will keep retrying): {e}"),
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
    switch_url: String,
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
            switch_url: String::new(),
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

fn usage() -> String {
    let d = Opts::default();
    format!(
        "\
switch-gui — ports and VLANs on the network switch, in a browser

  --bind <addr:port>    listen address            (default {default_bind})
  --env-file <path>     KEY=value secret file     (default {default_env})
  --ttl <secs>          re-read the switch after this long (default {default_ttl})
  --switch-url <url>    link to the switch's own UI
  --read-only           never send a configuration command
  --dev-user <name>     skip sign-in (loopback binds only)
  --no-auth             accept any request (loopback binds only)

Required environment: SWITCH_GUI_LOGIN, SWITCH_GUI_PASSWORD
Optional:             SWITCH_GUI_HOST (10.3.1.2), SWITCH_GUI_PORT (23),
                      SWITCH_GUI_ENABLE_LOGIN, SWITCH_GUI_ENABLE_PASSWORD,
                      SWITCH_GUI_CLI_TIMEOUT (20), SWITCH_GUI_REQUIRED_GROUP (Infra),
                      SWITCH_GUI_OIDC_CLIENT_ID, SWITCH_GUI_OIDC_CLIENT_SECRET,
                      SWITCH_GUI_OIDC_AUTHORIZE, SWITCH_GUI_OIDC_TOKEN,
                      SWITCH_GUI_OIDC_INTROSPECT, SWITCH_GUI_OIDC_REDIRECT,
                      SWITCH_GUI_CURL, SWITCH_GUI_COOKIE_NAME
",
        default_bind = d.bind,
        default_env = DEFAULT_ENV,
        default_ttl = d.ttl,
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

    match (req.method.as_str(), path.as_str()) {
        ("GET", "/") | ("GET", "/index.html") => page(stream, shared, &session),
        ("GET", "/api/snapshot") => {
            let snap = shared.snapshot(false)?;
            json(stream, 200, &api::snapshot_json(&snap))
        }
        ("POST", "/api/refresh") => match shared.snapshot(true) {
            Ok(s) => {
                shared.audit(&user, "refresh", "ok");
                text(stream, 200, &format!("refreshed: {} ports", s.ports.len()))
            }
            Err(e) => {
                shared.audit(&user, "refresh", &format!("error: {e}"));
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
                "description" => commands::port_description(port, v),
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
            apply(stream, shared, &user, &format!("port {port} {verb}"), cmds)
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
                "delete" => shared.check_delete(id_num),
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
            apply(stream, shared, &user, &format!("vlan {id_num} {verb}"), cmds)
        }
        ("POST", "/api/command") => {
            let cmd = query.get("cmd").cloned().unwrap_or_default();
            run_read_command(stream, shared, &user, &cmd)
        }
        ("GET", "/api/audit") => json(stream, 200, &shared.audit_json()),
        _ => text(stream, 404, "not found"),
    }
}

/// The dashboard: the cache, refreshed first if it has gone stale.
fn page(stream: &TcpStream, shared: &Shared, session: &oidc::Session) -> R<()> {
    let snap = shared.snapshot(false)?;
    let privilege = shared.privilege();
    let page = gui::Page {
        snap: &snap,
        privilege: privilege.label(),
        is_manager: privilege.is_manager() && !shared.cfg.read_only,
        user: &session.username,
        email: session.email.as_deref(),
        read_only: shared.cfg.read_only,
        cache_age_secs: shared.cache_age().as_secs(),
        switch_url: &shared.cfg.switch_url,
    };
    html(stream, 200, &gui::dashboard(&page))
}

/// Validate, refuse, send, audit.  Every write path ends here.
fn apply(stream: &TcpStream, shared: &Shared, user: &str, what: &str, cmds: R<Vec<String>>) -> R<()> {
    let cmds = match cmds {
        Ok(c) => c,
        // Rejected before reaching the switch: the caller's input was wrong,
        // so 400 with the reason rather than a 500.
        Err(e) => {
            shared.audit(user, what, &format!("refused: {e}"));
            return text(stream, 400, &e);
        }
    };
    if shared.cfg.read_only {
        let msg = "this instance runs with --read-only".to_string();
        shared.audit(user, what, &msg);
        return text(stream, 403, &msg);
    }
    if !shared.privilege().is_manager() {
        let msg = "the switch put this session in the Operator (read-only) context, so it will \
                   not accept configuration — set SWITCH_GUI_ENABLE_LOGIN and \
                   SWITCH_GUI_ENABLE_PASSWORD to a Manager account"
            .to_string();
        shared.audit(user, what, &msg);
        return text(stream, 403, &msg);
    }

    let body = match shared.run(&cmds) {
        Ok(()) => (200, format!("applied: {}", cmds.join(" ; "))),
        Err(e) => (502, format!("switch refused: {e}")),
    };
    shared.audit(user, what, &body.1);
    // The configuration may have changed either way — a failure halfway
    // through a plan is still a change — so force a re-read.
    shared.invalidate();
    text(stream, body.0, &body.1)
}

fn run_read_command(stream: &TcpStream, shared: &Shared, user: &str, raw: &str) -> R<()> {
    let cmd = match commands::read_command(raw) {
        Ok(c) => c,
        Err(e) => {
            shared.audit(user, &format!("console: {raw}"), &format!("refused: {e}"));
            return text(stream, 400, &e);
        }
    };
    match shared.command(&cmd) {
        Ok(out) => {
            shared.audit(user, &format!("console: {cmd}"), "ok");
            text(stream, 200, &out)
        }
        Err(e) => {
            shared.audit(user, &format!("console: {cmd}"), &format!("error: {e}"));
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
        let ok = shared.cache.lock().map(|g| g.is_some()).unwrap_or(false);
        let body = format!(
            "{{\"ok\":{ok},\"cacheAge\":{},\"privilege\":\"{}\"}}",
            shared.cache_age().as_secs(),
            shared.privilege().label()
        );
        return json(stream, if ok { 200 } else { 503 }, &body);
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
                    &gui::login("/auth/login", oidc.required_group(), &shared.cfg.switch.host, Some(&err)),
                );
            };
            let state = cookie_value(cookie_header, STATE_COOKIE).unwrap_or_default();
            // The state cookie is single-use: clear it whatever happens so a
            // replay of the callback URL cannot be reused.
            let clear = format!("{STATE_COOKIE}=\"\"; Path=/; Max-Age=0");
            match oidc.finish_login(code, &state) {
                Ok((session_cookie, session)) => {
                    shared.audit(&session.username, "login", "ok");
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
                    shared.audit("?", "login", &format!("denied: {}", e.0));
                    // The reason (e.g. "not a member of Infra") is worth
                    // showing, and comes from authentik or from our own gate.
                    html(
                        stream,
                        401,
                        &gui::login("/auth/login", oidc.required_group(), &shared.cfg.switch.host, Some(&e.0)),
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

    /// Forget the snapshot and the session, forcing a reconnect and a re-read.
    fn invalidate(&self) {
        if let Ok(mut g) = self.cached_at.lock() {
            *g = None;
        }
        if let Ok(mut g) = self.cli.lock() {
            *g = None;
        }
    }

    fn audit(&self, user: &str, action: &str, result: &str) {
        let at = now_secs();
        if let Ok(mut g) = self.audit.lock() {
            if g.len() >= AUDIT_MAX {
                g.pop_front();
            }
            g.push_back(AuditEntry {
                at,
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
                    "{{\"at\":{},\"user\":\"{}\",\"action\":\"{}\",\"result\":\"{}\"}}",
                    e.at,
                    crate::jsonw::escape(&e.user),
                    crate::jsonw::escape(&e.action),
                    crate::jsonw::escape(&e.result)
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    /// The cached snapshot, refreshed when older than the TTL (or on demand).
    fn snapshot(&self, force: bool) -> R<Snapshot> {
        if !force && self.cache_age() <= self.cfg.ttl {
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
                *guard = Some(switch::Switch::connect(&self.cfg.switch)?);
            }
            let sw = guard.as_mut().unwrap();
            let mut snap = switch::read_snapshot(sw)?;
            if snap.ports.is_empty() && !snap.warnings.is_empty() {
                last = snap.warnings.join("; ");
                if attempt == 0 {
                    eprintln!("switch-gui: read failed on the cached session ({last}); reconnecting");
                    drop(guard);
                    self.invalidate();
                    continue;
                }
            }
            snap.taken_at_secs = now_secs();
            return Ok(snap);
        }
        Err(if last.is_empty() {
            "could not read the switch".to_string()
        } else {
            last
        })
    }

    /// Run one command on the shared session, reconnecting once if the switch
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
                    eprintln!("switch-gui: `{cmd}` failed on the cached session ({e}); reconnecting");
                }
            }
        }
        *guard = None;
        let mut sw = switch::Switch::connect(&self.cfg.switch)?;
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
            *guard = Some(switch::Switch::connect(&self.cfg.switch)?);
        }
        let sw = guard.as_mut().unwrap();
        for cmd in cmds {
            sw.command(cmd)?;
        }
        Ok(())
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
    }
}
