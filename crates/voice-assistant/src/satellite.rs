//! The room end: a microphone in, a spoken answer out.
//!
//! ## What this process actually is
//!
//! A pipe with judgement in the middle.  Something produces a continuous stream of
//! 16 kHz mono PCM16 ([`crate::source`]), [`crate::gate`] cuts utterances out of it,
//! the gateway transcribes and answers, and the answer goes to something that can make
//! noise ([`crate::sink`]).  Nothing here holds state that matters: restart it and the
//! only loss is the counters in `status.json`.
//!
//! ## What sits behind the two arrows
//!
//! The microphone and the loudspeaker are **traits** (`crate::source`, `crate::sink`), and every
//! device noun — the address of the restream, the tool that decodes it, the shell command that
//! makes noise — lives in `crate::drivers`. So this file is about *a satellite*: cut utterances
//! out of a stream, decide whether one was addressed to us, ask, and then pay for what it cost to
//! make a noise. Which camera, which browser tab or which microcontroller it came from is a new
//! module in `drivers/` plus a row in the unit, never an edit here
//! (`common/check-platform-boundary.nix` is the gate that keeps that true, and
//! `docs/esp32-voice-satellites.md` is the contract for the next platform).
//!
//! ## Half-duplex, because nothing here cancels echo
//!
//! The deployed sink is a device whose own status says it has no echo canceller, so its speaker
//! feeds its microphone, and a naive loop has the assistant answering itself forever. The rule
//! this implements is the one the browser demo already uses for its confirmation ping
//! (`crates/whisper-ws/README.md`: the mic is muted for 350 ms after the ping), taken to its
//! conclusion: **while an answer is playing, the captured audio is thrown away**, the gate is
//! reset, and the silence after the answer is played is added on top. It is one piece of
//! arithmetic — [`capture_to_discard`] — driven by what the *sink reports about itself*, not by a
//! hardcoded belief about a device, so a platform that cancels its own output can keep listening
//! while it talks. That is what barge-in needs, and it is the thing `docs/follow-ups.md` has been
//! blocked on. There is no barge-in today; the receipts, including the measurement that says why,
//! are in `docs/voice-assistant-2026-11-16.md`.
//!
//! ## The speaker is off until you say otherwise
//!
//! `--speaker off` is the default and it is not shyness: answers are logged, counted and written
//! to the state directory, and *nothing reaches the room* (`crate::drivers::play_nothing`). The
//! two things that must be true before that flips — someone having heard it, and a key a
//! `DynamicUser` may legitimately read — are argued in
//! `machines/services1/services/voice-assistant.nix`, which owns that decision.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};


use crate::gate;
use crate::gateway::{ask_turn, transcribe_slice, transcribe_whole, warm, Live};
use crate::sink::{Capabilities, Sink};
use crate::source::SourceFactory;
use crate::wav;

pub struct Options {
    /// How to begin a stream; the loop below owns the *restarting* and a driver only says how to
    /// begin (`crate::source`).
    pub make_source: SourceFactory,
    /// For the log line and `status.json`'s `source` key.  An opaque label: nothing here parses
    /// it, which is what lets a driver put whatever a human needs in it.
    pub source_label: String,
    pub gateway: String,
    pub wake_words: Vec<String>,
    /// How far into a transcript a wake phrase may start (see `crate::wake`).
    pub search_tokens: usize,
    pub gate: gate::Config,
    /// Where the answer goes.  Held here rather than fetched per turn because the loop hands it
    /// out mutably across stream restarts; the *choice* of sink is made by
    /// `crate::drivers::open_sink` from the operator's `--speaker` word.
    pub sink: Box<dyn Sink>,
    /// `sink.label()`, kept as a string so the status writer does not need the sink itself.
    pub sink_label: String,
    pub status_path: PathBuf,
    pub out_dir: PathBuf,
    /// Transcribe and decide, but never ask and never answer: the way to measure
    /// what this thing would say in a room full of people before it says any of it.
    pub probe_only: bool,
    /// Bounded capture, for `probe`: stop after this many seconds of **audio** (0 =
    /// listen until someone presses Ctrl-C).  The unit never sets it; a human typing
    /// `just voice-probe 8` does, because a probe that has to be interrupted is a probe
    /// nobody runs twice.
    pub capture_secs: f64,
    /// `probe --out`: leave one WAV behind for a human to listen to.  The clip the gate
    /// actually sent to whisper if there was one; if the gate never opened, **the whole
    /// capture**, because "why did it ignore me" is answered by hearing the room, and a
    /// probe that produces nothing on a quiet run tells you nothing about why.
    pub out_file: Option<PathBuf>,
    /// How long to wait for the request after a bare "hey jarvis".
    pub follow_up: Duration,
    /// Extra audio thrown away after an answer starts, on top of the playing time.
    pub guard: Duration,
    /// Refuse to say anything longer than this (the answer budget is enforced in
    /// `spoken.rs`; this is the belt, for a `--max-speech-chars` set far too high).
    pub max_play_secs: f64,
    /// Transcribe an open window **as it fills** rather than after it closes, and warm the engine
    /// with the first partial. Off is the old behaviour: one whisper call for the whole window,
    /// after the speaker has stopped and while they wait.
    pub warm: bool,
    /// Seconds of audio per transcription slice. Shorter means less left to transcribe when
    /// someone stops talking, and more whisper calls per sentence.
    pub asr_slice_secs: f64,
}

