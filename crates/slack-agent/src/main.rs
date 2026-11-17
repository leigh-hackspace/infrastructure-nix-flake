//! `slack-agent` — the harness: Slack in, tools out, answer back.
//!
//! Four subcommands, listed in the order they were built, because that order is
//! the point (`docs/slack-agent-2026-11-16.md` §9):
//!
//! ```text
//! slack-agent tools                 # what the model would be offered, from the MCP servers
//! slack-agent probe                 # which engine answers, what it is serving, how fast
//! slack-agent ask "is it cold…"     # the whole loop, printed, no Slack involved
//! slack-agent serve [--dry-run]     # Slack Socket Mode; --dry-run posts nothing
//! ```
//!
//! ## What this binary deliberately does not know
//!
//! It has no idea what a temperature sensor is. The tool list is read from the MCP
//! servers at startup (`--mcp`, repeatable), so `hackspace-mcp` is the only place
//! that knows about the building, and a second agent — a Grafana one, a switch one,
//! the "pi room dashboard" one someone will want next — is this same binary pointed
//! at different servers. That separation is why this file is as short as it is.
//!
//! ## Two things held in memory, and why that is honest
//!
//! Conversation history is a bounded map keyed `channel:thread`, in this process
//! only. A restart forgets it, and forgetfulness is the right default for a bot that
//! answers "what's the temperature" rather than one that promises a memory. When
//! someone wants it to remember, the place for that is the Postgres `switch-gui`
//! and `gocardless-dashboard` already use — not a `sessions.json` nobody declared.
//!
//! Secrets come from the environment or an `EnvironmentFile` and are never
//! arguments, never logged, never written into a store path. The inference endpoints
//! need no secret at all: `strataBearer` injects the key at the front door
//! (`common/nginx/front-door.nix`), which is why there is no `--api-key` here and
//! why a 401 from an engine means the row or the route is wrong, not that a
//! credential is missing.

mod agent;
mod engine;
mod prompt;
mod slack;
mod listen;
mod triage;

