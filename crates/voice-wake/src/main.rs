//! `voice-wake` — measure a streaming wake-word engine against *our* audio.
//!
//! The always-on ear in the voice tier.  Today `crates/voice-assistant`'s satellite cuts
//! utterances out of cam8's stream with an energy gate and sends **every** one of them to
//! Whisper (~900 ms of CPU per audio-second on aibox's 8 threads), then searches the
//! *transcript* for a wake phrase.  That is an ASR call per sentence the room happens to
//! say; a wake-word detector in front of it turns that into an ASR call per request, which
//! is the difference between "one microphone per core" and "many".
//!
//! This binary is the spike in front of the daemon, and it exists because the decision is a
//! threshold decision: an always-on detector in a hackspace is judged by how often it
//! interrupts strangers, and that number has to be measured in this room rather than
//! inherited from a vendor's README.  So it reports the three things that actually settle
//! it:
//!
//! * **peak score** per clip — what the detector thought of it;
//! * **detections per hour** of real room audio — the false-accept rate we would live with;
//! * **milliseconds of CPU per second of audio**, read out of `/proc/self/stat` rather than
//!   inferred from wall time, because that is the number that says how many streams a box
//!   holds (and a wall-time figure moves whenever the runtime changes its thread count,
//!   which is precisely what we are trying to find out).
//!
//! The engine is `livekit-wakeword` (Apache-2.0): one streaming `.onnx` per phrase, 80 ms
//! steps, per-model state owned by the crate, and the same model files the browser client
//! scores with.  Nothing in here is a model — it is a harness.
//!
//! ```text
//! voice-wake score <model.onnx> <clip.wav> [--threshold T]
//! voice-wake bench <model.onnx> <clip.wav|dir>… [--threshold T] [--cooldown S]
//! ```
//!
//! `bench` is deliberately stateful **across files** — one model, no reset between them —
//! because a false accept in hour three of a quiet evening is exactly what `score` on one
//! clip cannot show.  A detector that resets itself between clips is not the detector that
//! runs all day.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hound::{SampleFormat, WavReader};
use livekit_wakeword::WakeWordModel;

mod ear;
mod engines;
mod oww;
mod owwbench;
mod serve;

/// Samples in one 80 ms model step at 16 kHz.
///
/// Feeding exactly one step per `predict` call is what makes a detection timestamp mean
/// something: the crate returns the *maximum* score among the steps a call completed, so a
/// bigger chunk blurs that moment away.
pub(crate) const STEP_SAMPLES: usize = 1280;
const STEP_SECS: f64 = 0.080;

/// One input clip, already checked to be mono 16-bit PCM.
struct Clip {
    name: String,
    reader: WavReader<std::io::BufReader<std::fs::File>>,
    rate: u32,
}

struct Args {
    verb: String,
    model: PathBuf,
    files: Vec<PathBuf>,
    threshold: f32,
    cooldown: f64,
    /// Samples per `predict` call.  The default is one 80 ms step, which is what a
    /// streaming server gets from a network device; a bigger number measures how much of
    /// the cost is per-call overhead rather than inference, and the two must not be
    /// confused when the figure is quoted.
    chunk: usize,
}

const USAGE: &str = "\
voice-wake — streaming wake-word detection over WAV files, with the numbers that matter

  voice-wake serve  --model <model.onnx>        loopback HTTP scoring service (see README)
  voice-wake score <model.onnx> <clip.wav>      one clip, a line per step above 0.05
  voice-wake bench <model.onnx> <clip|dir>…     many clips, one model, no reset between
  --threshold T     score at or above which a step counts as a detection (default 0.5)
  --cooldown S      ignore further detections for S seconds (default 2.0, like OWW)
  --chunk N         samples per predict call (default 1280 = one 80 ms step, i.e. what a
                    network device actually delivers; raise it to separate per-call
                    overhead from inference cost)

Inputs must be mono 16-bit PCM WAVs — which is what `voice-assistant probe` writes and what
the camera's own stream carries.  The rate of the first file is the rate advertised to the
model; mixed rates in one run are refused rather than quietly resampled twice.

