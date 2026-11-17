//! `listen` — the bot's answer on a socket, for whoever is asking.
//!
//! ## Why this exists at all
//!
//! Until now the only way to ask Vulcan a question was to be in Slack: the turn was
//! welded to a Slack event, and the answer was welded to a channel.  But the loop is
//! not Slack-shaped — `agent.rs` is an engine and a set of MCP tools, and
//! `main.rs`'s own `ask` verb has always run it with *"no Slack involved"*.  So this
//! is that loop with a socket in front of it, and it is what lets the voice front end
//! (`crates/voice-assistant/`) be a **client** rather than a second bot: one set of
//! tools, one grounding rule, one thing to audit, and a second one to write the next
//! time something else wants to ask (Grafana's alert evaluator, the switches' GUI, a
//! phone shortcut).
//!
//! ## What a request looks like
//!
//! ```text
//! POST /v1/ask  {"text": "is it cold in the workshop", "voice": true}
//! 200 {"answer":"…","engine":"services2 (RTX 3060)","rounds":2,
//!      "tools":["space_status"],"secs":7.74,"warnings":[]}
//! ```
//!
//! `voice` selects the *spoken* system prompt and token budget
//! (`prompt::spoken_system_prompt`, `agent::MAX_SPEECH_TOKENS`) — see below.  Errors
//! are `{"error": "…"}` with a status that means something: 429 when another turn is
//! in flight, 502 when no engine or no tool could answer.
//!
//! ## Two conversations, not one
//!
//! The system prompt is per-transcript, so a voice turn and a text turn cannot share
//! one without one of them inheriting the other's rules.  They are kept apart by
//! `Kind`, which is also the honest model of the world: a room talking to a speaker
//! and a person typing into a box are not the same conversation, and an "and the
//! workshop?" only resolves against its own channel's history.  Each is dropped after
//! `--reset-after-secs` of silence, because a transcript that has been sitting for an
//! hour is a source of stale facts, which is the one failure this bot's prompt exists
//! to prevent.
//!
//! ## One turn at a time
//!
//! The engine answers are serialised by a lock, and a second caller is refused rather
//! than queued.  A 20-second queue of voice questions is not what anyone wants: the
//! room has moved on, and `voice-assistant` reports the refusal as "the bot is busy"
//! instead of a silent timeout.  Slack's socket is unaffected — it has its own
//! ordering, which is the whole reason the two are separate units.
//!
//! ## Loopback, and who reaches it
//!
//! `--listen` off loopback is refused, like `hackspace-mcp`: no authentication is the
//! design *because* the reach is the boundary.  `voice-assistant` is on this box.  It
//! is not exposed through the front door, and neither is this socket.

use std::collections::{BTreeMap, VecDeque};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use common_json::{self as json, Json};
use common_service as service;
use common_web::{self as web, Reply, Request};

use crate::agent::{self, Agent, MAX_SPEECH_TOKENS};
use crate::engine;
use crate::prompt;

/// The turn's own timeout, from `--timeout-secs`, applied per engine attempt.
pub struct Options {
    pub listen: String,
    pub mcp: Vec<String>,
    pub endpoints: String,
    pub timeout: Duration,
    /// Silence after which the conversation starts again.
    pub reset_after: Duration,
    /// Keep the question text in `/status.json` — the same decision, and the same
    /// default, as `voice-assistant`'s gateway.
    pub record_questions: bool,
    /// Whether the engine may think out loud before answering. The voice channel is the one
    /// where this is *audible*: every round of private monologue is a second of silence in
    /// front of a person who asked a question out loud.
    pub thinking: crate::engine::Thinking,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Text,
    Voice,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Voice => "voice",
        }
    }

    fn system(self) -> String {
        match self {
            Kind::Text => prompt::system_prompt(),
            // The spoken prompt *is* the Slack one plus the override; one grounding
            // rule, so the two can never disagree about what counts as a fact.
            Kind::Voice => prompt::spoken_system_prompt(),
        }
    }

    fn budget(self) -> u32 {
        match self {
            Kind::Text => agent::MAX_ANSWER_TOKENS,
            Kind::Voice => MAX_SPEECH_TOKENS,
        }
    }
}

struct Session {
    transcript: Vec<Json>,
    last: Instant,
}

#[derive(Default)]
struct Stats {
    turns: AtomicU64,
    errors: AtomicU64,
    tool_calls: AtomicU64,
    last_turn_ms: AtomicU64,
    last_error: Mutex<String>,
    recent: Mutex<VecDeque<String>>,
}

