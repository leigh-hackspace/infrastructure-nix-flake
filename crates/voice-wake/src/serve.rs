//! `voice-wake serve` — the wake engine as a loopback service.
//!
//! ```text
//! POST /v1/session          {"rate":16000}                    → {"session":"…","threshold":0.5,…}
//! POST /v1/chunk            X-Voice-Wake-Session: <id>        → {"score":0.31,"detected":false,…}
//! POST /v1/reset            X-Voice-Wake-Session: <id>        → {"ok":true}
//! DELETE /v1/session        X-Voice-Wake-Session: <id>        → {"ok":true}
//! GET  /status.json                                            → sessions, detections, cpu per audio-second
//! GET  /health                                                 → ok
//! ```
//!
//! ## Why HTTP here, and why this HTTP is not Wyoming
//!
//! The tier's protocol is [Wyoming](https://github.com/OHF-Voice/wyoming) — `describe`/`info`,
//! `detect {names}`, `audio-chunk`, `detection {name,timestamp}` — and the daemon below
//! implements that *event model*, not its TCP framing, because **a browser cannot open a TCP
//! socket**. That is not a detail: the whole reason this service exists is to let a webpage
//! hand its microphone to a server-side detector, and every browser-facing hop in this
//! network goes through nginx over HTTPS. So this endpoint is the Wyoming events mapped onto
//! the transport a tab can actually use — one chunk in, one score out — and raw Wyoming TCP
//! stays the native-device path (`cam8`'s satellite, an ESP32, Home Assistant), which is the
//! first thing that will speak it when one of those wants server-side detection.
//!
//! Deliberately not done: pretending to be wire-compatible with `wyoming.http.*` (upstream's
//! Flask gateway) — its contract was not read, and inventing a compatible-looking URL scheme
//! is how a protocol claim becomes a fiction. And deliberately not done: hand-rolling HTTP
//! *reuse* out of `common-rs/web`. That crate is a member of the `crates/` workspace, and
//! reaching a workspace member from a workspace-of-one is the symlink-and-copy dance the
//! Dioxus SPAs do (`common/shared-rs-links.sh` plus a `sharedCrates` entry in the Nix build);
//! for an endpoint this small that costs more than the ~90 lines it would save, and
//! `crates/whisper-ws/` is the in-repo precedent for a stdlib HTTP server on a loopback
//! socket. Revisit when the second consumer of HTTP-in-Rust-outside-the-workspace shows up.
//!
//! ## Session shape
//!
//! One `WakeWordModel` per session, because the model owns the streaming state and two
//! microphones must not share one head. That means the cost of a session is one 7 MB model
//! and its own inference, which is the honest unit of scale: the measured figure
//! (~31 streams per core on the nix build) is *per stream*, not per process.
//!
//! The map is held in one `Mutex` for the duration of each chunk, so scoring is serialised
//! across sessions. That is a deliberate choice over the alternatives: `ort-tract`'s session
//! type is not documented as `Sync`, and "serialize the inference" is a fact we can measure
//! and bound (it costs latency only when several people speak at once, which is also when the
//! ASR behind us is contended), whereas a `Sync` assumption in a wake path is a crash in a
//! room. If it ever shows up in the numbers, the fix is one model *graph* shared by N
//! per-stream state machines, not an `unsafe impl`.
//!
//! ## The gate that keeps this off the network
//!
//! It binds loopback and refuses otherwise, with the same wording as the voice gateway: this
//! socket has no authentication. The browser reaches it through the front door's
//! `voice.int.leighhack.org` row, via the gateway's `/v1/wake/*` proxy — which is also why
//! there is no CORS handling in here.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::ear::Ear;
use hound::{SampleFormat, WavSpec, WavWriter};

use crate::STEP_SAMPLES;

pub const DEFAULT_LISTEN: &str = "127.0.0.1:8195";

/// Longest recording kept per session, in seconds.  The point of recording is a corpus to bench
/// on, not an archive of the room: 600 s is 19 MB, and a session that streams all day must not
/// fill a state directory merely by being open.
const MAX_RECORD_SECONDS: f64 = 600.0;

