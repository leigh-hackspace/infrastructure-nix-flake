//! The camera end: cam8's mic in, a spoken answer out.
//!
//! ## What this process actually is
//!
//! A pipe.  `ffmpeg` pulls the restreamed camera audio off `services1`'s own go2rtc
//! (16 kHz mono PCM16, already up there because frigate's feed carries the audio
//! track), [`crate::gate`] cuts utterances out of it, the gateway transcribes and
//! answers, and the answer goes back to the camera's speaker.  Nothing here holds
//! state that matters: restart it and the only loss is the counters in
//! `status.json`.
//!
//! ## Why the mic comes from go2rtc rather than from the camera
//!
//! Measured on 2026-11-16: cam8's RTSP server has `max_clients = 2` and **one**
//! session already live — go2rtc's `ffmpeg:` producer, which asks for
//! `video+audio` on `/ch0` and publishes `L16/16000` on its restream.  So the space's
//! microphone is already on this box, and a second direct RTSP client would fill the
//! camera's last slot and starve frigate on the next reconnect.  Reading the
//! restream instead costs nothing, asks nothing of the camera, and is the same
//! "one consumer, many readers" discipline `cameras/README.md` states as a rule.
//!
//! The alternative that would remove ffmpeg is raptor's own HTTP audio endpoint
//! (`[http] enabled = true`, which serves `/audio` as WAV), but that is a device
//! write plus a streamer restart, i.e. a blip on frigate's `cam8` feed, so it is a
//! decision for a window rather than a step to take here.
//!
//! ## Half-duplex, because there is no echo cancellation
//!
//! `rac status` on cam8 reports `aec: false`.  The speaker feeds the mic, so a naive
//! loop has the assistant answering itself forever.  The rule this implements is the
//! one the browser demo already uses for its confirmation ping
//! (`crates/whisper-ws/README.md`: the mic is muted for 350 ms after the ping), taken
//! to its conclusion: **while an answer is playing, the captured audio is thrown
//! away**, the gate is reset, and the silence after the answer is played is added on
//! top.  There is no barge-in and no "shut up", and there will not be until someone
//! does AEC against the reference signal we generated ourselves — which is the only
//! place it can be done honestly here, since we know the far end exactly.  See
//! `docs/follow-ups.md`.
//!
//! ## The speaker is off until you say otherwise
//!
//! `--speaker off` is the default: answers are logged, counted and written to the
//! state directory, and *nothing reaches the camera*.  `--speaker ssh` puts the
//! answer through `rac play -` over the hop key.  Both paths exist so the pipeline
//! can be run in a live space with people in it before anyone decides a ceiling
//! speaker should talk.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use common_json::Json;

use crate::{b64, gate, wav};

/// Where the answer goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Speaker {
    /// Log it, count it, write the WAV, play nothing.  The default.
    Off,
    /// `ssh <user>@<host> 'rac play - -r 16000'` with the answer's PCM on stdin.
    Ssh {
        host: String,
        user: String,
        key: PathBuf,
    },
}

pub struct Options {
    pub ffmpeg: PathBuf,
    pub source: String,
    pub gateway: String,
    pub wake_words: Vec<String>,
    /// How far into a transcript a wake phrase may start (see `crate::wake`).
    pub search_tokens: usize,
    pub gate: gate::Config,
    pub speaker: Speaker,
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
    pub ssh_timeout: Duration,
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
    pub dry_run: usize,
    pub ffmpeg_restarts: usize,
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
            dry_run: 0,
            ffmpeg_restarts: 0,
            last_error: String::new(),
            last_transcript: String::new(),
            last_action: String::new(),
            floor_dbfs: 0.0,
            captured_secs: 0.0,
            peak_dbfs: -120.0,
            threshold_dbfs: 0.0,
            speech_frames: 0,
            transcripts: 0,
        }
    }
}

impl Stats {
    /// A probe that never opened the gate.
    pub fn windows_of_nothing(&self) -> bool {
        self.windows == 0
    }
}

pub fn run(opts: Options) -> Result<(), String> {
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
        "voice-assistant satellite: source={} wake={:?} speaker={:?}",
        opts.source, opts.wake_words, opts.speaker
    );
    if opts.speaker == Speaker::Off {
        eprintln!("speaker is off: answers are logged and written to {} only", opts.out_dir.display());
    }

    loop {
        match one_stream(&opts, &mut session, &mut stats, started) {
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
            Ok(false) => eprintln!("ffmpeg exited cleanly; restarting in 5 s"),
            Err(e) => {
                stats.last_error = e.clone();
                eprintln!("stream failed: {e}");
            }
        }
        stats.ffmpeg_restarts += 1;
        write_status(&opts, &stats, started);
        std::thread::sleep(Duration::from_secs(5));
    }
}

