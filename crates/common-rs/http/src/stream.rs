//! Reading a response **as it arrives**, one line or one chunk at a time.
//!
//! ## Why this exists next to [`crate::request`]
//!
//! `request` and `request_bytes` are the right shape for a call whose whole answer is one
//! object: they buffer the body, hand back a `String`, and the caller parses it. A **streaming**
//! answer is not one object — an inference engine sending server-sent events, or a voice
//! gateway forwarding a turn — and the entire value of it is in seeing part 1 before part 5
//! exists. Buffering the body first, which is what `request_bytes` does even when it
//! understands the chunked framing, is exactly the thing that turns a streaming server into a
//! slow non-streaming one: the client then reports "the engine is slow" when the engine was
//! streaming happily and the client was hoarding.
//!
//! So: same connection rules, same URL parser, same three body framings as the buffered client
//! — but the framing is stripped by a [`Read`] adapter and the caller reads incrementally.
//!
//! ## Timeout means something different here, and it is the idle gap
//!
//! For a buffered request, `timeout` is "how long the answer may take". For a stream that
//! question has no answer: an answer that takes 30 seconds is fine *because it took 30 seconds
//! of arriving*. The deadline that means something is **how long silence is tolerated**, so
//! that is what the socket read timeout is set to here, and the error says so. A caller that
//! also needs a total budget enforces its own clock — `crates/voice-assistant/src/agent.rs`
//! does, and names which of the two it hit.
//!
//! ## Lines, not characters
//!
//! [`Stream::next_line`] exists because the two consumers of this are line-oriented formats
//! (SSE's `data: …`, JSON-lines). It returns `String`s and is therefore lossy for arbitrary
//! binary; a caller that wants bytes has [`Stream::read_chunk`], which hands back what the
//! framing produced without any text assumption.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::{parse_url, MAX_RESPONSE_BYTES};

/// A response whose body is still arriving.
///
/// The `BufReader` is over a `Box<dyn Read>` because the body's framing is decided after the
/// head is read: `Transfer-Encoding: chunked` gets [`Chunked`], `Content-Length` gets
/// `Take(len)`, and neither (which `Connection: close` makes well-defined) gets the socket.
pub struct Stream {
    /// The status line, already read.
    pub status: u16,
    pub headers: HashMap<String, String>,
    url: String,
    idle: Duration,
    reader: BufReader<Box<dyn Read + Send>>,
}

/// Hand-written: the body is a live socket, and `{:?}` on that would either print nothing
/// useful or block.  A caller that logs a stream wants who and what status, which is this.
impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stream")
            .field("url", &self.url)
            .field("status", &self.status)
            .field("idle", &self.idle)
            .finish()
    }
}

impl Stream {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(|s| s.as_str())
    }

    /// The next line of the body, without its terminator.  `Ok(None)` is a clean end of body.
    ///
    /// A read that times out is an `Err` naming the idle window rather than `None`: "the
    /// server said it was finished" and "the server has not said anything for 45 s" are
    /// different faults and the caller must not have to guess which one it is holding.
    pub fn next_line(&mut self) -> Result<Option<String>, String> {
        let mut line = String::new();
        loop {
            match self.reader.read_line(&mut line) {
                Ok(0) => return Ok(None),
                Ok(_) => {
                    while line.ends_with('\n') || line.ends_with('\r') {
                        line.pop();
                    }
                    return Ok(Some(line));
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    return Err(format!(
                        "{}: nothing for {:.0?} (idle ceiling)",
                        self.url, self.idle
                    ))
                }
                Err(e) => return Err(format!("{}: read body line: {e}", self.url)),
            }
        }
    }

    /// Read one bufferful of body bytes, framing stripped, for a caller that wants the parts
    /// rather than the lines.  `Ok(None)` is a clean end of body.
    pub fn read_chunk(&mut self, buf: &mut [u8]) -> Result<Option<usize>, String> {
        loop {
            match self.reader.read(buf) {
                Ok(0) => return Ok(None),
                Ok(n) => return Ok(Some(n)),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    return Err(format!(
                        "{}: nothing for {:.0?} (idle ceiling)",
                        self.url, self.idle
                    ))
                }
                Err(e) => return Err(format!("{}: read body: {e}", self.url)),
            }
        }
    }

    /// The rest of the body, as text, capped: for the one case where a caller needs a
    /// non-2xx's explanation and must not read an unbounded stream to get it.
    pub fn read_rest(&mut self, cap: usize) -> String {
        let mut out = Vec::new();
        let mut tmp = [0u8; 4096];
        while out.len() < cap {
            match self.reader.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&tmp[..n]),
                Err(_) => break,
            }
        }
        out.truncate(cap);
        String::from_utf8_lossy(&out).into_owned()
    }
}

