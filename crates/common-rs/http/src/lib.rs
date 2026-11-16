//! `common-http` — the plain-HTTP/1.1 client shared by the programs in this repo
//! that read the LAN (today `hackspace-mcp`).
//!
//! ## Why this exists, and why it is not `reqwest`
//!
//! Every source an agent here is pointed at answers **plain HTTP on the LAN**: the
//! space API (`http://10.3.1.30:8081`), Home Assistant (`:8123`), Frigate and
//! Prometheus on the box itself (`127.0.0.1:5000`, `:9090`), the router's lease API
//! (`10.3.1.1:8091`). Those were probed, not assumed — `just slack-agent-tools`
//! re-measures them. TLS on that path buys nothing (the front door's TLS name and
//! the LAN origin are the same service), and it would drag a crypto stack into a
//! tool process whose entire job is to make HTTP calls and shrink the answers.
//!
//! So **`https://` is refused rather than silently downgraded** (see [`request`]).
//! A caller that genuinely needs TLS — `slack-agent` talking to the inference
//! engines through the front door, which is where the key gets injected — uses
//! `reqwest` and its rustls, which the workspace already vendors. The rule is:
//! this crate is for origins that are already HTTP. That is the same split
//! `moonraker-exporter` made when it chose `std::net::TcpStream` over a client
//! library; this is that code, in one place, so the second caller does not
//! fork it (AGENTS.md rule 2).
//!
//! ## What it does do
//!
//! One request per connection, `Connection: close`, timeouts on connect, read and
//! write, and the three body framings a LAN server actually uses: `Content-Length`,
//! `Transfer-Encoding: chunked`, and neither (read to EOF, which `Connection: close`
//! makes well-defined). Responses are capped at [`MAX_RESPONSE_BYTES`] — a tool that
//! fetches 200 Home Assistant entities is fine, a tool that fetches an unbounded
//! log is not, and the cap turns "typo in the URL" into an error rather than an
//! out-of-memory in a unit that other things depend on.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Largest response body accepted.  Home Assistant's whole state machine is a few
/// hundred entities (~150 KiB) and a Prometheus range query with a small step is
/// bigger than that but not by orders of magnitude, so this is headroom for the
/// real answers and a wall against the wrong URL.
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// The default per-request timeout.  Every one of these sources answers in
/// milliseconds on the LAN; a second is already the answer "something is wrong",
/// and a tool call that takes longer than this has no business holding up an
/// agent's turn.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Error text for a non-2xx, with the first line of the body: these servers
    /// say *why* (`401: Unauthorized`, dnsmasq-less 404s, Prometheus' JSON
    /// `{"status":"error","error":…}`), and that is the difference between a tool
    /// reporting "the broker is down" and reporting "no token".
    pub fn describe(&self) -> String {
        let first = self
            .body
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .chars()
            .take(200)
            .collect::<String>();
        if first.is_empty() {
            format!("HTTP {}", self.status)
        } else {
            format!("HTTP {}: {}", self.status, first)
        }
    }
}

/// An `http://host[:port]` origin plus a path.
struct Target {
    host: String,
    port: u16,
    path: String,
}

fn parse_url(url: &str) -> Result<Target, String> {
    let rest = match url.strip_prefix("http://") {
        Some(r) => r,
        None => {
            return Err(format!(
                "not a plain-HTTP origin: {url} (this client refuses https:// — see its header: \
                 use reqwest for TLS)"
            ))
        }
    };
    // `host:8081/path`, `host/path`, `host`, `v4:8081/path`.  Splitting on the
    // first `/` separates authority from path; a bare authority gets "/".
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return Err(format!("no host in {url}"));
    }
    // Only a trailing `:port` is a port: a bare IPv6 host would need brackets, and
    // nothing here is reached over IPv6 (the LAN origin table is IPv4 throughout).
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            let port: u16 = p
                .parse()
                .map_err(|_| format!("port out of range in {url}"))?;
            (h, port)
        }
        _ => (authority, 80),
    };
    Ok(Target {
        host: host.to_string(),
        port,
        path: path.to_string(),
    })
}

/// One HTTP exchange.  `headers` are `(name, value)` pairs added verbatim after
/// the host, content-type and auth headers this function may set itself.
pub fn request(
    method: &str,
    url: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
    timeout: Duration,
) -> Result<Response, String> {
    let t = parse_url(url)?;
    let addrs: Vec<std::net::SocketAddr> = (t.host.as_str(), t.port)
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {}: {}", t.host, e))?
        .collect();
    let addr = addrs
        .first()
        .copied()
        .ok_or_else(|| format!("no address for {}:{}", t.host, t.port))?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| format!("connect {}:{}: {}", t.host, t.port, e))?;
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .map_err(|e| format!("set timeout: {e}"))?;

    let mut head = format!("{method} {} HTTP/1.1\r\n", t.path);
    head.push_str(&format!("Host: {}:{}\r\n", t.host, t.port));
    head.push_str("Connection: close\r\n");
    head.push_str("Accept: */*\r\n");
    head.push_str("User-Agent: hackspace-mcp (infrastructure-nix-flake)\r\n");
    if let Some(b) = body {
        head.push_str("Content-Type: application/json\r\n");
        head.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    for (n, v) in headers {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    head.push_str("\r\n");

    stream
        .write_all(head.as_bytes())
        .and_then(|()| body.map(|b| stream.write_all(b.as_bytes())).unwrap_or(Ok(())))
        .map_err(|e| format!("send {method} {url}: {e}"))?;

    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|e| format!("clone socket: {e}"))?,
    );
    read_exchange(&mut reader, url)
}

