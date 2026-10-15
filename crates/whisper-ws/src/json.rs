//! Minimal JSON (RFC 8259) parser + serializer, zero dependencies.
//!
//! Objects keep insertion order (a `Vec` of pairs) so the events we emit
//! match OpenAI's field ordering closely enough to be diff-friendly.
//! Only the subset needed by the Realtime protocol is implemented, but it is
//! a fully general (if small) JSON implementation.

#[derive(Debug, Clone)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    pub fn object(pairs: Vec<(&str, Value)>) -> Value {
        Value::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn parse(s: &str) -> Result<Value, String> {
        let mut p = Parser { b: s.as_bytes(), i: 0 };
        let v = p.value()?;
        p.skip_ws();
        if p.i != p.b.len() {
            return Err(format!("trailing characters at offset {}", p.i));
        }
        Ok(v)
    }

    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Num(n) => {
                // Integers render without a trailing ".0"; floats render via
                // Rust's shortest round-trip formatting.
                if n.is_finite() && n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
                    out.push_str(&format!("{n:.0}"));
                } else {
                    out.push_str(&format!("{n}"));
                }
            }
            Value::Str(s) => write_string(s, out),
            Value::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Value::Obj(pairs) => {
                out.push('{');
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn value(&mut self) -> Result<Value, String> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b't') => {
                self.literal("true")?;
                Ok(Value::Bool(true))
            }
            Some(b'f') => {
                self.literal("false")?;
                Ok(Value::Bool(false))
            }
            Some(b'n') => {
                self.literal("null")?;
                Ok(Value::Null)
            }
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            _ => Err(format!("unexpected character at offset {}", self.i)),
        }
    }

    fn literal(&mut self, lit: &str) -> Result<(), String> {
        if self.b.len() - self.i < lit.len() || &self.b[self.i..self.i + lit.len()] != lit.as_bytes() {
            return Err(format!("invalid literal at offset {}", self.i));
        }
        self.i += lit.len();
        Ok(())
    }

    fn object(&mut self) -> Result<Value, String> {
        self.i += 1; // '{'
        let mut pairs = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Value::Obj(pairs));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(format!("expected string key at offset {}", self.i));
            }
            let key = self.string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(format!("expected ':' at offset {}", self.i));
            }
            self.i += 1;
            let val = self.value()?;
            pairs.push((key, val));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.i += 1;
                }
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Value::Obj(pairs));
                }
                _ => return Err(format!("expected ',' or '}}' at offset {}", self.i)),
            }
        }
    }

    fn array(&mut self) -> Result<Value, String> {
        self.i += 1; // '['
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Value::Arr(items));
        }
        loop {
            items.push(self.value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.i += 1;
                }
                Some(b']') => {
                    self.i += 1;
                    return Ok(Value::Arr(items));
                }
                _ => return Err(format!("expected ',' or ']' at offset {}", self.i)),
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.i += 1; // opening quote
        let mut out = String::new();
        loop {
            let c = *self
                .b
                .get(self.i)
                .ok_or_else(|| format!("unterminated string at offset {}", self.i))?;
            self.i += 1;
            match c {
                b'"' => return Ok(out),
                b'\\' => {
                    let esc = *self
                        .b
                        .get(self.i)
                        .ok_or_else(|| "unterminated escape".to_string())?;
                    self.i += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hex = self.b.get(self.i..self.i + 4).ok_or("bad \\u escape")?;
                            let cp = u32::from_str_radix(std::str::from_utf8(hex).map_err(|_| "bad \\u")?, 16)
                                .map_err(|_| "bad \\u escape")?;
                            self.i += 4;
                            // Surrogate pair (out of BMP).
                            if (0xD800..=0xDBFF).contains(&cp) {
                                if self.b.get(self.i) == Some(&b'\\') && self.b.get(self.i + 1) == Some(&b'u') {
                                    let hex2 = self.b.get(self.i + 2..self.i + 6).ok_or("bad \\u surrogate")?;
                                    let lo = u32::from_str_radix(
                                        std::str::from_utf8(hex2).map_err(|_| "bad \\u surrogate")?,
                                        16,
                                    )
                                    .map_err(|_| "bad \\u surrogate")?;
                                    if (0xDC00..=0xDFFF).contains(&lo) {
                                        self.i += 6;
                                        let combined = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                        out.push(char::from_u32(combined).unwrap_or('\u{FFFD}'));
                                        continue;
                                    }
                                }
                                out.push('\u{FFFD}');
                            } else {
                                out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                            }
                        }
                        _ => return Err(format!("invalid escape '\\{}'", esc as char)),
                    }
                }
                c if c < 0x20 => return Err("control character in string".to_string()),
                c => out.push(c as char),
            }
        }
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        while self.i < self.b.len() && (self.b[self.i].is_ascii_digit() || matches!(self.b[self.i], b'.' | b'e' | b'E' | b'+' | b'-')) {
            self.i += 1;
        }
        let raw = std::str::from_utf8(&self.b[start..self.i]).map_err(|_| "bad number")?;
        let n: f64 = raw.parse().map_err(|_| format!("invalid number {raw:?}"))?;
        Ok(Value::Num(n))
    }
}