struct Ctx {
    agent: Mutex<Agent>,
    sessions: Mutex<BTreeMap<Kind, Session>>,
    opts: Options,
    started: Instant,
    stats: Stats,
    tools: Vec<String>,
    startup_warnings: Vec<String>,
}

pub fn run(opts: Options) -> Result<(), String> {
    // Before anything is built: an off-loopback bind must fail without first
    // connecting to every MCP server, because that is the difference between a
    // refused configuration and a leaked tool connection.
    if !service::is_loopback(&opts.listen) {
        return Err(format!(
            "refusing to bind {}: this socket has no authentication and its answers come from \
             tools that hold the building's read tokens. `voice-assistant` is on this box; \
             nothing off it is meant to reach this port.",
            opts.listen
        ));
    }
    let mut engines = engine::Engines::new(engine::parse_candidates(&opts.endpoints), opts.timeout);
    engines.set_thinking(opts.thinking);
    let (agent, warnings) = Agent::new(engines, &opts.mcp);
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    if agent.tool_count() == 0 {
        return Err(format!(
            "no tools at all ({} MCP server(s) configured), so every answer would be invented",
            opts.mcp.len()
        ));
    }
    let tools = agent.tool_names();
    let ctx = Arc::new(Ctx {
        agent: Mutex::new(agent),
        sessions: Mutex::new(BTreeMap::new()),
        started: Instant::now(),
        stats: Stats::default(),
        tools,
        startup_warnings: warnings,
        opts,
    });
    let listener = TcpListener::bind(&ctx.opts.listen).map_err(|e| format!("bind: {e}"))?;
    println!(
        "slack-agent listen on {} — {} tools from {:?}; voice prompt on, reset after {}s",
        ctx.opts.listen,
        ctx.tools.len(),
        ctx.opts.mcp,
        ctx.opts.reset_after.as_secs(),
    );
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let ctx = Arc::clone(&ctx);
        thread::spawn(move || {
            if let Err(e) = handle(stream, &ctx) {
                eprintln!("connection: {e}");
            }
        });
    }
    Ok(())
}

fn handle(mut stream: std::net::TcpStream, ctx: &Arc<Ctx>) -> std::io::Result<()> {
    // Longer than the agent's own turn budget: the client gives up first, and says so.
    stream.set_read_timeout(Some(ctx.opts.timeout * 3 + Duration::from_secs(30)))?;
    let Some(req) = web::read_request(&mut stream) else {
        return Ok(());
    };
    route(&req, ctx).send(&mut stream)
}

fn route(req: &Request, ctx: &Arc<Ctx>) -> Reply {
    match (req.method.as_str(), req.path()) {
        ("GET", "/health") => {
            let s = &ctx.stats;
            Reply::json(
                200,
                &format!(
                    "{{\"status\":\"ok\",\"uptime_secs\":{:.0},\"turns\":{},\"errors\":{},\
                     \"tools\":{},\"last_error\":{}}}",
                    ctx.started.elapsed().as_secs_f64(),
                    s.turns.load(Ordering::Relaxed),
                    s.errors.load(Ordering::Relaxed),
                    ctx.tools.len(),
                    json::quoted(&s.last_error.lock().unwrap_or_else(|p| p.into_inner()).clone()),
                ),
            )
        }
        ("GET", "/metrics") => Reply::new(
            200,
            "text/plain; version=0.0.4; charset=utf-8",
            prometheus(ctx).into_bytes(),
        ),
        ("GET", "/status.json") => status(ctx),
        ("POST", "/v1/ask") => ask(req, ctx),
        _ => Reply::text(404, "not found; try /health, /metrics, /status.json or POST /v1/ask"),
    }
}

