//! Building the document pi reads, and merging it into the file that is already there.
//!
//! The file is **not owned by this program**. It is `~/.pi/agent/models.json`, pi's
//! documented config for compatible endpoints, and whoever sits at the keyboard is
//! allowed to have written a provider in it by hand. So the merge rule is narrow and
//! it is the whole design:
//!
//! * provider ids named by the generated config are replaced (that is the point);
//! * every other provider id, and every other top-level key, is copied through
//!   untouched — including `$schema` and anything pi has not invented yet;
//! * an endpoint that could not be probed **keeps whatever was in the file**, because
//!   a stale entry is a model that may work and a deleted entry is a certain
//!   regression — with `services2`'s engine stopped for the afternoon (measured
//!   2026-11-14), this is the difference between a fleet with one dead model and a
//!   fleet with one model.
//!
//! Provenance of every field is decided here rather than in `probe.rs`, because the
//! ladder "engine → previous run → declared → give up" needs to see the old file.

use crate::config::{Endpoint, ReasoningMode};
use crate::probe::Report;
use common_json::{emit, number_from_int, object, string, string_array, Json};
use std::collections::BTreeMap;

/// What a write did, in the words the status file and the journal want.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Provider ids written from a live probe.
    pub written: Vec<String>,
    /// Provider ids kept from the previous file because the probe failed.
    pub kept_stale: Vec<String>,
    /// Provider ids in the file that this config does not manage.
    pub preserved: Vec<String>,
    /// Everything worth reading later: unknown context windows, skipped models.
    pub warnings: Vec<String>,
}

fn obj(pairs: &[(&str, Json)]) -> Json {
    object(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())))
}

/// A model entry from the previous file, if it had one.
fn previous_model(previous: Option<&Json>, id: &str) -> Option<Json> {
    let models = previous?.get("models")?.as_array()?;
    models
        .iter()
        .find(|m| m.get("id").and_then(|j| j.as_str()) == Some(id))
        .cloned()
}

fn num_field(value: Option<&Json>, key: &str) -> Option<u64> {
    value.and_then(|v| v.get(key)).and_then(|j| j.as_u64())
}

/// The output limit, from the context window.
///
/// `n_ctx` is the whole window — prompt *and* completion — so an output limit has to
/// be a slice of it. One eighth reproduces both hand-written values this replaces
/// (131072 → 16384, 262144 → 32768, the two numbers `machines/aibox/pi-models.json`
/// carried), and it is a *policy* rather than a measurement: nothing an engine
/// reports says how much of the window you are allowed to ask for back. Hence
/// `maxTokensDivisor`/`Min`/`Max` in the generated config, so this is changed in Nix
/// and not in Rust.
pub fn max_tokens_for(ctx: u64, divisor: u64, min: u64, max: u64) -> u64 {
    let divisor = divisor.max(1);
    (ctx / divisor).clamp(min, max).min(ctx)
}