/// One ffmpeg session.  Returns when the child dies.
/// Returns `Ok(true)` when `--seconds` ran out (the bounded probe), `Ok(false)` when the
/// stream ended on its own, `Err` when it failed.
fn one_stream(
    opts: &Options,
    session: &mut Session,
    stats: &mut Stats,
    started: Instant,
) -> Result<bool, String> {
    let mut cmd = Command::new(&opts.ffmpeg);
    cmd.arg("-hide_banner").arg("-loglevel").arg("error").arg("-nostdin");
    // `-rtsp_transport tcp` is an RTSP **input** option, and ffmpeg refuses it for
    // anything else (measured: `Option rtsp_transport not found` on a WAV file, which
    // is how a test source and a live source would otherwise need two flags).  TCP is
    // what a camera wants — UDP fragments die on a 2.4 GHz link — and a file needs
    // nothing, so the option travels with the protocol it belongs to.
    if opts.source.starts_with("rtsp://") {
        cmd.arg("-rtsp_transport").arg("tcp").arg("-fflags").arg("nobuffer");
    }
    let mut child = cmd
        .arg("-i")
        .arg(&opts.source)
        .arg("-vn")
        .arg("-map")
        .arg("a?")
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg(wav::SAMPLE_RATE.to_string())
        .arg("-f")
        .arg("s16le")
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| {
            format!(
                "cannot run {} (the unit names it via --ffmpeg; `-vn -f s16le` is what we need \
                 from it): {e}",
                opts.ffmpeg.display()
            )
        })?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "ffmpeg: no stdout pipe".to_string())?;

    let mut gate = gate::Gate::new(opts.gate.clone());
    let mut pending: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut chunk = [0u8; 3200]; // 100 ms of 16 kHz mono PCM16
    let mut last_status = Instant::now();
    // Seconds of capture still to be thrown away: the half-duplex rule, applied to
    // the audio that was already sitting in the pipe while the answer played.
    let mut drop_secs = 0.0f64;
    // The whole capture, for `probe --out` on a run where the gate never opened.  Bounded
    // by `capture_secs` (or 30 s, when that is unbounded) so a probe cannot eat the disk.
    let cap_samples = ((if opts.capture_secs > 0.0 { opts.capture_secs } else { 30.0 }).max(5.0)
        * wav::SAMPLE_RATE as f64) as usize;
    let mut raw: Vec<i16> = Vec::with_capacity(cap_samples.min(1 << 20));

    loop {
        let n = stdout
            .read(&mut chunk)
            .map_err(|e| format!("read ffmpeg: {e}"))?;
        if n == 0 {
            // EOF: the child closed, or go2rtc dropped the session.  Whatever the gate
            // is still holding is the last thing anyone said, so it goes to the gate's
            // `flush` rather than into the pipe it never came back from.
            let _ = child.wait();
            if let Some(window) = gate.flush() {
                let secs = window.samples.len() as f64 / wav::SAMPLE_RATE as f64;
                eprintln!("window {secs:.1} s — the stream ended mid-word, sending it anyway");
                handle_window(opts, session, stats, started, window);
            }
            return Ok(false);
        }
        stats.captured_secs += n as f64 / 2.0 / wav::SAMPLE_RATE as f64;
        if opts.capture_secs > 0.0 && stats.captured_secs >= opts.capture_secs {
            // Keep whatever the gate was mid-way through; the cap is a human's patience,
            // not a claim that the utterance ended.
            let _ = child.kill();
            let _ = child.wait();
            if let Some(window) = gate.flush() {
                let secs = window.samples.len() as f64 / wav::SAMPLE_RATE as f64;
                eprintln!("window {secs:.1} s — the probe ran out, sending it anyway");
                handle_window(opts, session, stats, started, window);
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
                    "window {secs:.1} s peak {:.0} dBFS floor {:.0} dBFS after {:.1} s quiet{}",
                    window.peak_dbfs,
                    gate.floor_dbfs(),
                    window.started_after_silence_secs,
                    if window.flushed { " (input ended)" } else { "" }
                );
                drop_secs += handle_window(opts, session, stats, started, window);
                stats.floor_dbfs = gate.floor_dbfs();
                stats.threshold_dbfs = gate.threshold_dbfs();
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
                 loudest {:.0} dBFS, {} speech frame(s), {} window(s), {} ffmpeg restart(s)",
                stats.captured_secs,
                stats.floor_dbfs,
                stats.threshold_dbfs,
                stats.peak_dbfs,
                stats.speech_frames,
                stats.windows,
                stats.ffmpeg_restarts
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
    opts: &Options,
    session: &mut Session,
    stats: &mut Stats,
    started: Instant,
    window: gate::Window,
) -> f64 {
    let pcm = window.samples;
    let body = wav::mono16(&pcm, wav::SAMPLE_RATE);
    if let Some(path) = &opts.out_file {
        if let Err(e) = std::fs::write(path, &body) {
            eprintln!("cannot write {}: {e}", path.display());
        }
    }
    let reply = common_http::request_bytes(
        "POST",
        &format!("{}/v1/transcribe", opts.gateway),
        Some(&body),
        "audio/wav",
        &[],
        Duration::from_secs(60),
    );
    stats.windows += 1;
    let reply = match reply {
        Ok(r) if r.ok() => r,
        Ok(r) => {
            stats.ignored += 1;
            stats.last_error = format!("transcribe: {}", r.describe());
            eprintln!("transcribe failed: {}", stats.last_error);
            return 0.0;
        }
        Err(e) => {
            stats.ignored += 1;
            stats.last_error = format!("transcribe: {e}");
            eprintln!("{e}");
            return 0.0;
        }
    };
    let value = match common_json::parse(&reply.body) {
        Ok(v) => v,
        Err(e) => {
            stats.last_error = format!("transcribe body: {e}");
            return 0.0;
        }
    };
    let text = value
        .get("text")
        .and_then(Json::as_str)
        .unwrap_or("")
        .to_string();
    let described = !matches!(value.get("speech").and_then(Json::as_bool), Some(true));
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
            let answer = match ask(opts, &question) {
                Ok(a) => a,
                Err(e) => {
                    stats.last_error = e.clone();
                    eprintln!("ask failed: {e}");
                    return 0.0;
                }
            };
            stats.answers += 1;
            eprintln!("answered ({}): {}", answer.engine, answer.text);
            let Some(audio) = answer.audio else {
                eprintln!("no audio in the answer: {:?}", answer.warnings);
                return 0.0;
            };
            if answer.audio_secs > opts.max_play_secs {
                eprintln!(
                    "not speaking: {:.0} s is over the --max-play-secs of {:.0} s",
                    answer.audio_secs, opts.max_play_secs
                );
                return 0.0;
            }
            match &opts.speaker {
                Speaker::Off => {
                    stats.dry_run += 1;
                    let path = opts.out_dir.join(format!(
                        "answer-{}.wav",
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0)
                    ));
                    if let Err(e) = std::fs::write(&path, &audio) {
                        eprintln!("cannot write {}: {e}", path.display());
                    } else {
                        eprintln!("dry run: {} ({} bytes) at {}", answer.text, audio.len(), path.display());
                    }
                    0.0
                }
                Speaker::Ssh { host, user, key } => {
                    let decoded = match wav::parse(&audio) {
                        Ok(w) => w.mono(),
                        Err(e) => {
                            stats.last_error = format!("answer audio: {e}");
                            return 0.0;
                        }
                    };
                    if let Err(e) = play(opts, host, user, key, &decoded) {
                        stats.last_error = e.clone();
                        eprintln!("play failed: {e}");
                        return 0.0;
                    }
                    stats.spoken += 1;
                    // The half-duplex rule: nothing heard while it played, and
                    // nothing heard while the room notices it.
                    answer.audio_secs + opts.guard.as_secs_f64()
                }
            }
        }
    };
    write_status(opts, stats, started);
    dropped
}

