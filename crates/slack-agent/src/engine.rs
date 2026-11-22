//! `engine` — the inference endpoints, which one is up, and the OpenAI-dialect
//! chat call with tools.
//!
//! ## Two rules this file exists to enforce
//!
//! **No model id is written down.** Each candidate is a base URL; the model id and
//! the context window are read from that URL's `GET /v1/models` when the engine is
//! first used and re-read after a failover. That is AGENTS.md rule 31 applied here:
//! the fleet has already been bitten by a checked-in list of model ids and `n_ctx`
//! values where *every* class of it had drifted, and `common/pi-endpoints.nix` is
//! the precedent for "declare the endpoint, discover the rest".
//! `slack-agent --probe` prints what it discovered, which is the thing to run after
//! an engine bump (`nix flake update strata` moves both engines at once).
//!
//! **No credential is held.** The default candidates are the front door's `*.int`
//! names, and `strataBearer` on those rows (`common/nginx/front-door.nix`) makes
//! nginx replace the `Authorization` header with the engine key. So this process
//! authenticates by *being on the LAN* and has nothing to leak — which is why there
//! is no `--api-key` flag, and why a request that 401s is reported as "the door did
//! not inject a key", i.e. something is wrong with the row or the route, not with a
//! secret we should go and fetch.
//!
//! ## The two quirks of these engines that a caller must not repeat
//!
//! - **`reasoning_content` is a sibling of `content`, not part of it.** Measured on
//!   services2 2026-11-16: a tool-calling reply came back with
//!   `content: null, reasoning_content: "We need answer user's question…",
//!   tool_calls:[…]`. This file drops it. Echoing it back on the next turn would
//!   inflate every re-prefill (decode is ~32 tok/s here) and some OpenAI-dialect
//!   servers reject an unexpected field on a `role: assistant` message.
//! - **`finish_reason` is the loop's control flow.** `"tool_calls"` means the
//!   message is a request to do something and must never be shown as an answer;
//!   `"stop"` (or anything else with text) is the answer. A model that emits both
//!   is handled: the text is kept as `content` and the calls are executed, and the
//!   text becomes part of the transcript rather than being sent on its own.

use common_json::{self as json, Json};
use std::time::{Duration, Instant};

/// One inference endpoint we were told about.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Origin, no trailing slash, e.g. `https://strata.int.leighhack.org/v1`.
    pub base: String,
    pub label: String,
}

// The streaming half lives in `stream.rs`, and its types are re-exported here rather than moved at
// every caller: `agent.rs` and `listen.rs` name them, and `engine::` is how this crate's callers
// have always spelled "the thing that talks to the engine".
pub use crate::stream::StreamFail;

/// What [`Engines::prefill`] reports.
#[derive(Debug)]
pub struct Prefill {
    pub engine: String,
    pub prompt_tokens: u64,
    /// What the engine says it reused.  `0` is a real answer and the reason this struct exists:
    /// it is the difference between a warm-up that saved seconds and one that cost a request.
    pub cached_tokens: u64,
    pub millis: u128,
}

/// What a candidate turned out to be serving.
#[derive(Debug, Clone)]
pub struct Engine {
    pub base: String,
    pub label: String,
    pub model: String,
    /// `meta.n_ctx`, when the engine reports one.  Read out by `probe` and never
    /// assumed: nothing here sizes a window from a constant, because a constant is
    /// how `pi-models.json` ended up claiming 131072 while the engine said 262144.
    pub context: Option<f64>,
}

#[derive(Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Default)]
pub struct Usage {
    pub prompt: u64,
    pub completion: u64,
}

/// One assistant turn: what to show, what to do, and what it cost.
#[derive(Debug, Default)]
pub struct Turn {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub finish: String,
    pub usage: Usage,
    pub millis: u128,
    pub engine: String,
}

pub struct Engines {
    candidates: Vec<Candidate>,
    current: Option<Engine>,
    client: reqwest::blocking::Client,
    thinking: Thinking,
}

