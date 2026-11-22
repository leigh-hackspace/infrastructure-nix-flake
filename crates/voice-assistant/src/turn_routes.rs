//! The HTTP surface of a **streamed** turn: `POST /v1/turn` and `POST /v1/warm`.
//!
//! Both live here rather than in [`crate::api`] because both are about the stream rather than
//! about the request: they open an SSE body, hand every event to [`crate::turn`] the moment it
//! exists, and stop the moment the caller stops reading. `api.rs` is the router and the buffered
//! routes — read the head, write one reply — and this file is the two that cannot be written that
//! way. `api.rs` dispatches here; nothing else does.
//!
//! ## What each route promises
//!
//! - **`/v1/turn`** answers a question with events, in order, exactly once terminated: `asr`
//!   (if audio was sent), `log`, `token`, `audio`, then `done` or `error`. Its body is
//!   [`crate::events::Out`], and that file — not this one — is the definition of what each event
//!   contains. A caller that has to guess whether the turn ended is a caller that hangs, so the
//!   last event is written even when the turn failed.
//! - **`/v1/warm`** fires the engine prefill and returns the engine's own numbers. It never
//!   generates an answer, and it is a separate route rather than a flag on `/v1/turn` because it
//!   answers a different question — "be ready for this" — and one that must never be answered by
//!   doing the work.
//!
//! ## Two things that must not be lost when editing either
//!
//! - **Nothing is buffered.** `web::Sse` is the only writer to the socket, it flushes per event,
//!   and it sets `X-Accel-Buffering: no` because the front door would otherwise sit on the whole
//!   answer and hand the browser a turn that arrived all at once — which is the exact behaviour
//!   this pipeline exists to remove, arriving as a 200 and a long silence.
//! - **The caller leaving is normal.** `emit` returns false when the socket is gone, and the turn
//!   stops buying GPU time for nobody. That path must not be logged as an error; `api.rs`'s
//!   request log would fill with turns the user simply closed.

use std::net::TcpStream;
use std::sync::Arc;
use std::time::Instant;

use common_web::{self as web, Reply, Request};

use crate::api::Ctx;
use crate::{b64, turn, wake};
/// `POST /v1/warm` — fire the engine warm-up for a question that is not being asked yet.
///
/// The satellite calls this from the middle of an utterance, on the first partial transcript it
/// has, which is the only moment the warm-up can actually earn anything: the engine spends its
/// prompt-prefill time while the speaker is still talking. It is a separate route rather than a
/// flag on `/v1/turn` because it is a different question — "prepare for this" — and one that must
/// never be answered by generating an answer.
pub(crate) fn warm_route(req: &Request, stream: &mut TcpStream, ctx: &Arc<Ctx>) -> std::io::Result<()> {
    let value = match common_json::parse(&req.body_text()) {
        Ok(v) => v,
        Err(e) => {
            return Reply::json(400, &common_json::error_object(&format!("body is not JSON: {e}")))
                .send(stream)
        }
    };
    let text = value
        .get("text")
        .and_then(common_json::Json::as_str)
        .unwrap_or("")
        .to_string();
    if text.trim().is_empty() {
        return Reply::json(400, &common_json::error_object("expected {\"text\": \"so far\"}"))
            .send(stream);
    }
    match ctx.agent.prefill(&text, true) {
        Ok(p) => Reply::json(
            200,
            &format!(
                "{{\"ok\":{},\"engine\":{},\"prompt_tokens\":{},\"cached_tokens\":{},\
                 \"millis\":{},\"skipped\":{}}}",
                p.ok,
                common_json::quoted(&p.engine),
                p.prompt_tokens,
                p.cached_tokens,
                p.millis,
                common_json::quoted(&p.skipped)
            ),
        )
        .send(stream),
        Err(e) => Reply::json(502, &common_json::error_object(&e)).send(stream),
    }
}

