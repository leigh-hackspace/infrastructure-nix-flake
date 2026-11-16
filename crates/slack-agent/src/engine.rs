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
    pub fn chat(&mut self, messages: &[Json], tools: &[Json], max_tokens: u32) -> Result<Turn, String> {
        let engine = self.engine()?;
        let url = format!("{}/chat/completions", engine.base);
        let mut body = vec![
            ("model".to_string(), json::string(engine.model.clone())),
            ("messages".to_string(), Json::Arr(messages.to_vec())),
            ("max_tokens".to_string(), json::number_from_int(max_tokens as u64)),
            ("temperature".to_string(), json::number_from_f64(0.2)),
            ("stream".to_string(), json::boolean(false)),
        ];
        if !tools.is_empty() {
            body.push(("tools".to_string(), Json::Arr(tools.to_vec())));
            // Measured on services2: the engine calls tools correctly when asked
            // with `auto`. Sent explicitly because an OpenAI-dialect server that
            // defaults `tool_choice` to `none` makes a tool-using agent look
            // broken rather than different — which is the exact bug reported
            // against other harnesses for this class of backend.
            body.push(("tool_choice".to_string(), json::string("auto")));
        }
        let payload = json::emit(&json::object(body), 0);
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
}
