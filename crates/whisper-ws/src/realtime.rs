//! Per-connection handler for the OpenAI-Realtime-compatible protocol.
//!
//! Implements the transcription subset of
//! https://platform.openai.com/docs/guides/realtime:
//!
//! Client → server events:
//!   session.update                     (only `input_audio_transcription.language` is honoured)
//!   input_audio_buffer.append          (base64 pcm16 @ 24 kHz)
//!   input_audio_buffer.commit          (transcribe what has been appended)
//!   input_audio_buffer.clear
//!   wake_word.probe                    (transcribe an exact audio window, no buffering)
//!
//! Server → client events:
//!   session.created
//!   input_audio_buffer.committed
//!   input_audio_buffer.cleared
//!   conversation.item.input_audio_transcription.completed
//!   conversation.item.input_audio_transcription.failed
//!   wake_word.probe_result             (wake-word scan result for a probe window)
//!   error
//!
//! Audio is buffered at 16 kHz (resampled from the 24 kHz wire format) and
//! sent to whisper-server as one WAV per commit. The buffer auto-commits once
//! it exceeds `max_window_secs` so a client that never commits cannot grow
//! memory without bound.
//!
//! Plain HTTP GETs on the same port are routed to the status page
//! (`/`, `/status.json`, `/health` — see `status.rs`).

use std::io::Write;
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::audio;
use crate::http;
use crate::json::Value;
use crate::status::{self, CompletedItem, StatusStore};
use crate::whisper::WhisperClient;
use crate::ws::{self, Message, WsError};

/// Samples per second after resampling (16 kHz).
const TARGET_SAMPLES_PER_SEC: usize = audio::TARGET_RATE as usize;

static EVENT_SEQ: AtomicU64 = AtomicU64::new(0);
static SESSION_SEQ: AtomicU64 = AtomicU64::new(0);

fn next_event_id() -> String {
    format!("event_{}", EVENT_SEQ.fetch_add(1, Ordering::Relaxed))
}

pub struct Config {
    pub whisper: WhisperClient,
    /// Auto-commit threshold (seconds of buffered audio).
    pub max_window_secs: u32,
    /// Shared status store, updated as sessions/commits happen.
    pub status: Arc<Mutex<StatusStore>>,
}

struct Session {
    id: String,
    model: String,
    language: Option<String>,
    item_counter: u64,
    previous_item: Option<String>,
    /// 16 kHz mono PCM buffered since the last commit.
    audio: Vec<i16>,
    cfg: Config,
}

/// Entry point: run the whole connection (handshake → events → close).
pub fn handle(stream: &mut TcpStream, cfg: Config) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(300)));

    let head = match http::read_request_head(stream) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("request head: {e}");
            return;
        }
    };

    // A request without `Upgrade: websocket` is the status page/JSON — serve
    // that and close. WebSocket requests continue to the Realtime protocol.
    let wants_ws = head
        .header("upgrade")
        .map(|u| u.to_ascii_lowercase().contains("websocket"))
        .unwrap_or(false);
    if !wants_ws {
        status::serve_http(stream, &head, &cfg.status);
        return;
    }

    let key = match validate_handshake(&head) {
        Ok(k) => k,
        Err(e) => {
            let _ = stream
                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
            eprintln!("handshake rejected: {e}");
            return;
        }
    };

    let accept = ws::accept_key(&key);
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    if let Err(e) = stream.write_all(response.as_bytes()) {
        eprintln!("handshake write: {e}");
        return;
    }

    let model = head
        .path
        .split('?')
        .nth(1)
        .and_then(|q| {
            q.split('&').find_map(|kv| {
                kv.split_once('=')
                    .and_then(|(k, v)| (k == "model").then_some(v.to_string()))
            })
        })
        .unwrap_or_else(|| "whisper-1".to_string());

    let mut session = Session {
        id: format!("sess_{}", SESSION_SEQ.fetch_add(1, Ordering::Relaxed)),
        model,
        language: None,
        item_counter: 0,
        previous_item: None,
        audio: Vec::new(),
        cfg,
    };

    {
        let mut s = session.cfg.status.lock().unwrap();
        s.session_opened(&session.id);
    }

    send_session_created(stream, &session);

    loop {
        match ws::read_message(stream) {
            Ok(Message::Text(text)) => handle_event(&mut session, stream, &text),
            Ok(Message::Binary(b)) => {
                // Not part of the Realtime protocol (audio is base64 in JSON).
                eprintln!("ignoring binary frame ({} bytes)", b.len());
            }
            Err(WsError::Closed { code, reason }) => {
                eprintln!("connection closed: {code} {reason}");
                break;
            }
            Err(WsError::Protocol(e)) => {
                eprintln!("protocol error: {e}");
                let _ = ws::write_close(stream, 1002, "protocol error");
                break;
            }
            Err(WsError::Io(e)) => {
                // Includes the 300 s idle timeout.
                eprintln!("connection io error: {e}");
                break;
            }
        }
    }

    {
        let mut s = session.cfg.status.lock().unwrap();
        s.session_closed(&session.id);
    }
}

