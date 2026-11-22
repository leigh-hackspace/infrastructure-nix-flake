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

use crate::engine::{assistant_message, tool_result, Engines, Turn};
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
/// The spoken answer's cap — raised from 200 on 2026-11-17 for a reason, not a feeling.
///
/// Reasoning tokens are billed against this same number (`docs/slack-agent-2026-11-16.md`
/// §15), so a thinking model can spend the entire cap on private monologue, answer with
/// `content: null`, and make the loop retry the whole turn: measured **twice** in
/// `journalctl -u slack-agent-listen` since the voice path went live. This cap is not what
/// controls answer length — the prompt's one-sentence rule is, and real answers used 96–161
/// tokens — so widening it cannot make replies longer; it only removes the cliff. If
/// `--thinking low` ever ships for voice, this can come back down.
pub const MAX_SPEECH_TOKENS: u32 = 320;

/// Compile-time, because the failure mode is silent and one-directional: someone
/// trimming `MAX_ANSWER_TOKENS` for a Slack reason quietly makes the *spoken* answer
/// the longer of the two, and the room only learns by waiting.  A test would catch it
/// too, but a test has to be run; this has to be compiled (`docs/voice-assistant-2026-11-16.md`).
const _: () = assert!(MAX_SPEECH_TOKENS < MAX_ANSWER_TOKENS);

/// What a streaming answer hands its listener, one piece at a time.
///
/// Two cases and not one, because the loop cannot know in advance whether a round that starts
/// with words will end as an answer or as a tool call.  A listener that renders or speaks text as
/// it arrives therefore *must* be able to be told to take it back, or the failure mode is the
/// assistant saying the same clause twice in a room full of people.
#[derive(Debug, Clone)]
pub enum Piece {
    Text(String),
    /// Discard everything shown so far for this turn: the round that produced it did not answer.
    Reset,
}