use agent::Agent;
use common_json::{self as json, Json};
use common_service as service;
use common_mcp::Client as McpClient;
use engine::{Engines, DEFAULT_CANDIDATES};
use slack::{Backoff, Seen, Slack};
use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_MCP: &str = "http://127.0.0.1:8191/mcp";
const DEFAULT_HEALTH: &str = "127.0.0.1:8192";
/// `listen`'s socket: the bot's answer for anything that is not Slack.  Fourth
/// loopback port on the box (8191 is the MCP server, 8192 this unit's health), and
/// the one `voice-assistant` dials - see `crates/voice-assistant/src/agent.rs`.
const DEFAULT_ASK_LISTEN: &str = "127.0.0.1:8194";
/// Silence after which `listen` starts a new conversation.  Long enough for a room to
/// ask three related questions, short enough that a transcript left overnight cannot
/// answer today's question with yesterday's tool results.
const DEFAULT_ASK_RESET_SECS: u64 = 900;
const DEFAULT_STALL_SECS: u64 = 300;
/// Conversations kept in memory; past this the least-recently-used goes.
const MAX_SESSIONS: usize = 300;

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let sub = argv.get(1).map(String::as_str).unwrap_or("");
    let config = match Config::from_args(&argv) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            usage();
            std::process::exit(2);
        }
    };
    let outcome = match sub {
        "tools" => run_tools(&config),
        "probe" => run_probe(&config),
        "ask" => run_ask(&config, &argv),
        "call" => run_call(&config, &argv),
        "events" => run_events(&config, &argv),
        "digest" => run_digest(&config, &argv),
        "serve" => run_serve(&config),
        "listen" => run_listen(&config),
        "" => {
            usage();
            std::process::exit(2);
        }
        other => {
            eprintln!("unknown subcommand {other}");
            usage();
            std::process::exit(2);
        }
    };
    if let Err(e) = outcome {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn usage() {
    eprintln!(
        "usage: slack-agent <tools|probe|ask|call|events|digest|serve|listen> [flags]\n\
       \n\
       \x20 tools                 list the tools the MCP servers offer\n\
       \x20 probe                 which inference endpoint answers, and what it serves\n\
       \x20 ask \"<question>\"      run one turn and print it (never touches Slack)
\
       \x20 call <tool> [json]   call one tool on the running MCP server, raw — no model, no token\n\
       \x20 events \"<question>\"  what the deterministic calendar path would reply (no model, no Slack)\n\
       \x20 digest [--to NAME] [--days N] [--title TEXT] [--dry-run]\n\
       \x20                     post the week's events to a channel — this is the Monday timer\n\
       \x20 serve [--dry-run]     Slack Socket Mode; --dry-run reads everything and posts nothing\n\
       \n\
       flags:\n\
       \x20 --mcp URL             MCP server (repeatable). Default {DEFAULT_MCP}\n\
       \x20 --endpoints SPEC      url|label,url|label… (Nix renders this from common/llm-endpoints.nix)\n\
       \x20 --endpoints-file P    the same, from a file\n\
       \x20 --env-file PATH       KEY=VALUE file holding the Slack tokens (repeatable)\n\
       \x20 --channels a,b        channels to answer in unmentioned: names, ids, or `*` for every\n\
       \x20                     channel we can see, gated by `triage::answers` (see that file:\n\
       \x20                     questions and hackspace topics yes, human-to-human chatter no)\n\
       \x20 --no-dms              ignore direct messages\n\
       \x20 --defer-vulcan-words  leave Vulcan's bare commands (temps, status, events…) to\n\
       \x20                     vulcan-bot: only while it is connected to the same Slack app\n\
       \x20 --no-thread-replies   answer in the channel instead of in a thread\n\
       \x20 --listen ADDR         health/metrics bind for `serve`. Default {DEFAULT_HEALTH}\n\
       \x20 --timeout-secs N      engine and MCP timeout. Default 120\n\
       \x20 --max-silence-secs N  exit when Slack has sent nothing this long. Default {DEFAULT_STALL_SECS}\n\
       \x20 --dry-run             (serve) log what it would say, send nothing\n\
       \x20 --emoji NAME          the reaction that means \"thinking\". Default eyes"
    );
}

struct Config {
    mcp: Vec<String>,
    endpoints: String,
    /// Whether the engine may think out loud. See `engine::Thinking` for the measurements;
    /// the default there is today's behaviour and this line is what changes it deliberately.
    thinking: engine::Thinking,
    listen: String,
    channels: Vec<String>,
    allow_dms: bool,
    defer_vulcan_words: bool,
    thread_replies: bool,
    dry: bool,
    timeout: Duration,
    stall: Duration,
    emoji: String,
    /// `listen`'s bind and conversation window.
    ask_listen: String,
    ask_reset: Duration,
    env: BTreeMap<String, String>,
}

impl Config {
    fn from_args(argv: &[String]) -> Result<Config, String> {
        let mut mcp: Vec<String> = Vec::new();
        let mut env: BTreeMap<String, String> = BTreeMap::new();
        for (i, a) in argv.iter().enumerate() {
            if a == "--mcp" {
                if let Some(v) = argv.get(i + 1) {
                    mcp.push(v.clone());
                }
            }
            if a == "--env-file" {
                if let Some(path) = argv.get(i + 1) {
                    // Later files do not override earlier ones, which is the systemd
                    // `EnvironmentFile` order and means a secret dropped in two files
                    // cannot silently disagree depending on the unit's flag order.
                    for (k, v) in service::env_file_if_present(path) {
                        env.entry(k).or_insert(v);
                    }
                }
            }
        }
        let get = |flag: &str, key: &str| -> Option<String> {
            service::arg_opt(argv, flag).or_else(|| env.get(key).cloned())
        };
        if mcp.is_empty() {
            if let Some(spec) = get("--mcp", "SLACK_AGENT_MCP_URLS") {
                mcp = spec.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            }
        }
        if mcp.is_empty() {
            mcp.push(DEFAULT_MCP.to_string());
        }
        // The endpoint table comes from a flag in development and from a file when
        // deployed (rendered from `common/llm-endpoints.nix`) — a store path someone
        // can `cat` and diff beats a long argv, and argv is world-readable.
        let endpoints = match service::arg_opt(argv, "--endpoints-file") {
            Some(path) => std::fs::read_to_string(&path)
                .map(|s| s.trim().to_string())
                .map_err(|e| format!("cannot read --endpoints-file {path}: {e}"))?,
            None => get("--endpoints", "SLACK_AGENT_ENDPOINTS").unwrap_or_else(|| DEFAULT_CANDIDATES.to_string()),
        };
        if endpoints.trim().is_empty() {
            return Err("--endpoints is empty: there would be no inference engine to ask".to_string());
        }
        Ok(Config {
            mcp,
            thinking: engine::Thinking::parse(
                &get("--thinking", "SLACK_AGENT_THINKING").unwrap_or_else(|| "on".into()),
            ),
            endpoints,
            listen: get("--listen", "SLACK_AGENT_LISTEN").unwrap_or_else(|| DEFAULT_HEALTH.to_string()),
            channels: get("--channels", "SLACK_AGENT_CHANNELS")
                .map(|s| s.split(',').map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect())
                .unwrap_or_default(),
            allow_dms: !argv.iter().any(|a| a == "--no-dms"),
            defer_vulcan_words: argv.iter().any(|a| a == "--defer-vulcan-words"),
            thread_replies: !argv.iter().any(|a| a == "--no-thread-replies"),
            dry: argv.iter().any(|a| a == "--dry-run"),
            timeout: Duration::from_secs(
                get("--timeout-secs", "SLACK_AGENT_TIMEOUT_SECS").and_then(|v| v.parse().ok()).unwrap_or(120),
            ),
            stall: Duration::from_secs(
                get("--max-silence-secs", "SLACK_AGENT_MAX_SILENCE_SECS").and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_STALL_SECS),
            ),
            emoji: get("--emoji", "SLACK_AGENT_EMOJI").unwrap_or_else(|| "eyes".to_string()),
            ask_listen: get("--ask-listen", "SLACK_AGENT_ASK_LISTEN")
                .unwrap_or_else(|| DEFAULT_ASK_LISTEN.to_string()),
            ask_reset: Duration::from_secs(
                get("--ask-reset-after-secs", "SLACK_AGENT_ASK_RESET_SECS")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(DEFAULT_ASK_RESET_SECS),
            ),
            env,
        })
    }

    /// A token by any of several names, from the environment or the env-file.  Not
    /// a flag: see the module header.
    fn token(&self, names: &[&str]) -> Result<String, String> {
        names
            .iter()
            .find_map(|n| std::env::var(n).ok().or_else(|| self.env.get(*n).cloned()))
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                format!(
                    "no {} in the environment or the --env-file. On services1 it is the runtime \
                     sops secret `slack_agent_env` (0440, one group); it is never a flag, because \
                     argv is world-readable.",
                    names.join(" or ")
                )
            })
    }
}

