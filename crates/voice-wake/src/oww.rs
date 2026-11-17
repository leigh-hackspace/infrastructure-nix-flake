//! `oww` — openWakeWord's three-model pipeline, run natively, for one reason: to compare it
//! honestly against `livekit-wakeword` on **identical bytes**.
//!
//! The report from the floor was that the browser's wake detection is much better than the
//! server's. That compared two different detectors (openWakeWord's `hey_jarvis_v0.1` + a
//! silero VAD gate, against LiveKit's single `hey_jarvis.onnx` with no gate) on two paths
//! that do not see the same samples (a browser AudioWorklet tap versus ~0.5 s PCM chunks over
//! HTTP). None of the three — model, gate, or runtime — was isolated, and `wasm` was
//! guessed at; wasm is how a browser gets to run ONNX at all, not a property worth
//! inheriting. So: the browser's own graphs, natively, on the same files, in one `bench`.
//!
//! The graphs come from the paths `crates/whisper-ws/package.nix` pins by `sha256` — the
//! same store paths the page fetches — which makes "identical model to the browser" a hash
//! rather than an assertion. `--oww-dir` points at that directory.
//!
//! The pipeline below is a transcription of
//! `crates/whisper-ws/web/openwakeword/engine/WakeWordEngine.js`, because that file is the
//! implementation with the field record behind it. Constants worth not silently changing, and
//! where they come from:
//!
//! * **frame = 1280 samples at 16 kHz** (80 ms) — the same framing `livekit-wakeword` uses,
//!   which is a small mercy for the comparison.
//! * **mel post-transform `x/10 + 2`**, applied to every one of the **5** mel frames the
//!   model emits per audio frame, each frame **32** wide.
//! * **embedding every 76 mel frames, consuming 8** → an input of `[1,76,32,1]` producing
//!   **96** floats.
//! * **keyword window** = 16 embeddings by default (`embeddingWindowSize`), zero-initialised,
//!   so `[1,16,96]`; the first 16 embeddings are therefore warmed, not meaningful.
//! * **silero VAD on the same 1280 samples**, confidence > 0.5, **hangover 12 frames** (≈1 s)
//!   of "speech still active" after the last positive — and a detection is only emitted while
//!   speech is active. The gate is plausibly doing real work against music and room chatter,
//!   which is exactly why it is not optional here but is reported separately.
//! * **detection = `score > threshold && speech_active && !cooldown`**.
//!
//! Cooldown is counted in **audio time**, as in `serve`: a wall-clock cooldown lets a paused
//! stream fire twice on the tail of the same utterance, and the point of this module is
//! measurement, where that error would be read as recall.

use ndarray::{Array1, Array2, Array3, Array4};
use ort::session::{Session, SessionInputValue};
use ort::value::{DynValue, Tensor};
use std::path::Path;

/// `ort` with `alternative-backend` panics on first use unless a backend has been installed;
/// `livekit-wakeword` installs the tract one, and so do we — the same call, the same backend,
/// behind a `Once` because both engines can be alive in one process.
static BACKEND: std::sync::Once = std::sync::Once::new();

/// Samples per frame, and the rate the graphs expect.
pub const FRAME: usize = 1280;
const MEL_FRAMES: usize = 5;
const MEL_WIDTH: usize = 32;
const MEL_WINDOW: usize = 76;
const MEL_STEP: usize = 8;
const EMBED: usize = 96;
const VAD_HANGOVER: usize = 12;
const VAD_THRESHOLD: f32 = 0.5;

/// What one frame produced. `score` is the last keyword score computed while feeding this
/// frame (0.0 until the embedding history has been warmed by the graph at least once).
#[derive(Debug, Default, Clone, Copy)]
pub struct Step {
    pub score: f32,
    pub speech: bool,
    pub detected: bool,
}

pub struct Oww {
    mel: Session,
    emb: Session,
    kw: Session,
    /// `None` when the gate is off — see `Oww::new`, where the reason is written down.
    vad: Option<Session>,
    /// Pending mel frames, oldest first: 32 floats each, flattened end to end.
    mel_buf: Vec<f32>,
    /// The keyword model's embedding history, oldest first.
    history: Vec<Vec<f32>>,
    window: usize,
    /// silero's LSTM state, which the graphs insist on being handed back.
    vad_h: Option<DynValue>,
    vad_c: Option<DynValue>,
    hangover: usize,
    speech: bool,
    threshold: f32,
    /// Whether a detection also requires the silero VAD to be open, which is what the browser
    /// does. Off exists to answer "is it the model or is it the gate?" separately.
    gate: bool,
    cooldown_frames: usize,
    since_detection: usize,
    frames: u64,
    last: f32,
}