/// One turn.  The whole body of this function is the reason the voice work needed no
/// new bot: parse, ask, report.
fn ask(req: &Request, ctx: &Arc<Ctx>) -> Reply {
    let body = match json::parse(&req.body_text()) {
        Ok(v) => v,
        Err(e) => return Reply::json(400, &json::error_object(&format!("body is not JSON: {e}"))),
    };
    let question = match body.get("text").and_then(Json::as_str) {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => {
            return Reply::json(
                400,
                &json::error_object("expected {\"text\": \"the question\", \"voice\": true|false}"),
            )
        }
    };
    let kind = if body.get("voice").and_then(Json::as_bool) == Some(false) {
        Kind::Text
    } else {
        Kind::Voice
    };
    let mut guard = match ctx.agent.try_lock() {
        Ok(agent) => agent,
        Err(_) => {
            ctx.stats.errors.fetch_add(1, Ordering::Relaxed);
            return Reply::json(
                429,
                &json::error_object("another turn is in flight; this endpoint answers one at a time"),
            );
        }
    };
    let mut transcript = take_transcript(&ctx.sessions, kind, ctx.opts.reset_after);
    let started = Instant::now();
    let outcome = guard.ask_budget(
        &mut transcript,
        &question,
        kind.budget(),
        |event| eprintln!("   · {event}"),
    );
    let answer = match outcome {
        Ok(a) => a,
        Err(e) => {
            // A failed turn does not keep its half-asked question in the history: the
            // next "and the workshop?" would resolve against a transcript whose tool
            // results never arrived.
            ctx.stats.errors.fetch_add(1, Ordering::Relaxed);
            *ctx.stats.last_error.lock().unwrap_or_else(|p| p.into_inner()) = e.clone();
            return Reply::json(502, &json::error_object(&e));
        }
    };
    keep_transcript(&ctx.sessions, kind, transcript);
    let secs = started.elapsed().as_secs_f64();
    ctx.stats.turns.fetch_add(1, Ordering::Relaxed);
    ctx.stats
        .tool_calls
        .fetch_add(answer.tools_used.len() as u64, Ordering::Relaxed);
    ctx.stats
        .last_turn_ms
        .store(answer.millis as u64, Ordering::Relaxed);
    record(ctx, &question, &answer.text, secs);
    let tools: Vec<String> = answer.tools_used.iter().map(|t| json::quoted(t)).collect();
    Reply::json(
        200,
        &format!(
            "{{\"answer\":{},\"engine\":{},\"rounds\":{},\"tools\":[{}],\"secs\":{:.2},\
             \"millis\":{},\"prompt_tokens\":{},\"completion_tokens\":{},\"channel\":{},\
             \"warnings\":[]}}",
            json::quoted(answer.text.trim()),
            json::quoted(&answer.engine),
            answer.rounds,
            tools.join(","),
            secs,
            answer.millis,
            answer.prompt_tokens,
            answer.completion_tokens,
            json::quoted(kind.as_str()),
        ),
    )
}

/// The transcript for a channel, fresh if it has expired.  Split out from the socket
/// code because "when does a conversation start again" is the whole policy, and it is
/// the part worth a test.
fn take_transcript(
    sessions: &Mutex<BTreeMap<Kind, Session>>,
    kind: Kind,
    reset_after: Duration,
) -> Vec<Json> {
    let mut sessions = sessions.lock().unwrap_or_else(|p| p.into_inner());
    match sessions.get_mut(&kind) {
        Some(s) if s.last.elapsed() <= reset_after => s.transcript.clone(),
        _ => agent::seed_with(&kind.system()),
    }
}

fn keep_transcript(
    sessions: &Mutex<BTreeMap<Kind, Session>>,
    kind: Kind,
    transcript: Vec<Json>,
) {
    sessions
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(kind, Session { transcript, last: Instant::now() });
}

fn record(ctx: &Arc<Ctx>, question: &str, answer: &str, secs: f64) {
    if !ctx.opts.record_questions {
        return;
    }
    let line = format!(
        "{} — {} → {} [{:.1} s]",
        epoch(),
        question.replace('\n', " "),
        answer.replace('\n', " "),
        secs
    );
    let mut recent = ctx.stats.recent.lock().unwrap_or_else(|p| p.into_inner());
    recent.push_back(line);
    while recent.len() > 20 {
        recent.pop_front();
    }
}