fn engines(config: &Config) -> Engines {
    let mut engines = Engines::new(engine::parse_candidates(&config.endpoints), config.timeout);
    engines.set_thinking(config.thinking);
    engines
}

// --- tools --------------------------------------------------------------------

/// List what the model would be offered.  Non-zero exit if any MCP server failed:
/// a missing tool group is the difference between an answer and a confident
/// invention, so it is a finding rather than a notice.
fn run_tools(config: &Config) -> Result<(), String> {
    let (agent, warnings) = Agent::new(engines(config), &config.mcp);
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    for tool in agent.tools() {
        println!("{} — {}", tool.name, first_sentence(&tool.description));
        println!("  schema: {}", json::emit(&tool.schema, 0));
    }
    println!(
        "\n{} tools from {} MCP server(s), {} endpoint(s)",
        agent.tool_count(),
        config.mcp.len(),
        engine::parse_candidates(&config.endpoints).len()
    );
    if !warnings.is_empty() {
        return Err("at least one MCP server did not answer".to_string());
    }
    Ok(())
}

// --- probe --------------------------------------------------------------------

/// Which endpoint answers, what model id it is serving right now, and roughly how
/// fast it decodes.  Run after `nix flake update strata`, which moves both engines
/// at once: the ids here are discovered, never declared.
fn run_probe(config: &Config) -> Result<(), String> {
    let mut engines = engines(config);
    let candidates = engines.candidates().to_vec();
    let mut any = false;
    for candidate in &candidates {
        match Engines::discover(candidate, engines.http()) {
            Ok(e) => {
                any = true;
                println!(
                    "ok   {} serving `{}`{}",
                    e.label,
                    e.model,
                    e.context.map(|c| format!(" (n_ctx {})", c as u64)).unwrap_or_default()
                );
                match engines.chat(
                    &[json::object(vec![
                        ("role".to_string(), json::string("user")),
                        ("content".to_string(), json::string("Reply with exactly: pong")),
                    ])],
                    &[],
                    40,
                ) {
                    Ok(t) => {
                        let tokens = t.usage.completion.max(1);
                        println!(
                            "     {} ms, {} tokens ≈ {:.0} tok/s decode, text {:?}",
                            t.millis,
                            tokens,
                            tokens as f64 / (t.millis.max(1) as f64 / 1000.0),
                            t.text.clone().unwrap_or_default()
                        );
                    }
                    Err(e) => println!("     reachable, but the chat call failed: {e}"),
                }
            }
            Err(e) => println!("down {e}"),
        }
    }
    if !any {
        return Err("no inference endpoint answered".to_string());
    }
    Ok(())
}

// --- ask ----------------------------------------------------------------------

