//! Hand-rolled HTTP/1.1 plumbing shared by the in-tree servers in this repo
//! (`status-dashboard`, `network-status`, `switch-gui`).
//!
//! Both dashboards used to carry their own copy of: reading a request head off a
//! `TcpStream`, splitting method/path/headers, and writing a response.  The two
//! copies had drifted; this is the merged behaviour, and the differences are
//! called out below so nobody re-introduces them by copy-paste:
//!
//! * **Status line.** The copies had different `match status` tables, so an
//!   unlisted code (say 403 on network-status) was sent as `HTTP/1.1 403 OK`.
//!   `reason_phrase` now covers every code the servers emit, and falls back to
//!   "OK" for anything unknown (still better than an empty reason).
//! * **Query strings.** Only network-status stripped `?...` from the path.
//!   `Request::path()` strips it for both, so a status-dashboard URL with a
//!   query string (a cache-buster, a proxy health check) no longer 404s.
//! * **Response headers.** Both sent `Content-Type`, `Content-Length`,
//!   `Connection: close` and `Cache-Control: no-store`; kept as-is for
//!   [`respond`], since the dashboards are polled and must never be cached.
//!   A server that also serves a compiled SPA bundle wants the opposite for its
//!   assets (a megabyte of wasm re-fetched on every page load is not "instant"),
//!   which is what [`Reply`] is for: same defaults, and a caller may name its own
//!   `Cache-Control` and add `Set-Cookie`/`Location`.
//!
//! Request bodies and the query/cookie helpers were added for `switch-gui`,
//! whose client is a SPA that posts JSON rather than a hand-written `fetch` in a
//! server-rendered page.  They live here rather than in that app because all
//! three of these servers parse the same two formats, and the per-app versions
//! that existed disagreed about `+` (see [`query_params`]).
//!
//! Deliberately *not* shared: the routing tables, the JSON encoders and the
//! static-file handling.  `json_str` was byte-identical in both copies, but
//! moving it here would mean a shared JSON *API* before anyone has decided what
//! it should be; that is a bigger call than removing 17 duplicated lines.
//!
//! No dependencies, matching the house style of the crates that use it.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;

/// Largest request head accepted, bytes.  Both servers used 64 KiB.
pub const MAX_HEAD_BYTES: usize = 64 * 1024;

/// Largest request body accepted, bytes.
///
/// Every body these servers take is a small JSON intent or a `show` command, so
/// 1 MiB is generous; the cap is what stops a POST of a gigabyte from being
/// buffered in RAM before anyone looks at it.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;

/// A parsed request: request line, headers and (when `Content-Length` names
/// one) the body.
pub struct Request {
    pub method: String,
    /// Raw path exactly as it appeared in the request line.
    pub raw_path: String,
    pub headers: HashMap<String, String>,
    /// The body, already read in full.
    ///
    /// Reading it here rather than in the handler is deliberate: the head and
    /// whatever of the body already arrived are in one buffer, and a handler that
    /// forgot to drain the socket would leave the rest in the kernel buffer with
    /// nothing reading it.
    pub body: Vec<u8>,
}

impl Request {
    /// Path with any `?query` removed.  Static routes resolve by path only, and
    /// a query string must not turn an existing page into a 404.
    pub fn path(&self) -> &str {
        match self.raw_path.split_once('?') {
            Some((path, _)) => path,
            None => &self.raw_path,
        }
    }

    /// Header value, or `""`.  Names are lower-cased on the way in, so callers
    /// pass them lower-cased (`x-webauth-user`).
    pub fn header(&self, name: &str) -> &str {
        self.headers.get(name).map(|s| s.as_str()).unwrap_or("")
    }

