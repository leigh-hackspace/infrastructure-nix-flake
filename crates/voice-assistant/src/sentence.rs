//! Turning a **stream** of answer text into units worth synthesising one at a time.
//!
//! ## Why the voice path needs a splitter and the Slack path does not
//!
//! The agent's answer is one string to Slack, and `spoken::speakable` cleans that one string in
//! one pass. A spoken answer is different in one way that changes the shape of the code: the
//! first sentence is useful *now*, and the rest is still being decoded at 14–39 tok/s
//! (`docs/slack-agent-2026-11-16.md`). So the text arrives in fragments of a few characters, and
//! something has to decide — without ever seeing the whole answer — where a piece is complete
//! enough to hand to piper. That decision is this file, and it is the whole reason audio can
//! start while the model is still talking.
//!
//! ## What a "unit" is, and why the rules are the ones they are
//!
//! A unit is what one piper call gets: normally a sentence. Four rules, each learned from the
//! shape of a real answer rather than from prose style:
//!
//! 1. **A terminator only ends a sentence when something follows it.** `.` at the end of a
//!    fragment is not the end of a sentence; the next fragment might be `2 °C`.
//! 2. **A dot between digits is a decimal point.** `12.2` is the temperature of the workshop and
//!    the single most common number in this bot's answers. Splitting after `12` hands piper
//!    `"12"` and then `".2 °C …"`, which is not merely ugly — it is a *different number* said
//!    aloud ("twelve" … "point two"), and a voice that gets numbers wrong is worse than a slow
//!    one. So: no split when a digit is on either side of the dot.
//! 3. **Abbreviations do not end sentences.** `e.g.`, `i.e.`, `etc.`, `St.`, `No.`, `approx.`,
//!    `vs.`, and a lone initial (`Q.`). The list is short on purpose: it only needs the ones this
//!    bot's own answers contain, and a missed one costs a pause, not a wrong fact.
//! 4. **A fragment too short to be a sentence is joined to the next one.** `"Yes."` on its own is
//!    a whole piper call and a whole paragraph break for two syllables, so it waits for
//!    [`MIN_JOIN_CHARS`] more text or for the end. Newline-separated lines (bullets) are exempt:
//!    a bullet is a unit whatever its length, and `- 18 °C` followed by `- 21 °C` must not be
//!    run together.
//!
//! Plus a backstop: a run-on answer with no punctuation at all is force-split at the last clause
//! boundary past [`MAX_UNIT_CHARS`], because "never split" and "never speak" are the same failure
//! from the room's side.
//!
//! ## What this file does *not* do
//!
//! It does not clean markdown, drop URLs or enforce the speech budget — that is
//! [`crate::spoken`], once per unit, on text that is now known to be complete. Two
//! implementations of the cleanup would be two answers, one of them spoken.

/// Longest unit before it is force-split, characters. ~45 words: the point is not style, it is
/// that a unit is the unit of *latency* — nothing is said until the whole unit is synthesised.
pub const MAX_UNIT_CHARS: usize = 220;
/// Fewer words than this, a completed unit waits for the next one (rule 4).  Words and not
/// characters, because the failure this guards against is `"Yes."` — four characters, one
/// syllable, and a whole piper call and paragraph break — while `"It is 12.2 °C."` is short in
/// characters, complete in meaning, and worth speaking the moment it exists.
pub const MIN_JOIN_WORDS: usize = 3;

/// Case-insensitive, without the trailing dot.
const ABBREVIATIONS: &[&str] = &[
    "e.g", "i.e", "etc", "approx", "vs", "st", "dr", "mr", "mrs", "ms", "fig", "inc", "jun",
    "jul", "sep", "oct", "dept", "est",
];

/// Incremental sentence splitter. State is one `String`: whatever has arrived and not yet been
/// handed out.
#[derive(Default)]
pub struct Splitter {
    tail: String,
}

impl Splitter {
    pub fn new() -> Splitter {
        Splitter::default()
    }

    /// Feed the next fragment; get back every unit that is now complete.
    pub fn push(&mut self, delta: &str) -> Vec<String> {
        self.tail.push_str(delta);
        let mut out = Vec::new();
        // Char index → byte offset, every time: `drain` and the slice are both byte ranges, and
        // the first non-ASCII character in an answer (`°C`, `—`, `”`) is where a splitter that
        // mixed the two starts cutting strings in half.
        while let Some(chars_taken) = self.cut() {
            let bytes = self.byte_offset(chars_taken);
            let unit = self.tail[..bytes].trim().to_string();
            self.tail.drain(..bytes);
            if !unit.is_empty() {
                out.push(unit);
            }
        }
        // Rule 4: a unit that ended the stream but is too short to deserve its own breath waits
        // for company — unless the caller ends the turn, which calls [`flush`].
        out
    }

    /// End of the answer: whatever is left, however incomplete.  Dropping it would cut off the
    /// last sentence the model said, and a voice that stops mid-word sounds broken.
    pub fn flush(&mut self) -> Vec<String> {
        let rest = self.tail.trim().to_string();
        self.tail.clear();
        if rest.is_empty() {
            Vec::new()
        } else {
            vec![rest]
        }
    }