/// The half-duplex rule, as one piece of arithmetic a room's behaviour depends on.
///
/// How many seconds of capture to throw away after an answer.  Three cases, and the third is why
/// this is a function with tests rather than a line in a loop:
///
/// * nothing was played → nothing to ignore, so a dry run must not deafen the room;
/// * the sink cancels its own output → keep listening, which is what barge-in is;
/// * otherwise → the answer's own length, plus `guard` for the room noticing that it spoke.
///
/// It takes what the sink **reported** rather than the length of the buffer we handed it: an
/// inferred playing time is a guess, and this number is the only thing standing between a speaker
/// and an assistant that answers itself.
pub fn capture_to_discard(played_secs: f64, cap: &Capabilities, guard_secs: f64) -> f64 {
    if played_secs <= 0.0 || cap.echo_cancelled {
        0.0
    } else {
        played_secs + guard_secs
    }
}

/// What to do with one transcript.  Split out from all the I/O so the part with
/// judgement in it can be argued with in a test.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Not for us, or not speech.  The string is the reason, for the log.
    Ignore(&'static str),
    /// Ask this.
    Ask(String),
    /// They woke us and said nothing else: the next window is the request.
    Await,
}

/// The conversation state, which is one boolean.
pub struct Session {
    pub wake_words: Vec<String>,
    pub search_tokens: usize,
    awaiting: bool,
    pub awaiting_until: Option<Instant>,
    pub follow_up: Duration,
}

impl Session {
    pub fn new(wake_words: Vec<String>, search_tokens: usize, follow_up: Duration) -> Session {
        Session {
            wake_words,
            search_tokens,
            awaiting: false,
            awaiting_until: None,
            follow_up,
        }
    }

    pub fn on_transcript(&mut self, text: &str, is_description: bool) -> Action {
        if is_description {
            return Action::Ignore("whisper described the noise instead of words");
        }
        if text.trim().is_empty() {
            return Action::Ignore("empty transcript");
        }
        if self.awaiting {
            let expired = self
                .awaiting_until
                .map(|t| Instant::now() > t)
                .unwrap_or(false);
            self.awaiting = false;
            self.awaiting_until = None;
            if expired {
                return Action::Ignore("woke us, then said nothing in time");
            }
            return Action::Ask(text.trim().to_string());
        }
        match crate::wake::find(text, &self.wake_words, self.search_tokens) {
            Some(hit) if hit.request.trim().is_empty() => {
                self.awaiting = true;
                self.awaiting_until = Some(Instant::now() + self.follow_up);
                Action::Await
            }
            Some(hit) => Action::Ask(hit.request),
            None => Action::Ignore("no wake word"),
        }
    }
}