Published models (`gh release download -R livekit/livekit-wakeword -p '*.onnx'`):
hey_jarvis.onnx.  A file exported by `livekit-wakeword train` for another phrase works the
same way; v1/openWakeWord classifier files do not, and say so.";

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // The A/B verb for the openWakeWord pipeline (see `owwbench`). It exits here rather than
    // joining the score/bench plumbing on purpose: the numbers in `README.md` came out of
    // `bench`, so `bench` must keep producing exactly those.
    if argv.first().is_some_and(|a| a == "oww-bench") {
        std::process::exit(owwbench::run(&argv[1..]));
    }
    if argv.first().is_some_and(|a| a == "serve") {
        let opts = match serve_options(&argv[1..]) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("voice-wake serve: {e}\n");
                                eprintln!("usage: voice-wake serve --model <hey_jarvis.onnx> [--listen 127.0.0.1:8195]");
                eprintln!("                         [--threshold T] [--cooldown S] [--idle S] [--max-sessions N]");
                eprintln!("                         serve also takes [--engine livekit|oww] [--oww-models DIR]");
                eprintln!("                         [--oww-keyword NAME] [--oww-threshold T]: the engine flag sets the");
                eprintln!("                         DEFAULT for a session; each POST /v1/session may name its own");
                eprintln!("                         (livekit | oww | both), and both scores one stream twice.");
                eprintln!("                         [--engine livekit|oww] [--oww-models <dir>] [--oww-keyword NAME]");
                eprintln!("                         [--record-dir <dir>]   (writes what a session was fed; room audio)");
                std::process::exit(2);
            }
        };
        match serve::serve(opts) {
            Ok(()) => return,
            Err(e) => {
                eprintln!("voice-wake serve: {e}");
                std::process::exit(1);
            }
        }
    }

    let args = match parse(argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("voice-wake: {e}\n");
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    if args.verb == "help" {
        println!("{USAGE}");
        return;
    }

    let mut clips = Vec::new();
    for (i, path) in args.files.iter().enumerate() {
        match open_clip(path) {
            Ok(c) => clips.push(c),
            Err(e) => {
                eprintln!("voice-wake: {}: {e}", path.display());
                // A bench run with nothing readable in it must not report zero false
                // accepts, which would read as a pass.
                if i == args.files.len() - 1 && clips.is_empty() {
                    std::process::exit(1);
                }
            }
        }
    }

    let rate = clips[0].rate;
    let started = Instant::now();
    let model = match WakeWordModel::new(std::slice::from_ref(&args.model), rate) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "voice-wake: cannot load {} as a streaming model: {e}\n\
                 (only the streaming export is accepted — not v1 or openWakeWord classifiers)",
                args.model.display()
            );
            std::process::exit(1);
        }
    };
    println!(
        "model {} · {:?} · input {} Hz · {} ms steps · loaded {:.2}s",
        args.model.display(),
        model.model_names().collect::<Vec<_>>(),
        rate,
        (STEP_SECS * 1000.0) as u32,
        started.elapsed().as_secs_f64()
    );

    std::process::exit(measure(model, &mut clips, &args));
}

fn open_clip(path: &Path) -> Result<Clip, String> {
    let reader = WavReader::open(path).map_err(|e| format!("cannot read: {e}"))?;
    let spec = reader.spec();
    if spec.channels != 1 {
        return Err(format!(
            "{} channels — this harness takes mono (the camera and the browser are both mono)",
            spec.channels
        ));
    }
    if !matches!(spec.sample_format, SampleFormat::Int) || spec.bits_per_sample != 16 {
        return Err(format!(
            "{}-bit {:?} — needs 16-bit PCM",
            spec.bits_per_sample, spec.sample_format
        ));
    }
    let rate = spec.sample_rate;
    if rate < 8_000 {
        return Err(format!(
            "{rate} Hz — too low for an 80 ms step to see anything"
        ));
    }
    Ok(Clip {
        name: path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string()),
        reader,
        rate,
    })
}

