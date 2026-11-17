//! `voice-assistant` — the hackspace's voice front end, in one binary and four verbs.
//!
//! ```text
//! voice-assistant serve                 # the API every caller shares (127.0.0.1:8193)
//! voice-assistant satellite             # cam8's mic in, an answer out (speaker off by default)
//! voice-assistant probe                 # satellite with asking switched off: what would it say?
//! voice-assistant ask "is it cold"      # one question, printed, no audio, no Slack
//! voice-assistant tts "hello" -o a.wav  # the voice alone, to a file
//! ```
//!
//! The design, the measured latency budget and the constraints that shaped it are in
//! `crates/voice-assistant/README.md`; the wiring is
//! `machines/services1/services/voice-assistant.nix`.  What belongs in this file is
//! only argument parsing, which is why every flag has an `Environment=` twin — the
//! unit renders flags, and an operator debugging on the box reads env vars out of
//! `systemctl show`.
//!
//! ## Two rules the parsing enforces rather than documents
//!
//! - A `--listen` that is not loopback is refused (`api::serve`), because the socket
//!   has no authentication and rule 17's lesson is that the guard has to be code.
//! - `satellite` will not touch the camera's speaker unless `--speaker ssh` is typed
//!   in full.  The default writes the answer to disk and logs it, because this is a
//!   loud feature on a ceiling mount in a room with people in it.

mod agent;
mod api;
mod b64;
mod gate;
mod satellite;
mod spoken;
mod stt;
mod tts;
mod wake;
mod wav;

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let verb = argv.first().map(String::as_str).unwrap_or("");
    if matches!(verb, "" | "-h" | "--help" | "help") {
        print!("{}", usage());
        std::process::exit(if verb.is_empty() { 2 } else { 0 });
    }
    if matches!(verb, "-V" | "--version") {
        println!("voice-assistant {VERSION}");
        return;
    }
    let rest = &argv[1..];
    let result = match verb {
        "serve" => api::serve(serve_options(rest)),
        "satellite" => satellite::run(satellite_options(rest, false)),
        "probe" => satellite::run(satellite_options(rest, true)),
        "ask" => run_ask(rest),
        "tts" => run_tts(rest),
        "gate-check" => run_gate_check(rest),
        other => Err(format!(
            "unknown verb {other:?}; try serve, satellite, probe, ask, gate-check or tts (and --help)"
        )),
    };
    if let Err(e) = result {
        eprintln!("voice-assistant {verb}: {e}");
        std::process::exit(1);
    }
}

fn setting(args: &[String], flag: &str, env: &str, default: &str) -> String {
    let from_env = env_value(env).unwrap_or_else(|| default.to_string());
    common_service::arg(args, flag, Some(&from_env))
}

fn env_value(env: &str) -> Option<String> {
    std::env::var(env).ok().filter(|v| !v.trim().is_empty())
}

fn number(args: &[String], flag: &str, env: &str, default: f64) -> f64 {
    let text = setting(args, flag, env, &default.to_string());
    match text.parse::<f64>() {
        Ok(n) => n,
        Err(_) => {
            eprintln!("voice-assistant: {flag}={text:?} is not a number; using {default}");
            default
        }
    }
}

fn serve_options(a: &[String]) -> api::Options {
    let state = setting(a, "--state-dir", "VOICE_STATE_DIR", "/var/lib/voice-assistant");
    api::Options {
        listen: setting(a, "--listen", "VOICE_LISTEN", api::DEFAULT_LISTEN),
        whisper_url: setting(a, "--whisper-url", "VOICE_WHISPER_URL", stt::DEFAULT_URL),
        whisper_language: match setting(a, "--whisper-language", "VOICE_WHISPER_LANGUAGE", "en") {
            s if s.eq_ignore_ascii_case("auto") || s.is_empty() => None,
            s => Some(s),
        },
        agent_url: setting(a, "--agent-url", "VOICE_AGENT_URL", agent::DEFAULT_URL),
        piper_bin: setting(a, "--piper-bin", "VOICE_PIPER_BIN", "piper"),
        piper_model: setting(a, "--piper-model", "VOICE_PIPER_MODEL", ""),
        length_scale: number(a, "--length-scale", "VOICE_LENGTH_SCALE", 1.05) as f32,
        max_speech_chars: number(
                a,
                "--max-speech-chars",
                "VOICE_MAX_SPEECH_CHARS",
                spoken::DEFAULT_MAX_CHARS as f64,
            ) as usize,
        allow_origins: setting(
            a,
            "--allow-origin",
            "VOICE_ALLOW_ORIGIN",
            "https://whisper.int.leighhack.org",
        )
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect(),
        satellite_status: Some(PathBuf::from(format!("{state}/satellite-status.json"))),
        out_dir: PathBuf::from(format!("{state}/answers")),
        wake_words: wake_list(a),
        record_questions: !a.iter().any(|s| s == "--no-record-questions"),
    }
}