fn status(ctx: &Arc<Ctx>) -> Reply {
    let recent: Vec<String> = ctx
        .stats
        .recent
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .rev()
        .map(|l| json::quoted(l))
        .collect();
    let tools: Vec<String> = ctx.tools.iter().map(|t| json::quoted(t)).collect();
    let warns: Vec<String> = ctx
        .startup_warnings
        .iter()
        .map(|w| json::quoted(w))
        .collect();
    let sessions: Vec<String> = ctx
        .sessions
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .map(|(k, s)| {
            format!(
                "{{\"channel\":{},\"age_secs\":{:.0}}}",
                json::quoted(k.as_str()),
                s.last.elapsed().as_secs_f64()
            )
        })
        .collect();
    Reply::json(
        200,
        &format!(
            "{{\"uptime_secs\":{:.0},\"listen\":{},\"turns\":{},\"errors\":{},\
             \"tool_calls\":{},\"last_turn_ms\":{},\"last_error\":{},\"tools\":[{}],\
             \"tool_count\":{},\"sessions\":[{}],\"mcp\":[{}],\"startup_warnings\":[{}],\
             \"record_questions\":{},\"thinking\":{},\"recent\":[{}]}}",
            ctx.started.elapsed().as_secs_f64(),
            json::quoted(&ctx.opts.listen),
            ctx.stats.turns.load(Ordering::Relaxed),
            ctx.stats.errors.load(Ordering::Relaxed),
            ctx.stats.tool_calls.load(Ordering::Relaxed),
            ctx.stats.last_turn_ms.load(Ordering::Relaxed),
            json::quoted(&ctx.stats.last_error.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            tools.join(","),
            ctx.tools.len(),
            sessions.join(","),
            ctx.opts
                .mcp
                .iter()
                .map(|m| json::quoted(m))
                .collect::<Vec<_>>()
                .join(","),
            warns.join(","),
            ctx.opts.record_questions,
            json::quoted(ctx.opts.thinking.label()),
            recent.join(","),
        ),
    )
}

/// The same shape as `serve`'s `/metrics`, with `ask` in the name, so the two sockets
/// can be scraped at once and a dashboard can tell "the Slack bot is quiet" from "the
/// voice bot is quiet".
fn prometheus(ctx: &Arc<Ctx>) -> String {
    format!(
        "# HELP slack_agent_ask_turns_total Questions answered on this socket.\n\
         # TYPE slack_agent_ask_turns_total counter\n\
         slack_agent_ask_turns_total {}\n\
         # HELP slack_agent_ask_errors_total Turns that failed.\n\
         # TYPE slack_agent_ask_errors_total counter\n\
         slack_agent_ask_errors_total {}\n\
         # HELP slack_agent_ask_tool_calls_total Tool calls made.\n\
         # TYPE slack_agent_ask_tool_calls_total counter\n\
         slack_agent_ask_tool_calls_total {}\n\
         # HELP slack_agent_ask_last_turn_ms Duration of the most recent turn.\n\
         # TYPE slack_agent_ask_last_turn_ms gauge\n\
         slack_agent_ask_last_turn_ms {}\n\
         # HELP slack_agent_ask_tools Tools offered by the configured MCP servers.\n\
         # TYPE slack_agent_ask_tools gauge\n\
         slack_agent_ask_tools {}\n\
         # HELP slack_agent_ask_uptime_secs Since the endpoint started.\n\
         # TYPE slack_agent_ask_uptime_secs gauge\n\
         slack_agent_ask_uptime_secs {:.0}\n",
        ctx.stats.turns.load(Ordering::Relaxed),
        ctx.stats.errors.load(Ordering::Relaxed),
        ctx.stats.tool_calls.load(Ordering::Relaxed),
        ctx.stats.last_turn_ms.load(Ordering::Relaxed),
        ctx.tools.len(),
        ctx.started.elapsed().as_secs_f64(),
    )
}

fn epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(listen: &str) -> Options {
        Options {
            listen: listen.to_string(),
            mcp: vec!["http://127.0.0.1:1/mcp".to_string()],
            endpoints: "http://127.0.0.1:1/v1|nowhere".to_string(),
            timeout: Duration::from_secs(5),
            reset_after: Duration::from_millis(30),
            record_questions: true,
            thinking: crate::engine::Thinking::On,
        }
    }

    /// The one test that must never be able to touch the network: the bind check runs
    /// before any MCP server is contacted, so a mis-set flag fails in milliseconds
    /// rather than after a connect timeout per server.
    #[test]
    fn an_off_loopback_bind_is_refused_before_anything_is_built() {
        let e = run(opts("10.3.1.20:8194")).unwrap_err();
        assert!(e.contains("no authentication"), "{e}");
        assert!(e.contains("10.3.1.20"), "{e}");
        // Rule 17's shape: a hostname that merely looks local is not loopback.
        assert!(run(opts("127.example.com:8194")).is_err());
    }

    #[test]
    fn the_voice_channel_is_the_one_with_the_spoken_rules() {
        let text = Kind::Text.system();
        let voice = Kind::Voice.system();
        assert!(voice.len() > text.len(), "voice prompt is not the text one plus the override");
        assert!(voice.starts_with(&text), "the voice prompt does not inherit the grounding rule");
        assert_eq!(Kind::Text.budget(), agent::MAX_ANSWER_TOKENS);
        assert_eq!(Kind::Voice.budget(), MAX_SPEECH_TOKENS);
        // The ordering of the two budgets is *not* asserted here: `agent.rs` makes it a
        // compile-time constant assertion, so this test checks what is only visible at
        // run time — that a channel actually gets its own number.
        assert_eq!(Kind::Voice.as_str(), "voice");
    }

    #[test]
    fn a_conversation_expires_and_the_other_channel_is_unaffected() {
        let sessions: Mutex<BTreeMap<Kind, Session>> = Mutex::new(BTreeMap::new());
        // First ask on each: seeded transcripts, and they differ by system prompt.
        let voice1 = take_transcript(&sessions, Kind::Voice, Duration::from_secs(600));
        let text1 = take_transcript(&sessions, Kind::Text, Duration::from_secs(600));
        assert_ne!(
            voice1[0].get("content").and_then(Json::as_str),
            text1[0].get("content").and_then(Json::as_str),
            "both channels seeded the same prompt, so the voice override is not applied"
        );
        // A turn lands, and the next one on the same channel inherits it.
        let mut grown = voice1.clone();
        grown.push(json::object(vec![
            ("role".to_string(), json::string("user")),
            ("content".to_string(), json::string("and the workshop?")),
        ]));
        keep_transcript(&sessions, Kind::Voice, grown);
        assert_eq!(take_transcript(&sessions, Kind::Voice, Duration::from_secs(600)).len(), voice1.len() + 1);
        // Text never sees it.
        assert_eq!(take_transcript(&sessions, Kind::Text, Duration::from_secs(600)).len(), text1.len());
        // And after the silence window, the history is gone rather than stale.
        keep_transcript(&sessions, Kind::Voice, voice1.clone());
        thread::sleep(Duration::from_millis(40));
        assert_eq!(
            take_transcript(&sessions, Kind::Voice, Duration::from_millis(10)).len(),
            voice1.len(),
            "an expired conversation kept its history"
        );
    }

    #[test]
    fn the_metrics_are_the_shape_prometheus_reads() {
        let ctx = Arc::new(Ctx {
            agent: Mutex::new(make_stub_agent()),
            sessions: Mutex::new(BTreeMap::new()),
            opts: opts("127.0.0.1:0"),
            started: Instant::now(),
            stats: Stats::default(),
            tools: vec!["space_status".to_string(), "camera_activity".to_string()],
            startup_warnings: vec!["one server was down".to_string()],
        });
        ctx.stats.turns.store(3, Ordering::Relaxed);
        let text = prometheus(&ctx);
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let (name, value) = line.split_once(' ').unwrap_or_else(|| panic!("bad metric line {line:?}"));
            assert!(!name.is_empty() && !value.is_empty(), "{line:?}");
            assert!(name.starts_with("slack_agent_ask_"), "{name} is not prefixed");
            assert!(value.parse::<f64>().is_ok(), "{line:?} is not a number");
        }
        assert!(text.contains("slack_agent_ask_tools 2"), "{text}");
        assert!(text.contains("slack_agent_ask_turns_total 3"));
        // And status.json must parse, because `voice-assistant` reads it.
        let reply = status(&ctx);
        let body = String::from_utf8(reply.body.clone()).unwrap();
        let parsed = json::parse(&body).unwrap_or_else(|e| panic!("status.json is not JSON ({e}): {body}"));
        // Which thinking mode this socket runs is a fact the caller is entitled to: it is
        // the difference between a 7 s answer and a 12 s one, and it is not visible in the
        // answer itself.
        assert_eq!(
            parsed.get("thinking").and_then(|v| v.as_str()),
            Some("on"),
            "status.json does not say how the engine is thinking: {body}"
        );
        assert_eq!(parsed.get("tool_count").and_then(Json::as_f64), Some(2.0));
        let warnings = parsed.get("startup_warnings").and_then(Json::as_array).expect("array");
        assert_eq!(warnings[0].as_str(), Some("one server was down"));
        assert!(parsed.get("recent").and_then(Json::as_array).is_some());
    }

    /// An Agent that is never asked anything: these tests only read the metadata and
    /// the session policy, so no engine and no MCP connection is needed — but the
    /// struct needs *one*.  Zero servers is honest here: `Agent::new` with an empty
    /// list yields an agent with no tools, which is exactly what we want to hold in a
    /// test that must not open a socket.
    fn make_stub_agent() -> Agent {
        let engines = engine::Engines::new(vec![], Duration::from_millis(1));
        // `Thinking::On` is the default and this stub never reaches an engine.
        let (agent, _warnings) = Agent::new(engines, &[]);
        assert_eq!(agent.tool_count(), 0, "the stub must not reach out");
        agent
    }
}
