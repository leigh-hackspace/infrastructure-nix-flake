//! The HTTP surface every caller shares: `transcribe`, `ask`, `tts`.
//!
//! ## Why one socket for three jobs
//!
//! The browser page, the room satellite and `just voice-ask` all need the same
//! three things, and the interesting decisions (what whisper is asked, how an answer
//! is made speakable, which voice, how long is too long to say) must not be made
//! three times.  So they are made once, here, and the satellite is as thin a client
//! as the page is.  That is also what makes the page a *test* of the camera path: if
//! the page answers well, the pipeline answers well, and the only thing left to
//! prove is the transducer in the ceiling.
//!
//! ## Loopback, and what that means for the page
//!
//! Like `hackspace-mcp` and `slack-agent`, this binds loopback and there is no
//! authentication, because the reach is the boundary: the front door's
//! `voice.int.leighhack.org` row (`common/front-door.nix`) is the only thing that
//! reaches this socket from a phone, and `CONFIG.ROUTER_ONLY` on the origin is what
//! makes "reachable from the router, nothing else" true.  A `--listen` beyond
//! loopback is refused for the same reason `hackspace-mcp`'s is
//! (`crates/hackspace-mcp/src/main.rs`): an unauthenticated socket on a LAN address
//! is not a service with no auth *yet*, it is a service with no auth.
//!
//! ## Cross-origin, because the page is served from elsewhere
//!
//! The mic page lives on `whisper.int.leighhack.org` (that is where the
//! OpenWakeWord runtime, the mic capture and the level meter already are,
//! `crates/whisper-ws/README.md`) and so calls this origin cross-site.  Preflights
//! are answered only for the origins named by `--allow-origin`, echoed back exactly:
//! a wildcard here would let any web page any member opens in a browser on this
//! network ask the assistant questions and read the answers, which is a small
//! privacy leak and a free LLM.

use std::collections::VecDeque;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use common_json::Json;
use common_web::{self as web, Reply, Request};

use crate::{agent, b64, spoken, stt, tts, turn, wake, wav};

pub const DEFAULT_LISTEN: &str = "127.0.0.1:8193";

/// Where `voice-wake serve` listens by default.  This gateway is the only thing a browser
/// talks to, so the engine's address is loopback-only and never published to a client.
pub const DEFAULT_WAKE_URL: &str = "http://127.0.0.1:8195";

/// One line of the recent-activity ring.
#[derive(Clone)]
pub struct Entry {
    pub at: u64,
    pub route: String,
    pub detail: String,
    pub secs: f64,
    pub ok: bool,
    pub note: String,
}

#[derive(Default)]
pub struct Counters {
    pub requests: u64,
    pub transcribes: u64,
    pub asks: u64,
    pub tts: u64,
    pub errors: u64,
    pub last_error: String,
    pub recent: VecDeque<Entry>,
}

pub struct Options {
    pub listen: String,
    pub whisper_url: String,
    pub whisper_language: Option<String>,
    pub agent_url: String,
    /// The wake engine, or empty when this box has none.  `/v1/wake/*` is a **proxy**, so the
    /// browser keeps talking to exactly one origin (no second CORS pair, no second vhost, no
    /// second DNS row) and the engine's own refusal to bind anywhere but loopback stays
    /// intact.  See `wake_proxy`.
    pub wake_url: String,
    pub piper_bin: String,
    pub piper_model: String,
    pub length_scale: f32,
    pub max_speech_chars: usize,
    pub allow_origins: Vec<String>,
    /// Where `voice-assistant satellite` writes its own status, so one HTTP surface
    /// answers "is the camera listening too".  Absent = no satellite on this box.
    pub satellite_status: Option<PathBuf>,
    /// Spoken answers are written here as WAV, for `just voice-say` and for the
    /// satellite's dry-run output.  Nothing serves this directory.
    pub out_dir: PathBuf,
    /// The words the browser page's utterance may still begin with.  The satellite
    /// strips them itself (it needs to know *whether* it was addressed); this is the
    /// shared copy of the rule for every other caller, and `wake::strip` is the only
    /// implementation of it.
    pub wake_words: Vec<String>,
    /// Fire the engine warm-up (`slack-agent`'s `/v1/ask/prefill`) as soon as a turn has a
    /// question, so the real request prefills only its tail.  `--no-warm` turns it off;
    /// `/status.json` reports what the engines have said they reused, because a warm-up that
    /// never hits a cache is a cost and the only way to know is to look.
    pub warm: bool,
    /// Rolling-ASR slice length, seconds. The latency dial for a spoken question: the shorter it
    /// is, the less of it is left to transcribe when someone stops talking — and the more whisper
    /// calls the same sentence costs.
    pub asr_slice_secs: f64,
    /// Where the resident piper's per-sentence WAVs are written (and removed from). Inside the
    /// unit's state directory, because it is scratch that must not outlive a sentence.
    pub work_dir: PathBuf,
    /// Keep the question text in `/status.json`.  On by default, because
    /// `whisper-ws` already publishes its last 20 transcriptions and this service is
    /// no more sensitive than that; `--no-record-questions` is the answer for a room
    /// where someone objects, and it degrades the page, not the assistant.
    pub record_questions: bool,
}

