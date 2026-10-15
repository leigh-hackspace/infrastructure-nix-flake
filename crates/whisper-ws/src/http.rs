//! Minimal HTTP/1.1 plumbing, stdlib only.
//!
//! Three halves:
//!   1. Read/parse an inbound request head for the WebSocket upgrade. This
//!      reads byte-by-byte so it never consumes past the `\r\n\r\n` terminator
//!      — the WebSocket frames start immediately after.
//!   2. A tiny client (`http_post` / `http_get`) used to talk to whisper-server
//!      (audio uploads, `/health` probes).
//!   3. A small server-side response writer for the status page endpoints.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub struct RequestHead {
    pub method: String,
    pub path: String,
    /// Lower-cased header names.
    pub headers: Vec<(String, String)>,
}

impl RequestHead {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

const MAX_HEAD_BYTES: usize = 16 * 1024;

/// Read one request head without over-reading (see module docs).
pub fn read_request_head(stream: &mut TcpStream) -> Result<RequestHead, String> {
    let mut buf = Vec::new();
    loop {
        let mut b = [0u8; 1];
        stream
            .read_exact(&mut b)
            .map_err(|e| format!("read request head: {e}"))?;
        buf.push(b[0]);
        if buf.len() >= 4 && &buf[buf.len() - 4..] == b"\r\n\r\n" {
            break;
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err("request head too large".into());
        }
    }
    let head = String::from_utf8(buf).map_err(|_| "request head is not UTF-8".to_string())?;
    parse_request_head(&head)
}

pub fn parse_request_head(head: &str) -> Result<RequestHead, String> {
    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or("empty request")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("missing method")?.to_string();
    let path = parts.next().ok_or("missing path")?.to_string();
    let mut headers = Vec::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_lowercase(), v.trim().to_string()));
        }
    }
    Ok(RequestHead { method, path, headers })
}

/// Write a complete server response (connection closed afterwards).
pub fn write_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &str) {
    write_response_bytes(stream, status, content_type, body.as_bytes());
}

/// Like [`write_response`] but for binary bodies (e.g. recorded WAV audio).
pub fn write_response_bytes(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "Status",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

/// Shared client plumbing: write a request (head + optional body), read the
/// full response. whisper-server (cpp-httplib) always sends Content-Length, so
/// we trust it; without one we fall back to reading until EOF (Connection:
/// close).
fn http_exchange(host: &str, port: u16, head: &str, body: &[u8]) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect((host, port))
        .map_err(|e| format!("connect {host}:{port}: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(120))).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(30))).map_err(|e| e.to_string())?;

    stream.write_all(head.as_bytes()).map_err(|e| format!("write request: {e}"))?;
    stream.write_all(body).map_err(|e| format!("write body: {e}"))?;
    stream.flush().map_err(|e| format!("flush: {e}"))?;

    // Status line + headers.
    let mut head_buf = Vec::new();
    loop {
        let mut b = [0u8; 1];
        match stream.read(&mut b) {
            Ok(0) => return Err("connection closed while reading headers".into()),
            Ok(_) => {
                head_buf.push(b[0]);
                if head_buf.len() >= 4 && &head_buf[head_buf.len() - 4..] == b"\r\n\r\n" {
                    break;
                }
                if head_buf.len() > MAX_HEAD_BYTES {
                    return Err("response head too large".into());
                }
            }
            Err(e) => return Err(format!("read response head: {e}")),
        }
    }
    let head = String::from_utf8_lossy(&head_buf);
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .ok_or("response has no status")?
        .parse()
        .map_err(|_| "bad status line")?;

    let mut content_length: Option<usize> = None;
    for line in head.split("\r\n").skip(1) {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().ok();
            }
        }
    }

    let mut body_bytes = Vec::new();
    match content_length {
        Some(n) => {
            body_bytes.resize(n, 0);
            stream.read_exact(&mut body_bytes).map_err(|e| format!("read body: {e}"))?;
        }
        None => {
            stream.read_to_end(&mut body_bytes).map_err(|e| format!("read body: {e}"))?;
        }
    }

    Ok((status, String::from_utf8_lossy(&body_bytes).to_string()))
}

/// POST a body to `host:port/path`; returns `(status, body-as-utf8)`.
pub fn http_post(
    host: &str,
    port: u16,
    path: &str,
    content_type: &str,
    body: &[u8],
) -> Result<(u16, String), String> {
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    http_exchange(host, port, &head, body)
}

/// GET `host:port/path`; returns `(status, body-as-utf8)`.
pub fn http_get(host: &str, port: u16, path: &str) -> Result<(u16, String), String> {
    let head = format!("GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
    http_exchange(host, port, &head, &[])
}
