//! The JSON output primitives this crate hand-rolls.
//!
//! `common-rs/json` is a *parser* plus `quoted`; the emitters stayed in the
//! servers that needed them (that split is called out in its header).  This is
//! the crate's single copy — they had begun to duplicate into the connection
//! table's renderer, which is exactly how `service` and `json`'s emitters
//! drifted apart before `common-rs` existed.

use common_json::quoted;

/// A finite number, or `null`: a NaN rate in a JSON body is invalid JSON, and
/// the SPA's `JSON.parse` would fail the whole snapshot over one bad row.
pub(crate) fn jnum(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "null".into()
    }
}

/// `None` as JSON `null`, so the SPA can tell "zero bytes" from "the kernel is
/// not counting this connection" (see `conntrack`'s note on `acct`).
pub(crate) fn jopt_num(v: Option<f64>) -> String {
    v.map(jnum).unwrap_or_else(|| "null".into())
}

pub(crate) fn jopt_u64(v: Option<u64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "null".into())
}

pub(crate) fn jopt_u16(v: Option<u16>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "null".into())
}

/// An optional string, quoted or `null`.
pub(crate) fn opt_str(v: Option<&str>) -> String {
    v.map(quoted).unwrap_or_else(|| "null".into())
}
