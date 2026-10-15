//! Status store, `/status.json` snapshot, `/health`, and the status page.
//!
//! One `StatusStore` is shared (behind a mutex) by every connection thread and
//! the whisper-server health probe. The page itself is a Solid.js +
//! TypeScript app (web/app) that polls `/status.json` — no SSE machinery
//! needed for a LAN dashboard. The built dist tree (bundle + vendored
//! OpenWakeWord engine / onnxruntime-web / ONNX models) is embedded via the
//! generated `src/assets.rs` (see web/app/gen_assets.sh), so the binary is
//! fully self-contained.

use std::collections::{HashMap, VecDeque};
use std::net::TcpStream;
use std::time::Instant;

use crate::http::RequestHead;
use crate::json::Value;

/// How many completed transcriptions the page keeps around.
const RECENT_CAP: usize = 20;

pub struct WhisperHealth {
    pub ok: bool,
    pub latency_ms: u64,
    pub error: Option<String>,
    pub checked_at: Instant,
}

/// A transcription currently being processed (commit in flight).
pub struct ItemProgress {
    pub item_id: String,
    pub started_at: Instant,
    pub audio_secs: f32,
}

pub struct SessionInfo {
    pub connected_at: Instant,
    pub last_event_at: Instant,
    pub buffered_secs: f32,
    pub current: Option<ItemProgress>,
}

pub struct CompletedItem {
    pub item_id: String,
    pub session_id: String,
    pub completed_at: Instant,
    pub audio_secs: f32,
    pub duration_ms: u64,
    pub language: Option<String>,
    pub transcript: String,
    pub ok: bool,
    pub error: Option<String>,
    /// The recorded audio as 16 kHz mono WAV — exactly what whisper-server
    /// heard. Empty for clips too short to transcribe (no round-trip).
    pub audio: Vec<u8>,
}

pub struct StatusStore {
    pub started: Instant,
    pub whisper_url: String,
    pub whisper: WhisperHealth,
    pub sessions: HashMap<String, SessionInfo>,
    pub recent: VecDeque<CompletedItem>,
    pub sessions_total: u64,
    pub transcriptions_total: u64,
    pub failed_total: u64,
}

impl StatusStore {
    pub fn new(whisper_url: String, whisper: WhisperHealth) -> StatusStore {
        StatusStore {
            started: Instant::now(),
            whisper_url,
            whisper,
            sessions: HashMap::new(),
            recent: VecDeque::new(),
            sessions_total: 0,
            transcriptions_total: 0,
            failed_total: 0,
        }
    }

    pub fn session_opened(&mut self, id: &str) {
        let now = Instant::now();
        self.sessions.insert(
            id.to_string(),
            SessionInfo {
                connected_at: now,
                last_event_at: now,
                buffered_secs: 0.0,
                current: None,
            },
        );
        self.sessions_total += 1;
    }

    pub fn session_closed(&mut self, id: &str) {
        self.sessions.remove(id);
    }

    pub fn session_touched(&mut self, id: &str, buffered_secs: f32) {
        if let Some(s) = self.sessions.get_mut(id) {
            s.last_event_at = Instant::now();
            s.buffered_secs = buffered_secs;
        }
    }

    pub fn commit_started(&mut self, session_id: &str, item_id: &str, audio_secs: f32) {
        if let Some(s) = self.sessions.get_mut(session_id) {
            s.current = Some(ItemProgress {
                item_id: item_id.to_string(),
                started_at: Instant::now(),
                audio_secs,
            });
        }
    }

    pub fn commit_finished(&mut self, item: CompletedItem) {
        let failed = !item.ok;
        if let Some(s) = self.sessions.get_mut(&item.session_id) {
            s.current = None;
            s.buffered_secs = 0.0;
        }
        self.transcriptions_total += 1;
        if failed {
            self.failed_total += 1;
        }
        self.recent.push_front(item);
        while self.recent.len() > RECENT_CAP {
            self.recent.pop_back();
        }
    }

    /// The recorded WAV for a completed item, if still in the recent window.
    pub fn audio_for(&self, session_id: &str, item_id: &str) -> Option<&[u8]> {
        self.recent
            .iter()
            .find(|c| c.session_id == session_id && c.item_id == item_id)
            .map(|c| c.audio.as_slice())
    }

    pub fn health_json(&self) -> String {
        let status = if self.whisper.ok { "ok" } else { "degraded" };
        let whisper = if self.whisper.ok {
            "ok".to_string()
        } else {
            self.whisper.error.clone().unwrap_or_else(|| "unreachable".to_string())
        };
        Value::object(vec![
            ("status", Value::Str(status.into())),
            ("whisper", Value::Str(whisper)),
            ("connections", Value::Num(self.sessions.len() as f64)),
            ("uptime_secs", Value::Num(self.started.elapsed().as_secs_f64())),
        ])
        .to_json()
    }

