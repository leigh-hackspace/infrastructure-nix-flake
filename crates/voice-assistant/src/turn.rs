//! One spoken turn, run as a **pipeline** rather than as a queue.
//!
//! ## The shape, and why it is not three function calls
//!
//! ```text
//!   audio in ──▶ rolling ASR ──▶ warm-up ──▶ agent stream ──▶ sentence splitter ──▶ piper ──▶ PCM out
//!                   │              │              │                                        │
//!                   └──────────────┴──────────────┴────────────────────────────────────────┴─▶ events
//! ```
//!
//! The buffered version of this — transcribe, then ask, then synthesise, then play — costs the
//! **sum** of its parts and the measurements are in `docs/voice-assistant-2026-11-16.md`: 2.7 s
//! of whisper, 7.7 s of model, 2.5 s of piper, ~13 s of a room standing still. Each of those
//! numbers is honest and the total is still wrong, because they are sequential only by choice.
//! Overlapped, what a person waits for is the first *audio*, which is: the tail of the ASR after
//! they stopped speaking, plus the model's first sentence, plus that sentence's synthesis — and
//! the model's second sentence and the rest of the audio are arriving while it is being said.
//!
//! So this file owns the overlapping and nothing else: [`crate::asr`] slices the audio,
//! [`crate::agent`] streams the answer, [`crate::sentence`] decides where a sentence ends,
//! [`crate::tts`] speaks it. Every one of those is independently testable and none of them knows
//! the others exist.
//!
//! ## The event stream is the product
//!
//! The caller is either a browser (which wants to play audio and show text) or the satellite
//! (which wants to feed a speaker). Both get the same events, and [`Out`] *is* the wire
//! contract: [`Out::to_json`] and [`Out::from_json`] are the only definition of it, in this file,
//! so the gateway and its client cannot drift apart. `api.rs` writes them as server-sent events;
//! `satellite.rs` reads them.
//!
//! ## Threading, and why exactly two threads
//!
//! One thread cannot both read the model's socket and write audio to the client, and it certainly
//! cannot do either while piper is working. So: this thread drives the ASR and the model stream
//! and is the **only** writer to the response; a worker thread owns the voice process and turns
//! sentences into PCM. They talk over two channels and nothing else, which is what keeps
//! `SentenceSplitter` and `Synthesiser` single-threaded — they hold state that would be a
//! correctness bug to share (a half-assembled sentence, a live child process).
//!
//! ## Two rules the pipeline enforces
//!
//! - **The first thing out is never silence.** A unit that would synthesise to nothing (a bullet
//!   of dashes, a URL alone) is dropped with a warning rather than played as a click, because a
//!   click from a ceiling speaker in a quiet room is alarming and this is the code that makes it
//!   possible.
//! - **The `emit` callback answers `false` when the caller has gone away**, and everything
//!   upstream stops: no more whisper slices, no more model tokens, no more sentences to piper. A
//!   client that hung up is not a reason to spend 15 seconds of a GPU on its behalf.

use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;


use crate::{agent, asr, sentence, spoken, stt, tts, wav};

// The event type *is* the wire contract, and it lives in `events.rs` beside its parser. Re-exported
// rather than moved at every caller: `turn::Out` is how `api.rs` and `satellite.rs` have always
// spelled it, and a contract should not be renamed by a file-size rule.
pub use crate::events::{Out, Timings, Warm};

/// Samples per audio event: 200 ms at 16 kHz, and 6.4 kB of base64 per event on the wire. Small
/// enough that playback starts on the first one; large enough that the framing cost is noise.
pub const AUDIO_CHUNK_SAMPLES: usize = 3200;

/// The "failure" that a turn produced no speech at all and no error either — a question that was
/// only a wake word, an answer with nothing speakable in it. Reported, never logged as a fault,
/// because the assistant saying nothing on a question it did not understand is the correct
/// behaviour and a caller that counts it as an error makes a quiet room look broken.
pub const NO_AUDIO: &str = "the turn produced no audio";

/// What the caller asked for, in one of the two ways anything arrives here.
pub enum Input {
    /// Already transcribed: the browser page's live transcript, or a typed question. The ASR
    /// stage is skipped entirely, which is why the page can be the fastest path in the system.
    Text(String),
    /// 16 kHz mono PCM16 WAV, transcribed in slices as this function reads it.
    Wav(Vec<u8>),
}

