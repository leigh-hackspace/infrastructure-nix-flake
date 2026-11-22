//! `stream` — one chat request, answered **as it is produced**.
//!
//! This is the streaming half of [`crate::engine`], split out because the two halves have
//! different failure shapes and the file that holds both had grown past the point where a reader
//! could tell which was which. The buffered call in `engine.rs` either returns a turn or it does
//! not. This one can return a turn *and* be wrong, because it handed pieces of it to the caller on
//! the way, and everything in here exists to keep that case visible.
//!
//! ## What the wire actually looks like, and what the parser therefore does
//!
//! These are the measured behaviours of the engines in `common/hosts.nix` (strata on services2,
//! services3 and aibox), each of which is a reason a naive `for line in body.lines()` breaks:
//!
//! - **A `data:` line may be anything.** A keep-alive comment (`: ping`) and a proxy that injects
//!   a stray line both arrive; the first is skipped and the second is **counted**
//!   ([`StreamStats::malformed`]) rather than fatal, because a turn that dies at line nine of a
//!   streaming answer is worse than a turn with one junk line in it — and invisible counting is
//!   how a permanently broken proxy looks healthy forever.
//! - **A chunk may carry no `choices` at all.** The final usage chunk does exactly that, so a
//!   parser that indexes `choices[0]` unconditionally ends the turn with a panic or a spurious
//!   error on a request that succeeded.
//! - **Tool calls arrive fragmented, keyed by `index`.** The first fragment carries the `id` and
//!   `name`, and the `arguments` string comes in pieces across later chunks — assembled here, by
//!   index, into one [`crate::engine::ToolCall`] byte-identical in shape to the buffered path's.
//! - **`reasoning_content` is a sibling of `content`.** Dropped, for the reason stated at the top
//!   of `engine.rs`.
//!
//! ## The one fact a caller must not lose
//!
//! [`StreamFail::emitted`]: whether any part of the answer has already been shown when the stream
//! breaks. Retrying a turn that has already spoken duplicates the words the room heard, and no
//! amount of retry logic inside this file can undo that — so it is reported, and the decision
//! belongs to [`crate::agent`].

use common_json::{self as json, Json};
use std::time::{Duration, Instant};

use crate::engine::{Engines, ToolCall, Turn, Usage};

/// What a **streaming** request saw while it was open, beyond the assembled [`Turn`].
#[derive(Debug, Default)]
pub struct StreamStats {
    /// Milliseconds from sending the request to the first content or tool-call delta.
    /// `None` means "never", which `Some(0)` must not be able to mean as well: on a loopback
    /// a first token really can land inside a millisecond, and an `Option` is the only honest
    /// way to tell that from "no token at all".
    pub first_token_ms: Option<u128>,
    /// Milliseconds to the first delta *of any kind*, including the role-only chunk.
    pub first_chunk_ms: Option<u128>,
    /// `data:` lines that were not JSON.  Counted rather than fatal: a proxy that injects a
    /// stray line must not end a turn, and a server that emits garbage on every event must not
    /// be invisible either.  This is the only way the second case can ever be noticed.
    pub malformed: u64,
    /// How many `data:` chunks arrived.  A low number with a long wall clock is a server that
    /// said it was streaming and buffered anyway, which is measurable, and this is the only
    /// place in the path where it can be caught.
    pub chunks: u64,
    /// An idle gap or transport failure after some part of the answer had already been handed
    /// to the caller.  `true` means the caller **must not** retry the turn quietly: it has
    /// already shown the room words that are about to be duplicated.
    pub emitted: bool,
}

/// A streaming request that did not finish, and **whether it had already handed anything to
/// the caller**.  That second half is the whole reason this is a type rather than a `String`:
/// "the engine failed" has two completely different meanings mid-stream, one of which the
/// caller can recover from by asking again and one of which it cannot, because the listener has
/// already been shown words that a retry would produce twice.
#[derive(Debug)]
pub struct StreamFail {
    pub msg: String,
    pub emitted: bool,
}

impl From<String> for StreamFail {
    /// A failure before the stream was even open: nothing went out.
    fn from(msg: String) -> Self {
        StreamFail { msg, emitted: false }
    }
}

/// A failure that happened while a stream may already have handed parts of the answer over.
fn fail(stats: &StreamStats, msg: String) -> StreamFail {
    StreamFail {
        msg,
        emitted: stats.emitted,
    }
}