pub(crate) struct Ctx {
    pub(crate) opts: Options,
    pub(crate) stt: stt::Client,
    pub(crate) agent: agent::Client,
    /// The voice, behind a lock: a streamed turn hands sentences to it from its own thread, and
    /// one piper process speaking two answers at once is not a thing worth debugging.
    pub(crate) tts: turn::Voice,
    pub(crate) started: Instant,
    pub(crate) counters: Mutex<Counters>,
    /// The turn counters the streamed route adds, kept off `Counters` because they are timings
    /// rather than a history and a reader wants the latest, not the last twenty.
    pub(crate) turns: Mutex<TurnStats>,
}

/// What the last streamed turn cost, and whether the warm-up earned anything.
#[derive(Default, Clone)]
pub struct TurnStats {
    pub turns: u64,
    pub last_total_ms: u128,
    pub last_first_audio_ms: u128,
    pub last_asr_tail_ms: u128,
    pub warm_ok: u64,
    pub warm_cached_tokens: u64,
    pub warm_prompt_tokens: u64,
    pub last_warm: String,
}

impl Ctx {
    /// The voice's name and rate, from whichever piper is up.
    pub(crate) fn voice(&self) -> tts::Voice {
        self.tts.lock().unwrap_or_else(|p| p.into_inner()).voice()
    }

    /// The origin policy, in one place: every route answers CORS with the same list, and a route
    /// that streams has to check it **before** it writes an event, not after.
    pub(crate) fn allows_origin(&self, origin: &str) -> bool {
        self.opts
            .allow_origins
            .iter()
            .any(|a| a.eq_ignore_ascii_case(origin))
    }

    pub(crate) fn note(&self, route: &str, detail: &str, secs: f64, ok: bool, note: &str) {
        let mut c = self.counters.lock().unwrap_or_else(|p| p.into_inner());
        c.requests += 1;
        match route {
            "/v1/transcribe" => c.transcribes += 1,
            "/v1/ask" => c.asks += 1,
            "/v1/tts" => c.tts += 1,
            _ => {}
        }
        if !ok {
            c.errors += 1;
            c.last_error = note.to_string();
        }
        if self.opts.record_questions || route.starts_with("/v1/t") {
            // With questions off, the counts and timings still get recorded — "it is
            // slow" and "it is answering the wrong things" are different faults and
            // both need the ring.
            if self.opts.record_questions {
                c.recent.push_back(Entry {
                    at: epoch(),
                    route: route.to_string(),
                    detail: detail.chars().take(140).collect(),
                    secs,
                    ok,
                    note: note.chars().take(140).collect(),
                });
            }
        }
        while c.recent.len() > 20 {
            c.recent.pop_front();
        }
    }
}