    pub fn snapshot(&self) -> Value {
        let now = Instant::now();
        let mut sessions = Vec::new();
        for (id, s) in &self.sessions {
            let current = match &s.current {
                Some(p) => Value::object(vec![
                    ("item_id", Value::Str(p.item_id.clone())),
                    ("elapsed_secs", Value::Num(now.duration_since(p.started_at).as_secs_f64())),
                    ("audio_secs", Value::Num(f64::from(p.audio_secs))),
                ]),
                None => Value::Null,
            };
            sessions.push(Value::object(vec![
                ("id", Value::Str(id.clone())),
                ("connected_secs", Value::Num(now.duration_since(s.connected_at).as_secs_f64())),
                (
                    "last_event_secs_ago",
                    Value::Num(now.duration_since(s.last_event_at).as_secs_f64()),
                ),
                ("buffered_secs", Value::Num(f64::from(s.buffered_secs))),
                ("current_item", current),
            ]));
        }

        let recent = self
            .recent
            .iter()
            .map(|c| {
                Value::object(vec![
                    ("item_id", Value::Str(c.item_id.clone())),
                    ("session_id", Value::Str(c.session_id.clone())),
                    (
                        "completed_secs_ago",
                        Value::Num(now.duration_since(c.completed_at).as_secs_f64()),
                    ),
                    ("audio_secs", Value::Num(f64::from(c.audio_secs))),
                    ("duration_ms", Value::Num(c.duration_ms as f64)),
                    (
                        "language",
                        c.language.clone().map_or(Value::Null, Value::Str),
                    ),
                    ("transcript", Value::Str(c.transcript.clone())),
                    ("ok", Value::Bool(c.ok)),
                    ("error", c.error.clone().map_or(Value::Null, Value::Str)),
                    ("has_audio", Value::Bool(!c.audio.is_empty())),
                ])
            })
            .collect();

        Value::object(vec![
            ("uptime_secs", Value::Num(now.duration_since(self.started).as_secs_f64())),
            ("whisper_url", Value::Str(self.whisper_url.clone())),
            (
                "whisper",
                Value::object(vec![
                    ("ok", Value::Bool(self.whisper.ok)),
                    ("latency_ms", Value::Num(self.whisper.latency_ms as f64)),
                    (
                        "checked_secs_ago",
                        Value::Num(now.duration_since(self.whisper.checked_at).as_secs_f64()),
                    ),
                    ("error", self.whisper.error.clone().map_or(Value::Null, Value::Str)),
                ]),
            ),
            ("connections", Value::Num(self.sessions.len() as f64)),
            ("sessions_total", Value::Num(self.sessions_total as f64)),
            ("transcriptions_total", Value::Num(self.transcriptions_total as f64)),
            ("failed_total", Value::Num(self.failed_total as f64)),
            ("sessions", Value::Arr(sessions)),
            ("recent", Value::Arr(recent)),
        ])
    }
}

/// Route a plain (non-WebSocket) GET request: status page / JSON / health.
///
/// Everything except the dynamic endpoints (`/status.json`, `/health`,
/// `/audio/*`) is served from the embedded web/app dist tree (src/assets.rs):
/// `/` → the Solid.js status page, `/assets/*` its bundle, and `/engine/*`,
/// `/ort/*`, `/models/*` the vendored OpenWakeWord engine, onnxruntime-web
/// and ONNX models (see web/openwakeword/README.md).
pub fn serve_http(stream: &mut TcpStream, head: &RequestHead, store: &std::sync::Mutex<StatusStore>) {
    if head.method != "GET" {
        crate::http::write_response(stream, 405, "text/plain", "method not allowed");
        return;
    }
    let path = head.path.split('?').next().unwrap_or("/");
    let s = store.lock().unwrap();

    // /audio/<session_id>/<item_id>.wav — recorded audio for a recent
    // transcription, as the 16 kHz mono WAV whisper-server heard.
    if let Some(rest) = path.strip_prefix("/audio/") {
        match rest.strip_suffix(".wav").and_then(|r| r.split_once('/')) {
            Some((sid, iid)) => match s.audio_for(sid, iid) {
                Some(audio) if !audio.is_empty() => {
                    crate::http::write_response_bytes(stream, 200, "audio/wav", audio);
                }
                _ => crate::http::write_response(stream, 404, "text/plain", "audio not found"),
            },
            None => crate::http::write_response(stream, 404, "text/plain", "not found"),
        }
        return;
    }

    match path {
        "/status.json" => {
            let body = s.snapshot().to_json();
            crate::http::write_response(stream, 200, "application/json", &body);
        }
        "/health" => {
            let body = s.health_json();
            crate::http::write_response(stream, 200, "application/json", &body);
        }
        _ => match crate::assets::asset(path.trim_start_matches('/')) {
            Some((bytes, content_type)) => {
                crate::http::write_response_bytes(stream, 200, content_type, bytes);
            }
            None => crate::http::write_response(stream, 404, "text/plain", "not found"),
        },
    }
}