impl Oww {
    /// Load the four graphs. The paths are the browser's, not a re-export of them.
    pub fn new(
        dir: &Path,
        keyword: &str,
        threshold: f32,
        cooldown_secs: f32,
        window: usize,
        gate: bool,
    ) -> Result<Self, String> {
        BACKEND.call_once(|| {
            ort::set_api(ort_tract::api());
        });
        let open = |name: &str| -> Result<Session, String> {
            let path = dir.join(name);
            Session::builder()
                .map_err(|e| format!("ort builder: {e}"))?
                .commit_from_file(&path)
                .map_err(|e| format!("{name}: {e}"))
        };
        let mut me = Oww {
            mel: open("melspectrogram.onnx")?,
            emb: open("embedding_model.onnx")?,
            kw: open(&format!("{keyword}.onnx"))?,
            // **Only load silero when the gate is on**, because this export of it cannot be
            // loaded by tract at all: `Failed to parse model: Translating node #72 "If_25" If`
            // — the graph uses the ONNX `If` op (control flow), which onnxruntime-web runs and
            // tract does not translate. Measured 2026-11-17, and it is the honest answer to
            // "just run the browser's pipeline natively": there is an op-support gap, in the
            // *gate*, not a speed gap in the runtime. So `--gate off` skips the graph entirely
            // and measures the wake model on its own.
            vad: if gate { Some(open("silero_vad.onnx")?) } else { None },
            mel_buf: Vec::new(),
            history: Vec::new(),
            window,
            vad_h: None,
            vad_c: None,
            hangover: 0,
            speech: false,
            threshold,
            gate,
            cooldown_frames: ((cooldown_secs * 12.5) as usize).max(1),
            since_detection: usize::MAX,
            frames: 0,
            last: 0.0,
        };
        me.history = vec![vec![0.0f32; EMBED]; me.window];
        // silero wants its state tensors shaped [2,1,64]; the first call has none, so make
        // zeros of the shape the graph itself declares rather than hardcoding it.
        // silero keeps its LSTM state in two [2,1,64] tensors.
        let z = || -> Result<DynValue, String> {
            Tensor::from_array(Array3::<f32>::from_elem((2, 1, 64), 0.0f32))
                .map(|t| t.into_dyn())
                .map_err(|e| format!("state zeros: {e}"))
        };
        me.vad_h = Some(z()?);
        me.vad_c = Some(z()?);
        Ok(me)
    }

    /// Audio seconds processed so far, for reporting.
    pub fn audio_secs(&self) -> f64 {
        self.frames as f64 * FRAME as f64 / 16_000.0
    }

    /// Feed one 1280-sample frame of float audio in −1.0..=1.0.
    pub fn push_frame(&mut self, frame: &[f32]) -> Result<Step, String> {
        if frame.len() != FRAME {
            return Err(format!("openwakeword frames are {FRAME} samples, got {}", frame.len()));
        }
        self.frames += 1;
        self.since_detection = self.since_detection.saturating_add(1);

        let speech_now = if self.gate { self.vad(frame)? } else { true };
        if speech_now {
            self.speech = true;
            self.hangover = VAD_HANGOVER;
        } else if self.speech {
            self.hangover = self.hangover.saturating_sub(1);
            if self.hangover == 0 {
                self.speech = false;
            }
        }

        let score = self.score_frame(frame)?;
        self.last = score;
        let detected = score > self.threshold
            && (!self.gate || self.speech)
            && self.since_detection > self.cooldown_frames;
        if detected {
            self.since_detection = 0;
        }
        Ok(Step { score, speech: self.speech, detected })
    }

    fn vad(&mut self, frame: &[f32]) -> Result<bool, String> {
        let input = tensor2(frame)?;
        let vad = self.vad.as_mut().ok_or("vad graph not loaded (--gate off)")?;
        let sr = Tensor::from_array(Array1::<i64>::from_vec(vec![16_000]))
            .map(|t| t.into_dyn())
            .map_err(|e| format!("sr: {e}"))?;
        let h = self.vad_h.take().ok_or("vad h state missing")?;
        let c = self.vad_c.take().ok_or("vad c state missing")?;
        let inputs: Vec<SessionInputValue> =
            vec![input.into(), sr.into(), h.into(), c.into()];
        let mut out = vad
            .run(inputs.as_slice())
            .map_err(|e| format!("silero: {e}"))?;
        // The state comes back as `hn`/`cn`; the confidence as `output`. Names are looked up
        // because silero has shipped these graphs with more than one spelling, and a silent
        // wrong index here would look like a detector that never fires.
        let take = |out: &mut ort::session::SessionOutputs, name: &str| -> Option<DynValue> {
            out.remove(name)
        };
        let conf = take(&mut out, "output")
            .or_else(|| take(&mut out, "State0"))
            .ok_or("silero: no confidence output")?
            .try_extract_array::<f32>()
            .map(|a| *a.iter().next().unwrap_or(&0.0))
            .map_err(|e| format!("silero confidence: {e}"))?;
        self.vad_h = take(&mut out, "hn").or_else(|| take(&mut out, "State1"));
        self.vad_c = take(&mut out, "cn").or_else(|| take(&mut out, "State2"));
        if self.vad_h.is_none() || self.vad_c.is_none() {
            return Err("silero: no state outputs to hand back".into());
        }
        Ok(conf > VAD_THRESHOLD)
    }