/// The counters `status.json` carries, and the gateway shows.
/// No `derive(Default)`: `peak_dbfs` has to start at the bottom of the scale, and a
/// derived 0.0 there is a measurement that claims full scale.  See `Default for Stats`.
pub struct Stats {
    pub windows: usize,
    pub ignored: usize,
    pub asked: usize,
    pub answers: usize,
    pub spoken: usize,
    /// Answers that were produced and deliberately not played (`--speaker off`).
    pub unplayed: usize,
    /// How many times the ingest had to be reopened.  Named for the event, not for one tool: a
    /// dropped WebSocket and a dead child process mean the same thing to whoever is reading it.
    pub ingest_restarts: usize,
    pub last_error: String,
    pub last_transcript: String,
    pub last_action: String,
    pub floor_dbfs: f64,
    /// Seconds of audio actually read, which is what `--seconds` bounds.
    pub captured_secs: f64,
    /// Loudest frame seen, dBFS — with `floor_dbfs` and `threshold_dbfs`, the three
    /// numbers that say whether a silent probe was a quiet room or a gate set above it.
    /// Starts at the bottom of the scale, not at 0.0: `Default` would make an untouched
    /// field read "0 dBFS", which is full scale, which reads as "something clipped" — and
    /// that is exactly the lie the first live probe told in a quiet room, because the
    /// field only ever moved inside a completed window.
    pub peak_dbfs: f64,
    /// What a frame had to beat at the end of the run.
    pub threshold_dbfs: f64,
    /// Frames the gate thought were speech, and the windows they did (or did not) make.
    pub speech_frames: usize,
    /// Transcripts that came back with words in them (vs a `(soft music)` description).
    pub transcripts: usize,
    /// Transcription slices the rolling path sent (0 = it never opened, or is switched off).
    pub asr_slices: usize,
    /// The last slice's wall clock: what the satellite is still waiting for when someone stops
    /// talking. With `asr_slices`, this is the whole case for rolling transcription in one pair
    /// of numbers.
    pub asr_tail_ms: u128,
    /// Milliseconds from the end of an utterance to the first audio of the answer, last turn.
    pub first_audio_ms: u128,
    /// Warm-up requests this satellite fired.
    pub warms: usize,
}

impl Default for Stats {
    /// Hand-written, not derived, for one field: `peak_dbfs` must start at the floor of
    /// the scale. See its comment.
    fn default() -> Self {
        Self {
            windows: 0,
            ignored: 0,
            asked: 0,
            answers: 0,
            spoken: 0,
            unplayed: 0,
            ingest_restarts: 0,
            last_error: String::new(),
            last_transcript: String::new(),
            last_action: String::new(),
            floor_dbfs: 0.0,
            captured_secs: 0.0,
            peak_dbfs: -120.0,
            threshold_dbfs: 0.0,
            speech_frames: 0,
            transcripts: 0,
            asr_slices: 0,
            asr_tail_ms: 0,
            first_audio_ms: 0,
            warms: 0,
        }
    }
}

impl Stats {
    /// A probe that never opened the gate.
    pub fn windows_of_nothing(&self) -> bool {
        self.windows == 0
    }
}

