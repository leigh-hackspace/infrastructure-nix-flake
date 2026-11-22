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

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::wav;

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
        crate::testkit::write_exec(&bin, script);
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

// ---------------------------------------------------------------------------
// The resident voice: one piper, kept warm, fed one sentence at a time.
// ---------------------------------------------------------------------------

/// What [`Resident`] reports about the process it is holding.
#[derive(Debug, Default, Clone)]
pub struct TtsStats {
    /// Sentences synthesised by the warm process.
    pub warm: u64,
    /// Sentences that went to a cold one-shot piper instead, and why (see `last_fallback`).
    pub cold: u64,
    pub restarts: u64,
    pub last_fallback: String,
    /// Milliseconds of the most recent synthesis, so `status.json` can show the difference
    /// this makes rather than assert it.
    pub last_ms: u128,
}

/// A piper that stays up.
///
/// The one-shot path is 2.5 s for a 31-word answer of which about 2.4 s is Python and
/// onnxruntime loading the voice. Per-sentence synthesis — which is what a *streamed* answer
/// needs, so the room can hear sentence one while the model is on sentence three — would pay
/// that startup once per sentence, i.e. the streaming version of the pipeline would be slower
/// than the buffered one. Keeping the process up is the whole reason streaming the voice out is
/// a win rather than a curiosity.
///
/// ## The protocol, which is piper's own CLI and nothing else
///
/// ```text
///   piper -m MODEL --output-dir DIR --output-dir-naming timestamp
///     ← one sentence per line on stdin, held open
///     → one WAV per line in DIR, in the order they were given
///     → `INFO:__main__:Wrote DIR/<ns>.wav` on stderr, one per WAV, after that WAV is closed
/// ```
///
/// The stderr line is the **frame boundary**: with `--output-raw` piper concatenates everything
/// on stdout with no delimiter at all, so there is no way to tell where sentence one's audio ends
/// and sentence two's begins, and a stream you cannot frame is a stream you cannot time out or
/// restart cleanly. `Wrote` is logged after the `with wav_file:` block closes the file, so a
/// path that has been reported is a path that is complete on disk.
pub struct Resident {
    stdin: std::process::ChildStdin,
    /// Paths as they are reported, in order.
    written: mpsc::Receiver<PathBuf>,
    /// The last non-`Wrote` stderr line, which after a failure is the explanation.
    noise: Arc<Mutex<String>>,
    child: Child,
    dir: PathBuf,
    voice: Voice,
    timeout: Duration,
}