fn epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Serve until the process is stopped.  Never returns.
pub fn serve(opts: Options) -> Result<(), String> {
    if !common_service::is_loopback(&opts.listen) {
        return Err(format!(
            "refusing to bind {}: this socket has no authentication. The page reaches it \
             through the front door's `voice.int.leighhack.org` row, which is the only \
             supported path in from another machine.",
            opts.listen
        ));
    }
    let stt = stt::Client::new(&opts.whisper_url, opts.whisper_language.clone());
    let agent = agent::Client::new(&opts.agent_url);
    let tts_dir = opts.work_dir.clone();
    let synthesiser = tts::Synthesiser::new(
        &opts.piper_bin,
        &opts.piper_model,
        opts.length_scale,
        &tts_dir,
    );
    std::fs::create_dir_all(&opts.out_dir)
        .map_err(|e| format!("cannot create {}: {e}", opts.out_dir.display()))?;
    std::fs::create_dir_all(&opts.work_dir)
        .map_err(|e| format!("cannot create {}: {e}", opts.work_dir.display()))?;
    // Start the voice process now rather than on the first answer: the cost of loading it is the
    // whole reason a warm one exists, and paying it inside a person's first question is the
    // same 2.4 s, merely in a worse place.
    let warm = Mutex::new(synthesiser);
    let voice = warm.lock().unwrap_or_else(|p| p.into_inner()).voice();
    let started_up = if warm.lock().unwrap_or_else(|p| p.into_inner()).warm() {
        "warm"
    } else {
        "cold (one-shot piper: see the warnings above)"
    };
    let ctx = Arc::new(Ctx {
        opts,
        stt,
        agent,
        tts: warm,
        started: Instant::now(),
        counters: Mutex::new(Counters::default()),
        turns: Mutex::new(TurnStats::default()),
    });
    let listener = TcpListener::bind(&ctx.opts.listen).map_err(|e| format!("bind: {e}"))?;
    println!(
        "voice-assistant serving {} — whisper={} agent={} voice={} ({} Hz, {started_up}), \
         max {} chars, origins {:?}",
        ctx.opts.listen,
        ctx.opts.whisper_url,
        ctx.opts.agent_url,
        voice.name,
        voice.sample_rate,
        ctx.opts.max_speech_chars,
        ctx.opts.allow_origins,
    );
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let ctx = Arc::clone(&ctx);
                thread::spawn(move || {
                    if let Err(e) = handle(stream, &ctx) {
                        eprintln!("connection: {e}");
                    }
                });
            }
            Err(e) => eprintln!("accept: {e}"),
        }
    }
    Ok(())
}

fn handle(mut stream: TcpStream, ctx: &Arc<Ctx>) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(180)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let Some(req) = web::read_request(&mut stream) else {
        return Ok(()); // hung up, or oversized: nothing to answer
    };
    // The routes that answer **as they work** take the socket, so this router never has to model a
    // response with no body yet. They live in `turn_routes.rs`, beside the SSE plumbing they exist
    // to serve, and they apply the same origin rule as below — before a single event is written,
    // since a preflight never reaches them (`route` answers OPTIONS).
    if req.method == "POST" && req.path() == "/v1/turn" {
        return crate::turn_routes::turn_route(req, stream, ctx);
    }
    if req.method == "POST" && req.path() == "/v1/warm" {
        return crate::turn_routes::warm_route(&req, &mut stream, ctx);
    }
    let reply = route(&req, ctx);
    let reply = with_cors(reply, &req, ctx);
    reply.send(&mut stream)
}

/// The CORS answer, which is also the origin policy.
fn with_cors(reply: Reply, req: &Request, ctx: &Arc<Ctx>) -> Reply {
    let origin = req.header("origin");
    if origin.is_empty() {
        return reply;
    }
    let allowed = ctx.allows_origin(origin);
    if !allowed {
        // No headers at all: the browser blocks it, and the reason shows up in the
        // page's console rather than us having to invent an error body.
        return reply;
    }
    let mut reply = reply
        .header("Access-Control-Allow-Origin", origin)
        .header("Vary", "Origin");
    if req.header("access-control-request-private-network") == "true" {
        // Chrome asks this when a page dials a private-network address.  Everything
        // that can reach this socket is on the private network already — the check is
        // `is_loopback` on the bind plus the front door — so the answer is always yes
        // *for an origin we allowed*, and refusing it would only break the page.
        reply = reply.header("Access-Control-Allow-Private-Network", "true");
    }
    reply
}

