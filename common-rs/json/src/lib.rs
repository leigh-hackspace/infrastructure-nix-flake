//! Minimal JSON parser, written from scratch so the zero-external-crate tools
//! (dns-sync, moonraker-exporter, status-dashboard) keep their house style.
//!
//! Supports objects, arrays, strings (with escapes), numbers, booleans and
//! null. Field order is not preserved (we only ever look values up by key).
//!
//! This is the superset of what the two callers need: `get`/`at` for lookup,
//! `as_str`/`as_f64`/`as_bool`/`as_u64`/`as_array` for values. Adding an
//! accessor here is cheaper than copying the parser again.

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
        if self.bytes.len() - self.pos < bytes.len() || &self.bytes[self.pos..self.pos + bytes.len()] != bytes {
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
                Some(b'"') => {
                    return Ok(String::from_utf8_lossy(&out).to_string())
                }
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
                        return Err(format!("invalid escape '\\{}' at byte {}", c as char, self.pos))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape the DigitalOcean DNS API returns (dns-sync reads this).
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
        assert_eq!(records[0].get("name").and_then(|j| j.as_str()), Some("filestore.int"));
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
            doc.at(&["result", "print_stats", "filename"]).and_then(|j| j.as_str()),
            Some("benchy.gcode")
        );
        let used = doc.at(&["result", "print_stats", "filament_used"]).unwrap();
        assert_eq!(used.as_f64(), Some(1234.5));
        assert_eq!(doc.at(&["result", "temperature"]).and_then(|j| j.as_f64()), Some(45.25));
        assert_eq!(doc.at(&["result", "is_active"]).and_then(|j| j.as_bool()), Some(true));
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
        assert_eq!(s.as_str().unwrap(), "a\"b\\c/d\n\t\r\u{8}\u{c}\u{e9}\u{6210}");
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
            "",                     // empty document
            "{",                    // unterminated object
            "[1,",                  // unterminated array
            r#""unterminated"#,     // unterminated string
            "{\"a\" 1}",           // missing colon
            "{\"a\":1,}",          // trailing comma
            "1 2",                  // trailing value
            "nul",                  // misspelled literal
            "tru",
            r#""\q""#,             // invalid escape
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
        // BTreeMap insert-overwrite: worth pinning because dns-sync builds the
        // expected-name set from these documents.
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