fn model_value(
    ep: &Endpoint,
    discovered: &crate::probe::Model,
    previous: Option<&Json>,
    warnings: &mut Vec<String>,
) -> Json {
    let prev = previous_model(previous, &discovered.id);
    let mut pairs: Vec<(String, Json)> = Vec::new();
    pairs.push(("id".into(), string(discovered.id.clone())));
    pairs.push((
        "name".into(),
        string(format!("{} — {}", discovered.id, ep.name)),
    ));

    // --- context window: engine, then last time, then declared, then say so -------
    // The provenance string is used only to decide whether to warn; the per-model
    // source that matters to a reader is the one in the status file, which records
    // what the *engine* said.
    let ctx = match (discovered.context_window, num_field(prev.as_ref(), "contextWindow"), ep.context_window) {
        (Some(c), _, _) => Some(c),
        (None, Some(c), _) => Some(c),
        (None, None, Some(c)) => Some(c),
        (None, None, None) => None,
    };
    if ctx.is_none() {
        warnings.push(format!(
            "{}: no context window from {}, the last file, or the config — pi will assume its own default (128000)",
            discovered.id, ep.id
        ));
    }
    if let Some(c) = ctx {
        pairs.push(("contextWindow".into(), number_from_int(c)));
    }

    // --- output limit -------------------------------------------------------------
    let max_tokens = match (ep.max_tokens, ctx) {
        (Some(m), _) => Some(m),
        (None, Some(c)) => Some(max_tokens_for(c, ep.max_tokens_divisor, ep.max_tokens_min, ep.max_tokens_max)),
        (None, None) => num_field(prev.as_ref(), "maxTokens"),
    };
    if let Some(m) = max_tokens {
        pairs.push(("maxTokens".into(), number_from_int(m)));
    }

    // --- input modalities ---------------------------------------------------------
    let input = if !discovered.input.is_empty() {
        discovered.input.clone()
    } else {
        prev.as_ref()
            .and_then(|p| p.get("input"))
            .and_then(|j| j.as_array())
            .map(|arr| arr.iter().filter_map(|j| j.as_str().map(str::to_string)).collect())
            .filter(|v: &Vec<String>| !v.is_empty())
            .unwrap_or_else(|| ep.input.clone())
    };
    pairs.push(("input".into(), string_array(input)));

    // --- reasoning ----------------------------------------------------------------
    let reasoning = match ep.reasoning {
        ReasoningMode::Yes => Some((true, "declared".to_string())),
        ReasoningMode::No => Some((false, "declared".to_string())),
        ReasoningMode::Auto => discovered
            .reasoning
            .map(|r| (r, discovered.reasoning_source.clone()))
            .or_else(|| {
                prev.as_ref()
                    .and_then(|p| p.get("reasoning"))
                    .and_then(|j| j.as_bool())
                    .map(|r| (r, "previous run".to_string()))
            }),
    };
    match reasoning {
        Some((r, source)) => {
            pairs.push(("reasoning".into(), Json::Bool(r)));
            // `previous run` and `declared` are recorded in the status file rather
            // than warned about: they are the designed fallbacks, and a note that
            // appears on every single run teaches nobody to read the notes.
            let _ = source;
        }
        None => warnings.push(format!(
            "{}: cannot tell whether it thinks (no /props, nothing in the last file, `reasoning: auto`) — omitted, so pi treats it as not reasoning",
            discovered.id
        )),
    }

    pairs.push(("cost".into(), ep.cost.clone()));
    if let Some(map) = &ep.thinking_level_map {
        // Not inheritable: `thinkingLevelMap` is a model field in pi's schema.
        pairs.push(("thinkingLevelMap".into(), map.clone()));
    }

    let mut map = BTreeMap::new();
    for (k, v) in pairs {
        map.insert(k, v);
    }
    Json::Obj(map)
}

/// The same provider as configured, with the model list the previous run discovered.
///
/// This is what a *dead endpoint* becomes, and the split is the point: the model list
/// is what only the endpoint can say, so it is kept; everything else — `baseUrl`,
/// `apiKey`, `compat`, `headers` — is what the flake declares, so it is replaced. The
/// first hand-written copies of this file dialled the engines' box addresses directly
/// and therefore carried the real `strata_api_key` (measured 2026-11-14, four boxes).
/// Refreshing the shape from config is what stops "endpoint is temporarily off" from
/// meaning "the old credential lives here forever".
fn stale_provider(ep: &Endpoint, previous: &Json) -> Json {
    let mut pairs: Vec<(&str, Json)> = vec![
        ("name", string(ep.name.clone())),
        ("baseUrl", string(ep.base_url.clone())),
        ("api", string(ep.api.clone())),
        ("apiKey", string(ep.api_key.clone())),
    ];
    if let Some(h) = &ep.headers {
        pairs.push(("headers", h.clone()));
    }
    if let Some(c) = &ep.compat {
        pairs.push(("compat", c.clone()));
    }
    let models = previous
        .get("models")
        .and_then(|j| j.as_array())
        .map(|a| a.to_vec())
        .unwrap_or_default();
    pairs.push(("models", Json::Arr(models)));
    obj(&pairs)
}