pub struct Options {
    /// The spoken system prompt and token budget, rather than Slack's.
    pub voice: bool,
    /// Fire the engine warm-up as soon as there is a first partial transcript.
    pub warm: bool,
    /// Rolling-ASR slice length, seconds.
    pub slice_secs: f64,
    /// The spoken-answer backstop, shared with the buffered path (`--max-speech-chars`).
    pub max_speech_chars: usize,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            voice: true,
            warm: true,
            slice_secs: 1.2,
            max_speech_chars: spoken::DEFAULT_MAX_CHARS,
        }
    }
}

/// The shared voice process, held so exactly one turn is speaking at a time.
pub type Voice = Mutex<tts::Synthesiser>;

/// Run one turn. `emit` returns `false` when the caller has gone away and the turn must stop.
///
/// Errors are reported through `Out::Error` **and** returned, because the caller (`api.rs`) needs
/// to know to log a failure while the client needs a well-formed last event.
pub fn run(
    input: Input,
    opts: &Options,
    whisper: &stt::Client,
    bot: &agent::Client,
    voice: &Voice,
    emit: &mut dyn FnMut(&Out) -> bool,
) -> Result<(), String> {
    let started = Instant::now();
    let mut timings = Timings::default();
    // Declared before the question, because stage 1 is where the warm-up can still pay.
    let (warm_tx, warm_rx) = mpsc::channel();
    let mut warm_thread: Option<std::thread::JoinHandle<()>> = None;
    let input_was_text;

    // ---------------- stage 1: the question ----------------
    let question = match input {
        Input::Text(q) => {
            input_was_text = true;
            q
        }
        Input::Wav(body) => {
            input_was_text = false;
            let file = wav::parse(&body).map_err(|e| format!("cannot read the audio: {e}"))?;
            let samples = file.mono();
            let mut rolling = asr::Rolling::from_whisper(opts.slice_secs, &whisper.url, whisper.language.clone());
            let step = (opts.slice_secs / 2.0).max(0.05) * wav::SAMPLE_RATE as f64;
            let mut first_partial: Option<String> = None;
            let chunks: Vec<&[i16]> = samples.chunks(step as usize).collect();
            for (n, chunk) in chunks.iter().enumerate() {
                match rolling.push(chunk) {
                    Ok(Some(text)) => {
                        if first_partial.is_none() {
                            first_partial = Some(text.clone());
                            // **The only moment a warm-up can pay**: there is speech left to
                            // listen to, so the engine can be doing its prompt prefill while the
                            // person still is. Nothing else is left holding slices, so the
                            // borrow here is uncontended by construction.
                            if opts.warm && warm_thread.is_none() && n + 1 < chunks.len() {
                                warm_thread = Some(fire_prefill(
                                    &bot.url,
                                    rolling.text(),
                                    warm_tx.clone(),
                                ));
                            }
                        }
                        if !emit(&Out::Asr {
                            text,
                            done: false,
                        }) {
                            return Ok(());
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        // A failed slice does not end the turn: the next slice may be fine, and
                        // `finish` still gets the whole utterance. Silence is the failure that
                        // would be reported as an answer.
                        let _ = emit(&Out::Warn(format!("slice failed: {e}")));
                    }
                }
            }
            let text = rolling
                .finish()
                .map_err(|e| format!("transcription failed: {e}"))?;
            timings.asr_ms = rolling.stats.millis;
            timings.asr_tail_ms = rolling.stats.last_ms;
            timings.asr_calls = rolling.stats.calls;
            if text.trim().is_empty() {
                let why = if rolling.stats.described > 0 {
                    "no words in that audio — whisper described it as noise".to_string()
                } else {
                    "no words in that audio".to_string()
                };
                let out = Out::Error(why.clone());
                emit(&out);
                return Err(why);
            }
            if !emit(&Out::Asr {
                text: text.clone(),
                done: true,
            }) {
                return Ok(());
            }
            first_partial.unwrap_or_else(|| text.clone())
        }
    };
    if question.trim().is_empty() {
        let out = Out::Error("nothing was asked".to_string());
        emit(&out);
        return Err("nothing was asked".to_string());
    }

    // ---------------- stage 2: the warm-up, and what it was worth ----------------
    //
    // Fired **only while there is speech left to hear**, because that is the only case in which
    // it is not a competing request: measured on services1 2026-11-22, a warm-up of this fleet's
    // 4 079-token prefix cost **4 629 ms** of the 3060 and reported `cached 0`, so the same
    // prefill fired the instant the question was final simply made the engine do the work twice
    // and — until the listen socket learned to wait for a warm-up's lock instead of refusing —
    // made it fail the turn it was warming. A turn whose question is already text has no time
    // left to buy, and says so in `Timings::warm` rather than pretending it tried.
    let warm_skipped = warm_skip_reason(opts.warm, input_was_text, warm_thread.is_some());

    // ---------------- stages 3 and 4: the answer and the voice, overlapped ----------------
    let mut pipe = Pipe {
        split: sentence::Splitter::new(),
        timings,
        started,
        seq: 0,
        spoken_left: opts.max_speech_chars,
        max_chars: opts.max_speech_chars,
    };
    let mut answer: Option<agent::Answer> = None;
    let mut ask_error: Option<String> = None;

    {
        let mut guard = voice.lock().unwrap_or_else(|p| p.into_inner());
        std::thread::scope(|scope| {
            let (say_tx, say_rx) = mpsc::channel::<String>();
            let (pcm_tx, pcm_rx) = mpsc::channel::<Pcm>();
            let synthesiser: &mut tts::Synthesiser = &mut guard;
            scope.spawn(move || {
                for sentence in say_rx.iter() {
                    match synthesiser.say(&sentence) {
                        Ok(speech) => {
                            for chunk in tts::pcm_at_16k(&speech).chunks(AUDIO_CHUNK_SAMPLES) {
                                if pcm_tx
                                    .send(Pcm {
                                        pcm: chunk.to_vec(),
                                        failed: String::new(),
                                    })
                                    .is_err()
                                {
                                    return; // the turn is over; stop spending the box's time
                                }
                            }
                        }
                        // One sentence failing must not silence the rest of the answer, and the
                        // room must not be told the answer arrived when part of it is missing.
                        Err(e) => {
                            eprintln!("tts failed for {sentence:?}: {e}");
                            if pcm_tx
                                .send(Pcm {
                                    pcm: Vec::new(),
                                    failed: e,
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                }
            });

            // The model stream, event by event. After **every** event the voice queue is drained,
            // which is the entire mechanism by which audio reaches the room while the model is
            // still answering: the two are never waiting on each other.
            match bot.ask_stream(&question, opts.voice, &mut |ev| -> bool {
                let out = match ev {
                    // The tool list gets one line, because it is the first question about a slow turn: a
            // reply that took ten seconds is usually calling something, and "which tools did it
            // even have" narrows it to the right half of the pipeline.
            agent::Event::Start { tools } => Some(Out::Log(format!(
                "{} tools: {}",
                tools.len(),
                if tools.is_empty() {
                    "none".to_string()
                } else {
                    tools.join(", ")
                }
            ))),
                    agent::Event::Log(line) => Some(Out::Log(line)),
                    agent::Event::Reset => Some(Out::Reset),
                    agent::Event::Partial(t) => {
                        pipe.on_token(&t, &say_tx);
                        Some(Out::Token(t))
                    }
                };
                if let Some(o) = out {
                    if !pipe.emit(o, &mut |o| emit(o)) {
                        // The client is gone: stop reading the model's socket and stop feeding
                        // the voice, rather than finishing a turn for nobody.
                        return false;
                    }
                }
                pipe.drain(&pcm_rx, &mut |o| emit(o))
            }) {
                Ok(a) => answer = Some(a),
                Err(e) => ask_error = Some(e),
            }
            // **The fast path has no tokens, and the room still has to hear it.** A
            // deterministic answer (`space_status`, the clock, a temperature) arrives as one
            // `answer` event with `first_token_ms == 0`: nothing streamed, so the splitter was
            // never fed, and a pipeline that speaks only what it streamed would answer the most
            // common question in the room with a **silent** 200. Measured on the deployed
            // gateway: `answer` at 8 ms, `0 sentence(s)`, no audio. So whatever never arrived
            // part by part is handed to the same splitter now — same `spoken::speakable`
            // budget, same worker, same rules — and `sentences == 0` after this point means the
            // answer genuinely had nothing speakable in it.
            if pipe.timings.parts == 0 {
                if let Some(a) = answer.as_ref() {
                    if !a.text.trim().is_empty() {
                        let text = a.text.clone();
                        pipe.on_token(&text, &say_tx);
                    }
                }
            }
            // The answer is done. Anything the splitter is still holding is the **end of the
            // answer** and goes through the same worker as every other sentence: the voice thread
            // owns the process, so speaking here as well would be a second writer to it.
            for unit in pipe.split.flush() {
                if let Some(say) = spoken::speakable(&unit, pipe.spoken_left) {
                    pipe.spoken_left = pipe.spoken_left.saturating_sub(say.chars().count());
                    pipe.timings.sentences += 1;
                    let _ = say_tx.send(say);
                }
            }
            // Closing the sender is what tells the voice thread it has heard the last sentence;
            // then the queue is drained until it really has, because the voice can legitimately
            // be two sentences behind a model that finished.
            drop(say_tx);
            while let Ok(chunk) = pcm_rx.recv() {
                if !pipe.emit_pcm(chunk, &mut |o| emit(o)) {
                    break;
                }
            }
        });
    }

    let mut timings = pipe.timings;
    // The warm-up is allowed to outlive the turn it warmed: it is a favour, not a dependency. Take
    // the result if it has landed, and say honestly that it had not if it has not — then **drop**
    // the handle rather than join it, because joining here would make the answer wait for a
    // request whose whole purpose was to not be waited for.
    timings.warm = if warm_thread.is_some() {
        Some(match warm_rx.recv_timeout(Duration::from_millis(25)) {
            Ok(w) => w,
            Err(_) => Warm {
                ok: false,
                detail: "still running when the answer finished".to_string(),
                ..Default::default()
            },
        })
    } else {
        warm_skipped
    };
    drop(warm_thread);
    if let Some(a) = answer {
        timings.engine = a.engine.clone();
        timings.rounds = a.rounds;
        timings.total_ms = started.elapsed().as_millis();
        let _ = emit(&Out::Answer {
            text: a.text.clone(),
            engine: a.engine,
            rounds: a.rounds,
            tools: a.tools,
        });
        let _ = emit(&Out::Done(timings));
        return Ok(());
    }
    let why = ask_error.unwrap_or_else(|| "the turn ended without an answer".to_string());
    timings.total_ms = started.elapsed().as_millis();
    if why == agent::CLIENT_GONE {
        // Nobody is left to hear it. Report success — the turn ended for the right reason — and
        // write nothing more, because a cancelled turn logged as an error makes a page the user
        // closed look like an outage. Every upstream stage (whisper slices, model tokens, piper
        // sentences) already stopped on the same signal.
        return Ok(());
    }
    let _ = emit(&Out::Error(why.clone()));
    Err(why)
}

/// Whether the warm-up was not fired, and what to say about it in `Timings::warm`.
///
/// Three states, and the third is the one that reached production: a warm-up fired **when the
/// question is already final** is not a head start, it is a second request for the same
/// prompt-prefix work on a single engine, and until the listen socket learned to wait for a
/// warm-up's lock it made the turn lose with `another turn is in flight`. So the rule is:
/// warm only while there is still speech to hear. `None` means it fired, and only
/// `None`-with-a-thread does; an off flag is reported as absent ("never fired") rather than
/// as a skip, because the operator who turned it off does not want to read about it per turn.
fn warm_skip_reason(warm: bool, question_was_final: bool, fired: bool) -> Option<Warm> {
    if fired {
        return None;
    }
    if !warm {
        return None;
    }
    Some(Warm {
        ok: false,
        detail: if question_was_final {
            "skipped: the question was already final, so a warm-up would only compete".to_string()
        } else {
            "skipped: no partial transcript arrived while speech was still coming".to_string()
        },
        ..Default::default()
    })
}

/// Send the engine its prompt prefix and go away. Returns the handle so the caller can *drop*
/// it: a warm-up that could delay the turn it warms would be a bug rather than an optimisation,
/// so nothing here joins, waits on the result, or treats a failure as the turn's.
fn fire_prefill(
    url: &str,
    partial: &str,
    tx: mpsc::Sender<Warm>,
) -> std::thread::JoinHandle<()> {
    let client = agent::Client::new(url);
    let q = partial.to_string();
    std::thread::spawn(move || {
        let _ = tx.send(match client.prefill(&q, true) {
            Ok(p) => Warm {
                ok: p.ok,
                prompt_tokens: p.prompt_tokens,
                cached_tokens: p.cached_tokens,
                millis: p.millis,
                detail: if p.skipped.is_empty() {
                    p.engine
                } else {
                    p.skipped
                },
            },
            Err(e) => Warm {
                ok: false,
                detail: e,
                ..Default::default()
            },
        });
    })
}

/// What the voice thread hands back: samples, or the reason there are none.
/// What the voice thread hands back. Named `Pcm` rather than the acronym shouting, because
/// clippy is right and the type is private anyway.
struct Pcm {
    pcm: Vec<i16>,
    failed: String,
}

/// The turn's own state, kept in one place so the two channels' bookkeeping (what has been
/// spoken, what has been sent, what has been paid for) cannot be updated in one and not the
/// other.
struct Pipe {
    split: sentence::Splitter,
    timings: Timings,
    started: Instant,
    seq: u32,
    spoken_left: usize,
    max_chars: usize,
}

impl Pipe {
    fn on_token(&mut self, t: &str, say_tx: &mpsc::Sender<String>) {
        if self.timings.first_token_ms == 0 {
            self.timings.first_token_ms = self.started.elapsed().as_millis();
        }
        self.timings.parts += 1;
        for unit in self.split.push(t) {
            let Some(say) = spoken::speakable(&unit, self.spoken_left) else {
                // Rule: never synthesise nothing. A skipped unit is a warning, not a click.
                self.timings
                    .warnings
                    .push(format!("nothing speakable in {unit:?} — skipped"));
                continue;
            };
            if self.spoken_left == 0 {
                self.timings.warnings.push(format!(
                    "stopped speaking after {} characters (--max-speech-chars)",
                    self.max_chars
                ));
                break;
            }
            self.spoken_left = self.spoken_left.saturating_sub(say.chars().count());
            self.timings.sentences += 1;
            if say_tx.send(say).is_err() {
                break;
            }
        }
    }

    /// Send anything the voice has already produced. Returns whether the caller is still there.
    fn drain(&mut self, pcm_rx: &mpsc::Receiver<Pcm>, emit: &mut dyn FnMut(&Out) -> bool) -> bool {
        let mut alive = true;
        while let Ok(chunk) = pcm_rx.try_recv() {
            alive = self.emit_pcm(chunk, emit);
            if !alive {
                break;
            }
        }
        alive
    }

    fn emit_pcm(&mut self, chunk: Pcm, emit: &mut dyn FnMut(&Out) -> bool) -> bool {
        if !chunk.failed.is_empty() {
            self.timings.warnings.push(format!("no audio for one sentence: {}", chunk.failed));
            return emit(&Out::Warn(format!("no audio: {}", chunk.failed)));
        }
        if chunk.pcm.is_empty() {
            return true;
        }
        if self.timings.first_audio_ms == 0 {
            self.timings.first_audio_ms = self.started.elapsed().as_millis();
        }
        self.timings.audio_secs += chunk.pcm.len() as f64 / wav::SAMPLE_RATE as f64;
        let alive = emit(&Out::Audio {
            seq: self.seq,
            pcm: chunk.pcm,
        });
        self.seq += 1;
        alive
    }

    fn emit(&mut self, out: Out, emit: &mut dyn FnMut(&Out) -> bool) -> bool {
        if let Out::Warn(w) = &out {
            self.timings.warnings.push(w.clone());
        }
        emit(&out)
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit;
    use common_json as json;

    /// What the pipeline emitted, with the moment each event arrived. The timestamps are the
    /// point of the type: an ordering claim ("the room heard the first sentence before the model
    /// had finished the last") is only checkable if the test watched the clock.
    #[derive(Debug)]
    struct Seen {
        names: Vec<String>,
        at: Vec<u128>,
        audio: usize,
        tokens: String,
        warns: Vec<String>,
    }

    impl Seen {
        fn watch(&mut self, started: Instant, out: &Out) {
            let name = out.name().to_string();
            self.names.push(name);
            self.at.push(started.elapsed().as_millis());
            match out {
                Out::Audio { .. } => self.audio += 1,
                Out::Token(t) => self.tokens.push_str(t),
                Out::Warn(w) => self.warns.push(w.clone()),
                _ => {}
            }
        }

        fn first(&self, name: &str) -> Option<usize> {
            self.names.iter().position(|n| n == name)
        }

        fn ms(&self, name: &str) -> u128 {
            self.first(name).map(|i| self.at[i]).unwrap_or(u128::MAX)
        }
    }

    fn voice(dir: &std::path::Path, die_after: usize, pause_ms: u64) -> Voice {
        let model = testkit::voice_model(dir);
        let bin = testkit::fake_piper(dir, die_after, pause_ms);
        Mutex::new(tts::Synthesiser::new(
            bin.to_str().unwrap(),
            model.to_str().unwrap(),
            1.0,
            &dir.join("frames"),
        ))
    }

    fn agent(events: Vec<testkit::SseEvent>, connections: usize) -> (agent::Client, std::thread::JoinHandle<()>) {
        let (url, handle) = testkit::fake_agent(events, connections);
        (agent::Client::new(&url), handle)
    }

    fn answer_event(text: &str) -> testkit::SseEvent {
        (
            "answer",
            format!(
                "{{\"answer\":{},\"engine\":\"fake (bench GPU)\",\"rounds\":1,\"tools\":[],\"secs\":0.5}}",
                json::quoted(text)
            ),
            0,
        )
    }

    fn partial(t: &str) -> testkit::SseEvent {
        ("partial", format!("{{\"t\":{}}}", json::quoted(t)), 0)
    }

    /// **The claim of the whole file:** the room hears the first sentence while the model is
    /// still producing the rest of it.
    ///
    /// The fixture is what makes this falsifiable rather than tautological. The model is slow on
    /// purpose — a partial every 400 ms, like a decode at the measured 14–39 tok/s — and piper
    /// takes 300 ms a sentence, so a real overlap exists to be measured. The assertion is against
    /// the **last token**, not against the `answer` event: `Out::Answer` is deliberately emitted
    /// only once the voice queue has drained (a turn that said "here is my answer" while still
    /// speaking it would be a lie), so an assertion against it would pass even for a pipeline that
    /// buffered everything — which is exactly the regression this test exists to catch, and what
    /// an earlier version of it did.
    #[test]
    fn the_first_audio_arrives_while_the_model_is_still_answering() {
        let dir = testkit::tmp("overlap");
        // The first partial ends a sentence; the two that follow do not, until they do.
        let mut late = answer_event("The space is open. It is nineteen degrees in the main room.");
        late.2 = 100;
        let (bot, thread) = agent(
            vec![
                ("start", r#"{"tools":["space_status"]}"#.to_string(), 0),
                ("partial", r#"{"t":"The space is open. "}"#.to_string(), 0),
                ("partial", r#"{"t":"It is nineteen degrees "}"#.to_string(), 400),
                ("partial", r#"{"t":"in the main room."}"#.to_string(), 400),
                late,
            ],
            1,
        );
        let stt = stt::Client::new("http://127.0.0.1:1", None);
        let mut seen = Seen {
            names: vec![],
            at: vec![],
            audio: 0,
            tokens: String::new(),
            warns: vec![],
        };
        let started = Instant::now();
        let opts = Options {
            warm: false,
            ..Default::default()
        };
        run(
            Input::Text("is the space open".into()),
            &opts,
            &stt,
            &bot,
            // 300 ms a sentence: slow enough that the model is still talking when the first one
            // is ready, fast enough that the whole turn fits in a test's patience.
            &voice(&dir, 9, 300),
            &mut |o| {
                seen.watch(started, o);
                true
            },
        )
        .unwrap();
        thread.join().unwrap();

        assert!(seen.audio > 0, "no audio at all: {:?}", seen.names);
        assert_eq!(
            seen.tokens,
            "The space is open. It is nineteen degrees in the main room."
        );
        let audio = seen.ms("audio");
        let last_token = seen
            .names
            .iter()
            .rposition(|n| n == "token")
            .expect("no tokens reached the caller");
        let last_token_ms = seen.at[last_token];
        assert!(
            audio < last_token_ms,
            "first audio at {audio} ms, last token at {last_token_ms} ms — nothing was \
             spoken until the model had finished, which is the buffered pipeline this replaced"
        );
        // And the summary the operator reads has to carry both numbers, or the claim cannot be
        // checked from the log the next time someone doubts it.
        assert!(
            seen.ms("answer") > audio,
            "the answer event preceded the audio that belongs to it: {:?}",
            seen.names
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The rule that reached production as an outage, in both directions.
    ///
    /// A warm-up fired **when the question is already final** costs the same prompt-prefix work
    /// a second time on a single engine (measured on services1 2026-11-22: 4 629 ms, `cached
    /// 0/4079`) and, before the listen socket learned to wait for a warm-up's lock, made the
    /// turn lose with `another turn is in flight`. So the only legitimate moment is while speech
    /// is still arriving — and the report says which of the two happened, because "we did not
    /// warm up" and "warming up would have made it worse" are different sentences.
    #[test]
    fn a_warm_up_is_only_fired_while_there_is_speech_left_to_hear() {
        let detail = |w: Option<Warm>| w.map(|x| x.detail).unwrap_or_default();
        assert!(
            warm_skip_reason(false, true, false).is_none(),
            "an off flag is 'never fired', not a skip the operator must read every turn"
        );
        assert!(
            warm_skip_reason(true, false, true).is_none(),
            "a warm-up that fired must never be reported as skipped"
        );
        assert!(
            detail(warm_skip_reason(true, true, false)).contains("already final"),
            "a text turn must report that warming up now would only compete"
        );
        assert!(
            detail(warm_skip_reason(true, false, false)).contains("speech was still coming"),
            "an audio turn with no early partial must say so"
        );
    }

    /// End to end for the same rule: a `/v1/turn` asked with text reports the skip in its own
    /// timings, rather than firing a prefill at the moment it is about to ask.
    #[test]
    fn a_turn_that_arrived_as_text_does_not_warm_the_engine() {
        let dir = testkit::tmp("nowarm");
        // One connection, the real ask: a warm-up fired here would open a second one, and the
        // warm-up's own report below would then say "still running" instead of the skip.
        let (bot, _thread) = agent(
            vec![
                ("start", r#"{"tools":[]}"#.to_string(), 0),
                partial("It is open."),
                answer_event("It is open."),
            ],
            1,
        );
        let stt = stt::Client::new("http://127.0.0.1:1", None);
        let opts = Options {
            warm: true,
            ..Default::default()
        };
        let mut warm: Option<Warm> = None;
        run(
            Input::Text("is the space open".into()),
            &opts,
            &stt,
            &bot,
            &voice(&dir, 9, 0),
            &mut |o| {
                if let Out::Done(t) = o {
                    warm = t.warm.clone();
                }
                true
            },
        )
        .unwrap();
        let w = warm.expect("a text turn still reports what the warm-up did not do");
        assert!(!w.ok);
        assert!(
            w.detail.contains("already final"),
            "warm-up report said {:?} — expected the skip, not an attempt",
            w.detail
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A deterministic answer arrives as one `answer` event with nothing streamed, and the room
    /// must still hear it: an assistant that answers "is the space open" by silently returning a
    /// well-formed 200 is broken in the way nobody notices until someone says it out loud.
    #[test]
    fn a_fast_path_answer_with_no_streamed_text_is_still_spoken() {
        let dir = testkit::tmp("fastpath");
        // Exactly what `slack-agent` sends when the recogniser answered: `start`, then `answer`.
        let (bot, _thread) = agent(
            vec![
                ("start", r#"{"tools":["space_status"]}"#.to_string(), 0),
                answer_event("Leigh Hackspace is CLOSED, since 17:44 UTC."),
            ],
            1,
        );
        let stt = stt::Client::new("http://127.0.0.1:1", None);
        let opts = Options {
            warm: false,
            ..Default::default()
        };
        let mut seen = Seen {
            names: vec![],
            at: vec![],
            audio: 0,
            tokens: String::new(),
            warns: vec![],
        };
        let mut sentences = 0;
        let started = Instant::now();
        run(
            Input::Text("is the space open".into()),
            &opts,
            &stt,
            &bot,
            &voice(&dir, 9, 0),
            &mut |o| {
                seen.watch(started, o);
                if let Out::Done(t) = o {
                    sentences = t.sentences;
                }
                true
            },
        )
        .unwrap();
        assert_eq!(
            seen.tokens, "",
            "the fast path must not invent streamed tokens"
        );
        assert!(
            seen.audio > 0 && sentences > 0,
            "silent fast path: {} audio chunk(s), {} sentence(s) — {:?}",
            seen.audio,
            sentences,
            seen.names
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A client that closes the socket is not an outage. Everything upstream must stop, and the
    /// turn must not come back as an error.
    #[test]
    fn a_caller_that_hangs_up_ends_the_turn_without_an_error() {
        let dir = testkit::tmp("hangup");
        let (bot, _thread) = agent(
            vec![
                ("start", r#"{"tools":[]}"#.to_string(), 0),
                partial("The space is open."),
                answer_event("The space is open."),
            ],
            1,
        );
        let stt = stt::Client::new("http://127.0.0.1:1", None);
        let opts = Options {
            warm: false,
            ..Default::default()
        };
        let mut seen = 0;
        let result = run(
            Input::Text("is it open".into()),
            &opts,
            &stt,
            &bot,
            &voice(&dir, 9, 0),
            &mut |_o| {
                seen += 1;
                // Leave after the first event: the callback is the caller, and the caller has gone.
                seen < 1
            },
        );
        assert!(
            result.is_ok(),
            "a cancelled turn was reported as a failure: {result:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The ASR path, and the number that justifies rolling transcription: what the speaker waits
    /// for is the **tail** slice, not the sum of all of them.
    #[test]
    fn the_room_pays_for_the_last_slice_not_the_whole_transcription() {
        let dir = testkit::tmp("tail");
        // 250 ms a call, three calls: `asr_ms` will be ~750 and `asr_tail_ms` ~250.
        let (url, whisper) = testkit::fake_whisper("is the space open", 250, 8);
        let (bot, agent_thread) = agent(vec![("start", r#"{"tools":[]}"#.to_string(), 0), answer_event("Yes, it is open.")], 1);
        let stt = stt::Client::new(&url, None);
        // Four seconds of audio, one-second slices: the slices overlap, so the count is a floor
        // on calls rather than an exact expectation, and the assertion is about the ratio.
        let samples = (4 * wav::SAMPLE_RATE) as usize;
        let pcm: Vec<i16> = (0..samples).map(|i| ((i / 32) as i16) & 0x3f).collect();
        let body = wav::mono16(&pcm, wav::SAMPLE_RATE);
        let mut timings: Option<Timings> = None;
        let opts = Options {
            warm: false,
            slice_secs: 1.0,
            ..Default::default()
        };
        let out = run(
            Input::Wav(body),
            &opts,
            &stt,
            &bot,
            &voice(&dir, 9, 0),
            &mut |o| {
                if let Out::Done(t) = o {
                    timings = Some(t.clone());
                }
                true
            },
        );
        let _ = whisper;
        let _ = agent_thread;
        out.expect("the turn should answer");
        let t = timings.expect("a Done event with timings");
        assert!(t.asr_calls >= 3, "only {} slices for 4 s of audio", t.asr_calls);
        assert!(
            t.asr_tail_ms * 2 < t.asr_ms,
            "tail {} ms is not less than the whole {} ms — nothing overlapped",
            t.asr_tail_ms,
            t.asr_ms
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Audio that contains no words must be answered with a reason, not with silence: the room
    /// cannot tell "nothing was said" from "the assistant is broken", and only this code knows
    /// which one happened.
    #[test]
    fn audio_with_no_words_in_it_says_so_instead_of_answering() {
        let dir = testkit::tmp("nowords");
        let (url, _whisper) = testkit::fake_whisper("", 0, 8);
        let (bot, _agent) = agent(vec![], 0);
        let stt = stt::Client::new(&url, None);
        let pcm: Vec<i16> = vec![0; (2 * wav::SAMPLE_RATE) as usize];
        let body = wav::mono16(&pcm, wav::SAMPLE_RATE);
        let opts = Options {
            warm: false,
            slice_secs: 1.0,
            ..Default::default()
        };
        let mut names = vec![];
        let result = run(
            Input::Wav(body),
            &opts,
            &stt,
            &bot,
            &voice(&dir, 9, 0),
            &mut |o| {
                names.push(o.name().to_string());
                true
            },
        );
        assert!(result.is_err());
        assert!(
            names.contains(&"error".to_string()),
            "no `error` event, only {names:?}"
        );
        assert!(!names.contains(&"answer".to_string()), "{names:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