fn validate_handshake(head: &http::RequestHead) -> Result<String, String> {
    if head.method != "GET" {
        return Err(format!("method {}", head.method));
    }
    let upgrade = head.header("upgrade").unwrap_or("");
    if !upgrade.to_ascii_lowercase().contains("websocket") {
        return Err("missing Upgrade: websocket".into());
    }
    let connection = head.header("connection").unwrap_or("");
    if !connection.to_ascii_lowercase().contains("upgrade") {
        return Err("missing Connection: Upgrade".into());
    }
    head.header("sec-websocket-key")
        .map(|k| k.to_string())
        .ok_or_else(|| "missing Sec-WebSocket-Key".into())
}

// ---------------------------------------------------------------------------
// Event handling
// ---------------------------------------------------------------------------

fn handle_event(session: &mut Session, stream: &mut TcpStream, text: &str) {
    let v = match Value::parse(text) {
        Ok(v) => v,
        Err(e) => {
            send_error(stream, "invalid_request_error", &format!("invalid JSON: {e}"));
            return;
        }
    };
    let event_type = match v.get("type").and_then(|t| t.as_str()) {
        Some(t) => t,
        None => {
            send_error(stream, "invalid_request_error", "event missing 'type'");
            return;
        }
    };

    match event_type {
        "session.update" => handle_session_update(session, &v),
        "input_audio_buffer.append" => handle_append(session, stream, &v),
        "input_audio_buffer.commit" => commit(session, stream),
        "input_audio_buffer.clear" => {
            session.audio.clear();
            {
                let mut s = session.cfg.status.lock().unwrap();
                s.session_touched(&session.id, 0.0);
            }
            send(
                stream,
                Value::object(vec![
                    ("type", Value::Str("input_audio_buffer.cleared".into())),
                    ("event_id", Value::Str(next_event_id())),
                ]),
            );
        }
        "wake_word.probe" => handle_wake_probe(session, stream, &v),
        // Events that only make sense against an LLM session.
        "response.create"
        | "response.cancel"
        | "conversation.item.create"
        | "conversation.item.truncate"
        | "conversation.item.delete" => {
            send_error(
                stream,
                "invalid_request_error",
                &format!("'{event_type}' is not supported (transcription-only service)"),
            );
        }
        other => {
            send_error(stream, "invalid_request_error", &format!("unknown event type '{other}'"));
        }
    }
}

fn handle_session_update(session: &mut Session, v: &Value) {
    if let Some(sess) = v.get("session") {
        if let Some(lang) = sess
            .get("input_audio_transcription")
            .and_then(|t| t.get("language"))
            .and_then(|l| l.as_str())
        {
            session.language = Some(lang.to_string());
        }
    }
}

fn handle_append(session: &mut Session, stream: &mut TcpStream, v: &Value) {
    let audio = match v.get("audio").and_then(|a| a.as_str()) {
        Some(a) => a,
        None => {
            send_error(stream, "invalid_request_error", "input_audio_buffer.append requires 'audio'");
            return;
        }
    };
    let samples = match audio::pcm16_from_base64(audio) {
        Ok(s) => s,
        Err(e) => {
            send_error(stream, "invalid_request_error", &format!("bad audio payload: {e}"));
            return;
        }
    };
    let resampled = audio::resample(&samples, audio::INPUT_RATE, audio::TARGET_RATE);
    session.audio.extend_from_slice(&resampled);

    let buffered_secs = session.audio.len() as f32 / TARGET_SAMPLES_PER_SEC as f32;
    {
        let mut s = session.cfg.status.lock().unwrap();
        s.session_touched(&session.id, buffered_secs);
    }

    // Auto-commit once the window cap is hit.
    let cap = (session.cfg.max_window_secs as usize) * TARGET_SAMPLES_PER_SEC;
    if session.audio.len() >= cap {
        commit(session, stream);
    }
}

