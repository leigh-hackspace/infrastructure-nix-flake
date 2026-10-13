//! A minimal Telnet client — enough of RFC 854 to drive the ProCurve CLI.
//!
//! Two things here are load-bearing and both were learned against the switch
//! at 10.3.1.2 rather than from the RFC:
//!
//! * **Negotiation must at least be *skipped*.** The switch opens by sending
//!   IAC option requests; if they are not consumed as protocol they arrive in
//!   the text stream as 0xFF bytes and end up inside parsed table rows.  We
//!   answer every WILL with DONT and every DO with WONT — we want the plain
//!   NVT, and the CLI's own screen output is what we parse anyway.  Each
//!   (verb, option) pair is answered once so a server that repeats the request
//!   cannot start a reply loop.
//!
//! * **Lines end in LF, not CR LF.** The CLI counts a CR *and* an LF as a
//!   keypress, so `agent\r\n` submits `agent` and then submits an empty
//!   password line: the login fails with `User authentication failure.` and
//!   looks like a bad credential.  `send_line` therefore writes a bare LF.

use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use crate::R;

const IAC: u8 = 255;
const SE: u8 = 240;
const SB: u8 = 250;
const WILL: u8 = 251;
const WONT: u8 = 252;
const DO: u8 = 253;
const DONT: u8 = 254;

/// Where in a Telnet protocol sequence the last byte left us.  Kept on the
/// struct because a negotiation can straddle two `read()`s.
#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    Normal,
    /// Saw IAC; the next byte is a verb (or a second IAC, meaning a real 0xFF).
    Iac,
    /// Saw IAC + verb; the next byte is the option.
    Verb,
    /// Inside IAC SB ... IAC SE, discarding.
    Sub,
    /// Inside a subnegotiation, having just seen an IAC.
    SubIac,
}

/// A read that did not end on the prompt it was waiting for.
///
/// The bytes received before the failure are kept rather than dropped.  The CLI
/// paints the echoed command and the prompt for the *next* one on the same row
/// (`interface 23 name "x"switch1(config)#`), so a buffer that contains the
/// command is proof the console read it — which is exactly the difference
/// between "safe to send again" and "sending it again applies it twice".
/// A bare `String` error throws that evidence away, which is how a write that
/// had in fact been applied came to be reported as a command the switch had
/// never seen.
pub struct ReadFail {
    pub message: String,
    /// Everything received before the failure, protocol bytes already removed.
    pub text: String,
}

impl ReadFail {
    /// The failure on its own, for a caller that cannot use the partial text.
    pub fn into_message(self) -> String {
        self.message
    }
}

pub struct Telnet {
    stream: TcpStream,
    state: State,
    /// The verb of the negotiation currently being read; only meaningful in
    /// [`State::Verb`].
    verb: u8,
    answered: Vec<(u8, u8)>,
}

