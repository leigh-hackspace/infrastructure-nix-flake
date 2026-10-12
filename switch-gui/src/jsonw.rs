//! Writing JSON — the escape rules only.
//!
//! `common-rs/json` parses and deliberately does not serialise, and this tool
//! emits two small documents (`/api/snapshot`, `/api/audit`), so the escaping
//! lives here rather than as a new dependency.
//!
//! The rules are RFC 8259 §7: `"` and `\` are escaped, the six one-letter
//! control escapes get their short form, and any other code point below 0x20
//! becomes `\u00XX`.  Control characters are not academic here — a port
//! description or a `show` command's output can contain a tab, and an audit
//! entry can contain anything an operator typed.

pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// `s` wrapped in double quotes, escaped.
pub fn quoted(s: &str) -> String {
    format!("\"{}\"", escape(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use common_json as json;

    #[test]
    fn the_mandatory_escapes() {
        assert_eq!(escape("a\"b"), "a\\\"b");
        assert_eq!(escape("a\\b"), "a\\\\b");
        assert_eq!(escape("line\nbreak\ttab\r"), "line\\nbreak\\ttab\\r");
        assert_eq!(escape("\u{8}\u{c}"), "\\b\\f");
    }

    #[test]
    fn other_control_characters_use_hex() {
        assert_eq!(escape("\u{1}\u{1f}"), "\\u0001\\u001f");
    }

    #[test]
    fn printable_text_is_untouched() {
        assert_eq!(escape("NAS2 Port 1 · 100/1000T"), "NAS2 Port 1 · 100/1000T");
        assert_eq!(escape(""), "");
    }

    /// The real test: everything we can produce must parse back to the same
    /// string through the repo's own parser.
    #[test]
    fn round_trips_through_the_parser() {
        for s in [
            "plain",
            "quote\"inside",
            "back\\slash",
            "tab\tand\nnewline",
            "\u{0}null byte",
            "emoji 🎛",
        ] {
            let doc = format!("{{\"v\":{}}}", quoted(s));
            let got = json::parse(&doc)
                .unwrap_or_else(|e| panic!("{s:?} -> {doc} did not parse: {e}"))
                .get("v")
                .and_then(|v| v.as_str())
                .expect("a string")
                .to_string();
            assert_eq!(got, s);
        }
    }
}