pub fn run(mut opts: Options) -> Result<(), String> {
    let mut session = Session::new(
        opts.wake_words.clone(),
        opts.search_tokens,
        opts.follow_up,
    );
    let mut stats = Stats::default();
    let started = Instant::now();
    std::fs::create_dir_all(&opts.out_dir)
        .map_err(|e| format!("cannot create {}: {e}", opts.out_dir.display()))?;
    write_status(&opts, &stats, started);
    eprintln!(
        "voice-assistant satellite: source={} [{}] wake={:?} speaker={} ({})",
        opts.source_label,
        crate::source::FORMAT,
        opts.wake_words,
        opts.sink_label,
        opts.sink.details()
    );
    if !opts.sink.capabilities().audible {
        eprintln!("speaker is off: answers are logged and written to {} only", opts.out_dir.display());
    }

    loop {
        match one_stream(&mut opts, &mut session, &mut stats, started) {
            // The bounded probe: the cap is reached, so report and stop.  Everything
            // else is a stream that died and a satellite that gets another one.
            Ok(true) => {
                write_status(&opts, &stats, started);
                eprintln!(
                    "probe: {:.0} s of audio, {} gate window(s), {} transcript(s), {} ignored as not-speech",
                    stats.captured_secs, stats.windows, stats.transcripts, stats.ignored
                );
                if let Some(path) = &opts.out_file {
                    eprintln!("clip: {}", path.display());
                }
                if stats.windows_of_nothing() {
                    // Two different failures, and the fix is the opposite for each, so
                    // say which one the numbers show.  (Getting the sign wrong here —
                    // "−8 dB of gap" — is how a quiet room starts looking like a loud one.)
                    let gap = stats.threshold_dbfs - stats.peak_dbfs;
                    if gap > 0.0 {
                        eprintln!(
                            "the gate never opened: loudest frame {:.0} dBFS is {:.0} dB below \
                             the threshold of {:.0} (floor {:.0}). That is a quiet room. If it \
                             was not quiet, lower --min-open-dbfs or --margin-db, and listen to \
                             the clip.",
                            stats.peak_dbfs, gap, stats.threshold_dbfs, stats.floor_dbfs
                        );
                    } else {
                        eprintln!(
                            "the gate never opened although the loudest frame ({:.0} dBFS) beat \
                             the threshold ({:.0} dBFS) by {:.0} dB: {} speech-like frame(s) \
                             never made a window. That is the shape tests or the run of \
                             --open-frames, not the level — run `gate-check` on the clip and \
                             read the column that says no.",
                            stats.peak_dbfs,
                            stats.threshold_dbfs,
                            -gap,
                            stats.speech_frames
                        );
                    }
                }
                return Ok(());
            }
            Ok(false) => eprintln!("stream ended; restarting in 5 s"),
            Err(e) => {
                stats.last_error = e.clone();
                eprintln!("stream failed: {e}");
            }
        }
        stats.ingest_restarts += 1;
        write_status(&opts, &stats, started);
        std::thread::sleep(Duration::from_secs(5));
    }
}

