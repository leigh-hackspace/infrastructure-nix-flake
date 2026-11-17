//! `hackspace-mcp` — the hackspace's data as MCP tools, for an agent to read.
//!
//! One static binary, one Streamable-HTTP MCP server bound to **loopback**, seven
//! tools (`src/tools.rs` explains why seven). `slack-agent` is its only intended
//! client; it is the thing that knows about Home Assistant, the Space API,
//! Frigate, Prometheus and the router's lease table, so the agent binary itself
//! knows about none of them.
//!
//! ## Run it
//!
//! ```text
//! hackspace-mcp                              # serve 127.0.0.1:8191/mcp
//! hackspace-mcp --list-tools                 # what the model would be offered
//! hackspace-mcp --call space_status          # one tool, raw, no model involved
//! hackspace-mcp --self-test                  # every tool, once, with timings
//! ```
//!
//! The last three are the reason this can be proven without a Slack message
//! existing (`docs/slack-agent-2026-11-16.md` §9): every claim in this crate's
//! tool descriptions is checkable from a terminal on services1.
//!
//! ## Security posture, stated because it is load-bearing
//!
//! - **No authentication, therefore loopback only.** A non-loopback `--listen` is
//!   refused unless `--allow-non-loopback` is also given, and the refusal says why.
//!   This mirrors the `is_loopback` gate `common-rs/service` exists for: the
//!   unauthenticated surface of a service is a property of the address it binds,
//!   not of the intent in its config file.
//! - **Read-only by construction.** No handler issues anything but `GET`; there is
//!   no code path here that writes to Home Assistant, a switch, a door or a file.
//!   That is the answer to "Slack text is untrusted input", and it does not depend
//!   on the model being well-behaved.
//! - **The Home Assistant token is never an argument.** argv is world-readable on
//!   Linux, so the token comes from the process environment or an `EnvironmentFile`
//!   (`--env-file`, parsed with `common-rs/service`) and is only ever written into
//!   an `Authorization` header. There is deliberately no `--ha-token` flag to reach
//!   for.
//!
//! ## Why this process holds the token and `slack-agent` does not
//!
//! The agent's whole input is attacker-writable text. The tool server's input is a
//! JSON-RPC envelope from a process on the same machine. Putting the credential on
//! the side that has structured input — and where the only thing a caller can ask
//! for is one of seven named, filtered, read-only queries — is the smaller blast
//! radius, and it is why "add a write tool" has to touch this crate rather than the
//! agent's prompt.

mod args;
mod calc;
mod clock;
mod sources;
mod tools;
mod tools_basics;
mod tools_hass;
mod tools_space;
mod tools_watch;

