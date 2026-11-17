//! Text-to-speech: one piper process per answer, and why that is not a daemon.
//!
//! The voice is piper's `en_GB-alba-medium` — a British English male voice, medium
//! quality, fetched by URL in `machines/services1/services/voice-assistant.nix`
//! alongside its `.onnx.json`, because nixpkgs packages `piper-tts` (the engine,
//! 1.8.0 at this lock) but **not** the voices, exactly as `crates/whisper-ws/package.nix`
//! has to fetch the OpenWakeWord models itself.
//!
//! ## The measured cost, which is the whole design decision
//!
//! Synthesising the answer to "is the space open" (31 words) took **2.5 s of wall
//! clock to produce 7.8 s of audio**.  Almost all of that 2.5 s is Python and
//! onnxruntime startup: piper's steady-state rate is roughly 3× real time, so the
//! synthesis itself is under 0.5 s for a one-line answer.  Two consequences, both
//! taken:
//!
//! - Spawning per request is **correct** (no daemon to supervise, no model held in
//!   RAM between answers, no state to corrupt, and the unit is a plain
//!   `DynamicUser` process) but it puts ~2 s in front of every answer.  That is
//!   acceptable against the 7.7 s the agent turn itself costs (measured,
//!   `crates/voice-assistant/README.md`) and is the first thing to fix if it ever
//!   is not — a warm piper is a protocol change, not a rewrite.
//! - We ask for **raw PCM** (`--output-raw`) rather than a WAV, so there is no need
//!   to parse a container the same process just wrote, and the sample rate comes
//!   from the voice's own config file rather than from a guess.
//!
//! piper takes the text on stdin.  Writing it *before* reading stdout is safe and is
//! what the code does, because an answer is a few hundred bytes — well under the
//! 64 KiB pipe buffer — so the child can never block on us while we block on it.  If
//! a caller ever feeds this a document, that becomes a thread; there is a test for
//! the ordering assumption in the sense that it is the documented one.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Fallback if the voice's config cannot be read.  Every `medium` piper voice is
/// 22 050 Hz; a `low` one is 16 kHz, which is why this is read rather than assumed.
pub const FALLBACK_RATE: u32 = 22_050;

#[derive(Debug, Clone)]
pub struct Voice {
    /// What the voice is called in logs and in `/status.json` — the file stem.
    pub name: String,
    pub sample_rate: u32,
}

pub struct Piper {
    pub bin: PathBuf,
    pub model: PathBuf,
    /// piper's own tempo control: > 1.0 is slower.  1.0 is the voice's design rate;
    /// 1.1 is what a room full of people talking over each other needs, and it is a
    /// flag rather than a constant because that is a judgement about this space.
    pub length_scale: f32,
    pub timeout: Duration,
}

/// One synthesised answer.
#[derive(Debug)]
pub struct Speech {
    pub pcm: Vec<i16>,
    pub voice: Voice,
    /// Seconds of audio produced.
    pub secs: f64,
    pub wall: Duration,
}

impl Piper {
    pub fn new(bin: &str, model: &str, length_scale: f32) -> Piper {
        Piper {
            bin: PathBuf::from(bin),
            model: PathBuf::from(model),
            length_scale,
            timeout: Duration::from_secs(60),
        }
    }