/// One provider entry: the endpoint as configured plus the models as discovered.
pub fn provider_value(ep: &Endpoint, report: &Report, previous: Option<&Json>, warnings: &mut Vec<String>) -> Json {
    let mut pairs: Vec<(&str, Json)> = vec![
        ("name", string(ep.name.clone())),
        ("baseUrl", string(ep.base_url.clone())),
        ("api", string(ep.api.clone())),
        ("apiKey", string(ep.api_key.clone())),
    ];
    if let Some(h) = &ep.headers {
        pairs.push(("headers", h.clone()));
    }
    if let Some(c) = &ep.compat {
        // Provider-level: pi merges model compat over provider compat per field, so
        // one copy serves every model the endpoint loads next week.
        pairs.push(("compat", c.clone()));
    }
    let models: Vec<Json> = report
        .models
        .iter()
        .map(|m| model_value(ep, m, previous, warnings))
        .collect();
    pairs.push(("models", Json::Arr(models)));
    obj(&pairs)
}

/// Replace every credential-looking value with a marker, for printing.
///
/// `--dry-run` prints the document it was going to write, and that document can
/// contain a provider this program merely *preserved* — including, until 2026-11-14,
/// the four hand-written copies of this file, which carried the real `strata_api_key`
/// because they dialled the engines' addresses directly instead of the front door.
/// AGENTS.md rule 13 is not satisfied by "the file is 0600" once the same bytes go to
/// a terminal, and a terminal is where a journal or a transcript starts.
pub fn redact(text: &str) -> String {
    match common_json::parse(text) {
        Err(_) => "<unreadable JSON>".to_string(),
        Ok(doc) => emit(&redact_value(&doc), 2),
    }
}

fn redact_value(value: &Json) -> Json {
    match value {
        Json::Obj(map) => {
            let mut out = BTreeMap::new();
            for (k, v) in map {
                // Two names carry a credential in pi's schema, at provider level and at
                // model level alike, so the test is on the key and not on the depth.
                let secret_key = (k == "apiKey" && matches!(v, Json::Str(_)))
                    || (k == "headers" && matches!(v, Json::Obj(_)));
                out.insert(k.clone(), if secret_key { redacted_shape(v) } else { redact_value(v) });
            }
            Json::Obj(out)
        }
        Json::Arr(items) => Json::Arr(items.iter().map(redact_value).collect()),
        other => other.clone(),
    }
}

/// Redact without flattening: `"apiKey": "«redacted»"`, and an object of headers
/// keeps its keys so the shape is still reviewable.
fn redacted_shape(value: &Json) -> Json {
    match value {
        Json::Obj(map) => Json::Obj(
            map.keys()
                .map(|k| (k.clone(), string("«redacted»")))
                .collect(),
        ),
        _ => string("«redacted»"),
    }
}

fn provider_map(doc: &Json) -> BTreeMap<String, Json> {
    match doc.get("providers") {
        Some(Json::Obj(map)) => map.clone(),
        _ => BTreeMap::new(),
    }
}

