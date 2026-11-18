//! Which ears a session gets, and the wire shape of a reply that carries more than one score.
//!
//! ## Why this is a module rather than four functions in `serve.rs`
//!
//! `serve.rs` is an HTTP server: request shapes, session lifetimes, the single mutex, recording.
//! This is a decision rule and a format. They changed for different reasons on the same day — the
//! server grew an `--idle` and a recorder, this grew a second engine — and the guard in
//! `just check-source-size` (AGENTS.md rule 26) is what forced the recognition rather than my
//! planning it, which is worth stating plainly because it is the usual way a seam shows up:
//! discovered by a rule, not designed in advance.
//!
//! ## The rule this module exists to enforce
//!
//! **An engine name that is not loaded is a refusal, never a fallback.** That is the whole design.
//! A process that quietly scored `{"engine":"oww"}` with the LiveKit model would return real,
//! confident, *wrong-attributed* numbers, and "I selected the other detector and it heard nothing"
//! would be indistinguishable from "that detector is bad" — which is the exact confusion this
//! feature was built to end, so getting it wrong here would make the feature a lie. Hence the
//! refusal names both the request and what is available.
//!
//! ## Why the two engines do not share a threshold
//!
//! They were trained separately on the same phrase; `0.5` is not the same claim in each. A single
//! dial would mean a session on one engine judged by the other's scale, so each has its own and a
//! session is told which one its primary ear is using.

use crate::ear::Hit;

/// The engines a process can score with. `has_oww` is "the model directory was given", which is
/// not the same as "the models load" — that is checked at boot, where a bad directory fails the
/// process rather than surprising the first session that asks.
pub fn available_engines(has_oww: bool) -> Vec<&'static str> {
    let mut v = vec!["livekit"];
    if has_oww {
        v.push("oww");
    }
    v
}

