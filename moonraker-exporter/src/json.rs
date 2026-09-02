//! Minimal JSON parser, written from scratch so the project keeps its
//! "zero external crates" house style (see status-dashboard/).
//!
//! This is a copy of the parser used by `dns-sync` (../../dns-sync/src/json.rs)
//! with a couple of extra accessors (`as_f64`, `as_bool`, `at`) that the
//! moonraker-exporter needs.
//!
//! Supports objects, arrays, strings (with escapes), numbers, booleans and
//! null. Field order is not preserved (we only ever look values up by key).

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
                    other => {
                        return Err(format!("invalid escape at byte {}", self.pos))
                    }
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
