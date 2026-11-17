//! Should we answer this message at all?
//!
//! Vulcan answered **unmentioned, in whatever channel the words were typed in** — `temps`,
//! `status`, `members` — and that is the behaviour being asked for here ("always answer on
//! the channel the request is made from, just like the old implementation"). It worked
//! because Vulcan matched a fixed vocabulary: a keyword that did not match produced silence,
//! so a busy `#general` cost nothing.
//!
//! An LLM has no such natural floor. Asked nothing, it still answers — so "answer everywhere"
//! without a gate turns the bot into a participant in every conversation, and the fix for that
//! is never "make the prompt shyer". So the dial moves from *which channel* to **is this a
//! request**: a message is answered when it is addressed to us (mention, DM) or when it reads
//! like someone asking the hackspace something. Boring, deterministic, and — deliberately —
//! cheap enough to run on every frame.
//!
//! What it must not do is answer a human asking humans a question. "does anyone have a spare
//! ethernet cable" ends in a `?` and is about the space; a bot replying to that is the exact
//! noise this file exists to prevent, so there is a veto list below, and it is a *narrow* one
//! ("does anyone have", not "anyone") because `is anyone in the hackspace right now?` is the
//! single most likely thing to be asked of this bot and it contains the word "anyone".

/// Every word is matched against lowercased, trimmed text with Slack's control tokens
/// already stripped by `slack::parse_event`.
pub struct Triage {
    /// Words that make a message a request on their own, wherever they appear. Topic words,
    /// not verbs: the hackspace, its rooms, its kit, its data.
    topics: &'static [&'static str],
    /// The same idea, but matched as **whole words only** — because these are short enough to
    /// live inside other English words ("at**temp**t", "rea**state**"), and a gate that fires on
    /// a substring is a gate that interrupts a conversation about something else. This is also
    /// where Vulcan's command vocabulary now lives: `apps1:vulcan-bot.service` was stopped on
    /// 2026-11-16, so someone typing `status` at the space must get an answer from *this* bot.
    whole_words: &'static [&'static str],
    /// Openers that read as a question put to the room at large rather than to us.
    human_addressed: &'static [&'static str],
    /// A message longer than this many words is prose, not a question.
    max_words: usize,
    /// Defer Vulcan's bare command words to it.  **Off by default**: `vulcan-bot` was
    /// stopped and masked on `apps1` on 2026-11-16, so the new bot owns `temps`, `status`,
    /// `events` and the rest.  Turn it on (`--defer-vulcan-words`) if the old bot is ever
    /// connected to the same Slack app again, because Slack duplicates events to both
    /// consumers and those words would then be answered twice.
    pub defer_vulcan_words: bool,
}

impl Default for Triage {
    fn default() -> Self {
        Triage {
            whole_words: &[
                "temp",
                "temps",
                "temperature",
                "status",
                "state",
                "open",
                "closed",
                "humidity",
                "members",
                "events",
                "printer",
                "printers",
            ],
            topics: &[
                // the place itself
                "hackspace",
                "hack space",
                "the space",
                "the workshop",
                "main space",
                "craft space",
                "fabrication",
                "pi room",
                "dark room",
                "rack 1",
                "front door",
                "door",
                // its state and data
                "temperature",
                "temperatures",
                "temp ",
                "temps",
                "humidity",
                "celsius",
                "centigrade",
                "open?",
                "closed?",
                "is it open",
                "is the space open",
                "members",
                "membership",
                "events",
                "calendar",
                "tonight",
                "this week",
                "this weekend",
                // Deliberately no bare weekday names: "see you monday" and "the fire alarm
                // test is on monday" are statements, and a bot that answers them is the noise
                // this file exists to prevent. A *question* about monday arrives through the
                // opener rule ("what's on monday"), which is the only shape anyone asks it in.
                // (Found by the suite, 2026-11-16: this test's first version asserted the
                // opposite and was wrong.)
                "printer",
                "printers",
                "print queue",
                "3d print",
                "moonraker",
                "klipper",
                "camera",
                "cameras",
                "cam1",
                "cam5",
                "cam7",
                "cam8",
                "cam9",
                "frigate",
                "nas",
                "server",
                "servers",
                "ups",
                "power cut",
                "powercut",
                "wifi",
                "internet",
                "bandwidth",
                "lan",
                "vlan",
                "mqtt",
                "prometheus",
                "grafana",
                "authentik",
                "tailscale",
                "router",
                "switch",
                "leases",
                "who is in",
                "who's in",
                "anyone in",
                "anyone there",
                "anyone at the",
            ],
            human_addressed: &[
                "does anyone have",
                "does anyone still have",
                "has anyone got",
                "has anyone got a",
                "has anyone seen",
                "has anyone touched",
                "can someone",
                "could someone",
                "someone bring",
                "someone's left",
                "someone left",
                "i've left",
                "i left",
                "i'll bring",
                "ill bring",
                "i'm bringing",
                "im bringing",
                "thanks!",
                "thank you",
                "cheers",
                "nice one",
                "lol",
                "brb",
            ],
            max_words: 60,
            defer_vulcan_words: false,
        }
    }
}

