//! `events` — the wire contract of one spoken turn, and nothing else.
//!
//! Split out of [`crate::turn`] because the two halves answer different questions: `turn.rs` is
//! the pipeline that *produces* these, this file is what they *say*. Both ends of every socket
//! agree because there is exactly one definition of each: [`Out::to_json`] here, [`Out::from_json`]
//! here, and the satellite reads them with the same `Out` rather than a second struct that could
//! drift. `AGENTS.md`'s one-fact rule, applied to a protocol instead of an IP address.
//!
//! ## The names on the wire, and who switches on them
//!
//! `asr` · `log` · `token` · `reset` · `audio` · `warn` · `answer` · `done` · `error`.
//!
//! A browser page and a camera on a roof both switch on these strings, so a renamed event is a
//! broken client that no compile error will find. The names are pinned by a test in this file, and
//! [`Out::from_json`] ignores anything it does not know: an old satellite on a new gateway keeps
//! playing the audio it understands instead of failing to parse the turn.
//!
//! ## Two shapes worth knowing before adding a variant
//!
//! - **Audio is base64 PCM16 inside JSON**, not a second connection and not a raw chunked body.
//!   That costs a third more bytes on a loopback socket and buys a client that can correlate each
//!   sample with the text and the timing it belongs to, over one CORS-allowed origin.
//! - **`done` carries every timing and `error` carries the reason**, and exactly one of them is
//!   last. A client that has to guess whether the turn is over is a client that hangs.

use common_json::{self as json, Json};

use crate::wav;
/// What the turn cost, and when each part of it landed.
#[derive(Debug, Clone, Default)]
pub struct Timings {
    pub asr_ms: u128,
    /// The slice that was still running when the speaker stopped: the ASR cost the user actually
    /// waits for, as opposed to the total the box spent.
    pub asr_tail_ms: u128,
    pub asr_calls: usize,
    pub first_token_ms: u128,
    pub first_audio_ms: u128,
    pub total_ms: u128,
    pub audio_secs: f64,
    pub sentences: usize,
    pub parts: usize,
    /// Everything that was skipped or dropped, in order. A turn that produced less audio than
    /// the words it was given has to say why, or the next person to read this assumes the model
    /// answered short.
    pub warnings: Vec<String>,
    /// The warm-up's own report, or its absence ("never fired").
    pub warm: Option<Warm>,
    pub engine: String,
    pub rounds: u64,
}