/// One turn, in full, to stdout.  This is the verb that proves the design without
/// a single Slack message existing.
fn run_ask(config: &Config, argv: &[String]) -> Result<(), String> {
    // `-` reads the question from stdin.  It exists for `just slack-agent-ask`,
    // because a question is prose — apostrophes, dollars, quotes — and threading
    // prose through just's interpolation into `bash -c` into another shell level is
    // how "what's on tonight?" arrives as `whats on tonight` or fails outright.
    let question = match argv.get(2).map(String::as_str) {
        Some("-") => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("reading the question from stdin: {e}"))?;
            let text = buf.split_whitespace().collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                return Err("stdin was empty, so there is no question to ask".to_string());
            }
            text
        }
        Some(q) if !q.trim().is_empty() => q.trim().to_string(),
        _ => {
            return Err(
                "ask needs a question (or `-` to read it from stdin), e.g. slack-agent ask                  \"is it cold in the hackspace\""
                    .to_string(),
            )
        }
    };
    let (mut agent, warnings) = Agent::new(engines(config), &config.mcp);
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    if agent.tool_count() == 0 {
        return Err("no tools at all: every MCP server failed, so any answer would be invented".to_string());
    }
    let mut transcript = agent::seed_transcript();
    println!("question: {question}\n({} tools available)", agent.tool_count());
    let answer = agent.ask(&mut transcript, &question, |event| println!("   · {event}"))?;
    println!("\nanswer:\n{}", answer.text);
    println!(
        "\n[{} rounds · {} · {} ms · {} in / {} out tokens · {} tool call(s)]",
        answer.rounds, answer.engine, answer.millis, answer.prompt_tokens, answer.completion_tokens, answer.log.len()
    );
    for line in &answer.log {
        println!("   {line}");
    }
    Ok(())
}

// --- listen -------------------------------------------------------------------

/// The bot on a socket, for whoever is asking — see `listen.rs` for why this is a second face of the same loop rather than a
/// second bot, and for the two things that make having it at all safe: loopback only,
/// and one turn at a time.
fn run_listen(config: &Config) -> Result<(), String> {
    listen::run(listen::Options {
        listen: config.ask_listen.clone(),
        mcp: config.mcp.clone(),
        endpoints: config.endpoints.clone(),
        timeout: config.timeout,
        reset_after: config.ask_reset,
        record_questions: !std::env::args().any(|a| a == "--no-record-questions"),
        thinking: config.thinking,
    })
}

// --- call ---------------------------------------------------------------------

/// Call one tool on the running MCP server and print exactly what it returned.
///
/// This is the verb that lets a tool be tested against the *deployed* server, which
/// is the only one holding the Home Assistant token — so testing a tool never means
/// copying a secret into a shell, and never means asking a model to do the
/// explaining. `--call` inside `hackspace-mcp` is the same check with the token in
/// the process environment instead; this one is the version you can run as yourself.
fn run_call(config: &Config, argv: &[String]) -> Result<(), String> {
    let name = argv
        .get(2)
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "call needs a tool name; `tools` lists them".to_string())?;
    let arguments_raw = argv.get(3).map(String::as_str).unwrap_or("{}");
    let arguments = json::parse(arguments_raw)
        .map_err(|e| format!("the arguments are not a JSON object ({e}): {arguments_raw}"))?;
    if !matches!(arguments, Json::Obj(_)) {
        return Err(format!("expected a JSON object of arguments, got: {arguments_raw}"));
    }
    let mut failures = Vec::new();
    for url in &config.mcp {
        let client = McpClient::new(url.clone());
        match client.call_text(name, arguments.clone()) {
            Ok(text) => {
                println!("{text}");
                return Ok(());
            }
            Err(e) => failures.push(e),
        }
    }
    Err(failures.join("\n"))
}

// --- the calendar, without a model ---------------------------------------------

/// Days ahead for one deterministic calendar answer, from the words in the message.
fn calendar_args(days: i64) -> Json {
    json::object(vec![(
        "days".to_string(),
        json::number_from_int(days.clamp(1, 60) as u64),
    )])
}

/// Answer "what's on" straight from the tool. Returns `Err` when anything in the path fails,
/// and the caller then asks the model instead — a slow answer beats none.
///
/// No `eyes` reaction either: that one exists because a model turn takes ~17 s and a person
/// deserves to know they were not ignored. This takes the length of one HTTP GET.
fn answer_events(
    agent: &mut Agent,
    slack: &Slack,
    config: &Config,
    metrics: &Metrics,
    mention: &slack::Mention,
    days: i64,
) -> Result<(), String> {
    let started = Instant::now();
    let text = agent.call("space_events", calendar_args(days))?;
    let (thread_ts, at) = slack::reply_target(
        mention.thread_ts.as_deref(),
        config.thread_replies,
        &mention.user,
    );
    let body = match at {
        Some(at) => format!("{at} {}", slack::escape(&text)),
        None => slack::escape(&text),
    };
    slack.post(&mention.channel, thread_ts.as_deref(), &body)?;
    metrics.turns.fetch_add(1, Ordering::Relaxed);
    metrics.tool_calls.fetch_add(1, Ordering::Relaxed);
    metrics
        .last_turn_ms
        .store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
    println!(
        "deterministic: space_events(days={days}) answered {} in {} ms (no model)",
        mention.user,
        started.elapsed().as_millis()
    );
    Ok(())
}

