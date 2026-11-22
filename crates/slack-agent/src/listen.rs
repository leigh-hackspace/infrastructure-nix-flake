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
//! ## Streaming: `/v1/ask/stream`, and why a turn has to be written as it happens
//!
//! `POST /v1/ask` can only answer once. A voice turn cannot wait for that: measured decode is
//! 14–39 tok/s (`docs/slack-agent-2026-11-16.md`), so the difference between a whole answer and
//! its first sentence is 2–4 seconds of a room standing still — and the first sentence is all a
//! speaker needs to *start*. So `/v1/ask/stream` runs the same loop and writes it out as
//! server-sent events (`common_web::stream`): `start`, `log`, `partial`, `reset`, `answer`,
//! `error`. `/v1/ask` stays exactly as it is, because a caller that wants one JSON object should
//! not have to parse a stream.
//!
//! `POST /v1/ask/prefill` is the other half: a turn's prompt prefix (system prompt + tool
//! schemas + the question as far as it is known) sent to the engine for **one token**, so the
//! real turn prefills only its tail. It is deliberately not a turn: it takes no transcript, keeps
//! no history, and refuses politely when a turn is in flight rather than queueing behind one.
//! Whether it pays is measurable from this socket's own counters (`prefills`,
//! `prefill_cached_tokens`).
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
    /// How many of those turns were answered with one tool call and no model. The hit rate is
    /// the whole point of the fast path: it says which questions people actually ask, and
    /// when it is 100 % of a quiet hour, the model was never needed for that hour at all.
    quick: AtomicU64,
    last_turn_ms: AtomicU64,
    /// Warm-up requests accepted, and the tokens the engine reported as **reused** for them.
    /// Both counters exist so the question "does prefilling pay here" is answered by this
    /// socket rather than argued: `prefills` climbing while `prefill_cached_tokens` stays at 0
    /// is a feature costing a request and buying nothing.
    prefills: AtomicU64,
    prefill_prompt_tokens: AtomicU64,
    prefill_cached_tokens: AtomicU64,
    last_prefill_ms: AtomicU64,
    /// `partial` events written by `/v1/ask/stream` — the stream's own liveness counter.
    parts: AtomicU64,
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
    // The two routes that answer *as they work* must own the socket, so they are matched before
    // the buffered router — which is the only reason `route` gets to keep returning a `Reply`.
    if req.method == "POST" && matches!(req.path(), "/v1/ask/stream" | "/v1/ask/prefill") {
        return streaming(req, stream, ctx);
    }
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
    let (question, kind) = match parse_ask(req) {
        Ok(v) => v,
        Err(why) => return Reply::json(400, &json::error_object(&why)),
    };
    // **The voice channel is where a fast path is audible.** Slack can tolerate nine seconds,
    // because a person walked away and came back; a room with people standing in front of a
    // speaker cannot. Same bargain `answer_events` has made since phase 1, and the same rule
    // on failure: a tool declining is not an answer, it is a reason to go ask the model.
    let quick_started = Instant::now();
    let quick = ctx
        .agent
        .try_lock()
        .ok()
        .and_then(|mut a| a.quick_answer(&question, |w| eprintln!("warning: {w}")));
    if let Some(hit) = quick {
        let ms = quick_started.elapsed().as_millis() as u64;
        let secs = ms as f64 / 1000.0;
        ctx.stats.quick.fetch_add(1, Ordering::Relaxed);
        ctx.stats.turns.fetch_add(1, Ordering::Relaxed);
        ctx.stats.tool_calls.fetch_add(1, Ordering::Relaxed);
        ctx.stats.last_turn_ms.store(ms, Ordering::Relaxed);
        record(ctx, &question, &hit.text, secs);
        eprintln!("deterministic: {} answered in {ms} ms (no model)", hit.label);
        return Reply::json(
            200,
            &format!(
                "{{\"answer\":{},\"engine\":\"none (fast path)\",\"rounds\":0,\"tools\":[{}],\
                 \"secs\":{secs:.2},\"millis\":{ms},\"prompt_tokens\":0,\"completion_tokens\":0,\
                 \"channel\":{},\"warnings\":[]}}",
                json::quoted(hit.text.trim()),
                json::quoted(&hit.label),
                json::quoted(kind.as_str()),
            ),
        );
    }
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

/// The one body both answer routes accept: the question, and which channel it came in on.
///
/// Shared because the two routes must not be able to disagree about what a request means — a
/// `/v1/ask/stream` that picked a different prompt or budget from `/v1/ask` for the same body
/// would be the same bug as two system prompts, just harder to see.
fn parse_ask(req: &Request) -> Result<(String, Kind), String> {
    let body = json::parse(&req.body_text()).map_err(|e| format!("body is not JSON: {e}"))?;
    let question = body
        .get("text")
        .and_then(Json::as_str)
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| "expected {\"text\": \"the question\", \"voice\": true|false}".to_string())?;
    let kind = if body.get("voice").and_then(Json::as_bool) == Some(false) {
        Kind::Text
    } else {
        Kind::Voice
    };
    Ok((question, kind))
}

