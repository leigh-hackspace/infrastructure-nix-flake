//! Deciding, without a model, whether the last half second is a person talking to
//! us — and cutting one utterance out of the camera's continuous stream.
//!
//! ## Why this is here instead of a neural network
//!
//! The honest answer is that the network's own wake-word runtime is a browser one:
//! [OpenWakeWord](https://github.com/dscripka/openWakeWord) runs through
//! `onnxruntime-web` on the whisper page (`crates/whisper-ws/README.md`), and the
//! box this runs on has no ONNX runtime in it.  Putting one behind this gate is the
//! right change *after* the gate has been measured (the follow-up is in
//! `docs/follow-ups.md`); the reason it is not the first change is that the gate is
//! the part that has to be cheap, and a DSP gate costs microseconds per frame on a
//! box that is also carrying the space's inference.
//!
//! What the gate buys, and the number that makes it necessary: **cam8's mic hears
//! the room's music** — measured 2026-11-16, three seconds of that mic sent straight
//! to whisper.cpp came back `(soft music)`, and the space plays music most of the
//! day.  An energy-only VAD would therefore hand whisper a window every few seconds
//! forever, at ~0.9× real time per window (`crates/voice-assistant/README.md`),
//! which is the space's CPU.  So the gate asks three things of a frame, all of them
//! cheap, and the second and third are what music tends to fail:
//!
//! 1. it is louder than the room's own floor, by a margin;
//! 2. it modulates — speech has ~3–8 Hz syllable envelope, a held note or a fan does
//!    not, so the spread of recent frame levels is a real discriminator;
//! 3. it has presence above ~2 kHz — fricatives and stops put energy there and a
//!    steady tone does not.
//!
//! None of that is a claim to *recognise* speech, and the code never pretends to:
//! the window it produces goes to whisper, and the wake-word match in `wake.rs`
//! happens on the **transcript**, which is the part that is actually reliable.  A
//! false window therefore costs one whisper pass, not one assistant answer, and
//! `--probe-only` exists to count exactly those costs in the real room before
//! anyone plugs a speaker in.

use std::collections::VecDeque;

/// Frame length, ms.  20 ms is the usual speech frame: one 16 kHz frame is 320
/// samples, and the syllable rate this gate looks for is 3–8 Hz, so a frame is
/// comfortably shorter than the envelope it has to resolve.
pub const FRAME_MS: usize = 20;
/// Everything in this gate is 16 kHz mono PCM16: the camera's restream is `L16/16000`
/// (measured, `cameras/README.md`), the browser page resamples to it, and whisper.cpp
/// wants exactly it — so the rate is a constant here rather than an argument that could
/// disagree with the one in `wav.rs`.
pub const SAMPLE_RATE: usize = 16_000;
pub const FRAME_SAMPLES: usize = SAMPLE_RATE * FRAME_MS / 1000;

/// Four spectral groups, Hz, each probed at several centres.
///
/// A single Goertzel bin per group would be cheaper and wrong: a 20 ms frame resolves
/// 50 Hz, so one bin on each side of a boundary samples whatever happens to be near
/// that exact frequency and calls it the group's energy.  Probing three centres per
/// group and summing costs twelve Goertzel passes per 320-sample frame — trivially
/// cheap, and it is the difference between "there is 1 % of the energy above 1.8 kHz"
/// and "the frame is silent up there", which is the whole test.
const GROUPS: [[f64; 3]; 4] = [
    [120.0, 240.0, 360.0],                    // the fundamental and its first formant
    [450.0, 650.0, 850.0],                    // where the voice's body lives
    [1000.0, 1300.0, 1700.0],                 // presence
    [2000.0, 2600.0, 3400.0],                 // fricatives: s, f, t — the discriminator
];