    /// The body as UTF-8 text (invalid sequences replaced).
    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Read a request off `stream`: the head, then `Content-Length` bytes of body.
///
/// Returns `None` — and the caller drops the connection — when the peer closed
/// or failed mid-read, when the head exceeds [`MAX_HEAD_BYTES`], when the body
/// exceeds [`MAX_BODY_BYTES`], or when `Content-Length` is not a number.
///
/// Dropping rather than answering 413 is the existing behaviour for an oversized
/// head and is kept for the body too: a request this wrong is not worth a second
/// round trip, and no caller has to remember to check a size.
pub fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        match stream.read(&mut tmp) {
            Ok(0) => return None,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => return None,
        }
        if buf.len() > MAX_HEAD_BYTES {
            return None;
        }
    }

    let head_end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let mut parts = lines.next().unwrap_or("").split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let raw_path = parts.next().unwrap_or("").to_string();

    let mut headers = HashMap::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let want: usize = match headers.get("content-length").map(String::as_str) {
        None | Some("") => 0,
        Some(v) => match v.trim().parse::<usize>() {
            Ok(n) => n,
            Err(_) => return None,
        },
    };
    if want > MAX_BODY_BYTES {
        return None;
    }

    // Part of the body usually arrives in the same read as the head, so only the
    // shortfall is asked for.
    let mut body = buf[head_end + 4..].to_vec();
    body.truncate(want);
    while body.len() < want {
        let mut chunk = [0u8; 8192];
        match stream.read(&mut chunk) {
            Ok(0) => return None,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(_) => return None,
        }
    }

    Some(Request {
        method,
        raw_path,
        headers,
        body,
    })
}

/// Reason phrase for a status code.  Every code these servers actually send is
/// listed; anything else falls back to "OK" rather than emitting an empty
/// reason (which is what the per-server tables did for unlisted codes).
pub fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "OK",
    }
}

/// Write a complete response and finish the connection (`Connection: close`,
/// so no keep-alive bookkeeping is needed anywhere in these servers).
///
/// A polled dashboard must never be cached, which is why `no-store` is the
/// default here; a server serving a content-hashed SPA bundle wants the opposite
/// for its assets, and uses [`Reply`] instead.
pub fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    write_response(
        stream,
        status,
        content_type,
        body.as_bytes(),
        &[("Cache-Control".to_string(), "no-store".to_string())],
    )
}

/// Write a response with extra headers.
///
/// `extra` is emitted verbatim after `Content-Length`, which is the only way to
/// send more than one `Set-Cookie` (a `Reply`-shaped API cannot express that any
/// better without another type, and these servers set at most two).
///
/// `Connection: close` is always sent last, so no caller has to get it right.
pub fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    extra: &[(String, String)],
) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n",
        body.len(),
        reason = reason_phrase(status),
    );
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)
}

/// A response worth more than `respond`'s four arguments: a status, a content
/// type, bytes (a wasm bundle is not a `&str`) and optional extra headers.
///
/// `Cache-Control: no-store` is sent unless the caller names its own, because
/// every API these servers expose is polled and a cached answer is a bug — while
/// an asset bundle is exactly the thing that *should* be cached, and
/// wasm-bindgen's file names are content-hashed, so a long `max-age` is safe.
pub struct Reply {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
}

impl Reply {
    pub fn new(status: u16, content_type: &str, body: Vec<u8>) -> Reply {
        Reply {
            status,
            content_type: content_type.to_string(),
            body,
            headers: Vec::new(),
        }
    }

    pub fn text(status: u16, body: &str) -> Reply {
        Reply::new(
            status,
            "text/plain; charset=utf-8",
            body.as_bytes().to_vec(),
        )
    }

    pub fn html(status: u16, body: &str) -> Reply {
        Reply::new(status, "text/html; charset=utf-8", body.as_bytes().to_vec())
    }

    pub fn json(status: u16, body: &str) -> Reply {
        Reply::new(status, "application/json", body.as_bytes().to_vec())
    }

    /// A 302 with the `Cache-Control` a redirect needs (a cached redirect is how
    /// a signed-out user gets stuck on the sign-in page forever).
    pub fn redirect(location: &str, cookies: &[String]) -> Reply {
        let mut r = Reply::new(302, "text/plain; charset=utf-8", Vec::new());
        r.headers
            .push(("Location".to_string(), location.to_string()));
        r.headers
            .push(("Cache-Control".to_string(), "no-store".to_string()));
        for c in cookies {
            r.headers.push(("Set-Cookie".to_string(), c.clone()));
        }
        r
    }

    pub fn header(mut self, name: &str, value: &str) -> Reply {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// Write it out.  See [`write_response`] for the header order.
    pub fn send(self, stream: &mut TcpStream) -> std::io::Result<()> {
        let mut extra = self.headers.clone();
        if !extra
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("cache-control"))
        {
            extra.push(("Cache-Control".to_string(), "no-store".to_string()));
        }
        write_response(stream, self.status, &self.content_type, &self.body, &extra)
    }
}