struct Answered {
    text: String,
    engine: String,
    audio: Option<Vec<u8>>,
    audio_secs: f64,
    warnings: Vec<String>,
}

fn ask(opts: &Options, question: &str) -> Result<Answered, String> {
    let body = format!(
        "{{\"text\":{},\"voice\":true}}",
        common_json::quoted(question)
    );
    let reply = common_http::request_bytes(
        "POST",
        &format!("{}/v1/ask", opts.gateway),
        Some(body.as_bytes()),
        "application/json",
        &[],
        Duration::from_secs(180),
    )
    .map_err(|e| format!("gateway: {e}"))?;
    if !reply.ok() {
        return Err(format!("gateway: {}", reply.describe()));
    }
    let v = common_json::parse(&reply.body).map_err(|e| format!("gateway body: {e}"))?;
    Ok(Answered {
        text: v
            .get("answer")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string(),
        engine: v
            .get("engine")
            .and_then(Json::as_str)
            .unwrap_or("(unknown)")
            .to_string(),
        audio: v
            .get("audio")
            .and_then(Json::as_str)
            .map(b64::decode)
            .and_then(Result::ok),
        audio_secs: v
            .get("audio_secs")
            .and_then(Json::as_f64)
            .unwrap_or(0.0),
        warnings: v
            .get("warnings")
            .and_then(Json::as_array)
            .map(|a| a.iter().filter_map(Json::as_str).map(|s| s.to_string()).collect())
            .unwrap_or_default(),
    })
}