/// Whether the model is allowed to think out loud before it answers.
///
/// **The model behind this bot thinks by default, on every call, and we were never asking
/// for it.** Measured on services2 2026-11-17 with the sheep riddle ("all but 9 die"):
///
/// | request | prompt tok | completion tok | wall | reasoning |
/// | --- | --- | --- | --- | --- |
/// | default | 77 | 64 (hit the cap!) | **2.86 s** | 217 chars |
/// | `chat_template_kwargs.enable_thinking=false` | 37 | **2** | **0.90 s** | none |
/// | `reasoning_effort=low` | 65 | 22 | 2.07 s | 53 chars |
///
/// Same correct answer ("9") every time. Three things follow, and the third is why this is
/// a setting rather than a one-line deletion:
///
/// 1. The kwarg has to be **nested in `chat_template_kwargs`**. A top-level
///    `enable_thinking=false` is accepted silently and ignored — the model thinks anyway.
///    `{"thinking": false}` likewise. Only the nested spelling does anything, and a flag
///    that is accepted-and-ignored is worse than one that errors.
/// 2. Reasoning tokens are billed against `max_tokens`, so a thinking model can spend the
///    whole voice budget (200) on private monologue and return `content: null`. `agent.rs`
///    already retries that, which doubles the latency of one bad turn — a slow answer and
///    an answer that needed a retry have the same shape in the trace.
/// 3. It turns out to be **earning its keep.** The A/B ran over the whole 14-question
///    fixture and thinking-off was **33 % slower** (359.6 s against 270.8 s) with *more*
///    rounds and *more* completion tokens, because a model without its scratchpad calls
///    more tools. So the default is `On`, and the table, the mechanism and the two
///    production retries are `docs/slack-agent-2026-11-16.md` §15. Reaching for `Off`
///    again because one riddle came back 3× faster is the mistake that was already made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thinking {
    /// Ask for the private monologue to be suppressed (nested kwarg — the only spelling
    /// the engine honours).
    Off,
    /// Keep thinking but ask for less of it (`reasoning_effort`).
    Low,
    /// Say nothing and take whatever the template does. Today, and the default.
    On,
}

impl Thinking {
    /// Anything unrecognised is `On`, i.e. the behaviour the fleet already has: a typo in a
    /// setting must not silently change how a live bot reasons.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "no" | "none" | "false" => Thinking::Off,
            "low" => Thinking::Low,
            _ => Thinking::On,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Thinking::Off => "off",
            Thinking::Low => "low",
            Thinking::On => "on",
        }
    }
}

/// The default fleet, in the order to try them.  Declared here so a bare
/// `slack-agent ask "…"` works from a terminal, and *overridden wholesale* by
/// `--endpoints` / the file Nix renders — the copy in Nix is the deployed truth
/// (`common/llm-endpoints.nix`).
pub const DEFAULT_CANDIDATES: &str = "https://strata.int.leighhack.org/v1|services2 (RTX 3060),\
     https://strata2.int.leighhack.org/v1|aibox (iGPU),\
     https://llama.int.leighhack.org/v1|services3 (GTX 1080 Ti)";

pub fn parse_candidates(spec: &str) -> Vec<Candidate> {
    spec.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| {
            let (base, label) = match s.split_once('|') {
                Some((b, l)) => (b.trim(), l.trim().to_string()),
                None => (s, s.to_string()),
            };
            Candidate {
                base: base.trim_end_matches('/').to_string(),
                label,
            }
        })
        .collect()
}

impl Engines {
    pub fn new(candidates: Vec<Candidate>, timeout: Duration) -> Self {
        Engines {
            candidates,
            current: None,
            thinking: Thinking::On,
            // The timeout is the client's, once, rather than a field to forget: a
            // per-request timeout that has to be remembered is a per-request timeout
            // that is eventually not passed.
            client: reqwest::blocking::Client::builder()
                .timeout(timeout)
                .connect_timeout(Duration::from_secs(10))
                .user_agent(concat!("slack-agent/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("a reqwest client with no TLS features configured"),
        }
    }

    pub fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    /// The HTTP client, so `probe` can call [`Engines::discover`] against each
    /// candidate without each caller building its own TLS config.
    pub fn http(&self) -> &reqwest::blocking::Client {
        &self.client
    }

    /// `GET {base}/models`. Returns the engine this URL is actually serving.
    pub fn discover(candidate: &Candidate, client: &reqwest::blocking::Client) -> Result<Engine, String> {
        let url = format!("{}/models", candidate.base);
        let resp = client
            .get(&url)
            .send()
            .map_err(|e| format!("{}: {e}", candidate.label))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!(
                "{}: {url} answered {status}{}",
                candidate.label,
                if status.as_u16() == 401 {
                    " — the front door did not inject a key (is there a `strataBearer` row for \
                     this name, and is this box on the LAN ACL?)"
                } else {
                    ""
                }
            ));
        }
        let body = resp.text().map_err(|e| format!("{}: reading {url}: {e}", candidate.label))?;
        let doc = json::parse(&body)
            .map_err(|e| format!("{}: {url} is not JSON ({e}): {:.160}", candidate.label, body))?;
        let models = doc
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| format!("{}: {url} has no `data` array", candidate.label))?;
        let Some(first) = models.first() else {
            return Err(format!("{}: {url} lists no models right now", candidate.label));
        };
        let id = first
            .get("id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("{}: {url}'s first model has no id", candidate.label))?;
        let context = first.at(&["meta", "n_ctx"]).and_then(|v| v.as_f64());
        Ok(Engine {
            base: candidate.base.clone(),
            label: candidate.label.clone(),
            model: id.to_string(),
            context,
        })
    }