/// Transcribe an exact audio window (base64 pcm16 @ 24 kHz) for wake-word
/// scanning, without touching the session's append buffer or the status
/// dashboard. The client keeps the rolling window client-side and sends the
/// audio it wants checked. Responds with `wake_word.probe_result` carrying
/// the raw transcript plus a `detected` flag.
fn handle_wake_probe(session: &mut Session, stream: &mut TcpStream, v: &Value) {
    let wake_word = v
        .get("wake_word")
        .and_then(|w| w.as_str())
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .unwrap_or("computer");
    let audio = match v.get("audio").and_then(|a| a.as_str()) {
        Some(a) => a,
        None => {
            send_error(stream, "invalid_request_error", "wake_word.probe requires 'audio'");
            return;
        }
    };
    let samples = match audio::pcm16_from_base64(audio) {
        Ok(s) => s,
        Err(e) => {
            send_error(stream, "invalid_request_error", &format!("bad audio payload: {e}"));
            return;
        }
    };
    let resampled = audio::resample(&samples, audio::INPUT_RATE, audio::TARGET_RATE);

    // Too short to say anything: answer immediately rather than round-tripping
    // a near-empty WAV through whisper-server (mirrors the commit guard).
    let result = if resampled.len() < 1000 {
        Ok(String::new())
    } else {
        let wav = audio::wav_bytes(&resampled, audio::TARGET_RATE);
        session
            .cfg
            .whisper
            .transcribe(&wav, session.language.as_deref())
    };

    let mut fields = vec![
        ("type", Value::Str("wake_word.probe_result".into())),
        ("event_id", Value::Str(next_event_id())),
    ];
    match result {
        Ok(transcript) => {
            fields.push(("detected", Value::Bool(wake_word_matches(&transcript, wake_word))));
            fields.push(("transcript", Value::Str(transcript)));
        }
        Err(e) => {
            fields.push(("detected", Value::Bool(false)));
            fields.push(("error", Value::Str(e)));
        }
    }
    send(stream, Value::object(fields));
}

/// Case- and punctuation-insensitive wake-word match. Hyphens join the word
/// back together (whisper splits "computer" as "com-puter" at syllable
/// boundaries); any other non-alphanumeric character is a word separator.
/// "Computer," hits; "computers" does not.
fn wake_word_matches(transcript: &str, wake_word: &str) -> bool {
    let norm = |s: &str| -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            if c.is_alphanumeric() {
                out.extend(c.to_lowercase());
            } else if c != '-' {
                out.push(' ');
            }
        }
        out
    };
    let target = norm(wake_word);
    if target.is_empty() {
        return false;
    }
    norm(transcript).split_whitespace().any(|w| w == target)
}

/// Transcribe the buffered audio as one item, then clear the buffer.
fn commit(session: &mut Session, stream: &mut TcpStream) {
    let item_id = format!("item_{}", session.item_counter);
    session.item_counter += 1;
    let previous_item_id = session.previous_item.clone().unwrap_or_default();
    session.previous_item = Some(item_id.clone());

    let samples = std::mem::take(&mut session.audio);
    let audio_secs = samples.len() as f32 / TARGET_SAMPLES_PER_SEC as f32;
    let start = Instant::now();

    {
        let mut s = session.cfg.status.lock().unwrap();
        s.commit_started(&session.id, &item_id, audio_secs);
    }

    send(
        stream,
        Value::object(vec![
            ("type", Value::Str("input_audio_buffer.committed".into())),
            ("event_id", Value::Str(next_event_id())),
            ("previous_item_id", Value::Str(previous_item_id)),
            ("item_id", Value::Str(item_id.clone())),
        ]),
    );

    // Nothing (or a whisper of audio): complete with an empty transcript
    // rather than round-tripping an empty WAV through whisper-server. Keep
    // the WAV either way so the status page can play the recording back.
    let (result, wav) = if samples.len() < 1000 {
        (Ok(String::new()), Vec::new())
    } else {
        let wav = audio::wav_bytes(&samples, audio::TARGET_RATE);
        (
            session
                .cfg
                .whisper
                .transcribe(&wav, session.language.as_deref()),
            wav,
        )
    };

    record_and_send(
        session,
        stream,
        &item_id,
        audio_secs,
        start.elapsed().as_millis() as u64,
        wav,
        result,
    );
}

