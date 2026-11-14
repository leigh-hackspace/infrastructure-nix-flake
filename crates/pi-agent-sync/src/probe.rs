//! Asking an endpoint what it is running, and reading the answer.
//!
//! Two requests per endpoint, both plain GETs:
//!
//! * `<baseUrl>/models` — the OpenAI model list. Every engine here answers it, in two
//!   dialects that are both real on this network (measured 2026-11-14, and both are
//!   test fixtures under `tests/fixtures/`):
//!   - **Strata** (aibox, services2) answers with `data[]` carrying `meta.n_ctx`,
//!     `status.value` and `architecture.input_modalities` — the context window, which
//!     is the number this program exists to stop hardcoding;
//!   - **llama.cpp** (services3) answers with `data[]` carrying `architecture` and an
//!     **`models[]` Ollama-compatibility list** carrying `capabilities`. Same data,
//!     different key, so a reader that only looks at one of them sees nothing.
//! * `<origin>/props` — not part of the OpenAI API, but both engines serve it and it
//!   is the only place the *loaded* model's context window, modality table and chat
//!   template live. It is the fallback for everything `/models` does not carry, and
//!   `modalities.vision` is a better answer than a name that happens to contain
//!   "vision".
//!
//! Nothing here is a secret and nothing is logged beyond URLs, status codes and model
//! ids: the request path is a GET with no body, so a transcript of this program's
//! output cannot leak a key (see the header of `common/pi-endpoints.nix` for who does
//! authenticate, and why it is not this program).

use crate::config::Endpoint;
use common_json::parse;
use std::process::Command;
use std::time::Instant;

/// One model an endpoint reported, with **where each field came from** attached.
///
/// The provenance is not decoration. Every field has three possible sources — the
/// engine said so, the config declared it, or the previous run said it — and when pi
/// does something surprising at 3 a.m. the question is which of those a value was.
/// It is also the only way to tell "this engine has no context to report" from "this
/// engine is down and the number on screen is a week old".
#[derive(Debug, Clone)]
pub struct Model {
    pub id: String,
    pub context_window: Option<u64>,
    pub ctx_source: String,
    pub input: Vec<String>,
    pub input_source: String,
    /// `None` = unknown; the caller decides between the declared default and the
    /// previous run's answer.
    pub reasoning: Option<bool>,
    pub reasoning_source: String,
    pub loaded: Option<bool>,
}

/// What one endpoint said, and how it failed when it did not.
#[derive(Debug, Clone)]
pub struct Report {
    pub endpoint_id: String,
    pub models_url: String,
    pub props_url: String,
    pub models_http: String,
    pub props_http: String,
    pub models: Vec<Model>,
    /// `/props` said the model emits a thinking block (its chat template contains
    /// `<think>`). `None` means `/props` was unavailable or had no template.
    pub template_thinks: Option<bool>,
    pub error: Option<String>,
    pub elapsed_ms: u128,
}

impl Report {
    /// A usable answer is "at least one model". A 200 with an empty list is a real
    /// HTTP success and a useless result, and treating it as success is how an
    /// engine that has not finished loading deletes every model from every box.
    pub fn usable(&self) -> bool {
        !self.models.is_empty()
    }
}

struct Fetched {
    body: String,
    code: String,
    /// `None` only when the request succeeded: the caller never asks "was it ok" and
    /// then separately asks what went wrong, because forgetting the second half is how
    /// a failed probe becomes an empty answer.
    error: Option<String>,
}

fn fetch(curl: &str, url: &str, timeout_s: u64) -> Fetched {
    let out = Command::new(curl)
        .args([
            "-sS",
            "-m",
            &timeout_s.to_string(),
            // The status code goes on the end of stdout rather than being asked for
            // separately: `--fail` would throw the body away, and the body of a 502
            // from the front door is the one thing that says *which* upstream died.
            "-w",
            "\n%{http_code}",
            url,
        ])
        .output();
    match out {
        Err(e) => Fetched {
            body: String::new(),
            code: "000".into(),
            error: Some(format!("{curl} could not start: {e}")),
        },
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout).to_string();
            let (body, code) = match stdout.rsplit_once('\n') {
                Some((b, c)) => (b.to_string(), c.trim().to_string()),
                None => (String::new(), stdout.trim().to_string()),
            };
            let status = o.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            // curl exits 22 on `--fail` only, which is not passed; a 4xx/5xx still
            // exits 0 and carries its body. So "ok" is the code, not the exit status,
            // and the exit status only decides whether stderr is worth showing.
            let numeric: u16 = code.parse().unwrap_or(0);
            let code_text = code.clone();
            let ok = status == 0 && (200..300).contains(&numeric);
            Fetched {
                body,
                code: code_text,
                error: if ok {
                    None
                } else if !stderr.is_empty() {
                    Some(stderr)
                } else if status != 0 {
                    Some(format!("curl exited {status}"))
                } else {
                    Some(format!("HTTP {code}"))
                },
            }
        }
    }
}