    /// The engine to use now, discovering it if needed and walking the candidate
    /// list until one answers.
    /// Set it once, at startup, from `Config`; a per-request toggle would make two answers
    /// to one question possible and unexplainable.
    pub fn set_thinking(&mut self, t: Thinking) {
        self.thinking = t;
    }

    pub fn engine(&mut self) -> Result<Engine, String> {
        if let Some(e) = &self.current {
            return Ok(e.clone());
        }
        let mut failures = Vec::new();
        for c in self.candidates.clone() {
            match Self::discover(&c, &self.client) {
                Ok(e) => {
                    println!(
                        "engine: {} serving `{}`{}",
                        e.label,
                        e.model,
                        e.context.map(|c| format!(", n_ctx {}", c as u64)).unwrap_or_default()
                    );
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                    self.current = Some(e.clone());
                    return Ok(e);
                }
                Err(e) => failures.push(e),
            }
        }
        Err(format!(
            "no inference endpoint answered. Tried:\n  {}",
            failures.join("\n  ")
        ))
    }

    /// Call it down; the next `engine()` re-discovers.
    pub fn give_up(&mut self) {
        if let Some(e) = self.current.take() {
            eprintln!("engine: dropping {} after a failed request", e.label);
        }
    }

    /// One `POST {base}/chat/completions`.
    ///
    /// `messages` and `tools` are already-built JSON, because the transcript is
    /// JSON-shaped from the start and rebuilding it through a typed model here
    /// would only add a second representation to keep in sync with the server's.
    /// The body of a chat request, shared by [`chat`][Self::chat] and
    /// [`chat_stream`][Self::chat_stream] so the two cannot drift: one flag apart is the
    /// difference between an answer and a turn that starts arriving, and a field list written
    /// twice is a field list that gets updated once.
    pub(crate) fn chat_body(&self, engine: &Engine, messages: &[Json], tools: &[Json], max_tokens: u32, stream: bool) -> String {
        let mut body = vec![
            ("model".to_string(), json::string(engine.model.clone())),
            ("messages".to_string(), Json::Arr(messages.to_vec())),
            ("max_tokens".to_string(), json::number_from_int(max_tokens as u64)),
            ("temperature".to_string(), json::number_from_f64(0.2)),
            ("stream".to_string(), json::boolean(stream)),
        ];
        if stream {
            // These engines only report `usage` on a stream if it is asked for, and the turn's
            // receipts (`prompt_tokens`, `completion_tokens`) are what `listen` publishes.
            body.push((
                "stream_options".to_string(),
                json::object(vec![("include_usage".to_string(), json::boolean(true))]),
            ));
        }
        // The one spelling this engine honours, measured: nested in `chat_template_kwargs`.
        // Top-level `enable_thinking` is accepted and ignored, which is the kind of silence
        // that turns a config option into a rumour.
        match self.thinking {
            Thinking::Off => body.push((
                "chat_template_kwargs".to_string(),
                json::object(vec![("enable_thinking".to_string(), json::boolean(false))]),
            )),
            Thinking::Low => body.push(("reasoning_effort".to_string(), json::string("low"))),
            Thinking::On => {}
        }
        if !tools.is_empty() {
            body.push(("tools".to_string(), Json::Arr(tools.to_vec())));
            // Measured on services2: the engine calls tools correctly when asked
            // with `auto`. Sent explicitly because an OpenAI-dialect server that
            // defaults `tool_choice` to `none` makes a tool-using agent look
            // broken rather than different — which is the exact bug reported
            // against other harnesses for this class of backend.
            body.push(("tool_choice".to_string(), json::string("auto")));
        }
        json::emit(&json::object(body), 0)
    }