/// Record the outcome in the status store, then emit the protocol event.
fn record_and_send(
    session: &mut Session,
    stream: &mut TcpStream,
    item_id: &str,
    audio_secs: f32,
    duration_ms: u64,
    audio_wav: Vec<u8>,
    result: Result<String, String>,
) {
    let (ok, transcript, error) = match result {
        Ok(t) => (true, t, None),
        Err(e) => (false, String::new(), Some(e)),
    };
    {
        let mut s = session.cfg.status.lock().unwrap();
        s.commit_finished(CompletedItem {
            item_id: item_id.to_string(),
            session_id: session.id.clone(),
            completed_at: Instant::now(),
            audio_secs,
            duration_ms,
            language: session.language.clone(),
            transcript: transcript.clone(),
            ok,
            error: error.clone(),
            audio: audio_wav,
        });
    }
    if ok {
        send_transcription(stream, item_id, &transcript);
    } else {
        send(
            stream,
            Value::object(vec![
                ("type", Value::Str("conversation.item.input_audio_transcription.failed".into())),
                ("event_id", Value::Str(next_event_id())),
                ("item_id", Value::Str(item_id.to_string())),
                ("content_index", Value::Num(0.0)),
                (
                    "error",
                    Value::object(vec![
                        ("type", Value::Str("server_error".into())),
                        ("code", Value::Null),
                        ("message", Value::Str(error.unwrap_or_default())),
                        ("param", Value::Null),
                    ]),
                ),
            ]),
        );
    }
}

fn send_transcription(stream: &mut TcpStream, item_id: &str, transcript: &str) {
    send(
        stream,
        Value::object(vec![
            ("type", Value::Str("conversation.item.input_audio_transcription.completed".into())),
            ("event_id", Value::Str(next_event_id())),
            ("item_id", Value::Str(item_id.to_string())),
            ("content_index", Value::Num(0.0)),
            ("transcript", Value::Str(transcript.to_string())),
        ]),
    );
}

fn send_session_created(stream: &mut TcpStream, session: &Session) {
    let session_obj = Value::object(vec![
        ("id", Value::Str(session.id.clone())),
        ("model", Value::Str(session.model.clone())),
        ("modalities", Value::Arr(vec![Value::Str("text".into())])),
        ("instructions", Value::Str(String::new())),
        ("voice", Value::Str(String::new())),
        ("input_audio_format", Value::Str("pcm16".into())),
        (
            "input_audio_transcription",
            Value::object(vec![("model", Value::Str("whisper-1".into()))]),
        ),
        ("turn_detection", Value::Null),
        ("tools", Value::Arr(vec![])),
        ("tool_choice", Value::Str("none".into())),
        ("temperature", Value::Num(0.0)),
        ("max_output_tokens", Value::Null),
    ]);
    send(
        stream,
        Value::object(vec![
            ("type", Value::Str("session.created".into())),
            ("event_id", Value::Str(next_event_id())),
            ("session", session_obj),
        ]),
    );
}

fn send_error(stream: &mut TcpStream, err_type: &str, message: &str) {
    send(
        stream,
        Value::object(vec![
            ("type", Value::Str("error".into())),
            ("event_id", Value::Str(next_event_id())),
            (
                "error",
                Value::object(vec![
                    ("type", Value::Str(err_type.into())),
                    ("code", Value::Null),
                    ("message", Value::Str(message.into())),
                    ("param", Value::Null),
                ]),
            ),
        ]),
    );
}

fn send(stream: &mut TcpStream, v: Value) {
    let text = v.to_json();
    if let Err(e) = ws::write_text(stream, &text) {
        eprintln!("write event: {e}");
    }
}