fn route(req: &Request, ctx: &Arc<Ctx>) -> Reply {
    match (req.method.as_str(), req.path()) {
        ("OPTIONS", _) => {
            let methods = req.header("access-control-request-method");
            let mut r = Reply::new(204, "text/plain; charset=utf-8", Vec::new());
            r.headers.push((
                "Access-Control-Allow-Methods".to_string(),
                if methods.is_empty() {
                    "POST, GET, OPTIONS".to_string()
                } else {
                    methods.to_string()
                },
            ));
            r.headers.push((
                "Access-Control-Allow-Headers".to_string(),
                req.header("access-control-request-headers").to_string(),
            ));
            r.headers
                .push(("Access-Control-Max-Age".to_string(), "600".to_string()));
            r
        }
        ("GET", "/") => Reply::html(200, PAGE),
        ("GET", "/health") => {
            let c = ctx.counters.lock().unwrap_or_else(|p| p.into_inner());
            Reply::json(
                200,
                &format!(
                    "{{\"status\":\"ok\",\"uptime_secs\":{:.0},\"asks\":{},\"errors\":{},\
                     \"last_error\":{}}}",
                    ctx.started.elapsed().as_secs_f64(),
                    c.asks,
                    c.errors,
                    common_json::quoted(&c.last_error)
                ),
            )
        }
        ("GET", "/status.json") => status_json(ctx),
        ("POST", "/v1/transcribe") => transcribe(req, ctx),
        ("POST", "/v1/tts") => tts_route(req, ctx),
        ("POST", "/v1/ask") => ask(req, ctx),
        ("GET", "/v1/voice") => {
            let v = ctx.voice();
            Reply::json(
                200,
                &format!(
                    "{{\"voice\":{},\"sample_rate\":{},\"speech\":\"audio/wav; 16000 Hz mono \
                     PCM16\",\"length_scale\":{}}}",
                    common_json::quoted(&v.name),
                    v.sample_rate,
                    ctx.opts.length_scale
                ),
            )
        }
        ("POST", "/v1/wake/session") => wake_proxy(req, ctx, "POST", "/v1/session"),
        ("POST", "/v1/wake/chunk") => wake_proxy(req, ctx, "POST", "/v1/chunk"),
        ("POST", "/v1/wake/reset") => wake_proxy(req, ctx, "POST", "/v1/reset"),
        ("DELETE", "/v1/wake/session") => wake_proxy(req, ctx, "DELETE", "/v1/session"),
        // The engine's own counters, through the same door: it is the page's way of asking
        // "is there an engine at all, and what is it costing", and a 503 here says no.
        ("GET", "/v1/wake/status.json") => wake_proxy(req, ctx, "GET", "/status.json"),
        ("POST", path) if path.starts_with("/v1/") => web::Reply::text(404, "no such v1 route"),
        _ => Reply::text(
            404,
            "unknown path; the routes are /, /health, /status.json, /v1/transcribe, /v1/ask, \
             /v1/tts, /v1/voice",
        ),
    }
}

/// `POST /v1/transcribe` — a WAV body, a transcript.
fn transcribe(req: &Request, ctx: &Arc<Ctx>) -> Reply {
    if req.body.len() < 44 {
        return json_error(400, "body is shorter than a WAV header");
    }
    let parsed = match wav::parse(&req.body) {
        Ok(w) => w,
        Err(e) => return json_error(400, &format!("cannot read the audio: {e}")),
    };
    let secs = parsed.seconds();
    if secs < 0.2 {
        return json_error(
            400,
            "under 0.2 s of audio: nothing in that long is a request",
        );
    }
    // Send it on at its own rate: whisper.cpp resamples internally, and re-doing it
    // here would be one more place for a rate to be wrong.
    let mono = wav::mono16(&parsed.mono(), parsed.sample_rate);
    match ctx.stt.transcribe(&mono) {
        Ok(t) => {
            let described = t.is_description();
            ctx.note(
                "/v1/transcribe",
                &t.text,
                t.wall.as_secs_f64(),
                true,
                if described { "described the noise" } else { "" },
            );
            Reply::json(
                200,
                &format!(
                    "{{\"text\":{},\"audio_secs\":{:.3},\"wall_ms\":{:.0},\"speech\":{},\
                     \"language\":{},\"whisper_duration\":{}}}",
                    common_json::quoted(&t.text),
                    secs,
                    t.wall.as_secs_f64() * 1000.0,
                    !described,
                    t.language
                        .as_deref()
                        .map(common_json::quoted)
                        .unwrap_or_else(|| "null".to_string()),
                    t.duration_secs
                        .map(|d| format!("{d:.3}"))
                        .unwrap_or_else(|| "null".to_string()),
                ),
            )
        }
        Err(e) => {
            ctx.note("/v1/transcribe", "", 0.0, false, &e);
            json_error(502, &e)
        }
    }
}