    /// The byte offset just past `n` characters of the tail.
    fn byte_offset(&self, n: usize) -> usize {
        self.tail
            .char_indices()
            .nth(n)
            .map(|(i, _)| i)
            .unwrap_or(self.tail.len())
    }

    /// **Character** index just past the first complete unit — callers convert it with
    /// [`Splitter::byte_offset`], because every comparison here is about characters and every
    /// slice of a `String` is about bytes.
    fn cut(&self) -> Option<usize> {
        let chars: Vec<char> = self.tail.chars().collect();
        let mut i = 0usize;
        let mut words = 0usize;
        while i < chars.len() {
            let c = chars[i];
            if c == '\n' {
                return Some(i + 1);
            }
            if c.is_whitespace() {
                words += 1;
            }
            if is_terminator(c) {
                // Consume any closing punctuation that belongs to the same sentence: `?"`, `!).`
                let mut j = i + 1;
                while j < chars.len() && matches!(chars[j], '"' | '\'' | ')' | ']' | '”' | '’') {
                    j += 1;
                }
                let next = chars.get(j).copied();
                match next {
                    // Nothing after it yet: this is the end of what has arrived, not the end of
                    // the answer. A `.` after a digit is the case that must wait — `12.` is a
                    // complete fragment and `12.2 °C` is one number, and only the next character
                    // says which. Holding costs one read of latency; guessing costs a wrong
                    // figure said aloud.
                    None => {
                        // **A full stop is only a full stop once the next character has been
                        // seen.** `…sensors, e.` could be the end of a sentence or the first
                        // letter of `e.g.`; `12.` could be a figure or `12.2`. A `!`, `?` or `…`
                        // has no such ambiguity, so those end a unit at the edge of the buffer
                        // and a `.` waits for the next read — which is milliseconds of decode,
                        // against a wrong number or a torn abbreviation read aloud.
                        if c == '.' {
                            return None;
                        }
                        return Some(j);
                    }
                    Some(n) if n.is_whitespace() => {}
                    // `12.2`, `e.g.`, an initial: a non-space after the dot means it was not the
                    // end of a sentence, whatever the character class says.
                    Some(_) => {
                        i = j;
                        continue;
                    }
                }
                // `words` counts the spaces *before* the terminator, so a candidate's word count
                // is one more. Getting that off by one makes every three-word sentence wait.
                if words + 1 >= MIN_JOIN_WORDS
                    && !is_decimal(&chars, i)
                    && !after_abbreviation(&chars, i)
                {
                    return Some((j + 1).min(chars.len()));
                }
                // Rule 4: too short to deserve its own breath. Keep looking; the next
                // terminator cuts the joined pair.
                i = j;
            }
            i += 1;
        }
        if chars.len() >= MAX_UNIT_CHARS {
            return Some(clause_boundary(&chars).unwrap_or(chars.len()));
        }
        None
    }
}

fn is_terminator(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '…')
}

/// A dot with digits on both sides is a decimal point, not a full stop.  Measured in this bot's
/// own answers: `12.2 °C`, `43.9 %`, `10.3.1.20` (an address, which piper would otherwise read as
/// four separate numbers).
fn is_decimal(chars: &[char], at: usize) -> bool {
    if chars.get(at) != Some(&'.') {
        return false;
    }
    let before = at.checked_sub(1).and_then(|k| chars.get(k)).copied();
    let after = chars.get(at + 1).copied();
    matches!(before, Some(c) if c.is_ascii_digit()) && matches!(after, Some(c) if c.is_ascii_digit())
}

