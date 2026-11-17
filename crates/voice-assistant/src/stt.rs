//! The speech-to-text side: one POST of a WAV to whisper.cpp, one transcript back.
//!
//! ## Why HTTP and not the Realtime WebSocket that already exists
//!
//! `crates/whisper-ws/` speaks the OpenAI Realtime transcription subset on
//! `ws://10.3.1.32:8083/v1/realtime`, and that is the right protocol for the browser
//! page, which streams mic audio continuously and wants a transcript when it commits.
//! For this service the audio is already a bounded window by the time it is sent —
//! `gate.rs` cut it — so the extra protocol buys nothing: one `multipart/form-data`
//! POST to whisper-server's own `POST /inference` answers the question, needs no
//! WebSocket framing, and puts one fewer long-lived session on the aibox box.
//!
//! The measured budget that makes this the last thing to optimise: **3.0 s of real
//! room audio transcribed in 2.7 s of wall clock** (2026-11-16, aibox,
//! whisper.cpp 1.9.5, `ggml-medium.bin`, `-t 8`).  Whisper-server is running **on
//! CPU** — it holds no `/dev/dri` fd, and strata owns the iGPU — so the ~0.9× real
//! time figure is 8 threads of the same 12-core box the LLM tools share.
//!
//! ## Non-speech is a real answer, and it is not a transcript
//!
//! whisper.cpp's habit with music, silence and applause is to return a parenthetical
//! description of the noise rather than text: the first three seconds from the space's microphone,
//! sent to this endpoint during the test, came back as `(soft music)`.  A caller that
//! treats that as speech will "hear" the space's playlist all day, so
//! [`looks_like_description`] is part of this client's contract and the caller must
//! ask it.

use std::time::{Duration, Instant};

use common_json::Json;

/// The default whisper-server origin (aibox; `machines/aibox/whisper.nix` binds it
/// to that address on purpose, so `127.0.0.1` on aibox is not a thing).
pub const DEFAULT_URL: &str = "http://10.3.1.32:8082";

/// A transcription takes seconds, not milliseconds, and a long window legitimately
/// takes tens of them.  This is a request timeout, not a budget: the caller's own
/// deadline for "the user is still waiting" is elsewhere.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug)]
pub struct Transcript {
    pub text: String,
    /// whisper's own report of how long the audio was, seconds.  Compared against
    /// the length we sent, a mismatch is a good early sign the audio was not what
    /// we thought it was.
    pub duration_secs: Option<f64>,
    pub language: Option<String>,
    pub wall: Duration,
}

impl Transcript {
    /// True when whisper described the noise instead of transcribing speech.
    pub fn is_description(&self) -> bool {
        looks_like_description(&self.text)
    }
}

pub struct Client {
    pub url: String,
    pub language: Option<String>,
    pub timeout: Duration,
}

impl Client {
    pub fn new(url: &str, language: Option<String>) -> Client {
        Client {
            url: url.trim_end_matches('/').to_string(),
            language,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// One window, one transcript.  `wav` must be a PCM16 WAV (see `crate::wav`).
    pub fn transcribe(&self, wav: &[u8]) -> Result<Transcript, String> {
        let started = Instant::now();
        let body = multipart(wav, self.language.as_deref());
        let ctype = format!("multipart/form-data; boundary={BOUNDARY}");
        let reply = common_http::request_bytes(
            "POST",
            &format!("{}/inference", self.url),
            Some(&body),
            &ctype,
            &[],
            self.timeout,
        )
        .map_err(|e| format!("whisper: {e}"))?;
        if !reply.ok() {
            return Err(format!("whisper: {} {}", reply.status, reply.describe()));
        }
        let value = common_json::parse(&reply.body).map_err(|e| {
            format!(
                "whisper returned something that is not the JSON we asked for ({e}); first 120 \
                 bytes: {}",
                reply.body.chars().take(120).collect::<String>()
            )
        })?;
        let text = value
            .get("text")
            .and_then(Json::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        Ok(Transcript {
            text,
            duration_secs: value.get("duration").and_then(Json::as_f64),
            language: value
                .get("language")
                .and_then(Json::as_str)
                .map(|s| s.to_string()),
            wall: started.elapsed(),
        })
    }
}

const BOUNDARY: &str = "voiceassistantwindow7f3a2c";

/// The form body.  whisper.cpp's `/inference` reads its options as form fields
/// alongside the file (`server.cpp`'s `form_field` handling), so the options travel
/// as parts rather than a query string.
///
/// `temperature=0` is the one that matters: whisper's default sampling with a
/// temperature > 0 on a short, noisy window is how the same audio yields a different
/// transcript twice, and a wake word that matches sometimes is worse than one that
/// never matches.
fn multipart(wav: &[u8], language: Option<&str>) -> Vec<u8> {
    let mut out = Vec::with_capacity(wav.len() + 512);
    part(&mut out, "file", Some(("window.wav", "audio/wav")), wav);
    part(&mut out, "response_format", None, b"json");
    part(&mut out, "temperature", None, b"0");
    part(&mut out, "temperature_inc", None, b"0");
    part(&mut out, "no_speech_thold", None, b"0.4");
    part(&mut out, "translate", None, b"false");
    if let Some(l) = language {
        part(&mut out, "language", None, l.as_bytes());
    }
    out.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    out
}

fn part(out: &mut Vec<u8>, name: &str, file: Option<(&str, &str)>, value: &[u8]) {
    out.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    match file {
        Some((filename, ctype)) => out.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n\
                 Content-Type: {ctype}\r\n\r\n"
            )
            .as_bytes(),
        ),
        None => out.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        ),
    }
    out.extend_from_slice(value);
    out.extend_from_slice(b"\r\n");
}

