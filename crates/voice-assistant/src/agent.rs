//! The client half of the generalised bot: ask a question, get an answer.
//!
//! The answering is done by `slack-agent`, which since 2026-11-17 also serves
//! `POST /v1/ask` on loopback (`machines/services1/services/slack-agent.nix`) for
//! exactly this caller.  Nothing here knows the question came from a voice: the same
//! endpoint answers the browser page, the room satellite and `just voice-ask`.
//!
//! Two facts from `docs/slack-agent-2026-11-16.md` and from measuring, that are the
//! reason this client is written the way it is:
//!
//! - **A turn is seconds.** Measured 2026-11-16: 7.7 s for a one-tool question
//!   (`qwen3.8-flash-next-coder-iq1m` on the RTX 3060, 2 rounds).  The unit's own
//!   `--timeout-secs` default is 120, so the client waits at least that long: a
//!   client that gave up at 30 s would be reporting a fault that had not happened,
//!   and the person speaking would simply never get an answer.
//! - **The answer can be empty legitimately.** `triage.rs` decides some messages are
//!   not for the bot, and `agent.rs` refuses to answer when every MCP server failed
//!   ("any answer would be invented").  Both arrive as an error from the endpoint,
//!   and the caller must not turn one into a spoken "I don't know" — the prompt's
//!   whole grounding rule depends on "I cannot see that" coming from a model that
//!   looked, and not from our own silence.

use std::time::Duration;

/// `slack-agent`'s ask endpoint: `127.0.0.1:8194`, which is `slack-agent listen`
/// (`machines/services1/services/slack-agent.nix`).  8191 is the MCP server and 8192 its
/// health/metrics socket, so this is the fourth loopback port on the box and the
/// second of the bot's own.
pub const DEFAULT_URL: &str = "http://127.0.0.1:8194";

pub struct Client {
    pub url: String,
    pub timeout: Duration,
}

#[derive(Debug, Default)]
pub struct Answer {
    pub text: String,
    /// Which engine answered, as the agent reports it: `services2 (RTX 3060)`.
    pub engine: String,
    pub rounds: u64,
    pub tools: Vec<String>,
    /// The agent's own wall-clock report, seconds.
    pub secs: f64,
    pub warnings: Vec<String>,
}

impl Client {
    pub fn new(url: &str) -> Client {
        Client {
            url: url.trim_end_matches('/').to_string(),
            // Longer than the server's own 120 s turn budget on purpose: whoever is
            // first to give up should be the one that knows why.
            timeout: Duration::from_secs(150),
        }
    }

    pub fn ask(&self, question: &str, voice: bool) -> Result<Answer, String> {
        if question.trim().is_empty() {
            return Err("no question to ask".to_string());
        }
        let body = format!(
            "{{\"text\":{},\"voice\":{}}}",
            common_json::quoted(question),
            if voice { "true" } else { "false" }
        );
        // `request` rather than `post_json`, because `post_json` carries
        // `DEFAULT_TIMEOUT` (2 s) and a turn is 8–20 s: the whole point of the field
        // is that this client waits longer than the server does.
        let reply = common_http::request(
            "POST",
            &format!("{}/v1/ask", self.url),
            Some(&body),
            &[],
            self.timeout,
        )
        .map_err(|e| format!("slack-agent: {e}"))?;
        if reply.status == 429 {
            return Err("slack-agent is busy (its ask endpoint takes one turn at a time)".into());
        }
        let value = common_json::parse(&reply.body)
            .map_err(|e| format!("slack-agent replied non-JSON ({e}): {:.160}", reply.body))?;
        if !reply.ok() {
            let why = value
                .get("error")
                .and_then(common_json::Json::as_str)
                .unwrap_or("no error text");
            return Err(format!("slack-agent: HTTP {}: {}", reply.status, why));
        }
        let text = value
            .get("answer")
            .and_then(common_json::Json::as_str)
            .unwrap_or("")
            .to_string();
        if text.trim().is_empty() {
            return Err("slack-agent answered 200 with no answer text".to_string());
        }
        Ok(Answer {
            text,
            engine: value
                .get("engine")
                .and_then(common_json::Json::as_str)
                .unwrap_or("(unknown engine)")
                .to_string(),
            rounds: value
                .get("rounds")
                .and_then(common_json::Json::as_f64)
                .map(|n| n as u64)
                .unwrap_or(0),
            tools: value
                .get("tools")
                .and_then(common_json::Json::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(common_json::Json::as_str)
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default(),
            secs: value
                .get("secs")
                .and_then(common_json::Json::as_f64)
                .unwrap_or(0.0),
            warnings: value
                .get("warnings")
                .and_then(common_json::Json::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(common_json::Json::as_str)
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn answer_with(body: &'static str, status: &'static str) -> (String, std::sync::mpsc::Receiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut all: Vec<u8> = Vec::new();
                let mut buf = [0u8; 4096];
                while let Ok(n) = s.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    all.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&all).to_string();
                    let Some((_, rest)) = text.split_once("Content-Length: ") else { break };
                    let want: usize = rest.trim().split("\r\n").next().unwrap_or("0").trim().parse().unwrap_or(0);
                    let head = text.find("\r\n\r\n").unwrap_or(0) + 4;
                    if all.len() >= head + want {
                        break;
                    }
                }
                let _ = tx.send(String::from_utf8_lossy(&all).to_string());
                let out = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(out.as_bytes());
                let _ = s.flush();
            }
        });
        (format!("http://127.0.0.1:{port}"), rx)
    }