impl Triage {
    /// True when we should answer.
    ///
    /// `question` is the message text. The rule, in order:
    ///
    /// 1. nothing to say → no;
    /// 2. addressed to humans → no (checked before the topic rule, see the header);
    /// 3. mentions a hackspace topic → yes;
    /// 4. a short interrogative sentence (`?`, or starting with a question word) → yes, because
    ///    in a channel the bot can see, a bare question is usually meant for the room, and
    ///    Vulcan's users expect it to have an opinion;
    /// 5. otherwise → no.
    pub fn answers(&self, question: &str) -> bool {
        let text = question.trim().to_lowercase();
        if text.is_empty() || !text.chars().any(|c| c.is_alphanumeric()) {
            return false;
        }
        if text.split_whitespace().count() > self.max_words {
            return false;
        }
        if self.human_addressed.iter().any(|h| text.contains(h)) {
            return false;
        }
        if self.defer_vulcan_words && vulcan_command(&text) {
            return false;
        }
        if self.topics.iter().any(|t| text.contains(t)) {
            return true;
        }
        if text
            .split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-')
            .any(|w| self.whole_words.contains(&w))
        {
            return true;
        }
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.len() > 12 {
            // A long question with no topic word is not about the hackspace; it is a
            // conversation we have wandered into.
            return false;
        }
        text.ends_with('?')
            || words.first().is_some_and(|w| {
                matches!(
                    *w,
                    "is" | "are"
                        | "was"
                        | "were"
                        | "do"
                        | "does"
                        | "did"
                        | "can"
                        | "could"
                        | "will"
                        | "would"
                        | "should"
                        | "how"
                        | "what"
                        | "whats"
                        | "what's"
                        | "when"
                        | "where"
                        | "which"
                        | "who"
                        | "whos"
                        | "who's"
                        | "why"
                        | "any"
                        | "tell"
                        | "check"
                        | "list"
                        | "show"
                        | "please"
                )
            })
    }
}

/// "Someone is asking what is on" — and if so, how many days ahead to list.
///
/// This is the **deterministic** events path: no model, no engine, no tokens, no chance of a
/// date being reordered or a title being paraphrased, and it answers in the time it takes to
/// fetch the calendar rather than the ~17 s a two-round model turn takes. It exists because
/// "what's on?" is the single most asked question of a hackspace bot and the only honest way
/// to answer it is from the calendar itself. `vulcan-bot` had this as `@respond_to(r"^events")`
/// — which is to say, it answered anything that *started* with the letters "events", including
/// "events can be tricky sometimes" — so the shape is the same and the matching is not.
///
/// Both directions are pinned by the suite, and the failure modes are asymmetric on purpose: a
/// **false negative** falls through to the model, which will call the same tool and answer
/// anyway (slower, slightly worse, not wrong), while a **false positive** posts a week of
/// calendar into a channel in the middle of a conversation about something else.
/// Words that can appear in "asking for the list" and nowhere else in this domain. A message
/// made only of these is a request; one extra word ("events can be tricky sometimes") is a
/// sentence, and that is the difference between this and `vulcan-bot`'s `^events` regex.
const CALENDAR_WORDS: &[&str] = &[
    "event", "events", "calendar", "agenda", "diary", "schedule", "whats", "what's", "what", "on",
    "the", "a", "an", "any", "anything", "something", "upcoming", "coming", "this", "that",
    "week", "weekend", "tonight", "today", "tomorrow", "month", "next", "please", "show", "list",
    "tell", "check", "us", "me", "we", "got", "have", "do", "does", "is", "are", "there",
    "happening", "going",
];