impl Engines {
    /// [`chat`] with the answer arriving **as it is generated**: every content delta is handed
    /// to `on_text` the moment it lands, and the [`Turn`] that comes back is the same shape a
    /// buffered call would have produced, so the loop above needs no second code path.
    ///
    /// ## Why this is worth the extra protocol
    ///
    /// The only caller that needs it is a **voice** turn, where the answer's first sentence is
    /// worth more than the answer: measured decode on these engines is 14–39 tok/s
    /// (`docs/slack-agent-2026-11-16.md`), so a 60-token reply is 1.5–4 s of words that can be
    /// synthesised and played *while* the rest is still being decoded. With a buffered call the
    /// room waits for all of it, then waits for piper, then hears the first word.
    ///
    /// ## The two things the wire did that the docs did not say (measured, services2)
    ///
    /// - **The usage chunk has no `choices` at all** — `"choices": []` — so any parser that
    ///   indexes `choices[0]` unconditionally panics on the last chunk of every successful
    ///   stream. It is handled, and there is a test that feeds exactly that shape.
    /// - **Tool calls arrive in pieces**: the first delta carries `id` and `function.name` with
    ///   `arguments: ""`, and the arguments then stream as fragments (`"{"`, `"}"`). They are
    ///   accumulated by `index`, which is the only stable key, and the assembled [`Turn`] is
    ///   byte-identical in shape to the buffered one so `assistant_message` still closes it.
    ///
    /// ## Timeout, and the one failure the caller has to know about
    ///
    /// The client's own timeout is a **total** one, which for a stream is the right budget for
    /// "the model may think for two minutes" but useless for "the model has gone quiet". So the
    /// loop also enforces an idle ceiling of its own and says which of the two fired.
    ///
    /// If a delta has already been handed to `on_text` when the stream breaks,
    /// [`StreamStats::emitted`] is `true` and the caller must treat the turn as *partly shown*
    /// — resetting the conversation or starting over would duplicate the words the room has
    /// already heard, which is why this is reported rather than papered over internally.
    pub fn chat_stream(
        &mut self,
        messages: &[Json],
        tools: &[Json],
        max_tokens: u32,
        idle: Duration,
        on_text: &mut dyn FnMut(&str),
    ) -> Result<(Turn, StreamStats), StreamFail> {
        let engine = self.engine()?;
        let url = format!("{}/chat/completions", engine.base);
        let payload = self.chat_body(&engine, messages, tools, max_tokens, true);
        let started = Instant::now();
        let mut resp = self
            .http()
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .body(payload)
            .send()
            .map_err(|e| StreamFail::from(format!("{}: POST {url}: {e}", engine.label)))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp
                .text()
                .unwrap_or_default()
                .chars()
                .take(300)
                .collect::<String>();
            return Err(StreamFail::from(format!(
                "{}: {url} answered {status}: {text}",
                engine.label
            )));
        }

        let mut text = String::new();
        let mut calls: Vec<ToolCall> = Vec::new();
        let mut finish = String::new();
        let mut usage = Usage::default();
        let mut stats = StreamStats::default();
        let mut line = String::new();
        let mut got_content = false;
        // `blocking::Response` is a `Read`, so the stream is drained with ordinary reads: 16 KiB
        // is larger than any event these engines send, so one read is one or more whole chunks
        // and never splits one.
        let mut buf = vec![0u8; 16 * 1024];