/// The two routes that stream.  They take the socket by value because from here on the response
/// is written over time rather than returned.
fn streaming(
    req: Request,
    mut stream: std::net::TcpStream,
    ctx: &Arc<Ctx>,
) -> std::io::Result<()> {
    if req.path() == "/v1/ask/prefill" {
        return prefill(&req, &mut stream, ctx);
    }
    ask_stream(&req, stream, ctx)
}

/// `POST /v1/ask/prefill` — warm the engine with a turn's prompt prefix and nothing else.
fn prefill(req: &Request, stream: &mut std::net::TcpStream, ctx: &Arc<Ctx>) -> std::io::Result<()> {
    let (question, kind) = match parse_ask(req) {
        Ok(v) => v,
        Err(why) => return Reply::json(400, &json::error_object(&why)).send(stream),
    };
    // A warm-up never waits behind a live turn: it is an optimisation for the turn that is
    // coming, and queueing it would delay the one that is running.  `skipped` is a 200 because
    // the caller asked for a favour, not a guarantee.
    let mut guard = match ctx.agent.try_lock() {
        Ok(g) => g,
        Err(_) => {
            return Reply::json(
                200,
                r#"{"ok":false,"skipped":"a turn is in flight; a warm-up must not delay it"}"#,
            )
            .send(stream)
        }
    };
    let transcript = take_transcript(&ctx.sessions, kind, ctx.opts.reset_after);
    match guard.prefill(&transcript, &question) {
        Ok(p) => {
            ctx.stats.prefills.fetch_add(1, Ordering::Relaxed);
            ctx.stats
                .prefill_prompt_tokens
                .fetch_add(p.prompt_tokens, Ordering::Relaxed);
            ctx.stats
                .prefill_cached_tokens
                .fetch_add(p.cached_tokens, Ordering::Relaxed);
            ctx.stats
                .last_prefill_ms
                .store(p.millis as u64, Ordering::Relaxed);
            eprintln!(
                "prefill: {} cached {}/{} prompt tokens in {} ms",
                p.engine, p.cached_tokens, p.prompt_tokens, p.millis
            );
            Reply::json(
                200,
                &format!(
                    "{{\"ok\":true,\"engine\":{},\"prompt_tokens\":{},\"cached_tokens\":{},\
                     \"millis\":{},\"channel\":{}}}",
                    json::quoted(&p.engine),
                    p.prompt_tokens,
                    p.cached_tokens,
                    p.millis,
                    json::quoted(kind.as_str()),
                ),
            )
            .send(stream)
        }
        Err(e) => {
            *ctx.stats.last_error.lock().unwrap_or_else(|p| p.into_inner()) = e.clone();
            Reply::json(502, &json::error_object(&e)).send(stream)
        }
    }
}