/// Rebuild `models.json`. `existing` is the current file's text, if there is one.
pub fn merge(existing: &str, endpoints: &[Endpoint], reports: &[Report]) -> (String, Outcome) {
    let mut outcome = Outcome::default();
    let existing_doc = if existing.trim().is_empty() {
        Json::Obj(BTreeMap::new())
    } else {
        match common_json::parse(existing) {
            Ok(d) => d,
            Err(e) => {
                // Unparseable, so there is nothing to preserve and no way to know
                // what was in it. Say it in the output rather than dying quietly, and
                // let the caller's atomic write keep the file until it can be fixed.
                outcome.warnings.push(format!(
                    "the existing models.json could not be parsed ({e}); its hand-written providers cannot be preserved"
                ));
                Json::Obj(BTreeMap::new())
            }
        }
    };

    let mut providers = provider_map(&existing_doc);
    let managed: Vec<&String> = endpoints.iter().map(|e| &e.id).collect();
    for id in providers.keys() {
        if !managed.contains(&id) {
            outcome.preserved.push(id.clone());
        }
    }

    for ep in endpoints {
        let report = reports.iter().find(|r| r.endpoint_id == ep.id);
        let previous = providers.get(&ep.id).cloned();
        match report {
            Some(r) if r.usable() => {
                let mut warnings = Vec::new();
                let value = provider_value(ep, r, previous.as_ref(), &mut warnings);
                outcome.warnings.append(&mut warnings);
                providers.insert(ep.id.clone(), value);
                outcome.written.push(ep.id.clone());
            }
            // Dead endpoint: keep the previous *model list* under the *current*
            // config. `previous` is None only when this endpoint has never been
            // written, in which case there is nothing to keep and the status file
            // carries the reason.
            _ => {
                let previous_models = previous
                    .as_ref()
                    .and_then(|p| p.get("models"))
                    .and_then(|j| j.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let reason = report.and_then(|r| r.error.clone()).unwrap_or_else(|| "not probed".into());
                if previous_models > 0 {
                    if let Some(prev) = previous.as_ref() {
                        providers.insert(ep.id.clone(), stale_provider(ep, prev));
                        outcome.kept_stale.push(ep.id.clone());
                        outcome.warnings.push(format!(
                            "{}: keeping the previous model list ({} model(s)) — {reason}",
                            ep.id, previous_models
                        ));
                    }
                } else {
                    outcome.warnings.push(format!("{}: written nothing — {reason}", ep.id));
                }
            }
        }
    }

    // Keep every top-level key the file already had, then the providers we own.
    let mut top: Vec<(String, Json)> = Vec::new();
    if let Json::Obj(map) = &existing_doc {
        for (k, v) in map {
            if k != "providers" {
                top.push((k.clone(), v.clone()));
            }
        }
    }
    top.push((
        "providers".into(),
        Json::Obj(providers.into_iter().collect()),
    ));
    (emit(&object(top), 2) + "\n", outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load;
    use crate::probe::Model;

    #[test]
    fn a_dead_endpoint_keeps_its_models_but_not_its_credential() {
        // The shape of the pre-flake file: the *engine's* address, so the file has to
        // carry the engine's Bearer key. A dead endpoint must not preserve that.
        let existing = r#"{"providers":{"strata":{"name":"Old","baseUrl":"http://10.3.1.21:8080/v1",
          "apiKey":"a-real-secret","models":[{"id":"m","contextWindow":131072}]}}}"#;
        let dead = Report { error: Some("502".into()), models_http: "502".into(), ..report("strata", vec![]) };
        let (text, outcome) = merge(existing, std::slice::from_ref(&ep("strata")), &[dead]);
        assert!(!text.contains("a-real-secret"), "the secret must be gone:\n{text}");
        let doc = common_json::parse(&text).unwrap();
        let p = doc.get("providers").unwrap().get("strata").unwrap();
        assert_eq!(p.get("baseUrl").unwrap().as_str(), Some("http://x/v1"), "current config, not the old address");
        assert_eq!(p.get("apiKey").unwrap().as_str(), Some("door"), "the placeholder, not the old key");
        assert_eq!(
            p.get("models").unwrap().as_array().unwrap()[0].get("contextWindow").unwrap().as_u64(),
            Some(131072),
            "the model list is the only thing worth keeping"
        );
        assert_eq!(outcome.kept_stale, vec!["strata".to_string()]);
    }

    #[test]
    fn printing_a_document_never_prints_a_credential() {
        let (text, _) = merge(
            r#"{"providers":{"hand":{"baseUrl":"http://h/v1","apiKey":"s3cret","models":[],
               "headers":{"X-Key":"tk-999"}}}}"#,
            &[],
            &[],
        );
        let shown = redact(&text);
        assert!(!shown.contains("s3cret") && !shown.contains("tk-999"), "{shown}");
        assert!(shown.contains("\"apiKey\": \"«redacted»\""), "{shown}");
        assert!(shown.contains("X-Key"), "the shape stays reviewable: {shown}");
        assert!(redact("not json").contains("unreadable"));
    }

    fn ep(id: &str) -> Endpoint {
        let json = format!(
            r#"{{"endpoints":[{{"id":"{id}","name":"Test {id}","baseUrl":"http://x/v1",
               "apiKey":"door","compat":{{"maxTokensField":"max_tokens"}},
               "thinkingLevelMap":{{"off":"off","high":"high"}}}}]}}"#
        );
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("pi-agent-sync-render-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!("{id}-{}.json", N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        std::fs::write(&p, json).unwrap();
        load(&p).unwrap().remove(0)
    }

    fn report(id: &str, models: Vec<Model>) -> Report {
        Report {
            endpoint_id: id.into(),
            models_url: "http://x/v1/models".into(),
            props_url: "http://x/props".into(),
            models_http: "200".into(),
            props_http: "200".into(),
            models,
            template_thinks: Some(true),
            error: None,
            elapsed_ms: 1,
        }
    }

    fn model(id: &str, ctx: Option<u64>) -> Model {
        Model {
            id: id.into(),
            context_window: ctx,
            ctx_source: "v1/models meta.n_ctx".into(),
            input: vec!["text".into()],
            input_source: "v1/models architecture.input_modalities".into(),
            reasoning: Some(true),
            reasoning_source: "v1/models".into(),
            loaded: Some(true),
        }
    }

    #[test]
    fn the_output_limit_is_an_eighth_of_the_window_and_reproduces_the_hand_written_numbers() {
        // The two values `machines/aibox/pi-models.json` carried, as measured.
        assert_eq!(max_tokens_for(131072, 8, 4096, 32768), 16384);
        assert_eq!(max_tokens_for(262144, 8, 4096, 32768), 32768);
        // Clamped at both ends, and never larger than the window it comes from.
        assert_eq!(max_tokens_for(393216, 8, 4096, 32768), 32768);
        assert_eq!(max_tokens_for(32768, 8, 4096, 32768), 4096);
        assert_eq!(max_tokens_for(2048, 8, 4096, 32768), 2048);
    }

    #[test]
    fn writes_discovered_ids_and_context_and_carries_declared_policy() {
        let (text, outcome) = merge(
            "",
            std::slice::from_ref(&ep("strata")),
            &[report("strata", vec![model("qwen3.8-flash-next-iq2xs", Some(262144))])],
        );
        assert_eq!(outcome.written, vec!["strata".to_string()]);
        let doc = common_json::parse(&text).unwrap();
        let p = doc.get("providers").unwrap().get("strata").unwrap();
        assert_eq!(p.get("baseUrl").unwrap().as_str(), Some("http://x/v1"));
        assert_eq!(p.at(&["compat", "maxTokensField"]).unwrap().as_str(), Some("max_tokens"));
        let m = p.get("models").unwrap().as_array().unwrap()[0].clone();
        assert_eq!(m.get("id").unwrap().as_str(), Some("qwen3.8-flash-next-iq2xs"));
        assert_eq!(m.get("contextWindow").unwrap().as_u64(), Some(262144));
        assert_eq!(m.get("maxTokens").unwrap().as_u64(), Some(32768));
        assert_eq!(m.get("reasoning").unwrap().as_bool(), Some(true));
        assert_eq!(m.at(&["thinkingLevelMap", "high"]).unwrap().as_str(), Some("high"));
        assert_eq!(m.get("cost").unwrap().get("input").unwrap().as_u64(), Some(0));
        assert!(text.contains("\"id\": \"qwen3.8-flash-next-iq2xs\""), "{text}");
    }

    #[test]
    fn a_hand_written_provider_survives_and_is_named() {
        let existing = r#"{"$schema":"https://pi.dev/schemas/models.schema.json",
          "providers":{"hand":{"baseUrl":"http://other/v1","api":"openai-completions","apiKey":"k",
            "models":[{"id":"m","contextWindow":9999}]}}}"#;
        let (text, outcome) = merge(
            existing,
            std::slice::from_ref(&ep("strata")),
            &[report("strata", vec![model("a", Some(131072))])],
        );
        assert_eq!(outcome.preserved, vec!["hand".to_string()]);
        let doc = common_json::parse(&text).unwrap();
        assert_eq!(
            doc.at(&["providers", "hand", "models"]).unwrap().as_array().unwrap()[0]
                .get("contextWindow").unwrap().as_u64(),
            Some(9999),
            "the hand-written entry must be byte-identical in meaning"
        );
        assert_eq!(doc.get("$schema").unwrap().as_str(), Some("https://pi.dev/schemas/models.schema.json"));
    }

    #[test]
    fn a_failed_endpoint_keeps_its_previous_models_and_says_so() {
        let existing = r#"{"providers":{"strata":{"name":"Old","baseUrl":"http://old/v1",
          "models":[{"id":"qwen-coder","contextWindow":131072,"maxTokens":16384,"reasoning":true,"input":["text"]}]}}}"#;
        let dead = Report {
            error: Some("connect refused".into()),
            models_http: "000".into(),
            ..report("strata", vec![])
        };
        let (text, outcome) = merge(existing, std::slice::from_ref(&ep("strata")), &[dead]);
        assert!(outcome.kept_stale == vec!["strata".to_string()], "{outcome:?}");
        assert!(outcome.written.is_empty());
        let doc = common_json::parse(&text).unwrap();
        assert_eq!(
            doc.at(&["providers", "strata", "models"]).unwrap().as_array().unwrap()[0]
                .get("id").unwrap().as_str(),
            Some("qwen-coder")
        );
        assert!(outcome.warnings.iter().any(|w| w.contains("keeping the previous model list")), "{:?}", outcome.warnings);
    }

    #[test]
    fn a_stale_context_window_is_sticky_and_an_unknown_one_is_a_warning() {
        // Engine up, but says nothing about context: keep last time's number.
        let existing = r#"{"providers":{"strata":{"models":[{"id":"a","contextWindow":131072,"maxTokens":16384}]}}}"#;
        let (text, outcome) = merge(
            existing,
            std::slice::from_ref(&ep("strata")),
            &[report("strata", vec![model("a", None)])],
        );
        let doc = common_json::parse(&text).unwrap();
        let m = &doc.at(&["providers", "strata", "models"]).unwrap().as_array().unwrap()[0];
        assert_eq!(m.get("contextWindow").unwrap().as_u64(), Some(131072));
        assert_eq!(m.get("maxTokens").unwrap().as_u64(), Some(16384), "also sticky: not recomputed from nothing");
        assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);

        // Brand new provider, no context anywhere: loud.
        let (text, outcome) = merge("", std::slice::from_ref(&ep("strata")), &[report("strata", vec![model("a", None)])]);
        assert!(outcome
            .warnings
            .iter()
            .any(|w| w.contains("no context window") && w.contains("128000")), "{:?}", outcome.warnings);
        let doc = common_json::parse(&text).unwrap();
        let m = &doc.at(&["providers", "strata", "models"]).unwrap().as_array().unwrap()[0];
        assert!(m.get("contextWindow").is_none(), "absent, not invented: {m:?}");
    }

    #[test]
    fn a_reasoning_model_whose_props_are_unavailable_stays_reasoning() {
        let existing = r#"{"providers":{"strata":{"models":[{"id":"a","reasoning":true}]}}}"#;
        let mut m = model("a", Some(131072));
        m.reasoning = None;
        m.reasoning_source = "none".into();
        let r = Report { template_thinks: None, ..report("strata", vec![m]) };
        let (text, outcome) = merge(existing, std::slice::from_ref(&ep("strata")), &[r]);
        let doc = common_json::parse(&text).unwrap();
        let m = &doc.at(&["providers", "strata", "models"]).unwrap().as_array().unwrap()[0];
        assert_eq!(m.get("reasoning").unwrap().as_bool(), Some(true), "a lost /props must not turn thinking off");
        // Quietly sticky, by design: the status file records `propsSaysThinking: null`
        // and the model's `reasoningSource`, which is where a reader looks.
        assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    }

    #[test]
    fn rerunning_on_an_unchanged_endpoint_produces_the_same_bytes() {
        let eps = vec![ep("strata")];
        let reps = vec![report("strata", vec![model("b", Some(262144)), model("a", Some(262144))])];
        let (once, _) = merge("", &eps, &reps);
        let (twice, _) = merge(&once, &eps, &reps);
        assert_eq!(once, twice, "a file rewritten on a timer must not churn");
    }
}