/// `POST /v1/tts` — `{"text": "…"}` → `audio/wav`, resampled to 16 kHz mono.
fn tts_route(req: &Request, ctx: &Arc<Ctx>) -> Reply {
    let text = match field(req, "text") {
        Some(t) => t,
        None => return json_error(400, "expected {\"text\": \"…\"}"),
    };
    let speak = if is_true(req, "raw") {
        text.clone()
    } else {
        match spoken::speakable(&text, ctx.opts.max_speech_chars) {
            Some(s) => s,
            None => {
                return json_error(
                    400,
                    "nothing to speak in that text once markdown and links were removed",
                )
            }
        }
    };
    let spoken_now = ctx.tts.lock().unwrap_or_else(|p| p.into_inner()).say(&speak);
    match spoken_now {
        Ok(speech) => {
            let pcm = tts::pcm_at_16k(&speech);
            let body = wav::mono16(&pcm, wav::SAMPLE_RATE);
            ctx.note("/v1/tts", &speak, speech.wall.as_secs_f64(), true, "");
            save(ctx, "tts", &body);
            Reply::new(200, "audio/wav", body).header(
                "X-Voice-Secs",
                &format!("{:.2}", speech.secs),
            )
        }
        Err(e) => {
            ctx.note("/v1/tts", "", 0.0, false, &e);
            json_error(500, &e)
        }
    }
}

/// `POST /v1/ask` — `{"text": "…", "voice": true}` → the answer, its receipts, and
/// the spoken version as base64 WAV (or `null`, with the reason in `warnings`).
fn ask(req: &Request, ctx: &Arc<Ctx>) -> Reply {
    let question = match field(req, "text") {
        Some(t) => t,
        None => return json_error(400, "expected {\"text\": \"what was asked\"}"),
    };
    let want_voice = !matches!(field(req, "voice").as_deref(), Some("false") | Some("0"));
    let question = wake::strip(&question, &ctx.opts.wake_words);
    if question.is_empty() {
        return json_error(400, "that was only the wake word — ask me something");
    }
    let asked_at = Instant::now();
    let answer = match ctx.agent.ask(&question, want_voice) {
        Ok(a) => a,
        Err(e) => {
            ctx.note("/v1/ask", &question, asked_at.elapsed().as_secs_f64(), false, &e);
            return json_error(
                if e.contains("busy") { 429 } else { 502 },
                &e,
            );
        }
    };
    let mut warnings: Vec<String> = answer.warnings.clone();
    let speak = spoken::speakable(&answer.text, ctx.opts.max_speech_chars);
    let mut audio = String::new();
    let mut audio_secs = 0.0f64;
    match speak.as_deref() {
        Some(text) => match ctx.tts.lock().unwrap_or_else(|p| p.into_inner()).say(text) {
            Ok(speech) => {
                let pcm = tts::pcm_at_16k(&speech);
                let body = wav::mono16(&pcm, wav::SAMPLE_RATE);
                audio_secs = speech.secs;
                audio = b64::encode(&body);
                save(ctx, "answer", &body);
            }
            Err(e) => {
                warnings.push(format!("no audio: {e}"));
                ctx.note("/v1/ask", &question, 0.0, false, &e);
            }
        },
        None => warnings.push("no audio: the answer had nothing speakable in it".to_string()),
    }
    let tools: Vec<String> = answer.tools.iter().map(|t| common_json::quoted(t)).collect();
    let warns: Vec<String> = warnings.iter().map(|w| common_json::quoted(w)).collect();
    let total = asked_at.elapsed().as_secs_f64();
    ctx.note(
        "/v1/ask",
        &format!("{} → {}", question, answer.text),
        total,
        true,
        &answer.engine,
    );
    Reply::json(
        200,
        &format!(
            "{{\"answer\":{},\"engine\":{},\"rounds\":{},\"tools\":[{}],\"agent_secs\":{:.2},\
             \"secs\":{:.2},\"speech\":{},\"audio\":{},\"audio_secs\":{:.2},\"warnings\":[{}]}}",
            common_json::quoted(&answer.text),
            common_json::quoted(&answer.engine),
            answer.rounds,
            tools.join(","),
            answer.secs,
            total,
            speak
                .as_deref()
                .map(common_json::quoted)
                .unwrap_or_else(|| "null".to_string()),
            if audio.is_empty() {
                "null".to_string()
            } else {
                common_json::quoted(&audio)
            },
            audio_secs,
            warns.join(","),
        ),
    )
}

