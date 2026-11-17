//! `owwbench` — the head-to-head: the browser's openWakeWord pipeline against the engine we
//! actually ship, on the same bytes, in the same process family, on the same runtime.
//!
//! A separate verb rather than a flag on `bench`, because `bench`'s loop is written around
//! `livekit_wakeword`'s "give me N samples, get the max score of the steps it completed"
//! contract, and openWakeWord is a three-graph pipeline with its own buffering. Rewriting a
//! working harness mid-investigation to be generic over both is how you end up trusting
//! neither: the numbers in `README.md` came out of `bench`, so `bench` must keep producing
//! them unchanged while this settles a question.
//!
//! The runtime is the same either way — `ort` with `alternative-backend`, i.e. tract — so
//! **anything this measures cannot be about wasm**. It measures model and gating, which is
//! the thing the floor report was actually about.
//!
//! ```text
//! voice-wake oww-bench <model-dir> <clip.wav|dir>… [--keyword hey_jarvis_v0.1]
//!                                          [--threshold T] [--cooldown S] [--gate on|off]
//! ```

use crate::oww::{Oww, FRAME};
use hound::WavReader;
use std::path::{Path, PathBuf};
use std::time::Instant;

struct Clip {
    name: String,
    samples: Vec<f32>,
}

pub fn run(argv: &[String]) -> i32 {
    let opts = match parse(argv) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}");
            usage();
            return 2;
        }
    };
    let mut clips = Vec::new();
    for p in &opts.inputs {
        match load(p) {
            Ok(c) => clips.push(c),
            Err(e) => {
                eprintln!("error: {p:?}: {e}");
                return 2;
            }
        }
    }
    if clips.is_empty() {
        eprintln!("error: no audio to measure");
        return 2;
    }

    let bench_start = Instant::now();
    let mut ear = match Oww::new(
        &opts.dir,
        &opts.keyword,
        opts.threshold,
        opts.cooldown,
        opts.window,
        opts.gate,
    ) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("error: loading the openWakeWord graphs: {e}");
            return 1;
        }
    };
    let loaded = bench_start.elapsed().as_secs_f64();
    println!(
        "openwakeword · {} · input {} Hz · {} ms steps · vad gate {} · loaded {loaded:.2}s",
        opts.keyword,
        16_000,
        FRAME as f64 * 1000.0 / 16_000.0,
        if opts.gate { "on" } else { "off (measuring the raw model)" }
    );

    let mut total_samples = 0usize;
    let mut peak = 0.0f32;
    let mut detections = 0usize;
    let mut gated_aways = 0usize;
    let mut hit_files = 0usize;
    let mut steps = 0usize;

    for clip in &clips {
        let mut clip_peak = 0.0f32;
        let mut clip_hits = 0usize;
        let mut at = 0.0f64;
        for frame in clip.samples.chunks(FRAME) {
            let mut buf = [0.0f32; FRAME];
            buf[..frame.len()].copy_from_slice(frame);
            steps += 1;
            match ear.push_frame(&buf) {
                Ok(step) => {
                    if step.score > clip_peak {
                        clip_peak = step.score;
                    }
                    let would_fire = step.score > opts.threshold;
                    if would_fire && !step.detected {
                        gated_aways += 1;
                    }
                    if step.detected {
                        clip_hits += 1;
                        println!(
                            "  DETECT {} at {at:.2}s  score {:.3}  vad {}",
                            clip.name,
                            step.score,
                            if step.speech { "speech" } else { "silent" }
                        );
                    }
                }
                Err(e) => {
                    eprintln!("error: {} at {at:.2}s: {e}", clip.name);
                    return 1;
                }
            }
            at += FRAME as f64 / 16_000.0;
        }
        total_samples += clip.samples.len();
        if clip_hits > 0 {
            hit_files += 1;
        }
        detections += clip_hits;
        if clip_peak > peak {
            peak = clip_peak;
        }
        let secs = clip.samples.len() as f64 / 16_000.0;
        println!(
            "  {:<34} {:6.1}s  peak {:.3}  {} detection(s)",
            clip.name,
            secs,
            clip_peak,
            clip_hits
        );
    }

    let audio = total_samples as f64 / 16_000.0;
    // `Instant::now().elapsed()` is ~zero by construction — it measures from the instant it
    // was called. The clock has to be the one the run started on, or the cost line prints
    // 0.0 ms per audio-second for a pipeline that definitely did work (which is exactly the
    // kind of number that gets believed because it is flattering).
    let wall = bench_start.elapsed().as_secs_f64();
    let ms_per_second = (wall * 1000.0) / audio;
    println!("──────────────────────────────────────────────────────────");
    println!("peak score          {peak:.3} (threshold {:.2})", opts.threshold);
    println!(
        "detections          {detections} across {} of {} file(s)",
        hit_files,
        clips.len()
    );
    println!("gated by cooldown   {gated_aways} (score crossed, not emitted)");
    println!("audio               {audio:.1}s over {} file(s), {steps} frames", clips.len());
    println!("cost                {ms_per_second:.1} ms CPU per audio-second = {:.1}% of one core", ms_per_second / 10.0);
    if audio > 60.0 && detections > 0 {
        println!("rate                {:.2} detections/hour of this audio", detections as f64 / audio * 3600.0);
    }
    0
}