        loop {
            let since = Instant::now();
            let n = std::io::Read::read(&mut resp, &mut buf)
                .map_err(|e| fail(&stats, format!("{}: stream: {e}", engine.label)))?;
            if since.elapsed() > idle {
                // Measured: these engines send a `: keep-alive` SSE comment about every 10 s,
                // which is a read, so this only fires on a genuinely silent upstream rather
                // than on one that is thinking.
                return Err(fail(
                    &stats,
                    format!(
                        "{}: no part of the answer for {:.0?}",
                        engine.label,
                        since.elapsed()
                    ),
                ));
            }
            if n == 0 {
                break;
            }
            stats.chunks += 1;
            let bytes = &buf[..n];
            for piece in String::from_utf8_lossy(bytes).chars() {
                line.push(piece);
                if piece != '\n' {
                    continue;
                }
                let raw = line.trim_end_matches(['\r', '\n']).to_string();
                line.clear();
                let Some(data) = raw.strip_prefix("data:").map(|s| s.trim()) else {
                    continue; // `: keep-alive`, `event: …`, blank lines
                };
                if data == "[DONE]" {
                    break;
                }
                let Ok(doc) = json::parse(data) else {
                    // A foreign or truncated `data:` line: skip it, count it.
                    stats.malformed += 1;
                    continue;
                };
                if let Some(err) = doc.at(&["error", "message"]).and_then(|v| v.as_str()) {
                    return Err(fail(
                        &stats,
                        format!("{}: engine error mid-stream: {err}", engine.label),
                    ));
                }
                if stats.first_chunk_ms.is_none() {
                    stats.first_chunk_ms = Some(started.elapsed().as_millis());
                }
                if let Some(u) = doc.get("usage") {
                    usage.prompt = u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                    usage.completion =
                        u.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                }
                // Measured: the usage-only chunk carries `"choices": []`.
                let Some(choice) = doc
                    .at(&["choices"])
                    .and_then(|c| c.as_array())
                    .and_then(|a| a.first())
                    .cloned()
                else {
                    continue;
                };
                if let Some(f) = choice.get("finish_reason").and_then(|v| v.as_str()) {
                    finish = f.to_string();
                }
                let Some(delta) = choice.get("delta") else { continue };
                if let Some(t) = delta.get("content").and_then(|v| v.as_str()) {
                    if !t.is_empty() {
                        if !got_content {
                            stats.first_token_ms = Some(started.elapsed().as_millis());
                            got_content = true;
                        }
                        text.push_str(t);
                        on_text(t);
                        stats.emitted = true;
                    }
                }
                if let Some(list) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                    for c in list {
                        let index = c.get("index").and_then(|v| v.as_f64()).unwrap_or(0.0) as usize;
                        while calls.len() <= index {
                            calls.push(ToolCall {
                                id: String::new(),
                                name: String::new(),
                                arguments: String::new(),
                            });
                        }
                        if let Some(id) = c.get("id").and_then(|v| v.as_str()) {
                            if !id.is_empty() {
                                calls[index].id = id.to_string();
                            }
                        }
                        if let Some(n) = c.at(&["function", "name"]).and_then(|v| v.as_str()) {
                            if !n.is_empty() {
                                calls[index].name = n.to_string();
                            }
                        }
                        if let Some(a) = c.at(&["function", "arguments"]).and_then(|v| v.as_str()) {
                            calls[index].arguments.push_str(a);
                        }
                        stats.emitted = true;
                    }
                }
            }
        }

        let millis = started.elapsed().as_millis();
        let mut turn = Turn {
            text: {
                let t = text.trim().to_string();
                (!t.is_empty()).then_some(t)
            },
            tool_calls: calls.into_iter().filter(|c| !c.name.is_empty()).collect(),
            finish,
            usage,
            millis,
            engine: engine.label.clone(),
        };
        if turn.finish.is_empty() && turn.tool_calls.is_empty() && turn.text.is_some() {
            // A stream that ended without saying how: a closed socket with a plausible answer is
            // not the same as a finished turn, so say what actually happened rather than "stop".
            turn.finish = "stream-closed".to_string();
        }
        Ok((turn, stats))
    }
}

/// Shared with `engine.rs`'s tests (which is why it is `pub(crate)` rather than private): the
/// stand-in engine is one fixture, and a second copy of a fake HTTP server is a second thing to
/// keep honest about the wire.
#[cfg(test)]
pub(crate) mod tests {
    use crate::engine::{Candidate, Engines};
    use std::io::Write;
    use std::thread;
    use std::time::Duration;

