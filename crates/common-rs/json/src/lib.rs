//! Minimal JSON, written from scratch so the zero-external-crate tools
//! (moonraker-exporter, status-dashboard, switch-gui) keep their house style.
//!
//! Parsing: objects, arrays, strings (with escapes), numbers, booleans and null.
//! Field order is not preserved (we only ever look values up by key).
//!
//! Emitting: [`quoted`] and [`error_object`], for the services that hand-build a
//! response body with `format!` and need a value they did not write escaped, and
//! [`emit`] for the one program that has to write back a document it did not fully
//! author (`pi-agent-sync` merges its providers into somebody else's `models.json`,
//! so everything it does not understand has to survive the round trip byte-for-byte
//! in meaning — which is what the round-trip tests below are for).
//!
//! Object key order is not preserved by [`parse`] (`BTreeMap`), so [`emit`] writes
//! keys sorted. That is invisible to every reader here (JSON objects are unordered)
//! and it is what makes a generated file stable enough to diff.
//!
//! This is the superset of what the callers need: `get`/`at` for lookup,
//! `as_str`/`as_f64`/`as_bool`/`as_u64`/`as_array` for values, and the two
//! emitters.  Adding an accessor here is cheaper than copying the parser again.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(map) => map.get(key),
            _ => None,
        }
    }

    /// Look up a nested value by a path of keys, e.g. `at(&["result", "status"])`.
    pub fn at(&self, path: &[&str]) -> Option<&Json> {
        let mut cur = self;
        for key in path {
            cur = cur.get(key)?;
        }
        Some(cur)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Json::Num(n) => Some(*n as u64),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    /// The object's entries, for a caller that has to walk a map it did not
    /// choose the keys of (a label set, a per-camera stats object, a DNS table).
    /// `BTreeMap`, so the walk is in key order and the output is stable.
    pub fn as_object(&self) -> Option<&BTreeMap<String, Json>> {
        match self {
            Json::Obj(m) => Some(m),
            _ => None,
        }
    }
}

