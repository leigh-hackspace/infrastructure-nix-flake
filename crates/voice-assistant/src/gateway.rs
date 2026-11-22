//! The satellite's side of the gateway: the four requests it can make, and the one piece of
//! rolling state it keeps.
//!
//! Split out of [`crate::satellite`] because those two halves fail in different ways.
//! `satellite.rs` is a microphone loop — ingest, gate, discard, report — and its failures are
//! things like a source that stopped producing frames or a floor that learned the wrong room.
//! This file is a client of an HTTP service on another machine, and its failures are timeouts,
//! 404s from a gateway that has not been switched, and an assistant that answered nothing.
//!
//! ## Why the satellite has no whisper URL and no agent URL of its own
//!
//! It asks the gateway for both (`/v1/transcribe`, `/v1/turn`, `/v1/warm`), so "where is the
//! speech-to-text" and "where is the answer" each have one answer in the unit. A camera end that
//! dialled whisper directly would be a second configuration to keep in step and a second path for
//! the same audio, and the camera cannot even resolve most of those names without the router.
//!
//! ## The warm-up is a favour, not a step
//!
//! [`warm`] runs on a detached thread and its result is nobody's error. It exists so the
//! inference engine is holding this conversation's prefix before the real request arrives, and a
//! warm-up that could delay or fail the turn it was meant to speed up would be a bug rather than
//! an optimisation. See `docs/voice-assistant-2026-11-16.md` for what it buys.

use std::time::Duration;

use common_json::{self as common_json, Json};

use crate::satellite::{Options, Stats};
use crate::sink::Played;
use crate::wav;
/// An utterance being transcribed **while it is still being spoken**.
///
/// The satellite owns one of these per open window. It is fed from [`crate::gate::Gate::take_open_samples`]
/// — each sample exactly once — and when the window closes, whatever is left is one last slice.
/// The payoff is that a five-second sentence costs the listener only the *tail* slice after they
/// stop, instead of five seconds of whisper; the `asr_tail_ms` in `status.json` is that number,
/// measured rather than claimed.
///
/// `fed` is why the tail is not lost: the frames that closed the window left the gate inside the
/// `Window`, so the window itself says how much of it this rolling transcriber has never seen.
pub struct Live {
    pub rolling: crate::asr::Rolling,
    /// How many samples of the current window have already been handed to `rolling`. The frames
    /// that *closed* the window leave the gate inside its `Window`, so this is the only way to
    /// know which part of an utterance nobody has transcribed yet.
    pub fed: usize,
    /// Whether this utterance has already fired a warm-up: one per utterance, not one per slice.
    pub warmed: bool,
}

/// One slice of an open window, through the gateway: the satellite has no whisper URL of its own
/// (that is the gateway's configuration), and a camera end that opened its own whisper client
/// would be a second answer to "where is the speech-to-text" that could disagree.
pub(crate) fn transcribe_slice(gateway: &str, samples: &[i16]) -> Result<String, String> {
    let body = wav::mono16(samples, wav::SAMPLE_RATE);
    let reply = common_http::request_bytes(
        "POST",
        &format!("{gateway}/v1/transcribe"),
        Some(&body),
        "audio/wav",
        &[],
        Duration::from_secs(60),
    )
    .map_err(|e| format!("transcribe: {e}"))?;
    if !reply.ok() {
        return Err(format!("transcribe: {}", reply.describe()));
    }
    let value = common_json::parse(&reply.body).map_err(|e| format!("transcribe body: {e}"))?;
    Ok(value
        .get("text")
        .and_then(Json::as_str)
        .unwrap_or("")
        .to_string())
}