#[derive(Debug, Clone)]
pub struct Config {
    /// dBFS above the tracked floor a frame must reach before it counts.
    pub margin_db: f64,
    /// Absolute floor, dBFS: below this nothing is ever "loud", however it modulates.
    pub min_open_dbfs: f64,
    /// Consecutive speech-like frames needed to open.  Three is 60 ms: long enough
    /// to reject a click or a door slam's onset, short enough not to clip a word.
    pub open_frames: usize,
    /// Silence before a window closes.
    pub hold_secs: f64,
    /// Shorter than this and the window is dropped (a cough, a single consonant).
    pub min_secs: f64,
    /// Hard cap, so someone talking for a minute still gets transcribed in pieces.
    pub max_secs: f64,
    /// Audio kept before the trigger, so the first syllable is not lost.
    pub pre_roll_secs: f64,
    /// Minimum share of energy above ~1.8 kHz for a frame to be speech-like.
    ///
    /// 0.004 is not arbitrary: a voice with a 1/h harmonic spectrum puts about 1 % of
    /// its power up there and a **pure tone puts essentially none** — which is the
    /// single discriminator between "someone talking" and "the room's music, a fan, a
    /// held note", and the reason a *held* 440 Hz tone at any level never opens.
    pub hf_share: f64,
    /// Minimum share in the two lowest groups (the voice's body).
    pub voiced_share: f64,
    /// Relative spread of recent frame levels (stddev / mean).
    pub modulation: f64,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            margin_db: 8.0,
            min_open_dbfs: -42.0,
            open_frames: 3,
            hold_secs: 0.7,
            min_secs: 0.45,
            max_secs: 10.0,
            pre_roll_secs: 0.35,
            hf_share: 0.004,
            voiced_share: 0.45,
            modulation: 0.12,
        }
    }
}

/// One completed utterance.
pub struct Window {
    pub samples: Vec<i16>,
    /// Why the caller should believe this is speech, for the log line and for
    /// `--probe-only`'s counting.
    pub peak_dbfs: f64,
    pub started_after_silence_secs: f64,
    /// True when the window ended because the **input stopped**, not because the room
    /// went quiet.  It means the last syllable may be clipped and the transcript may be
    /// short — worth logging, because a stream that restarts every few seconds shows up
    /// as an assistant that keeps half-hearing people.
    pub flushed: bool,
}

/// Root-mean-square of a frame, as a fraction of full scale (0.0 … 1.0).
pub fn rms(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| (s as f64 / 32768.0) * (s as f64 / 32768.0)).sum();
    (sum / samples.len() as f64).sqrt()
}

/// RMS of a frame in dBFS.  The floor is what silence reads as, and `-120` keeps it
/// finite rather than pretending it is a number with meaning.
pub fn dbfs(samples: &[i16]) -> f64 {
    let r = rms(samples);
    if r <= 1e-6 {
        -120.0
    } else {
        20.0 * r.log10()
    }
}

/// Goertzel power at `freq`, the classic single-bin DFT.  Production reads the spectrum
/// through `shares`, so this wrapper exists for the test that pins the frequency ratio —
/// a factor-of-two error in the bin index does not error, it answers "no energy
/// anywhere", which is exactly how a gate comes to never open — and for anyone debugging
/// a room with `gate-check`.
#[cfg(test)]
pub fn band_energy(samples: &[i16], freq: f64, rate: u32) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    goertzel(&samples.iter().map(|&s| s as f64).collect::<Vec<f64>>(), freq, rate)
}

/// The Goertzel recurrence over already-converted samples, so a caller that has a
/// windowed copy (see [`shares`]) does not rebuild one per bin.
fn goertzel(x: &[f64], freq: f64, rate: u32) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    let n = x.len() as f64;
    let omega = 2.0 * std::f64::consts::PI * freq / rate as f64;
    let coeff = 2.0 * omega.cos();
    let mut s1 = 0.0f64;
    let mut s2 = 0.0f64;
    for &v in x {
        let s0 = v + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - coeff * s1 * s2) / (n * n)
}