/// The wake phrases, one flag, comma separated, shared by the two long-running modes:
/// the satellite needs them to decide whether it was addressed and the gateway needs
/// them to clean up what the browser page sends.
fn wake_list(a: &[String]) -> Vec<String> {
    setting(
        a,
        "--wake-words",
        "VOICE_WAKE_WORDS",
        "hey jarvis,jarvis,hey hackspace,hackspace",
    )
    .split(',')
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .collect()
}

/// The speech gate's numbers, from flags or `VOICE_*` — one definition, because
/// `gate-check` has to run the *same* gate the satellite does or its output is a story.
fn gate_config(a: &[String]) -> gate::Config {
    gate::Config {
        margin_db: number(a, "--margin-db", "VOICE_MARGIN_DB", 8.0),
        min_open_dbfs: number(a, "--min-open-dbfs", "VOICE_MIN_OPEN_DBFS", -42.0),
        open_frames: number(a, "--open-frames", "VOICE_OPEN_FRAMES", 3.0) as usize,
        hold_secs: number(a, "--hold-secs", "VOICE_HOLD_SECS", 0.7),
        min_secs: number(a, "--min-secs", "VOICE_MIN_SECS", 0.45),
        max_secs: number(a, "--max-secs", "VOICE_MAX_SECS", 10.0),
        pre_roll_secs: number(a, "--pre-roll-secs", "VOICE_PRE_ROLL_SECS", 0.35),
        hf_share: number(a, "--hf-share", "VOICE_HF_SHARE", 0.02),
        voiced_share: number(a, "--voiced-share", "VOICE_VOICED_SHARE", 0.08),
        modulation: number(a, "--modulation", "VOICE_MODULATION", 0.12),
    }
}