    const GOOD: &str = "{\"answer\":\"Yes — open since 18:40 UTC.\",\"engine\":\"services2 (RTX 3060)\",\
         \"rounds\":2,\"tools\":[\"space_status\"],\"secs\":7.74,\"warnings\":[]}";

    #[test]
    fn a_real_answer_arrives_with_the_receipts_the_log_needs() {
        let (url, rx) = answer_with(GOOD, "200 OK");
        let a = Client::new(&url).ask("is the space open?", true).unwrap();
        assert!(a.text.contains("open since 18:40"));
        assert_eq!(a.engine, "services2 (RTX 3060)");
        assert_eq!(a.rounds, 2);
        assert_eq!(a.tools, vec!["space_status".to_string()]);
        assert!((a.secs - 7.74).abs() < 1e-9);
        let sent = rx.recv().unwrap();
        assert!(sent.contains("POST /v1/ask"), "{sent}");
        assert!(sent.contains("\"voice\":true"), "{sent}");
        assert!(sent.contains("is the space open?"), "{sent}");
    }

    #[test]
    fn a_question_with_quotes_survives_the_json_we_build() {
        let (url, rx) = answer_with(GOOD, "200 OK");
        Client::new(&url)
            .ask("what's on tonight — and is it \"warm\"?", false)
            .unwrap();
        let sent = rx.recv().unwrap();
        assert!(sent.contains("what\\u0027s") || sent.contains("what's"), "{sent}");
        assert!(sent.contains("\\\"warm\\\""), "quote not escaped: {sent}");
        assert!(sent.contains("\"voice\":false"), "{sent}");
    }

    #[test]
    fn the_server_saying_no_is_not_rewritten_into_an_answer() {
        let (url, _rx) = answer_with("{\"error\":\"no MCP server answered, refusing to invent\"}", "502 Bad Gateway");
        let e = Client::new(&url).ask("temps", true).unwrap_err();
        assert!(e.contains("refusing to invent"), "{e}");
        assert!(e.contains("502"), "{e}");
    }

    #[test]
    fn a_busy_agent_says_so_in_words() {
        let (url, _rx) = answer_with("{\"error\":\"busy\"}", "429 Too Many Requests");
        assert!(Client::new(&url).ask("x", true).unwrap_err().contains("busy"));
    }

    #[test]
    fn an_empty_200_is_an_error_because_silence_would_be_spoken() {
        let (url, _rx) = answer_with("{\"answer\":\"  \"}", "200 OK");
        assert!(Client::new(&url).ask("x", true).unwrap_err().contains("no answer text"));
    }

    #[test]
    fn an_empty_question_never_reaches_the_socket() {
        assert!(Client::new(DEFAULT_URL).ask("   ", true).unwrap_err().contains("no question"));
    }
}