fn status_json(ctx: &Arc<Ctx>) -> Reply {
    let voice = ctx.voice();
    let c = ctx.counters.lock().unwrap_or_else(|p| p.into_inner());
    let recent: Vec<String> = c
        .recent
        .iter()
        .rev()
        .map(|e| {
            format!(
                "{{\"at\":{},\"route\":{},\"detail\":{},\"secs\":{:.2},\"ok\":{},\"note\":{}}}",
                e.at,
                common_json::quoted(&e.route),
                common_json::quoted(&e.detail),
                e.secs,
                e.ok,
                common_json::quoted(&e.note)
            )
        })
        .collect();
    let satellite = ctx
        .opts
        .satellite_status
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| common_json::parse(&text).ok())
        .map(|v| common_json::emit(&v, 0))
        .unwrap_or_else(|| "null".to_string());
    let allowed: Vec<String> = ctx
        .opts
        .allow_origins
        .iter()
        .map(|o| common_json::quoted(o))
        .collect();
    let t = ctx.turns.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let (voice_warm, v) = {
        let g = ctx.tts.lock().unwrap_or_else(|p| p.into_inner());
        // `warm` here is the *live* question — is the voice process up right now — and the
        // counters beside it are the history. Both are needed: the first answers "will the next
        // answer be quick", the second answers "has this been working".
        (g.warm_live(), g.stats.clone())
    };
    Reply::json(
        200,
        &format!(
            "{{\"uptime_secs\":{:.1},\"listen\":{},\"whisper\":{},\"agent\":{},\
             \"voice\":{{\"name\":{},\"sample_rate\":{}}},\"speak_rate_hz\":{},\
             \"max_speech_chars\":{},\"allow_origins\":[{}],\"record_questions\":{},\
             \"requests\":{},\"transcribes\":{},\"asks\":{},\"tts\":{},\"errors\":{},\
             \"last_error\":{},\"recent\":[{}],\"turns\":{{\"total\":{},\"last_total_ms\":{},\
             \"last_first_audio_ms\":{},\"last_asr_tail_ms\":{},\"warm_ok\":{},\
             \"warm_cached_tokens\":{},\"warm_prompt_tokens\":{},\"last_warm\":{}}},\
             \"voice\":{{\"warm\":{},\"warm_sentences\":{},\"cold_sentences\":{},\"restarts\":{},\
             \"last_ms\":{},\"last_fallback\":{}}},\"satellite\":{}}}",
            ctx.started.elapsed().as_secs_f64(),
            common_json::quoted(&ctx.opts.listen),
            common_json::quoted(&ctx.opts.whisper_url),
            common_json::quoted(&ctx.opts.agent_url),
            common_json::quoted(&voice.name),
            voice.sample_rate,
            wav::SAMPLE_RATE,
            ctx.opts.max_speech_chars,
            allowed.join(","),
            ctx.opts.record_questions,
            c.requests,
            c.transcribes,
            c.asks,
            c.tts,
            c.errors,
            common_json::quoted(&c.last_error),
            recent.join(","),
            t.turns,
            t.last_total_ms,
            t.last_first_audio_ms,
            t.last_asr_tail_ms,
            t.warm_ok,
            t.warm_cached_tokens,
            t.warm_prompt_tokens,
            common_json::quoted(&t.last_warm),
            voice_warm,
            v.warm,
            v.cold,
            v.restarts,
            v.last_ms,
            common_json::quoted(&v.last_fallback),
            satellite,
        ),
    )
}