/// The diagnostic verb: run the gate over a WAV, frame by frame, and say what it
/// thought of each one.  "It never opened" has two completely different answers — the
/// room was quieter than the threshold, or the threshold is above the room — and only
/// one of them is fixed by turning a knob.  `--calibrate` prints the level distribution
/// and the `--min-open-dbfs` it would pick from it.
fn run_gate_check(a: &[String]) -> Result<(), String> {
    // The first non-flag word is the file; `--in` is the same thing for anyone whose
    // shell has already eaten it.
    let named = setting(a, "--in", "VOICE_IN", "");
    let path = a
        .iter()
        .find(|w| !w.starts_with('-'))
        .cloned()
        .or(if named.is_empty() { None } else { Some(named) })
        .ok_or_else(|| "gate-check needs a WAV: voice-assistant gate-check clip.wav".to_string())?;
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let file = wav::parse(&bytes)?;
    let cfg = gate_config(a);
    let mut g = gate::Gate::new(cfg.clone());
    g.trace_on();
    for chunk in file.samples.chunks(3200) {
        g.push(chunk);
    }
    // The end of a file is not the end of a sentence: take the window the gate is still
    // holding, or a clip that opened at 0.38 s reads as "the gate never opened".
    g.flush();
    let trace = g.take_trace();
    let (windows, short) = (g.windows, g.dropped_short);

    let mut levels: Vec<f64> = trace.iter().map(|f| f.level).collect();
    levels.sort_by(|a, b| a.total_cmp(b));
    let pct = |p: f64| {
        levels
            .get(((levels.len() as f64) * p) as usize)
            .copied()
            .unwrap_or(-120.0)
    };
    let speech = trace.iter().filter(|f| f.speech_like).count();
    println!(
        "{path}: {:.1} s, {} frames, {} speech-like, {} window(s){short} \
         [min {min:.0} / median {med:.0} / p90 {p90:.0} / max {max:.0} dBFS]",
        file.samples.len() as f64 / wav::SAMPLE_RATE as f64,
        trace.len(),
        speech,
        windows,
        short = if short > 0 {
            format!(", {short} dropped as shorter than --min-secs")
        } else {
            String::new()
        },
        max = levels.last().copied().unwrap_or(-120.0),
        min = pct(0.0),
        med = pct(0.5),
        p90 = pct(0.9),
    );
    if trace.is_empty() {
        return Ok(());
    }

    // The interesting frames only: everything the gate thought was speech, and the loud
    // near-misses that it did not — a 40-second clip printed frame by frame is a wall
    // that says nothing, and the near-misses are the ones that explain a missed word.
    let floor = trace.last().map(|f| f.floor).unwrap_or(0.0);
    println!(
        "threshold at the end: {:.0} dBFS (floor {:.0} + margin {:.0}, absolute floor {:.0})",
        (floor + cfg.margin_db).max(cfg.min_open_dbfs),
        floor,
        cfg.margin_db,
        cfg.min_open_dbfs
    );
    println!("   secs   level   floor  thresh    mod     hf  voiced  said");
    let mut shown = 0usize;
    for f in trace.iter() {
        let near = f.level > (f.floor + cfg.margin_db).max(cfg.min_open_dbfs) - 6.0;
        if !(f.speech_like || near || f.opened || f.closed) {
            continue;
        }
        shown += 1;
        if shown > 60 {
            println!("  … ({} more; --calibrate for the distribution)", trace.len());
            break;
        }
        let mut said = String::new();
        if f.opened {
            said.push_str("OPEN ");
        }
        if f.closed {
            said.push_str("CLOSE ");
        }
        if f.speech_like {
            said.push_str("speech");
        } else if !near {
            said.push_str("quiet");
        } else if f.level <= (f.floor + cfg.margin_db).max(cfg.min_open_dbfs) {
            said.push_str("too quiet");
        } else {
            // Loud enough: which of the three shape tests said no.
            let mut why = Vec::new();
            if f.modulation <= cfg.modulation {
                why.push(format!("mod {:.2}", f.modulation));
            }
            if f.hf <= cfg.hf_share {
                why.push(format!("hf {:.3}", f.hf));
            }
            if f.voiced <= cfg.voiced_share {
                why.push(format!("voiced {:.3}", f.voiced));
            }
            said.push_str(&format!("no: {}", why.join(" ")));
        }
        println!(
            "  {:>7.2}  {:>6.0}  {:>6.0}  {:>6.0}  {:>5.2}  {:>5.3}  {:>6.3}  {}",
            f.secs, f.level, f.floor, f.threshold, f.modulation, f.hf, f.voiced, said
        );
    }
    if a.iter().any(|w| w == "--calibrate") {
        let pick = (pct(0.5) + 12.0).clamp(-52.0, -18.0);
        println!(
            "suggested --min-open-dbfs {pick:.0} (median + 12 dB, clamped to −18…−52); \
             the gate's own floor does the rest, so this number is only the line below \
             which nothing counts at all"
        );
    }
    Ok(())
}

fn satellite_options(a: &[String], probe_only: bool) -> satellite::Options {
    let state = setting(a, "--state-dir", "VOICE_STATE_DIR", "/var/lib/voice-assistant");
    let speaker = match setting(a, "--speaker", "VOICE_SPEAKER", "off").as_str() {
        "off" | "dry-run" | "none" => satellite::Speaker::Off,
        "ssh" => satellite::Speaker::Ssh {
            host: setting(a, "--speaker-host", "VOICE_SPEAKER_HOST", "cam8.int.leighhack.org"),
            user: setting(a, "--speaker-user", "VOICE_SPEAKER_USER", "root"),
            key: PathBuf::from(setting(
                a,
                "--ssh-key",
                "VOICE_SSH_KEY",
                &format!("{}/.ssh/agent-hop-key", std::env::var("HOME").unwrap_or_default()),
            )),
        },
        other => {
            eprintln!("voice-assistant: unknown --speaker {other:?}; using the safe default, off");
            satellite::Speaker::Off
        }
    };
    satellite::Options {
        ffmpeg: PathBuf::from(setting(a, "--ffmpeg", "VOICE_FFMPEG", "ffmpeg")),
        source: setting(
            a,
            "--source",
            "VOICE_SOURCE",
            "rtsp://127.0.0.1:8554/cam8",
        ),
        gateway: setting(a, "--gateway", "VOICE_GATEWAY", "http://127.0.0.1:8193"),
        wake_words: wake_list(a),
        search_tokens: number(a, "--wake-search-tokens", "VOICE_WAKE_SEARCH_TOKENS", 6.0) as usize,
        gate: gate_config(a),
        speaker: if probe_only {
            satellite::Speaker::Off
        } else {
            speaker
        },
        status_path: PathBuf::from(format!("{state}/satellite-status.json")),
        out_dir: PathBuf::from(format!("{state}/answers")),
        probe_only,
        capture_secs: number(a, "--seconds", "VOICE_CAPTURE_SECS", 0.0),
        out_file: {
            let p = setting(a, "--out", "VOICE_OUT", "");
            if p.is_empty() {
                None
            } else {
                Some(PathBuf::from(p))
            }
        },
        follow_up: Duration::from_secs_f64(number(a, "--follow-up-secs", "VOICE_FOLLOW_UP_SECS", 6.0)),
        guard: Duration::from_secs_f64(number(a, "--guard-secs", "VOICE_GUARD_SECS", 1.2)),
        max_play_secs: number(a, "--max-play-secs", "VOICE_MAX_PLAY_SECS", 45.0),
        ssh_timeout: Duration::from_secs(20),
    }
}