impl Resident {
    pub fn spawn(bin: &Path, model: &Path, length_scale: f32, dir: &Path) -> Result<Resident, String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create the piper output dir {}: {e}", dir.display()))?;
        let mut child = Command::new(bin)
            .arg("-m")
            .arg(model)
            .arg("--output-dir")
            .arg(dir)
            .arg("--output-dir-naming")
            .arg("timestamp")
            .arg("--length-scale")
            .arg(length_scale.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                format!(
                    "cannot start the resident piper ({}) — the unit passes --piper-bin from Nix, \
                     which is not on a login PATH: {e}",
                    bin.display()
                )
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "piper: no stdin pipe".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "piper: no stderr pipe".to_string())?;
        let (tx, rx) = mpsc::channel();
        let noise = Arc::new(Mutex::new(String::new()));
        let noise_thread = Arc::clone(&noise);
        thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let t = line.trim_end();
                if t.is_empty() {
                    continue;
                }
                // `INFO:__main__:Wrote /tmp/x/17…wav` — the last field is the path, and taking
                // it by split rather than by regex is deliberate: piper's log format is a detail
                // of a pinned wheel, and this is the one thing about it this code depends on.
                match t.rsplit_once("Wrote ") {
                    Some((_, path)) => {
                        if tx.send(PathBuf::from(path.trim())).is_err() {
                            break;
                        }
                    }
                    None => {
                        let mut n = noise_thread.lock().unwrap_or_else(|p| p.into_inner());
                        *n = t.to_string();
                    }
                }
            }
        });
        let voice = Piper::new(&bin.display().to_string(), &model.display().to_string(), length_scale).voice();
        Ok(Resident {
            stdin,
            written: rx,
            noise,
            child,
            dir: dir.to_path_buf(),
            voice,
            timeout: Duration::from_secs(30),
        })
    }

    pub fn voice(&self) -> &Voice {
        &self.voice
    }

    /// Whether the process is still there. Checked before a sentence rather than after, because
    /// writing to a dead stdin is a broken-pipe whose cause is three lines away.
    pub fn is_up(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                let noise = self.noise.lock().unwrap_or_else(|p| p.into_inner()).clone();
                eprintln!("resident piper exited {status} (last output: {noise})");
                false
            }
            Err(e) => {
                eprintln!("cannot tell whether the resident piper is up: {e}");
                false
            }
        }
    }

    /// Speak one sentence, and hand back its PCM at the voice's own rate.
    pub fn speak(&mut self, text: &str) -> Result<Speech, String> {
        if text.trim().is_empty() {
            return Err("nothing to speak".to_string());
        }
        let started = Instant::now();
        writeln!(self.stdin, "{text}")
            .and_then(|()| self.stdin.flush())
            .map_err(|e| format!("send to resident piper: {e} (it is probably gone)"))?;
        let path = self
            .written
            .recv_timeout(self.timeout)
            .map_err(|e| {
                let noise = self.noise.lock().unwrap_or_else(|p| p.into_inner()).clone();
                match e {
                    mpsc::RecvTimeoutError::Timeout => format!(
                        "resident piper produced nothing for {:.0?} (last output: {noise})",
                        self.timeout
                    ),
                    mpsc::RecvTimeoutError::Disconnected => {
                        format!("resident piper died (last output: {noise})")
                    }
                }
            })?;
        let bytes = std::fs::read(&path)
            .map_err(|e| format!("piper said it wrote {}: {e}", path.display()))?;
        // Remove it as we read it: this directory is inside the unit's state dir, and a
        // long-running gateway that never deletes would fill the disk one sentence at a time.
        let _ = std::fs::remove_file(&path);
        let file = wav::parse(&bytes).map_err(|e| format!("piper's WAV: {e}"))?;
        let pcm = file.mono();
        let secs = pcm.len() as f64 / self.voice.sample_rate.max(1) as f64;
        Ok(Speech {
            pcm,
            voice: self.voice.clone(),
            secs,
            wall: started.elapsed(),
        })
    }

    /// Kill it. Called on the way out and before every restart.
    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Its frame directory too: a gateway that speaks hourly would otherwise leave a decade of
        // old sentences under the state directory, one directory per voice, each holding the last
        // WAV it wrote.
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Drop for Resident {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The voice every answer goes through: warm when it can be, one-shot when it cannot, and
/// **never** a failed answer because of it.
///
/// The fallback exists because the warm process is a Python interpreter holding a 60 MB model on
/// a box that also runs PostgreSQL and the outposts. If it is OOM-killed at 03:00, the room still
/// gets its answer 2.4 s later than it would have, and the journal says which path was used.
pub struct Synthesiser {
    once: Piper,
    resident: Option<Resident>,
    dir: PathBuf,
    length_scale: f32,
    pub stats: TtsStats,
}

impl Synthesiser {
    pub fn new(bin: &str, model: &str, length_scale: f32, dir: &Path) -> Synthesiser {
        Synthesiser {
            once: Piper::new(bin, model, length_scale),
            resident: None,
            dir: dir.to_path_buf(),
            length_scale,
            stats: TtsStats::default(),
        }
    }

    /// The voice, from whichever piper is available: the resident one if it is up, else the
    /// model's own config. Same file either way, so the sample rate cannot disagree with itself.
    pub fn voice(&self) -> Voice {
        match &self.resident {
            Some(r) => r.voice().clone(),
            None => self.once.voice(),
        }
    }

    /// Whether the warm process is up **right now**, without starting one. For a status file:
    /// "the next answer will be quick" is a different question from "start making it quick".
    pub fn warm_live(&self) -> bool {
        self.resident.is_some()
    }

    pub fn warm(&mut self) -> bool {
        let up = match self.resident.as_mut() {
            Some(r) => r.is_up(),
            None => false,
        };
        if up {
            return true;
        }
        if self.resident.is_some() {
            self.stats.restarts += 1;
            self.resident = None;
        }
        match Resident::spawn(
            &self.once.bin.clone(),
            &self.once.model.clone(),
            self.length_scale,
            &self.dir,
        ) {
            Ok(r) => {
                self.resident = Some(r);
                true
            }
            Err(e) => {
                self.stats.last_fallback = e;
                false
            }
        }
    }

    /// Synthesise one sentence, warm if possible.  `Err` means **both** paths failed, and the
    /// message says which one did what.
    pub fn say(&mut self, text: &str) -> Result<Speech, String> {
        if self.warm() {
            if let Some(r) = self.resident.as_mut() {
                match r.speak(text) {
                    Ok(s) => {
                        self.stats.warm += 1;
                        self.stats.last_ms = s.wall.as_millis();
                        return Ok(s);
                    }
                    Err(e) => {
                        self.stats.last_fallback = e.clone();
                        eprintln!("resident piper failed, falling back to a cold start: {e}");
                        self.stats.restarts += 1;
                        self.resident = None;
                    }
                }
            }
        }
        let speech = self.once.synth(text)?;
        self.stats.cold += 1;
        self.stats.last_ms = speech.wall.as_millis();
        Ok(speech)
    }
}

/// One synthesised answer's samples, at the only rate anything downstream wants.
///
/// Three callers needed `resample(pcm, voice.rate, 16000)` and the mistake of writing it
/// slightly differently in one of them is a chipmunk voice or a slowed-down one, so it is here.
pub fn pcm_at_16k(speech: &Speech) -> Vec<i16> {
    if speech.voice.sample_rate == wav::SAMPLE_RATE {
        return speech.pcm.clone();
    }
    wav::resample(&speech.pcm, speech.voice.sample_rate, wav::SAMPLE_RATE)
}

#[cfg(test)]
mod resident_tests {
    use super::*;

    /// A stand-in for piper's `--output-dir` mode: reads a line, writes a WAV whose name is the
    /// line number, and logs the `Wrote` line exactly the way piper does — including the
    /// `INFO:__main__:` prefix, because that prefix is why this is matched by `split` and not
    /// parsed as a whole line.
    fn fake_resident_piper(dir: &Path, die_after: usize) -> PathBuf {
        let bin = dir.join("piper");
        let script = r#"#!/bin/sh
dir=""
raw=0
while [ $# -gt 0 ]; do
  case "$1" in
    --output-dir) dir="$2"; shift 2;;
    --output-raw) raw=1; shift;;
    *) shift;;
  esac
done
# The one-shot path (`--output-raw`) and the resident path (`--output-dir`) are the two things
# this crate asks of piper, so the fixture answers both, with the same 32 samples either way.
if [ "$raw" -eq 1 ]; then
  IFS= read -r line
  j=0; while [ $j -lt 32 ]; do printf '\001\000'; j=$((j+1)); done
  exit 0
fi
mkdir -p "$dir"
i=0
while IFS= read -r line; do
  i=$((i+1))
  if [ "$i" -gt __DIE__ ]; then
    echo "Traceback: simulated crash on $line" >&2
    exit 7
  fi
  printf 'RIFFd\000\000\000WAVEfmt \020\000\000\000\001\000\001\000\200>\000\000\000}\000\000\002\000\020\000data@\000\000\000' > "$dir/$i.wav"
  j=0; while [ $j -lt 32 ]; do printf '\001\000' >> "$dir/$i.wav"; j=$((j+1)); done
  echo "INFO:__main__:Wrote $dir/$i.wav" >&2
done
"#.replace("__DIE__", &die_after.to_string());
        crate::testkit::write_exec(&bin, &script);
        bin
    }

    fn voice_dir(dir: &Path) -> PathBuf {
        let model = dir.join("en_GB-alba-medium.onnx");
        std::fs::write(&model, b"not a model").unwrap();
        std::fs::write(
            dir.join("en_GB-alba-medium.onnx.json"),
            br#"{"audio":{"sample_rate":16000}}"#,
        )
        .unwrap();
        model
    }

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("voice-resident-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn one_process_speaks_several_sentences_in_order() {
        let dir = tmp("warm");
        let bin = fake_resident_piper(&dir, 99);
        let model = voice_dir(&dir);
        let mut r = Resident::spawn(&bin, &model, 1.05, &dir.join("out")).unwrap();
        assert_eq!(r.voice().sample_rate, 16_000);
        let a = r.speak("first sentence").unwrap();
        let b = r.speak("second sentence").unwrap();
        // 44-byte header + 32 samples of payload, whatever the text was.
        assert_eq!(a.pcm.len(), 32);
        assert_eq!(b.pcm.len(), 32);
        assert_eq!((a.voice.sample_rate, b.voice.sample_rate), (16_000, 16_000));
        assert!((a.secs - 0.002).abs() < 1e-9, "{}", a.secs);
        // Each WAV is read and removed: the state directory must not grow one sentence at a time.
        assert_eq!(
            std::fs::read_dir(dir.join("out")).unwrap().count(),
            0,
            "synthesis files were left behind"
        );
        drop(r);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dead_voice_is_reported_as_dead_and_names_what_it_said_last() {
        let dir = tmp("dead");
        let bin = fake_resident_piper(&dir, 1);
        let model = voice_dir(&dir);
        let mut r = Resident::spawn(&bin, &model, 1.05, &dir.join("out")).unwrap();
        r.speak("fine").unwrap();
        let e = r.speak("boom").unwrap_err();
        assert!(e.contains("crash") || e.contains("died") || e.contains("nothing"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The point of the whole module: a warm process for sentence two, and a cold one for
    /// sentence one hundred when the box has eaten the voice at 03:00.
    #[test]
    fn the_synthesiser_falls_back_to_a_cold_piper_and_says_so() {
        let dir = tmp("fallback");
        let bin = fake_resident_piper(&dir, 1);
        let model = voice_dir(&dir);
        let mut s = Synthesiser::new(bin.to_str().unwrap(), model.to_str().unwrap(), 1.05, &dir.join("out"));
        assert!(s.warm(), "the resident process did not start");
        s.say("one").unwrap();
        // The second sentence kills the fake; the synthesiser must still answer, cold.
        s.say("two").unwrap();
        assert_eq!(s.stats.warm, 1);
        assert_eq!(s.stats.cold, 1, "no cold fallback happened: {:?}", s.stats);
        assert!(s.stats.last_fallback.contains("crash"), "{:?}", s.stats.last_fallback);
        // And it tries to be warm again for the next one, rather than staying cold forever.
        s.say("three").unwrap();
        assert!(s.stats.warm >= 2 || s.stats.cold >= 2, "{:?}", s.stats);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_piper_is_a_startup_failure_the_message_can_act_on() {
        let dir = tmp("missing");
        let mut s = Synthesiser::new(
            "/nonexistent/piper",
            "/nonexistent/v.onnx",
            1.0,
            &dir.join("out"),
        );
        assert!(!s.warm());
        assert!(s.stats.last_fallback.contains("--piper-bin"), "{}", s.stats.last_fallback);
        let e = s.say("hello").unwrap_err();
        assert!(e.contains("nonexistent/piper"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_voice_that_already_is_16k_is_not_resampled_and_one_that_is_not_is() {
        let speech = |rate: u32, n: usize| Speech {
            pcm: vec![0i16; n],
            voice: Voice {
                name: "v".into(),
                sample_rate: rate,
            },
            secs: 0.0,
            wall: Duration::from_millis(1),
        };
        assert_eq!(pcm_at_16k(&speech(16_000, 10)).len(), 10);
        // 22 050 → 16 000 is the deployed direction, and it must actually shrink.
        assert!(pcm_at_16k(&speech(22_050, 2205)).len() < 2205);
    }
}