/// The word before the dot is a known abbreviation, or a lone initial (rule 3).
///
/// The token is whatever stands before the dot up to the **whitespace**, not up to the first
/// non-alphanumeric character, and that distinction is not pedantry: in `19.4 °C.` the token is
/// `°C`, and a scan that stopped at `°` would see a single uppercase letter, call it an initial,
/// and refuse to end the sentence — which is every temperature answer this bot gives.
fn after_abbreviation(chars: &[char], at: usize) -> bool {
    let mut end = at;
    // A `)` or quote that closed the abbreviation: `(e.g.)`.
    while end > 0 && matches!(chars[end - 1], ')' | ']' | '"' | '\'') {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    let word: String = chars[start..end].iter().collect();
    let lower = word
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .to_ascii_lowercase();
    let lower = lower.trim_end_matches('.');
    if ABBREVIATIONS.contains(&lower) {
        return true;
    }
    // A lone capital letter with whitespace (or the start) behind it: `J.` in `J. K. Rowling`.
    let mut cs = lower.chars();
    match (cs.next(), cs.next()) {
        (Some(c), None) => {
            c.is_ascii_alphabetic()
                && chars
                    .get(start)
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
                && (start == 0 || chars[start - 1].is_whitespace())
        }
        _ => false,
    }
}

/// The last whitespace at or before 80 % of the limit — a clause boundary, so a forced split
/// lands somewhere a listener would have paused anyway.
fn clause_boundary(chars: &[char]) -> Option<usize> {
    let limit = chars.len().min(MAX_UNIT_CHARS);
    let mut last = None;
    for (i, c) in chars.iter().enumerate().take(limit) {
        if c.is_whitespace() {
            last = Some(i + 1);
        }
    }
    last.filter(|n| *n >= MIN_JOIN_WORDS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(parts: &[&str]) -> Vec<String> {
        let mut s = Splitter::new();
        let mut out = Vec::new();
        for p in parts {
            out.extend(s.push(p));
        }
        out.extend(s.flush());
        out
    }

    #[test]
    fn a_two_word_answer_is_one_unit_not_two_piper_calls() {
        assert_eq!(
            feed(&["Yes. ", "It is open since 18:40."]),
            vec!["Yes. It is open since 18:40."]
        );
    }

    /// The one that matters most on this bot's answers: a temperature is a number, and a split
    /// after `12` turns it into two.
    #[test]
    fn a_decimal_point_is_not_the_end_of_a_sentence() {
        let units = feed(&["It is 12.2 °C in the workshop and 43.9 % humidity."]);
        assert_eq!(units.len(), 1, "{units:?}");
        assert!(units[0].contains("12.2"));
        // Two sentences, and the second starts at a real boundary.
        let units = feed(&["It is 12.2 °C. The workshop is 19.4 °C."]);
        assert_eq!(units.len(), 2, "{units:?}");
        assert_eq!(units[0], "It is 12.2 °C.");
    }

    #[test]
    fn an_abbreviation_and_an_initial_do_not_end_a_unit() {
        assert_eq!(
            feed(&["Rooms with sensors, e.g. the workshop, are listed."]),
            vec!["Rooms with sensors, e.g. the workshop, are listed."]
        );
        assert_eq!(
            feed(&["Dr. No said no. Then he left."]),
            vec!["Dr. No said no.", "Then he left."]
        );
    }

    #[test]
    fn bullets_are_units_whatever_their_length() {
        assert_eq!(
            feed(&["- 18 °C\n- 21 °C\n"]),
            vec!["- 18 °C", "- 21 °C"]
        );
    }

    #[test]
    fn a_terminator_at_the_end_of_a_fragment_is_not_a_boundary_yet() {
        // The fragment ends on `.` and the next one is `2 °C` — the same decimal case, but
        // arriving across a read boundary, which is where splitters get it wrong.
        assert_eq!(feed(&["It is 12.", "2 °C outside."]), vec!["It is 12.2 °C outside."]);
    }

    #[test]
    fn punctuation_with_closing_marks_ends_where_you_heard_it() {
        // Measured behaviour, and the right one: `?` and `.` *inside* quotes are content, not
        // boundaries, because the split test looks at what follows the quote and finds a letter.
        // `\"Warm,\" it said.` therefore stays one unit with the sentence around it.
        let units = feed(&["Is it open? \"Warm,\" it said. Done"]);
        assert_eq!(units.len(), 3, "{units:?}");
        assert_eq!(units[0], "Is it open?");
        assert_eq!(units[1], "\"Warm,\" it said.");
        assert_eq!(units[2], "Done");
    }

    #[test]
    fn a_run_on_answer_is_split_rather_than_held_forever() {
        let long = "the space is open and the temperature is fine and the door code is on the wiki and ".repeat(6);
        let units = feed(&[long.as_str()]);
        assert!(units.len() > 1, "a run-on answer was never split");
        assert!(
            units.iter().all(|u| u.chars().count() <= MAX_UNIT_CHARS + 40),
            "{:?}",
            units.iter().map(|u| u.chars().count()).collect::<Vec<_>>()
        );
        // Nothing was lost or duplicated: the units are the answer, in order.
        let joined: String = units.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
        let want: String = long.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(joined, want);
    }

    #[test]
    fn every_character_arrives_exactly_once_whatever_the_fragment_sizes() {
        let answer = "The space is open. It has been open since 18:40 on Tue 17 Nov 2026. \
                      The workshop is 19.4 °C, e.g. warmer than the pi room, which is 17.1 °C.";
        for size in [1usize, 3, 7, 40] {
            // Split by *characters*: a 1-byte fragment of a two-byte `°` is a replacement
            // character, and a test that feeds the splitter mojibake learns nothing about it.
            let chars: Vec<char> = answer.chars().collect();
            let parts: Vec<String> = chars.chunks(size).map(|c| c.iter().collect()).collect();
            let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
            let units = feed(&refs);
            let got: String = units.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
            let want = answer.split_whitespace().collect::<Vec<_>>().join(" ");
            assert_eq!(got, want, "fragment size {size} lost or reordered text");
        }
    }

    #[test]
    fn an_empty_fragment_and_a_blank_answer_speak_nothing() {
        assert!(feed(&[""]).is_empty());
        assert!(feed(&["   ", "\n  "]).is_empty());
    }

    #[test]
    fn a_multibyte_character_does_not_move_the_cut() {
        assert_eq!(
            feed(&["Workshop 19.4 °C. ", "Pi room 17.1 °C. "]),
            vec!["Workshop 19.4 °C.", "Pi room 17.1 °C."]
        );
        assert_eq!(feed(&["— open, — closed."]), vec!["— open, — closed."]);
    }
}