fn run_ask(a: &[String]) -> Result<(), String> {
    let question = match a.first().map(String::as_str) {
        Some("-") => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("reading the question from stdin: {e}"))?;
            buf.split_whitespace().collect::<Vec<_>>().join(" ")
        }
        Some(q) => q.to_string(),
        None => {
            return Err(
                "ask needs a question (or - to read it from stdin): voice-assistant ask \
                 \"is it cold in the workshop\""
                    .to_string(),
            )
        }
    };
    let url = setting(a, "--agent-url", "VOICE_AGENT_URL", agent::DEFAULT_URL);
    let started = std::time::Instant::now();
    let answer = agent::Client::new(&url).ask(&question, false)?;
    println!("--- answer ---");
    println!("{}", answer.text.trim());
    println!(
        "\n[{} · {:.1} s agent, {:.1} s here · {} round(s) · tools {:?}]",
        answer.engine,
        answer.secs,
        started.elapsed().as_secs_f64(),
        answer.rounds,
        answer.tools.join(", "),
    );
    for w in &answer.warnings {
        eprintln!("warning: {w}");
    }
    Ok(())
}

fn run_tts(a: &[String]) -> Result<(), String> {
    let text = match a.first().map(String::as_str) {
        Some("-") => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("reading text from stdin: {e}"))?;
            buf
        }
        Some(t) => t.to_string(),
        None => return Err("tts needs text (or - to read it from stdin)".to_string()),
    };
    let opts = serve_options(a);
    let piper = tts::Piper::new(&opts.piper_bin, &opts.piper_model, opts.length_scale);
    let speak = if a.iter().any(|s| s == "--raw") {
        text.clone()
    } else {
        spoken::speakable(&text, opts.max_speech_chars)
            .ok_or_else(|| "nothing speakable in that text".to_string())?
    };
    let speech = piper.synth(&speak)?;
    let pcm = wav::resample(&speech.pcm, speech.voice.sample_rate, wav::SAMPLE_RATE);
    let body = wav::mono16(&pcm, wav::SAMPLE_RATE);
    let out = match a.iter().position(|s| s == "-o" || s == "--out").and_then(|i| a.get(i + 1)) {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from("voice-tts.wav"),
    };
    std::fs::write(&out, &body).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    println!(
        "{} → {} ({} samples at {} Hz from {}, {:.1} s of audio, {:.1} s wall) → {}",
        speak,
        out.display(),
        pcm.len(),
        wav::SAMPLE_RATE,
        speech.voice.name,
        speech.secs,
        speech.wall.as_secs_f64(),
        if speech.voice.sample_rate == wav::SAMPLE_RATE {
            "no resample"
        } else {
            "resampled"
        }
    );
    Ok(())
}