    fn score_frame(&mut self, frame: &[f32]) -> Result<f32, String> {
        let input = tensor2(frame)?;
        let mut out = self
            .mel
            .run([input.into()].as_slice())
            .map_err(|e| format!("melspectrogram: {e}"))?;
        let mel = out[0]
            .try_extract_array::<f32>()
            .map_err(|e| format!("mel extract: {e}"))?;
        let mut flat: Vec<f32> = mel.iter().copied().collect();
        let mel_shape: Vec<usize> = mel.shape().to_vec();
        drop(mel);
        // A silent zero here would look like "the model never fires", which is a conclusion
        // about a detector rather than about our plumbing — so the shape is checked and the
        // mismatch is an error naming what actually arrived.
        if flat.len() != MEL_FRAMES * MEL_WIDTH {
            return Err(format!(
                "melspectrogram produced {:?} ({} floats); this pipeline expects {} floats \
                 ({MEL_FRAMES} frames of {MEL_WIDTH})",
                mel_shape,
                flat.len(),
                MEL_FRAMES * MEL_WIDTH
            ));
        }
        for v in flat.iter_mut() {
            *v = *v / 10.0 + 2.0;
        }
        // 5 frames of 32 per audio frame, appended.
        for j in 0..MEL_FRAMES {
            let start = j * MEL_WIDTH;
            if start + MEL_WIDTH <= flat.len() {
                self.mel_buf.extend_from_slice(&flat[start..start + MEL_WIDTH]);
            }
        }

        let mut score = 0.0;
        while self.mel_buf.len() >= MEL_WINDOW * MEL_WIDTH {
            let window: Vec<f32> = self.mel_buf[..MEL_WINDOW * MEL_WIDTH].to_vec();
            let emb_in = Tensor::from_array(
                Array4::from_shape_vec((1, MEL_WINDOW, MEL_WIDTH, 1), window)
                    .map_err(|e| format!("embedding shape: {e}"))?,
            )
            .map_err(|e| format!("embedding tensor: {e}"))?
            .into_dyn();
            let mut eout = self
                .emb
                .run([emb_in.into()].as_slice())
                .map_err(|e| format!("embedding: {e}"))?;
            let embedding: Vec<f32> = eout[0]
                .try_extract_array::<f32>()
                .map_err(|e| format!("embedding extract: {e}"))?
                .iter()
                .copied()
                .collect();
            if embedding.len() != EMBED {
                return Err(format!(
                    "embedding produced {} floats, the keyword graph expects {EMBED}",
                    embedding.len()
                ));
            }

            self.history.remove(0);
            self.history.push(embedding);
            let mut stacked = Vec::with_capacity(self.window * EMBED);
            for h in &self.history {
                stacked.extend_from_slice(h);
            }
            let kw_in = Tensor::from_array(
                Array3::from_shape_vec((1, self.window, EMBED), stacked)
                    .map_err(|e| format!("keyword shape (try --oww-window): {e}"))?,
            )
            .map_err(|e| format!("keyword tensor: {e}"))?
            .into_dyn();
            let mut kout = self
                .kw
                .run([kw_in.into()].as_slice())
                .map_err(|e| format!("keyword: {e}"))?;
            score = kout[0]
                .try_extract_array::<f32>()
                .map_err(|e| format!("keyword extract: {e}"))?
                .iter()
                .next()
                .copied()
                .unwrap_or(0.0);

            self.mel_buf.drain(..MEL_STEP * MEL_WIDTH);
        }
        Ok(score)
    }
}

fn tensor2(frame: &[f32]) -> Result<SessionInputValue, String> {
    Tensor::from_array(
        ndarray::Array2::from_shape_vec((1, FRAME), frame.to_vec())
            .map_err(|e| format!("shape: {e}"))?,
    )
    .map(|t| t.into_dyn().into())
    .map_err(|e| format!("tensor: {e}"))
}