    /// The voice's own name and rate, read from the `<model>.json` piper expects
    /// beside the model.
    ///
    /// Reading it is the difference between a correct resample and a chipmunk: a 16
    /// kHz `low` voice played at 22.05 kHz is 38 % too fast, and nobody notices on a
    /// test clip.
    pub fn voice(&self) -> Voice {
        let name = self
            .model
            .file_name()
            .map(|s| s.to_string_lossy().trim_end_matches(".onnx").to_string())
            .unwrap_or_else(|| "voice".to_string());
        let json_path = format!("{}.json", self.model.display());
        let path = Path::new(&json_path);
        let rate = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| common_json::parse(&text).ok())
            .and_then(|v| {
                v.at(&["audio", "sample_rate"])
                    .and_then(common_json::Json::as_f64)
            })
            .map(|r| r as u32)
            .filter(|r| (8_000..=48_000).contains(r));
        match rate {
            Some(r) => Voice {
                name,
                sample_rate: r,
            },
            None => {
                eprintln!(
                    "warning: cannot read {} (piper needs it beside the model); assuming {FALLBACK_RATE} Hz",
                    path.display()
                );
                Voice {
                    name,
                    sample_rate: FALLBACK_RATE,
                }
            }
        }
    }

    /// Synthesise `text`.  An empty `text` is refused rather than answered with a
    /// click — see `crate::spoken`'s note on why silence from a ceiling speaker is
    /// worse than nothing.
    pub fn synth(&self, text: &str) -> Result<Speech, String> {
        if text.trim().is_empty() {
            return Err("nothing to speak: the answer was empty after markdown was removed".into());
        }
        let voice = self.voice();
        let started = Instant::now();
        let mut child = Command::new(&self.bin)
            .arg("-m")
            .arg(&self.model)
            .arg("--output-raw")
            .args([
                "--length-scale",
                &self.length_scale.to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                format!(
                    "cannot run {} (--piper-bin (the unit passes `${{pkgs.piper-tts}}/bin/piper` from Nix, which is not on a login PATH)): {e}",
                    self.bin.display()
                )
            })?;

        // stdin first, and closed: see the header for why this cannot deadlock here.
        {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| "piper: no stdin pipe".to_string())?;
            stdin
                .write_all(text.as_bytes())
                .and_then(|()| stdin.flush())
                .map_err(|e| format!("piper: send text: {e}"))?;
        }
        let stderr_child = child.stderr.take();
        let mut raw = Vec::new();
        child
            .stdout
            .take()
            .ok_or_else(|| "piper: no stdout pipe".to_string())?
            .read_to_end(&mut raw)
            .map_err(|e| format!("piper: read audio: {e}"))?;
        // A `wait` with a deadline.  Without one, a piper stuck inside onnxruntime
        // keeps the connection's thread forever, and a caller that retries every
        // second turns a hung subprocess into an exhausted thread pool — which looks
        // like the gateway is dead, from the outside.
        let (tx, rx) = std::sync::mpsc::channel();
        let pid = child.id();
        std::thread::spawn(move || {
            let _ = tx.send(child.wait());
        });
        let status = match rx.recv_timeout(self.timeout) {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => return Err(format!("piper: wait: {e}")),
            Err(_) => {
                // `Child::id()` on a child we stopped waiting for: kill by pid, which
                // is what `drop` would do anyway, and say so.
                eprintln!(
                    "piper: no exit after {:.0?}; killing pid {pid}",
                    self.timeout
                );
                let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
                return Err(format!(
                    "piper timed out after {:.0?} producing nothing",
                    self.timeout
                ));
            }
        };
        if !status.success() {
            let mut err = String::new();
            if let Some(mut e) = stderr_child {
                let _ = e.read_to_string(&mut err);
            }
            let last = err.trim().lines().last().unwrap_or("(no output)").to_string();
            return Err(format!("piper exited {status}: {last}"));
        }
        if raw.len() < 2 {
            return Err(format!(
                "piper produced {}/2 bytes of raw PCM: it is installed but cannot speak this \
                 text (or the model is not where the unit says it is)",
                raw.len()
            ));
        }
        let mut pcm = Vec::with_capacity(raw.len() / 2);
        for pair in raw.as_chunks::<2>().0 {
            pcm.push(i16::from_le_bytes(*pair));
        }
        let secs = pcm.len() as f64 / voice.sample_rate.max(1) as f64;
        Ok(Speech {
            pcm,
            voice,
            secs,
            wall: started.elapsed(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A stand-in for piper: reads the text on stdin, writes `bytes` of raw PCM and
    /// exits 0.  `fail` makes it exit non-zero with a message, to pin the error path.
    fn fake_piper(dir: &Path, fail: bool) -> PathBuf {
        let bin = dir.join("piper");
        let script = if fail {
            "#!/bin/sh\ncat >/dev/null\necho 'piper: boom' >&2\nexit 3\n"
        } else {
            // 16 raw bytes = 8 samples, whatever the text was.  `printf 'ab'` eight
            // times, because shell printf's octal escapes are their own compatibility
            // hazard and this test is about the pipe, not about printf.
            "#!/bin/sh\ncat >/dev/null\ni=0\nwhile [ $i -lt 8 ]; do printf ab; \
i=$((i+1)); done\nexit 0\n"
        };
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("voice-tts-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_sample_rate_is_read_from_the_voice_config() {
        let dir = tmp("rate");
        let model = dir.join("en_GB-alba-medium.onnx");
        std::fs::write(&model, b"not a model").unwrap();
        std::fs::write(
            dir.join("en_GB-alba-medium.onnx.json"),
            br#"{"audio":{"sample_rate":22050,"quality":"medium"},"language":{"code":"en_GB"}}"#,
        )
        .unwrap();
        let p = Piper::new("/bin/true", model.to_str().unwrap(), 1.0);
        let v = p.voice();
        assert_eq!(v.sample_rate, 22_050);
        assert_eq!(v.name, "en_GB-alba-medium");

        // A `low` voice is 16 kHz, and the fallback must not be used silently.
        std::fs::write(
            dir.join("en_GB-alba-medium.onnx.json"),
            br#"{"audio":{"sample_rate":16000}}"#,
        )
        .unwrap();
        assert_eq!(p.voice().sample_rate, 16_000);
        std::fs::remove_file(dir.join("en_GB-alba-medium.onnx.json")).unwrap();
        assert_eq!(p.voice().sample_rate, FALLBACK_RATE);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn raw_pcm_comes_back_as_samples_with_honest_timing() {
        let dir = tmp("synth");
        let bin = fake_piper(&dir, false);
        std::fs::write(
            dir.join("v.onnx.json"),
            br#"{"audio":{"sample_rate":16000}}"#,
        )
        .unwrap();
        let p = Piper::new(bin.to_str().unwrap(), dir.join("v.onnx").to_str().unwrap(), 1.0);
        let s = p.synth("the space is open").unwrap();
        assert_eq!(s.pcm.len(), 8, "16 raw bytes are 8 samples");
        assert_eq!(s.pcm[0], i16::from_le_bytes(*b"ab"));
        assert_eq!(s.voice.sample_rate, 16_000);
        assert!((s.secs - 0.0005).abs() < 1e-6, "{}", s.secs);
        assert!(s.wall < Duration::from_secs(10));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failing_piper_is_reported_with_its_own_last_line() {
        let dir = tmp("fail");
        let bin = fake_piper(&dir, true);
        std::fs::write(dir.join("v.onnx.json"), br#"{"audio":{"sample_rate":16000}}"#).unwrap();
        let p = Piper::new(bin.to_str().unwrap(), dir.join("v.onnx").to_str().unwrap(), 1.0);
        let e = p.synth("hello").unwrap_err();
        assert!(e.contains("boom"), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_empty_answer_is_refused_before_the_process_is_spawned() {
        let p = Piper::new("/nonexistent/piper", "/nonexistent/v.onnx", 1.0);
        assert!(p.synth("   \n ").unwrap_err().contains("nothing to speak"));
        // And a missing binary says which one, and where the switch keeps it.
        let e = p.synth("hello").unwrap_err();
        assert!(e.contains("nonexistent/piper"), "{e}");
        // Names the flag the operator can actually do something with, not an option
        // string from a module they may not have open.
        assert!(e.contains("--piper-bin"), "{e}");
    }
}