/// One event of a server-sent event stream: a name (`message` when the sender named none)
/// and the joined `data:` payload.
#[derive(Debug, Clone, PartialEq)]
pub struct SseEvent {
    pub name: String,
    pub data: String,
}

impl Stream {
    /// Read the next **whole event** of an SSE stream.
    ///
    /// One implementation, because there are two callers of this in the voice path (the gateway
    /// reading the agent, and the satellite reading the gateway) and an event that one of them
    /// parses differently is a bug that shows up as a missing word, far from either.  Rules it
    /// implements: `data:` lines join with `\n`, a blank line ends an event, `event:` names it,
    /// `:` lines are comments (keep-alives) and are skipped, and `data: [DONE]` is a clean end.
    pub fn read_event(&mut self) -> Result<Option<SseEvent>, String> {
        let mut name = String::new();
        let mut data: Vec<String> = Vec::new();
        let mut saw_any = false;
        loop {
            let Some(line) = self.next_line()? else {
                return Ok(if saw_any {
                    Some(SseEvent {
                        name: finish_name(name),
                        data: data.join("\n"),
                    })
                } else {
                    None
                });
            };
            if line.is_empty() {
                if saw_any {
                    return Ok(Some(SseEvent {
                        name: finish_name(name),
                        data: data.join("\n"),
                    }));
                }
                continue;
            }
            let Some((field, value)) = line.split_once(':') else {
                saw_any = true; // a bare line is not valid SSE, but it is not fatal either
                continue;
            };
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => {
                    name = value.to_string();
                    saw_any = true;
                }
                "data" => {
                    if value == "[DONE]" {
                        return Ok(None);
                    }
                    data.push(value.to_string());
                    saw_any = true;
                }
                // `id:`, `retry:` and anything starting with `:` (a comment) are accepted and
                // ignored: a keep-alive must not become an event, and an unknown field must not
                // end a turn that is going fine.
                _ => {}
            }
        }
    }
}

fn finish_name(name: String) -> String {
    if name.is_empty() {
        "message".to_string()
    } else {
        name
    }
}

/// `Transfer-Encoding: chunked` seen as a `Read`: chunk-size lines and their trailing CRLF are
/// invisible, and the zero-length chunk is EOF.
struct Chunked {
    inner: BufReader<Box<dyn Read + Send>>,
    /// Bytes left in the current chunk; when it hits 0 the next size line is read.
    left: u64,
    done: bool,
}

impl Read for Chunked {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            let mut size = String::new();
            self.inner.read_line(&mut size)?;
            if size.is_empty() {
                // Head cut off mid-framing: EOF, but a truncated one, which the caller
                // cannot distinguish from a clean finish. Treat a totally empty read as
                // clean and let anything with a partial size line fail below.
                return Ok(0);
            }
            let hex = size.trim().split(';').next().unwrap_or("").trim();
            self.left = u64::from_str_radix(hex, 16).map_err(|_| {
                std::io::Error::new(
                    ErrorKind::InvalidData,
                    format!("bad chunk size {size:?}"),
                )
            })?;
            if self.left == 0 {
                // Terminated: consume the optional trailer lines up to the blank line, then EOF.
                loop {
                    let mut tail = String::new();
                    match self.inner.read_line(&mut tail) {
                        Ok(0) | Ok(_) if tail.trim().is_empty() => break,
                        Ok(_) => continue,
                        Err(e) => return Err(e),
                    }
                }
                self.done = true;
                return Ok(0);
            }
            if self.left > MAX_RESPONSE_BYTES as u64 {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidData,
                    format!(
                        "chunk of {} bytes over this client's cap of {MAX_RESPONSE_BYTES}",
                        self.left
                    ),
                ));
            }
        }
        let want = (self.left as usize).min(buf.len());
        let n = self.inner.read(&mut buf[..want])?;
        self.left -= n as u64;
        if self.left == 0 && n > 0 {
            // The CRLF that follows every chunk's data.
            let mut crlf = [0u8; 2];
            self.inner.read_exact(&mut crlf)?;
        }
        Ok(n)
    }
}