/// Split a query string into its pairs, percent-decoded, `+` read as a space.
///
/// The per-app copies of this disagreed: `switch-gui` decoded `+` (correct for a
/// `application/x-www-form-urlencoded` string, which is what a browser's
/// `URLSearchParams` produces) while `gocardless-dashboard` did not, so a port
/// name with a space arrived as `NAS2+Port+1` on one server and `NAS2 Port 1` on
/// another.  Decoding `+` is the standard reading and is what a browser sends.
///
/// A later duplicate of a key wins, and a pair with no `=` is stored with an
/// empty value rather than dropped, so `?v` reads as "present and empty".
pub fn query_params(raw_path: &str) -> HashMap<String, String> {
    let query = match raw_path.split_once('?') {
        Some((_, q)) => q,
        None => return HashMap::new(),
    };
    let mut out = HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (k, v),
            None => (pair, ""),
        };
        out.insert(
            percent_decode(&k.replace('+', " ")),
            percent_decode(&v.replace('+', " ")),
        );
    }
    out
}

/// The value of one cookie out of a `Cookie:` header value, or `None`.
///
/// Takes the header value rather than the request so the same helper serves a
/// `HashMap` of headers, a `Request`, and the tiny_http servers that already
/// collected their headers that way.
pub fn cookie_value(cookie_header: &str, name: &str) -> Option<String> {
    cookie_header.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        if k.trim() == name {
            Some(v.trim().to_string())
        } else {
            None
        }
    })
}