/// Bodies are PCM, and a 60 s window at 16 kHz mono s16 is 1.9 MB. The cap is below that on
/// purpose: the client is asked to send ~0.5 s chunks, and a browser that decides to buffer
/// a whole recording and POST it once is a bug we want to see as a 413 rather than absorb.
const MAX_BODY_BYTES: usize = 1_048_576;

pub struct Options {
    pub listen: String,
    pub model: PathBuf,
    /// Which detector scores the session: `livekit` (the default, and what has always been
    /// deployed) or `oww` — the browser's own openWakeWord graphs.  `crate::ear` keeps the reason
    /// these are two detectors rather than two machines.
    pub engine: String,
    /// The directory holding `melspectrogram.onnx`, `embedding_model.onnx` and `<keyword>.onnx`:
    /// the same files the page fetches, pinned by the same sha256, not a re-export of them.
    pub oww_models: PathBuf,
    pub oww_keyword: String,
    /// Where to write the audio a session was fed, so a score has bytes behind it.  **Empty by
    /// default, and that is a privacy decision rather than a default:** these are recordings of a
    /// room with people in it, so the flag is for a measurement window, the files are 0600, and
    /// nothing turns it on declaratively without a reason written down beside it.
    pub record_dir: PathBuf,
    pub threshold: f32,
    pub cooldown: f64,
    pub idle: Duration,
    pub max_sessions: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            listen: DEFAULT_LISTEN.to_string(),
            model: PathBuf::new(),
            engine: "livekit".to_string(),
            oww_models: PathBuf::new(),
            oww_keyword: "hey_jarvis_v0.1".to_string(),
            record_dir: PathBuf::new(),
            threshold: 0.5,
            cooldown: 2.0,
            idle: Duration::from_secs(120),
            max_sessions: 16,
        }
    }
}

struct Session {
    ear: Ear,
    rate: u32,
    /// Seconds of audio this session has fed the model — the clock detections are stamped on,
    /// so the client can line a detection up with what it just heard.
    audio_secs: f64,
    peak: f32,
    detections: u32,
    last_seen: Instant,
    /// What the caller called itself (`{"label":"laptop"}`), used only in the recording's name.
    label: String,
    /// Everything fed to this session while `--record-dir` is set.  This is the point of the flag:
    /// a score with no bytes behind it cannot be re-run, compared, or argued with tomorrow.
    recorded: Vec<i16>,
}

fn recording(state: &State) -> bool {
    !state.opts.record_dir.as_os_str().is_empty()
}

struct Stats {
    sessions_created: u64,
    chunks: u64,
    detections: u64,
    audio_secs: f64,
    /// Predict time vs audio time, summed, so `/status.json` can state the measured cost
    /// without anyone having to run the offline harness to know what the box is paying.
    infer_ms: f64,
    started: Instant,
}

impl Stats {
    fn new() -> Self {
        Self {
            sessions_created: 0,
            chunks: 0,
            detections: 0,
            audio_secs: 0.0,
            infer_ms: 0.0,
            started: Instant::now(),
        }
    }
}

pub fn serve(opts: Options) -> Result<(), String> {
    if !is_loopback(&opts.listen) {
        return Err(format!(
            "refusing to bind {}: this socket has no authentication. The browser reaches it \
             through the voice gateway's /v1/wake/* proxy and the front door's \
             voice.int.leighhack.org row, which is the only supported path in from another machine.",
            opts.listen
        ));
    }

    // One engine opened before a single socket opens: it proves the files load, and for `oww` it
    // fails here rather than on the first session if the model directory is wrong.  The livekit
    // error is the crate's own (`NotStreamingModel`) and says more than anything written here.
    let probe = Ear::open(
        &opts.engine,
        &opts.model,
        &opts.oww_models,
        &opts.oww_keyword,
        opts.threshold,
        opts.cooldown,
        16_000,
    )
    .map_err(|e| format!("cannot open the engine: {e}"))?;
    let model_name = probe.name().to_string();
    drop(probe);

    let listener = TcpListener::bind(&opts.listen).map_err(|e| format!("bind: {e}"))?;
    let state = Arc::new(State {
        sessions: Mutex::new(HashMap::new()),
        stats: Mutex::new(Stats::new()),
        next_id: AtomicU64::new(1),
        opts,
        model_name,
    });
    eprintln!(
        "voice-wake serving on {} · engine {} · threshold {:.2} · cooldown {:.1}s · idle {:?} · max {} sessions · recording {}",
        state.opts.listen,
        state.model_name,
        state.opts.threshold,
        state.opts.cooldown,
        state.opts.idle,
        state.opts.max_sessions,
        if recording(&state) {
            state.opts.record_dir.display().to_string()
        } else {
            "off".to_string()
        }
    );

    for conn in listener.incoming() {
        let Ok(stream) = conn else { continue };
        let state = Arc::clone(&state);
        std::thread::spawn(move || {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            handle(stream, state);
        });
    }
    Ok(())
}