/// `slack-agent events "what's on tonight"` — exactly what the socket handler would send for
/// those words, printed here. It is the whole reason the fast path is testable before anybody
/// reads it in public.
fn run_events(config: &Config, argv: &[String]) -> Result<(), String> {
    let question = match argv.get(2).map(String::as_str) {
        Some("-") => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf).map_err(|e| e.to_string())?;
            buf
        }
        Some(q) => q.to_string(),
        None => return Err("events needs a question, or `-` to read it from stdin".to_string()),
    };
    let Some(days) = triage::events_window(&question) else {
        println!("{question:?} is not a calendar question, so the model would answer it instead.");
        return Ok(());
    };
    println!("{question:?} → space_events(days={days}), no model.");
    let client = McpClient::new(config.mcp.first().cloned().unwrap_or_default());
    println!("{}", client.call_text("space_events", calendar_args(days))?);
    Ok(())
}

/// `slack-agent digest [--to general] [--days 7] [--title …] [--dry-run]` — the replacement
/// for `vulcan-bot`'s `@schedule(day_of_week=0, hour=9)` digest, run by a systemd timer.
///
/// Better than the old one in the two ways that mattered: the list is **in time order** and
/// grouped under the day it happens on (`vulcan-bot` emitted the API's own order, so Monday's
/// social could sit under Thursday), and the times are the **event's published wall clock**
/// rather than a UTC rendering or an `arrow.humanize()` string like "in 3 hours" that reads
/// correctly only at the instant it was written.
fn run_digest(config: &Config, argv: &[String]) -> Result<(), String> {
    let get = |flag: &str, default: &str| -> String {
        argv.iter()
            .position(|a| a == flag)
            .and_then(|i| argv.get(i + 1))
            .cloned()
            .unwrap_or_else(|| default.to_string())
    };
    let to = get("--to", "general");
    let days: i64 = get("--days", "7").parse().unwrap_or(7);
    let title = get(
        "--title",
        "Happy Monday! Here is what is on in the hackspace calendar:",
    );

    let slack = Slack::new(
        config.token(&["SLACK_BOT_TOKEN", "SM_SLACK_BOT_TOKEN"])?,
        config.token(&["SLACK_APP_TOKEN", "SM_SLACK_APP_TOKEN"])?,
        config.dry,
    )?;
    // A name, because the old bot's config held a name too (`EVENTS_NOTIFICATION_ROOM`,
    // default `general`); an id passes through. Failing loudly here is the point — a digest
    // that silently posts to the wrong room is a bug nobody notices until someone asks why.
    let channel = if to.starts_with(['C', 'D', 'G']) {
        to.clone()
    } else {
        slack
            .channel_ids()?
            .get(&to)
            .cloned()
            .ok_or_else(|| format!("no channel named {to:?} — the digest has nowhere to go"))?
    };
    let client = McpClient::new(config.mcp.first().cloned().unwrap_or_default());
    let list = client.call_text("space_events", calendar_args(days))?;
    let body = format!("{title}\n\n{}", slack::escape(&list));
    slack.post(&channel, None, &body)?;
    println!("digest posted to {channel} ({} days)", days);
    Ok(())
}

// --- serve --------------------------------------------------------------------