/// Percent-decode a path segment (`%2F` -> `/`).  Invalid escapes are passed
/// through as-is rather than dropped, and invalid UTF-8 is replaced.
pub fn percent_decode(s: &str) -> String {
    fn hex_val(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }

    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(path: &str) -> Request {
        Request {
            method: "GET".into(),
            raw_path: path.into(),
            headers: HashMap::new(),
            body: Vec::new(),
        }
    }

    #[test]
    fn path_strips_the_query_string() {
        assert_eq!(req("/api/history?since=5").path(), "/api/history");
    }

    #[test]
    fn path_without_a_query_is_unchanged() {
        assert_eq!(req("/index.html").path(), "/index.html");
    }

    #[test]
    fn every_emitted_status_has_a_reason() {
        // The bug this replaced: 403 was missing from one server's table, so it
        // went out as "HTTP/1.1 403 OK".
        assert_eq!(reason_phrase(403), "Forbidden");
        assert_eq!(reason_phrase(503), "Service Unavailable");
        assert_eq!(reason_phrase(500), "Internal Server Error");
    }

    #[test]
    fn percent_decode_handles_escapes_and_garbage() {
        assert_eq!(
            percent_decode("mnt%2Dfilestore.mount"),
            "mnt-filestore.mount"
        );
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn headers_are_lower_cased_and_trimmed() {
        let mut r = req("/api/restart/x.service");
        r.headers
            .insert("x-webauth-user".to_string(), "cjdell".to_string());
        assert_eq!(r.header("x-webauth-user"), "cjdell");
        assert_eq!(r.header("x-status-token"), "");
    }

    #[test]
    fn query_params_decodes_and_splits() {
        let q = query_params("/api/x?switch=switch1&v=7-9");
        assert_eq!(q.get("switch").map(String::as_str), Some("switch1"));
        assert_eq!(q.get("v").map(String::as_str), Some("7-9"));
        assert!(query_params("/api/x").is_empty());
        assert!(query_params("/api/x?").is_empty());
    }

    /// The bug this replaces: a `+` in a name is a space, and one server read it
    /// literally, so renaming a port to `NAS2 Port 1` wrote `NAS2+Port+1`.
    #[test]
    fn query_params_reads_plus_as_a_space() {
        let q = query_params("/api/x?v=NAS2+Port+1&alt=%20x%2B%20");
        assert_eq!(q.get("v").map(String::as_str), Some("NAS2 Port 1"));
        assert_eq!(q.get("alt").map(String::as_str), Some(" x+ "));
    }

    #[test]
    fn query_params_keeps_an_empty_value() {
        let q = query_params("/api/x?tagged=&untagged=1");
        assert_eq!(q.get("tagged").map(String::as_str), Some(""));
        assert_eq!(q.get("untagged").map(String::as_str), Some("1"));
        assert_eq!(
            query_params("/api/x?flag").get("flag").map(String::as_str),
            Some("")
        );
    }

    #[test]
    fn cookie_value_finds_one_of_many() {
        let h = "a=1; switch_session=abc123; b=2";
        assert_eq!(cookie_value(h, "switch_session").as_deref(), Some("abc123"));
        assert_eq!(cookie_value(h, "missing"), None);
        assert_eq!(cookie_value("", "switch_session"), None);
        // An empty value is still a value: it means "signed out", not "absent".
        assert_eq!(
            cookie_value("switch_session=; a=1", "switch_session").as_deref(),
            Some("")
        );
    }

    /// A body that arrived in the same read as the head must not be asked for
    /// again, and a body split over two reads must not be truncated.
    #[test]
    fn read_request_takes_the_body_whole() {
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let whole = b"POST /api/intent HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello";
        std::thread::spawn(move || {
            let mut c = TcpStream::connect(addr).unwrap();
            // Head and part of the body first, the rest a moment later: the shape
            // a real client's two writes make on the wire.
            c.write_all(&whole[..38]).unwrap();
            c.flush().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));
            c.write_all(&whole[38..]).unwrap();
            c.flush().unwrap();
        });
        let (mut stream, _) = listener.accept().unwrap();
        let req = read_request(&mut stream).expect("a request");
        assert_eq!(req.method, "POST");
        assert_eq!(req.path(), "/api/intent");
        assert_eq!(req.body_text(), "hello");
        let mut extra = [0u8; 1];
        assert_eq!(stream.read(&mut extra).unwrap(), 0, "nothing left to read");
    }

    #[test]
    fn an_oversized_or_malformed_body_is_refused() {
        use std::io::Write;
        use std::net::{TcpListener, TcpStream};

        let serve = |bytes: &[u8]| -> Option<Request> {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let body = bytes.to_vec();
            std::thread::spawn(move || {
                let mut c = TcpStream::connect(addr).unwrap();
                c.write_all(&body).ok();
            });
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .ok();
            read_request(&mut stream)
        };

        // Content-Length bigger than the cap: refused before it is buffered.
        assert!(serve(b"POST / HTTP/1.1\r\nContent-Length: 99999999\r\n\r\n").is_none());
        // Content-Length that is not a number at all.
        assert!(serve(b"POST / HTTP/1.1\r\nContent-Length: many\r\n\r\n").is_none());
        // A peer that hangs up mid-body is a dropped connection, not a short one.
        assert!(serve(b"POST / HTTP/1.1\r\nContent-Length: 20\r\n\r\nshort").is_none());
        // No Content-Length means no body, which is a request, not a failure.
        let ok = serve(b"GET / HTTP/1.1\r\n\r\n").expect("a request");
        assert!(ok.body.is_empty());
    }

    /// `Cache-Control` must appear exactly once: the default and a caller's own
    /// override sent together would make the response ambiguous.
    #[test]
    fn a_reply_sends_cache_control_once() {
        use std::io::Read;
        use std::net::{TcpListener, TcpStream};

        let head_of = |reply: Reply| {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                reply.send(&mut stream).ok();
            });
            let mut stream = TcpStream::connect(addr).unwrap();
            let mut buf = Vec::new();
            stream.read_to_end(&mut buf).ok();
            String::from_utf8_lossy(&buf).into_owned()
        };

        let plain = head_of(Reply::json(200, "{}"));
        assert_eq!(plain.matches("Cache-Control").count(), 1);
        assert!(plain.contains("Cache-Control: no-store"));
        assert!(plain.contains("Connection: close"));

        let cached = head_of(
            Reply::new(200, "application/wasm", vec![0, 1, 2])
                .header("Cache-Control", "public, max-age=604800"),
        );
        assert_eq!(cached.matches("Cache-Control").count(), 1);
        assert!(cached.contains("max-age=604800"));
        // A body that is not text still goes out at exactly its length.
        assert!(cached.contains("Content-Length: 3"));

        let redir = head_of(Reply::redirect("/auth/login", &["a=1; Path=/".to_string()]));
        assert!(redir.contains("Location: /auth/login"));
        assert!(redir.contains("Set-Cookie: a=1; Path=/"));
    }
}
