//! `pi-agent-sync` — write pi's `models.json` from the endpoints this flake declares,
//! asking each one what it is actually running.
//!
//! Why the file cannot be generated at build time, and so why this program exists, is
//! in `docs/pi-agent-models.md`. The short version: the two numbers that matter most
//! — which models an endpoint currently serves, and how big a context it serves them
//! with — are properties of a running engine, and every hand-written copy of them in
//! this repo had already gone stale (aibox reports `n_ctx` 262144 against the 131072
//! its checked-in config claimed).
//!
//! Two modes:
//!
//! * **sync** (the default, what the timer runs): probe every endpoint in
//!   `--config`, merge the answers into `--out`, write the run's provenance to
//!   `--status`. Exits 0 unless the *config* or the *filesystem* is the problem —
//!   a dead engine is a result, not a crash, and this runs as a timer.
//! * **report** (`--report <status>`): print what the last run saw, for a human or
//!   for `just pi-agent-status`. This is the half that exists because a program that
//!   succeeds silently is indistinguishable from one that did nothing.
//!
//! ```text
//! pi-agent-sync [--config P] --out P [--status P] [--settings P]
//!               [--curl PATH] [--timeout SECS] [--dry-run] [--create-missing] [--check]
//! pi-agent-sync --report P [--max-age SECS]
//! ```

mod config;
mod probe;
mod render;

use common_json::{emit, number_from_int, object, string, string_array, Json};
use common_service::{arg, arg_opt};
use probe::Report;
use std::io::Write;
use std::path::{Path, PathBuf};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Whole seconds since the epoch. Deliberately not a formatted timestamp: formatting
/// one needs either a date library or a `date` subprocess, and a status file a
/// machine reads does not need one. `--report` turns it into an age.
fn epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn usage() -> ! {
    eprintln!(
        "pi-agent-sync {VERSION} — write ~/.pi/agent/models.json from discovered endpoints\n\
         \n  sync:   pi-agent-sync --config /etc/pi-agent/endpoints.json --out ~/.pi/agent/models.json\n\
         \n  report: pi-agent-sync --report ~/.pi/agent/pi-agent-status.json\n\
         \nSee the header of src/main.rs and docs/pi-agent-models.md."
    );
    std::process::exit(2);
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--help" || a == "-h") {
        usage();
    }

    let timeout: u64 = arg(&argv, "--timeout", Some("20"))
        .parse()
        .unwrap_or_else(|_| {
            eprintln!("--timeout must be a number of seconds");
            std::process::exit(2);
        });
    let curl = arg(&argv, "--curl", Some("curl"));

    if let Some(status_path) = arg_opt(&argv, "--report") {
        std::process::exit(report(&PathBuf::from(status_path)));
    }

    let cfg_path = PathBuf::from(arg(&argv, "--config", Some("/etc/pi-agent/endpoints.json")));
    let out_path = match arg_opt(&argv, "--out") {
        Some(p) => PathBuf::from(p),
        None => usage(),
    };
    let status_path = arg_opt(&argv, "--status").map(PathBuf::from);
    let settings_path = arg_opt(&argv, "--settings").map(PathBuf::from);
    let dry_run = argv.iter().any(|a| a == "--dry-run");
    let create_missing = argv.iter().any(|a| a == "--create-missing");
    let strict = argv.iter().any(|a| a == "--check");

    // A bad config is the one failure that must not be half-applied: refuse before
    // touching anything, and list every finding.
    let endpoints = match config::load(&cfg_path) {
        Ok(e) => e,
        Err(findings) => {
            for f in &findings {
                eprintln!("config: {f}");
            }
            eprintln!("{} finding(s) in {}; nothing was written", findings.len(), cfg_path.display());
            std::process::exit(1);
        }
    };

    // pi's agent directory has to exist for its config to mean anything. Creating it
    // on a box pi was never installed on would be inventing state, so the default is
    // to skip — loudly, on stdout, in the journal, and in the status file. Not
    // `ConditionPathIsDirectory=` on the unit: that is the silent kind of skip, and a
    // sync that silently never ran is the failure mode this whole file is written
    // against (AGENTS.md: a probe that prints nothing is broken, not negative).
    let parent = out_path.parent().unwrap_or_else(|| Path::new(""));
    if !parent.is_dir() {
        if !create_missing {
            // No status file either: its natural home is the directory that is
            // missing, and writing it somewhere else would be inventing a second
            // location for `--report` to look in and not find. The journal keeps this
            // line, which is the record that the timer ran and chose not to write.
            println!(
                "SKIP: {} does not exist — pi is not installed for this user here (or --out is wrong). Nothing written, and no status file to read back: this line in `journalctl -u pi-agent-<user>` is the record.",
                parent.display()
            );
            std::process::exit(0);
        }
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("cannot create {}: {e}", parent.display());
            std::process::exit(1);
        }
    }

    println!("pi-agent-sync {VERSION}: {} endpoint(s) from {}", endpoints.len(), cfg_path.display());
    let mut reports = Vec::new();
    for ep in &endpoints {
        let r = probe::probe(ep, &curl, timeout);
        print_line(&r);
        reports.push(r);
    }

    let existing = std::fs::read_to_string(&out_path).unwrap_or_default();
    let (text, outcome) = render::merge(&existing, &endpoints, &reports);
    for w in &outcome.warnings {
        println!("  note: {w}");
    }

    if dry_run {
        // Redacted: this document can contain a provider that was only preserved, and
        // a preserved provider can contain somebody's key (see `render::redact`).
        println!("--- dry run, nothing written (credentials redacted) ---\n{}", render::redact(&text));
    } else if text.trim() == existing.trim() {
        println!("{}: unchanged", out_path.display());
    } else if let Err(e) = atomic_write(&out_path, text.as_bytes()) {
        eprintln!("cannot write {}: {e}", out_path.display());
        std::process::exit(1);
    } else {
        println!("{}: written ({} provider(s) updated)", out_path.display(), outcome.written.len());
    }

    let default = check_default(settings_path.as_deref(), &text);
    if let Some(line) = &default {
        println!("  note: {line}");
    }

    write_status(status_path.as_deref(), &endpoints, &reports, Some(&outcome), &out_path, if dry_run { "dry-run" } else { "synced" }, &default.iter().cloned().collect::<Vec<_>>());

    let failed: Vec<&Report> = reports.iter().filter(|r| !r.usable()).collect();
    if !failed.is_empty() {
        println!(
            "{} of {} endpoint(s) unusable — their entries were kept from the previous file where one existed.",
            failed.len(),
            reports.len()
        );
    }
    if strict && !failed.is_empty() {
        std::process::exit(1);
    }
}