    pub fn chat(&mut self, messages: &[Json], tools: &[Json], max_tokens: u32) -> Result<Turn, String> {
        let engine = self.engine()?;
        let url = format!("{}/chat/completions", engine.base);
        let payload = self.chat_body(&engine, messages, tools, max_tokens, false);
        let started = Instant::now();
        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .body(payload)
            .send()
            .map_err(|e| format!("{}: POST {url}: {e}", engine.label))?;
        let status = resp.status();
        let text = resp
            .text()
            .map_err(|e| format!("{}: reading reply: {e}", engine.label))?;
        let millis = started.elapsed().as_millis();
        if !status.is_success() {
            return Err(format!(
                "{}: {url} answered {status}: {:.300}",
                engine.label,
                text.lines().next().unwrap_or("")
            ));
        }
        let doc = json::parse(&text)
            .map_err(|e| format!("{}: reply is not JSON ({e}): {:.300}", engine.label, text))?;
        if let Some(err) = doc.at(&["error", "message"]).and_then(|v| v.as_str()) {
            return Err(format!("{}: engine error: {err}", engine.label));
        }
        let choice = doc
            .at(&["choices"])
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .ok_or_else(|| format!("{}: reply had no choices[]: {:.200}", engine.label, text))?;
        let message = choice.get("message");
        let mut turn = Turn {
            finish: choice
                .get("finish_reason")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            millis,
            engine: engine.label.clone(),
            ..Default::default()
        };
        if let Some(u) = doc.get("usage") {
            turn.usage.prompt = u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            turn.usage.completion = u.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        }
        // `content` may be null (a pure tool call) or a string.  `reasoning_content`
        // is deliberately not read: see the module header.
        turn.text = message
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if let Some(calls) = message.and_then(|m| m.get("tool_calls")).and_then(|t| t.as_array()) {
            for c in calls {
                let name = c.at(&["function", "name"]).and_then(|v| v.as_str()).unwrap_or("").to_string();
                if name.is_empty() {
                    continue;
                }
                turn.tool_calls.push(ToolCall {
                    id: c.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    name,
                    // Some servers emit an object rather than a string here.
                    arguments: match c.at(&["function", "arguments"]) {
                        Some(Json::Str(s)) => s.clone(),
                        Some(v @ Json::Obj(_)) => json::emit(v, 0),
                        _ => "{}".to_string(),
                    },
                });
            }
        }
        Ok(turn)
    }