/// `(soft music)`, `(silence)`, `(applause)`, `(bank)`, `…` — a description, not
/// words.  The whole string must be one parenthetical (or a run of them) for this to
/// be true, because "the workshop is (probably) cold" is a real transcript and the
/// caller needs to know that.
pub fn looks_like_description(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return true;
    }
    if t.chars().all(|c| !c.is_alphanumeric()) {
        return true;
    }
    // Strip every balanced parenthetical, then see if any word was left.
    let mut rest = t.to_string();
    while let Some(open) = rest.find(['(', '[']) {
        let Some(close) = rest[open..].find([')', ']']).map(|c| open + c) else {
            break;
        };
        rest.replace_range(open..=close, " ");
    }
    !rest.split_whitespace().any(|w| w.chars().any(|c| c.is_alphanumeric()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    /// A whisper.cpp stand-in: answers with a fixed JSON body and reports the
    /// Content-Type it was given, so the multipart framing is what is under test.
    fn fake_whisper(body: &'static str) -> (String, std::sync::mpsc::Receiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                // Read the head, then exactly Content-Length bytes: a fixture that
                // guesses where the body ends either hangs or answers twice.
                let mut all = Vec::new();
                let mut tmp = [0u8; 8192];
                let mut head_end = None;
                while let Ok(n) = s.read(&mut tmp) {
                    if n == 0 {
                        break;
                    }
                    all.extend_from_slice(&tmp[..n]);
                    if head_end.is_none() {
                        head_end = all.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4);
                    }
                    if let Some(h) = head_end {
                        let want: usize = String::from_utf8_lossy(&all[..h])
                            .lines()
                            .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                            .and_then(|l| l.split(':').nth(1))
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        if all.len() >= h + want {
                            break;
                        }
                    }
                }
                let head = String::from_utf8_lossy(&all).to_string();
                let _ = tx.send(head);
                let out = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(out.as_bytes());
                let _ = s.flush();
            }
        });
        (format!("http://127.0.0.1:{port}"), rx)
    }

    #[test]
    fn a_transcript_comes_back_with_its_timing() {
        let (url, rx) = fake_whisper("{\"text\":\" is the space open\",\"duration\":2.9}");
        let c = Client::new(&url, None);
        let t = c.transcribe(b"RIFFfakeaudio").unwrap();
        assert_eq!(t.text, "is the space open");
        assert_eq!(t.duration_secs, Some(2.9));
        assert!(!t.is_description());
        let head = rx.recv().unwrap();
        assert!(head.contains("multipart/form-data; boundary="), "{head}");
        assert!(head.contains("POST /inference"), "{head}");
    }

    #[test]
    fn the_form_carries_the_options_that_make_a_window_reproducible() {
        let (url, rx) = fake_whisper("{\"text\":\"x\"}");
        let c = Client::new(&url, Some("en".to_string()));
        c.transcribe(b"RIFFfakeaudio").unwrap();
        let head = rx.recv().unwrap();
        for want in [
            "name=\"response_format\"",
            "name=\"temperature\"",
            "name=\"no_speech_thold\"",
        ] {
            assert!(head.contains(want), "missing {want} in {head}");
        }
    }

    #[test]
    fn a_non_json_reply_says_so_with_the_bytes_that_prove_it() {
        let (url, _rx) = fake_whisper("<html>502 Bad Gateway</html>");
        let c = Client::new(&url, None);
        let e = c.transcribe(b"RIFF").unwrap_err();
        assert!(e.contains("not the JSON"), "{e}");
        assert!(e.contains("html"), "{e}");
    }

    #[test]
    fn the_noise_whisper_describes_instead_of_transcribing_is_not_speech() {
        // The measured one first: this is exactly what the space's microphone returned, with the
        // space's music playing, on 2026-11-16.
        assert!(looks_like_description("(soft music)"));
        assert!(looks_like_description("(silence)"));
        assert!(looks_like_description("(applause)  (laughs)"));
        assert!(looks_like_description(""));
        assert!(looks_like_description("  ~~~  "));
        // And the ones that must not be swallowed.
        assert!(!looks_like_description("the workshop is (probably) cold"));
        assert!(!looks_like_description("is the space open"));
        assert!(!looks_like_description("(1) open the door"), "a numbered list is words");
    }

    #[test]
    fn a_trailing_slash_on_the_origin_does_not_double_the_path() {
        assert_eq!(Client::new("http://h:8082/", None).url, "http://h:8082");
    }
}