/// Which engines a `POST /v1/session` asks for, validated against what this process loaded.
///
/// Primitives in, decision out: no `Options`, no request, no HTTP. Every bug in engine selection
/// would live here — a typo, an unloaded engine, `both` asked of a one-model process — and all of
/// them are testable without loading a single ONNX file.
pub fn select_engines(
    wanted: Option<&str>,
    default_engine: &str,
    available: &[&'static str],
) -> Result<Vec<String>, (u16, String)> {
    let want = wanted.unwrap_or(default_engine);
    let picked: Vec<String> = match want {
        // `both` is why this is an API parameter rather than a restart-time flag. Switching
        // engines by hand cannot earn a comparison — the two never hear the same audio, in the
        // same room, at the same moment. One stream, both scores, byte-identical input.
        "both" => available.iter().map(|s| s.to_string()).collect(),
        other => vec![other.to_string()],
    };
    if picked.is_empty() {
        return Err((400, "no engine selected".to_string()));
    }
    for e in &picked {
        if !available.contains(&e.as_str()) {
            return Err((
                400,
                format!(
                    "engine {:?} is not loaded here; this process offers [{}]. Start it with \
                     --oww-models <dir> to add \"oww\"",
                    e,
                    available.join(", ")
                ),
            ));
        }
    }
    Ok(picked)
}

/// Render a list of engine names as a JSON array.  One function because two hand-rolled
/// `format!`s of the same array is how `/status.json` came out as `["livekit,"oww]` — invalid
/// JSON, from a `join(",")` whose items had already been quoted and then got re-quoted — and a
/// status endpoint that does not parse is worse than no status endpoint, because the readers of
/// it are exactly the tools someone reaches for when something looks wrong.
pub fn engine_array<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    format!(
        "[{}]",
        names
            .into_iter()
            .map(|n| format!(r#""{n}""#))
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// Per-engine results for one chunk, keyed by engine name.
///
/// Takes flattened rows rather than the session's own slots so the shape of the wire contract is
/// testable without a model: a test that needs a 7 MB ONNX file to check a format is a test that
/// does not get run, and this is the object an A/B caller reads both scores out of.
pub fn engines_json(rows: &[(String, f32, u32, Hit)]) -> String {
    let parts: Vec<String> = rows
        .iter()
        .map(|(name, peak, detections, h)| {
            format!(
                r#""{}":{{"score":{:.4},"detected":{},"peak":{:.4},"detections":{}}}"#,
                name, h.score, h.detected, peak, detections
            )
        })
        .collect();
    format!("{{{}}}", parts.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: [&str; 1] = ["livekit"];
    const TWO: [&str; 2] = ["livekit", "oww"];

    fn want(engine: &str) -> Option<&str> {
        Some(engine)
    }

    #[test]
    fn a_session_that_names_nothing_gets_the_startup_engine() {
        assert_eq!(
            select_engines(None, "livekit", &ONE).unwrap(),
            vec!["livekit".to_string()]
        );
        assert_eq!(
            select_engines(None, "oww", &TWO).unwrap(),
            vec!["oww".to_string()],
            "the startup default is honoured even when it is not the first name"
        );
    }

    #[test]
    fn an_engine_that_was_never_loaded_is_refused_loudly() {
        // It must NOT fall back to livekit. A substituted engine returns real, confident numbers,
        // so nothing in the reply is suspicious and the caller concludes the model is broken.
        let err = select_engines(want("oww"), "livekit", &ONE).unwrap_err();
        assert_eq!(err.0, 400);
        assert!(err.1.contains("--oww-models"), "must say how to get it: {}", err.1);
        assert!(err.1.contains("livekit"), "must say what IS here: {}", err.1);
    }

    #[test]
    fn a_typo_never_becomes_the_default_engine() {
        let err = select_engines(want("livekwit"), "livekit", &TWO).unwrap_err();
        // The refusal echoes what was asked for, so it cannot be misread as "fine, we used the
        // default" — that substitution is the silent-zero failure this function exists to prevent.
        assert!(err.1.contains("livekwit"), "must echo the request: {}", err.1);
        assert_eq!(err.0, 400);
    }

    #[test]
    fn both_asks_for_every_loaded_engine_once() {
        assert_eq!(
            select_engines(want("both"), "livekit", &TWO).unwrap(),
            vec!["livekit".to_string(), "oww".to_string()]
        );
        // On a one-engine process `both` is neither an error nor a doubling: it is one ear.
        assert_eq!(
            select_engines(want("both"), "livekit", &ONE).unwrap(),
            vec!["livekit".to_string()]
        );
    }

    #[test]
    fn one_side_of_the_ab_can_be_asked_for_alone() {
        assert_eq!(
            select_engines(want("oww"), "livekit", &TWO).unwrap(),
            vec!["oww".to_string()]
        );
    }

    #[test]
    fn the_reply_carries_both_scores_keyed_by_engine_so_a_caller_cannot_misread_which_is_which() {
        let rows = vec![
            (
                "livekit/hey_jarvis".to_string(),
                0.61_f32,
                2_u32,
                Hit { score: 0.62, detected: true },
            ),
            (
                "openwakeword/hey_jarvis_v0.1".to_string(),
                0.02_f32,
                0_u32,
                Hit { score: 0.02, detected: false },
            ),
        ];
        let j = engines_json(&rows);
        assert!(
            j.contains(r#""livekit/hey_jarvis":{"score":0.6200,"detected":true,"peak":0.6100,"detections":2}"#),
            "the primary ear must be readable by name: {j}"
        );
        assert!(
            j.contains(r#""openwakeword/hey_jarvis_v0.1":{"score":0.0200,"detected":false,"peak":0.0200,"detections":0}"#),
            "and so must the second, with its own peak and its own count: {j}"
        );
    }

    #[test]
    fn openwakeword_is_offered_only_when_its_models_were_given() {
        assert_eq!(available_engines(false), vec!["livekit"]);
        assert_eq!(available_engines(true), vec!["livekit", "oww"]);
    }
    #[test]
    fn the_engine_list_is_valid_json_even_with_two_names_in_it() {
        // The exact shape that came out broken the first time: quoted items joined by a comma
        // that then got quoted again.  `json.loads` is the real gate; this is the cheap one.
        assert_eq!(engine_array(available_engines(false)), r#"["livekit"]"#);
        assert_eq!(engine_array(available_engines(true)), r#"["livekit","oww"]"#);
    }
}
