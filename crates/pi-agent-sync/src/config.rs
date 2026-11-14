//! The endpoint table, as `/etc/pi-agent/endpoints.json` renders it.
//!
//! The file is **generated** (by `common/pi-agent.nix` from `common/pi-endpoints.nix`)
//! and this module is its reader. Two rules shape it:
//!
//! * **Absent means default, wrong means refuse.** Every field except the three that
//!   identify an endpoint may be omitted, because a Nix submodule with defaults will
//!   omit them; but a document that is *malformed* (`baseUrl` without a scheme,
//!   `maxTokensDivisor: 0`) is refused with every finding listed at once. Half a
//!   config applied silently is how a fleet ends up with four boxes pointing at the
//!   wrong engine.
//! * **No secrets.** Nothing here is a credential and nothing here is written into a
//!   secret-bearing place: the file is world-readable by design (`/etc`, `0444`)
//!   because the endpoints it names authenticate by the front door injecting a header
//!   or by the LAN ACL, and `apiKey` is a placeholder whose only job is to make pi
//!   list the model. That is the whole reason `strata_api_key` is *not* in here —
//!   see the header of `common/pi-endpoints.nix`.

use common_json::{parse, Json};
use std::path::Path;

/// How the tool decides a model's `reasoning` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningMode {
    /// Ask the engine: its chat template renders a `<|im_start|>` block, so the
    /// model emits thinking the client is expected to separate.
    Auto,
    Yes,
    No,
}

/// One inference endpoint pi may be pointed at.
#[derive(Debug, Clone)]
pub struct Endpoint {
    /// The pi provider id. Load-bearing: `settings.json`'s `defaultProvider` names it.
    pub id: String,
    /// Display name, used as the provider name and as the suffix of each model's.
    pub name: String,
    pub base_url: String,
    pub api: String,
    /// Placeholder credential (see the module header). Never a real key.
    pub api_key: String,
    /// Where `/props` lives; defaults to the origin of `base_url`.
    pub probe_root: String,
    pub reasoning: ReasoningMode,
    /// Declared capabilities, used when the engine does not report any.
    pub input: Vec<String>,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
    pub max_tokens_divisor: u64,
    pub max_tokens_min: u64,
    pub max_tokens_max: u64,
    /// Pass-through objects, emitted verbatim: these are *verified* facts about the
    /// endpoint's request/response behaviour and no probe can discover them.
    pub compat: Option<Json>,
    pub thinking_level_map: Option<Json>,
    pub headers: Option<Json>,
    pub cost: Json,
    /// Prose for the human reader of the generated config; never emitted to pi.
    pub note: Option<String>,
}

/// A declared positive integer, or the default: absent is a default, present-but-silly
/// is a finding.
fn positive(value: &Json, key: &str, index: usize, findings: &mut Vec<String>, default: u64) -> u64 {
    match value.get(key) {
        None | Some(Json::Null) => default,
        Some(j) => match j.as_u64() {
            Some(n) if n > 0 => n,
            _ => {
                findings.push(format!("endpoints[{index}]: `{key}` must be a positive integer, so the default {default} is used"));
                default
            }
        },
    }
}

impl Endpoint {
    fn from_json(value: &Json, findings: &mut Vec<String>, index: usize) -> Option<Self> {
        let field = |key: &str| -> Option<String> {
            value
                .get(key)
                .and_then(|j| j.as_str())
                .map(|s| s.to_string())
        };

        let id = field("id").unwrap_or_default();
        let base_url = field("baseUrl").unwrap_or_default();
        if id.is_empty() {
            findings.push(format!("endpoints[{index}]: `id` is required"));
        }
        if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
            findings.push(format!(
                "endpoints[{index}] ({id}): `baseUrl` must start with http:// or https://, got `{base_url}`"
            ));
        }

        let reasoning = match field("reasoning").as_deref() {
            None | Some("") | Some("auto") => ReasoningMode::Auto,
            Some("true") | Some("yes") => ReasoningMode::Yes,
            Some("false") | Some("no") => ReasoningMode::No,
            Some(other) => {
                findings.push(format!(
                    "endpoints[{index}] ({id}): `reasoning` must be \"auto\", \"true\" or \"false\", got `{other}`"
                ));
                ReasoningMode::Auto
            }
        };

        let input = value
            .get("input")
            .and_then(|j| j.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|j| j.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .filter(|v: &Vec<String>| !v.is_empty())
            .unwrap_or_else(|| vec!["text".to_string()]);

        let probe_root = field("probeRoot").unwrap_or_else(|| origin(&base_url));

        // Computed before the literal below: each one can record a finding, and three
        // mutable borrows of `findings` inside one struct expression is a borrow-checker
        // argument nobody needs to have.
        let divisor = positive(value, "maxTokensDivisor", index, findings, 8);
        let tokens_min = positive(value, "maxTokensMin", index, findings, 4096);
        let tokens_cap = positive(value, "maxTokensMax", index, findings, 32768);

        Some(Self {
            id,
            name: field("name").unwrap_or_default(),
            base_url,
            api: field("api").unwrap_or_else(|| "openai-completions".to_string()),
            api_key: field("apiKey").unwrap_or_else(|| "pi-agent-sync".to_string()),
            probe_root,
            reasoning,
            input,
            context_window: value.get("contextWindow").and_then(|j| j.as_u64()),
            max_tokens: value.get("maxTokens").and_then(|j| j.as_u64()),
            max_tokens_divisor: divisor,
            max_tokens_min: tokens_min,
            max_tokens_max: tokens_cap,
            compat: value.get("compat").cloned().filter(|j| !matches!(j, Json::Null)),
            thinking_level_map: value
                .get("thinkingLevelMap")
                .cloned()
                .filter(|j| !matches!(j, Json::Null)),
            headers: value
                .get("headers")
                .cloned()
                .filter(|j| !matches!(j, Json::Null)),
            cost: value
                .get("cost")
                .cloned()
                .filter(|j| !matches!(j, Json::Null))
                .unwrap_or_else(zero_cost),
            note: field("note"),
        })
    }
}