/// One ffmpeg session.  Returns when the child dies.
/// Returns `Ok(true)` when `--seconds` ran out (the bounded probe), `Ok(false)` when the
/// stream ended on its own, `Err` when it failed.
fn one_stream(
    opts: &mut Options,
    session: &mut Session,
    stats: &mut Stats,
    started: Instant,
) -> Result<bool, String> {
    let mut stream = (opts.make_source)()?;

    let mut gate = gate::Gate::new(opts.gate.clone());
    let mut pending: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut chunk = [0u8; 3200]; // 100 ms of 16 kHz mono PCM16
    let mut last_status = Instant::now();
    // Seconds of capture still to be thrown away: the half-duplex rule, applied to
    // the audio that was already sitting in the pipe while the answer played.
    let mut drop_secs = 0.0f64;
    // The window being transcribed right now, if any, and if rolling transcription is on.
    let mut live: Option<Live> = None;
    // The whole capture, for `probe --out` on a run where the gate never opened.  Bounded
    // by `capture_secs` (or 30 s, when that is unbounded) so a probe cannot eat the disk.
    let cap_samples = ((if opts.capture_secs > 0.0 { opts.capture_secs } else { 30.0 }).max(5.0)
        * wav::SAMPLE_RATE as f64) as usize;
    let mut raw: Vec<i16> = Vec::with_capacity(cap_samples.min(1 << 20));

    loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            // End of stream: the device dropped its session, the file finished, the link went
            // away — the driver has already reaped whatever it owned.  Whatever the gate is still
            // holding is the last thing anyone said, so it goes to the gate's `flush` rather than
            // into the stream it never came back from.
            if let Some(window) = gate.flush() {
                let secs = window.samples.len() as f64 / wav::SAMPLE_RATE as f64;
                eprintln!("window {secs:.1} s — the stream ended mid-word, sending it anyway");
                let live = live.take();
                handle_window(opts, session, stats, started, window, live);
            }
            return Ok(false);
        }
        stats.captured_secs += n as f64 / 2.0 / wav::SAMPLE_RATE as f64;
        if opts.capture_secs > 0.0 && stats.captured_secs >= opts.capture_secs {
            // Keep whatever the gate was mid-way through; the cap is a human's patience,
            // not a claim that the utterance ended.
            stream.stop();
            if let Some(window) = gate.flush() {
                let secs = window.samples.len() as f64 / wav::SAMPLE_RATE as f64;
                eprintln!("window {secs:.1} s — the probe ran out, sending it anyway");
                let live = live.take();
                handle_window(opts, session, stats, started, window, live);
            }
            if let Some(path) = &opts.out_file {
                if stats.windows == 0 {
                    write_capture(path, &raw)?;
                }
            }
            return Ok(true);
        }
        pending.extend_from_slice(&chunk[..n]);
        while pending.len() >= 2 {
            let take = pending.len() - (pending.len() % 2);
            let samples: Vec<i16> = pending[..take]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| i16::from_le_bytes(*p))
                .collect();
            pending.drain(..take);
            let samples = if drop_secs > 0.0 {
                let n = (drop_secs * wav::SAMPLE_RATE as f64) as usize;
                let n = n.min(samples.len());
                drop_secs -= n as f64 / wav::SAMPLE_RATE as f64;
                // Start the gate clean: whatever it half-collected was the assistant
                // talking to the room, and a window that begins mid-answer is how it
                // answers itself.
                gate = gate::Gate::new(opts.gate.clone());
                samples[n..].to_vec()
            } else {
                samples
            };
            // The loudest frame the gate saw, on the gate's own scale.  Read from the
            // gate rather than measured here, so this number and `gate-check`'s column
            // are one measurement and not two that differ by the averaging window.
            if gate.peak_frame_dbfs > stats.peak_dbfs {
                stats.peak_dbfs = gate.peak_frame_dbfs;
            }
            if opts.out_file.is_some() && raw.len() < cap_samples {
                let room = cap_samples - raw.len();
                raw.extend_from_slice(if samples.len() <= room { &samples } else { &samples[..room] });
            }
            for window in gate.push(&samples) {
                let secs = window.samples.len() as f64 / wav::SAMPLE_RATE as f64;
                if window.peak_dbfs > stats.peak_dbfs {
                    stats.peak_dbfs = window.peak_dbfs;
                }
                eprintln!(
                    "window {secs:.1} s peak {:.0} dBFS floor {:.0} dBFS after {:.1} s quiet{}{}",
                    window.peak_dbfs,
                    gate.floor_dbfs(),
                    window.started_after_silence_secs,
                    if window.flushed { " (input ended)" } else { "" },
                    match &live {
                        Some(l) if l.fed > 0 => format!(", {:.1} s transcribed live", l.fed as f64 / wav::SAMPLE_RATE as f64),
                        _ => String::new(),
                    }
                );
                let taken = live.take();
                drop_secs += handle_window(opts, session, stats, started, window, taken);
                stats.floor_dbfs = gate.floor_dbfs();
                stats.threshold_dbfs = gate.threshold_dbfs();
            }
            // Transcribe the open window as it fills. After `push`, so the samples this chunk
            // added are in the gate's account of the window, and drained through the gate rather
            // than measured here: it is the gate that knows where the window is.
            if opts.asr_slice_secs > 0.0 && gate.is_open() {
                let fresh = gate.take_open_samples();
                if !fresh.is_empty() {
                    let live = live.get_or_insert_with(|| Live {
                        rolling: crate::asr::Rolling::new(
                            opts.asr_slice_secs,
                            crate::asr::DEFAULT_OVERLAP_SECS,
                            {
                                let gateway = opts.gateway.clone();
                                move |samples| transcribe_slice(&gateway, samples)
                            },
                        ),
                        fed: 0,
                        warmed: false,
                    });
                    live.fed += fresh.len();
                    match live.rolling.push(&fresh) {
                        Ok(Some(text)) => {
                            stats.asr_slices = live.rolling.stats.calls;
                            stats.asr_tail_ms = live.rolling.stats.last_ms;
                            eprintln!(
                                "partial {text:?} — {:.1} of {:.1} s spoken, slice {} in {} ms",
                                live.fed as f64 / wav::SAMPLE_RATE as f64,
                                gate.open_secs(),
                                live.rolling.stats.calls,
                                live.rolling.stats.last_ms
                            );
                            // The warm-up fires once, on the *first* partial: its whole purpose is
                            // to spend the engine's prompt-prefill time during the seconds the
                            // speaker still has left.
                            if opts.warm && !live.warmed && !text.trim().is_empty() {
                                live.warmed = true;
                                stats.warms += 1;
                                // The *whole* partial, not just this slice's words: the warm-up is
                                // buying the prefix's prefill, and a prefix is only a prefix.
                                stats.last_transcript = live.rolling.text().to_string();
                                warm(&opts.gateway, live.rolling.text());
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            stats.last_error = format!("live slice: {e}");
                            eprintln!("live transcription failed ({e}); the window will be sent whole");
                            *live = Live {
                                rolling: crate::asr::Rolling::new(0.0, 0.0, |_| Ok(String::new())),
                                fed: live.fed,
                                warmed: true,
                            };
                        }
                    }
                }
            }
        }
        // Every chunk, not every five seconds: these three numbers are the whole answer
        // to "why is it not hearing me", and a copy refreshed only on a completed window
        // or a five-second timer reported the *initial* values for a short stream — which
        // for `floor`/`threshold` means 0.0 dBFS, i.e. "full scale", the single most
        // misleading thing that line could say.
        stats.floor_dbfs = gate.floor_dbfs();
        stats.threshold_dbfs = gate.threshold_dbfs();
        stats.speech_frames = gate.speech_frames;
        if last_status.elapsed() > Duration::from_secs(5) {
            // The heartbeat. In the journal this is what distinguishes a satellite that
            // is listening to a quiet room from one whose gate is set above the room,
            // without anyone having to run a probe.
            eprintln!(
                "listening: {:.0} s captured, floor {:.0} dBFS, threshold {:.0} dBFS, \
                 loudest {:.0} dBFS, {} speech frame(s), {} window(s), {} ingest restart(s)",
                stats.captured_secs,
                stats.floor_dbfs,
                stats.threshold_dbfs,
                stats.peak_dbfs,
                stats.speech_frames,
                stats.windows,
                stats.ingest_restarts
            );
            write_status(opts, stats, started);
            last_status = Instant::now();
        }
    }
}