impl Telnet {
    pub fn connect(host: &str, port: u16, timeout: Duration) -> R<Telnet> {
        let stream = TcpStream::connect((host, port))
            .map_err(|e| format!("cannot reach {host}:{port}: {e}"))?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| format!("set_read_timeout: {e}"))?;
        stream
            .set_nodelay(true)
            .map_err(|e| format!("set_nodelay: {e}"))?;
        Ok(Telnet {
            stream,
            state: State::Normal,
            verb: 0,
            answered: Vec::new(),
        })
    }

    /// Send `s` followed by a bare LF — see the module comment for why not CR LF.
    pub fn send_line(&mut self, s: &str) -> R<()> {
        let mut buf = s.as_bytes().to_vec();
        buf.push(b'\n');
        self.stream
            .write_all(&buf)
            .map_err(|e| format!("send failed: {e}"))
    }

    /// Send raw bytes with no line ending — the paging key is a lone space.
    pub fn send_raw(&mut self, s: &str) -> R<()> {
        self.stream
            .write_all(s.as_bytes())
            .map_err(|e| format!("send failed: {e}"))
    }

    /// Read until `done` is satisfied by everything received so far.
    ///
    /// Returns the accumulated *text* (protocol bytes removed, lossy UTF-8).
    /// A timeout is a failure rather than a short return: every caller is
    /// waiting for a specific prompt, and handing back a partial response
    /// would let the next command read the previous one's tail.  The failure
    /// carries the partial text with it — see [`ReadFail`].
    pub fn read_until<F: Fn(&str) -> bool>(
        &mut self,
        done: F,
        timeout: Duration,
    ) -> Result<String, ReadFail> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| ReadFail {
                message: "timeout out of range".to_string(),
                text: String::new(),
            })?;
        let mut out: Vec<u8> = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let text = String::from_utf8_lossy(&out).into_owned();
            if done(&text) {
                return Ok(text);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(ReadFail {
                    message: format!(
                        "timed out after {}s waiting for a prompt; last {} bytes: {:?}",
                        timeout.as_secs(),
                        out.len(),
                        tail_for_error(&text)
                    ),
                    text,
                });
            }
            self.stream
                .set_read_timeout(Some(deadline - now))
                .map_err(|e| ReadFail {
                    message: format!("set_read_timeout: {e}"),
                    text: text.clone(),
                })?;
            match self.stream.read(&mut buf) {
                Ok(0) => {
                    return Err(ReadFail {
                        message: "the switch closed the connection".to_string(),
                        text,
                    })
                }
                Ok(n) => self.feed(&buf[..n], &mut out).map_err(|e| ReadFail {
                    message: e,
                    text: text.clone(),
                })?,
                // read_until owns the deadline, so a per-read timeout just
                // means "loop again and re-check it".
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(e) => {
                    return Err(ReadFail {
                        message: format!("read failed: {e}"),
                        text,
                    })
                }
            }
        }
    }

    /// Split `bytes` into protocol and data, answering negotiations as needed.
    fn feed(&mut self, bytes: &[u8], out: &mut Vec<u8>) -> R<()> {
        for &b in bytes {
            match self.state {
                State::Normal => {
                    if b == IAC {
                        self.state = State::Iac;
                    } else {
                        out.push(b);
                    }
                }
                State::Iac => match b {
                    // IAC IAC is an escaped 0xFF of data, not a command.
                    IAC => {
                        out.push(IAC);
                        self.state = State::Normal;
                    }
                    WILL | WONT | DO | DONT => {
                        self.verb = b;
                        self.state = State::Verb;
                    }
                    SB => self.state = State::Sub,
                    // SE, IP, AYT, EC, EL ... nothing here needs a body.
                    _ => self.state = State::Normal,
                },
                State::Verb => {
                    self.answer(b)?;
                    self.state = State::Normal;
                }
                State::Sub => {
                    if b == IAC {
                        self.state = State::SubIac;
                    }
                }
                State::SubIac => {
                    self.state = if b == SE {
                        State::Normal
                    } else {
                        // IAC IAC inside a subnegotiation: still inside it.
                        State::Sub
                    };
                }
            }
        }
        Ok(())
    }

    /// Refuse `option`, once.  WONT/DONT from the server need no answer.
    fn answer(&mut self, option: u8) -> R<()> {
        let refuse = match self.verb {
            WILL => DONT,
            DO => WONT,
            _ => return Ok(()),
        };
        let key = (self.verb, option);
        if self.answered.contains(&key) {
            return Ok(());
        }
        self.answered.push(key);
        self.stream
            .write_all(&[IAC, refuse, option])
            .map_err(|e| format!("negotiation reply failed: {e}"))
    }
}

/// A short, printable excerpt for timeout messages.
fn tail_for_error(s: &str) -> String {
    let tail: String = s.chars().rev().take(120).collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    tail.replace('\u{1b}', "^ESC")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Telnet {
        // No socket is needed to exercise the state machine: `feed` and
        // `answer` only touch `stream` when a negotiation has to be answered,
        // and these tests use WONT/DONT, which are answered with nothing.
        use std::net::TcpListener;
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let s = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let _accepted = l.accept().unwrap();
        Telnet {
            stream: s,
            state: State::Normal,
            verb: 0,
            answered: Vec::new(),
        }
    }

    #[test]
    fn plain_text_passes_through() {
        let mut t = client();
        let mut out = Vec::new();
        t.feed(b"switch1>", &mut out).unwrap();
        assert_eq!(out, b"switch1>");
    }

    #[test]
    fn iac_iac_is_a_data_byte() {
        let mut t = client();
        let mut out = Vec::new();
        t.feed(&[IAC, IAC, b'x'], &mut out).unwrap();
        assert_eq!(out, vec![IAC, b'x']);
        assert_eq!(t.state, State::Normal);
    }

    #[test]
    fn negotiation_never_reaches_the_text() {
        let mut t = client();
        let mut out = Vec::new();
        // WILL ECHO, then WONT ECHO (no reply needed), then text.
        t.feed(&[IAC, WONT, 1, b'h', b'i'], &mut out).unwrap();
        assert_eq!(out, b"hi");
        assert_eq!(t.state, State::Normal);
    }

    #[test]
    fn a_negotiation_split_across_reads_still_works() {
        let mut t = client();
        let mut out = Vec::new();
        t.feed(&[IAC], &mut out).unwrap();
        t.feed(&[WONT], &mut out).unwrap();
        t.feed(&[1, b'o'], &mut out).unwrap();
        assert_eq!(out, b"o");
        assert_eq!(t.state, State::Normal);
    }

    #[test]
    fn subnegotiation_is_discarded() {
        let mut t = client();
        let mut out = Vec::new();
        t.feed(&[IAC, SB, 24, 0, 1, IAC, SE, b'z'], &mut out)
            .unwrap();
        assert_eq!(out, b"z");
        assert_eq!(t.state, State::Normal);
    }
}