fn usage() -> String {
    format!(
        "voice-assistant {VERSION} — the hackspace voice front end\n\n\
         usage: voice-assistant <serve|satellite|probe|ask|tts> [flags]\n\n\
         serve                      the shared API on --listen (default 127.0.0.1:{});\n\
                                POST /v1/transcribe (a WAV body), /v1/ask, /v1/tts\n\
         satellite                  --source's mic through the gate and the API; the\n\
                                camera speaker is OFF unless --speaker ssh is typed\n\
         probe                      satellite with asking off: counts what it would say\n\
                                    --seconds N stops after N s of audio (0 = until\n\
                                    Ctrl-C); --out FILE leaves one WAV to listen to\n\
         ask QUESTION               one question through the bot, printed (no audio)\n\
         tts TEXT -o FILE           the voice alone; - reads stdin, --raw skips the\n\
                                markdown cleanup\n\n\
         common flags (each also reads the VOICE_* environment):\n\
           --listen, --state-dir, --whisper-url, --whisper-language, --agent-url,\n\
           --piper-bin, --piper-model, --length-scale, --max-speech-chars,\n\
           --allow-origin (comma separated, the only origins CORS will answer)\n\
         satellite flags:\n\
           --source, --gateway, --ffmpeg, --wake-words, --wake-search-tokens,\n\
           --speaker off|ssh, --speaker-host, --speaker-user, --ssh-key,\n\
           --follow-up-secs, --guard-secs, --max-play-secs, and the gate's own:\n\
           --margin-db, --min-open-dbfs, --open-frames, --hold-secs, --min-secs,\n\
           --max-secs, --pre-roll-secs, --hf-share, --voiced-share, --modulation\n\n\
         why the flags exist, and what was measured to pick their defaults:\n\
         crates/voice-assistant/README.md\n",
        api::DEFAULT_LISTEN
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(|w| w.to_string()).collect()
    }

    #[test]
    fn the_satellite_defaults_do_not_touch_the_speaker() {
        // The one default that must never be wrong: a fresh unit in a room with
        // people in it must not be able to start talking.
        let o = satellite_options(&argv(""), false);
        assert_eq!(o.speaker, satellite::Speaker::Off);
        assert!(!o.probe_only);
        // And an unrecognised value falls back to off rather than to ssh.
        let o = satellite_options(&argv("--speaker yes-please"), false);
        assert_eq!(o.speaker, satellite::Speaker::Off);
        let o = satellite_options(&argv("--speaker ssh --speaker-host cam8.int"), false);
        match o.speaker {
            satellite::Speaker::Ssh { host, user, key } => {
                assert_eq!(host, "cam8.int");
                assert_eq!(user, "root");
                assert!(key.ends_with("agent-hop-key"), "{}", key.display());
            }
            other => panic!("--speaker ssh did not select ssh: {other:?}"),
        }
    }

    #[test]
    fn probe_mode_cannot_speak_even_if_asked_to() {
        let o = satellite_options(&argv("--speaker ssh"), true);
        assert_eq!(o.speaker, satellite::Speaker::Off);
        assert!(o.probe_only);
    }

    #[test]
    fn a_probe_can_be_told_to_stop_and_to_leave_a_clip() {
        // `just voice-probe 8` depends on both flags existing and defaulting the way
        // the recipe says: an unbounded probe is one nobody runs twice, and a probe with
        // nothing to listen to afterwards cannot answer "why did it ignore me".
        let o = satellite_options(&argv("--seconds 5 --out /tmp/x.wav"), true);
        assert_eq!(o.capture_secs, 5.0);
        assert_eq!(o.out_file.as_deref(), Some(std::path::Path::new("/tmp/x.wav")));
        let d = satellite_options(&argv(""), false);
        assert_eq!(d.capture_secs, 0.0, "the unit's satellite is not bounded");
        assert!(d.out_file.is_none());
    }

    #[test]
    fn the_gateway_defaults_are_the_ports_and_names_the_units_render() {
        let o = serve_options(&argv(""));
        assert_eq!(o.listen, api::DEFAULT_LISTEN);
        assert_eq!(o.whisper_url, stt::DEFAULT_URL);
        assert_eq!(o.agent_url, agent::DEFAULT_URL);
        assert_eq!(o.whisper_language.as_deref(), Some("en"));
        assert_eq!(
            o.allow_origins,
            vec!["https://whisper.int.leighhack.org".to_string()]
        );
        // `auto` means "let whisper decide", which is a different thing from a name.
        let o = serve_options(&argv("--whisper-language auto"));
        assert_eq!(o.whisper_language, None);
    }

    #[test]
    fn a_bad_number_is_a_warning_and_the_default_not_a_crash() {
        let o = serve_options(&argv("--max-speech-chars huge"));
        assert_eq!(o.max_speech_chars, spoken::DEFAULT_MAX_CHARS);
        assert_eq!(number(&argv("--margin-db x"), "--margin-db", "", 8.0), 8.0);
    }

    #[test]
    fn help_lists_every_verb_the_match_can_dispatch() {
        let text = usage();
        for verb in ["serve", "satellite", "probe", "ask", "tts"] {
            assert!(text.contains(verb), "{verb} missing from --help");
        }
        assert!(text.contains("README.md"), "help does not say where the design is");
    }
}
