//! An answer written for Slack, turned into something worth saying out loud.
//!
//! The agent's answer is markdown: `crates/slack-agent/src/prompt.rs` tells the
//! model to use `-` bullets, and `slack::escape` only touches Slack's own
//! metacharacters.  Read aloud, that text is a catastrophe in three specific ways,
//! all of them measured on the real `space_status` answer this pipeline exists for:
//!
//! 1. **a URL is not pronounceable.** `http://10.3.1.21:8080/metrics` said aloud is
//!    forty syllables of noise.  It goes.
//! 2. **structure is invisible.** Bullets and headings are *position*, and a voice
//!    has no position, so they become commas and full stops or the sentence runs on.
//! 3. **it is far too long.** The Slack answer budget is 700 tokens (~1,500 words,
//!    `agent.rs`).  At the measured rate of the deployed voice — 31 words synthesised
//!    in 7.8 s of audio, 2.5 s of wall clock — that is a five-minute monologue out of
//!    a camera speaker, with the room waiting.  So the budget here is *sentences*, and
//!    the truncation lands on a sentence boundary, because a voice that stops
//!    mid-word sounds broken even when it is only short.
//!
//! What is **not** touched: digits, and the words around them.  "12.2" must come out
//! as the temperature, so no number is rounded, expanded or paraphrased here.

/// Longest answer, in characters, that we are willing to speak.  ~55 words, ~14 s of
/// audio at the measured rate.  A question that genuinely needs more is a question for a
/// screen, and `--max-speech-chars` says so out loud rather than silently.
///
/// This is the CLI default, not a second policy: a deployed unit always passes
/// `--max-speech-chars`, from `infra.voiceAssistant.maxSpeechChars`, which is why
/// `main.rs` names this constant instead of writing the number a second time.
pub const DEFAULT_MAX_CHARS: usize = 320;
/// Markdown/Slack-flavoured text → speech text.
///
/// Returns `None` when there is nothing left to say (the answer was only a link, or
/// only a code block), which the caller must treat as "do not speak" rather than as
/// success: piper given an empty string still produces a click, and a click from a
/// ceiling speaker in a quiet room is alarming.
pub fn speakable(text: &str, max_chars: usize) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;

    for raw in text.lines() {
        let line = raw.trim_end();
        let t = line.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if t.is_empty() {
            out.push(' ');
            continue;
        }
        // A table is a two-dimensional object; read aloud it is meaningless either
        // way, and the separator row (`|---|---|`) is the clearest sign of one.
        // Any pipe-delimited row: a table read aloud is meaningless either way, and
        // the separator row is the clearest sign of one, but the header and the data
        // rows go too.
        if t.starts_with('|') || t.contains("---|") || t.starts_with("|--") {
            continue;
        }
        let mut piece = t.to_string();
        // Heading, blockquote, list marker → prose.  Numbered lists keep their
        // number: "1. open the door" read aloud is a step, and dropping the number
        // makes two steps one sentence.
        while piece.starts_with('#') || piece.starts_with('>') {
            piece.remove(0);
        }
        let piece = piece.trim_start();
        let piece = piece
            .strip_prefix("- ")
            .or_else(|| piece.strip_prefix("* "))
            .or_else(|| piece.strip_prefix("• "))
            .unwrap_or(piece);
        out.push_str(&spoken_line(piece));
        out.push(' ');
    }

    let mut collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    // Join bullet fragments into sentences: a bullet with no terminal punctuation
    // was one clause of a list, and the listener needs the pause.
    collapsed = collapsed.replace(" .", ".");
    let collapsed = tidy_units(&collapsed);
    let trimmed = truncate_sentences(&collapsed, max_chars);
    let trimmed = trimmed.trim().trim_matches(',').trim().to_string();
    if trimmed.is_empty() || trimmed.chars().all(|c| !c.is_alphanumeric()) {
        return None;
    }
    // End on real punctuation, or piper runs the last clause up against the silence
    // with rising prosody, which sounds like a question and like an interruption.
    let ends_clean = trimmed
        .chars()
        .last()
        .is_some_and(|c| matches!(c, '.' | '!' | '?' | ':'));
    if ends_clean {
        Some(trimmed)
    } else {
        Some(format!("{trimmed}."))
    }
}

