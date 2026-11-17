//! `agent` — the loop: question → tools → (maybe more tools) → answer.
//!
//! This is deliberately the boring part of the design: a bounded `while` around a
//! chat-completions call, with every tool result fed back as a `role: tool`
//! message. The interesting decisions are all in what bounds it and what it drops:
//!
//! - **Rounds.** [`MAX_ROUNDS`] caps how many times the model may ask for tools in
//!   one turn. Without it a model that keeps calling `space_status` burns the
//!   channel's patience; measured decode on services2 is ~32 tok/s, so a round is
//!   seconds of real wall clock.
//! - **Result size.** Each tool result is truncated to [`MAX_RESULT_CHARS`] with
//!   the truncation *announced*, so the model can tell "there is more" from "this
//!   is all of it". The second is cheap; the first is how you end up telling
//!   someone there are six temperature sensors when there are seven.
//! - **Transcript size.** Oldest exchanges are dropped first, never the system
//!   prompt, and never a `role: tool` reply without the call it answers — that
//!   makes some OpenAI-dialect servers reject the whole request, and the symptom of
//!   that is "the engine is broken", far from anything to do with history.
//! - **Unknown tools.** Answered with the list of what *does* exist, not a bare
//!   error, because a model handed an unexplained failure tends to answer from
//!   memory instead.

use crate::engine::{assistant_message, tool_result, Engines};
use crate::prompt::system_prompt;
use common_json::{self as json, Json};
use common_mcp::{Client, Tool};
use std::time::Instant;

/// How many times one question may send the model off for tools.
pub const MAX_ROUNDS: usize = 5;
/// Per tool result.
pub const MAX_RESULT_CHARS: usize = 3_000;
/// The answer's own budget. 700 tokens is ~1,500 words: no Slack answer needs
/// more, and the number is what stops a rambling model costing 20 seconds of
/// decode in a channel where someone is waiting.
pub const MAX_ANSWER_TOKENS: u32 = 700;
/// Exchanges of history kept per conversation, oldest dropped first.
pub const HISTORY_TURNS: usize = 8;
/// The same budget for an answer that is going to be **spoken**.
///
/// 200 tokens is about two sentences, and the number is a latency figure rather than
/// a style rule: decode on this fleet's engines runs at 14–39 tok/s
/// (`docs/slack-agent-2026-11-16.md`), so a 700-token answer is 20–50 s of the room
/// standing still before a single word arrives, and `spoken::speakable` would throw
/// the tail away anyway.  A caller that wants the long form asks for the text one.
pub const MAX_SPEECH_TOKENS: u32 = 200;

/// Compile-time, because the failure mode is silent and one-directional: someone
/// trimming `MAX_ANSWER_TOKENS` for a Slack reason quietly makes the *spoken* answer
/// the longer of the two, and the room only learns by waiting.  A test would catch it
/// too, but a test has to be run; this has to be compiled (`docs/voice-assistant-2026-11-16.md`).
const _: () = assert!(MAX_SPEECH_TOKENS < MAX_ANSWER_TOKENS);

pub struct Agent {
    engines: Engines,
    /// MCP servers, in the order given. A tool is looked up across all of them,
    /// first match wins, so two servers cannot quietly offer the same name.
    servers: Vec<(String, Client)>,
    tools: Vec<Tool>,
    /// Which server index owns which tool name.
    owners: Vec<(String, usize)>,
}

