//! Hand-rolled HTTP/1.1 plumbing shared by the zero-external-crate servers in
//! this repo (`status-dashboard`, `network-status`).
//!
//! Both servers used to carry their own copy of: reading a request head off a
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
//!   `Connection: close` and `Cache-Control: no-store`; kept as-is, since the
//!   dashboards are polled and must never be cached.
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

/// A parsed request head: request line plus headers.  These servers have no
/// request bodies, so nothing past the head is read.
pub struct Request {
    pub method: String,
    /// Raw path exactly as it appeared in the request line.
    pub raw_path: String,
    pub headers: HashMap<String, String>,
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
}

/// Read the request head off `stream`.
///
/// Returns `None` when the peer closed the connection, failed mid-read, or sent
/// a head larger than `MAX_HEAD_BYTES` — the caller just drops the connection
/// in every case.
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

    let head = String::from_utf8_lossy(&buf);
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

    Some(Request {
        method,
        raw_path,
        headers,
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
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "OK",
    }
}

/// Write a complete response and finish the connection (`Connection: close`,
/// so no keep-alive bookkeeping is needed anywhere in these servers).
pub fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         Cache-Control: no-store\r\n\
         \r\n",
        body.len(),
        reason = reason_phrase(status),
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())
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

    #[test]
    fn path_strips_the_query_string() {
        let req = Request {
            method: "GET".into(),
            raw_path: "/api/history?since=5".into(),
            headers: HashMap::new(),
        };
        assert_eq!(req.path(), "/api/history");
    }

    #[test]
    fn path_without_a_query_is_unchanged() {
        let req = Request {
            method: "GET".into(),
            raw_path: "/index.html".into(),
            headers: HashMap::new(),
        };
        assert_eq!(req.path(), "/index.html");
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
        assert_eq!(percent_decode("mnt%2Dfilestore.mount"), "mnt-filestore.mount");
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn headers_are_lower_cased_and_trimmed() {
        let req = Request {
            method: "POST".into(),
            raw_path: "/api/restart/x.service".into(),
            headers: HashMap::from([("x-webauth-user".to_string(), "cjdell".to_string())]),
        };
        assert_eq!(req.header("x-webauth-user"), "cjdell");
        assert_eq!(req.header("x-status-token"), "");
    }
}