/// `[voiced share, high share]` of a frame's energy: the two groups low enough to be
/// a voice, and the two high enough to only be one if someone is saying "s" or "f".
fn shares(samples: &[i16], rate: u32) -> (f64, f64) {
    // Hann-windowed.  A 20 ms frame analysed with a rectangular window has -13 dB
    // sidelobes, which is enough for a loud 440 Hz tone to *look* like it has energy
    // above 2 kHz — 0.08 % of it, measured — and the whole point of the high group is
    // that a tone has none.  Hann costs one multiply per sample per frame and buys
    // three orders of separation.
    let n = samples.len();
    let win: Vec<f64> = samples
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let w = if n > 1 {
                0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos()
            } else {
                1.0
            };
            s as f64 * w
        })
        .collect();
    let mut group = [0.0f64; 4];
    let mut total = 0.0f64;
    for (g, probes) in GROUPS.iter().enumerate() {
        for f in probes {
            let e = goertzel(&win, *f, rate);
            group[g] += e;
            total += e;
        }
    }
    if total <= 1e-16 {
        return (0.0, 0.0);
    }
    ((group[0] + group[1]) / total, (group[2] + group[3]) / total)
}

const FLOOR_SECS: usize = 6;

/// The gate, fed 16 kHz mono PCM in whatever sizes the reader produces.
pub struct Gate {
    cfg: Config,
    pending: Vec<i16>,
    /// Rolling floor: the quietest frame in the last [`FLOOR_SECS`].
    recent_levels: VecDeque<f64>,
    /// The last few frame levels, for the modulation test.
    envelope: VecDeque<f64>,
    pre_roll: VecDeque<i16>,
    open: Option<Opening>,
    run: usize,
    silence_frames: usize,
    pub windows: usize,
    pub dropped_short: usize,
    /// Frames that passed all four tests.  With `windows`, the pair distinguishes the
    /// two ways a room can be ignored: zero speech-like frames is a quiet room or a
    /// threshold above it; speech-like frames and no window is the `--open-frames`
    /// run-of-three rule or `--min-secs`, and those are different knobs.
    pub speech_frames: usize,
    /// Loudest 20 ms frame the gate has measured, dBFS — the gate's own scale, so the
    /// number in the heartbeat and the number in `gate-check`'s table are the same
    /// measurement of the same thing.
    pub peak_frame_dbfs: f64,
    pub seconds_seen: f64,
    /// Per-frame decision inputs, kept **only** when someone asked for them.
    /// `gate-check` is how a gate that "never opens" is diagnosed — the alternative is
    /// reading the room off a log of final decisions, which is the difference between
    /// "the room was quiet" and "the threshold is above the room".  `None` costs a
    /// branch per frame, so the live path pays nothing.
    trace: Option<Vec<FrameTrace>>,
}

/// One frame, as the gate saw it.
#[derive(Debug, Clone, Copy)]
pub struct FrameTrace {
    pub secs: f64,
    pub level: f64,
    pub floor: f64,
    pub threshold: f64,
    pub modulation: f64,
    pub voiced: f64,
    pub hf: f64,
    pub speech_like: bool,
    pub opened: bool,
    pub closed: bool,
}

struct Opening {
    samples: Vec<i16>,
    peak_dbfs: f64,
    silence_before: f64,
}

impl Gate {
    pub fn new(cfg: Config) -> Gate {
        let floor_frames = FLOOR_SECS * 1000 / FRAME_MS;
        Gate {
            pending: Vec::with_capacity(FRAME_SAMPLES),
            recent_levels: VecDeque::with_capacity(floor_frames),
            envelope: VecDeque::with_capacity(14),
            pre_roll: VecDeque::with_capacity((cfg.pre_roll_secs * 1000.0 / FRAME_MS as f64) as usize + 1),
            open: None,
            run: 0,
            silence_frames: 0,
            windows: 0,
            dropped_short: 0,
            speech_frames: 0,
            peak_frame_dbfs: -120.0,
            seconds_seen: 0.0,
            trace: None,
            cfg,
        }
    }