/// The field of a small JSON body, as text.
fn field(req: &Request, name: &str) -> Option<String> {
    let value = common_json::parse(&req.body_text()).ok()?;
    value.get(name).and_then(Json::as_str).map(|s| s.to_string())
}

fn is_true(req: &Request, name: &str) -> bool {
    common_json::parse(&req.body_text())
        .ok()
        .and_then(|v| v.get(name).and_then(Json::as_bool))
        .unwrap_or(false)
}

fn json_error(status: u16, msg: &str) -> Reply {
    Reply::json(status, &common_json::error_object(msg))
}

/// Keep a copy of what was said out loud, so "what did it tell the room" has an
/// answer that is not a guess.  Written into the unit's state directory, which
/// `StateDirectoryKey=private` keeps out of everyone else's reach.
fn save(ctx: &Arc<Ctx>, kind: &str, wav_body: &[u8]) {
    let path = ctx.opts.out_dir.join(format!(
        "{}-{}.wav",
        kind,
        epoch()
    ));
    if let Ok(mut f) = std::fs::File::create(&path) {
        let _ = f.write_all(wav_body);
        let _ = f.flush();
    }
    // Best effort on purpose: an unwritable out_dir must not fail the answer, which
    // is what a caller is waiting for.  Keep the directory small either way.
    wav::prune_wavs(&ctx.opts.out_dir, 40);
}