/// `{scheme}://{host}[:port]` of an API base URL — where a llama.cpp-style server
/// keeps `/props`, `/slots` and its health endpoints.
pub fn origin(base_url: &str) -> String {
    let scheme_end = match base_url.find("://") {
        Some(i) => i + 3,
        None => return base_url.trim_end_matches('/').to_string(),
    };
    let rest = &base_url[scheme_end..];
    let end = rest.find('/').unwrap_or(rest.len());
    base_url[..scheme_end + end].to_string()
}

/// Self-hosted, so free. Pi requires the four cost fields; zero is the honest answer
/// and the reason a session's cost footer reads `$0.00` rather than being wrong.
pub fn zero_cost() -> Json {
    common_json::object(
        ["input", "output", "cacheRead", "cacheWrite"]
            .into_iter()
            .map(|k| (k.to_string(), Json::Num(0.0))),
    )
}

/// Read the generated config. `Err` carries **every** finding, not the first.
pub fn load(path: &Path) -> Result<Vec<Endpoint>, Vec<String>> {
    let text = std::fs::read_to_string(path).map_err(|e| vec![format!("read {}: {e}", path.display())])?;
    let doc = parse(&text).map_err(|e| vec![format!("parse {}: {e}", path.display())])?;
    let items = match doc.get("endpoints") {
        Some(Json::Arr(items)) => items,
        Some(_) => return Err(vec![format!("{}: `endpoints` must be an array", path.display())]),
        None => return Err(vec![format!("{}: no `endpoints` array", path.display())]),
    };
    let mut findings = Vec::new();
    let mut endpoints = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if let Some(ep) = Endpoint::from_json(item, &mut findings, i) {
            if seen.contains(&ep.id) {
                findings.push(format!("endpoints[{i}]: duplicate id `{}`", ep.id));
            }
            seen.push(ep.id.clone());
            endpoints.push(ep);
        }
    }
    if findings.is_empty() {
        Ok(endpoints)
    } else {
        Err(findings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO: &str = r#"{
      "endpoints": [
        {"id":"strata","name":"Strata (aibox)","baseUrl":"https://strata2.int.leighhack.org/v1",
         "apiKey":"front-door","compat":{"maxTokensField":"max_tokens"},"maxTokensDivisor":8},
        {"id":"llama","baseUrl":"http://10.3.2.213:8080/v1","reasoning":"false",
         "input":["text","image"],"contextWindow":262144,"maxTokens":32768}
      ]
    }"#;

    /// `cargo test` runs tests on threads of one process, so a per-process temp name
    /// is a race between two tests writing the same file — which showed up as a JSON
    /// parse error on a fixture that is valid. Unique per call.
    fn write_tmp(text: &str) -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("pi-agent-sync-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!("endpoints-{}.json", N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        std::fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn absent_fields_take_defaults_and_declared_ones_win() {
        let eps = load(&write_tmp(TWO)).unwrap();
        assert_eq!(eps.len(), 2);
        assert_eq!(eps[0].name, "Strata (aibox)");
        assert_eq!(eps[0].api, "openai-completions");
        assert_eq!(eps[0].max_tokens_divisor, 8);
        assert_eq!(eps[0].max_tokens_min, 4096);
        assert_eq!(eps[0].reasoning, ReasoningMode::Auto);
        assert_eq!(eps[0].probe_root, "https://strata2.int.leighhack.org");
        assert!(eps[0].context_window.is_none());
        assert_eq!(eps[1].reasoning, ReasoningMode::No);
        assert_eq!(eps[1].input, vec!["text", "image"]);
        assert_eq!(eps[1].max_tokens, Some(32768));
        assert_eq!(eps[1].probe_root, "http://10.3.2.213:8080");
        // Absent `apiKey` is still a placeholder, because pi needs *something*.
        assert_eq!(eps[1].api_key, "pi-agent-sync");
    }

    #[test]
    fn every_finding_is_reported_at_once() {
        let bad = r#"{"endpoints":[
            {"name":"no id, no url"},
            {"id":"x","baseUrl":"ftp://nope","reasoning":"sometimes","maxTokensDivisor":0},
            {"id":"x","baseUrl":"http://ok/v1"}
        ]}"#;
        let findings = load(&write_tmp(bad)).unwrap_err();
        let joined = findings.join("\n");
        assert!(joined.contains("`id` is required"), "{joined}");
        assert!(joined.contains("must start with http://"), "{joined}");
        assert!(joined.contains("`reasoning` must be"), "{joined}");
        assert!(joined.contains("maxTokensDivisor"), "{joined}");
        assert!(joined.contains("duplicate id `x`"), "{joined}");
    }

    #[test]
    fn origin_handles_ports_and_bare_hosts() {
        assert_eq!(origin("https://x.int:8443/v1"), "https://x.int:8443");
        assert_eq!(origin("http://10.0.0.1:8080/v1/"), "http://10.0.0.1:8080");
        assert_eq!(origin("https://x.int"), "https://x.int");
    }
}