    /// Start recording frames.  For `gate-check`.
    pub fn trace_on(&mut self) {
        self.trace = Some(Vec::new());
    }

    pub fn take_trace(&mut self) -> Vec<FrameTrace> {
        self.trace.take().unwrap_or_default()
    }

    /// Record the frame that was just decided.  Called from `frame`, which is the only
    /// place that knows what the inputs were; a trace written from outside would be a
    /// second implementation of the gate, and it is the agreement with the live path
    /// that makes the output worth reading.
    fn record(&mut self, level: f64, floor: f64, modulation: f64, voiced: f64, hf: f64, speech_like: bool) {
        let Some(trace) = self.trace.as_mut() else {
            return;
        };
        trace.push(FrameTrace {
            secs: self.seconds_seen,
            level,
            floor,
            threshold: (floor + self.cfg.margin_db).max(self.cfg.min_open_dbfs),
            modulation,
            voiced,
            hf,
            speech_like,
            opened: false,
            closed: false,
        });
    }

    fn mark_last(&mut self, opened: bool, closed: bool) {
        if let Some(trace) = self.trace.as_mut() {
            if let Some(last) = trace.last_mut() {
                last.opened = opened;
                last.closed = closed;
            }
        }
    }

    /// The floor the gate is currently comparing against, dBFS.  Exposed because
    /// "why is it not opening" and "why is it always opening" are both answered by
    /// looking at it, and neither is answerable from a log of decisions.
    pub fn floor_dbfs(&self) -> f64 {
        self.recent_levels.iter().copied().fold(0.0f64, f64::min)
    }

    /// The level a frame has to beat to count as speech, right now — `floor_dbfs` and
    /// the config's two limits resolved to the one number that matters.  `probe` prints
    /// it beside the loudest frame it saw, because "the gate never opened" is only
    /// actionable if you can see the gap: 12 s of a room peaking at −52 against a −39
    /// threshold is a quiet room, and −30 against −39 is a gate set wrong.
    pub fn threshold_dbfs(&self) -> f64 {
        (self.floor_dbfs() + self.cfg.margin_db).max(self.cfg.min_open_dbfs)
    }

    /// Whether a window is being collected right now.  Read by the tests and by
    /// `gate-check`; the live path reports the same thing through the satellite's status
    /// file, so nothing on the hot path needs it — hence `cfg(test)` rather than a
    /// dead-code allowance.
    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Feed PCM.  Returns the window each time one completes.
    pub fn push(&mut self, pcm: &[i16]) -> Vec<Window> {
        let mut out = Vec::new();
        self.pending.extend_from_slice(pcm);
        while self.pending.len() >= FRAME_SAMPLES {
            let frame: Vec<i16> = self.pending.drain(..FRAME_SAMPLES).collect();
            self.seconds_seen += FRAME_MS as f64 / 1000.0;
            if let Some(w) = self.frame(&frame) {
                out.push(w);
            }
        }
        out
    }

    /// End of input: hand back a window that was still open.
    ///
    /// Without this, **the last thing anyone said is thrown away** whenever the stream
    /// ends — which is not only the end of a clip: go2rtc drops its session, `probe
    /// --seconds 8` runs out, ffmpeg restarts, and in every one of those cases a person
    /// has just spoken a sentence that the gate was still holding because it wants 0.7 s
    /// of trailing silence to be sure the sentence ended.  The clip we have is the only
    /// clip there is, so it goes to whisper; a clipped end is a worse transcript, and no
    /// transcript at all is worse than a worse one.
    ///
    /// `--min-secs` still applies: a window shorter than that was never an utterance.
    pub fn flush(&mut self) -> Option<Window> {
        let done = self.open.take()?;
        let secs = done.samples.len() as f64 / SAMPLE_RATE as f64;
        if secs < self.cfg.min_secs {
            self.dropped_short += 1;
            return None;
        }
        self.windows += 1;
        Some(Window {
            samples: done.samples,
            peak_dbfs: done.peak_dbfs,
            started_after_silence_secs: done.silence_before,
            flushed: true,
        })
    }