fn run_serve(config: &Config) -> Result<(), String> {
    let slack = Slack::new(
        config.token(&["SLACK_BOT_TOKEN", "SM_SLACK_BOT_TOKEN"])?,
        config.token(&["SLACK_APP_TOKEN", "SM_SLACK_APP_TOKEN"])?,
        config.dry,
    )?;
    let (mut agent, warnings) = Agent::new(engines(config), &config.mcp);
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    if agent.tool_count() == 0 {
        // Starting anyway would mean a bot that answers every question from
        // nothing. Refuse, and let the unit's restart loop retry.
        return Err("no tools available, so every answer would be invented; not starting".to_string());
    }
    // `*` means "every channel we can see, gated by whether the message reads as a request",
    // which is the shape the old bot had: Vulcan answered `temps` wherever it was typed. The
    // gate is what an LLM needs and a keyword matcher did not — see `triage.rs`.
    let everywhere = config.channels.iter().any(|c| c == "*");
    println!(
        "slack-agent: {} tools from {:?}; user {}; mode {}; threads {}; dms {}; channels-without-mention {:?}",
        agent.tool_count(),
        config.mcp,
        slack.user_id,
        if config.dry { "DRY RUN (nothing is posted)" } else { "LIVE" },
        config.thread_replies,
        config.allow_dms,
        if everywhere {
            "*(gated by triage)".to_string()
        } else {
            config.channels.join(",")
        }
    );
    let _ = std::io::stdout().flush();

    // Resolve the allow-list to ids once.  A failure to list channels is reported
    // and ignored — ids still work, and a bot that cannot start because
    // `conversations.list` was busy helps nobody.
    let mut allowed: Vec<String> = Vec::new();
    if !config.channels.is_empty() && !everywhere {
        match slack.channel_ids() {
            Ok(map) => {
                for wanted in &config.channels {
                    if let Some(id) = map.get(wanted).cloned().or_else(|| {
                        // Already an id?  Channel ids start C/D/G private-ish; a
                        // wrong-looking entry only ever means "this row does
                        // nothing", which is reported below.
                        (wanted.starts_with('C') || wanted.starts_with('D') || wanted.starts_with('G'))
                            .then(|| wanted.clone())
                    }) {
                        allowed.push(id);
                    } else {
                        eprintln!("warning: no channel named {wanted} — that allow-list row does nothing");
                    }
                }
            }
            Err(e) => {
                eprintln!("warning: cannot list channels ({e}) — --channels must be ids");
                allowed = config.channels.clone();
            }
        }
    }

    let last_frame = Arc::new(AtomicU64::new(0));
    let metrics = Arc::new(Metrics::default());
    {
        let last = Arc::clone(&last_frame);
        let metrics = Arc::clone(&metrics);
        let listen = config.listen.clone();
        let stall = config.stall;
        thread::spawn(move || serve_health(&listen, &last, &metrics, stall));
    }
    let _watchdog = slack::watch_stall(Arc::clone(&last_frame), config.stall, 42);

    let mut sessions: BTreeMap<String, Vec<Json>> = BTreeMap::new();
    let mut lru: VecDeque<String> = VecDeque::new();
    let mut seen = Seen::default();
    let mut backoff = Backoff::new();

    loop {
        let (ws, url) = match slack.open_socket() {
            Ok(pair) => pair,
            Err(e) => {
                let wait = backoff.wait();
                eprintln!("cannot open a Slack socket: {e} — retrying in {}s", wait.as_secs());
                thread::sleep(wait);
                continue;
            }
        };
        backoff.reset();
        last_frame.store(slack::now_secs(), Ordering::Relaxed);
        println!("socket: connected ({url})");
        let _ = std::io::stdout().flush();

        let bot_id = slack.user_id.clone();
        let outcome = slack::frame_loop(ws, &last_frame, |frame, send| {
            let doc = json::parse(frame).map_err(|e| format!("frame is not JSON ({e}): {:.120}", frame))?;
            let kind = doc.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if kind == "events_api" {
                if let Some(envelope) = doc.get("envelope_id").and_then(|v| v.as_str()) {
                    // The ack goes out here, before anything slow: Slack's own
                    // timeout for a missing ack is a few seconds and a model turn is
                    // twenty, and an un-acked event is redelivered — which is a
                    // second answer unless this line stays exactly where it is.
                    send(slack::ack(envelope))?;
                }
            } else if kind == "disconnect" {
                return Err(format!(
                    "Slack asked to reconnect ({})",
                    doc.get("reason").and_then(|v| v.as_str()).unwrap_or("no reason given")
                ));
            } else if kind != "hello" {
                println!("socket: ignoring a frame of type {kind}");
            }

            let mention = match slack::parse_event(frame, &bot_id) {
                Ok(Some(m)) => m,
                Ok(None) => return Ok(()),
                Err(()) => return Err("frame is not JSON".to_string()),
            };
            if let Some(id) = &mention.event_id {
                if seen.contains(id) {
                    println!("dedup: redelivery of {id} answered once already — ignored");
                    return Ok(());
                }
                seen.insert(id.clone());
            }
            let asked_us = mention.is_mention || (config.allow_dms && mention.is_dm);
            let here = everywhere || allowed.iter().any(|a| a == &mention.channel);
            // Unmentioned means we are interrupting a room, so it takes a message that reads
            // like a request.  Logged either way: a bot that stays quiet for an unprintable
            // reason is indistinguishable from a bot that is broken (rule 29).
            let mut gate = triage::Triage::default();
            gate.defer_vulcan_words = config.defer_vulcan_words;
            let gated = gate.answers(&mention.text);
            if !(asked_us || (here && gated)) {
                println!(
                    "skip: {} said {:.80} in {} — {}{}{}",
                    mention.user,
                    mention.text,
                    mention.channel,
                    if asked_us { "" } else { "not addressed to us; " },
                    if here { "" } else { "channel not in --channels; " },
                    if gated {
                        ""
                    } else {
                        "and it does not read as a question (see triage.rs)"
                    },
                );
                return Ok(());
            }
            // The calendar question goes straight to the tool that already has the answer in
            // the right order — no engine, no tokens, no chance of a row being dropped. If the
            // fast path fails for any reason, fall through to the model rather than go silent:
            // a slower answer beats none, which is the same rule the tools follow.
            if let Some(days) = triage::events_window(&mention.text) {
                match answer_events(&mut agent, &slack, config, &metrics, &mention, days) {
                    Ok(()) => return Ok(()),
                    Err(e) => eprintln!("calendar fast path failed, asking the model instead: {e}"),
                }
            }
            answer(&mut agent, &slack, config, &metrics, &mut sessions, &mut lru, &mention);
            Ok(())
        });
        match outcome {
            Ok(()) => eprintln!("socket: loop ended cleanly"),
            Err(e) => eprintln!("socket: {e}"),
        }
        let _ = std::io::stderr().flush();
    }
}

