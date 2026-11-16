//! `sources` — where the building's data comes from, and the counters that say
//! whether it is answering.
//!
//! One struct holds every origin, resolved once from flags at startup. Handlers
//! get an `Arc<Sources>` and never see a URL built from a default they did not
//! choose, which is what makes `--space-url` a way to point the whole tool set at
//! a fixture for testing rather than a half-implemented escape hatch.
//!
//! ## Why these are plain HTTP origins
//!
//! Deliberate, and measured: `hackspace-mcp` is reached only by `slack-agent` on
//! the same box, and everything it reads answers plain HTTP on the LAN (AGENTS.md
//! does not need to know that; `common-rs/http`'s header does, because that is the
//! crate that refuses `https://`). The front door's `*.int` TLS names are the same
//! services seen from a browser. Going through them from here would add a TLS
//! handshake, a dependency, and — for `api.int` — a hop through the box that
//! answers DNS for the whole network, to fetch a 2 KiB JSON file.
//!
//! ## What is counted
//!
//! Every tool call, per tool, split into ok and error, on `/metrics`. A bot that
//! is quiet is indistinguishable from a bot whose tools all fail, and that is
//! exactly the failure this fleet has already been bitten by once
//! (`docs/nix-dev-workflow.md`, "a channel can die while still answering"). The
//! counters are the cheapest thing that makes those two cases different.

use common_json::{self as json, Json};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

/// Counters, keyed by tool name.  A `BTreeMap` so `/metrics` is stable across
/// scrapes and a diff of two scrapes means something.
#[derive(Default)]
pub struct Stats {
    pub http_ok: u64,
    pub http_err: u64,
    pub tools: BTreeMap<String, (u64, u64)>,
}

/// Every origin one process reads.
pub struct Sources {
    /// The hackspace Space API (`hackspace-api` on `apps1`).
    pub space: String,
    /// Home Assistant's HTTP root.
    pub hass: String,
    /// Long-lived access token for Home Assistant, or `None` — which is a state
    /// the tools report rather than an error they crash on, because half the tool
    /// set still works without it.
    pub hass_token: Option<String>,
    pub frigate: String,
    pub prometheus: String,
    /// The router's lease API (`network-status` on `gw`).
    pub leases: String,
    pub timeout: Duration,
    pub stats: Mutex<Stats>,
}

impl Sources {
    pub fn new(
        space: String,
        hass: String,
        hass_token: Option<String>,
        frigate: String,
        prometheus: String,
        leases: String,
        timeout: Duration,
    ) -> Self {
        Sources {
            space,
            hass,
            hass_token,
            frigate,
            prometheus,
            leases,
            timeout,
            stats: Mutex::new(Stats::default()),
        }
    }

    /// `http://host:port` with no trailing slash, so callers append paths.
    pub fn trim(origin: &str) -> String {
        origin.trim_end_matches('/').to_string()
    }

    /// GET and require 2xx, returning the body.
    pub fn fetch(&self, url: &str) -> Result<String, String> {
        let r = common_http::request("GET", url, None, &[], self.timeout);
        match r {
            Ok(r) if r.ok() => {
                if let Ok(mut s) = self.stats.lock() {
                    s.http_ok += 1;
                }
                Ok(r.body)
            }
            // A non-2xx carries the server's own reason, and that is the sentence
            // we want the model to be able to repeat.
            Ok(r) => {
                if let Ok(mut s) = self.stats.lock() {
                    s.http_err += 1;
                }
                Err(format!("{url}: {}", r.describe()))
            }
            Err(e) => {
                if let Ok(mut s) = self.stats.lock() {
                    s.http_err += 1;
                }
                Err(e)
            }
        }
    }

    pub fn fetch_json(&self, url: &str) -> Result<Json, String> {
        let body = self.fetch(url)?;
        json::parse(&body).map_err(|e| format!("{url}: body is not JSON ({e}): {:.120}", body.trim()))
    }