/// One request, one still-open response.  See the module header for what `idle` means.
pub fn request_stream(
    method: &str,
    url: &str,
    body: Option<&[u8]>,
    content_type: &str,
    headers: &[(&str, &str)],
    idle: Duration,
) -> Result<Stream, String> {
    let t = parse_url(url)?;
    let addrs: Vec<std::net::SocketAddr> = (t.host.as_str(), t.port)
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {}: {}", t.host, e))?
        .collect();
    let addr = addrs
        .first()
        .copied()
        .ok_or_else(|| format!("no address for {}:{}", t.host, t.port))?;
    let mut stream = TcpStream::connect_timeout(&addr, idle)
        .map_err(|e| format!("connect {}:{}: {}", t.host, t.port, e))?;
    // Nagle off in both directions of this conversation: the request body is often one small
    // write and the first response event is often one small read.
    stream.set_nodelay(true).ok();
    stream
        .set_read_timeout(Some(idle))
        .and_then(|()| stream.set_write_timeout(Some(idle)))
        .map_err(|e| format!("set timeouts: {e}"))?;

    let mut head = format!("{method} {} HTTP/1.1\r\n", t.path);
    head.push_str(&format!("Host: {}:{}\r\n", t.host, t.port));
    head.push_str("Connection: close\r\n");
    head.push_str("User-Agent: common-http-stream (infrastructure-nix-flake)\r\n");
    if let Some(b) = body {
        head.push_str(&format!("Content-Type: {content_type}\r\n"));
        head.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    for (n, v) in headers {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream
        .write_all(head.as_bytes())
        .and_then(|()| body.map(|b| stream.write_all(b)).unwrap_or(Ok(())))
        .map_err(|e| format!("send {method} {url}: {e}"))?;

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|e| format!("read status line from {url}: {e}"))?;
    if status_line.is_empty() {
        return Err(format!("{url}: closed without a response"));
    }
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("{url}: unreadable status line {:?}", status_line.trim()))?;
    let mut headers: HashMap<String, String> = HashMap::new();
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("read headers from {url}: {e}"))?;
        if line == "\r\n" || line == "\n" || line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let boxed: Box<dyn Read + Send> = Box::new(reader);
    let body_reader: Box<dyn Read + Send> =
        if headers
            .get("transfer-encoding")
            .map(|v| v.to_ascii_lowercase().contains("chunked"))
            .unwrap_or(false)
        {
            Box::new(Chunked {
                inner: BufReader::new(boxed),
                left: 0,
                done: false,
            })
        } else if let Some(len) = headers
            .get("content-length")
            .and_then(|v| v.trim().parse::<usize>().ok())
        {
            if len > MAX_RESPONSE_BYTES {
                return Err(format!(
                    "{url}: body is {len} bytes, over this client's cap of {MAX_RESPONSE_BYTES}"
                ));
            }
            Box::new(boxed.take(len as u64))
        } else {
            Box::new(boxed.take(MAX_RESPONSE_BYTES as u64))
        };

    Ok(Stream {
        status,
        headers,
        url: url.to_string(),
        idle,
        reader: BufReader::new(body_reader),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;

    /// A server that writes `raw` and hangs up.
    fn once(raw: &'static [u8]) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut tmp = [0u8; 1024];
                let _ = s.read(&mut tmp);
                let _ = s.write_all(raw);
                let _ = s.flush();
            }
        });
        format!("http://127.0.0.1:{port}/sse")
    }

    fn drain(mut s: Stream) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(Some(l)) = s.next_line() {
            out.push(l);
        }
        out
    }

    #[test]
    fn a_chunked_sse_body_arrives_as_lines_with_the_framing_invisible() {
        let url = once(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n\
              22\r\ndata: {\"a\":1}\r\n\r\ndata: {\"a\":2}\r\n\r\n\r\n0\r\n\r\n",
        );
        let s = request_stream("POST", &url, Some(b"{}"), "application/json", &[], Duration::from_secs(5))
            .unwrap();
        assert_eq!(s.status, 200);
        assert_eq!(s.header("content-type"), Some("text/event-stream"));
        assert_eq!(
            drain(s),
            vec!["data: {\"a\":1}", "", "data: {\"a\":2}", ""]
        );
    }

    /// The property the buffered client cannot have: line 1 is readable while the server is
    /// still holding line 2.
    #[test]
    fn the_first_line_is_readable_while_the_server_holds_the_second() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let (release, may_release) = mpsc::channel::<()>();
        thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut tmp = [0u8; 512];
            let _ = s.read(&mut tmp);
            let _ = s.write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\nfirst\n\r\n",
            );
            let _ = s.flush();
            let _ = may_release.recv();
            let _ = s.write_all(b"6\r\nsecond\r\n0\r\n\r\n");
            let _ = s.flush();
        });
        let url = format!("http://127.0.0.1:{port}/sse");
        let started = Instant::now();
        let mut s = request_stream("GET", &url, None, "text/plain", &[], Duration::from_secs(10))
            .unwrap();
        assert_eq!(s.next_line().unwrap().as_deref(), Some("first"));
        assert!(started.elapsed() < Duration::from_secs(5), "blocked for a line");
        release.send(()).unwrap();
        assert_eq!(s.next_line().unwrap().as_deref(), Some("second"));
        assert_eq!(s.next_line().unwrap(), None, "clean end of body");
    }

    #[test]
    fn a_chunk_boundary_may_fall_inside_a_line() {
        // `data: hello` split across two chunks: a reader that assumed one chunk per line
        // would return "data: he" and then "llo".
        let url = once(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n8\r\ndata: he\r\n5\r\nllo!!\r\n0\r\n\r\n",
        );
        let s = request_stream("GET", &url, None, "text/plain", &[], Duration::from_secs(5)).unwrap();
        assert_eq!(drain(s), vec!["data: hello!!"]);
    }

    /// The other half of the API: a caller streaming **audio** wants the framing gone and the
    /// bytes untouched, not a `String`.  A WAV body is not UTF-8, and this is the path that
    /// must not run it through one (`common-http`'s `request_bytes` exists for the same
    /// reason on the sending side).
    #[test]
    fn raw_bytes_come_back_byte_for_byte_with_the_framing_stripped() {
        let url = once(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n\xff\xfe\x00\x7f\r\n4\r\n\x01\x02\x03\x04\r\n0\r\n\r\n",
        );
        let mut s =
            request_stream("GET", &url, None, "audio/L16", &[], Duration::from_secs(5)).unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 3];
        while let Some(n) = s.read_chunk(&mut buf).unwrap() {
            got.extend_from_slice(&buf[..n]);
        }
        assert_eq!(got, vec![0xff, 0xfe, 0x00, 0x7f, 0x01, 0x02, 0x03, 0x04]);
    }

    /// A server that frames each `part` as exactly one HTTP chunk, so a fixture says what it
    /// means instead of hand-counting hex lengths (which is how a test comes to be asserting a
    /// framing bug of its own).
    fn sse(parts: &'static [&'static str]) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut tmp = [0u8; 1024];
                let _ = s.read(&mut tmp);
                let mut out =
                    String::from("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
                for p in parts {
                    out.push_str(&format!("{:x}\r\n{p}\r\n", p.len()));
                }
                out.push_str("0\r\n\r\n");
                let _ = s.write_all(out.as_bytes());
                let _ = s.flush();
            }
        });
        format!("http://127.0.0.1:{port}/sse")
    }

    #[test]
    fn sse_events_are_reassembled_across_data_lines_comments_and_reads() {
        let url = sse(&[
            "event: answer\ndata: {\"a\":1,\ndata: \"more\"}\n\n: keep-alive\n\n\n",
            "event: done\ndata: last\n\n",
        ]);
        let mut s = request_stream("GET", &url, None, "x", &[], Duration::from_secs(5)).unwrap();
        let a = s.read_event().unwrap().unwrap();
        assert_eq!(a.name, "answer");
        assert_eq!(a.data, "{\"a\":1,\n\"more\"}", "data lines join with a newline");
        let b = s.read_event().unwrap().unwrap();
        assert_eq!((b.name.as_str(), b.data.as_str()), ("done", "last"));
        assert!(s.read_event().unwrap().is_none(), "clean end");
    }

    #[test]
    fn an_event_split_across_two_reads_is_still_one_event() {
        // The shape a real network makes: the header line in one chunk, its data in the next.
        let url = sse(&["event: audio\n", "data: \"AAA\"\n\n"]);
        let mut s = request_stream("GET", &url, None, "x", &[], Duration::from_secs(5)).unwrap();
        assert_eq!(
            s.read_event().unwrap().unwrap(),
            SseEvent {
                name: "audio".into(),
                data: "\"AAA\"".into()
            }
        );
    }

    #[test]
    fn an_unnamed_event_is_called_message_and_done_ends_the_stream() {
        let url = sse(&["data: x\n\n", "data: [DONE]\n\n"]);
        let mut s = request_stream("GET", &url, None, "x", &[], Duration::from_secs(5)).unwrap();
        assert_eq!(
            s.read_event().unwrap().unwrap(),
            SseEvent {
                name: "message".into(),
                data: "x".into()
            }
        );
        assert!(s.read_event().unwrap().is_none());
    }

    #[test]
    fn a_content_length_body_ends_cleanly_and_a_plain_one_reads_to_eof() {
        let cl = once(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nab\r\ncd");
        assert_eq!(drain(request_stream("GET", &cl, None, "x", &[], Duration::from_secs(5)).unwrap()), vec!["ab", "cd"]);
        let eof = once(b"HTTP/1.1 200 OK\r\n\r\none\ntwo");
        assert_eq!(drain(request_stream("GET", &eof, None, "x", &[], Duration::from_secs(5)).unwrap()), vec!["one", "two"]);
    }

    #[test]
    fn silence_is_reported_as_silence_and_not_as_an_end() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut tmp = [0u8; 512];
            let _ = s.read(&mut tmp);
            // `hi\n` as a complete chunk: a line-oriented reader cannot hand back a line that
            // has no newline yet, and that is a property worth having seen (SSE always ends an
            // event with a blank line, so the events this client is for always terminate).
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nhi\n\r\n");
            let _ = s.flush();
            // Hold the connection open, say nothing, and let the client's idle ceiling answer.
            std::thread::sleep(Duration::from_millis(400));
        });
        let url = format!("http://127.0.0.1:{port}/sse");
        let mut s = request_stream("GET", &url, None, "x", &[], Duration::from_millis(120)).unwrap();
        assert_eq!(s.next_line().unwrap().as_deref(), Some("hi"));
        let e = s.next_line().unwrap_err();
        assert!(e.contains("idle ceiling"), "{e}");
    }

    #[test]
    fn a_non_2xx_can_have_its_explanation_read() {
        let url = once(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 16\r\n\r\n{\"error\":\"busy\"}");
        let mut s = request_stream("POST", &url, Some(b"{}"), "application/json", &[], Duration::from_secs(5))
            .unwrap();
        assert!(!s.ok());
        assert_eq!(s.status, 429);
        assert_eq!(s.read_rest(200), "{\"error\":\"busy\"}");
    }

    #[test]
    fn https_is_refused_here_too() {
        let e = request_stream("GET", "https://x/v1", None, "x", &[], Duration::from_secs(1)).unwrap_err();
        assert!(e.contains("refuses https"), "{e}");
    }
}