const INTERROGATIVES: &[&str] = &[
    "what", "whats", "what's", "when", "which", "any", "is", "are", "do", "does", "show", "list",
    "tell", "check", "please",
];

fn only_calendar_words(norm: &str) -> bool {
    norm.split_whitespace()
        .all(|w| CALENDAR_WORDS.contains(&w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')))
}

pub fn events_window(text: &str) -> Option<i64> {
    let norm = text
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if norm.is_empty() || norm.split_whitespace().count() > 14 {
        return None;
    }
    // The room talking to itself about bookings is not asking for the list.
    if ["book", "booked", "booking", "reserve", "reserved", "reservation", "ladder", "thanks"]
        .iter()
        .any(|v| norm.contains(v))
    {
        return None;
    }
    if ["does anyone have", "has anyone got", "can someone", "could someone"]
        .iter()
        .any(|h| norm.contains(h))
    {
        return None;
    }
    // What counts as asking about the calendar. Spoken forms first, because that is what a
    // person types: "whats on" arrives from Slack with the apostrophe already present in one
    // form or another, so both are listed rather than normalised away.
    let noun = [
        "event",
        "events",
        "calendar",
        "agenda",
        "diary",
        "schedule",
        "whats on",
        "what's on",
        "anything on",
        "something on",
        "going on",
        "happening",
    ]
    .iter()
    .any(|n| norm.contains(n));
    if !noun {
        return None;
    }
    // And that it is a question rather than a statement that happens to mention an event.
    let first = norm.split_whitespace().next().unwrap_or("");
    let asked = norm.ends_with('?') || only_calendar_words(&norm) || (INTERROGATIVES.contains(&first) && norm.split_whitespace().count() <= 8);
    if !asked {
        return None;
    }
    Some(if norm.contains("tonight") || norm.contains("today") {
        1
    } else if norm.contains("tomorrow") {
        2
    } else if norm.contains("next week") {
        14
    } else if norm.contains("month") {
        30
    } else {
        7
    })
}

/// The old bot's whole vocabulary, as bare commands.
///
/// Vulcan is still connected to the same Slack app, and an `app_mention` arrives at both
/// consumers: while the overlap lasts, a message that is *nothing but* one of these words is
/// the old bot's contract and answering it twice in the same channel is the failure the user
/// has already been shown. Matching **exact, bare** words is the whole trick — `what are the
/// temps today` is a sentence and stays ours.
///
/// Only consulted when `Triage::defer_vulcan_words` is on, i.e. while both bots are connected;
/// `apps1:vulcan-bot.service` was stopped, disabled, file-removed and masked on 2026-11-16
/// (parked in `/var/lib/systemd/disabled-units/` with its own restore note), so the default is
/// off and these words are ours.
pub fn vulcan_command(text: &str) -> bool {
    let one = text
        .trim()
        .trim_end_matches(['.', '!', ' ', '\u{a0}'])
        .to_lowercase();
    matches!(
        one.as_str(),
        "temps"
            | "temp"
            | "temperature"
            | "status"
            | "state"
            | "open"
            | "closed"
            | "humidity"
            | "members"
            | "member"
            | "member count"
            | "events"
            | "event"
            | "printer"
            | "printers"
            | "3dprinters"
    )
}

#[cfg(test)]
mod tests {
    use super::Triage;

    #[test]
    fn asking_what_is_on_is_answered_without_a_model() {
        for q in [
            "events",
            "events?",
            "events please",
            "what events are on?",
            "whats on",
            "what's on at the hackspace this week",
            "any events tonight",
            "is there anything on tomorrow",
            "show me the calendar",
            "what's happening on saturday?",
        ] {
            let days = super::events_window(q);
            assert!(days.is_some(), "{q:?} should be answered from the calendar");
        }
        assert_eq!(super::events_window("events"), Some(7));
        assert_eq!(super::events_window("any events tonight"), Some(1));
        assert_eq!(super::events_window("is there anything on tomorrow"), Some(2));
        assert_eq!(super::events_window("whats on next week"), Some(14));
        assert_eq!(super::events_window("what events are on this month?"), Some(30));
    }

    #[test]
    fn mentioning_an_event_is_not_asking_for_the_list() {
        // Each of these falls through to the model, which has the same tool; the fast path
        // only fires when it is confident it is *the* question.
        for q in [
            "events can be tricky sometimes",
            "i booked the event space for saturday",
            "who booked the workshop for an event?",
            "does anyone have a table for the event",
            "thanks for the event earlier",
            "is it cold in the hackspace",
            "temps",
        ] {
            assert_eq!(super::events_window(q), None, "{q:?} is not a request for the list");
        }
    }

    fn yes(text: &str) {
        assert!(Triage::default().answers(text), "should answer: {text:?}");
    }
    fn no(text: &str) {
        assert!(!Triage::default().answers(text), "should stay quiet: {text:?}");
    }

    #[test]
    fn it_owns_the_old_commands_now_that_vulcan_is_off() {
        // The words are still in the topic list, so they answer — which is the point: Vulcan
        // stopped 2026-11-16 and muscle memory from nine years of `temps` must keep working.
        for w in ["temps", "status", "events", "printers", "humidity", "members"] {
            yes(w);
        }
    }

    #[test]
    fn vulcan_owns_its_own_command_words_only_while_both_bots_are_live() {
        let defer = Triage {
            defer_vulcan_words: true,
            ..Default::default()
        };
        for w in ["temps", "status", "events", "printers", "humidity", "members"] {
            assert!(!defer.answers(w), "{w} must be deferred while Vulcan is connected");
            assert!(!defer.answers(&format!("{w}.")), "{w}. must be deferred");
            assert!(super::vulcan_command(w), "{w} is a Vulcan command shape");
        }
        // A sentence that merely contains the word is ours even then: the difference between
        // deferring to the old bot and pretending we cannot read.
        yes("what are the temps today");
        yes("is it cold in the hackspace");
        assert!(!super::vulcan_command("what are the temps today"));
        assert!(defer.answers("what are the temps today"));
    }

    #[test]
    fn the_questions_it_exists_to_answer() {
        for q in [
            "is it cold in the hackspace",
            "is the hackspace open?",
            "what's the temperature in the workshop",
            "what are the temps today",
            "how many members do we have",
            "what's on at the hackspace this week",
            "is anyone in the space right now",
            "are the 3d printers busy",
            "has the workshop been cooling all night",
            "which camera last saw someone",
            "how's the nas disk looking",
            "is the door locked",
        ] {
            yes(q);
        }
    }

    #[test]
    fn a_question_is_enough_even_without_a_topic_word() {
        yes("is it busy tonight?");
        yes("how are we doing on filament");
        yes("what time does it close");
    }

    /// The basics tier (`what_time`, `calculate`, `convert`) answers questions that are not
    /// about the building at all, so this gate has to be explicit about which shape reaches it:
    /// an interrogative yes, a bare imperative no. Voice and @-mentions never come through
    /// here, so a `no` only ever declines to barge into a conversation between two people — and
    /// "convert 12 feet to metres" is a thing one member says to another at the bench.
    #[test]
    fn the_basics_tier_is_reachable_by_question_but_not_by_command() {
        yes("what time is it");
        yes("what's 3/8 in mm");
        yes("how many litres is 3 gallons");
        yes("is it the weekend");
        no("convert 12 feet to metres");
        no("calculate the load for the truss");
    }

    #[test]
    fn humans_addressing_humans_get_no_reply() {
        for q in [
            "does anyone have a spare ethernet cable?",
            "has anyone got a soldering iron I can borrow?",
            "someone left a coffee in the workshop",
            "i'll bring the ladder on tuesday",
            "thanks! cheers",
            "nice one, lol",
        ] {
            no(q);
        }
    }

    #[test]
    fn chatter_and_prose_are_not_questions() {
        no("");
        no("   ");
        no("lol nice");
        no("I'll attempt a reprint of that bracket later");
        no("we should restate the bylaws at the AGM");
        no("see you monday");
        no("the fire alarm test is on monday");
        yes("what's on on monday"); // the question form of the same fact
        no(&format!("{}?", "word ".repeat(70)));
        no("and then he said that the whole thing about the thing could maybe possibly perhaps also have been described differently by someone else entirely");
    }
}
