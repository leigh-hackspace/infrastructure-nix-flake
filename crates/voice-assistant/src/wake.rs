//! Deciding whether a transcript was addressed to us, and pulling the request out
//! of it.
//!
//! The wake word is matched on the **transcript**, not on the audio.  That is a
//! deliberate, and slightly unfashionable, choice: `gate.rs`'s DSP cannot tell
//! "hey jarvis" from "hey, voices", but whisper.cpp can, and a match on text is a
//! match — there is a measured string in the log to argue with afterwards.  The
//! price is one transcription per gated window, which is the cost `--probe-only`
//! exists to count.
//!
//! Matching is **token-wise**, for the reason `crates/slack-agent/src/triage.rs`
//! learned first: a substring test makes `temp` live inside "attempt" and `state`
//! inside "restate", so a substring wake-word test in a room where people say
//! "attempt" would answer to it.  Here `jarvis` matches the word `jarvis`, and not
//! the one inside `jarvises`.
//!
//! Normalisation is shared with the request text: lower-case, digits and letters
//! only, so "Hey, Jarvis!" and "hey  jarvis" and "hey-jarvis" are the same three
//! tokens.

/// Where a wake phrase was found, and what was asked.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit<'a> {
    /// The configured wake phrase that matched, as given on the command line.
    pub phrase: &'a str,
    /// The request, with the wake phrase removed.  Empty means "they woke us up and
    /// said nothing else", which is a different state from no match: the right
    /// behaviour there is to listen for one more utterance, not to answer nothing.
    pub request: String,
    /// Position in tokens, for the log.
    pub at: usize,
}

/// Lower-case, and keep only letters and digits as token characters.
///
/// Letters and digits by `char::is_alphanumeric`, not by ASCII range, because the
/// space's members say "µController" and "2040" and a transcript with an en dash in
/// it is a normal thing for whisper to produce.
pub fn normalise(text: &str) -> String {
    let lowered = text.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut pending_space = false;
    for c in lowered.chars() {
        if c.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
        } else {
            pending_space = true;
        }
    }
    out
}

/// Tokenise a wake phrase from configuration: `hey jarvis` → `["hey","jarvis"]`.
pub fn phrase_tokens(phrase: &str) -> Vec<String> {
    normalise(phrase)
        .split(' ')
        .filter(|t| !t.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Find the first configured wake phrase in `transcript`, and return the request as
/// everything else.
///
/// `search_tokens` bounds **where a phrase may start**: the phrase has to begin
/// within the first `search_tokens` words.  Anywhere-in-the-first-few rather than
/// only at position 0 because whisper's own habit is to lead with an article or a
/// filler ("Okay jarvis…", "a jarvis…"), and a wake word missed because of a stray
/// "the" is indistinguishable, to the person talking, from the assistant being
/// deaf.  The bound is what stops "what did jarvis say about the kettle" — where
/// `jarvis` is the third word — from being read as a command.
///
/// When a phrase matches, the request is the remaining tokens joined; the tokens
/// *before* the phrase are kept too, because "okay what is the temperature jarvis"
/// is one question and dropping the front of it drops the question.
pub fn find<'a>(transcript: &str, phrases: &'a [String], search_tokens: usize) -> Option<Hit<'a>> {
    let norm = normalise(transcript);
    let tokens: Vec<&str> = norm.split(' ').filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() {
        return None;
    }
    let limit = search_tokens.min(tokens.len());
    for phrase in phrases {
        let want = phrase_tokens(phrase);
        if want.is_empty() {
            continue;
        }
        for start in 0..limit {
            if start + want.len() > tokens.len() {
                break;
            }
            if (0..want.len()).all(|i| tokens[start + i] == want[i]) {
                let mut rest: Vec<String> = Vec::with_capacity(tokens.len() - want.len());
                for (i, t) in tokens.iter().enumerate() {
                    if i < start || i >= start + want.len() {
                        rest.push((*t).to_string());
                    }
                }
                return Some(Hit {
                    phrase,
                    request: rest.join(" "),
                    at: start,
                });
            }
        }
    }
    None
}