    /// A stand-in engine that answers **both** requests a real one gets: `GET /v1/models`
    /// (`Engines::engine()` discovers the model id before any chat call) and then
    /// `POST /v1/chat/completions` with raw SSE bytes, so the wire framing is what is under
    /// test rather than a restatement of the parser.
    fn fake_sse(chunks: &'static [&'static str], status: &'static str) -> String {
        use std::io::Read;
        use std::net::TcpListener;

        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        thread::spawn(move || {
            // Discovery and the chat call are two connections on one `Connection: close`
            // client, so serve as many as arrive until the listener is dropped.
            for stream in l.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                if head.starts_with("GET") {
                    let body = "{\"data\":[{\"id\":\"fake-model\",\"meta\":{\"n_ctx\":8192}}]}";
                    let _ = s.write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                             Content-Length: {}\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    );
                    let _ = s.flush();
                    continue;
                }
                let mut out = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\n\
                     Transfer-Encoding: chunked\r\n\r\n"
                );
                for c in chunks {
                    out.push_str(&format!("{:x}\r\n{c}\r\n", c.len()));
                }
                out.push_str("0\r\n\r\n");
                let _ = s.write_all(out.as_bytes());
                let _ = s.flush();
            }
        });
        format!("http://127.0.0.1:{port}/v1")
    }

    pub(crate) fn engines_at(base: &str) -> Engines {
        Engines::new(
            vec![Candidate {
                base: base.to_string(),
                label: "fake".to_string(),
            }],
            Duration::from_secs(10),
        )
    }

    /// The shape every OpenAI-dialect server sends, including the two quirks measured on
    /// services2 that a naive parser dies on: a `delta` split across chunks, and a **final usage
    /// chunk whose `choices` is empty**.
    #[test]
    fn a_streamed_answer_arrives_in_order_and_comes_back_whole() {
        let base = fake_sse(
            &[
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Yes — open \"}}]}\n\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"since 18:40.\"},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":41,\"completion_tokens\":9,\"prompt_tokens_details\":{\"cached_tokens\":32}}}\n\n",
                "data: [DONE]\n\n",
            ],
            "200 OK",
        );
        let mut got = String::new();
        let (turn, stats) = engines_at(&base)
            .chat_stream(&[], &[], 200, Duration::from_secs(5), &mut |t| got.push_str(t))
            .unwrap();
        assert_eq!(turn.text.as_deref(), Some("Yes — open since 18:40."));
        assert_eq!(turn.finish, "stop");
        assert_eq!((turn.usage.prompt, turn.usage.completion), (41, 9));
        assert_eq!(got, "Yes — open since 18:40.", "deltas forwarded as they arrived");
        assert!(stats.emitted);
        assert!(stats.first_token_ms.is_some());
        assert_eq!(stats.malformed, 0, "every line here is JSON: {stats:?}");
    }

    /// The comment lines these engines emit every ~10 s must not be parsed as answers, and a
    /// tool call that arrives in fragments must still close as one call — the argument string is
    /// `"{` then `"}`` on the wire, which is the single most common way to get this wrong.
    #[test]
    fn keepalives_are_ignored_and_a_fragmented_tool_call_is_reassembled_by_index() {
        let base = fake_sse(
            &[
                ": keep-alive\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"space_status\",\"arguments\":\"\"}}]}}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\"}}]}}]}\n\n",
                // A fragment whose payload is a lone `}` — the tail of a real call's arguments,
                // and the fragment a parser that scans for structure outside strings loses.
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"}\"}}]}}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call_2\",\"function\":{\"name\":\"what_time\",\"arguments\":\"{}\"}}]}}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
                "data: [DONE]\n\n",
            ],
            "200 OK",
        );
        let mut text = String::new();
        let (turn, _) = engines_at(&base)
            .chat_stream(&[], &[], 200, Duration::from_secs(5), &mut |t| text.push_str(t))
            .unwrap();
        assert!(turn.text.is_none(), "a tool round is not an answer: {turn:?}");
        assert!(text.is_empty(), "no content deltas to forward");
        assert_eq!(turn.finish, "tool_calls");
        let names: Vec<&str> = turn.tool_calls.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["space_status", "what_time"]);
        assert_eq!(turn.tool_calls[0].arguments, "{}", "fragments joined in order");
        assert_eq!(turn.tool_calls[1].id, "call_2");
    }

    /// A proxy or a server that puts a non-JSON line on the stream must not end the turn — and
    /// must not get away with it silently either.
    #[test]
    fn a_foreign_line_skips_counts_and_does_not_end_the_turn() {
        let base = fake_sse(
            &[
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"warm\"}}]}\n\n",
                "data: <html>upstream timed out</html>\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\" and cold\"},\"finish_reason\":\"stop\"}]}\n\n",
            ],
            "200 OK",
        );
        let (turn, stats) = engines_at(&base)
            .chat_stream(&[], &[], 200, Duration::from_secs(5), &mut |_| {})
            .unwrap();
        assert_eq!(turn.text.as_deref(), Some("warm and cold"));
        assert_eq!(stats.malformed, 1, "the garbage line was not counted: {stats:?}");
    }

    #[test]
    fn a_mid_stream_break_says_whether_words_had_already_been_shown() {
        // An engine error on the wire after one content delta: the caller cannot retry, and this
        // is the only place that can say so.
        let base = fake_sse(
            &[
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Half\"}}]}\n\n",
                "data: {\"error\":{\"message\":\"CUDA out of memory\"}}\n\n",
            ],
            "200 OK",
        );
        let e = engines_at(&base)
            .chat_stream(&[], &[], 200, Duration::from_secs(5), &mut |_| {})
            .unwrap_err();
        assert!(e.msg.contains("out of memory"), "{}", e.msg);
        assert!(e.emitted, "the caller must be told words already went out");

        // A non-2xx happens before anything is open: retryable.
        let base = fake_sse(&["{\"error\":\"nope\"}"], "503 Service Unavailable");
        let e = engines_at(&base)
            .chat_stream(&[], &[], 200, Duration::from_secs(5), &mut |_| {})
            .unwrap_err();
        assert!(e.msg.contains("503"), "{}", e.msg);
        assert!(!e.emitted);
    }
}