fn print_line(r: &Report) {
    match &r.error {
        Some(err) => println!("  {:<18} FAILED  http={} {}ms\n                   {err}", r.endpoint_id, r.models_http, r.elapsed_ms),
        None => {
            let models: Vec<String> = r
                .models
                .iter()
                .map(|m| {
                    let ctx = m.context_window.map(|c| c.to_string()).unwrap_or_else(|| "?".into());
                    let loaded = match m.loaded {
                        Some(true) => "",
                        Some(false) => " unloaded",
                        None => "",
                    };
                    format!("{} ctx={}{} [{}]", m.id, ctx, loaded, m.ctx_source)
                })
                .collect();
            println!(
                "  {:<18} ok      http={} props={} {}ms  {}",
                r.endpoint_id,
                r.models_http,
                r.props_http,
                r.elapsed_ms,
                models.join(", ")
            );
        }
    }
}

/// Write to a sibling and rename. pi opens `models.json`, reads it and closes it — it
/// holds no descriptor between reloads — so a rename is safe here, and it is what
/// makes "half a file" impossible. (Compare AGENTS.md rule 29, where the *opposite*
/// is true of dnsmasq's lease file: the tool that owns the descriptor decides whether
/// replace-in-place or replace-the-path is the safe one.)
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.flush()?;
        f.sync_all()?;
    }
    // Keep the mode the file already had (0600 if a person made it private) rather
    // than the umask default of the timer.
    if let Ok(meta) = std::fs::metadata(path) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(meta.permissions().mode() & 0o7777));
    }
    std::fs::rename(&tmp, path)
}