/// Probe one endpoint. Never fails: the caller decides what a failure means, and a
/// box with one dead engine still has to have the others written.
pub fn probe(ep: &Endpoint, curl: &str, timeout_s: u64) -> Report {
    let started = Instant::now();
    let base = ep.base_url.trim_end_matches('/');
    let models_url = format!("{base}/models");
    let props_url = format!("{}/props", ep.probe_root.trim_end_matches('/'));

    let models = fetch(curl, &models_url, timeout_s);
    let props = fetch(curl, &props_url, timeout_s);

    let mut found = match &models.error {
        Some(_) => Vec::new(),
        None => models_from(&models.body),
    };
    let props_info = match &props.error {
        Some(_) => None,
        None => Some(props_from(&props.body)),
    };

    // `/props` is about the *loaded* model, so its context window is only a fallback
    // for a model `/models` did not give one — never an override. (It is also the
    // whole answer for a server whose /models has no meta at all, which is what an
    // older llama.cpp is.)
    if let Some(info) = &props_info {
        if let Some(n_ctx) = info.n_ctx {
            for m in found.iter_mut() {
                if m.context_window.is_none() {
                    m.context_window = Some(n_ctx);
                    m.ctx_source = "/props default_generation_settings.n_ctx".into();
                }
            }
        }
        if let Some(vision) = info.vision {
            for m in found.iter_mut() {
                if m.input_source == "none" {
                    m.input = if vision {
                        vec!["text".into(), "image".into()]
                    } else {
                        vec!["text".into()]
                    };
                    m.input_source = "/props modalities.vision".into();
                }
            }
        }
    }

    found.sort_by(|a, b| a.id.cmp(&b.id));
    found.iter_mut().for_each(|m| {
        if m.reasoning.is_none() {
            if let Some(thinks) = props_info.as_ref().and_then(|i| i.template_thinks) {
                m.reasoning = Some(thinks);
                m.reasoning_source = "/props chat_template <think>".into();
            }
        }
    });

    let error = match (&models.error, found.is_empty()) {
        (Some(e), _) => Some(format!("GET {models_url}: {e}")),
        // The shape that must not be mistaken for "this endpoint has no models": a
        // 200 with an empty list, usually an engine that has not finished loading. The
        // `/props` status is quoted because it is the one hint at whether the process
        // is up at all.
        (None, true) => Some(format!(
            "GET {models_url} returned HTTP {} with no models (/props answered {}; a loading engine, not an empty one — keeping the previous list)",
            models.code, props.code
        )),
        (None, false) => None,
    };

    Report {
        endpoint_id: ep.id.clone(),
        models_url,
        props_url,
        models_http: models.code,
        props_http: props.code,
        models: found,
        template_thinks: props_info.and_then(|i| i.template_thinks),
        error,
        elapsed_ms: started.elapsed().as_millis(),
    }
}

struct Props {
    n_ctx: Option<u64>,
    vision: Option<bool>,
    template_thinks: Option<bool>,
}

fn props_from(body: &str) -> Props {
    let doc = match parse(body) {
        Ok(d) => d,
        Err(_) => return Props { n_ctx: None, vision: None, template_thinks: None },
    };
    // `chat_template` is the largest string on this network that nobody reads: the
    // full Jinja of the loaded model, tens of KB. It is the only reliable statement
    // of whether the model is expected to emit a thinking block, so it is read here
    // as a substring and never echoed.
    Props {
        n_ctx: doc.at(&["default_generation_settings", "n_ctx"]).and_then(|j| j.as_u64()),
        vision: doc.get("modalities").and_then(|m| m.get("vision")).and_then(|j| j.as_bool()),
        template_thinks: doc
            .get("chat_template")
            .and_then(|j| j.as_str())
            .map(|t| t.contains("<think>")),
    }
}