/// Remove the wake phrase from a request that arrived with it still attached.
///
/// The satellite strips it while deciding whether it was addressed at all, so this is
/// for the other callers — the browser page sends the whole utterance, because its
/// wake-word detection happens in the page's WASM and the transcript it posts still
/// begins "Hey Jarvis…".  Both go through here so there is one rule for what is
/// address and what is question; a second copy in TypeScript is a second rule that
/// will disagree the first time a wake word is added.
///
/// A bare wake phrase returns the text unchanged: "hey jarvis" alone is not a question
/// worth asking, and the caller that wants to notice that uses [`find`].
pub fn strip(text: &str, phrases: &[String]) -> String {
    match find(text, phrases, 6) {
        Some(hit) if !hit.request.trim().is_empty() => hit.request,
        _ => text.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phrases() -> Vec<String> {
        ["hey jarvis", "jarvis", "hey hackspace", "hackspace"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn punctuation_and_case_are_not_a_second_wake_word() {
        let words = phrases();
        for said in ["Hey Jarvis, what's the temperature?", "HEY  JARVIS!", "okay jarvis.", "“jarvis”"] {
            assert!(find(said, &words, 6).is_some(), "missed {said:?}");
        }
    }

    #[test]
    fn the_request_is_what_is_left_and_not_the_wake_word_again() {
        let words = phrases();
        let hit = find("hey jarvis is the space open", &words, 6).unwrap();
        assert_eq!(hit.phrase, "hey jarvis");
        assert_eq!(hit.request, "is the space open");
        assert_eq!(hit.at, 0);
        // The longer phrase wins when it is listed first, and both are listed.
        let hit = find("jarvis is the workshop cold", &words, 6).unwrap();
        assert_eq!(hit.phrase, "jarvis");
        assert_eq!(hit.request, "is the workshop cold");
    }

    #[test]
    fn waking_up_and_saying_nothing_is_its_own_state() {
        let words = phrases();
        let hit = find("Hey Jarvis.", &words, 6).unwrap();
        assert!(hit.request.is_empty(), "got {:?}", hit.request);
    }

    #[test]
    fn words_containing_the_wake_word_are_not_the_wake_word() {
        // The triage.rs lesson, applied here: whole words only.
        assert!(find("this was an attempt at a joke", &["tempt".to_string()], 6).is_none());
        assert!(find("restate the answer", &["state".to_string()], 6).is_none());
        assert!(find("the hackspacey ones are loud", &["hackspace".to_string()], 6).is_none());
    }

    #[test]
    fn a_bare_mention_further_in_is_not_an_address() {
        let words = phrases();
        // "what did jarvis say" is not a request; the search window is the whole
        // difference between a conversation and a command.  `jarvis` here is the
        // third word, so a window of two words cannot see it and a window of three
        // can — which is exactly the knob, and why the default is 6.
        assert!(
            find("what did jarvis say about the kettle", &words, 2).is_none(),
            "the window did not bound the search"
        );
        assert!(find("what did jarvis say about the kettle", &words, 3).is_some());
    }

    #[test]
    fn nothing_said_and_nonsense_said_are_both_no_match() {
        let words = phrases();
        assert!(find("", &words, 6).is_none());
        assert!(find("   ", &words, 6).is_none());
        assert!(find("the printer finished a long print", &words, 6).is_none());
        assert!(find("hello jarvis", &[], 6).is_none(), "no phrases configured");
    }

    #[test]
    fn stripping_leaves_the_question_and_nothing_of_the_address() {
        let words = phrases();
        assert_eq!(
            strip("Hey Jarvis, is the space open?", &words),
            "is the space open"
        );
        // A phrase further in is still removed, and the front of the question stays.
        assert_eq!(strip("okay jarvis what is cold", &words), "okay what is cold");
        // No phrase, no change.
        assert_eq!(strip("what is the temperature", &words), "what is the temperature");
        // A bare wake word is not a question, so it is returned rather than emptied.
        assert_eq!(strip("hey jarvis", &words), "hey jarvis");
    }

    #[test]
    fn unicode_survives_normalisation() {
        assert_eq!(normalise("  2040 — µController’s "), "2040 µcontroller s");
        assert_eq!(phrase_tokens("Hey,  Jarvis!"), vec!["hey", "jarvis"]);
    }
}
