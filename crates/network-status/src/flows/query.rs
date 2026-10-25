use crate::flows::Kind;
use std::collections::HashMap;

/// Which flow to show first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Rate,
    Down,
    Up,
    Bytes,
    Age,
    Host,
}

impl Sort {
    pub(crate) fn parse(s: &str) -> Sort {
        match s {
            "down" => Sort::Down,
            "up" => Sort::Up,
            "bytes" => Sort::Bytes,
            "age" => Sort::Age,
            "host" => Sort::Host,
            _ => Sort::Rate,
        }
    }

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Sort::Rate => "rate",
            Sort::Down => "down",
            Sort::Up => "up",
            Sort::Bytes => "bytes",
            Sort::Age => "age",
            Sort::Host => "host",
        }
    }
}

/// What to show, after defaults and clamping.
#[derive(Clone, Debug)]
pub struct Query {
    /// Hide flows younger than this — the 5-second rule the page exists for.
    pub min_age: u64,
    /// Hide flows that have moved less than this in total.
    pub min_bytes: u64,
    /// Hide flows quieter than this.
    pub min_bps: f64,
    /// Case-insensitive substring of addresses, names, ports, protocol, state.
    pub needle: Option<String>,
    /// `None` keeps every kind.
    pub kind: Option<Kind>,
    pub sort: Sort,
    pub limit: usize,
}

impl Default for Query {
    fn default() -> Self {
        Query {
            min_age: 5,
            min_bytes: 0,
            min_bps: 0.0,
            needle: None,
            kind: None,
            sort: Sort::Rate,
            limit: 100,
        }
    }
}

/// Pull the filters out of a request's query string, falling back to the
/// operator's defaults and clamping `limit` to what `--conn-max-rows` allowed.
pub fn parse_query(params: &HashMap<String, String>, defaults: &Query) -> Query {
    let secs = |key: &str, cur: u64| -> u64 {
        params
            .get(key)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .map(|v| v as u64)
            .unwrap_or(cur)
    };
    Query {
        min_age: secs("min_age", defaults.min_age),
        min_bytes: secs("min_bytes", defaults.min_bytes),
        min_bps: params
            .get("min_bps")
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(defaults.min_bps),
        needle: params
            .get("q")
            .map(|v| v.trim().to_lowercase())
            .filter(|v| !v.is_empty()),
        kind: params.get("kind").and_then(|v| Kind::parse(v)),
        sort: params
            .get("sort")
            .map(|v| Sort::parse(v))
            .unwrap_or(defaults.sort),
        limit: params
            .get("limit")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(defaults.limit)
            .clamp(1, defaults.limit),
    }
}
