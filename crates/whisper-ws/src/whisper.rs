//! Client for the whisper.cpp HTTP server (POST /inference).
//!
//! whisper-server's per-request options are multipart form fields, *not* query
//! parameters (verified against its source). We always ask for
//! `response_format=verbose_json` and read the top-level `text` field.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::json::Value;

static BOUNDARY_SEQ: AtomicU64 = AtomicU64::new(0);

pub struct WhisperClient {
    /// Base URL, e.g. `http://127.0.0.1:8082` (no trailing slash).
    pub base_url: String,
}

impl WhisperClient {
    /// Upload a WAV and return the transcribed text.
    pub fn transcribe(&self, wav: &[u8], language: Option<&str>) -> Result<String, String> {
        let (host, port) = parse_base_url(&self.base_url)?;
        let boundary = format!(
            "whisper-ws-{}-{}",
            std::process::id(),
            BOUNDARY_SEQ.fetch_add(1, Ordering::Relaxed)
        );

        let mut body = Vec::new();
        let file_head = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        );
        body.extend_from_slice(file_head.as_bytes());
        body.extend_from_slice(wav);
        body.extend_from_slice(b"\r\n");

        let lang = language.map(str::to_string);
        let mut fields: Vec<(&str, &str)> = vec![("response_format", "verbose_json")];
        if let Some(ref l) = lang {
            fields.push(("language", l));
        }
        for (name, value) in fields {
            let part = format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            );
            body.extend_from_slice(part.as_bytes());
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

        let content_type = format!("multipart/form-data; boundary={boundary}");
        let (status, resp) =
            crate::http::http_post(&host, port, "/inference", &content_type, &body)?;
        if status != 200 {
            return Err(format!(
                "whisper-server returned HTTP {status}: {}",
                truncate(&resp, 200)
            ));
        }
        let v = Value::parse(&resp).map_err(|e| format!("bad JSON from whisper-server: {e}"))?;
        v.get("text")
            .and_then(|t| t.as_str())
            .map(|s| s.trim().to_string())
            .ok_or_else(|| "whisper-server response has no 'text' field".to_string())
    }
}

/// Split `http://host:port` into `(host, port)`.
pub fn parse_base_url(url: &str) -> Result<(String, u16), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("unsupported URL (need http://host:port): {url}"))?;
    let (host, port) = match rest.split_once(':') {
        Some((h, p)) => {
            let port: u16 = p.parse().map_err(|_| format!("bad port in URL: {url}"))?;
            (h.to_string(), port)
        }
        None => (rest.to_string(), 80),
    };
    if host.is_empty() {
        return Err(format!("empty host in URL: {url}"));
    }
    Ok((host, port))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}