/// One line, with the things a voice cannot say removed.
fn spoken_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // [text](url) → text, and a bare http(s) URL → nothing at all.
        if chars[i] == '[' {
            if let Some(close) = chars[i..].iter().position(|&c| c == ']') {
                let label: String = chars[i + 1..i + close].iter().collect();
                let after = i + close + 1;
                let had_target = chars.get(after) == Some(&'(');
                out.push_str(&label);
                i = if had_target {
                    match chars[after..].iter().position(|&c| c == ')') {
                        Some(p) => after + p + 1,
                        None => after,
                    }
                } else {
                    after
                };
                continue;
            }
        }
        if chars[i] == '*' || chars[i] == '_' || chars[i] == '~' || chars[i] == '`' {
            // Emphasis and inline code are typography.  Inline code is *dropped*
            // along with its content below only when it is a whole token; a stray
            // asterisk between words just goes.
            i += 1;
            continue;
        }
        if chars[i] == '<' {
            // A Slack `<https://x|label>` or an HTML tag: keep the label.
            if let Some(close) = chars[i..].iter().position(|&c| c == '>') {
                let inner: String = chars[i + 1..i + close].iter().collect();
                let label = inner.rsplit('|').next().unwrap_or("").to_string();
                if !label.contains("://") {
                    out.push_str(&label);
                }
                i += close + 1;
                continue;
            }
        }
        if chars[i] == ':' {
            // A Slack emoji shortcode `:thumbsup:`: a colon, a short word of the
            // alphabet Slack itself allows, a closing colon, and whitespace or
            // punctuation after.  The shape matters — without it, "open at 18:40 on
            // the :hearts:" would lose the time as well as the heart.
            let after: String = chars[i + 1..].iter().take(40).collect();
            let word_len = after
                .find(':')
                .filter(|p| *p > 0 && *p <= 30)
                .filter(|p| after[..*p].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '_')))
                .filter(|p| {
                    let next = chars.get(i + 2 + *p);
                    next.map(|c| c.is_whitespace() || matches!(c, ',' | '.' | '!' | '?' | ')' | ';'))
                        .unwrap_or(true)
                });
            if let Some(len) = word_len {
                i += len + 2;
                continue;
            }
        }
        // A bare URL: not pronounceable.  Checked only where a word can start, so
        // the scan stays linear.
        if (chars[i] == 'h' || chars[i] == 'H')
            && (i == 0 || chars[i - 1] == ' ' || chars[i - 1] == '(' || chars[i - 1] == '<')
        {
            let head: String = chars[i..(i + 8).min(chars.len())].iter().collect();
            if head.to_lowercase().starts_with("http://") || head.to_lowercase().starts_with("https://") {
                let end = chars[i..]
                    .iter()
                    .position(|&c| c.is_whitespace())
                    .map(|p| i + p)
                    .unwrap_or(chars.len());
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// The units this building's sensors actually report, said properly.
fn tidy_units(text: &str) -> String {
    let mut t = text.replace("°C", " degrees Celsius").replace("°F", " degrees Fahrenheit");
    t = t.replace("%", " per cent");
    for (from, to) in [
        ("->", " to "),
        ("=>", " to "),
        ("–", "-"),
        // A spaced em dash first, with its spaces: the bare rule below left the space
        // that had preceded the dash in place and produced "closed , it's" — which is a
        // comma spoken as a half-second pause where the sentence had none.  Measured on
        // the deployed answer for "is the hackspace open", so this is what a room heard.
        (" — ", ", "),
        (" - ", ", "),
        ("—", ", "),
        (" / ", " or "),
        ("  ", " "),
    ] {
        t = t.replace(from, to);
    }
    // Whatever produced it, a space before a comma is a pause the text did not ask for,
    // and piper honours it.
    while t.contains(" ,") {
        t = t.replace(" ,", ",");
    }
    // "12.2 C" (how the tools print it when the degree sign is lost) still means
    // Celsius, and a bare "12.2 C" read as "point c" is the difference between an
    // answer and a riddle.
    let mut out = String::with_capacity(t.len());
    let chars: Vec<char> = t.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let after_c = chars
            .get(i + 1)
            .map(|c| c.is_whitespace() || matches!(c, ',' | '.' | ';' | ')'))
            .unwrap_or(true);
        if chars[i] == 'C' && i > 1 && chars[i - 1] == ' ' && after_c && {
            let prev = chars[i - 2];
            prev.is_ascii_digit() || prev == '.'
        } {
            out.push_str("degrees Celsius ");
        } else {
            out.push(chars[i]);
        }
        i += 1;
    }
    while out.contains("  ") {
        out = out.replace("  ", " ");
    }
    out.trim().to_string()
}

/// Cut to at most `max_chars`, at the last sentence boundary that fits.
///
/// If not even one sentence fits, the first sentence is returned whole, longer than
/// the budget: a truncated mid-clause answer is worse than a long one, because the
/// listener cannot tell a cut-off from a malfunction.
fn truncate_sentences(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut best: Option<usize> = None;
    let mut i = 0;
    while i < chars.len() && i < max_chars {
        if matches!(chars[i], '.' | '!' | '?') {
            // A boundary is a full stop followed by a space or the end, which is also
            // what keeps "12.2" one number: the digit after the point is not a space.
            if i + 1 >= chars.len() || chars[i + 1].is_whitespace() {
                best = Some(i + 1);
            }
        }
        i += 1;
    }
    match best {
        Some(end) => chars[..end].iter().collect(),
        None => {
            // No sentence finished inside the budget.  Stop at a word boundary and
            // say nothing about having stopped: an em dash in text read aloud is a
            // click, and the truncation belongs in the caller's log, where a person
            // debugging "why did it stop halfway" will actually look.
            let cut = chars
                .iter()
                .take(max_chars)
                .rposition(|&c| c == ' ')
                .unwrap_or(max_chars);
            chars[..cut].iter().collect::<String>()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape `space_status` actually produces through the model: bullets, a
    /// degree sign, a link, and far more than anyone asked for.
    const REAL_ANSWER: &str = "**Space:** open since 18:40 UTC\n\
         - Main Space 19.4 C, Workshop 12.2 C\n\
         - Printers: both offline (see [the monitor](http://10.3.1.20:3000/d/prints))\n\
         \n\
         ```json\n{\"open\": true}\n```\n\
         Details in https://grafana.int.leighhack.org/d/space if you want them.";

    #[test]
    fn the_real_answer_becomes_one_speakable_paragraph() {
        let s = speakable(REAL_ANSWER, DEFAULT_MAX_CHARS).unwrap();
        assert!(!s.contains('{'), "code block survived: {s}");
        assert!(!s.contains("http"), "a URL survived: {s}");
        assert!(!s.contains('['), "a markdown link survived: {s}");
        assert!(s.contains("monitor"), "the link's *label* should survive: {s}");
        assert!(s.contains("19.4"), "the temperature went away: {s}");
        assert!(s.contains("12.2"), "the second temperature went away: {s}");
        assert!(s.contains("degrees Celsius"), "no unit spoken: {s}");
        assert!(!s.contains("**"), "emphasis markers survived: {s}");
        assert!(s.ends_with('.'), "no terminal punctuation: {s}");
    }

    #[test]
    fn numbers_are_never_rewritten() {
        let s = speakable("It is 12.2 C and dropping 0.7 C overnight; 7 people in.", 320).unwrap();
        assert!(s.contains("12.2") && s.contains("0.7") && s.contains("7"), "{s}");
        assert!(s.contains("degrees Celsius"), "{s}");
    }

    #[test]
    fn truncation_lands_on_a_sentence() {
        let long = "The workshop is cold. The main space is warm. Fabrication is unheated as usual. The craft space has a smell again.";
        let s = speakable(long, 45).unwrap();
        assert!(s.len() < long.len());
        assert!(s.ends_with('.'), "cut mid-sentence: {s}");
        assert!(s.contains("cold"), "{s}");
        assert!(!s.contains("Fabrication"), "kept too much: {s}");
    }

    #[test]
    fn a_single_long_sentence_is_not_cut_mid_clause() {
        let one = "The workshop is cold at twelve point two degrees and the heater on that wall has been off since Monday which is the third time this month";
        let s = speakable(one, 40).unwrap();
        assert!(!s.contains(" —"), "an em dash was spoken at the room: {s}");
        // Whole first sentence wins even though it busts the budget.
        assert!(s.starts_with("The workshop is cold"));
    }

    #[test]
    fn nothing_worth_saying_is_a_refusal_not_a_click() {
        assert!(speakable("", 320).is_none());
        assert!(speakable("   \n\n  ", 320).is_none());
        assert!(speakable("```x = 1```", 320).is_none(), "only code");
        assert!(speakable("http://example.com/a/b", 320).is_none(), "only a URL");
        assert!(speakable("---|---|---", 320).is_none(), "only a table rule");
        assert!(speakable(":thumbsup:", 320).is_none(), "only an emoji");
    }

    #[test]
    fn bullets_become_prose_and_a_table_disappears() {
        let s = speakable("- open\n- closed\n\n| room | C |\n|--|--|\n| shop | 12 |", 320).unwrap();
        assert!(!s.contains('|'), "{s}");
        assert!(s.contains("open") && s.contains("closed"), "{s}");
    }

    #[test]
    fn slack_link_syntax_keeps_the_label() {
        let s = speakable("see <https://x.int/dash|the dashboard> for more", 320).unwrap();
        assert!(s.contains("dashboard"), "{s}");
        assert!(!s.contains("x.int"), "{s}");
    }

    #[test]
    fn a_decimal_point_is_not_a_sentence_boundary() {
        // The first '.' in "19.4" is a decimal point and the second one is a real
        // stop; a truncation that chose the first would end the answer with "19.".
        let s = speakable("Main 19.4 C is fine. Workshop 12.2 C is cold.", 35).unwrap();
        assert_eq!(s, "Main 19.4 degrees Celsius is fine.", "{s}");
        // And where no sentence fits at all: it stops on a word boundary, never inside
        // the number, and adds nothing but the full stop piper needs.
        let s = speakable("Main 19.4 C is fine. Workshop 12.2 C is cold.", 30).unwrap();
        assert!(s.len() < 34, "{s}");
        assert!(s.contains("19.4"), "lost the figure: {s}");
        assert!(!s.contains("19. "), "cut inside the number: {s}");
        // The property that matters: the spoken text is a *prefix* of the tidied
        // answer, so the only thing ever invented is the full stop piper needs.
        assert!(
            tidy_units("Main 19.4 C is fine. Workshop 12.2 C is cold.").starts_with(s.trim_end_matches('.')),
            "not a clean prefix: {s}"
        );
    }

    #[test]
    fn a_time_is_not_an_emoji_shortcode() {
        let s = speakable("Open since 18:40 UTC :thumbsup:", 320).unwrap();
        assert!(s.contains("18:40"), "the time went away: {s}");
        assert!(!s.contains("thumbsup"), "{s}");
    }

    #[test]
    fn a_spaced_dash_becomes_a_comma_and_not_a_comma_with_a_pause_before_it() {
        // The exact answer the deployed gateway produced for "is the hackspace open
        // right now" on 2026-11-17, verbatim.  Its em dash came out as "closed , it's":
        // the dash became ", " and the space that had preceded it stayed, so the room
        // heard a half-second hesitation the sentence never asked for.  Text this short
        // is nearly all pause markers, so which one is where is the whole difference
        // between an answer and a dictation.
        let answer = "No, the space is closed — it's been closed for about two hours \
            and seventeen minutes, since 22:23 UTC.";
        let s = speakable(answer, 320).unwrap();
        assert!(!s.contains(" ,"), "a pause before a comma reached the room: {s}");
        assert!(s.contains("closed, it's been"), "{s}");
        assert!(!s.contains('—') && !s.contains("  "), "{s}");
    }
}