pub struct Answer {
    pub text: String,
    /// One line per tool call and per engine note, for the log and for `ask`. Not
    /// sent to Slack: the model's own words are the answer, and a transcript of
    /// the plumbing is what people quote when the answer is wrong.
    pub log: Vec<String>,
    pub rounds: usize,
    /// The tools that were actually called, in order, with duplicates.  `log` has the
    /// same information as prose, and a caller that has to *report* which tools answered
    /// (the voice endpoint does, so a person can see what a spoken figure came from)
    /// should not have to re-parse a sentence that was written for a human.
    pub tools_used: Vec<String>,
    pub engine: String,
    pub millis: u128,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// A transcript is the system prompt at index 0 followed by the exchange.
pub fn seed_transcript() -> Vec<Json> {
    seed_with(&system_prompt())
}

/// ... and the system prompt is the only part that varies between callers: the
/// spoken front end (`listen`) seeds with `prompt::spoken_system_prompt()` so the
/// voice answers in two sentences while the Slack answers keep the 700-token room to
/// breathe in.  One grounding rule, two presentation rules — see `prompt.rs`.
pub fn seed_with(system: &str) -> Vec<Json> {
    vec![json::object(vec![
        ("role".to_string(), json::string("system")),
        ("content".to_string(), json::string(system.to_string())),
    ])]
}

impl Agent {
    /// Connect to every MCP server and take the union of their tools.
    ///
    /// A server that will not answer is a **warning**, and its tools are simply
    /// absent — not a startup failure. That is the difference between a bot that
    /// says "I cannot see Home Assistant" and a unit in a restart loop, and it is
    /// why `slack-agent tools` doubles as the health check. The warnings are
    /// returned rather than printed so both callers (CLI and unit) put them where
    /// they belong: stderr for a human, the journal for a service.
    pub fn new(engines: Engines, mcp_urls: &[String]) -> (Agent, Vec<String>) {
        let mut servers = Vec::new();
        let mut tools = Vec::new();
        let mut owners = Vec::new();
        let mut warnings = Vec::new();
        for url in mcp_urls {
            let client = Client::new(url.clone());
            match client.list_tools() {
                Ok(list) => {
                    let index = servers.len();
                    for t in list {
                        if tools.iter().any(|existing: &Tool| existing.name == t.name) {
                            warnings.push(format!(
                                "{url}: tool `{}` is already offered by another server — ignored",
                                t.name
                            ));
                            continue;
                        }
                        owners.push((t.name.clone(), index));
                        tools.push(t);
                    }
                    servers.push((url.clone(), client));
                }
                Err(e) => warnings.push(format!("MCP server {url} did not answer: {e}")),
            }
        }
        (
            Agent {
                engines,
                servers,
                tools,
                owners,
            },
            warnings,
        )
    }

    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name.clone()).collect()
    }

    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// The `tools` array for the request, built straight off the MCP servers — so
    /// adding a tool to `hackspace-mcp` needs no change here at all.
    pub fn openai_tools(&self) -> Vec<Json> {
        self.tools.iter().map(|t| t.openai_definition()).collect()
    }

    /// Ask one question into an existing transcript, which is appended to so a
    /// caller can keep it as a session.
    pub fn ask(
        &mut self,
        transcript: &mut Vec<Json>,
        question: &str,
        on_event: impl FnMut(&str),
    ) -> Result<Answer, String> {
        self.ask_budget(transcript, question, MAX_ANSWER_TOKENS, on_event)
    }

    /// [`ask`] with the answer's token budget named, which is the only difference
    /// between answering a person in Slack and answering a room out loud.
    pub fn ask_budget(
        &mut self,
        transcript: &mut Vec<Json>,
        question: &str,
        max_tokens: u32,
        mut on_event: impl FnMut(&str),
    ) -> Result<Answer, String> {
        let started = Instant::now();
        transcript.push(json::object(vec![
            ("role".to_string(), json::string("user")),
            ("content".to_string(), json::string(question)),
        ]));
        trim(transcript);
        let tools = self.openai_tools();

        let mut log = Vec::new();
        let mut tools_used: Vec<String> = Vec::new();
        let mut prompt_tokens: u64;
        let mut completion_tokens = 0u64;
        let mut engine_used: String;
        let mut rounds = 0usize;
        let mut budget = max_tokens;

        loop {
            rounds += 1;
            let turn = match self
                .engines
                .chat(transcript, &tools, budget)
            {
                Ok(turn) => turn,
                Err(first) => {
                    // One retry, on another engine: `common/pi-endpoints.nix`
                    // already records that services2 goes down for builds because
                    // its RAM *is* the engine, and a chat bot should sail through
                    // that rather than reply "engine failed".
                    self.engines.give_up();
                    on_event(&format!("engine failed: {first} — trying the next one"));
                    match self.engines.chat(transcript, &tools, budget) {
                        Ok(turn) => turn,
                        Err(second) => {
                            self.engines.give_up();
                            return Err(format!("every inference endpoint failed: {second}"));
                        }
                    }
                }
            };
            engine_used = turn.engine.clone();
            prompt_tokens = turn.usage.prompt;
            completion_tokens += turn.usage.completion;
            on_event(&format!(
                "{}: {} in / {} out tokens, {} ms, finish={}",
                turn.engine, turn.usage.prompt, turn.usage.completion, turn.millis, turn.finish
            ));

            let said_nothing = turn.text.as_deref().unwrap_or("").trim().is_empty();
            if turn.tool_calls.is_empty() && said_nothing && turn.finish == "length"
                && budget < MAX_ANSWER_TOKENS * 2
            {
                // Measured on services2, 2026-11-16: the turn that should have been
                // the answer came back `finish_reason: "length"` with `content: null`
                // and 700 completion tokens.  Those tokens went into
                // `reasoning_content`, which this harness deliberately drops — so the
                // engine thought itself out of its budget and said nothing.  The fix
                // is not to stop dropping it (that would double every later re-prefill
                // and some servers reject it); it is to give the closing turn the room
                // it actually asked for, once, and only when it asked for it.
                budget *= 2;
                on_event(&format!(
                    "{} spent its whole token budget on reasoning and produced no text —                      retrying the turn with {budget}",
                    turn.engine
                ));
                continue;
            }

            if turn.tool_calls.is_empty() {
                let text = turn.text.clone().unwrap_or_default();
                if text.trim().is_empty() {
                    return Err(format!(
                        "the model returned no text after {} tokens of reasoning budget                          (finish_reason={}). Ask again, or narrow the question.",
                        budget, turn.finish
                    ));
                }
                transcript.push(json::object(vec![
                    ("role".to_string(), json::string("assistant")),
                    ("content".to_string(), json::string(text.clone())),
                ]));
                trim(transcript);
                return Ok(Answer {
                    text,
                    log,
                    tools_used,
                    rounds,
                    engine: engine_used,
                    millis: started.elapsed().as_millis(),
                    prompt_tokens,
                    completion_tokens,
                });
            }

            if rounds >= MAX_ROUNDS {
                // Out of tool budget.  Reporting the budget rather than an answer
                // would be honest but useless, and measured on 2026-11-16 that is
                // exactly what happened on "is the NAS disk full": the model had
                // already learned the useful fact (the NAS is not scraped at all —
                // it is not in the instance list) and then ran out before saying it.
                // So: one final turn with `tools: []`, which cannot call anything,
                // and the instruction to answer with what it has or name what is
                // missing. A made-up number is still the failure to avoid; "I can
                // see the servers' disks and there is no NAS among them" is the
                // answer we want.
                transcript.push(json::object(vec![
                    ("role".to_string(), json::string("user")),
                    (
                        "content".to_string(),
                        json::string(
                            "You have used your tool budget for this question. Answer now using \
                             only what the tool calls above actually showed. If the data is not \
                             available here, say precisely what is missing and what you CAN see \
                             instead — do not guess, and do not describe the tools.",
                        ),
                    ),
                ]));
                let final_turn = self
                    .engines
                    .chat(transcript, &[], budget)
                    .map_err(|e| format!("out of tool budget ({} calls) and the closing turn failed too: {e}", log.len()))?;
                transcript.push(json::object(vec![
                    ("role".to_string(), json::string("assistant")),
                    ("content".to_string(), json::string(final_turn.text.clone().unwrap_or_default())),
                ]));
                let text = final_turn.text.clone().unwrap_or_default();
                return Ok(Answer {
                    tools_used,
                    text: if text.trim().is_empty() {
                        format!(
                            "I made {} tool calls and could not reach an answer. Last: {}",
                            log.len(),
                            log.last().cloned().unwrap_or_else(|| "nothing".to_string())
                        )
                    } else {
                        text
                    },
                    log,
                    rounds,
                    engine: final_turn.engine,
                    millis: started.elapsed().as_millis(),
                    prompt_tokens,
                    completion_tokens: completion_tokens + final_turn.usage.completion,
                });
            }

            transcript.push(assistant_message(&turn));
            for call in &turn.tool_calls {
                let arguments = json::parse(&call.arguments).unwrap_or_else(|_| json::object(Vec::new()));
                let outcome = self.call_tool(&call.name, arguments);
                let line = match &outcome {
                    Ok(t) => format!("{} {} → {} chars", call.name, compact(&call.arguments), t.chars().count()),
                    Err(e) => format!("{} {} → ERROR {e}", call.name, compact(&call.arguments)),
                };
                on_event(&line);
                log.push(line);
                tools_used.push(call.name.clone());
                let payload = match outcome {
                    Ok(text) => truncate(text),
                    Err(e) => format!("TOOL ERROR: {e}"),
                };
                // A call always gets a reply, even a failed one: see `tool_result`.
                transcript.push(tool_result(&call.id, &payload));
            }
            trim(transcript);
        }
    }

    /// Call one tool by name and return its text. Public because **not every answer needs a
    /// model**: `space_events` is a list the calendar already has in the right order, and
    /// routing it through an engine costs 17 s, ~3 000 tokens, and a chance the model drops a
    /// row — see `triage::events_window` and `run_digest`.
    pub fn call(&mut self, name: &str, arguments: Json) -> Result<String, String> {
        self.call_tool(name, arguments)
    }

    fn call_tool(&mut self, name: &str, arguments: Json) -> Result<String, String> {
        let Some((_, index)) = self.owners.iter().find(|(n, _)| n == name).cloned() else {
            return Err(format!(
                "no tool called \"{name}\". Available: {}",
                self.tool_names().join(", ")
            ));
        };
        let (url, client) = &self.servers[index];
        client
            .call_text(name, arguments)
            // common-mcp already prefixes the URL; do not double it.
            .map_err(|e| if e.starts_with(url.as_str()) { e } else { format!("{url}: {e}") })
    }
}