use common_service as service;
use sources::Sources;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_LISTEN: &str = "127.0.0.1:8191";
const DEFAULT_TIMEOUT_SECS: &str = "5";

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let env_path = std::env::var("HACKSPACE_MCP_ENV_FILE").unwrap_or_default();
    let mut env = service::env_file_if_present(&env_path);
    // A unit passes --env-file rather than the environment only to name several
    // files; the first one wins, which is what systemd's EnvironmentFile ordering
    // would do too.
    if let Some(extra) = service::arg_opt(&argv, "--env-file") {
        let e = service::env_file_if_present(&extra);
        for (k, v) in e {
            env.entry(k).or_insert(v);
        }
    }
    // Flag, then real environment, then the EnvironmentFile, then the built-in.
    // The built-ins are the addresses this flake already declares (common/hosts.nix
    // for the two off-box origins), which is what lets the unit pass nothing at all
    // in the normal case and still let `--space-url` point the whole tool set at a
    // fixture.
    let setting = |flag: &str, env_key: &str, default: &str| -> String {
        service::arg_opt(&argv, flag)
            .or_else(|| std::env::var(env_key).ok())
            .or_else(|| env.get(env_key).cloned())
            .unwrap_or_else(|| default.to_string())
    };

    let listen = setting("--listen", "HACKSPACE_MCP_LISTEN", DEFAULT_LISTEN);
    let timeout = setting("--timeout-secs", "HACKSPACE_MCP_TIMEOUT_SECS", DEFAULT_TIMEOUT_SECS)
        .parse::<u64>()
        .unwrap_or(5);
    // Never a flag: argv is world-readable.
    let ha_token = ["HACKSPACE_MCP_HA_TOKEN", "HA_TOKEN", "DE_HOME_ASSISTANT_ACCESS_TOKEN"]
        .iter()
        .find_map(|k| {
            std::env::var(k)
                .ok()
                .or_else(|| env.get(*k).cloned())
                .filter(|v| !v.trim().is_empty())
        });

    let src = Arc::new(Sources::new(
        Sources::trim(&setting("--space-url", "HACKSPACE_MCP_SPACE_URL", "http://10.3.1.30:8081")),
        Sources::trim(&setting("--hass-url", "HACKSPACE_MCP_HASS_URL", "http://10.3.1.30:8123")),
        ha_token,
        Sources::trim(&setting("--frigate-url", "HACKSPACE_MCP_FRIGATE_URL", "http://127.0.0.1:5000")),
        // 9091, not 9090: cockpit owns 9090 on this box and answers **200 with an
        // HTML page** on an API path, so a status-code check alone would have
        // called that Prometheus.  Measured 2026-11-16; `observability.nix` says
        // the same thing in its header.
        Sources::trim(&setting("--prometheus-url", "HACKSPACE_MCP_PROM_URL", "http://127.0.0.1:9091")),
        Sources::trim(&setting("--leases-url", "HACKSPACE_MCP_LEASES_URL", "http://10.3.1.1:8091")),
        Duration::from_secs(timeout),
    ));
    let srv = Arc::new(tools::build(&src));

    if argv.iter().any(|a| a == "--list-tools") {
        for name in srv.tool_names() {
            println!("{name}");
        }
        println!("# {} tools, {} sources, HA token: {}", srv.tool_count(), 6, if src.hass_token.is_some() { "present" } else { "ABSENT" });
        return;
    }

    if let Some(idx) = argv.iter().position(|a| a == "--call") {
        let Some(name) = argv.get(idx + 1) else {
            eprintln!("--call needs a tool name (see --list-tools)");
            std::process::exit(2);
        };
        let arguments = argv.get(idx + 2).map(String::as_str).unwrap_or("{}");
        let body = envelope(name, arguments);
        match srv.handle(&body) {
            Some(reply) => println!("{reply}"),
            None => {
                eprintln!("the server declined to answer {name}");
                std::process::exit(1);
            }
        }
        return;
    }

    if argv.iter().any(|a| a == "--self-test") {
        std::process::exit(self_test(&srv));
    }

    // The bind is the security decision, so it is checked rather than assumed.
    let bind_host = listen.rsplit_once(':').map(|(h, _)| h).unwrap_or(&listen);
    if !service::is_loopback(bind_host) && !argv.iter().any(|a| a == "--allow-non-loopback") {
        eprintln!(
            "refusing to bind {listen}: this server has no authentication and its tools read \
             every corner of the building through one Home Assistant token.\n\
             It is meant to be reached by slack-agent on this machine only. If you have decided \
             otherwise, pass --allow-non-loopback and make the decision where it can be seen."
        );
        std::process::exit(2);
    }

    let listener = match TcpListener::bind(&listen) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot bind {listen}: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "hackspace-mcp listening on {listen}/mcp — {} tools; sources space={} hass={} (token {}) frigate={} prom={} leases={}",
        srv.tool_count(),
        src.space,
        src.hass,
        if src.hass_token.is_some() { "present" } else { "ABSENT" },
        src.frigate,
        src.prometheus,
        src.leases
    );
    let _ = std::io::stdout().flush();

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let srv = Arc::clone(&srv);
                let src = Arc::clone(&src);
                thread::spawn(move || {
                    if let Err(e) = serve_conn(stream, srv, src) {
                        // Not a fault to report loudly: a client that gives up
                        // mid-request is ordinary, and this unit runs forever.
                        eprintln!("connection: {e}");
                    }
                });
            }
            Err(e) => eprintln!("accept: {e}"),
        }
    }
}

fn serve_conn(
    mut stream: TcpStream,
    srv: Arc<common_mcp::Server>,
    src: Arc<Sources>,
) -> std::io::Result<()> {
    let Some(req) = common_web::read_request(&mut stream) else {
        return Ok(());
    };
    let path = req.path().to_string();
    let (status, ctype, body) = match (req.method.as_str(), path.as_str()) {
        ("POST", "/mcp") => match srv.handle(&req.body_text()) {
            Some(reply) => (200u16, "application/json", reply),
            None => (202, "application/json", String::new()),
        },
        ("GET", "/health") => (200, "text/plain; charset=utf-8", "ok\n".to_string()),
        ("GET", "/metrics") => (200, "text/plain; version=0.0.4", src.render_metrics()),
        ("GET", "/mcp") | ("HEAD", "/mcp") => (
            405,
            "text/plain; charset=utf-8",
            "hackspace-mcp speaks MCP over POST /mcp (JSON-RPC 2.0). Try tools/list.\n".to_string(),
        ),
        ("POST", _) => (404, "application/json", common_json::error_object("not found; the only POST path is /mcp")),
        _ => (404, "text/plain; charset=utf-8", "not found\n".to_string()),
    };
    common_web::respond(&mut stream, status, ctype, &body)
}