    /// Send a turn's **prompt prefix** to an engine and ask for one token, for the sole purpose
    /// of making the real request cheaper.
    ///
    /// ## What this is for
    ///
    /// These engines cache prompt prefixes across requests, and the prefix that costs time is
    /// the one every turn shares: the system prompt plus the tool schemas, which is a couple of
    /// thousand tokens the voice path would otherwise pay for *after* the person had finished
    /// speaking. So the voice gateway fires this as soon as it has a first partial transcript,
    /// and the real request then prefills only the new tail.
    ///
    /// ## `max_tokens: 1`, and not 0 — measured, not guessed
    ///
    /// `max_tokens: 0` is the honest-looking spelling of "prefill only, generate nothing", and
    /// these engines **ignore it**: measured on services2, a `max_tokens: 0` request answered
    /// `"One, two, three."` with `completion_tokens: 7`. One token is the smallest thing the
    /// server will actually stop at, and the second request in that same probe came back in
    /// **0.1 s against 138 s** with `prompt_tokens_details.cached_tokens` non-zero — which is
    /// the whole claim, measured on the wire rather than asserted from llama.cpp folklore.
    ///
    /// ## What this must never do
    ///
    /// Touch the transcript. It is not a turn, its one token is not an answer, and nothing here
    /// keeps it: the value is entirely inside the engine's cache, and the caller that wants the
    /// answer asks again with the real question.
    pub fn prefill(&mut self, messages: &[Json], tools: &[Json]) -> Result<Prefill, String> {
        let engine = self.engine()?;
        let url = format!("{}/chat/completions", engine.base);
        let payload = self.chat_body(&engine, messages, tools, 1, false);
        let started = Instant::now();
        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .body(payload)
            .send()
            .map_err(|e| format!("{}: prefill: {e}", engine.label))?;
        let status = resp.status();
        let body = resp
            .text()
            .map_err(|e| format!("{}: reading prefill reply: {e}", engine.label))?;
        if !status.is_success() {
            return Err(format!(
                "{}: prefill answered {status}: {:.200}",
                engine.label,
                body.lines().next().unwrap_or("")
            ));
        }
        let doc = json::parse(&body).unwrap_or(json::null());
        let usage = doc.get("usage");
        Ok(Prefill {
            engine: engine.label.clone(),
            prompt_tokens: usage
                .and_then(|u| u.get("prompt_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            cached_tokens: usage
                .and_then(|u| u.at(&["prompt_tokens_details", "cached_tokens"]))
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            millis: started.elapsed().as_millis(),
        })
    }
}

/// Append the assistant's turn to the transcript in the shape the next request
/// expects — including the tool calls, so the `role: tool` replies that follow
/// have something to answer.
pub fn assistant_message(turn: &Turn) -> Json {
    let mut pairs = vec![("role".to_string(), json::string("assistant"))];
    pairs.push((
        "content".to_string(),
        match &turn.text {
            Some(t) => json::string(t.clone()),
            None => json::null(),
        },
    ));
    if !turn.tool_calls.is_empty() {
        pairs.push((
            "tool_calls".to_string(),
            Json::Arr(
                turn.tool_calls
                    .iter()
                    .map(|c| {
                        json::object(vec![
                            ("id".to_string(), json::string(c.id.clone())),
                            ("type".to_string(), json::string("function")),
                            (
                                "function".to_string(),
                                json::object(vec![
                                    ("name".to_string(), json::string(c.name.clone())),
                                    ("arguments".to_string(), json::string(c.arguments.clone())),
                                ]),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ));
    }
    json::object(pairs)
}

/// The `role: tool` message carrying one call's result.  A call must never be
/// left unanswered: this file turns every failure into a text result so the
/// transcript always closes (a dangling tool call makes some backends 400 the
/// whole request, which would look like the engine being broken).
pub fn tool_result(call_id: &str, text: &str) -> Json {
    json::object(vec![
        ("role".to_string(), json::string("tool")),
        ("tool_call_id".to_string(), json::string(call_id)),
        ("content".to_string(), json::string(text)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::tests::engines_at;
    use std::io::Write;
    use std::thread;

    #[test]
    fn the_default_fleet_parses_into_three_named_candidates() {
        let c = parse_candidates(DEFAULT_CANDIDATES);
        assert_eq!(c.len(), 3, "{c:?}");
        assert_eq!(c[0].base, "https://strata.int.leighhack.org/v1");
        assert_eq!(c[0].label, "services2 (RTX 3060)");
        // A trailing slash must not become a doubled one in the URLs we dial.
        let c = parse_candidates("https://x.example/v1/,|l");
        assert_eq!(c[0].base, "https://x.example/v1");
    }

    #[test]
    fn a_pure_tool_call_serialises_the_way_the_next_round_demands() {
        let turn = Turn {
            text: None,
            tool_calls: vec![ToolCall {
                id: "call_1".to_string(),
                name: "space_status".to_string(),
                arguments: "{}".to_string(),
            }],
            finish: "tool_calls".to_string(),
            ..Default::default()
        };
        let msg = assistant_message(&turn);
        let wire = json::emit(&msg, 0);
        assert!(wire.contains("\"tool_calls\""), "{wire}");
        assert!(wire.contains("\"content\":null"), "{wire}");
        assert!(wire.contains("space_status"), "{wire}");
        // And the reply closes the call by id.
        let reply = json::emit(&tool_result("call_1", "Leigh Hackspace is CLOSED"), 0);
        assert!(reply.contains("\"tool_call_id\":\"call_1\""), "{reply}");
        assert!(reply.contains("CLOSED"), "{reply}");
    }

    /// The live gate, run only when told to: this talks to the fleet.
    #[test]
    #[ignore = "needs the inference engines up; `just slack-agent-probe` runs this for real"]
    fn a_real_engine_is_discoverable_and_calls_tools() {
        let mut e = Engines::new(parse_candidates(DEFAULT_CANDIDATES), Duration::from_secs(90));
        let engine = e.engine().expect("an engine is up");
        assert!(!engine.model.is_empty());
        let tools = vec![json::object(vec![
            ("type".to_string(), json::string("function")),
            (
                "function".to_string(),
                json::object(vec![
                    ("name".to_string(), json::string("space_status")),
                    (
                        "description".to_string(),
                        json::string("Whether the hackspace is open, and its temperatures"),
                    ),
                    (
                        "parameters".to_string(),
                        json::object(vec![("type".to_string(), json::string("object"))]),
                    ),
                ]),
            ),
        ])];
        let turn = e
            .chat(
                &[json::object(vec![
                    ("role".to_string(), json::string("user")),
                    ("content".to_string(), json::string("Is the hackspace open right now?")),
                ])],
                &tools,
                200,
            )
            .expect("a reply");
        assert_eq!(turn.finish, "tool_calls", "measured behaviour: {turn:?}");
        assert_eq!(turn.tool_calls.len(), 1);
    }

    /// A stand-in engine that answers a chat request with raw SSE bytes, so the wire framing is
    /// what is under test rather than a convenient restatement of the parser.
    #[test]
    fn the_prefill_request_asks_for_one_token_and_reports_what_was_reused() {
        // `cached_tokens` is the whole point of the warm-up, and it is read out of
        // `usage.prompt_tokens_details` — the spelling these engines use (measured on
        // services2, where a repeat of one prompt came back `cached_tokens: 9` in 0.1 s against
        // 138 s for the cold one).
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        thread::spawn(move || {
            use std::io::Read;
            // Two requests on one client: discovery, then the warm-up itself.
            for stream in l.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut all: Vec<u8> = Vec::new();
                let mut buf = [0u8; 1024];
                let mut head = None;
                let mut want = 0usize;
                while let Ok(n) = s.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    all.extend_from_slice(&buf[..n]);
                    if head.is_none() {
                        head = all.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4);
                        if let Some(h) = head {
                            want = String::from_utf8_lossy(&all[..h])
                                .lines()
                                .find(|line| line.to_ascii_lowercase().starts_with("content-length:"))
                                .and_then(|line| line.split(':').next_back())
                                .and_then(|v| v.trim().parse().ok())
                                .unwrap_or(0);
                        }
                    }
                    if let Some(h) = head {
                        if all.len() >= h + want {
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&all).into_owned();
                if text.starts_with("GET") {
                    let models = "{\"data\":[{\"id\":\"fake-model\"}]}";
                    let _ = s.write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{models}",
                            models.len()
                        )
                        .as_bytes(),
                    );
                    let _ = s.flush();
                    continue;
                }
                let _ = tx.send(text);
                let body = "{\"choices\":[{\"message\":{\"content\":\".\"},\"finish_reason\":\"length\"}],\
                              \"usage\":{\"prompt_tokens\":2010,\"completion_tokens\":1,\
                              \"prompt_tokens_details\":{\"cached_tokens\":1984}}}";
                let out = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(out.as_bytes());
                let _ = s.flush();
            }
        });
        let p = engines_at(&format!("http://127.0.0.1:{port}/v1"))
            .prefill(
                &[json::object(vec![
                    ("role".to_string(), json::string("user")),
                    ("content".to_string(), json::string("is it open")),
                ])],
                &[],
            )
            .unwrap();
        assert_eq!((p.prompt_tokens, p.cached_tokens), (2010, 1984));
        let sent = rx.recv().unwrap();
        // `max_tokens: 1`, and not 0: measured, these engines **ignore** `max_tokens: 0` and
        // answer the question (`"One, two, three."`, 7 completion tokens), so a "prefill only"
        // request written the honest-looking way is a normal, longer, generation.
        assert!(sent.contains("\"max_tokens\":1"), "must ask for one token:\n{sent}");
        assert!(sent.contains("\"stream\":false"), "a prefill is not a stream:\n{sent}");
        assert!(
            sent.contains("is it open"),
            "the question must be inside the prefix it warms:\n{sent}"
        );
    }
}