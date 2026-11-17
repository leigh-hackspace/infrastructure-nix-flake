//! Which detector a session is scored by, and the one rule that decides a detection.
//!
//! ## Why this exists
//!
//! `serve` was written around `livekit_wakeword`'s `predict()`. The reason it is now written
//! around an enum is that the browser and the server have never run the same detector: the page
//! runs openWakeWord (melspectrogram → embedding → keyword head) and the server has always run
//! LiveKit's single graph, so "the server is worse at wake words" has always been a comparison of
//! two models rather than of two machines. Reporting that as a mystery is not good enough when a
//! person in the room can hear the difference, so the server can now run the browser's model too
//! (`--engine oww`), and the same bytes can be scored by both in one process family.
//!
//! ## Where the threshold lives, and why it is not in the engine
//!
//! Each engine keeps its own *streaming* state, which is genuinely per-engine and cannot be
//! shared. The decision — `score >= threshold`, and at least `cooldown` of **audio** since the
//! last one — lives here, once, so switching engine cannot also switch the rule and quietly
//! change what "detected" means. Cooldown is audio time rather than wall clock for the reason
//! `serve.rs` already states: a session paused while the assistant talks must not fire again on
//! the tail of the same utterance.
//!
//! ## The honest limits of the openWakeWord engine here
//!
//! * **16 kHz only.** The livekit crate resamples internally; this pipeline does not, because the
//!   graphs want mel frames of exactly 80 ms at 16 kHz. A session at another rate is refused with
//!   a message saying which engine refused it, rather than resampled wrongly.
//! * **No silero gate.** That export of silero will not load under tract (`If` op), so this
//!   engine scores the raw keyword model. The gate is what the browser uses to keep *false accepts*
//!   down — it does not make recall better — so an ungated OWW is a legitimate thing to compare and
//!   is not the same detector the page runs. `gate.rs` in the voice gateway is the in-house
//!   candidate to replace it.
//! * **Thresholds do not transfer between engines.** openWakeWord and LiveKit were trained
//!   separately on the same phrase; a score of 0.5 means something different in each. Any number
//!   here is a starting point, not a tuned one.

use std::path::Path;

use livekit_wakeword::WakeWordModel;

use crate::oww::{Oww, FRAME};

/// What one chunk produced.
#[derive(Debug, Clone, Copy)]
pub struct Hit {
    /// The loudest moment in the chunk, over every step the engine completed.
    pub score: f32,
    /// `score` past the threshold, and the cooldown (in audio time) elapsed.
    pub detected: bool,
}

pub enum Engine {
    Livekit(WakeWordModel),
    Oww { inner: Oww, left: Vec<f32> },
}

pub struct Ear {
    kind: Engine,
    /// The sample rate the *client* is sending at. The livekit crate resamples; OWW refuses
    /// anything but 16 kHz at construction time.
    rate: u32,
    threshold: f32,
    cooldown: f64,
    /// Seconds of audio fed, which is the clock the cooldown is measured on.
    audio: f64,
    last_detect: f64,
    name: String,
}

impl Ear {
    /// Build one engine for a session. `oww_dir` must hold `melspectrogram.onnx`,
    /// `embedding_model.onnx` and `<keyword>.onnx` — the browser's own files, not a re-export.
    pub fn open(
        engine: &str,
        model: &Path,
        oww_dir: &Path,
        oww_keyword: &str,
        threshold: f32,
        cooldown: f64,
        rate: u32,
    ) -> Result<Ear, String> {
        let (kind, name) = match engine {
            "livekit" | "" => {
                let m = WakeWordModel::new(&[model.to_path_buf()], rate)
                    .map_err(|e| format!("livekit model: {e}"))?;
                let name = m
                    .model_names()
                    .next()
                    .unwrap_or("hey_jarvis")
                    .to_string();
                (Engine::Livekit(m), format!("livekit/{name}"))
            }
            "oww" => {
                if rate != 16_000 {
                    return Err(format!(
                        "the openwakeword engine (oww) takes 16000 Hz only, not {rate}: its \
                         mel graph is built for 80 ms frames at 16 kHz and nothing in this \
                         pipeline resamples"
                    ));
                }
                // The threshold handed to the graph is deliberately unreachable (2.0 > any score), and its
                // VAD gate is off: `Ear` below is the single place a detection is decided, and the
                // silero export does not load under tract (see `oww.rs`).
                let inner = Oww::new(oww_dir, oww_keyword, 2.0, cooldown as f32, 16, false)
                    .map_err(|e| format!("openwakeword graphs in {}: {e}", oww_dir.display()))?;
                (
                    Engine::Oww {
                        inner,
                        left: Vec::new(),
                    },
                    format!("openwakeword/{oww_keyword}"),
                )
            }
            other => return Err(format!("unknown engine {other:?} (livekit | oww)")),
        };
        Ok(Ear {
            kind,
            rate,
            threshold,
            cooldown,
            audio: 0.0,
            last_detect: f64::NEG_INFINITY,
            name,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn audio_secs(&self) -> f64 {
        self.audio
    }

    pub fn feed(&mut self, samples: &[i16]) -> Result<Hit, String> {
        self.audio += samples.len() as f64 / self.rate as f64;
        let score = match &mut self.kind {
            Engine::Livekit(m) => {
                let scores = m.predict(samples).map_err(|e| format!("predict: {e}"))?;
                scores.values().copied().fold(0.0_f32, f32::max)
            }
            Engine::Oww { inner, left } => {
                left.extend(samples.iter().map(|&s| s as f32 / 32768.0));
                let mut peak = 0.0_f32;
                // 80 ms at 16 kHz, and the remainder is kept: a chunk boundary that landed in the
                // middle of the word must not throw the sample away or re-frame the audio.
                while left.len() >= FRAME {
                    let frame: Vec<f32> = left.drain(..FRAME).collect();
                    let step = inner.push_frame(&frame)?;
                    if step.score > peak {
                        peak = step.score;
                    }
                }
                peak
            }
        };
        let detected = score >= self.threshold && self.audio - self.last_detect >= self.cooldown;
        if detected {
            self.last_detect = self.audio;
        }
        Ok(Hit { score, detected })
    }

    /// Forget the audio heard so far. Both halves have to go — the engine's history *and* the
    /// cooldown clock — or the next chunk inherits an "already fired" timestamp.
    pub fn reset(&mut self) -> Result<(), String> {
        // Both halves: the engine's history *and* the audio clock the cooldown is measured on.
        // Leaving the clock behind is how a reset session refuses to fire for two seconds.
        self.audio = 0.0;
        self.last_detect = f64::NEG_INFINITY;
        match &mut self.kind {
            Engine::Livekit(m) => m.reset().map_err(|e| format!("reset: {e}")),
            Engine::Oww { inner, left } => {
                left.clear();
                inner.reset();
                Ok(())
            }
        }
    }
}
