//! Hand-rolled HTTP server (house style — no web framework): JSON API,
//! event JPEGs, and the embedded Dioxus SPA built by build.rs from
//! frontend/dist.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::thread;

use crate::events;

pub struct Shared {
    pub data_dir: std::path::PathBuf,
    pub frames: u64,
    pub last_frame_ts: i64,
    pub scene_resets: u64,
    /// Captured frames that looked like stream glitches (solid colour) and
    /// were ignored instead of being analysed/buffered.
    pub blank_frames: u64,
    /// Events the detector fired but we did not store because no usable
    /// "before" snapshot existed (background reset mid-change / stale born).
    pub skipped_no_before: u64,
    pub last_error: String,
    pub latest: Option<Vec<u8>>,
}

fn read_request(stream: &mut TcpStream) -> Option<(String, String)> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16_384 {
            let head = String::from_utf8_lossy(&buf).to_string();
            let first = head.lines().next().unwrap_or("");
            let mut parts = first.split_whitespace();
            let method = parts.next().unwrap_or("").to_string();
            let target = parts.next().unwrap_or("/").to_string();
            return Some((method, target));
        }
    }
}

fn send(stream: &mut TcpStream, status: &str, content_type: &str, body: Vec<u8>) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
}

fn send_file(stream: &mut TcpStream, path: &Path, content_type: &str) {
    match std::fs::read(path) {
        Ok(b) => send(stream, "200 OK", content_type, b),
        Err(_) => send(stream, "404 Not Found", "text/plain", b"not found".to_vec()),
    }
}

fn not_found(stream: &mut TcpStream) {
    send(stream, "404 Not Found", "text/plain", b"not found".to_vec());
}

pub fn run(cfg: crate::Config, shared: Arc<RwLock<Shared>>) {
    let addr = format!("{}:{}", cfg.bind, cfg.port);
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("bind {addr}: {e}");
            return;
        }
    };
    eprintln!("frigate-monitor listening on http://{addr}");
    for stream in listener.incoming() {
        if let Ok(stream) = stream {
            let shared = Arc::clone(&shared);
            thread::spawn(move || handle_client(stream, shared));
        }
    }
}

fn handle_client(mut stream: TcpStream, shared: Arc<RwLock<Shared>>) {
    let (method, target) = match read_request(&mut stream) {
        Some(t) => t,
        None => return,
    };
    if method != "GET" {
        send(&mut stream, "405 Method Not Allowed", "text/plain", b"get only".to_vec());
        return;
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (target.clone(), None),
    };
    let s = shared.read().unwrap();

    match path.as_str() {
        "/" => serve_asset(&mut stream, "index.html"),
        "/api/status" => {
            let events_total = events::event_ids(&s.data_dir).len() as u64;
            let body = format!(
                "{{\"frames\":{},\"last_frame_ts\":{},\"scene_resets\":{},\"blank_frames\":{},\"skipped_no_before\":{},\"events\":{},\"last_error\":{}}}",
                s.frames,
                s.last_frame_ts,
                s.scene_resets,
                s.blank_frames,
                s.skipped_no_before,
                events_total,
                events::json_str(&s.last_error)
            );
            send(&mut stream, "200 OK", "application/json", body.into_bytes());
        }
        "/api/events" => {
            let q = query.as_deref().unwrap_or("");
            let limit = param_u64(q, "limit").unwrap_or(24).clamp(1, 200) as usize;
            let before = param_u64(q, "before");
            let (metas, has_more) = events::list_events_paged(&s.data_dir, before, limit);
            let body = format!(
                "{{\"has_more\":{},\"events\":[{}]}}",
                has_more,
                metas.join(",")
            );
            send(&mut stream, "200 OK", "application/json", body.into_bytes());
        }
        "/api/live" => match &s.latest {
            Some(bytes) => send(&mut stream, "200 OK", "image/jpeg", bytes.clone()),
            None => send(&mut stream, "503 Service Unavailable", "text/plain", b"no frame yet".to_vec()),
        },
        "/api/events/" => not_found(&mut stream),
        _ => {
            if path.starts_with("/api/events/") {
                let rest = &path["/api/events/".len()..];
                if let Ok(id) = rest.parse::<u64>() {
                    let dir = events::event_dir(&s.data_dir, id);
                    if let Some(meta) = events::read_meta(&dir) {
                        send(&mut stream, "200 OK", "application/json", meta.into_bytes());
                        return;
                    }
                }
                not_found(&mut stream);
            } else if path.starts_with("/files/") {
                let rest = &path["/files/".len()..];
                let parts: Vec<&str> = rest.split('/').collect();
                if parts.len() == 2 {
                    if let Ok(id) = parts[0].parse::<u64>() {
                        let dir = events::event_dir(&s.data_dir, id);
                        if events::EVENT_FILES.contains(&parts[1]) && dir.join(parts[1]).is_file() {
                            send_file(&mut stream, &dir.join(parts[1]), "image/jpeg");
                            return;
                        }
                    }
                }
                not_found(&mut stream);
            } else {
                // SPA asset (a top-level or nested file from the embedded
                // dist, looked up by exact name in the asset table).
                let name = path.trim_start_matches('/');
                if !name.contains("..") {
                    serve_asset(&mut stream, name);
                } else {
                    not_found(&mut stream);
                }
            }
        }
    }
}

fn param_u64(query: &str, key: &str) -> Option<u64> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| v.parse().ok()).flatten()
    })
}

fn serve_asset(stream: &mut TcpStream, name: &str) {
    match crate::assets::find(name) {
        Some(a) => send(stream, "200 OK", a.mime, a.data.to_vec()),
        None => not_found(stream),
    }
}