/// Parse a complete JSON document.
pub fn parse(input: &str) -> Result<Json, String> {
    let mut p = Parser {
        bytes: input.as_bytes(),
        pos: 0,
    };
    p.skip_ws();
    let v = p.value()?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(format!("trailing characters at byte {}", p.pos));
    }
    Ok(v)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len()
            && matches!(self.bytes[self.pos], b' ' | b'\t' | b'\n' | b'\r')
        {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let b = self.peek();
        if b.is_some() {
            self.pos += 1;
        }
        b
    }

    fn value(&mut self) -> Result<Json, String> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') => {
                self.literal("true")?;
                Ok(Json::Bool(true))
            }
            Some(b'f') => {
                self.literal("false")?;
                Ok(Json::Bool(false))
            }
            Some(b'n') => {
                self.literal("null")?;
                Ok(Json::Null)
            }
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            Some(c) => Err(format!(
                "unexpected character '{}' at byte {}",
                c as char, self.pos
            )),
            None => Err("unexpected end of input".into()),
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.next(); // '{'
        let mut map = BTreeMap::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.next();
            return Ok(Json::Obj(map));
        }
        loop {
            self.skip_ws();
            let key = match self.peek() {
                Some(b'"') => self.string()?,
                _ => return Err(format!("expected string key at byte {}", self.pos)),
            };
            self.skip_ws();
            if self.next() != Some(b':') {
                return Err(format!("expected ':' at byte {}", self.pos));
            }
            let val = self.value()?;
            map.insert(key, val);
            self.skip_ws();
            match self.next() {
                Some(b',') => continue,
                Some(b'}') => return Ok(Json::Obj(map)),
                _ => return Err(format!("expected ',' or '}}' at byte {}", self.pos)),
            }
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.next(); // '['
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.next();
            return Ok(Json::Arr(items));
        }
        loop {
            items.push(self.value()?);
            self.skip_ws();
            match self.next() {
                Some(b',') => continue,
                Some(b']') => return Ok(Json::Arr(items)),
                _ => return Err(format!("expected ',' or ']' at byte {}", self.pos)),
            }
        }
    }

    fn literal(&mut self, lit: &str) -> Result<(), String> {
        let bytes = lit.as_bytes();
        if self.bytes.len() - self.pos < bytes.len()
            || &self.bytes[self.pos..self.pos + bytes.len()] != bytes
        {
            return Err(format!("expected '{}' at byte {}", lit, self.pos));
        }
        self.pos += bytes.len();
        Ok(())
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.next();
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == b'.' || c == b'e' || c == b'E' || c == b'+' || c == b'-')
        {
            self.next();
        }
        let slice = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|e| format!("bad number bytes: {}", e))?;
        let n: f64 = slice
            .parse()
            .map_err(|_| format!("invalid number '{}'", slice))?;
        Ok(Json::Num(n))
    }

    fn string(&mut self) -> Result<String, String> {
        // consume the opening quote (callers dispatch on peek())
        if self.next() != Some(b'"') {
            return Err(format!("expected string at byte {}", self.pos));
        }
        let mut out: Vec<u8> = Vec::new();
        loop {
            match self.next() {
                Some(b'"') => return Ok(String::from_utf8_lossy(&out).to_string()),
                Some(b'\\') => match self.next() {
                    Some(b'"') => out.push(b'"'),
                    Some(b'\\') => out.push(b'\\'),
                    Some(b'/') => out.push(b'/'),
                    Some(b'b') => out.push(0x08),
                    Some(b'f') => out.push(0x0c),
                    Some(b'n') => out.push(b'\n'),
                    Some(b'r') => out.push(b'\r'),
                    Some(b't') => out.push(b'\t'),
                    Some(b'u') => {
                        let cp = self.hex4()?;
                        match char::from_u32(cp) {
                            Some(c) => {
                                let mut buf = [0u8; 4];
                                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                            }
                            None => out.extend_from_slice("\u{fffd}".as_bytes()),
                        }
                    }
                    Some(c) => {
                        return Err(format!(
                            "invalid escape '\\{}' at byte {}",
                            c as char, self.pos
                        ))
                    }
                    None => return Err(format!("unterminated escape at byte {}", self.pos)),
                },
                Some(c) if c < 0x20 => {
                    return Err(format!("unescaped control char at byte {}", self.pos))
                }
                Some(c) => out.push(c),
                None => return Err("unterminated string".into()),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let mut v: u32 = 0;
        for _ in 0..4 {
            let c = self.next().ok_or("truncated \\u escape")?;
            let d = match c {
                b'0'..=b'9' => (c - b'0') as u32,
                b'a'..=b'f' => (c - b'a' + 10) as u32,
                b'A'..=b'F' => (c - b'A' + 10) as u32,
                _ => return Err(format!("invalid hex digit '{}'", c as char)),
            };
            v = v * 16 + d;
        }
        Ok(v)
    }
}

// ------------------------------------------------------------------- emitting
//
// The other half of the same problem.  These services hand-build response bodies
// as strings — a status endpoint's body is one `format!` — and the part of that
// nobody gets right by eye is quoting a value they did not write: a file name, a
// mount path, an error message from a subprocess.  One stray `"` or raw newline
// makes the whole response unparseable, so every caller grew its own escaper.
// They are all here instead.

/// `s` as a JSON string literal: quoted, with the escapes JSON requires.
///
/// Escapes `"` and `\`, the control characters with a short form (`\b \t \n \f
/// \r`), and the rest of C0 as `\u00XX`.  Everything else — including everything
/// above U+007F — goes through as UTF-8, which is what JSON allows and what
/// [`parse`] reads back.
pub fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `{"error":"…"}` — the failure body every one of these services returns.
///
/// The message goes through [`quoted`] because it is nearly always something a
/// caller, a filesystem or a subprocess wrote rather than this program.
pub fn error_object(msg: &str) -> String {
    format!(r#"{{"error":{}}}"#, quoted(msg))
}

/// Serialise a document. `indent` is spaces per level; 0 means one line.
///
/// The reason this is here rather than inside the one caller that needs it: the
/// alternative is a caller that parses a file, keeps the parts it does not
/// understand as [`Json`] and has nowhere to put them back. Keys come out sorted
/// because [`parse`] does not keep order, and writing a *stable* file matters more
/// here than writing the input's order — a config that shuffles on every run cannot
/// be diffed, and `pi-agent-sync` rewrites its output on a timer.
pub fn emit(value: &Json, indent: usize) -> String {
    let mut out = String::new();
    write_value(&mut out, value, indent, 0);
    out
}

fn write_value(out: &mut String, value: &Json, indent: usize, depth: usize) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Num(n) => out.push_str(&number(*n)),
        Json::Str(s) => out.push_str(&quoted(s)),
        Json::Arr(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline_indent(out, indent, depth + 1);
                write_value(out, item, indent, depth + 1);
            }
            newline_indent(out, indent, depth);
            out.push(']');
        }
        Json::Obj(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline_indent(out, indent, depth + 1);
                out.push_str(&quoted(k));
                out.push(':');
                if indent > 0 {
                    out.push(' ');
                }
                write_value(out, v, indent, depth + 1);
            }
            newline_indent(out, indent, depth);
            out.push('}');
        }
    }
}