/// One turn: react, think, answer, un-react.
fn answer(
    agent: &mut Agent,
    slack: &Slack,
    config: &Config,
    metrics: &Metrics,
    sessions: &mut BTreeMap<String, Vec<Json>>,
    lru: &mut VecDeque<String>,
    mention: &slack::Mention,
) {
    // Keyed per thread, because two questions in one channel are two conversations
    // and collapsing them is how a bot answers a question nobody asked here.
    let key = format!(
        "{}:{}",
        mention.channel,
        mention.thread_ts.clone().unwrap_or_else(|| mention.ts.clone())
    );
    let mut transcript = sessions
        .remove(&key)
        .unwrap_or_else(agent::seed_transcript);
    touch(lru, &key);
    let started = Instant::now();
    slack.react(&mention.channel, &mention.ts, &config.emoji);
    println!("turn: {} asks {:?}", mention.user, mention.text);
    let outcome = agent.ask(&mut transcript, &mention.text, |event| println!("   · {event}"));
    slack.done_reacting(&mention.channel, &mention.ts, &config.emoji);
    metrics.turns.fetch_add(1, Ordering::Relaxed);
    metrics
        .last_turn_ms
        .store(started.elapsed().as_millis() as u64, Ordering::Relaxed);

    let reply = match outcome {
        Ok(answer) => {
            metrics
                .tool_calls
                .fetch_add(answer.log.len() as u64, Ordering::Relaxed);
            println!(
                "answered via {} in {} ms ({} rounds, {} tool call(s))",
                answer.engine, answer.millis, answer.rounds, answer.log.len()
            );
            // Both halves stay in the transcript, so "and the workshop?" lands on
            // the numbers just fetched instead of re-fetching them.
            sessions.insert(key.clone(), transcript);
            slack::escape(&answer.text)
        }
        Err(e) => {
            metrics.errors.fetch_add(1, Ordering::Relaxed);
            // Keep neither the failure nor the question: a half-finished exchange in
            // the history makes the next turn worse as well as stranger.
            transcript.pop();
            sessions.insert(key.clone(), transcript);
            eprintln!("turn failed: {e}");
            format!(
                "I could not answer that: {e}\n\
                 (I have {} tools wired up, so this is almost certainly the inference \
                 engine or an upstream, not your question.)",
                agent.tool_count()
            )
        }
    };
    if sessions.len() > MAX_SESSIONS {
        if let Some(old) = lru.pop_front() {
            sessions.remove(&old);
        }
    }
    let (thread_ts, at) = slack::reply_target(
        mention.thread_ts.as_deref(),
        config.thread_replies,
        &mention.user,
    );
    // The mention goes on *after* escaping, or `<@U05…>` arrives escaped and Slack shows the
    // literal instead of pinging the person — which is the entire point of adding it.
    let reply = match at {
        Some(at) => format!("{at} {reply}"),
        None => reply,
    };
    if let Err(e) = slack.post(&mention.channel, thread_ts.as_deref(), &reply) {
        // An answer we cannot post is the one failure worth printing in full: the
        // alternative is a member waiting forever with no trace of the reply.
        eprintln!("cannot post the answer: {e}\nthe answer was: {reply}");
    }
}

fn touch(lru: &mut VecDeque<String>, key: &str) {
    lru.retain(|k| k != key);
    lru.push_back(key.to_string());
    while lru.len() > MAX_SESSIONS {
        lru.pop_front();
    }
}

#[derive(Default)]
struct Metrics {
    turns: AtomicU64,
    errors: AtomicU64,
    tool_calls: AtomicU64,
    last_turn_ms: AtomicU64,
}