/// Both dialects, one pass: `data[]` if it is there (OpenAI), then `models[]` (the
/// Ollama compatibility list llama.cpp also serves).
fn models_from(body: &str) -> Vec<Model> {
    let doc = match parse(body) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();

    if let Some(items) = doc.get("data").and_then(|j| j.as_array()) {
        for item in items {
            let Some(id) = item.get("id").and_then(|j| j.as_str()) else { continue };
            let ctx = item.at(&["meta", "n_ctx"]).and_then(|j| j.as_u64());
            let modalities = item
                .at(&["architecture", "input_modalities"])
                .and_then(|j| j.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|j| j.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                });
            let (input, input_source) = match modalities {
                // pi's `input` is text|image only: `video` is real on services3's
                // Ternary and pi has no field to declare it, so it is dropped rather
                // than translated into an image claim that is not the same thing.
                Some(m) => {
                    let mut v = vec!["text".to_string()];
                    if m.iter().any(|x| x == "image") {
                        v.push("image".into());
                    }
                    (v, "v1/models architecture.input_modalities".to_string())
                }
                None => (Vec::new(), "none".to_string()),
            };
            out.push(Model {
                id: id.to_string(),
                context_window: ctx,
                ctx_source: if ctx.is_some() { "v1/models meta.n_ctx".into() } else { "none".into() },
                input,
                input_source,
                reasoning: None,
                reasoning_source: "none".into(),
                loaded: item
                    .at(&["status", "value"])
                    .and_then(|j| j.as_str())
                    .map(|s| s == "loaded"),
            });
        }
    }

    if out.is_empty() {
        if let Some(items) = doc.get("models").and_then(|j| j.as_array()) {
            for item in items {
                let Some(id) = item.get("name").and_then(|j| j.as_str()) else { continue };
                let caps = item
                    .get("capabilities")
                    .and_then(|j| j.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|j| j.as_str().map(str::to_string))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let mut input = vec!["text".to_string()];
                if caps.iter().any(|c| c == "multimodal" || c == "vision") {
                    input.push("image".into());
                }
                out.push(Model {
                    id: id.to_string(),
                    context_window: None,
                    ctx_source: "none".into(),
                    input_source: if caps.is_empty() { "none".into() } else { "v1/models capabilities".into() },
                    input,
                    reasoning: None,
                    reasoning_source: "none".into(),
                    loaded: None,
                });
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_stratas_meta_context_and_modalities() {
        // The real body, from strata2.int.leighhack.org/v1/models on 2026-11-14.
        let body = r#"{"object":"list","data":[{"id":"qwen3.8-flash-next-iq2xs","object":"model",
            "status":{"value":"loaded"},"meta":{"n_ctx":262144},
            "architecture":{"input_modalities":["text"],"output_modalities":["text"]}}]}"#;
        let m = models_from(body);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].id, "qwen3.8-flash-next-iq2xs");
        assert_eq!(m[0].context_window, Some(262144));
        assert_eq!(m[0].input, vec!["text".to_string()]);
        assert_eq!(m[0].loaded, Some(true));
    }

    #[test]
    fn reads_llamas_data_list_and_drops_video_as_unrepresentable() {
        let body = r#"{"object":"list","data":[{"id":"Ternary-Bonsai-2-9B-Q2_0",
            "architecture":{"input_modalities":["text","image","video"],"output_modalities":["text"]}}]}"#;
        let m = models_from(body);
        assert_eq!(m[0].input, vec!["text".to_string(), "image".to_string()]);
        assert_eq!(m[0].context_window, None, "llama.cpp puts n_ctx in /props, not here");
        assert_eq!(m[0].loaded, None);
    }

    #[test]
    fn falls_back_to_the_ollama_list_and_its_capabilities() {
        let body = r#"{"models":[{"name":"Ternary-Bonsai-2-9B-Q2_0","capabilities":["completion","multimodal"]}],
            "object":"list","data":[]}"#;
        let m = models_from(body);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].input, vec!["text".to_string(), "image".to_string()]);
        assert_eq!(m[0].input_source, "v1/models capabilities");
    }

    #[test]
    fn props_gives_context_vision_and_the_thinking_block() {
        let body = r#"{"model_alias":"a","modalities":{"vision":true,"audio":false},
            "default_generation_settings":{"n_ctx":131072,"params":{"reasoning_format":"none"}},
            "chat_template":"{{ '<|im_start|>assistant\n<think>\n' }}" }"#;
        let p = props_from(body);
        assert_eq!(p.n_ctx, Some(131072));
        assert_eq!(p.vision, Some(true));
        assert_eq!(p.template_thinks, Some(true), "reasoning_format is a request-parsing knob, not a capability");
    }

    #[test]
    fn a_template_without_a_think_block_is_not_reasoning() {
        let p = props_from(r#"{"chat_template":"{{- '<|im_start|>' + message.role -}}" }"#);
        assert_eq!(p.template_thinks, Some(false));
        assert_eq!(p.n_ctx, None);
        assert_eq!(p.vision, None);
    }

    /// The four bodies below are captured verbatim from the live engines on 2026-11-14
    /// (`strata2.int.leighhack.org` and `llama.int.leighhack.org`, both through the
    /// front door), with one exception stated in the fixture itself: `/props`'
    /// `chat_template` is ~40 KB of Jinja and only its tail — the part that emits the
    /// thinking block — is kept. They are the reason the two dialects below are a test
    /// and not a guess, and the reason a future engine update that renames `meta.n_ctx`
    /// is caught here rather than in four people's `models.json`.
    fn fixture(name: &str) -> String {
        let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: fixture missing ({e}) — it is a tracked file, see tests/fixtures/"))
    }

    #[test]
    fn the_captured_live_bodies_produce_the_measured_answers() {
        let strata = models_from(&fixture("strata2-v1-models.json"));
        assert_eq!(strata.len(), 1);
        assert_eq!(strata[0].id, "qwen3.8-flash-next-iq2xs");
        assert_eq!(strata[0].context_window, Some(262144), "the number the checked-in config said was 131072");
        assert_eq!(strata[0].input, vec!["text".to_string()]);
        let sp = props_from(&fixture("strata2-props.json"));
        assert_eq!(sp.n_ctx, Some(262144));
        assert_eq!(sp.vision, Some(false));
        assert_eq!(sp.template_thinks, Some(true));

        let llama = models_from(&fixture("llama-services3-v1-models.json"));
        assert_eq!(llama.len(), 1, "the data list is used, and the Ollama `models` list is not double-counted");
        assert_eq!(llama[0].id, "Ternary-Bonsai-2-9B-Q2_0");
        assert_eq!(llama[0].input, vec!["text".to_string(), "image".to_string()]);
        // Measured: llama.cpp on services3 *does* carry meta.n_ctx; the /props path
        // below is the fallback for one that does not, not the other engine's quirk.
        assert_eq!(llama[0].context_window, Some(262144));
        let lp = props_from(&fixture("llama-services3-props.json"));
        assert_eq!(lp.n_ctx, Some(262144));
        assert_eq!(lp.vision, Some(true));
        assert_eq!(lp.template_thinks, Some(true));
    }

    #[test]
    fn a_front_door_502_is_an_empty_answer_with_its_body_unread() {
        // services2's engine was stopped when this was captured; the door's HTML page
        // is what arrives, and the OpenAI parse must find nothing rather than panic.
        let body = fixture("front-door-502-body.txt");
        assert!(models_from(&body).is_empty());
        let p = props_from(&body);
        assert_eq!(p.n_ctx, None);
        assert_eq!(p.template_thinks, None);
    }

    #[test]
    fn unparseable_json_is_an_empty_answer_not_a_panic() {
        assert!(models_from("<html>502 Bad Gateway</html>").is_empty());
        let p = props_from("not json");
        assert_eq!(p.n_ctx, None);
        assert_eq!(p.template_thinks, None);
    }

    #[test]
    fn an_empty_list_is_not_a_usable_answer() {
        let r = Report {
            endpoint_id: "x".into(),
            models_url: String::new(),
            props_url: String::new(),
            models_http: "200".into(),
            props_http: "200".into(),
            models: Vec::new(),
            template_thinks: None,
            error: None,
            elapsed_ms: 0,
        };
        assert!(!r.usable(), "a 200 with no models must not erase the previous answer");
    }
}
