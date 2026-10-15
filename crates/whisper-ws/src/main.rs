//! whisper-ws — WebSocket gateway for whisper.cpp.
//!
//! Speaks the OpenAI Realtime protocol's transcription subset
//! (see `src/realtime.rs` for the event list), so existing Realtime clients
//! can point their WebSocket at this box instead of OpenAI:
//!
//!   ws://<host>:8083/v1/realtime?model=whisper-1
//!
//! Wake-word scanning (the status page's wake word card) uses an extra event:
//!   wake_word.probe / wake_word.probe_result — transcribe an exact audio
//!   window without touching the session buffer or the status dashboard.
//!
//! Audio flow:
//!   client ──base64 pcm16 @ 24 kHz──▶ whisper-ws ──wav @ 16 kHz──▶ whisper-server
//!                 (input_audio_buffer.append)          (POST /inference)
//!
//! Plain HTTP on the same port serves the status page (a Solid.js +
//! TypeScript app built under web/app, embedded at compile time):
//!   GET /            the dashboard (health, live sessions, recent transcriptions)
//!   GET /assets/*    the app bundle
//!   GET /engine/* /ort/* /models/*   vendored OpenWakeWord / onnxruntime assets
//!   GET /status.json JSON snapshot (polled by the page)
//!   GET /health      machine-readable health
//!
//! Deliberately zero external crates (house style): WebSocket, JSON, base64,
//! SHA-1 and the HTTP client are all implemented here on the stdlib.
//!
//! Usage:
//!   whisper-ws [--bind 127.0.0.1] [--port 8083]
//!              [--whisper-url http://127.0.0.1:8082] [--max-window-secs 120]

mod assets; // GENERATED — see web/app/gen_assets.sh (web/app/dist tree)
mod audio;
mod base64;
mod http;
mod json;
mod realtime;
mod sha1;
mod status;
mod whisper;
mod ws;

use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::status::WhisperHealth;

fn main() {
    let mut bind = "127.0.0.1".to_string();
    let mut port: u16 = 8083;
    let mut whisper_url = "http://127.0.0.1:8082".to_string();
    let mut max_window_secs: u32 = 120;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut next = || args.next().unwrap_or_else(|| {
            eprintln!("missing value for {arg}");
            std::process::exit(2);
        });
        match arg.as_str() {
            "--bind" => bind = next(),
            "--port" => {
                port = next().parse().unwrap_or_else(|_| {
                    eprintln!("bad --port value");
                    std::process::exit(2);
                })
            }
            "--whisper-url" => whisper_url = next(),
            "--max-window-secs" => {
                max_window_secs = next().parse().unwrap_or_else(|_| {
                    eprintln!("bad --max-window-secs value");
                    std::process::exit(2);
                })
            }
            "--help" | "-h" => {
                println!(
                    "usage: whisper-ws [--bind ADDR] [--port N] [--whisper-url URL] [--max-window-secs N]"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    // Shared status store; the page + /status.json read it, connections and
    // the health probe write to it.
    let store = Arc::new(Mutex::new(status::StatusStore::new(
        whisper_url.clone(),
        WhisperHealth {
            ok: false,
            latency_ms: 0,
            error: Some("not yet probed".into()),
            checked_at: Instant::now(),
        },
    )));

    // Background probe of whisper-server's /health (10 s interval).
    {
        let store = store.clone();
        let url = whisper_url.clone();
        thread::spawn(move || loop {
            let health = probe_whisper(&url);
            if let Ok(mut s) = store.lock() {
                s.whisper = health;
            }
            thread::sleep(Duration::from_secs(10));
        });
    }

    let listener = TcpListener::bind((bind.as_str(), port)).unwrap_or_else(|e| {
        eprintln!("cannot bind {bind}:{port}: {e}");
        std::process::exit(1);
    });
    eprintln!(
        "whisper-ws listening on ws://{bind}:{port}/v1/realtime → {whisper_url} (max window {max_window_secs}s)"
    );

    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let cfg = realtime::Config {
                    whisper: whisper::WhisperClient { base_url: whisper_url.clone() },
                    max_window_secs,
                    status: store.clone(),
                };
                thread::spawn(move || {
                    let mut s = stream;
                    realtime::handle(&mut s, cfg);
                });
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
}

/// Probe whisper-server's `/health` endpoint, returning the latency and any
/// error. Called by the background thread every 10 seconds.
fn probe_whisper(url: &str) -> WhisperHealth {
    let started = Instant::now();
    let result = (|| -> Result<u64, String> {
        let (host, port) = whisper::parse_base_url(url)?;
        let (status, _) = http::http_get(&host, port, "/health")?;
        if status != 200 {
            return Err(format!("whisper-server /health returned HTTP {status}"));
        }
        Ok(started.elapsed().as_millis() as u64)
    })();
    match result {
        Ok(latency_ms) => WhisperHealth {
            ok: true,
            latency_ms,
            error: None,
            checked_at: Instant::now(),
        },
        Err(e) => WhisperHealth {
            ok: false,
            latency_ms: 0,
            error: Some(e),
            checked_at: Instant::now(),
        },
    }
}