fn read_exchange<R: Read>(reader: &mut BufReader<R>, url: &str) -> Result<Response, String> {
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

    let body = if headers
        .get("transfer-encoding")
        .map(|v| v.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false)
    {
        read_chunked(reader, url)?
    } else if let Some(len) = headers.get("content-length").and_then(|v| v.trim().parse::<usize>().ok()) {
        if len > MAX_RESPONSE_BYTES {
            return Err(format!(
                "{url}: body is {len} bytes, over this client's cap of {MAX_RESPONSE_BYTES}"
            ));
        }
        let mut buf = vec![0u8; len];
        reader
            .read_exact(&mut buf)
            .map_err(|e| format!("read body from {url}: {e}"))?;
        buf
    } else {
        // `Connection: close` makes EOF the end of the body.
        let mut buf = Vec::new();
        reader
            .by_ref()
            .take(MAX_RESPONSE_BYTES as u64 + 1)
            .read_to_end(&mut buf)
            .map_err(|e| format!("read body from {url}: {e}"))?;
        if buf.len() > MAX_RESPONSE_BYTES {
            return Err(format!(
                "{url}: body over this client's cap of {MAX_RESPONSE_BYTES} bytes"
            ));
        }
        buf
    };

    Ok(Response {
        status,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn read_chunked<R: Read>(reader: &mut BufReader<R>, url: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let mut size_line = String::new();
        reader
            .read_line(&mut size_line)
            .map_err(|e| format!("read chunk size from {url}: {e}"))?;
        let size = u64::from_str_radix(size_line.trim().split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| format!("{url}: bad chunk size {:?}", size_line.trim()))?;
        if size == 0 {
            break;
        }
        if out.len() as u64 + size > MAX_RESPONSE_BYTES as u64 {
            return Err(format!(
                "{url}: chunked body over this client's cap of {MAX_RESPONSE_BYTES} bytes"
            ));
        }
        let mut chunk = vec![0u8; size as usize];
        reader
            .read_exact(&mut chunk)
            .map_err(|e| format!("read chunk from {url}: {e}"))?;
        out.append(&mut chunk);
        // The CRLF after each chunk.
        let mut crlf = [0u8; 2];
        reader
            .read_exact(&mut crlf)
            .map_err(|e| format!("read chunk terminator from {url}: {e}"))?;
    }
    Ok(out)
}

/// `GET` with the default timeout.
pub fn get(url: &str, headers: &[(&str, &str)]) -> Result<Response, String> {
    request("GET", url, None, headers, DEFAULT_TIMEOUT)
}

/// `POST` of a JSON body with the default timeout.
pub fn post_json(
    url: &str,
    body: &str,
    headers: &[(&str, &str)],
) -> Result<Response, String> {
    request("POST", url, Some(body), headers, DEFAULT_TIMEOUT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    /// A one-shot server that answers with `raw`, so the framing under test is
    /// whatever the fixture says it is.
    fn once(raw: &'static [u8]) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf); // the request; not inspected here
                let _ = s.write_all(raw);
                let _ = s.flush();
            }
        });
        format!("http://127.0.0.1:{port}/probe")
    }

    #[test]
    fn reads_a_content_length_body() {
        let url = once(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\n{\"a\":1}");
        let r = get(&url, &[]).unwrap();
        assert!(r.ok());
        assert_eq!(r.body, "{\"a\":1}");
    }

    #[test]
    fn reads_a_chunked_body() {
        let url = once(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n");
        let r = get(&url, &[]).unwrap();
        assert_eq!(r.body, "hello world");
    }

    #[test]
    fn reads_to_eof_when_nothing_is_announced() {
        let url = once(b"HTTP/1.1 200 OK\r\n\r\ntail");
        let r = get(&url, &[]).unwrap();
        assert_eq!(r.body, "tail");
    }

    #[test]
    fn a_non_2xx_describes_itself_with_the_servers_reason() {
        let url = once(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 12\r\n\r\nUnauthorized");
        let r = get(&url, &[]).unwrap();
        assert!(!r.ok());
        assert_eq!(r.status, 401);
        assert_eq!(r.describe(), "HTTP 401: Unauthorized");
    }

    #[test]
    fn https_is_refused_not_downgraded() {
        let e = get("https://api.int.leighhack.org/space.json", &[]).unwrap_err();
        assert!(e.contains("refuses https"), "{e}");
    }

    #[test]
    fn a_port_and_a_path_survive_the_parse() {
        let t = parse_url("http://10.3.1.30:8081/space.json").unwrap();
        assert_eq!((t.host.as_str(), t.port, t.path.as_str()), ("10.3.1.30", 8081, "/space.json"));
        let t = parse_url("http://127.0.0.1:9090").unwrap();
        assert_eq!((t.host.as_str(), t.port, t.path.as_str()), ("127.0.0.1", 9090, "/"));
        // A query string is part of the path, not a second authority.
        let t = parse_url("http://h/api/v1/query?query=up").unwrap();
        assert_eq!(t.path, "/api/v1/query?query=up");
    }
}