pub struct Agent {
    engines: Engines,
    /// How long a streaming answer may stay silent before this harness calls it broken.
    ///
    /// Not the request timeout: a stream that is *alive* can take minutes, and the whole point of
    /// streaming is to be able to say so.  What is worth a deadline is silence, and the measured
    /// engines send an SSE keep-alive comment about every 10 s, so 45 s of nothing is a dead
    /// upstream rather than a thinking model (`docs/slack-agent-2026-11-16.md` §16).
    stream_idle: std::time::Duration,
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
    /// Silence tolerance for a streaming answer; see [`Agent::stream_idle`].
    pub const STREAM_IDLE: std::time::Duration = std::time::Duration::from_secs(45);

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
                stream_idle: Self::STREAM_IDLE,
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
        self.run(transcript, question, max_tokens, &mut on_event, None)
    }

    /// [`ask_budget`] with the answer's **text handed out as it is decoded**.
    ///
    /// The loop is identical — same rounds, same tool budget, same grounding rule — and the
    /// only difference is that the closing round's tokens arrive while they are still being
    /// produced. They arrive as [`Piece`]s rather than strings because of one thing the loop
    /// cannot know in advance: a round that starts with words can still turn out to be a tool
    /// call, and a listener that has already shown or spoken those words must be told to throw
    /// them away. That is `Piece::Reset`, and it is the reason this is not just `&mut dyn
    /// FnMut(&str)`.
    pub fn ask_budget_stream(
        &mut self,
        transcript: &mut Vec<Json>,
        question: &str,
        max_tokens: u32,
        mut on_event: impl FnMut(&str),
        mut on_piece: impl FnMut(Piece),
    ) -> Result<Answer, String> {
        self.run(transcript, question, max_tokens, &mut on_event, Some(&mut on_piece))
    }

    /// Send a turn's prompt prefix to the engine so the real turn is cheaper.  See
    /// [`Engines::prefill`] for what it buys and for the measured `max_tokens: 1` that makes it
    /// work; the point here is that the **prefix must be the one the real request will send**,
    /// so this clones the transcript, appends the question and trims it exactly the way a turn
    /// does. A prefill that does not match is not a warm-up, it is a wasted request.
    pub fn prefill(
        &mut self,
        transcript: &[Json],
        question: &str,
    ) -> Result<crate::engine::Prefill, String> {
        let mut t = transcript.to_vec();
        t.push(json::object(vec![
            ("role".to_string(), json::string("user")),
            ("content".to_string(), json::string(question)),
        ]));
        trim(&mut t);
        let tools = self.openai_tools();
        self.engines.prefill(&t, &tools)
    }

    /// One `chat` call, buffered or streamed, with the streaming plumbing in exactly one place.
    ///
    /// `showed_text` is reported out through the argument because the caller needs both the
    /// [`Turn`] and the fact of whether words went out: a tool round that streamed words must be
    /// unsaid, and a stream that broke after words must not be retried.  Both are decisions the
    /// loop makes, not this function, so this function only reports.
    fn one_chat(
        &mut self,
        transcript: &[Json],
        tools: &[Json],
        budget: u32,
        stream: &mut Option<&mut dyn FnMut(Piece)>,
        showed_text: &mut bool,
    ) -> Result<Turn, crate::engine::StreamFail> {
        let idle = self.stream_idle;
        let Some(sink) = stream.as_deref_mut() else {
            return self
                .engines
                .chat(transcript, tools, budget)
                .map_err(|msg| crate::engine::StreamFail { msg, emitted: false });
        };
        match self.engines.chat_stream(
            transcript,
            tools,
            budget,
            idle,
            &mut |t: &str| sink(Piece::Text(t.to_string())),
        ) {
            Ok((turn, stats)) => {
                *showed_text = stats.emitted;
                Ok(turn)
            }
            Err(e) => Err(e),
        }
    }

    fn run(
        &mut self,
        transcript: &mut Vec<Json>,
        question: &str,
        max_tokens: u32,
        on_event: &mut dyn FnMut(&str),
        mut stream: Option<&mut dyn FnMut(Piece)>,
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
            // One chat call, two ways — see `one_chat`. A failure that happened before any
            // part of the answer reached the listener is retried on the next engine exactly as
            // it always was; one that happened *after* is not retryable, because the room has
            // already heard words it would then hear twice.
            let mut showed_text = false;
            let turn = match self.one_chat(transcript, &tools, budget, &mut stream, &mut showed_text) {
                Ok(turn) => turn,
                Err(first) => {
                    if first.emitted {
                        if let Some(s) = stream.as_deref_mut() {
                            s(Piece::Reset);
                        }
                        return Err(format!(
                            "the answer stream broke after parts of it had already been sent: {}",
                            first.msg
                        ));
                    }
                    self.engines.give_up();
                    on_event(&format!("engine failed: {} — trying the next one", first.msg));
                    match self.one_chat(transcript, &tools, budget, &mut stream, &mut showed_text) {
                        Ok(turn) => turn,
                        Err(second) => {
                            self.engines.give_up();
                            if second.emitted {
                                if let Some(s) = stream.as_deref_mut() {
                                    s(Piece::Reset);
                                }
                            }
                            return Err(format!("every inference endpoint failed: {}", second.msg));
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

            // A round that streamed words and then asked for tools has to be unsaid: those
            // words were a plan, not an answer, and a listener that is speaking as it goes has
            // already started saying them.  Measured on services2 a tool round carries no
            // `content` deltas at all, so this is rare — but "rare" on a voice path is the same
            // word as "sometimes", and this is the only place that can know.
            if showed_text && !turn.tool_calls.is_empty() {
                if let Some(sink) = stream.as_deref_mut() {
                    sink(Piece::Reset);
                }
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
                // The closing turn **is** the answer, so it streams like one: a room that has
                // waited through five tool rounds should not also wait for the whole reply.
                let mut closing_showed = false;
                let final_turn = self
                    .one_chat(transcript, &[], budget, &mut stream, &mut closing_showed)
                    .map_err(|e| format!(
                        "out of tool budget ({} calls) and the closing turn failed too: {}",
                        log.len(),
                        e.msg
                    ))?;
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

    /// Answer with **one tool call and no model**, when the question is that kind of question.
    ///
    /// `None` means "ask the model", and it is what happens on every problem: no shape
    /// matched, the tool errored, or the tool *refused* (`space_status` with a room it does
    /// not know returns an error precisely so it lands here). A fast path is only allowed to
    /// make an answer faster — never wronger, never shorter, never the answer to a different
    /// question — so it declines rather than improvises, and the fall-through is the same
    /// rule `answer_events` has followed since phase 1.
    ///
    /// The sentence is the tool's, not this crate's: see `hackspace-mcp`'s `focused`, which is
    /// where "how we say a room's temperature" lives, so a fast answer and a model-relayed
    /// answer cannot disagree about what 12.1 degrees sounds like.
    pub fn quick_answer(&mut self, text: &str, mut warn: impl FnMut(&str)) -> Option<QuickHit> {
        use crate::triage::Quick;
        let hit = crate::triage::quick(text)?;
        let (name, arguments, label) = match hit {
            Quick::Time => ("what_time", common_json::object(vec![]), "what_time".to_string()),
            Quick::Open => ("space_status", focus("open"), "space_status(open)".to_string()),
            Quick::Extremes => (
                "space_status",
                focus("extremes"),
                "space_status(extremes)".to_string(),
            ),
            Quick::Room(room) => {
                let label = format!("space_status({room})");
                ("space_status", focus(&room), label)
            }
            Quick::Events(days) => {
                let label = format!("space_events({days}d)");
                (
                    "space_events",
                    common_json::object(vec![(
                        "days".to_string(),
                        common_json::number_from_int(days.clamp(1, 60) as u64),
                    )]),
                    label,
                )
            }
            Quick::Devices => (
                "network_clients",
                common_json::object(vec![]),
                "network_clients".to_string(),
            ),
        };
        match self.call_tool(name, arguments) {
            Ok(answer) => Some(QuickHit { label, text: answer }),
            Err(e) => {
                warn(&format!("fast path {label} declined ({e}); asking the model"));
                None
            }
        }
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

/// A fast-path answer: the shape that matched (for the log and the metrics) and the sentence
/// the tool rendered.
#[derive(Debug)]
pub struct QuickHit {
    pub label: String,
    pub text: String,
}

/// `{"focus": "…"}`, the one argument `space_status` needs to answer a single question.
fn focus(value: &str) -> Json {
    common_json::object(vec![("focus".to_string(), common_json::string(value))])
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