/// `POST /v1/turn` — a question in, a **turn** out: partial transcript, tokens, and the spoken
/// answer as PCM, in the order they happened.
///
/// ```text
///   {"text":"is the space open"}                     the page's live transcript: no whisper
///   {"audio":"<base64 WAV>", "text":null}           16 kHz mono PCM16: transcribed while read
///   {"voice":false}                                  Slack's prompt and budget, not the spoken one
/// ```
///
/// The events are `turn::Out` — `asr`, `log`, `token`, `reset`, `audio`, `warn`, `answer`,
/// `done`/`error` — and `turn::Out::to_json` is their only definition, because the satellite is a
/// client of this route and two descriptions of a wire format are two chances to be wrong.
///
/// Audio is base64 in JSON rather than a second connection or a raw chunked body: 33 % more bytes
/// over a loopback socket and through one CORS-allowed origin, against a client that has to
/// correlate audio with the text and timings it belongs to. `common_http`'s reader reassembles
/// events across read boundaries, so chunk sizes are not the client's problem.
pub(crate) fn turn_route(req: Request, mut stream: TcpStream, ctx: &Arc<Ctx>) -> std::io::Result<()> {
    let origin = req.header("origin");
    if !origin.is_empty() && !ctx.allows_origin(origin) {
        // Same policy as every other route: no headers, the browser blocks it, and the reason is
        // in the page's console rather than invented by us.
        return Reply::json(403, r#"{"error":"origin not allowed"}"#).send(&mut stream);
    }
    let value = match common_json::parse(&req.body_text()) {
        Ok(v) => v,
        Err(e) => {
            return Reply::json(400, &common_json::error_object(&format!("body is not JSON: {e}")))
                .send(&mut stream)
        }
    };
    let text = value
        .get("text")
        .and_then(common_json::Json::as_str)
        .unwrap_or("")
        .to_string();
    let audio = value
        .get("audio")
        .and_then(common_json::Json::as_str)
        .map(|b| b.to_string());
    let input = match (wake::strip(&text, &ctx.opts.wake_words), audio) {
        (q, _) if !q.trim().is_empty() => turn::Input::Text(q),
        (_, Some(b64)) => match b64::decode(&b64) {
            Ok(wav) if wav.len() >= 44 => turn::Input::Wav(wav),
            Ok(_) => {
                return Reply::json(400, &common_json::error_object("that audio is too short to read"))
                    .send(&mut stream)
            }
            Err(e) => {
                return Reply::json(400, &common_json::error_object(&format!("bad audio: {e}")))
                    .send(&mut stream)
            }
        },
        _ => {
            return Reply::json(
                400,
                &common_json::error_object(r#"expected {"text": …} or {"audio": base64 WAV}"#),
            )
            .send(&mut stream)
        }
    };
    let voice = !matches!(
        value.get("voice").and_then(common_json::Json::as_bool),
        Some(false)
    );
    let opts = turn::Options {
        voice,
        warm: ctx.opts.warm,
        slice_secs: ctx.opts.asr_slice_secs,
        max_speech_chars: ctx.opts.max_speech_chars,
    };

    let mut sse = web::Sse::open(stream)?;
    let started = Instant::now();
    let mut wrote = true;
    let mut answer_text = String::new();
    let mut audio_bytes = 0usize;
    let mut done: Option<turn::Timings> = None;
    let mut error: Option<String> = None;
    let result = turn::run(
        input,
        &opts,
        &ctx.stt,
        &ctx.agent,
        &ctx.tts,
        &mut |out| {
            if !wrote {
                return false;
            }
            match out {
                turn::Out::Answer { text, .. } => answer_text = text.clone(),
                turn::Out::Audio { pcm, .. } => audio_bytes += pcm.len() * 2,
                turn::Out::Done(t) => done = Some(t.clone()),
                turn::Out::Error(e) => error = Some(e.clone()),
                _ => {}
            }
            wrote = sse
                .event(out.name(), &out.to_json())
                .is_ok();
            wrote
        },
    );
    if let Some(e) = error.or_else(|| result.err()) {
        ctx.note("/v1/turn", &format!("{text} → {e}"), started.elapsed().as_secs_f64(), false, &e);
    } else if let Some(t) = &done {
        ctx.note(
            "/v1/turn",
            &format!("{text} → {}", answer_text),
            started.elapsed().as_secs_f64(),
            true,
            &t.summary(),
        );
    }
    if let Some(t) = &done {
        let mut turns = ctx.turns.lock().unwrap_or_else(|p| p.into_inner());
        turns.turns += 1;
        turns.last_total_ms = t.total_ms;
        turns.last_first_audio_ms = t.first_audio_ms;
        turns.last_asr_tail_ms = t.asr_tail_ms;
        if let Some(w) = &t.warm {
            if w.ok {
                turns.warm_ok += 1;
            }
            turns.warm_cached_tokens += w.cached_tokens;
            turns.warm_prompt_tokens += w.prompt_tokens;
            turns.last_warm = format!(
                "{} cached of {} prompt tokens in {} ms{}",
                w.cached_tokens,
                w.prompt_tokens,
                w.millis,
                if w.detail.is_empty() { String::new() } else { format!(" ({})", w.detail) }
            );
        }
    }
    let _ = audio_bytes;
    sse.finish()
}

#[cfg(test)]
mod tests {
    use super::{turn_route, warm_route};
    use crate::api::{Counters, Ctx, Options, TurnStats};
    use crate::testkit;
    use crate::{agent, stt, tts};
    use common_web::Request;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    /// Run one route on a real socket in a thread, and return the production SSE client reading
    /// the answer.
    ///
    /// Deliberately not a mock. `Sse` writes chunked frames, sets `TCP_NODELAY` and adds the
    /// headers the front door reads; and the satellite on the other end of this contract is
    /// `common_http::stream`. Reading the raw socket here would mean hand-parsing chunked framing
    /// in a test, which is a second implementation of the thing under test — and a first draft of
    /// this file did exactly that, then reported a phantom event.
    fn drive(
        ctx: &Arc<Ctx>,
        path: &str,
        body: &str,
    ) -> common_http::stream::Stream {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let ctx = Arc::clone(ctx);
        let path_owned = path.to_string();
        let body_owned = body.to_string();
        std::thread::spawn(move || {
            let Ok((sock, _)) = listener.accept() else {
                return;
            };
            let req = Request {
                method: "POST".into(),
                raw_path: path_owned.clone(),
                headers: Default::default(),
                body: body_owned.as_bytes().to_vec(),
            };
            let mut stream = sock;
            let sent = match path_owned.as_str() {
                "/v1/turn" => turn_route(req, stream, &ctx).is_ok(),
                "/v1/warm" => {
                    warm_route(&req, &mut stream, &ctx).map(|_| ()).is_ok()
                        && {
                            drop(stream);
                            true
                        }
                }
                _ => false,
            };
            if !sent {
                eprintln!("route {path_owned} could not write its reply");
            }
        });
        common_http::stream::request_stream(
            "POST",
            &format!("http://{addr}{path}"),
            Some(body.as_bytes()),
            "application/json",
            &[],
            std::time::Duration::from_secs(30),
        )
        .expect("the route never answered")
    }

    /// Every event name a drive produced, in order.
    fn events(stream: &mut common_http::stream::Stream) -> Vec<String> {
        let mut names = Vec::new();
        loop {
            match stream.read_event() {
                Ok(Some(e)) => names.push(e.name),
                Ok(None) => break,
                Err(e) => panic!("the stream broke: {e}"),
            }
        }
        names
    }

    fn ctx(agent_url: &str, dir: &std::path::Path, warm: bool) -> Arc<Ctx> {
        let model = testkit::voice_model(dir);
        let bin = testkit::fake_piper(dir, 9, 0);
        Arc::new(Ctx {
            opts: Options {
                listen: "127.0.0.1:0".into(),
                whisper_url: "http://127.0.0.1:1".into(),
                whisper_language: None,
                agent_url: agent_url.into(),
                wake_url: String::new(),
                piper_bin: bin.to_str().unwrap().into(),
                piper_model: model.to_str().unwrap().into(),
                length_scale: 1.0,
                max_speech_chars: 320,
                allow_origins: vec!["https://whisper.int.leighhack.org".into()],
                satellite_status: None,
                out_dir: dir.join("out"),
                work_dir: dir.join("tts"),
                wake_words: vec!["hey jarvis".to_string()],
                record_questions: true,
                warm,
                asr_slice_secs: 1.0,
            },
            stt: stt::Client::new("http://127.0.0.1:1", None),
            agent: agent::Client::new(agent_url),
            tts: Mutex::new(tts::Synthesiser::new(
                bin.to_str().unwrap(),
                model.to_str().unwrap(),
                1.0,
                &dir.join("frames"),
            )),
            started: Instant::now(),
            counters: Mutex::new(Counters::default()),
            turns: Mutex::new(TurnStats::default()),
        })
    }

    /// The route's HTTP shape, which is the part `api.rs` owns: an SSE body the door must not
    /// buffer, and events that end exactly once.
    #[test]
    fn turn_streams_events_over_http_and_ends_once() {
        let dir = testkit::tmp("api-turn");
        let (url, fake) = testkit::fake_agent(
            vec![
                ("start", r#"{"tools":["space_status"]}"#.to_string(), 0),
                ("partial", r#"{"t":"The space is open."}"#.to_string(), 0),
                (
                    "answer",
                    r#"{"answer":"The space is open.","engine":"fake","rounds":1,"tools":[]}"#
                        .to_string(),
                    0,
                ),
            ],
            1,
        );
        let ctx = ctx(&url, &dir, false);
        let mut stream = drive(&ctx, "/v1/turn", r#"{"text":"is the space open"}"#);
        assert!(stream.ok(), "HTTP {}", stream.status);
        assert!(
            stream
                .headers
                .get("content-type")
                .is_some_and(|c| c.starts_with("text/event-stream")),
            "{:?}",
            stream.headers
        );
        assert_eq!(
            stream.headers.get("x-accel-buffering").map(String::as_str),
            Some("no"),
            "the front door would have buffered the stream: {:?}",
            stream.headers
        );
        let names = events(&mut stream);
        fake.join().unwrap();
        assert_eq!(
            names,
            vec!["log", "token", "audio", "answer", "done"],
            "wrong events or wrong order"
        );
        // The turn is on the record: `status.json` shows it, with the timings that say whether the
        // pipeline did its job. A route that answered but never counted would look healthy.
        let t = ctx.turns.lock().unwrap();
        assert_eq!(t.turns, 1);
        assert!(t.last_total_ms > 0);
        drop(t);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A body with neither words nor audio is the mistake a page makes when its recorder fails
    /// silently, and it must be refused before the engine is woken. (A bare wake word is *not*
    /// one of these: `wake::strip` leaves the text alone when nothing follows the phrase, which is
    /// `wake.rs`'s own tested behaviour.)
    #[test]
    fn a_turn_that_was_asked_nothing_is_refused_before_the_engine_is_woken() {
        let dir = testkit::tmp("api-refuse");
        let (url, _fake) = testkit::fake_agent(vec![], 0);
        let ctx = ctx(&url, &dir, false);
        for body in [r#"{}"#, r#"{"text":""}"#, r#"{"audio":"not base64!!"}"#, r#"{"audio":"a"}"#] {
            let stream = drive(&ctx, "/v1/turn", body);
            assert_eq!(stream.status, 400, "{body} → {}", stream.status);
        }
    }

    /// The warm-up route answers with the engine's numbers and never with an answer: if it ever
    /// generated text, the satellite would be paying for a second turn while the person speaks.
    #[test]
    fn the_warm_route_reports_the_cache_hit_it_bought() {
        let dir = testkit::tmp("api-warm");
        let (url, fake) = testkit::fake_agent(vec![], 1);
        let ctx = ctx(&url, &dir, false);
        let mut stream = drive(&ctx, "/v1/warm", r#"{"text":"is the space"}"#);
        assert!(stream.ok(), "HTTP {}", stream.status);
        let body = stream.read_rest(2000);
        fake.join().unwrap();
        assert!(body.contains("\"cached_tokens\":0"), "{body}");
        assert!(body.contains("900"), "the engine's prompt size is missing: {body}");
        assert_eq!(
            ctx.turns.lock().unwrap().turns,
            0,
            "a warm-up must not be counted as a turn"
        );
    }
}