fn measure(mut model: WakeWordModel, clips: &mut [Clip], args: &Args) -> i32 {
    let verbose = args.verb == "score";
    let cpu0 = cpu_ms();
    let mut audio_secs = 0.0_f64;
    let mut step_ms: Vec<f64> = Vec::new();
    let mut peaks: BTreeMap<String, f32> = BTreeMap::new();
    let mut detections: Vec<(f64, String, f32)> = Vec::new();
    let mut last_detect = f64::NEG_INFINITY;
    let mut buf: Vec<i16> = Vec::with_capacity(STEP_SAMPLES);

    for clip in clips.iter_mut() {
        let mut pos = 0usize;
        let mut clip_peak = 0.0_f32;
        let mut iter = clip.reader.samples::<i16>();
        loop {
            let n = fill(&mut iter, &mut buf, args.chunk);
            if n == 0 {
                break;
            }
            let t0 = Instant::now();
            let scores = match model.predict(&buf[..n]) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("voice-wake: predict failed after {audio_secs:.1}s of audio: {e}");
                    return 1;
                }
            };
            // Only a full step counts toward the step-cost percentiles: a trailing partial
            // step is cheaper in samples and would drag p50 down with a number that means
            // nothing.
            if n == args.chunk {
                step_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
            }
            // Timestamp of this step's *end*, in clip time: the moment a person would say
            // they finished the phrase, which is what detection latency is measured against.
            let at = (pos + n) as f64 / clip.rate as f64;
            pos += n;
            audio_secs += n as f64 / clip.rate as f64;

            let mut step_max = 0.0_f32;
            for (name, score) in scores {
                step_max = step_max.max(score);
                let e = peaks.entry(name.clone()).or_insert(0.0);
                if score > *e {
                    *e = score;
                }
                if score > clip_peak {
                    clip_peak = score;
                }
                if score >= args.threshold && at - last_detect >= args.cooldown {
                    last_detect = at;
                    detections.push((at, name.clone(), score));
                    eprintln!("  detection {:7.2}s  {name} {score:.3}", at);
                }
            }
            if verbose && step_max >= 0.05 {
                println!("  {:6.2}s  {:.3}", at - STEP_SECS, step_max);
            }
        }
        if !verbose {
            println!("  {:<62} peak {:.3}", clip.name, clip_peak);
        }
    }

    let cpu = cpu_ms() - cpu0;
    step_ms.sort_by(|a, b| a.total_cmp(b));
    let pct = |q: f64| {
        step_ms
            .get(((step_ms.len() as f64) * q) as usize)
            .copied()
            .unwrap_or(0.0)
    };

    println!("---");
    println!(
        "audio               {:.1}s over {} file(s), {} steps",
        audio_secs,
        clips.len(),
        step_ms.len()
    );
    println!(
        "peak by model       {}",
        peaks
            .iter()
            .map(|(k, v)| format!("{k} {v:.3}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "step cost           p50 {:.2}ms · p95 {:.2}ms · max {:.2}ms",
        pct(0.50),
        pct(0.95),
        step_ms.last().copied().unwrap_or(0.0)
    );
    if audio_secs > 0.0 {
        let per_sec = cpu / audio_secs;
        // ms of CPU per second of audio IS the core fraction directly: 1000 ms of CPU per
        // 1000 ms of audio is one core saturated.  Getting this factor wrong by two orders
        // of magnitude is how a 4 %-of-a-core figure gets quoted as 400 %.
        println!(
            "cpu                 {:.1} ms per second of audio = {:.1}% of one core{} ({} samples per call)",
            per_sec,
            per_sec / 10.0,
            if per_sec > 0.0 {
                format!(" · ≈{} streams per core", (1000.0 / per_sec) as u32)
            } else {
                String::new()
            },
            args.chunk
        );
        if audio_secs < 60.0 {
            println!(
                "detections          {} at threshold {:.2} over {:.1}s — too little audio for a rate. \
                 False accepts are per hour of real room noise, so bench a busy hour.",
                detections.len(),
                args.threshold,
                audio_secs
            );
        } else {
            println!(
                "false accepts       {} detection(s) at threshold {:.2} → {:.2}/hour of this audio",
                detections.len(),
                args.threshold,
                detections.len() as f64 / (audio_secs / 3600.0)
            );
        }
    }
    if detections.is_empty() {
        println!(
            "note: zero detections is a result only if a wake word is present in this audio.\n\
             false accepts scale with the room, so run this over the busiest hour you have;\n\
             recall needs clips of the phrase at the level the device actually hears."
        );
    }
    0
}

/// Fill `buf` with up to one step of samples; returns how many.
///
/// The short final partial step is fed rather than dropped — the crate keeps the remainder
/// for the next call, and discarding it would leave the tail of every clip unmeasured,
/// which is where the end of a wake phrase tends to sit.
fn fill<I: Iterator<Item = Result<i16, hound::Error>>>(
    iter: &mut I,
    buf: &mut Vec<i16>,
    chunk: usize,
) -> usize {
    buf.clear();
    while buf.len() < chunk {
        match iter.next() {
            Some(Ok(s)) => buf.push(s),
            Some(Err(e)) => {
                eprintln!("voice-wake: sample error: {e}");
                break;
            }
            None => break,
        }
    }
    buf.len()
}

/// CPU time (user + system) of this process in milliseconds, from `/proc/self/stat`.
fn cpu_ms() -> f64 {
    let Ok(stat) = std::fs::read_to_string("/proc/self/stat") else {
        return 0.0;
    };
    // `comm` sits in parentheses and may contain spaces, so count from the closing paren:
    // utime is the 12th field after it, stime the 13th.
    let Some((_, rest)) = stat.rsplit_once(')') else {
        return 0.0;
    };
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ticks = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    (ticks(11) + ticks(12)) / 100.0 * 1000.0 // USER_HZ is 100 on every Linux that runs this
}

fn serve_options(argv: &[String]) -> Result<serve::Options, String> {
    let mut o = serve::Options::default();
    let mut i = 0;
    while i < argv.len() {
        let need = |i: usize| -> Result<&String, String> {
            argv.get(i + 1).ok_or_else(|| format!("{} wants a value", argv[i]))
        };
        match argv[i].as_str() {
            "--model" => o.model = PathBuf::from(need(i)?),
            "--listen" => o.listen = need(i)?.clone(),
            "--threshold" => o.threshold = need(i)?.parse().map_err(|_| "--threshold wants a number")?,
            "--cooldown" => o.cooldown = need(i)?.parse().map_err(|_| "--cooldown wants seconds")?,
            "--idle" => o.idle = Duration::from_secs(need(i)?.parse().map_err(|_| "--idle wants seconds")?),
            "--max-sessions" => {
                o.max_sessions = need(i)?.parse().map_err(|_| "--max-sessions wants a count")?
            }
            "--engine" => o.engine = need(i)?.clone(),
            "--oww-models" => o.oww_models = PathBuf::from(need(i)?),
            "--oww-keyword" => o.oww_keyword = need(i)?.clone(),
            "--oww-threshold" => {
                o.oww_threshold = need(i)?.parse().map_err(|_| "--oww-threshold wants a number")?
            }
            "--record-dir" => o.record_dir = PathBuf::from(need(i)?),
            other => return Err(format!("unknown flag {other}")),
        }
        i += 2;
    }
    if o.model.as_os_str().is_empty() {
        return Err("--model is required".into());
    }
    // Checked at startup rather than on the first session: the openwakeword engine is three graphs
    // in a directory, and a wrong directory is a config error that belongs in the journal at boot.
    match o.engine.as_str() {
        "livekit" => {}
        "oww" => {
            if o.oww_models.as_os_str().is_empty() {
                return Err("--engine oww needs --oww-models <dir with melspectrogram.onnx, \
                            embedding_model.onnx and <keyword>.onnx>"
                    .into());
            }
        }
        other => return Err(format!("unknown --engine {other:?} (livekit | oww)")),
    }
    Ok(o)
}

fn parse(argv: Vec<String>) -> Result<Args, String> {
    if argv.is_empty() || argv[0] == "-h" || argv[0] == "--help" {
        return Ok(Args {
            verb: "help".into(),
            model: PathBuf::new(),
            files: vec![],
            threshold: 0.5,
            cooldown: 2.0,
            chunk: STEP_SAMPLES,
        });
    }
    let verb = argv[0].clone();
    if verb != "score" && verb != "bench" {
        return Err(format!("unknown verb {verb:?} (try `help`)"));
    }
    let mut model: Option<PathBuf> = None;
    let mut files: Vec<PathBuf> = Vec::new();
    let mut threshold = 0.5_f32;
    let mut cooldown = 2.0_f64;
    let mut chunk = STEP_SAMPLES;
    let mut i = 1;
    while i < argv.len() {
        let a = argv[i].as_str();
        if a == "--threshold" {
            threshold = argv
                .get(i + 1)
                .and_then(|v| v.parse().ok())
                .ok_or("--threshold wants a number")?;
            i += 2;
        } else if a == "--cooldown" {
            cooldown = argv
                .get(i + 1)
                .and_then(|v| v.parse().ok())
                .ok_or("--cooldown wants seconds")?;
            i += 2;
        } else if a == "--chunk" {
            chunk = argv
                .get(i + 1)
                .and_then(|v| v.parse().ok())
                .filter(|n: &usize| *n > 0)
                .ok_or("--chunk wants a sample count")?;
            i += 2;
        } else if a.starts_with("--") {
            return Err(format!("unknown flag {a}"));
        } else if model.is_none() {
            model = Some(PathBuf::from(a));
            i += 1;
        } else {
            let p = PathBuf::from(a);
            if p.is_dir() {
                let mut found: Vec<PathBuf> = std::fs::read_dir(&p)
                    .map_err(|e| format!("{}: {e}", p.display()))?
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|q| q.extension().is_some_and(|x| x == "wav"))
                    .collect();
                found.sort();
                files.extend(found);
            } else {
                files.push(p);
            }
            i += 1;
        }
    }
    Ok(Args {
        verb,
        model: model.ok_or("no model .onnx given")?,
        files,
        threshold,
        cooldown,
        chunk,
    })
}