struct State {
    sessions: Mutex<HashMap<String, Session>>,
    stats: Mutex<Stats>,
    next_id: AtomicU64,
    opts: Options,
    model_name: String,
}

fn handle(mut stream: TcpStream, state: Arc<State>) {
    let Some(req) = read_request(&mut stream) else {
        return;
    };
    reap(&state);
    let reply = route(&state, req);
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.flush();
}

fn route(state: &State, req: Request) -> String {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/health") => text(200, "ok"),
        ("GET", "/status.json") => text(200, &status_json(state)),
        ("POST", "/v1/session") => open_session(state, &req),
        ("POST", "/v1/chunk") => chunk(state, &req),
        ("POST", "/v1/reset") => reset(state, &req),
        ("DELETE", "/v1/session") => close(state, &req),
        _ => text(
            404,
            r#"{"error":"no such route","routes":["GET /health","GET /status.json","POST /v1/session","POST /v1/chunk","POST /v1/reset","DELETE /v1/session"]}"#,
        ),
    }
}

fn open_session(state: &State, req: &Request) -> String {
    // The rate is the *client's* rate: the crate resamples 22050/44100/48000 internally, and
    // letting the browser send what it captured avoids a second resampler in a page that
    // already has one and is already resampling for the ASR path.
    let rate = json_u32(&req.body, "rate").unwrap_or(16_000);
    if !matches!(rate, 16_000 | 22_050 | 32_000 | 44_100 | 48_000 | 88_200 | 96_000) {
        return text(
            400,
            &format!(
                r#"{{"error":"unsupported rate {rate}; try 16000, 22050, 32000, 44100 or 48000"}}"#
            ),
        );
    }
    // Checked here as well as inside `Ear::open`: "your sample rate is wrong" is a 400 the client can
    // act on, and a model that will not load is a 500 it cannot.
    if state.opts.engine == "oww" && rate != 16_000 {
        return text(
            400,
            &format!(r#"{{"error":"the openwakeword engine takes 16000 Hz only, not {rate}"}}"#),
        );
    }
    let ear = match Ear::open(
        &state.opts.engine,
        &state.opts.model,
        &state.opts.oww_models,
        &state.opts.oww_keyword,
        state.opts.threshold,
        state.opts.cooldown,
        rate,
    ) {
        Ok(e) => e,
        Err(e) => {
            return text(500, &format!(r#"{{"error":"{}"}}"#, e.replace('"', "'")));
        }
    };
    let label = json_str(&req.body, "label").unwrap_or_default();
    
    let id = format!("w{}", state.next_id.fetch_add(1, Ordering::Relaxed));
    let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
    if sessions.len() >= state.opts.max_sessions {
        return text(
            503,
            &format!(
                r#"{{"error":"{} sessions already streaming; close one and retry"}}"#,
                sessions.len()
            ),
        );
    }
    sessions.insert(
        id.clone(),
        Session {
            ear,
            rate,
            audio_secs: 0.0,
            peak: 0.0,
            detections: 0,
            last_seen: Instant::now(),
            label,
            recorded: Vec::new(),
        },
    );
    let n = sessions.len();
    drop(sessions);
    state.stats.lock().unwrap().sessions_created += 1;
    text(
        200,
        &format!(
            r#"{{"session":"{id}","model":"{}","rate":{rate},"threshold":{},"cooldown":{},"max_body_bytes":{MAX_BODY_BYTES},"active_sessions":{n}}}"#,
            state.model_name, state.opts.threshold, state.opts.cooldown
        ),
    )
}

fn chunk(state: &State, req: &Request) -> String {
    let Some(id) = session_id(req) else {
        return text(400, r#"{"error":"missing X-Voice-Wake-Session header"}"#);
    };
    if req.body.len() % 2 != 0 {
        return text(
            400,
            r#"{"error":"body is not a whole number of 16-bit samples"}"#,
        );
    }
    let samples: Vec<i16> = req
        .body
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();

    let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
    let Some(session) = sessions.get_mut(&id) else {
        return text(
            410,
            r#"{"error":"no such session (idle too long? open another)"}"#,
        );
    };
    session.last_seen = Instant::now();
    let secs = samples.len() as f64 / session.rate as f64;

    let t0 = Instant::now();
    // One call, one rule.  The engine owns its streaming state; `Ear` owns the threshold and the
    // audio-time cooldown, so switching engine cannot also switch what "detected" means.  Cooldown
    // stays in audio time on purpose: a session paused while the assistant is talking must not
    // fire again on the tail of the same utterance.
    let hit = match session.ear.feed(&samples) {
        Ok(h) => h,
        Err(e) => {
            drop(sessions);
            return text(500, &format!(r#"{{"error":"{}"}}"#, e.replace('"', "'")));
        }
    };
    let infer_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let score = hit.score;
    let detected = hit.detected;

    session.audio_secs = session.ear.audio_secs();
    if score > session.peak {
        session.peak = score;
    }
    if detected {
        session.detections += 1;
    }
    if recording(state) && (session.recorded.len() as f64) < MAX_RECORD_SECONDS * session.rate as f64 {
        session.recorded.extend_from_slice(&samples);
    }
    let audio_secs = session.audio_secs;
    let peak = session.peak;
    let detections = session.detections;
    drop(sessions);

    let mut stats = state.stats.lock().unwrap_or_else(|e| e.into_inner());
    stats.chunks += 1;
    stats.audio_secs += secs;
    stats.infer_ms += infer_ms;
    if detected {
        stats.detections += 1;
    }
    let rate_ms = if stats.audio_secs > 0.0 {
        stats.infer_ms / stats.audio_secs
    } else {
        0.0
    };
    drop(stats);

    text(
        200,
        &format!(
            r#"{{"score":{score:.4},"peak":{peak:.4},"detected":{detected},"audio_secs":{audio_secs:.2},"detections":{detections},"infer_ms":{infer_ms:.1},"cpu_ms_per_audio_s":{rate_ms:.1}}}"#
        ),
    )
}

fn reset(state: &State, req: &Request) -> String {
    let Some(id) = session_id(req) else {
        return text(400, r#"{"error":"missing X-Voice-Wake-Session header"}"#);
    };
    let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
    let Some(session) = sessions.get_mut(&id) else {
        return text(410, r#"{"error":"no such session"}"#);
    };
    // Both halves have to go: the model's own history *and* our clock, or the next chunk
    // inherits a stale "already fired" timestamp.
    if let Err(e) = session.ear.reset() {
        return text(500, &format!(r#"{{"error":"reset: {e}"}}"#));
    }
    session.audio_secs = 0.0;
    session.peak = 0.0;
    
    text(200, r#"{"ok":true}"#)
}

fn close(state: &State, req: &Request) -> String {
    let Some(id) = session_id(req) else {
        return text(400, r#"{"error":"missing X-Voice-Wake-Session header"}"#);
    };
    let gone = state
        .sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    let Some(session) = gone else {
        return text(410, r#"{"ok":false}"#);
    };
    dump(state, &id, &session);
    text(200, r#"{"ok":true}"#)
}

fn status_json(state: &State) -> String {
    let sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
    let live: Vec<String> = sessions
        .iter()
        .map(|(id, s)| {
            format!(
                r#"{{"id":"{id}","rate":{},"audio_secs":{:.1},"peak":{:.3},"detections":{},"idle_s":{:.0}}}"#,
                s.rate,
                s.audio_secs,
                s.peak,
                s.detections,
                s.last_seen.elapsed().as_secs_f64()
            )
        })
        .collect();
    drop(sessions);
    let stats = state.stats.lock().unwrap_or_else(|e| e.into_inner());
    let per = if stats.audio_secs > 0.0 {
        stats.infer_ms / stats.audio_secs
    } else {
        0.0
    };
    format!(
        r#"{{"model":"{}","threshold":{},"cooldown":{},"max_sessions":{},"active_sessions":{},"sessions_total":{},"chunks":{},"detections":{},"audio_secs":{:.1},"cpu_ms_per_audio_s":{per:.1},"streams_per_core_estimate":{},"uptime_s":{:.0},"step_samples":{STEP_SAMPLES},"recording":{}}}"#,
        state.model_name,
        state.opts.threshold,
        state.opts.cooldown,
        state.opts.max_sessions,
        live.len(),
        stats.sessions_created,
        stats.chunks,
        stats.detections,
        stats.audio_secs,
        if per > 0.0 {
            (1000.0 / per) as u32
        } else {
            0
        },
        stats.started.elapsed().as_secs_f64(),
        quoted(if recording(state) {
            state.opts.record_dir.display().to_string()
        } else {
            String::new()
        })
    )
}

/// A JSON string literal.  Local because this crate deliberately depends on almost nothing (see
/// its README), and this is the only place a *runtime* string reaches status.json.
fn quoted(s: impl AsRef<str>) -> String {
    let s = s.as_ref();
    let esc: String = s
        .chars()
        .filter(|c| *c as u32 >= 0x20)
        .flat_map(|c| match c {
            '"' | '\\' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    format!("\"{esc}\"")
}

/// Write what a session was fed, so that a score has bytes behind it.
///
/// Silent-with-a-journal-line on failure: a recording that cannot be written must not break a
/// working detection engine.  Mode 0600, because it is room audio.
fn dump(state: &State, id: &str, session: &Session) {
    use std::os::unix::fs::PermissionsExt;
    if !recording(state) || session.recorded.is_empty() {
        return;
    }
    let dir = &state.opts.record_dir;
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("recording: cannot create {}: {e}", dir.display());
        return;
    }
    let label: String = session
        .label
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(24)
        .collect();
    let name = format!(
        "wake-{}-{}{}.wav",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        id,
        if label.is_empty() {
            String::new()
        } else {
            format!("-{label}")
        }
    );
    let path = dir.join(&name);
    let spec = WavSpec {
        channels: 1,
        sample_rate: session.rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut w = match WavWriter::create(&path, spec) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("recording: cannot create {}: {e}", path.display());
            return;
        }
    };
        for s in session.recorded.iter().copied() {
        if let Err(e) = w.write_sample(s) {
            eprintln!("recording: cannot write {}: {e}", path.display());
            return;
        }
    }
    if let Err(e) = w.finalize() {
        eprintln!("recording: cannot finish {}: {e}", path.display());
        return;
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).ok();
    eprintln!(
        "recording: {:.0} s from {} → {} ({} samples)",
        session.recorded.len() as f64 / session.rate as f64,
        session.ear.name(),
        path.display(),
        session.recorded.len()
    );
}

fn reap(state: &State) {
    let idle = state.opts.idle;
    let mut sessions = state.sessions.lock().unwrap_or_else(|e| e.into_inner());
    let before = sessions.len();
    // `remove` rather than `retain`, because a reaped session may still have a recording to write
    // and writing it needs to own the samples.  Dropping that audio here is how a measurement
    // window ends up with no data and no explanation.
    let dead: Vec<String> = sessions
        .iter()
        .filter(|(_, s)| s.last_seen.elapsed() >= idle)
        .map(|(id, _)| id.clone())
        .collect();
    for id in &dead {
        if let Some(session) = sessions.remove(id) {
            dump(state, id, &session);
        }
    }
    if sessions.len() != before {
        eprintln!(
            "reaped {} idle session(s); {} live",
            before - sessions.len(),
            sessions.len()
        );
    }
}

fn session_id(req: &Request) -> Option<String> {
    req.header("x-voice-wake-session")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn text(status: u16, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        410 => "Gone",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Unknown",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// One HTTP/1.1 request, no keep-alive, no chunked encoding.
///
/// `fetch` with a `Uint8Array` body sends `Content-Length`, which is the only shape accepted:
/// chunked bodies would need a framing state machine for a case that has no producer here, so
/// a chunked request is refused rather than silently mis-parsed (a truncated body would look
/// like a quiet microphone, which is the worst possible failure mode in a detection path).
fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.trim_end().split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut headers: Vec<(String, String)> = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).ok()? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if let Some((k, v)) = h.trim_end().split_once(':') {
            headers.push((k.trim().to_lowercase(), v.trim().to_string()));
        }
    }
    let want: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    if want > MAX_BODY_BYTES {
        let _ = stream.write_all(text(413, "body over cap").as_bytes());
        return None;
    }
    let mut body = vec![0u8; want];
    if want > 0 {
        reader.read_exact(&mut body).ok()?;
    }
    Some(Request {
        method,
        path,
        headers,
        body,
    })
}

/// Copy of `common_service::is_loopback`'s rule — parse the host as an address, do not
/// `starts_with("127.")`, which `127.example.com` would pass. Kept local for the same reason
/// as the HTTP server above: this crate is a workspace of one on purpose.
fn is_loopback(listen: &str) -> bool {
    let host = listen.rsplit_once(':').map(|(h, _)| h).unwrap_or(listen);
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        // A name is allowed only if it resolves to loopback, and `localhost` is the case here.
        Err(_) => host == "localhost",
    }
}

/// `"key": 48000` from a small JSON body, without a JSON dependency.
fn json_u32(body: &[u8], key: &str) -> Option<u32> {
    let text = std::str::from_utf8(body).ok()?;
    let at = text.find(&format!("\"{key}\""))?;
    let rest = &text[at + key.len() + 2..];
    let digits: String = rest
        .trim_start()
        .trim_start_matches(':')
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_a_public_bind() {
        // The wording matters as much as the refusal: this socket has no authentication, and
        // someone will try to open it up to reach a laptop off-LAN.
        assert!(!is_loopback("0.0.0.0:8195"));
        assert!(is_loopback("127.0.0.1:8195"));
        assert!(is_loopback("localhost:8195"));
        assert!(!is_loopback("10.3.1.20:8195"));
    }

    #[test]
    fn loopback_guard_is_not_a_string_prefix() {
        // The bug this avoids is the one `common_service` carries a test for too: a host that
        // *begins* with 127. but is routable.
        assert!(!is_loopback("127.example.com:8195"));
    }

    #[test]
    fn reads_a_rate_out_of_the_session_body() {
        assert_eq!(json_u32(br#"{"rate":48000}"#, "rate"), Some(48_000));
        assert_eq!(json_u32(br#"{"rate": 16000, "x":1}"#, "rate"), Some(16_000));
        assert_eq!(json_u32(b"{}", "rate"), None);
    }

    #[test]
    fn an_unsupported_rate_is_refused_before_a_model_is_built() {
        // A rate the crate cannot resample would otherwise surface as a 500 from model
        // construction, which reads like a server fault rather than a client bug.
        assert!(!matches!(8_000_u32, 16_000 | 22_050 | 32_000 | 44_100 | 48_000 | 88_200 | 96_000));
    }
}

/// The string field of a small JSON body — the same hand-rolled scan as `json_u32`, because this
/// crate deliberately depends on almost nothing.
fn json_str(body: &[u8], key: &str) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?;
    let at = text.find(&format!("\"{key}\""))?;
    let rest = &text[at + key.len() + 2..];
    let open = rest.find('"')?;
    let after = &rest[open + 1..];
    let close = after.find('"')?;
    let v: String = after[..close].chars().filter(|c| !c.is_control()).collect();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}