    /// GET against Home Assistant, with the token, and a message that tells the
    /// model what is missing instead of pretending the building is empty.
    pub fn fetch_hass(&self, path_and_query: &str) -> Result<Json, String> {
        let url = format!("{}{}", self.hass, path_and_query);
        let token = self.hass_token.clone().ok_or_else(|| {
            "Home Assistant is not configured for this server (no HA token). Say so plainly: \
             report what the other tools can see and do not guess at entity states."
                .to_string()
        })?;
        let r = common_http::request(
            "GET",
            &url,
            None,
            &[("Authorization", &format!("Bearer {token}"))],
            self.timeout,
        );
        match r {
            Ok(r) if r.ok() => {
                if let Ok(mut s) = self.stats.lock() {
                    s.http_ok += 1;
                }
                json::parse(&r.body)
                    .map_err(|e| format!("{url}: body is not JSON ({e}): {:.120}", r.body.trim()))
            }
            Ok(r) => {
                if let Ok(mut s) = self.stats.lock() {
                    s.http_err += 1;
                }
                let why = if r.status == 401 || r.status == 403 {
                    "the Home Assistant token this server was given was refused (expired, or \
                     revoked — it needs re-minting)"
                } else {
                    "Home Assistant refused the request"
                };
                Err(format!("{url}: {} — {why}", r.describe()))
            }
            Err(e) => {
                if let Ok(mut s) = self.stats.lock() {
                    s.http_err += 1;
                }
                Err(e)
            }
        }
    }

    pub fn note_tool(&self, name: &str, ok: bool) {
        if let Ok(mut s) = self.stats.lock() {
            let e = s.tools.entry(name.to_string()).or_insert((0, 0));
            if ok {
                e.0 += 1;
            } else {
                e.1 += 1;
            }
        }
    }

    /// Prometheus text exposition.  Scraped by services1's own Prometheus.
    pub fn render_metrics(&self) -> String {
        let Ok(s) = self.stats.lock() else {
            return "# stats lock poisoned\n".to_string();
        };
        let mut out = String::new();
        out.push_str("# HELP hackspace_mcp_http_total Upstream fetches, by outcome.\n");
        out.push_str("# TYPE hackspace_mcp_http_total counter\n");
        out.push_str(&format!("hackspace_mcp_http_total{{outcome=\"ok\"}} {}\n", s.http_ok));
        out.push_str(&format!("hackspace_mcp_http_total{{outcome=\"error\"}} {}\n", s.http_err));
        out.push_str("# HELP hackspace_mcp_tool_calls_total Tool invocations, by outcome.\n");
        out.push_str("# TYPE hackspace_mcp_tool_calls_total counter\n");
        for (name, (ok, err)) in &s.tools {
            out.push_str(&format!("hackspace_mcp_tool_calls_total{{tool=\"{name}\",outcome=\"ok\"}} {ok}\n"));
            out.push_str(&format!("hackspace_mcp_tool_calls_total{{tool=\"{name}\",outcome=\"error\"}} {err}\n"));
        }
        out
    }
}

/// Percent-encode a query value.  Only needed for a Prometheus query string,
/// which is full of `{}`, `"` and `=` — characters that are not query-safe and
/// which no upstream will forgive a raw.
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A sensor value the way a person writes it: `15.7`, not `15.700000000000001`,
/// and not `15.0` where the source said `15`.
pub fn value(n: f64) -> String {
    let rounded = (n * 100.0).round() / 100.0;
    if (rounded - rounded.trunc()).abs() < f64::EPSILON {
        format!("{}", rounded as i64)
    } else {
        let s = format!("{rounded}");
        s
    }
}

/// `12.2 °C` from `{value: 12.2, unit: "°C"}` without a stray space when the
/// source already put the unit in the number.
pub fn with_unit(n: f64, unit: &str) -> String {
    let u = unit.trim();
    if u.is_empty() {
        value(n)
    } else if u.starts_with('°') || u.starts_with('%') {
        format!("{}{u}", value(n))
    } else {
        format!("{} {}", value(n), u)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prometheus_query_survives_the_query_string() {
        assert_eq!(
            urlencode(r#"node_hwmon_temp_celsius{chip="platform_coretemp_0"}"#),
            "node_hwmon_temp_celsius%7Bchip%3D%22platform_coretemp_0%22%7D"
        );
        assert_eq!(urlencode("avg by (job) (rate(x[5m]))"), "avg+by+%28job%29+%28rate%28x%5B5m%5D%29%29");
    }

    #[test]
    fn numbers_print_the_way_the_source_meant_them() {
        assert_eq!(value(15.7), "15.7");
        assert_eq!(value(39.0), "39");
        assert_eq!(value(12.0), "12");
        assert_eq!(with_unit(15.7, "°C"), "15.7°C");
        assert_eq!(with_unit(47.0, "%"), "47%");
        assert_eq!(with_unit(1.5, "kWh"), "1.5 kWh");
        assert_eq!(with_unit(3.0, ""), "3");
    }

    #[test]
    fn origins_lose_their_trailing_slash_once() {
        assert_eq!(Sources::trim("http://10.3.1.30:8081/"), "http://10.3.1.30:8081");
        assert_eq!(Sources::trim("http://127.0.0.1:9090"), "http://127.0.0.1:9090");
    }
}