/// The bounded probe's fallback clip: everything that was captured, so the person who
/// ran it can hear what the camera heard.
fn write_capture(path: &Path, raw: &[i16]) -> Result<(), String> {
    std::fs::write(path, wav::mono16(raw, wav::SAMPLE_RATE))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    eprintln!(
        "the gate never opened; wrote {:.1} s of raw capture to {}",
        raw.len() as f64 / wav::SAMPLE_RATE as f64,
        path.display()
    );
    Ok(())
}

/// What one transcript did, as the seconds of capture to discard afterwards — the
/// only way to say "the assistant has just spoken, do not believe anything it heard"
/// without an echo canceller.
fn handle_window(
    opts: &mut Options,
    session: &mut Session,
    stats: &mut Stats,
    started: Instant,
    window: gate::Window,
    mut live: Option<Live>,
) -> f64 {
    let pcm = window.samples;
    let body = wav::mono16(&pcm, wav::SAMPLE_RATE);
    if let Some(path) = &opts.out_file {
        if let Err(e) = std::fs::write(path, &body) {
            eprintln!("cannot write {}: {e}", path.display());
        }
    }
    stats.windows += 1;
    // The transcript, from wherever it can come from. Live first: it exists already, and asking
    // whisper the same question twice would give two answers and throw the better one away.
    let (text, described) = match live.as_mut() {
        Some(l) if l.rolling.stats.calls > 0 => {
            // The frames that closed the window left the gate inside `Window`, so this is the
            // only place they can still be heard: feed what was never sent, then finish.
            let rest = l.fed.min(pcm.len());
            if rest < pcm.len() {
                let tail = &pcm[rest..];
                l.fed = pcm.len();
                if let Err(e) = l.rolling.push(tail) {
                    eprintln!("last live slice failed: {e}");
                }
            }
            match l.rolling.finish() {
                Ok(t) => {
                    stats.asr_slices = l.rolling.stats.calls;
                    stats.asr_tail_ms = l.rolling.stats.last_ms;
                    let t = t.trim().to_string();
                    let described = crate::stt::looks_like_description(&t);
                    (t, described)
                }
                Err(e) => {
                    stats.last_error = format!("live transcription: {e}");
                    eprintln!("{e} — falling back to the whole window");
                    match transcribe_whole(opts, &body) {
                        Some(v) => v,
                        None => return 0.0,
                    }
                }
            }
        }
        _ => match transcribe_whole(opts, &body) {
            Some(v) => v,
            None => return 0.0,
        },
    };
    stats.last_transcript = text.clone();
    // Counted here rather than by the decision below, because the probe's whole purpose
    // is the ratio: how many windows came back as *words* at all, before anyone asked
    // whether they were for us.
    if !text.trim().is_empty() && !described {
        stats.transcripts += 1;
    }
    eprintln!("transcript {text:?}");

    let action = session.on_transcript(&text, described);
    stats.last_action = format!("{action:?}");
    let dropped: f64 = match action {
        Action::Ignore(why) => {
            stats.ignored += 1;
            eprintln!("ignored: {why}");
            0.0
        }
        Action::Await => {
            stats.asked += 1;
            eprintln!("woke us with no request; listening for {:.1} s", opts.follow_up.as_secs_f64());
            0.0
        }
        Action::Ask(question) => {
            stats.asked += 1;
            if opts.probe_only {
                eprintln!("probe-only: would ask {question:?}");
                return 0.0;
            }
            // The turn, streamed: the answer's first sentence reaches the speaker while the
            // model is still on its second, and the sink starts playing on the first buffer it is
            // given rather than when the answer is finished.
            match ask_turn(opts, &question, stats) {
                Ok(played) => {
                    stats.answers += 1;
                    let cap = opts.sink.capabilities();
                    if cap.audible {
                        stats.spoken += 1;
                    } else {
                        stats.unplayed += 1;
                    }
                    capture_to_discard(played.secs, &cap, opts.guard.as_secs_f64())
                }
                Err(e) => {
                    stats.last_error = e.clone();
                    if e != crate::turn::NO_AUDIO {
                        eprintln!("turn failed: {e}");
                    }
                    0.0
                }
            }
        }
    };
    write_status(opts, stats, started);
    dropped
}