/// `POST /v1/ask/stream` — the same turn as [`ask`], written out while it happens.
///
/// The event names are the contract (`docs/voice-assistant-2026-11-16.md` keeps the receipts);
/// the shape of each is deliberately boring JSON, because the caller is a voice pipeline that
/// has to decide, per event, whether to speak.
fn ask_stream(
    req: &Request,
    mut stream: std::net::TcpStream,
    ctx: &Arc<Ctx>,
) -> std::io::Result<()> {
    let (question, kind) = match parse_ask(req) {
        Ok(v) => v,
        Err(why) => return Reply::json(400, &json::error_object(&why)).send(&mut stream),
    };
    // RefCell rather than a second `&mut`: the loop's two callbacks (`on_event` for the plumbing
    // lines, `on_piece` for the answer's text) both need the same writer, they are never called
    // concurrently, and they run on this thread — which is what makes a cell correct and a
    // Mutex mere ceremony.
    let sse = std::cell::RefCell::new(web::Sse::open(stream)?);
    let _ = sse.borrow_mut().event(
        "start",
        &format!(
            "{{\"channel\":{},\"question\":{},\"tools\":{}}}",
            json::quoted(kind.as_str()),
            json::quoted(&question),
            json::emit(&Json::Arr(ctx.tools.iter().map(|t| Json::Str(t.clone())).collect()), 0),
        ),
    );

    // The deterministic fast path first, exactly as `ask` runs it: a question the calendar or
    // the clock can answer should not open a model stream at all, and `first_ms == 0` in the
    // answer event says honestly that no model was involved.
    let started = Instant::now();
    let quick = ctx
        .agent
        .try_lock()
        .ok()
        .and_then(|mut a| a.quick_answer(&question, |w| eprintln!("warning: {w}")));
    if let Some(hit) = quick {
        let ms = started.elapsed().as_millis() as u64;
        let secs = ms as f64 / 1000.0;
        ctx.stats.quick.fetch_add(1, Ordering::Relaxed);
        ctx.stats.turns.fetch_add(1, Ordering::Relaxed);
        ctx.stats.tool_calls.fetch_add(1, Ordering::Relaxed);
        ctx.stats.last_turn_ms.store(ms, Ordering::Relaxed);
        record(ctx, &question, &hit.text, secs);
        eprintln!("deterministic: {} answered in {ms} ms (no model)", hit.label);
        let _ = sse.borrow_mut().event(
            "answer",
            &format!(
                "{{\"answer\":{},\"engine\":\"none (fast path)\",\"rounds\":0,\"tools\":[{}],\
                 \"secs\":{secs:.2},\"millis\":{ms},\"prompt_tokens\":0,\
                 \"completion_tokens\":0,\"channel\":{},\"first_token_ms\":0,\"parts\":0}}",
                json::quoted(hit.text.trim()),
                json::quoted(&hit.label),
                json::quoted(kind.as_str()),
            ),
        );
        return sse.borrow_mut().finish();
    }

    let mut guard = match ctx.agent.try_lock() {
        Ok(agent) => agent,
        Err(_) => {
            ctx.stats.errors.fetch_add(1, Ordering::Relaxed);
            let _ = sse.borrow_mut().event(
                "error",
                &json::error_object("another turn is in flight; this endpoint answers one at a time"),
            );
            return sse.borrow_mut().finish();
        }
    };

    let mut transcript = take_transcript(&ctx.sessions, kind, ctx.opts.reset_after);
    let mut first_token_ms = 0u64;
    let mut parts = 0u64;
    let outcome = guard.ask_budget_stream(
        &mut transcript,
        &question,
        kind.budget(),
        |event| {
            eprintln!("   · {event}");
            let _ = sse.borrow_mut()
                .event("log", &format!("{{\"line\":{}}}", json::quoted(event)));
        },
        |piece| match piece {
            agent::Piece::Reset => {
                let _ = sse.borrow_mut().event("reset", "{}");
            }
            agent::Piece::Text(t) => {
                if first_token_ms == 0 {
                    first_token_ms = started.elapsed().as_millis() as u64;
                }
                parts += 1;
                let _ = sse
                    .borrow_mut()
                    .event("partial", &format!("{{\"t\":{}}}", json::quoted(&t)));
            }
        },
    );
    let answer = match outcome {
        Ok(a) => a,
        Err(e) => {
            ctx.stats.errors.fetch_add(1, Ordering::Relaxed);
            *ctx.stats.last_error.lock().unwrap_or_else(|p| p.into_inner()) = e.clone();
            let _ = sse
                .borrow_mut()
                .event("error", &json::error_object(&e));
            return sse.borrow_mut().finish();
        }
    };
    keep_transcript(&ctx.sessions, kind, transcript);
    let secs = started.elapsed().as_secs_f64();
    let ms = started.elapsed().as_millis() as u64;
    ctx.stats.turns.fetch_add(1, Ordering::Relaxed);
    ctx.stats
        .tool_calls
        .fetch_add(answer.tools_used.len() as u64, Ordering::Relaxed);
    ctx.stats.parts.fetch_add(parts, Ordering::Relaxed);
    ctx.stats.last_turn_ms.store(ms, Ordering::Relaxed);
    record(ctx, &question, &answer.text, secs);
    let tools: Vec<String> = answer.tools_used.iter().map(|t| json::quoted(t)).collect();
    let _ = sse.borrow_mut().event(
        "answer",
        &format!(
            "{{\"answer\":{},\"engine\":{},\"rounds\":{},\"tools\":[{}],\"secs\":{secs:.2},\
             \"millis\":{ms},\"prompt_tokens\":{},\"completion_tokens\":{},\"channel\":{},\
             \"first_token_ms\":{first_token_ms},\"parts\":{parts}}}",
            json::quoted(answer.text.trim()),
            json::quoted(&answer.engine),
            answer.rounds,
            tools.join(","),
            answer.prompt_tokens,
            answer.completion_tokens,
            json::quoted(kind.as_str()),
        ),
    );
    // Bound in a local so the `RefMut` is dropped before `sse` is; as a bare tail expression
    // its destructor would run after the `RefCell` it borrows.
    let ended = sse.borrow_mut().finish();
    ended
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
             \"record_questions\":{},\"thinking\":{},\"quick\":{},\"prefills\":{},\
             \"prefill_cached_tokens\":{},\"prefill_prompt_tokens\":{},\"last_prefill_ms\":{},\
             \"parts\":{},\"recent\":[{}]}}",
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
            ctx.stats.quick.load(Ordering::Relaxed),
            ctx.stats.prefills.load(Ordering::Relaxed),
            ctx.stats.prefill_cached_tokens.load(Ordering::Relaxed),
            ctx.stats.prefill_prompt_tokens.load(Ordering::Relaxed),
            ctx.stats.last_prefill_ms.load(Ordering::Relaxed),
            ctx.stats.parts.load(Ordering::Relaxed),
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
         # HELP slack_agent_ask_prefills_total Warm-up requests the engine answered.\n\
         # TYPE slack_agent_ask_prefills_total counter\n\
         slack_agent_ask_prefills_total {}\n\
         # HELP slack_agent_ask_prefill_cached_tokens_total Prompt tokens the engine reported reusing for warm-ups.\n\
         # TYPE slack_agent_ask_prefill_cached_tokens_total counter\n\
         slack_agent_ask_prefill_cached_tokens_total {}\n\
         # HELP slack_agent_ask_parts_total `partial` events streamed by /v1/ask/stream.\n\
         # TYPE slack_agent_ask_parts_total counter\n\
         slack_agent_ask_parts_total {}\n\
         # HELP slack_agent_ask_uptime_secs Since the endpoint started.\n\
         # TYPE slack_agent_ask_uptime_secs gauge\n\
         slack_agent_ask_uptime_secs {:.0}\n",
        ctx.stats.turns.load(Ordering::Relaxed),
        ctx.stats.errors.load(Ordering::Relaxed),
        ctx.stats.tool_calls.load(Ordering::Relaxed),
        ctx.stats.last_turn_ms.load(Ordering::Relaxed),
        ctx.tools.len(),
        ctx.stats.prefills.load(Ordering::Relaxed),
        ctx.stats.prefill_cached_tokens.load(Ordering::Relaxed),
        ctx.stats.parts.load(Ordering::Relaxed),
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

    /// The streaming route's failure shape, over a real socket: a header event, then an `error`
    /// event naming why, and a **terminated** body.  A stream that dies with a bare socket close
    /// is indistinguishable from a network failure at the other end, which is the difference
    /// between the voice gateway saying "the engine failed" and saying "cannot reach the bot".
    #[test]
    fn a_streamed_turn_that_fails_still_ends_its_body_with_a_reason() {
        let ctx = Arc::new(Ctx {
            agent: Mutex::new(make_stub_agent()),
            sessions: Mutex::new(BTreeMap::new()),
            opts: opts("127.0.0.1:0"),
            started: Instant::now(),
            stats: Stats::default(),
            tools: vec!["space_status".to_string()],
            startup_warnings: vec![],
        });
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let serve_ctx = Arc::clone(&ctx);
        thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let req = web::read_request(&mut s.try_clone().unwrap()).unwrap();
            ask_stream(&req, s, &serve_ctx).unwrap();
        });
        let mut c = std::net::TcpStream::connect(addr).unwrap();
        use std::io::Write;
        let body = r#"{"text":"is it open","voice":true}"#;
        c.write_all(
            format!(
                "POST /v1/ask/stream HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .unwrap();
        c.flush().unwrap();
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut c, &mut buf).ok();
        let text = String::from_utf8_lossy(&buf).into_owned();
        assert!(text.contains("text/event-stream"), "{text}");
        assert!(text.contains("X-Accel-Buffering: no"), "nginx must not buffer it: {text}");
        assert!(text.contains("event: start"), "{text}");
        assert!(text.contains("event: error"), "no reason given: {text}");
        // The zero-length chunk must be the very last thing on the wire; trimming would erase
        // exactly the bytes this asserts about.
        assert!(text.ends_with("0\r\n\r\n"), "body not terminated: {text:?}");
    }

    /// Both answer routes must read a body the same way, or a streamed ask and a buffered one
    /// answer different questions.
    #[test]
    fn the_two_answer_routes_parse_one_body_the_same_way() {
        let body = |raw: &str| {
            web::Request {
                method: "POST".into(),
                raw_path: "/v1/ask".into(),
                headers: Default::default(),
                body: raw.as_bytes().to_vec(),
            }
        };
        let (q, kind) = parse_ask(&body(r#"{"text":"  is it cold  "}"#)).unwrap();
        assert_eq!(q, "is it cold");
        assert_eq!(kind, Kind::Voice, "a missing flag is the voice default");
        assert_eq!(parse_ask(&body(r#"{"text":"x","voice":false}"#)).unwrap().1, Kind::Text);
        assert!(parse_ask(&body(r#"{"text":"   "}"#)).unwrap_err().contains("text"));
        assert!(parse_ask(&body("not json")).unwrap_err().contains("not JSON"));
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
        assert_eq!(
            parsed.get("quick").and_then(|v| v.as_u64()),
            Some(0),
            "status.json must count fast-path turns: {body}"
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