/// The human-facing page: is it up, how fast, and what did it last answer.
const PAGE: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>voice gateway</title>
<meta name="viewport" content="width=device-width,initial-scale=1">
<style>
 body{font:15px/1.5 system-ui,sans-serif;margin:2rem auto;max-width:52rem;padding:0 1rem;color:#15181d}
 h1{font-size:1.3rem} code{background:#f1f2f4;padding:.1em .3em;border-radius:3px}
 table{border-collapse:collapse;width:100%;font-size:.9rem} td,th{border-bottom:1px solid #dfe1e5;
 padding:.3rem .4rem;text-align:left;vertical-align:top}
 .ok{color:#0a7d34} .bad{color:#b00020} .muted{color:#5b6472}
</style></head>
<body>
<h1>Vulcan's voice gateway</h1>
<p class="muted">The speaking half of the hackspace assistant: whisper.cpp in, an answer out,
a piper voice on the way. The microphone lives on
<a href="https://whisper.int.leighhack.org/">whisper.int.leighhack.org</a>.</p>
<p>Endpoints: <code>POST /v1/transcribe</code> (a WAV body), <code>POST /v1/ask</code>
(<code>{"text":"is it cold"}</code>), <code>POST /v1/tts</code> (<code>{"text":"…"}</code> →
WAV), <code>GET /v1/voice</code>, <code>GET /status.json</code>, <code>GET /health</code>.</p>
<div id="s">loading…</div>
<script>
async function tick(){
  try{
    const r = await fetch('/status.json'); const d = await r.json();
    const w = d.satellite ? (d.satellite.wake_words ? d.satellite : null) : null;
    document.getElementById('s').innerHTML =
      '<table><tr><th>uptime</th><td>'+(d.uptime_secs/60).toFixed(1)+' min</td></tr>'+
      '<tr><th>voice</th><td>'+d.voice.name+' at '+d.voice.sample_rate+' Hz → '+
        d.speak_rate_hz+' Hz</td></tr>'+
      '<tr><th>whisper</th><td>'+d.whisper+'</td></tr>'+
      '<tr><th>agent</th><td>'+d.agent+'</td></tr>'+
      '<tr><th>requests</th><td>'+d.asks+' asked, '+d.transcribes+' transcribed, '+
        '<span class="'+(d.errors?'bad':'ok')+'">'+d.errors+' errors</span></td></tr>'+
      '<tr><th>satellite</th><td>'+(w? ('listening on '+w.source+', '+w.wake_words.length+
        ' wake words, speaker '+(w.speaker==='dry-run'?'<span class="bad">dry run (not wired)</span>':w.speaker))
        :'<span class="muted">not running here</span>')+'</td></tr></table>'+
      '<h2>last answers</h2><table><tr><th>when</th><th>route</th><th>detail</th><th>secs</th></tr>'+
      d.recent.map(e=>'<tr><td>'+new Date(e.at*1000).toLocaleTimeString()+'</td><td>'+e.route+
        '</td><td class="'+(e.ok?'':'bad')+'">'+(e.detail||e.note)+'</td><td>'+
        e.secs.toFixed(2)+'</td></tr>').join('')+'</table>';
    setTimeout(tick, 5000);
  }catch(e){ document.getElementById('s').textContent='status.json failed: '+e; setTimeout(tick,5000); }
}
tick();
</script></body></html>
"#;

/// Forward `/v1/wake/*` to `voice-wake serve`.
///
/// Three reasons this is a proxy rather than a second service the page talks to directly:
/// the browser keeps exactly **one** origin (so no second CORS allowlist, no second vhost row,
/// no second DNS record — all three of which are guard-enforced here), the engine's own
/// refusal to bind anywhere but loopback stays true, and the front door's `voice.int` row and
/// its timeouts/body cap continue to be the only policy between a tab and a model.
///
/// Only JSON moves this way. `common_http::Response` carries its body as a `String`, which is
/// correct for score and status replies and *wrong* for audio — so do not extend this route to
/// stream a WAV back; spoken answers already have their own path (`/v1/tts`, `/v1/ask`).
fn wake_proxy(req: &Request, ctx: &Arc<Ctx>, method: &str, upstream: &str) -> Reply {
    if ctx.opts.wake_url.is_empty() {
        return Reply::json(
            503,
            r#"{"error":"no wake engine on this box: --wake-url is unset"}"#,
        );
    }
    let url = format!(
        "{}{}",
        ctx.opts.wake_url.trim_end_matches('/'),
        upstream
    );
    // The session token is opaque: minted by the engine on POST /v1/session and echoed
    // straight through, so the gateway holds no wake state of its own and a restart of the
    // engine simply gives the page a 410 on the next chunk, which the page reports.
    let sid = req.header("x-voice-wake-session").to_string();
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if !sid.is_empty() {
        headers.push(("x-voice-wake-session", sid.as_str()));
    }
    let ctype = {
        let c = req.header("content-type");
        if c.is_empty() {
            if req.body.is_empty() {
                "application/json"
            } else {
                "application/octet-stream"
            }
        } else {
            c
        }
    };
    let body = if req.body.is_empty() {
        None
    } else {
        Some(req.body.as_slice())
    };
    // 25 s is a wide margin over the measured cost (a 0.5 s chunk infers in ~15 ms, and the
    // engine serialises scoring across sessions), and short enough that a wedged model shows
    // up as a 502 the page can name rather than a spinner.
    match common_http::request_bytes(
        method,
        &url,
        body,
        ctype,
        &headers,
        Duration::from_secs(25),
    ) {
        Ok(r) => Reply::new(r.status, "application/json", r.body.into_bytes()),
        Err(e) => Reply::json(
            502,
            &format!(
                "{{\"error\":\"wake engine unreachable: {}\"}}",
                e.replace('"', "'")
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_loopback_bind_is_refused_before_anything_listens() {
        let opts = Options {
            listen: "10.3.1.20:8193".into(),
            whisper_url: String::new(),
            whisper_language: None,
            agent_url: String::new(),
            wake_url: String::new(),
            piper_bin: String::new(),
            piper_model: String::new(),
            length_scale: 1.0,
            max_speech_chars: 320,
            allow_origins: vec![],
            satellite_status: None,
            out_dir: "/tmp/voice-refused-test".into(),
            work_dir: "/tmp/voice-refused-test/tts".into(),
            warm: true,
            asr_slice_secs: 1.2,
            wake_words: vec!["hey jarvis".to_string()],
            record_questions: true,
        };
        let e = serve(opts).unwrap_err();
        assert!(e.contains("no authentication"), "{e}");
        assert!(serve_like_loopback_is_rejected_on_a_hostname());
    }

    /// The rule-17 lesson, applied to this socket: a *name* that looks local is not
    /// a loopback address, and `is_loopback` must be the thing that decides.
    fn serve_like_loopback_is_rejected_on_a_hostname() -> bool {
        !common_service::is_loopback("127.example.com:8193")
            && common_service::is_loopback("127.0.0.1:8193")
            && !common_service::is_loopback("0.0.0.0:8193")
    }
}