    fn frame(&mut self, frame: &[i16]) -> Option<Window> {
        let level = dbfs(frame);
        let linear = rms(frame);
        let (voiced, hf) = shares(frame, 16_000);
        self.track_floor(level);
        if level > self.peak_frame_dbfs {
            self.peak_frame_dbfs = level;
        }
        let floor = self.floor_dbfs();
        let loud = level > floor + self.cfg.margin_db && level > self.cfg.min_open_dbfs;
        let modulating = self.modulation();
        let speech_like =
            loud && modulating > self.cfg.modulation && hf > self.cfg.hf_share && voiced > self.cfg.voiced_share;
        if speech_like {
            self.speech_frames += 1;
        }
        self.record(level, floor, modulating, voiced, hf, speech_like);

        // The envelope and pre-roll are updated before the state machine so an open
        // window always includes the frame that triggered it (and its predecessors).
        self.push_envelope(linear);
        self.push_pre_roll(frame);

        // A frame that arrives while a window is open belongs to it, whatever the
        // gate then decides: the trailing silence *is* part of what was said, and
        // whisper.cpp handles a short tail far better than a clipped end.
        if let Some(opening) = self.open.as_mut() {
            opening.peak_dbfs = opening.peak_dbfs.max(level);
            opening.samples.extend_from_slice(frame);
            self.silence_frames = if speech_like { 0 } else { self.silence_frames + 1 };
            let hold_frames = (self.cfg.hold_secs * 1000.0 / FRAME_MS as f64) as usize;
            let max_frames = (self.cfg.max_secs * 1000.0 / FRAME_MS as f64) as usize;
            let forced = opening.samples.len() / FRAME_SAMPLES >= max_frames;
            if self.silence_frames >= hold_frames || forced {
                let done = self.open.take().expect("just matched Some");
                self.mark_last(false, true);
                return self.finish(done);
            }
            return None;
        }

        // Closed: count a run of speech-like frames, and open on the third.
        if speech_like {
            self.run += 1;
        } else {
            self.run = 0;
        }
        if self.run >= self.cfg.open_frames {
            let silence_secs = self.silence_frames as f64 * FRAME_MS as f64 / 1000.0;
            // The pre-roll already holds this frame and the run that led to it, so
            // the window starts with the earliest of them and loses no syllable.
            let samples: Vec<i16> = self.pre_roll.iter().copied().collect();
            self.open = Some(Opening {
                samples,
                peak_dbfs: level,
                silence_before: silence_secs,
            });
            self.silence_frames = 0;
            self.run = 0;
            self.mark_last(true, false);
        }
        None
    }

    fn finish(&mut self, done: Opening) -> Option<Window> {
        let secs = done.samples.len() as f64 / 16_000.0;
        if secs < self.cfg.min_secs {
            self.dropped_short += 1;
            return None;
        }
        self.windows += 1;
        Some(Window {
            samples: done.samples,
            peak_dbfs: done.peak_dbfs,
            started_after_silence_secs: done.silence_before,
            flushed: false,
        })
    }

    fn track_floor(&mut self, level: f64) {
        let cap = FLOOR_SECS * 1000 / FRAME_MS;
        if self.recent_levels.len() == cap {
            self.recent_levels.pop_front();
        }
        self.recent_levels.push_back(level);
    }

    fn push_envelope(&mut self, level: f64) {
        let cap = 14;
        if self.envelope.len() == cap {
            self.envelope.pop_front();
        }
        self.envelope.push_back(level);
    }