/// Loopback health and Prometheus text, so "the bot is quiet" and "the bot is deaf"
/// are different answers — the dead-channel lesson from `docs/nix-dev-workflow.md`,
/// applied to a bot nobody can see inside.
fn serve_health(listen: &str, last_frame: &AtomicU64, metrics: &Metrics, stall: Duration) {
    let listener = match TcpListener::bind(listen) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot bind the health port {listen}: {e}");
            return;
        }
    };
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let mut buf = [0u8; 2048];
        let n = match stream.read(&mut buf) {
            Ok(n) => n,
            Err(_) => continue,
        };
        let path = String::from_utf8_lossy(&buf[..n])
            .split_whitespace()
            .nth(1)
            .unwrap_or("/")
            .split('?')
            .next()
            .unwrap_or("/")
            .to_string();
        let silent = slack::silence_secs(last_frame);
        let (status, reason, body) = match path.as_str() {
            "/health" if silent <= stall.as_secs() => (
                200,
                "OK",
                format!("ok, {silent}s since the last frame from Slack\n"),
            ),
            "/health" => (503, "Service Unavailable", format!("STALE: {silent}s since the last frame from Slack\n")),
            "/metrics" => (
                200,
                "OK",
                format!(
                    "# HELP slack_agent_turns_total Questions answered.\n\
                     # TYPE slack_agent_turns_total counter\n\
                     slack_agent_turns_total {}\n\
                     # HELP slack_agent_errors_total Turns that failed.\n\
                     # TYPE slack_agent_errors_total counter\n\
                     slack_agent_errors_total {}\n\
                     # HELP slack_agent_tool_calls_total Tool calls made.\n\
                     # TYPE slack_agent_tool_calls_total counter\n\
                     slack_agent_tool_calls_total {}\n\
                     # HELP slack_agent_last_turn_ms Duration of the most recent turn.\n\
                     # TYPE slack_agent_last_turn_ms gauge\n\
                     slack_agent_last_turn_ms {}\n\
                     # HELP slack_agent_seconds_since_slack_frame Liveness of the Socket Mode connection.\n\
                     # TYPE slack_agent_seconds_since_slack_frame gauge\n\
                     slack_agent_seconds_since_slack_frame {silent}\n",
                    metrics.turns.load(Ordering::Relaxed),
                    metrics.errors.load(Ordering::Relaxed),
                    metrics.tool_calls.load(Ordering::Relaxed),
                    metrics.last_turn_ms.load(Ordering::Relaxed)
                ),
            ),
            _ => (404, "Not Found", "not found; try /health or /metrics\n".to_string()),
        };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\n\
             Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    }
}

fn first_sentence(s: &str) -> String {
    let t = s.trim();
    match t.find(". ") {
        Some(i) if i < 160 => t[..i + 1].to_string(),
        _ => {
            let taken: String = t.chars().take(160).collect();
            if taken.chars().count() < t.chars().count() {
                format!("{taken}…")
            } else {
                taken
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_description_is_summarised_without_losing_a_unit() {
        assert_eq!(first_sentence("Cold here. Call this for temps."), "Cold here.");
        // No `". "` at all — e.g. one long sentence with a decimal in it — must come
        // back whole rather than cut at the decimal.
        assert_eq!(first_sentence("15.7 C is cold here"), "15.7 C is cold here");
        let long = "x".repeat(400);
        let s = first_sentence(&long);
        assert!(s.ends_with('…') && s.chars().count() == 161, "{} chars", s.chars().count());
    }

    /// The health endpoint exists to tell deaf from idle, so the silence number and
    /// the threshold are the behaviour under test.
    #[test]
    fn an_unstarted_clock_reads_as_silence_and_a_live_one_does_not() {
        let fresh = AtomicU64::new(slack::now_secs());
        assert!(slack::silence_secs(&fresh) <= 1);
        let never = AtomicU64::new(0);
        assert!(slack::silence_secs(&never) > DEFAULT_STALL_SECS);
    }

    #[test]
    fn the_lru_order_is_what_gets_evicted() {
        let mut lru = VecDeque::new();
        for k in ["a", "b", "c"] {
            touch(&mut lru, k);
        }
        touch(&mut lru, "a"); // using `a` again must move it to the back, not duplicate it
        assert_eq!(lru.len(), 3);
        assert_eq!(lru.front().map(String::as_str), Some("b"));
    }

    #[test]
    fn sessions_are_keyed_per_thread_not_per_channel() {
        let key = |channel: &str, thread: Option<&str>, ts: &str| format!("{channel}:{}", thread.unwrap_or(ts));
        assert_eq!(key("C1", Some("t1"), "t2"), "C1:t1");
        assert_ne!(key("C1", None, "t1"), key("C1", None, "t2"));
    }
}