/// Ask the gateway one question and **play the answer as it arrives**.
///
/// Returns the seconds the sink reports having played, which is what the caller's half-duplex
/// guard needs; a turn that produced no audio returns `Err(NO_AUDIO)`, which is not a fault and
/// is not logged as one.
pub(crate) fn ask_turn(opts: &mut Options, question: &str, stats: &mut Stats) -> Result<Played, String> {
    let gateway = opts.gateway.clone();
    let max_secs = opts.max_play_secs;
    let body = format!("{{\"text\":{},\"voice\":true}}", common_json::quoted(question));
    let mut events = common_http::stream::request_stream(
        "POST",
        &format!("{gateway}/v1/turn"),
        Some(body.as_bytes()),
        "application/json",
        &[],
        Duration::from_secs(180),
    )
    .map_err(|e| format!("gateway: {e}"))?;
    if !events.ok() {
        let status = events.status;
        let why = events.read_rest(200);
        return Err(format!("gateway: HTTP {status}: {why}"));
    }
    // A streamed sink is fed as chunks land; a sink that cannot stream gets the same samples
    // buffered and played at the end. The choice is the sink's (`Sink::streams`), never a guess
    // made here about a device this file is not allowed to know about.
    let mut live_sink = if opts.sink.streams() {
        match opts.sink.play_stream() {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("sink cannot stream, buffering instead: {e}");
                None
            }
        }
    } else {
        None
    };
    let mut buffered: Vec<i16> = Vec::new();
    let mut secs = 0.0f64;
    let mut truncated = false;
    let mut answer = String::new();
    let mut engine = String::new();
    let mut summary = String::new();
    let mut error: Option<String> = None;

    loop {
        let ev = match events.read_event() {
            Ok(Some(e)) => e,
            Ok(None) => break,
            Err(e) => return Err(format!("gateway stream: {e}")),
        };
        let Some(out) = crate::turn::Out::from_json(&ev.name, &ev.data) else {
            continue; // an event from a newer gateway: ignore it, keep the audio we can use
        };
        match out {
            crate::turn::Out::Asr { text, done } => {
                if done {
                    eprintln!("heard {text:?}");
                } else {
                    eprintln!("   heard {text:?} so far");
                }
            }
            crate::turn::Out::Log(line) => eprintln!("   · {line}"),
            crate::turn::Out::Token(t) => answer.push_str(&t),
            crate::turn::Out::Reset => answer.clear(),
            crate::turn::Out::Warn(w) => eprintln!("warning: {w}"),
            crate::turn::Out::Answer { text, engine: e, .. } => {
                answer = text;
                engine = e;
            }
            crate::turn::Out::Done(t) => summary = t.summary(),
            crate::turn::Out::Error(e) => error = Some(e),
            crate::turn::Out::Audio { pcm, .. } => {
                if truncated {
                    continue;
                }
                secs += pcm.len() as f64 / wav::SAMPLE_RATE as f64;
                if secs > max_secs {
                    // The belt, and it stops the answer rather than the room: a turn that runs on
                    // because the model was verbose should not talk for a minute after the
                    // operator's ceiling-speaker limit.
                    eprintln!(
                        "not speaking the rest: {:.0} s is over the --max-play-secs of {:.0} s",
                        secs, max_secs
                    );
                    truncated = true;
                    continue;
                }
                if let Some(s) = live_sink.as_mut() {
                    if let Err(e) = s.chunk(&pcm) {
                        return Err(format!("sink: {e}"));
                    }
                } else {
                    buffered.extend_from_slice(&pcm);
                }
            }
        }
    }

    let played = match live_sink {
        Some(s) => s.finish()?,
        None if !buffered.is_empty() => opts.sink.play(&buffered)?,
        None => Played { secs: 0.0 },
    };
    if let Some(e) = error {
        return Err(e);
    }
    if answer.trim().is_empty() && played.secs == 0.0 {
        return Err(crate::turn::NO_AUDIO.to_string());
    }
    stats.first_audio_ms = 0; // replaced by the gateway's own figure in `summary`
    eprintln!(
        "answered ({engine}): {answer}{}",
        if summary.is_empty() { String::new() } else { format!(" — {summary}") }
    );
    Ok(played)
}

/// Fire the engine warm-up, and do nothing at all about the answer. A warm-up is a favour: it has
/// no state to corrupt and no caller to report to, so it runs on a detached thread — waiting for
/// it here would put the thing it was meant to speed up *behind* it.
pub(crate) fn warm(gateway: &str, partial: &str) {
    let url = format!("{gateway}/v1/warm");
    let body = format!("{{\"text\":{}}}", common_json::quoted(partial));
    std::thread::spawn(move || {
        match common_http::request(
            "POST",
            &url,
            Some(&body),
            &[],
            Duration::from_secs(30),
        ) {
            Ok(r) if r.ok() => eprintln!("warm-up: {}", r.body),
            Ok(r) => eprintln!("warm-up answered {}: {}", r.status, r.describe()),
            Err(e) => eprintln!("warm-up failed (harmless): {e}"),
        }
    });
}

/// The buffered path: the whole window in one whisper call. Returns `(text, described)`, or `None`
/// after having counted and reported the failure.
pub(crate) fn transcribe_whole(opts: &Options, body: &[u8]) -> Option<(String, bool)> {
    let reply = match common_http::request_bytes(
        "POST",
        &format!("{}/v1/transcribe", opts.gateway),
        Some(body),
        "audio/wav",
        &[],
        Duration::from_secs(60),
    ) {
        Ok(r) if r.ok() => r,
        Ok(r) => {
            eprintln!("transcribe failed: {}", r.describe());
            return None;
        }
        Err(e) => {
            eprintln!("{e}");
            return None;
        }
    };
    let value = match common_json::parse(&reply.body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("transcribe body: {e}");
            return None;
        }
    };
    Some((
        value
            .get("text")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string(),
        !matches!(value.get("speech").and_then(Json::as_bool), Some(true)),
    ))
}