    fn push_pre_roll(&mut self, frame: &[i16]) {
        let cap = (self.cfg.pre_roll_secs * 1000.0 / FRAME_MS as f64) as usize * FRAME_SAMPLES;
        for &s in frame {
            if self.pre_roll.len() == cap {
                self.pre_roll.pop_front();
            }
            self.pre_roll.push_back(s);
        }
    }

    /// Relative spread of the last few frame levels: **stddev of the linear RMS over
    /// its mean**.
    ///
    /// Linear, not dB, and this is the one place the difference decides something.
    /// Measured on the fixture: the same signal whose envelope swings 2.5:1 reads
    /// 0.25 on this scale and 0.019 after a dB conversion, because dB compresses a
    /// loud signal's dynamic range to within a couple of units and dividing by a
    /// number around 120 (the +120 offset that keeps it positive) then reports any
    /// syllable rate at all as "steady".  A gate tuned on the dB version never opens.
    fn modulation(&self) -> f64 {
        let n = self.envelope.len();
        if n < 4 {
            return 0.0;
        }
        let mean = self.envelope.iter().sum::<f64>() / n as f64;
        if mean <= 1e-5 {
            return 0.0;
        }
        let var = self.envelope.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / n as f64;
        var.sqrt() / mean
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames of `n` ms of a sine at `freq`, with an optional 4 Hz amplitude
    /// modulation — the difference between "a held note" and "someone talking".
    fn tone(secs: f64, freq: f64, amplitude: f64, modulated: bool) -> Vec<i16> {
        let n = (secs * 16_000.0) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / 16_000.0;
                let env = if modulated {
                    0.55 + 0.45 * (2.0 * std::f64::consts::PI * 4.0 * t).cos()
                } else {
                    1.0
                };
                (amplitude * env * (2.0 * std::f64::consts::PI * freq * t).sin() * 32_700.0) as i16
            })
            .collect()
    }

    /// A crude stand-in for speech: a low modulated fundamental *plus* high-frequency
    /// presence, which is the part the gate's third question is about.
    fn speech_like(secs: f64, amplitude: f64) -> Vec<i16> {
        let n = (secs * 16_000.0) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / 16_000.0;
                let env = 0.4 + 0.6 * (2.0 * std::f64::consts::PI * 4.0 * t).cos().abs();
                let low = (2.0 * std::f64::consts::PI * 180.0 * t).sin();
                // A saw-ish harmonic series up to ~3 kHz: real speech's envelope
                // carries the modulation, and this reproduces the HF share honestly.
                let mut harm = 0.0;
                for h in 1..14 {
                    harm += (2.0 * std::f64::consts::PI * 180.0 * h as f64 * t).sin() / h as f64;
                }
                let noise = if i % 7 == 0 { 0.6 } else { -0.2 };
                (amplitude * env * (0.5 * low + 0.35 * harm + 0.15 * noise) * 22_000.0) as i16
            })
            .collect()
    }

    fn feed(gate: &mut Gate, pcm: &[i16]) -> Vec<Window> {
        let mut all = Vec::new();
        for chunk in pcm.chunks(FRAME_SAMPLES * 3) {
            all.extend(gate.push(chunk));
        }
        all
    }

    #[test]
    fn silence_never_opens_and_the_floor_stays_at_the_floor() {
        let mut gate = Gate::new(Config::default());
        let quiet = vec![0i16; 16_000 * 3];
        assert!(feed(&mut gate, &quiet).is_empty());
        assert!(gate.floor_dbfs() < -100.0, "floor read {}", gate.floor_dbfs());
        assert!(!gate.is_open());
    }

    #[test]
    fn a_held_note_is_not_someone_talking_to_us() {
        // The failure mode this exists for: the space's music, and a camera on a
        // desk with a fan.  Both are loud, both are steady.
        let mut gate = Gate::new(Config::default());
        assert!(
            feed(&mut gate, &tone(3.0, 440.0, 0.7, false)).is_empty(),
            "a steady 440 Hz tone opened the gate"
        );
        let mut gate = Gate::new(Config::default());
        assert!(
            feed(&mut gate, &tone(3.0, 120.0, 0.8, true)).is_empty(),
            "a modulated low hum opened the gate (no HF presence)"
        );
    }

    #[test]
    fn speech_opens_it_once_and_closes_on_the_pause() {
        let mut gate = Gate::new(Config::default());
        let mut pcm = tone(1.0, 100.0, 0.02, false); // a second of room first, to set the floor
        pcm.extend(speech_like(1.6, 0.9));
        pcm.extend(vec![0i16; 16_000]); // a second of silence: past the 0.7 s hold
        let windows = feed(&mut gate, &pcm);
        assert_eq!(windows.len(), 1, "expected exactly one window");
        let w = &windows[0];
        let secs = w.samples.len() as f64 / 16_000.0;
        // 1.6 s of speech, plus 0.35 s of pre-roll, plus the 0.7 s of silence that
        // closed it: the tail is part of the window by design (see `frame`), because
        // whisper handles a short silence far better than a clipped final syllable.
        assert!(
            (1.6..2.9).contains(&secs),
            "window was {secs:.2} s, so the pre-roll or the hold is wrong"
        );
        assert!(w.peak_dbfs > -20.0, "peak {}", w.peak_dbfs);
    }

    #[test]
    fn pre_roll_saves_the_first_syllable() {
        let mut gate = Gate::new(Config::default());
        let mut pcm = vec![0i16; 16_000];
        pcm.extend(speech_like(1.2, 0.9));
        pcm.extend(vec![0i16; 16_000]);
        let windows = feed(&mut gate, &pcm);
        assert_eq!(windows.len(), 1);
        // Three frames of confirmation plus 0.35 s of pre-roll: the window must be
        // at least as long as the speech, not shorter by the confirmation delay.
        let secs = windows[0].samples.len() as f64 / 16_000.0;
        assert!(secs > 1.5, "pre-roll lost audio: {secs:.2} s");
    }

    #[test]
    fn a_click_is_dropped_and_a_long_monologue_is_cut() {
        let mut gate = Gate::new(Config::default());
        let mut pop = vec![0i16; 16_000 / 4];
        pop.extend(vec![12_000i16; 40]);
        pop.extend(vec![0i16; 16_000]);
        assert!(feed(&mut gate, &pop).is_empty(), "a click made a window");

        let mut gate = Gate::new(Config {
            max_secs: 1.0,
            ..Default::default()
        });
        let mut pcm = vec![0i16; 16_000];
        pcm.extend(speech_like(2.6, 0.9));
        pcm.extend(vec![0i16; 16_000]);
        let windows = feed(&mut gate, &pcm);
        assert!(!windows.is_empty(), "a long monologue produced nothing");
        assert!(
            windows.iter().all(|w| w.samples.len() as f64 / 16_000.0 <= 1.2),
            "max_secs was not honoured"
        );
    }

    #[test]
    fn the_floor_tracks_a_room_that_gets_quieter() {
        // Measured shape of the real problem: music stops, someone then speaks
        // quietly.  A fixed threshold would need re-tuning per room; the floor has
        // to come down within the floor's own window (6 s), so 4 s of quiet room is
        // enough before a moderate voice counts as loud.
        let mut gate = Gate::new(Config {
            margin_db: 6.0,
            ..Default::default()
        });
        let mut pcm = tone(2.0, 300.0, 0.5, false);
        pcm.extend(vec![0i16; 16_000 * 4]);
        pcm.extend(speech_like(1.4, 0.25));
        pcm.extend(vec![0i16; 16_000]);
        assert!(!feed(&mut gate, &pcm).is_empty(), "floor never came down");
    }

    #[test]
    fn a_voice_has_a_thousandth_of_its_energy_up_top_and_a_tone_has_none() {
        // The measured shape of the default `hf_share`: the saw-ish fixture (a 1/h
        // harmonic spectrum, which is what a voiced sound is) lands around 1 %, a pure
        // tone lands two orders below it, and the default sits between.
        let voice = speech_like(0.5, 0.8);
        let tone = tone(0.5, 440.0, 0.8, false);
        let (_, hf_voice) = shares(&voice[..FRAME_SAMPLES], 16_000);
        let (_, hf_tone) = shares(&tone[..FRAME_SAMPLES], 16_000);
        assert!(hf_voice > 0.004, "voice hf {hf_voice:.5} below the default");
        assert!(hf_tone < hf_voice / 20.0, "tone hf {hf_tone:.5} vs voice {hf_voice:.5}");
        let (voiced, _) = shares(&voice[..FRAME_SAMPLES], 16_000);
        assert!(voiced > 0.45, "voice voiced {voiced:.3}");
    }

    #[test]
    fn band_energy_knows_where_the_energy_is() {
        let s = tone(0.2, 1000.0, 1.0, false);
        let hi = band_energy(&s, 1000.0, 16_000);
        let lo = band_energy(&s, 200.0, 16_000);
        assert!(hi > lo * 100.0, "{hi} vs {lo}");
        assert!(band_energy(&[], 1000.0, 16_000) == 0.0);
    }

    #[test]
    fn a_window_still_held_when_the_input_ends_is_not_thrown_away() {
        // The bug this pins: a stream that stops mid-sentence — go2rtc dropping its
        // session, `probe --seconds 8` running out, a 2.7 s test clip — used to discard
        // the utterance the gate was still holding, because a window only ends after
        // `hold_secs` of trailing silence.  It presented as "the gate never opened" on a
        // clip the trace showed opening at 0.38 s, which is a long way from the truth.
        let mut gate = Gate::new(Config::default());
        let mut pcm = tone(1.0, 100.0, 0.02, false); // a second of room, to set the floor
        pcm.extend(speech_like(1.6, 0.9)); // and then someone talks, and the input stops
        let windows = feed(&mut gate, &pcm);
        assert!(
            windows.is_empty(),
            "no trailing silence, so nothing should have closed yet"
        );
        assert!(gate.is_open(), "and the gate should still be holding it");
        let flushed = gate.flush().expect("the open window comes back, not the bin");
        assert!(flushed.flushed, "and says so, because the end may be clipped");
        let secs = flushed.samples.len() as f64 / SAMPLE_RATE as f64;
        assert!(secs >= gate.cfg.min_secs, "flush honours --min-secs, got {secs:.2}");
        // A second flush has nothing to give: `flush` takes, it does not peek.  A caller
        // that flushed twice would ask whisper the same question twice.
        assert!(gate.flush().is_none());
        // And a gate that was never opened flushes to nothing rather than inventing a
        // window of room tone at the end of every stream.
        let mut quiet = Gate::new(Config::default());
        feed(&mut quiet, &vec![0i16; 16_000]);
        assert!(quiet.flush().is_none());
    }

    #[test]
    fn dbfs_is_the_scale_we_think_it_is() {
        assert!(dbfs(&[0, 0, 0]) < -100.0);
        let full: Vec<i16> = (0..1600).map(|i| if i % 2 == 0 { 23170 } else { -23170 }).collect();
        // A 1 kHz sine at 0.7 amplitude reads about -3 dBFS.
        let d = dbfs(&tone(0.2, 1000.0, 0.707, false));
        // 0.707 peak is 0.5 RMS, which is -6 dBFS.
        assert!(d > -6.5 && d < -5.5, "{d}");
        assert!(dbfs(&full) > -3.2 && dbfs(&full) < -2.8, "{}", dbfs(&full));
    }
}