/// A `tools/call` envelope, built by the JSON writer rather than by `format!`.
///
/// This was a format string, and it emitted `"name":space_status` — a bare word
/// where JSON wants a string — so every call failed with a parse error and the
/// self-test reported eight failures against a server whose tools were fine. That
/// is the whole argument for not hand-writing JSON around a protocol: the bug is
/// invisible in the source and the symptom points at the far end.  `arguments` is
/// the one raw piece (it is a JSON object the human typed), so it is validated
/// here rather than pasted in blind.
fn envelope(name: &str, arguments: &str) -> String {
    let arguments = match common_json::parse(arguments) {
        Ok(v @ common_json::Json::Obj(_)) => v,
        Ok(_) => common_json::object(Vec::new()),
        Err(e) => {
            eprintln!("the arguments you gave are not a JSON object ({e}): {arguments}");
            std::process::exit(2);
        }
    };
    common_json::emit(
        &common_json::object(
            vec![
                ("jsonrpc", common_json::string("2.0")),
                ("id", common_json::number_from_int(1)),
                ("method", common_json::string("tools/call")),
                (
                    "params",
                    common_json::object(vec![
                        ("name".to_string(), common_json::string(name)),
                        ("arguments".to_string(), arguments),
                    ]),
                ),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<Vec<(String, common_json::Json)>>(),
        ),
        0,
    )
}

/// One self-test call. `expect` is a substring the answer must contain, or `None` when the
/// only honest check is "it answered" — which is the right check for a live sensor and the
/// wrong one for arithmetic.
struct Case {
    name: &'static str,
    args: &'static str,
    expect: Option<&'static str>,
    /// True for the tools that touch no network, which is what makes them the diagnostic.
    offline: bool,
}

/// Call every tool the way a client would, print what came back, and exit non-zero if any
/// of them failed.  This is the gate `docs/slack-agent-2026-11-16.md` sets for phase 1, and
/// it is the thing to re-run after any engine or network change.
fn self_test(srv: &common_mcp::Server) -> i32 {
    let cases = vec![
        Case { name: "space_status", args: "{}", expect: None, offline: false },
        Case { name: "space_events", args: r#"{"days":7}"#, expect: None, offline: false },
        Case { name: "home_assistant_states", args: r#"{"search":"temp","limit":6}"#, expect: None, offline: false },
        Case { name: "home_assistant_states", args: r#"{"domain":"lock","limit":6}"#, expect: None, offline: false },
        Case { name: "home_assistant_history", args: r#"{"entity_id":"sensor.workshop_temperature","hours":6}"#, expect: None, offline: false },
        Case { name: "camera_activity", args: r#"{"hours":12}"#, expect: None, offline: false },
        Case { name: "fleet_metric", args: r#"{"query":"up"}"#, expect: None, offline: false },
        Case { name: "network_clients", args: "{}", expect: None, offline: false },
        // The basics trio is here for a different reason from the other eight, and its answers
        // are *checked* rather than merely collected: an inch is 25.4 mm by definition, so a
        // wrong number means the binary is broken — and it needs no network to prove it.
        Case { name: "what_time", args: "{}", expect: Some("Europe/London"), offline: true },
        Case { name: "calculate", args: r#"{"expression":"3/8*25.4"}"#, expect: Some("9.525"), offline: true },
        Case { name: "convert", args: r#"{"amount":1,"from":"in","to":"mm"}"#, expect: Some("25.4 mm"), offline: true },
    ];
    let mut bad = 0;
    let mut offline_total = 0;
    let mut offline_bad = 0;
    let total = cases.len();
    for case in &cases {
        let (name, arguments) = (case.name, case.args);
        let body = envelope(name, arguments);
        let started = Instant::now();
        let reply = srv.handle(&body).unwrap_or_else(|| "{\"error\":\"no reply\"}".to_string());
        let ms = started.elapsed().as_millis();
        let parsed = common_json::parse(&reply).ok();
        let is_err = parsed
            .as_ref()
            .and_then(|d| d.at(&["result", "isError"]))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let text = extract(&parsed);
        // Answered, but not with the thing arithmetic says, counts as a failure too: "ok" that
        // hides a wrong number is precisely the fault this tier exists to remove.
        let wrong = is_err || case.expect.map(|want| !text.contains(want)).unwrap_or(false);
        let summary: String = text.chars().take(240).collect::<String>().replace('\n', " | ");
        let status = if wrong { "FAIL" } else { "ok  " };
        if wrong {
            bad += 1;
            if case.offline {
                offline_bad += 1;
            }
        }
        if case.offline {
            offline_total += 1;
        }
        let note = match (is_err, case.expect) {
            (false, Some(want)) => format!("  (checked for {want:?})"),
            _ => String::new(),
        };
        println!("[{status}] {name} {arguments}  {ms} ms{note}\n        {summary}");
    }
    // **The reason the offline trio is in this list at all.** Eight of these tools read the
    // building or the servers, so all eight can legitimately fail at 03:00 while this process
    // is perfectly healthy — and "8 of 11 failed" is then ambiguous about what to fix. The
    // three that read nothing but their own tables settle it in one line.
    if bad == 0 {
        println!("\nall {total} tool calls answered");
        0
    } else if offline_bad > 0 {
        println!(
            "\n{bad} of {total} failed, INCLUDING {offline_bad} of the {offline_total} that need no network — this process or its tables are broken, look here first."
        );
        1
    } else {
        println!(
            "\n{bad} of {total} failed, but the {offline_total} tools that need no network all answered: this process is fine, the failures above are upstreams."
        );
        1
    }
}

/// The first content block's text, which is the only part of a tool reply worth printing.
fn extract(parsed: &Option<common_json::Json>) -> String {
    parsed
        .as_ref()
        .and_then(|d| d.at(&["result", "content"]))
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|i| i.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("(unreadable reply)")
        .to_string()
}