/// Push PCM to the camera's speaker ring through `rac play`.
///
/// `rac` is thingino's own audio client and it reads PCM16 LE on stdin
/// (`rac play - -r 16000`); measured 2026-11-16 with a 350 ms tone, which the
/// device accepted and reported as played.  This is a root ssh into the camera on
/// purpose: there is no other unauthenticated-free path to the speaker today, the
/// hop key is already on every box in the fleet, and the alternative (raptor's ONVIF
/// backchannel) is a PCMU/8000 session on a firmware whose build advertises only
/// that codec — 16 kHz through `rac` is better audio *and* less code.
fn play(
    opts: &Options,
    host: &str,
    user: &str,
    key: &Path,
    pcm: &[i16],
) -> Result<(), String> {
    let started = Instant::now();
    let mut child = Command::new("ssh")
        .arg("-i")
        .arg(key)
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg(format!("ConnectTimeout={}", opts.ssh_timeout.as_secs().max(1)))
        .arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg(format!("{user}@{host}"))
        .arg(format!("rac play - -r {}", wav::SAMPLE_RATE))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn ssh: {e}"))?;
    {
        let mut stdin = child.stdin.take().ok_or("ssh: no stdin")?;
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        stdin
            .write_all(&bytes)
            .map_err(|e| format!("send pcm to rac: {e}"))?;
        stdin.flush().ok();
    }
    let status = child.wait().map_err(|e| format!("wait ssh: {e}"))?;
    if !status.success() {
        return Err(format!(
            "rac play exited {status} (is the camera's audio daemon up? `rac status` says)"
        ));
    }
    // The device played it; the room hears it `secs` after we started writing.
    let secs = pcm.len() as f64 / wav::SAMPLE_RATE as f64;
    let slack = (secs - started.elapsed().as_secs_f64()).max(0.0);
    std::thread::sleep(Duration::from_secs_f64(slack));
    Ok(())
}

/// Throw away `secs` of capture, keeping ffmpeg's pipe drained.
///
/// Draining matters: ffmpeg blocks on a full pipe, and a blocked ffmpeg means a
/// stopped camera audio path that nobody notices until the next reconnect.  The
/// dropping is therefore done *in the read loop*, which never stops reading, and this
/// is the note that says so rather than a second mechanism.
fn write_status(opts: &Options, stats: &Stats, started: Instant) {
    let body = format!(
        "{{\"uptime_secs\":{:.0},\"source\":{},\"wake_words\":[{}],\"speaker\":{},\
         \"probe_only\":{},\"windows\":{},\"ignored\":{},\"asked\":{},\"answers\":{},\
         \"spoken\":{},\"dry_run\":{},\"ffmpeg_restarts\":{},\"floor_dbfs\":{:.1},\
         \"last_transcript\":{},\"last_action\":{},\"last_error\":{}}}",
        started.elapsed().as_secs_f64(),
        common_json::quoted(&opts.source),
        opts.wake_words
            .iter()
            .map(|w| common_json::quoted(w))
            .collect::<Vec<_>>()
            .join(","),
        common_json::quoted(match &opts.speaker {
            Speaker::Off => "dry-run",
            Speaker::Ssh { .. } => "ssh:rac-play",
        }),
        opts.probe_only,
        stats.windows,
        stats.ignored,
        stats.asked,
        stats.answers,
        stats.spoken,
        stats.dry_run,
        stats.ffmpeg_restarts,
        stats.floor_dbfs,
        common_json::quoted(&stats.last_transcript),
        common_json::quoted(&stats.last_action),
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
        // The real measured transcript from cam8's mic.
        assert_eq!(
            s.on_transcript("(soft music)", true),
            Action::Ignore("whisper described the noise instead of words")
        );
        assert_eq!(s.on_transcript("", false), Action::Ignore("empty transcript"));
        assert_eq!(s.on_transcript("someone is talking about the printer", false), Action::Ignore("no wake word"));
    }

    #[test]
    fn the_speaker_default_is_off() {
        assert_eq!(Speaker::Off, Speaker::Off);
    }
}