impl Timings {
    /// A one-line summary for a log or a status file: the four numbers that say whether this
    /// turn was fast and whether the warm-up helped.
    pub fn summary(&self) -> String {
        format!(
            "asr {} ms ({} tail, {} slice(s)) · first token {} ms · first audio {} ms \
             · {} sentence(s) · {:.1} s of speech in {} ms · warm: {}{}",
            self.asr_ms,
            self.asr_tail_ms,
            self.asr_calls,
            self.first_token_ms,
            self.first_audio_ms,
            self.sentences,
            self.audio_secs,
            self.total_ms,
            match &self.warm {
                Some(w) => format!(
                    "cached {}/{} tokens in {} ms{}",
                    w.cached_tokens,
                    w.prompt_tokens,
                    w.millis,
                    if w.ok { "" } else { " (FAILED)" }
                ),
                None => "never fired".to_string(),
            },
            if self.warnings.is_empty() {
                String::new()
            } else {
                format!(" · {} warning(s)", self.warnings.len())
            }
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct Warm {
    pub ok: bool,
    pub prompt_tokens: u64,
    pub cached_tokens: u64,
    pub millis: u64,
    pub detail: String,
}

/// One event of a turn.
#[derive(Debug, Clone)]
pub enum Out {
    /// `{"asr":"…","done":false}` — what the microphone has so far.
    Asr { text: String, done: bool },
    /// Plumbing, for a status line. Never spoken.
    Log(String),
    /// Part of the answer's text.
    Token(String),
    /// Discard the text shown so far: a round that looked like an answer became a tool call.
    Reset,
    /// PCM16 at 16 kHz mono, in order, `seq` starting at 0.
    Audio { seq: u32, pcm: Vec<i16> },
    /// Something was dropped or refused, and the turn continued.
    Warn(String),
    /// The whole answer and its receipts, once.
    Answer {
        text: String,
        engine: String,
        rounds: u64,
        tools: Vec<String>,
    },
    /// The end, with every timing. Always the last event when the turn reached it.
    Done(Timings),
    /// The end, with a reason. Always the last event when something failed.
    Error(String),
}

impl Out {
    /// The SSE event name.
    pub fn name(&self) -> &'static str {
        match self {
            Out::Asr { .. } => "asr",
            Out::Log(_) => "log",
            Out::Token(_) => "token",
            Out::Reset => "reset",
            Out::Audio { .. } => "audio",
            Out::Warn(_) => "warn",
            Out::Answer { .. } => "answer",
            Out::Done(_) => "done",
            Out::Error(_) => "error",
        }
    }

    /// The JSON body. Written here, and read by [`Out::from_json`], so the gateway and the
    /// satellite cannot have two ideas about what `audio` means.
    pub fn to_json(&self) -> String {
        match self {
            Out::Asr { text, done } => format!(
                "{{\"asr\":{},\"done\":{}}}",
                json::quoted(text),
                done
            ),
            Out::Log(line) => format!("{{\"line\":{}}}", json::quoted(line)),
            Out::Token(t) => format!("{{\"t\":{}}}", json::quoted(t)),
            Out::Reset => "{}".to_string(),
            Out::Audio { seq, pcm } => {
                let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
                format!(
                    "{{\"seq\":{},\"rate\":{},\"pcm16\":{}}}",
                    seq,
                    wav::SAMPLE_RATE,
                    json::quoted(&crate::b64::encode(&bytes))
                )
            }
            Out::Warn(w) => format!("{{\"warn\":{}}}", json::quoted(w)),
            Out::Answer {
                text,
                engine,
                rounds,
                tools,
            } => format!(
                "{{\"answer\":{},\"engine\":{},\"rounds\":{},\"tools\":[{}]}}",
                json::quoted(text),
                json::quoted(engine),
                rounds,
                tools
                    .iter()
                    .map(|s| json::quoted(s))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Out::Done(t) => format!(
                "{{\"asr_ms\":{},\"asr_tail_ms\":{},\"asr_calls\":{},\"first_token_ms\":{},\
                 \"first_audio_ms\":{},\"total_ms\":{},\"audio_secs\":{:.2},\"sentences\":{},\
                 \"parts\":{},\"engine\":{},\"rounds\":{},\"warnings\":[{}],\"warm\":{}}}",
                t.asr_ms,
                t.asr_tail_ms,
                t.asr_calls,
                t.first_token_ms,
                t.first_audio_ms,
                t.total_ms,
                t.audio_secs,
                t.sentences,
                t.parts,
                json::quoted(&t.engine),
                t.rounds,
                t.warnings
                    .iter()
                    .map(|s| json::quoted(s))
                    .collect::<Vec<_>>()
                    .join(","),
                match &t.warm {
                    None => "null".to_string(),
                    Some(w) => format!(
                        "{{\"ok\":{},\"prompt_tokens\":{},\"cached_tokens\":{},\"millis\":{},\
                         \"detail\":{}}}",
                        w.ok, w.prompt_tokens, w.cached_tokens, w.millis, json::quoted(&w.detail)
                    ),
                }
            ),
            Out::Error(e) => json::error_object(e),
        }
    }

    /// Parse one wire event. `None` for anything unrecognised, which the caller logs rather than
    /// treats as a failure: a client that stops understanding is an *old* client, and an old
    /// client should still get the parts it knows.
    pub fn from_json(name: &str, data: &str) -> Option<Out> {
        let v = json::parse(data).ok()?;
        let text = |k: &str| -> String {
            v.get(k)
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string()
        };
        let num = |k: &str| -> u64 { v.get(k).and_then(Json::as_f64).map(|n| n as u64).unwrap_or(0) };
        match name {
            "asr" => Some(Out::Asr {
                text: text("asr"),
                done: v.get("done").and_then(Json::as_bool).unwrap_or(true),
            }),
            "log" => Some(Out::Log(text("line"))),
            "token" => Some(Out::Token(text("t"))),
            "reset" => Some(Out::Reset),
            "audio" => {
                let bytes = crate::b64::decode(&text("pcm16")).ok()?;
                let pcm = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|p| i16::from_le_bytes(*p))
                    .collect();
                Some(Out::Audio {
                    seq: num("seq") as u32,
                    pcm,
                })
            }
            "warn" => Some(Out::Warn(text("warn"))),
            "answer" => Some(Out::Answer {
                text: text("answer"),
                engine: text("engine"),
                rounds: num("rounds"),
                tools: v
                    .get("tools")
                    .and_then(Json::as_array)
                    .map(|a| a.iter().filter_map(Json::as_str).map(str::to_string).collect())
                    .unwrap_or_default(),
            }),
            "done" => Some(Out::Done(Timings {
                asr_ms: num("asr_ms") as u128,
                asr_tail_ms: num("asr_tail_ms") as u128,
                asr_calls: num("asr_calls") as usize,
                first_token_ms: num("first_token_ms") as u128,
                first_audio_ms: num("first_audio_ms") as u128,
                total_ms: num("total_ms") as u128,
                audio_secs: v.get("audio_secs").and_then(Json::as_f64).unwrap_or(0.0),
                sentences: num("sentences") as usize,
                parts: num("parts") as usize,
                warm: v.get("warm").and_then(Json::as_object).map(|_| Warm {
                    ok: v
                        .at(&["warm", "ok"])
                        .and_then(Json::as_bool)
                        .unwrap_or(false),
                    // Read from the nested object, like every other warm field: an earlier
                    // version of this line read a name that is never written, so the cache hit
                    // rate silently arrived as zero at whoever was drawing the graph.
                    prompt_tokens: v
                        .at(&["warm", "prompt_tokens"])
                        .and_then(Json::as_f64)
                        .map(|n| n as u64)
                        .unwrap_or(0),
                    cached_tokens: v
                        .at(&["warm", "cached_tokens"])
                        .and_then(Json::as_f64)
                        .map(|n| n as u64)
                        .unwrap_or(0),
                    millis: v.at(&["warm", "millis"]).and_then(Json::as_f64).map(|n| n as u64).unwrap_or(0),
                    detail: v.at(&["warm", "detail"]).and_then(Json::as_str).unwrap_or("").to_string(),
                }),
                engine: text("engine"),
                rounds: num("rounds"),
                warnings: v
                    .get("warnings")
                    .and_then(Json::as_array)
                    .map(|a| a.iter().filter_map(Json::as_str).map(str::to_string).collect())
                    .unwrap_or_default(),
            })),
            "error" => Some(Out::Error(
                v.get("error")
                    .and_then(Json::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
            )),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every event, out and back. `to_json`/`from_json` are the wire contract for a route this
    /// repo's own satellite calls, so a variant that loses a field would otherwise be found by a
    /// camera on a roof.
    #[test]
    fn every_event_round_trips_through_the_wire_format() {
        let timings = Timings {
            asr_ms: 1,
            asr_tail_ms: 2,
            asr_calls: 3,
            first_token_ms: 4,
            first_audio_ms: 5,
            total_ms: 6,
            audio_secs: 7.5,
            sentences: 8,
            parts: 9,
            warnings: vec!["a warning".into()],
            warm: Some(Warm {
                ok: true,
                prompt_tokens: 900,
                cached_tokens: 800,
                millis: 40,
                detail: "fake".into(),
            }),
            engine: "fake (bench GPU)".into(),
            rounds: 2,
        };
        let pcm: Vec<i16> = vec![1, -2, 300, -300];
        let events = vec![
            Out::Asr {
                text: "is the sp".into(),
                done: false,
            },
            Out::Log("calling space_status".into()),
            Out::Token("open".into()),
            Out::Reset,
            Out::Audio { seq: 4, pcm: pcm.clone() },
            Out::Warn("no audio: piper died".into()),
            Out::Answer {
                text: "It is open.".into(),
                engine: "fake".into(),
                rounds: 1,
                tools: vec!["space_status".into()],
            },
            Out::Done(timings.clone()),
            Out::Error("the engine refused".into()),
        ];
        for (want_seq, ev) in events.iter().enumerate() {
            let back = Out::from_json(ev.name(), &ev.to_json())
                .unwrap_or_else(|| panic!("{} did not survive the round trip", ev.name()));
            match (ev, back) {
                (Out::Audio { seq, pcm: a }, Out::Audio { seq: s2, pcm: b }) => {
                    assert_eq!(*seq, s2, "audio sequence number");
                    assert_eq!(a, &b, "audio samples");
                    assert_eq!(*seq as usize, want_seq, "audio events come in order");
                }
                (Out::Done(a), Out::Done(b)) => {
                    assert_eq!(a.total_ms, b.total_ms);
                    assert_eq!(a.asr_tail_ms, b.asr_tail_ms);
                    assert_eq!(a.first_audio_ms, b.first_audio_ms);
                    assert_eq!(a.warnings, b.warnings);
                    assert_eq!(
                        a.warm.as_ref().map(|w| (w.cached_tokens, w.prompt_tokens)),
                        b.warm.as_ref().map(|w| (w.cached_tokens, w.prompt_tokens))
                    );
                }
                (Out::Asr { text, done }, Out::Asr { text: t2, done: d2 }) => {
                    assert_eq!(text, &t2);
                    assert_eq!(done, &d2);
                }
                (Out::Answer { text, rounds, tools, .. }, Out::Answer { text: t2, rounds: r2, tools: to2, .. }) => {
                    assert_eq!(text, &t2);
                    assert_eq!(*rounds, r2);
                    assert_eq!(tools, &to2);
                }
                _ => {}
            }
        }
        // The names on the wire, pinned because they are the API a browser page switches on.
        let names: Vec<&str> = events.iter().map(|e| e.name()).collect();
        assert_eq!(
            names,
            vec![
                "asr", "log", "token", "reset", "audio", "warn", "answer", "done", "error"
            ]
        );
    }

    /// The summary is what `just voice-status` and the request log show. If it ever says nothing
    /// about the first audio, the one number this whole change is about becomes invisible.
    #[test]
    fn the_timing_summary_names_the_numbers_that_matter() {
        let t = Timings {
            asr_tail_ms: 260,
            first_audio_ms: 410,
            total_ms: 6100,
            sentences: 4,
            warm: Some(Warm {
                ok: true,
                cached_tokens: 800,
                prompt_tokens: 900,
                ..Default::default()
            }),
            engine: "services2 (RTX 3060)".into(),
            ..Default::default()
        };
        let s = t.summary();
        for want in [
            "260 tail",
            "first audio 410 ms",
            "4 sentence(s)",
            "cached 800/900",
            "6100 ms",
        ] {
            assert!(s.contains(want), "{s:?} has no {want:?}");
        }
    }
}