/// How many seconds of capture to throw away, and why the reader never stops reading.
///
/// The dropping happens *in the read loop*, which keeps draining: a source whose output nobody
/// reads blocks, and a blocked ingest is a stopped microphone that nobody notices until the next
/// reconnect.  That is why this returns a number instead of pausing anything, and why the note is
/// here rather than only in the loop — the tempting "optimisation" is to stop reading, and that
/// breaks the stream in a way that looks like a dead device.
fn write_status(opts: &Options, stats: &Stats, started: Instant) {
    let body = format!(
        "{{\"uptime_secs\":{:.0},\"source\":{},\"wake_words\":[{}],\"speaker\":{},\
         \"probe_only\":{},\"windows\":{},\"ignored\":{},\"asked\":{},\"answers\":{},\
         \"spoken\":{},\"unplayed\":{},\"ingest_restarts\":{},\"floor_dbfs\":{:.1},\
         \"threshold_dbfs\":{:.1},\"peak_dbfs\":{:.1},\"speech_frames\":{},\
         \"captured_secs\":{:.0},\"last_transcript\":{},\"last_action\":{},\"asr_slices\":{},\
         \"asr_tail_ms\":{},\"first_audio_ms\":{},\"warms\":{},\"last_error\":{}}}",
        started.elapsed().as_secs_f64(),
        common_json::quoted(&opts.source_label),
        opts.wake_words
            .iter()
            .map(|w| common_json::quoted(w))
            .collect::<Vec<_>>()
            .join(","),
        // The sink's own label, i.e. the deployment's word for it ("off", not "dry-run"): a
        // status file that answers a different question than the config asked — "is the speaker
        // off?" — is how an operator ends up reasoning about a setting nobody set.
        common_json::quoted(&opts.sink_label),
        opts.probe_only,
        stats.windows,
        stats.ignored,
        stats.asked,
        stats.answers,
        stats.spoken,
        stats.unplayed,
        stats.ingest_restarts,
        stats.floor_dbfs,
        // The three numbers that answer "is it deaf, and which kind of deaf" without
        // anyone ssh-ing to the box to read a journal: what a frame had to beat, the
        // loudest frame it actually got, and how many of them it thought were speech.
        // `just voice-status` renders them through the front door.
        stats.threshold_dbfs,
        stats.peak_dbfs,
        stats.speech_frames,
        stats.captured_secs,
        common_json::quoted(&stats.last_transcript),
        common_json::quoted(&stats.last_action),
        stats.asr_slices,
        stats.asr_tail_ms,
        stats.first_audio_ms,
        stats.warms,
        common_json::quoted(&stats.last_error),
    );
    let tmp = opts.status_path.with_extension("json.tmp");
    // Rename, not in-place: the reader here is the gateway, which opens the file per
    // request, so a swap is atomic from its side.  (The lesson in AGENTS.md rule 29 is
    // about a reader that holds the descriptor open — dnsmasq and the lease file — and
    // this reader does not, which is why rename is the right mechanism rather than the
    // dangerous one.)
    if let Err(e) = std::fs::write(&tmp, body).and_then(|()| std::fs::rename(&tmp, &opts.status_path))
    {
        eprintln!("cannot write {}: {e}", opts.status_path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session::new(
            vec!["hey jarvis".to_string(), "jarvis".to_string()],
            6,
            Duration::from_secs(6),
        )
    }

    #[test]
    fn a_wake_word_with_a_request_becomes_the_request() {
        let mut s = session();
        assert_eq!(
            s.on_transcript("hey jarvis is the space open", false),
            Action::Ask("is the space open".to_string())
        );
    }

    #[test]
    fn a_bare_wake_word_earns_one_more_utterance() {
        let mut s = session();
        assert_eq!(s.on_transcript("hey jarvis", false), Action::Await);
        assert_eq!(
            s.on_transcript("what temperature is the workshop", false),
            Action::Ask("what temperature is the workshop".to_string())
        );
        // And the next one needs no wake word at all, but the one after that does.
        assert_eq!(s.on_transcript("and the pi room", false), Action::Ignore("no wake word"));
    }

    #[test]
    fn the_follow_up_window_expires() {
        let mut s = Session::new(vec!["jarvis".into()], 6, Duration::from_millis(0));
        assert_eq!(s.on_transcript("jarvis", false), Action::Await);
        std::thread::sleep(Duration::from_millis(5));
        assert!(matches!(
            s.on_transcript("anything", false),
            Action::Ignore(_)
        ));
    }

    #[test]
    fn music_is_not_someone_talking() {
        let mut s = session();
        // The real measured transcript from the space's own microphone (`docs/voice-assistant-2026-11-16.md`).
        assert_eq!(
            s.on_transcript("(soft music)", true),
            Action::Ignore("whisper described the noise instead of words")
        );
        assert_eq!(s.on_transcript("", false), Action::Ignore("empty transcript"));
        assert_eq!(s.on_transcript("someone is talking about the printer", false), Action::Ignore("no wake word"));
    }

    /// The three cases of the half-duplex rule, each a different way for a speaker in a room to
    /// be wrong.
    #[test]
    fn an_answer_that_made_noise_makes_the_next_seconds_untrustworthy() {
        let noisy = Capabilities { audible: true, echo_cancelled: false };
        let d = capture_to_discard(3.0, &noisy, 1.2);
        assert!((d - 4.2).abs() < 1e-9, "{d}");
    }

    #[test]
    fn an_answer_nobody_could_hear_deafens_nobody() {
        // The dry run has the answer's seconds in hand and played none of them.  Dropping the
        // room's audio here is how a silent unit starts ignoring people for no reason at all.
        let silent = Capabilities { audible: false, echo_cancelled: false };
        assert_eq!(capture_to_discard(0.0, &silent, 1.2), 0.0);
    }

    #[test]
    fn a_sink_that_cancels_its_own_output_keeps_the_mic_open() {
        // Dead code today — no deployed sink claims this — and pinned anyway, so that the day one
        // does, the behaviour is tested before anyone believes it.
        let aec = Capabilities { audible: true, echo_cancelled: true };
        assert_eq!(capture_to_discard(3.0, &aec, 1.2), 0.0);
    }
}