fn newline_indent(out: &mut String, indent: usize, depth: usize) {
    if indent == 0 {
        return;
    }
    out.push('\n');
    for _ in 0..indent * depth {
        out.push(' ');
    }
}

/// Numbers round-trip through [`Json`] as `f64`, so an integer in a config file
/// would otherwise come back as `262144.0`. Pi wants a number either way, but the
/// files are read by people too, and a context window written `262144.0` is a
/// context window that looks like a bug.
fn number(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 {
        format!("{}", n as i64)
    } else {
        let s = format!("{n}");
        // `{:}` on a float gives `inf`/`NaN` for the non-finite cases, which JSON
        // has no spelling for.  Emit `null` rather than a document `parse` would
        // reject: silent data loss beats a file that cannot be read at all, and the
        // only way to get here is a caller that put a NaN in.
        if s == "inf" || s == "-inf" || s == "NaN" {
            "null".to_string()
        } else {
            s
        }
    }
}

/// A JSON string value, for callers building a document.
pub fn string(s: impl Into<String>) -> Json {
    Json::Str(s.into())
}

/// A JSON number from an integer, so callers do not each write `Json::Num(x as f64)`.
pub fn number_from_int(n: u64) -> Json {
    Json::Num(n as f64)
}

/// A JSON number from a float (a temperature, a ratio, a duration in seconds).
///
/// Named for its argument type rather than called `number`, because that name is
/// already this file's private float->text renderer, and two things called
/// `number` in one file is how a reader starts trusting the wrong one.
pub fn number_from_f64(n: f64) -> Json {
    Json::Num(n)
}

/// A JSON boolean, for the flags in a protocol message (`isError`, `open`…).  A
/// `bool` is also a native Rust type, so this only exists so that building a
/// document reads the same way for every field.
pub fn boolean(b: bool) -> Json {
    Json::Bool(b)
}

/// A JSON `null`, for the field that must be *present* and empty.
pub fn null() -> Json {
    Json::Null
}

/// A JSON array of any values.
pub fn array(items: impl IntoIterator<Item = Json>) -> Json {
    Json::Arr(items.into_iter().collect())
}

/// A JSON object from pairs, in any order (they are sorted on the way out).
pub fn object(pairs: impl IntoIterator<Item = (String, Json)>) -> Json {
    let mut map = std::collections::BTreeMap::new();
    for (k, v) in pairs {
        map.insert(k, v);
    }
    Json::Obj(map)
}