fn status_endpoint(ep: &config::Endpoint, r: Option<&Report>) -> Json {
    let Some(r) = r else {
        return object(vec![
            ("id".into(), string(ep.id.clone())),
            ("ok".into(), Json::Bool(false)),
            ("error".into(), string("not probed")),
        ]);
    };
    object(vec![
        ("id".into(), string(ep.id.clone())),
        ("ok".into(), Json::Bool(r.usable())),
        ("modelsUrl".into(), string(r.models_url.clone())),
        ("propsUrl".into(), string(r.props_url.clone())),
        ("modelsHttp".into(), string(r.models_http.clone())),
        ("propsHttp".into(), string(r.props_http.clone())),
        ("elapsedMs".into(), number_from_int(r.elapsed_ms as u64)),
        // The endpoint-level evidence for `reasoning`, kept even when a model's own
        // entry got its answer from the previous run: this says what /props said now.
        ("propsSaysThinking".into(), r.template_thinks.map(Json::Bool).unwrap_or(Json::Null)),
        ("note".into(), ep.note.as_ref().map(|n| string(n.clone())).unwrap_or(Json::Null)),
        ("error".into(), r.error.as_ref().map(|e| string(e.clone())).unwrap_or(Json::Null)),
        (
            "models".into(),
            Json::Arr(
                r.models
                    .iter()
                    .map(|m| {
                        object(vec![
                            ("id".into(), string(m.id.clone())),
                            (
                                "contextWindow".into(),
                                m.context_window.map(number_from_int).unwrap_or(Json::Null),
                            ),
                            ("ctxSource".into(), string(m.ctx_source.clone())),
                            (
                                "reasoning".into(),
                                m.reasoning.map(Json::Bool).unwrap_or(Json::Null),
                            ),
                            ("reasoningSource".into(), string(m.reasoning_source.clone())),
                            ("input".into(), string_array(m.input.clone())),
                            ("loaded".into(), m.loaded.map(Json::Bool).unwrap_or(Json::Null)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

#[allow(clippy::too_many_arguments)]
fn write_status(
    path: Option<&Path>,
    endpoints: &[config::Endpoint],
    reports: &[Report],
    outcome: Option<&render::Outcome>,
    out_path: &Path,
    state: &str,
    warnings: &[String],
) {
    let Some(path) = path else { return };
    let previous = std::fs::read_to_string(path).ok().and_then(|t| common_json::parse(&t).ok());
    // The last run in which at least one endpoint answered. `--report` shows it as an
    // age, which is the difference between "the engines are down" and "this timer has
    // not run since the box was reinstalled".
    let last_success = previous
        .as_ref()
        .and_then(|d| d.get("lastSuccessUtc"))
        .and_then(|j| j.as_u64());
    let any_ok = reports.iter().any(|r| r.usable());
    let empty = render::Outcome::default();
    let outcome = outcome.unwrap_or(&empty);
    let mut all_warnings: Vec<Json> = outcome.warnings.iter().map(|w| string(w.clone())).collect();
    all_warnings.extend(warnings.iter().map(|w| string(w.clone())));

    let doc = object(vec![
        ("tool".into(), string(format!("pi-agent-sync {VERSION}"))),
        ("state".into(), string(state)),
        ("generatedUtc".into(), number_from_int(epoch())),
        (
            "lastSuccessUtc".into(),
            number_from_int(if any_ok { epoch() } else { last_success.unwrap_or(0) }),
        ),
        ("out".into(), string(out_path.display().to_string())),
        (
            "endpoints".into(),
            Json::Arr(endpoints.iter().map(|ep| status_endpoint(ep, reports.iter().find(|r| r.endpoint_id == ep.id))).collect()),
        ),
        ("written".into(), string_array(outcome.written.clone())),
        ("keptStale".into(), string_array(outcome.kept_stale.clone())),
        ("preserved".into(), string_array(outcome.preserved.clone())),
        ("warnings".into(), Json::Arr(all_warnings)),
    ]);
    if let Err(e) = atomic_write(path, (emit(&doc, 2) + "\n").as_bytes()) {
        eprintln!("cannot write {}: {e}", path.display());
    }
}

/// Is the model pi starts on actually in the file we just wrote?
///
/// `defaultProvider`/`defaultModel` in `settings.json` are per-person preferences and
/// this program does not touch them — but it *can* make them impossible, by
/// discovering a different id than the one saved (the engine changed quant, or the
/// provider id moved). That is worth one line rather than a silent fallback to some
/// other model at startup.
fn check_default(settings: Option<&Path>, models_text: &str) -> Option<String> {
    let settings = settings?;
    let sdoc = match std::fs::read_to_string(settings).ok().map(|t| common_json::parse(&t)) {
        Some(Ok(d)) => d,
        _ => return None,
    };
    let provider = sdoc.get("defaultProvider").and_then(|j| j.as_str())?.to_string();
    let model = sdoc.get("defaultModel").and_then(|j| j.as_str())?.to_string();
    let mdoc = common_json::parse(models_text).ok()?;
    let p = mdoc.at(&["providers", &provider])?;
    let hit = p
        .get("models")
        .and_then(|j| j.as_array())
        .map(|arr| arr.iter().any(|m| m.get("id").and_then(|j| j.as_str()) == Some(model.as_str())))
        .unwrap_or(false);
    if hit {
        None
    } else {
        Some(format!(
            "settings.json wants {provider}/{model}, which this run did not write — /model to pick another, or the engine may be off"
        ))
    }
}

fn report(path: &Path) -> i32 {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cannot read {}: {e}", path.display());
            eprintln!("(a sync that skips because the agent directory is absent writes no status file — `journalctl -u pi-agent-<user>` is the record there)");
            return 1;
        }
    };
    let doc = match common_json::parse(&text) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{} is not readable JSON: {e}", path.display());
            return 1;
        }
    };
    let now = epoch();
    let age = |key: &str| -> String {
        match doc.get(key).and_then(|j| j.as_u64()) {
            Some(0) | None => "never".to_string(),
            Some(t) if t > now => "clock skew".to_string(),
            Some(t) => format!("{}s ago", now - t),
        }
    };
    println!("{} — {}", doc.get("tool").and_then(|j| j.as_str()).unwrap_or("?"), doc.get("state").and_then(|j| j.as_str()).unwrap_or("?"));
    println!("  ran {}   last endpoint that answered: {}", age("generatedUtc"), age("lastSuccessUtc"));
    println!("  out  {}", doc.get("out").and_then(|j| j.as_str()).unwrap_or("?"));
    if let Some(Json::Arr(items)) = doc.get("endpoints") {
        for e in items {
            let id = e.get("id").and_then(|j| j.as_str()).unwrap_or("?");
            let ok = e.get("ok").and_then(|j| j.as_bool()).unwrap_or(false);
            if ok {
                let models = e.get("models").and_then(|j| j.as_array()).map(|a| a.to_vec()).unwrap_or_default();
                let listed: Vec<String> = models
                    .iter()
                    .map(|m| {
                        let ctx = m.get("contextWindow").and_then(|j| j.as_u64()).map(|c| c.to_string()).unwrap_or_else(|| "?".into());
                        let src = m.get("ctxSource").and_then(|j| j.as_str()).unwrap_or("?");
                        let think = match m.get("reasoning").and_then(|j| j.as_bool()) {
                            Some(t) => format!("think={t}"),
                            None => "think=?".into(),
                        };
                        let tsrc = m.get("reasoningSource").and_then(|j| j.as_str()).unwrap_or("?");
                        format!("{} ctx={} [{src}] {think} [{tsrc}]", m.get("id").and_then(|j| j.as_str()).unwrap_or("?"), ctx)
                    })
                    .collect();
                println!("  {id:<18} ok      {}", listed.join(", "));
            } else {
                println!("  {id:<18} FAILED  {}", e.get("error").and_then(|j| j.as_str()).unwrap_or("(no reason recorded)"));
            }
        }
    }
    for (key, label) in [("keptStale", "kept from last time"), ("preserved", "not managed here")] {
        if let Some(Json::Arr(items)) = doc.get(key) {
            let names: Vec<String> = items.iter().filter_map(|j| j.as_str().map(str::to_string)).collect();
            if !names.is_empty() {
                println!("  {label}: {}", names.join(", "));
            }
        }
    }
    if let Some(Json::Arr(items)) = doc.get("warnings") {
        for w in items.iter().filter_map(|j| j.as_str()) {
            println!("  note: {w}");
        }
    }
    0
}