fn load(path: &Path) -> Result<Clip, String> {
    let mut r = WavReader::open(path).map_err(|e| format!("{e}"))?;
    let spec = r.spec();
    if spec.sample_rate != 16_000 {
        return Err(format!(
            "{} Hz — this harness feeds the graphs exactly what the browser feeds them, so \
             no resampling is done here. Convert with: ffmpeg -i IN -ar 16000 -ac 1 OUT.wav",
            spec.sample_rate
        ));
    }
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r
            .samples::<f32>()
            .map(|s| s.unwrap_or(0.0))
            .collect(),
        hound::SampleFormat::Int => match spec.bits_per_sample {
            16 => r
                .samples::<i16>()
                .map(|s| s.map(|s| s as f32 / 32768.0).unwrap_or(0.0))
                .collect(),
            b => return Err(format!("unsupported bit depth {b} (want 16-bit PCM or f32)")),
        },
    };
    if samples.is_empty() {
        return Err("no samples".into());
    }
    Ok(Clip {
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        samples,
    })
}

struct Opts {
    dir: PathBuf,
    inputs: Vec<PathBuf>,
    keyword: String,
    threshold: f32,
    cooldown: f32,
    window: usize,
    gate: bool,
}

fn parse(argv: &[String]) -> Result<Opts, String> {
    let mut dir = None;
    let mut inputs = Vec::new();
    let mut keyword = "hey_jarvis_v0.1".to_string();
    let mut threshold = 0.5f32;
    let mut cooldown = 2.0f32;
    let mut window = 16usize;
    let mut gate = true;
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        let mut need = || it.next().map(|s| s.to_string()).ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--keyword" => keyword = need()?,
            "--threshold" => threshold = need()?.parse().map_err(|_| "--threshold wants a number")?,
            "--cooldown" => cooldown = need()?.parse().map_err(|_| "--cooldown wants seconds")?,
            "--oww-window" => window = need()?.parse().map_err(|_| "--oww-window wants a number")?,
            "--gate" => gate = need()? != "off",
            s if s.starts_with("--") => return Err(format!("unknown flag {s}")),
            s => {
                let p = PathBuf::from(s);
                if p.is_dir() && dir.is_none() {
                    dir = Some(p);
                } else if p.is_dir() {
                    let mut more: Vec<PathBuf> = std::fs::read_dir(&p)
                        .map_err(|e| format!("{s}: {e}"))?
                        .filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|f| f.extension().map(|x| x == "wav").unwrap_or(false))
                        .collect();
                    more.sort();
                    inputs.extend(more);
                } else {
                    inputs.push(p);
                }
            }
        }
    }
    let dir = dir.ok_or("first argument is the directory holding the .onnx graphs")?;
    Ok(Opts { dir, inputs, keyword, threshold, cooldown, window, gate })
}

fn usage() {
    eprintln!(
        "usage: voice-wake oww-bench <model-dir> <clip.wav|dir>…\n\
       \n\
       \x20   --keyword NAME     keyword head to score (default hey_jarvis_v0.1)\n\
       \x20   --threshold T      score above which a frame counts (default 0.5)\n\
       \x20   --cooldown S       audio seconds to ignore after a hit (default 2)\n\
       \x20   --gate on|off      require silero VAD, as the browser does (default on)\n\
       \x20   --oww-window N     keyword embedding window (default 16)\n"
    );
}