/// A JSON array of strings.
pub fn string_array(items: impl IntoIterator<Item = String>) -> Json {
    Json::Arr(items.into_iter().map(Json::Str).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape the DigitalOcean DNS API returns.  Nothing in the tree reads that
    /// API any more (`*.int` is the router's, from common/int-dns.nix, and public
    /// names are hand-made), but it stays as the real-world-document case: an API
    /// page of nested objects is the input most likely to find a parser bug.
    #[test]
    fn parses_a_digitalocean_records_page() {
        let doc = parse(
            r#"{"links":{"next":"?page=2"},"meta":{"total":3},"page":1,"per_page":200,
                "records":[{"id":"1","name":"filestore.int","data":"10.3.1.20","type":"A","ttl":1800},
                           {"id":"2","name":"x.int","data":"","type":"CNAME"},
                           {"id":"3","name":"y","type":"TXT"}]}"#,
        )
        .unwrap();
        assert_eq!(doc.get("page").and_then(|j| j.as_u64()), Some(1));
        let records = doc.get("records").and_then(|j| j.as_array()).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(
            records[0].get("name").and_then(|j| j.as_str()),
            Some("filestore.int")
        );
        assert_eq!(records[0].get("ttl").and_then(|j| j.as_u64()), Some(1800));
        // A record with no `data` key is absent, not empty.
        assert!(records[2].get("data").is_none());
        assert_eq!(records[1].get("data").and_then(|j| j.as_str()), Some(""));
    }

    /// The shape Moonraker returns (moonraker-exporter reads this).
    #[test]
    fn parses_a_moonraker_status_response() {
        let doc = parse(
            r#"{"result":{"klippy_state":"ready","print_stats":{"state":"printing",
                 "filename":"benchy.gcode","filament_used":1234.5,"message":null},
                 "temperature":45.25,"is_active":true}}"#,
        )
        .unwrap();
        assert_eq!(
            doc.at(&["result", "klippy_state"]).and_then(|j| j.as_str()),
            Some("ready")
        );
        assert_eq!(
            doc.at(&["result", "print_stats", "filename"])
                .and_then(|j| j.as_str()),
            Some("benchy.gcode")
        );
        let used = doc.at(&["result", "print_stats", "filament_used"]).unwrap();
        assert_eq!(used.as_f64(), Some(1234.5));
        assert_eq!(
            doc.at(&["result", "temperature"]).and_then(|j| j.as_f64()),
            Some(45.25)
        );
        assert_eq!(
            doc.at(&["result", "is_active"]).and_then(|j| j.as_bool()),
            Some(true)
        );
        assert!(doc.at(&["result", "print_stats", "message"]).unwrap() == &Json::Null);
        // A missing path is None at every level, not a panic.
        assert!(doc.at(&["result", "nope", "deeper"]).is_none());
        assert!(doc.get("nope").is_none());
    }

    #[test]
    fn scalars_and_containers_round_trip() {
        assert_eq!(parse("null").unwrap(), Json::Null);
        assert_eq!(parse("true").unwrap(), Json::Bool(true));
        assert_eq!(parse("false").unwrap(), Json::Bool(false));
        assert_eq!(parse("  42  ").unwrap(), Json::Num(42.0));
        assert_eq!(parse("\"\"").unwrap(), Json::Str(String::new()));
        assert_eq!(parse("[]").unwrap(), Json::Arr(vec![]));
        assert_eq!(parse("{}").unwrap(), Json::Obj(BTreeMap::new()));
        assert_eq!(
            parse("[1, [2, [3]]]").unwrap(),
            Json::Arr(vec![
                Json::Num(1.0),
                Json::Arr(vec![Json::Num(2.0), Json::Arr(vec![Json::Num(3.0)])]),
            ])
        );
    }

    #[test]
    fn numbers_accept_negatives_exponents_and_zero() {
        for (src, want) in [
            ("-1", -1.0),
            ("0", 0.0),
            ("-0.5", -0.5),
            ("1e3", 1000.0),
            ("1.5e-2", 0.015),
            ("2E+2", 200.0),
        ] {
            assert_eq!(parse(src).unwrap(), Json::Num(want), "parsing {src}");
        }
    }

    #[test]
    fn strings_decode_escapes() {
        let s = parse(r#""a\"b\\c\/d\n\t\r\b\f\u00e9\u6210""#).unwrap();
        assert_eq!(
            s.as_str().unwrap(),
            "a\"b\\c/d\n\t\r\u{8}\u{c}\u{e9}\u{6210}"
        );
    }

    #[test]
    fn as_u64_truncates_a_float() {
        // The exporters read ints out of float-typed JSON; the truncation is
        // deliberate, so pin it.
        assert_eq!(parse("7.9").unwrap().as_u64(), Some(7));
        assert_eq!(parse("7").unwrap().as_u64(), Some(7));
        assert_eq!(parse("\"7\"").unwrap().as_u64(), None);
    }

    #[test]
    fn accessors_refuse_the_wrong_type() {
        let doc = parse(r#"{"s":"x","n":1,"b":true,"a":[],"o":{}}"#).unwrap();
        for k in ["s", "n", "b", "a", "o"] {
            let v = doc.get(k).unwrap();
            assert!(v.as_str().is_some() == (k == "s"), "{k}");
            assert!(v.as_f64().is_some() == (k == "n"), "{k}");
            assert!(v.as_bool().is_some() == (k == "b"), "{k}");
            assert!(v.as_array().is_some() == (k == "a"), "{k}");
            assert!(v.get("any").is_none(), "{k} is not an object");
        }
    }

    #[test]
    fn malformed_documents_error_instead_of_guessing() {
        for bad in [
            "",                 // empty document
            "{",                // unterminated object
            "[1,",              // unterminated array
            r#""unterminated"#, // unterminated string
            "{\"a\" 1}",        // missing colon
            "{\"a\":1,}",       // trailing comma
            "1 2",              // trailing value
            "nul",              // misspelled literal
            "tru",
            r#""\q""#, // invalid escape
        ] {
            let r = parse(bad);
            assert!(r.is_err(), "{bad} should not parse: {:?}", r);
        }
        // Whitespace outside a string is fine; only control characters *inside*
        // a string are rejected.
        assert!(parse("{\"a\"\n:1}").is_ok());
    }

    #[test]
    fn a_raw_control_char_in_a_string_is_rejected() {
        // A literal newline inside a string is not valid JSON; accepting it
        // would let a value smuggle a line break into the dnsmasq hosts file.
        assert!(parse("\"a\nb\"").is_err());
        assert!(parse("\"a\tb\"").is_err());
    }

    #[test]
    fn duplicate_keys_keep_the_last_value() {
        // BTreeMap insert-overwrite: worth pinning because a document that names a
        // key twice is a document some API will one day return.
        let doc = parse(r#"{"name":"first","name":"second"}"#).unwrap();
        assert_eq!(doc.get("name").and_then(|j| j.as_str()), Some("second"));
    }

    #[test]
    fn number_grammar_is_lenient_where_it_does_not_matter() {
        // A leading zero is not valid JSON, but the parser accepts it (and the
        // APIs it reads never send one).  Pinned so a future strictness change
        // is a deliberate one.
        assert_eq!(parse("01").unwrap(), Json::Num(1.0));
    }

    #[test]
    fn whitespace_is_ignored_everywhere() {
        let doc = parse(" {\n \"a\" :\t[ 1 ,\r\n 2 ]\n} ").unwrap();
        assert_eq!(
            doc.get("a").unwrap(),
            &Json::Arr(vec![Json::Num(1.0), Json::Num(2.0)])
        );
    }
}

#[cfg(test)]
mod emitting_tests {
    use super::*;

    #[test]
    fn quoted_round_trips_through_the_parser() {
        for s in [
            "",
            "plain",
            "a b",
            "he said \"hi\"",
            "back\\slash",
            "tab\there",
            "new\nline",
            "carriage\rreturn",
            "bell\u{7}",
            "nul\u{0}",
            "café",
            "emoji \u{1F600}",
            "angle <>&",
        ] {
            let out = quoted(s);
            assert!(out.starts_with('"') && out.ends_with('"'), "{out}");
            let parsed = parse(&out).unwrap_or_else(|e| panic!("{s:?} -> {out}: {e}"));
            assert_eq!(parsed.as_str(), Some(s));
        }
    }

    #[test]
    fn quoted_uses_the_short_escapes() {
        // Not merely parseable: a newline written as \n keeps a one-line response
        // one line, which is the reason these bodies go to the journal.
        assert_eq!(quoted("a\nb"), "\"a\\nb\"");
        assert_eq!(quoted("a\tb"), "\"a\\tb\"");
        assert_eq!(quoted("a\u{8}b"), "\"a\\bb\"");
        assert_eq!(quoted("a\u{c}b"), "\"a\\fb\"");
        assert_eq!(quoted("a\r\nb"), "\"a\\r\\nb\"");
    }

    #[test]
    fn quoted_escapes_the_rest_of_c0_as_a_codepoint() {
        assert_eq!(quoted("\u{1}\u{1f}"), "\"\\u0001\\u001f\"");
        assert_eq!(parse(&quoted("\u{1f}")).unwrap().as_str(), Some("\u{1f}"));
    }

    #[test]
    fn a_hostile_message_cannot_break_the_error_object() {
        // The case every copy of this helper existed for: a path or a subprocess
        // message that closes the string and starts a key of its own.
        let msg = "x\", \"admin\": true, \"y";
        let doc = parse(&error_object(msg)).unwrap();
        assert_eq!(doc.get("error").and_then(|j| j.as_str()), Some(msg));
        assert_eq!(doc.get("admin"), None);
    }

    #[test]
    fn error_object_survives_a_multiline_message() {
        let doc = parse(&error_object("line one\nline two")).unwrap();
        assert_eq!(
            doc.get("error").and_then(|j| j.as_str()),
            Some("line one\nline two")
        );
    }

    /// The property `pi-agent-sync` depends on: it parses a file it does not own,
    /// replaces the keys it does own, and writes the rest back. Anything it did not
    /// understand has to still be there, with the same values.
    #[test]
    fn emit_round_trips_a_document_it_did_not_write() {
        let src = r#"{"$schema":"https://pi.dev/schemas/models.schema.json",
                       "providers":{"hand":{"baseUrl":"http://x/v1","apiKey":"k","models":[],"n":3,"t":true,"z":null}},
                       "oddKey":[1,2.5,"s",[true],{"a":{"b":[]}}]}"#;
        let doc = parse(src).unwrap();
        let out = emit(&doc, 2);
        let again = parse(&out).unwrap();
        assert_eq!(doc, again, "emit(parse(x)) must equal parse(x): {out}");
        // Integers stay integers on the page, and nesting is indented.
        assert!(out.contains("\"n\": 3"), "{out}");
        assert!(out.contains("2.5"), "{out}");
        assert!(!out.contains("3.0"), "{out}");
    }

    #[test]
    fn emit_is_stable_and_empty_containers_are_compact() {
        let doc = parse(r#"{"b":1,"a":{"y":[],"z":{}},"c":"\u0001"}"#).unwrap();
        let once = emit(&doc, 2);
        assert_eq!(once, emit(&parse(&once).unwrap(), 2));
        assert!(once.contains("\"z\": {}"), "{once}");
        assert!(once.starts_with("{\n  \"a\""), "keys sorted: {once}");
        assert_eq!(parse(&emit(&doc, 0)).unwrap(), doc);
    }
}