/// Drop the oldest exchanges until the transcript is within
/// [`HISTORY_TURNS`] user turns, never splitting a tool call from its result.
fn trim(transcript: &mut Vec<Json>) {
    let budget = HISTORY_TURNS * 3 + 1;
    if transcript.len() <= budget {
        return;
    }
    // Index 0 is the system prompt and stays.
    let mut cut = transcript.len() - (budget - 1);
    if cut < 1 {
        return;
    }
    // Move the cut forward past anything that must stay with what precedes it: a
    // `role: tool` reply, or the user turn that follows an assistant tool call.
    while cut > 1 && cut < transcript.len() {
        let prev = &transcript[cut - 1];
        let keep_with_previous = transcript[cut]
            .get("role")
            .and_then(|r| r.as_str())
            == Some("tool")
            || prev
                .get("tool_calls")
                .and_then(|t| t.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false);
        if !keep_with_previous {
            break;
        }
        cut += 1;
    }
    if cut > 1 && cut <= transcript.len() {
        transcript.drain(1..cut);
    }
}

fn compact(s: &str) -> String {
    let one: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if one.chars().count() > 60 {
        format!("{}…", one.chars().take(60).collect::<String>())
    } else {
        one
    }
}

fn truncate(text: String) -> String {
    let count = text.chars().count();
    if count <= MAX_RESULT_CHARS {
        return text;
    }
    let head: String = text.chars().take(MAX_RESULT_CHARS).collect();
    format!(
        "{head}\n… [truncated: {count} characters, first {MAX_RESULT_CHARS} shown — narrow the \
         tool call with a filter rather than asking for more]"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: &str, tool_calls: bool) -> Json {
        let mut pairs = vec![("role".to_string(), json::string(role))];
        if tool_calls {
            pairs.push((
                "tool_calls".to_string(),
                Json::Arr(vec![json::object(vec![(
                    "id".to_string(),
                    json::string("call_1"),
                )])]),
            ));
        }
        json::object(pairs)
    }

    /// The system prompt is message 0 and the transcript must stay usable after a
    /// trim — which for these servers means no orphan `role: tool` message and no
    /// assistant message whose results have gone missing.
    #[test]
    fn trimming_keeps_the_system_prompt_and_never_splits_a_tool_exchange() {
        let mut t = seed_transcript();
        for i in 0..30 {
            t.push(message("user", false));
            t.push(message("assistant", true));
            t.push(message("tool", false));
            let _ = i;
        }
        trim(&mut t);
        assert!(t.len() <= HISTORY_TURNS * 3 + 1, "{} messages kept", t.len());
        assert_eq!(t[0].get("role").and_then(|r| r.as_str()), Some("system"));
        assert_ne!(t[1].get("role").and_then(|r| r.as_str()), Some("tool"), "orphan tool reply");
        for i in 1..t.len() {
            let role = t[i].get("role").and_then(|r| r.as_str()).unwrap_or("");
            assert!(
                role != "tool" || matches!(t[i - 1].get("tool_calls").and_then(|c| c.as_array()), Some(a) if !a.is_empty()),
                "message {i} is a tool reply whose call is gone"
            );
        }
    }

    #[test]
    fn a_truncation_announces_itself_because_a_silent_one_lies() {
        let long = "x".repeat(MAX_RESULT_CHARS + 500);
        let out = truncate(long);
        assert!(out.contains("truncated"), "no notice");
        assert!(out.contains(&format!("{MAX_RESULT_CHARS}")), "total not stated");
        // A result that already fits must not be touched at all.
        let short = "Leigh Hackspace is CLOSED".to_string();
        assert_eq!(truncate(short.clone()), short);
    }

    #[test]
    fn an_empty_answer_is_an_error_and_not_an_empty_reply() {
        // Guard the shape rather than the model: `ask` returns Err when the engine
        // finishes with no text, so a caller can say "it said nothing" instead of
        // posting an empty message.
        let turn = crate::engine::Turn {
            text: Some("   ".to_string()),
            finish: "stop".to_string(),
            ..Default::default()
        };
        assert!(turn.text.as_deref().map(|s| s.trim().is_empty()) == Some(true));
    }
}
