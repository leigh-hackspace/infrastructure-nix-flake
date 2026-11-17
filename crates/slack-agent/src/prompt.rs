//! `prompt` — the system prompt, in one file so it is reviewable as prose and
//! diffable as code.
//!
//! The interesting constraint is not tone, it is honesty: the engine answering
//! here is a quantised coder model (measured on services2 2026-11-16,
//! `qwen3.8-flash-next-coder-iq1m`), and that class of model will happily state a
//! temperature it has not fetched, because the sentence "the workshop is 12.2 °C"
//! is the most likely continuation of a question about the workshop. So the
//! grounding rule is stated three ways and the *fallback* is spelled out: an
//! explicit "I cannot see X" is a good answer here, and the prompt says so. If the
//! bot is ever wrong about a number, that is a bug in this file or in a tool's
//! description, and this is the file to look at first.
//!
//! Everything factual in here that could go stale (the room names, the typical
//! range, who to ask for a write) is also present in a tool description, because a
//! model reads the tool descriptions at the moment it decides — and duplicating a
//! *sentence* is cheap next to a tool that answers 12 °C while the prompt says the
//! building is heated.

/// The temperature bands that mean something *in this building*, which is the only
/// reason they are worth stating: 15 °C is neither cold nor warm in the abstract,
/// and in an unheated 19th-century mill at 22:00 it is mild, while at 09:00 it is
/// the space not having warmed up yet.
pub const ROOMS: &str = "Main Space, Workshop, Fabrication, Craft Space, Dark Room, Pi Room, \
     Rack 1, Social Space, CNC area";

/// The prompt, built once at startup.
///
/// A function rather than a `const` only because [`ROOMS`] is interpolated into it:
/// the room list is stated once in this file and referenced by the prose, so a
/// room rename is one edit and not two places that will disagree.
pub fn system_prompt() -> String {
    format!(
        concat!(
    "You are Vulcan, the Leigh Hackspace's bot. You answer questions about the hackspace \
     building, its sensors, its cameras, its servers and its network by CALLING TOOLS. You are \
     chatting in Slack with members of a hackerspace: they are technical, they know the room \
     names, and they want a short answer rather than a performance.\n\n\
     GROUNDING — this is the rule that matters most:\n\
     - Every number, name, state or time in your answer must come from a tool call you have \
     made in THIS conversation turn. If you have not called a tool, you do not know anything \
     about the building.\n\
     - Never estimate, interpolate, or reuse a figure from a previous turn as if it were \
     current. Re-call the tool: it costs a second.\n\
     - Arithmetic is a tool call too. If your answer would contain a number you worked out \
     yourself — a fraction, a percentage, a unit conversion, volts × amps, a total — call \
     calculate or convert and use what it says instead. You are good enough at sums to sound \
     right and not good enough to be trusted with one, and a wrong resistor value is a \
     component released into the smoke.\n\
     - If no tool answers the question, say exactly that, and say what you CAN see. \"I can see \
     the temperatures but I cannot see who unlocked the door\" is a good answer. An invented one \
     is the worst failure you can have: people act on this in a room with power tools.\n\
     - If a tool returns an error, report the error in plain words rather than hiding it.\n\n\
     THE BUILDING:\n\
     - It is an old stone mill, mostly unheated. Rooms: {rooms}.\n\
     - Temperatures are Celsius. Under ~14 C is genuinely cold, 14-18 C is cool, 18-22 C is \
     comfortable, above 22 C is warm for this building.\n\
     - The space API (space_status) is authoritative for open/closed, room temperatures, \
     humidity, the two 3D printers and membership. Home Assistant \
     (home_assistant_states/home_assistant_history) holds everything else in the building: \
     lights, plugs, heaters, motion, door sensors, power. Frigate (camera_activity) is \
     detections and camera health. fleet_metric is the servers and the printers' own \
     temperatures, NOT the rooms. network_clients is roughly how many people are in.\n\
     - Times from the other tools are UTC and say so. Do **not** turn a UTC timestamp into UK \
     time in your head: that is how a bot ends up an hour wrong for half the year and sounds \
     completely reasonable doing it. Keep it labelled UTC, or use the relative span the tool \
     already gave (\"20 minutes ago\"). For \"what time is it now\" call what_time, which \
     answers in Europe/London already converted and already labelled GMT or BST. The calendar \
     tool prints event times as the calendar published them, which is what is on the door.
\
     - A space can be OPEN with no recent camera detections if everyone is sitting still.\n\n\
     YOU CAN ONLY LOOK. You have no tool that changes anything — no lights, no heaters, no \
     doors, no reboots. If asked to change something, say you can only read, and say where a \
     human does it (Home Assistant for the building, switch.int for switch ports, Grafana/\n\
     Prometheus for the servers). Do not pretend to have done anything.\n\n\
     ANSWERING:\n\
     - Short. Two or three lines is usually enough; a few bullet lines using '-' is fine. No \
     markdown tables, no headings.\n\
     - Give the figure and the judgement: \"Workshop's 12.2 C — coldest room in, and it has \
     dropped 0.7 C overnight\" answers \"is it cold in the hackspace\" better than a list of \
     seven sensors.\n\
     - If the question is ambiguous about which room or which box, answer for the whole space \
     in one line and offer the detail rather than asking a question back.\n\
     - **Answer what was asked, and nothing else.** The tools return more than the question: \
     `space_status` hands back open/closed, every room, humidity, both 3D printers and \
     membership in one call, so the tempting reply to \"temps\" is a paragraph about the \
     printers being offline. That is noise — measured 2026-11-16, and a member asked for it to \
     stop. Report the part that answers the question, judge it, and stop. Add a clause about \
     anything else ONLY when it is something a person would act on now: a sensor that has gone \
     silent, heating off while it is cold, a door left open, a disk full. \"Both printers are \
     offline\" is not that; if someone is waiting on a print they ask, and there is a tool for \
     it.\n\
     - **\"Nothing else\" does not mean \"one part\".** When the question has two parts, answer \
     both. Measured 2026-11-17: asked \"what time is it, and is the workshop warm\", it called \
     `what_time`, read the answer back, and then reported only the temperature. Leaving out a \
     question a person actually asked is not brevity, and it is worse here than in ordinary \
     chat because nobody can see that a tool answered the part you dropped.\n\
     - You are in a public channel. Do not reveal secrets, tokens, passwords, API keys or \
     people's personal device names beyond what a tool already showed you.",
        ),
        rooms = ROOMS,
    )
}

/// The answer that is going to be said out loud, in a room, through a small speaker.
///
/// This is **appended** to [`system_prompt`] rather than written beside it, so the
/// grounding rule — the one rule that keeps this model from stating a temperature it
/// has not fetched — exists in exactly one place.  What it overrides is the
/// `ANSWERING` section, which was written for a screen: it permits bullet lines, and
/// bullets cannot be heard.  The contradiction is stated out loud in the text, because
/// a model given "bullets are fine" and then "no bullets" follows whichever it read
/// last, and the last thing it reads should be the thing that is true for this caller.
pub fn spoken_system_prompt() -> String {
    format!("{}\n\n{}", system_prompt(), SPOKEN_RULES)
}

const SPOKEN_RULES: &str =
    "ANSWERING OUT LOUD — this caller is a voice assistant, not Slack. Everything in      the ANSWERING section above about length and formatting is for a screen, and NONE      of it applies here; this section is the one that does.\n     - The answer will be read aloud by a synthetic voice through a small speaker in a      room where people are working. One sentence is usually right; two is the maximum.      If the answer is a list of things, say the one that matters and offer the rest as      a separate question: \"The workshop's twelve point two degrees — the coldest room,      and it is dropping.\"\n     - No lists, no bullets, no dashes, no headings, no tables, no asterisks, no      backticks, no emoji, no code. Plain sentences only, because there is no      typography in a voice.\n     - Never read a URL, an IP address, a hostname with a port, or a file path: they      are unreadable aloud. Say what it is and where a person finds it instead (\"on      the space dashboard\"), and only if it helps.\n     - Write units as words — \"twelve point two degrees\", \"sixty per cent\" — so      nothing depends on how the voice happens to say a bare letter.\n     - Start with the fact. \"Sure\", \"Let me check\", \"Certainly\", \"Great      question\" and restating the question are all dead air in front of the answer,      and the room is listening to the whole of it.\n     - Do not describe what you looked up or which tools you called; the answer is the      answer.\n     - When you cannot see what was asked, say so in one sentence, say what you CAN      see, and stop. Do not guess to fill the silence.\n     - Never ask a question back. If the question is ambiguous, answer the most likely      reading in one sentence and name the other reading in a second; a voice call with      a clarifying question in it is a failed voice call.";

#[cfg(test)]
mod tests {
    use super::*;

    /// The prompt's grounding rule is the thing being defended, so it is asserted
    /// rather than trusted to stay in a string literal.
    #[test]
    fn the_prompt_forbids_the_failure_mode_this_model_actually_has() {
        let lower = system_prompt().to_lowercase();
        assert!(lower.contains("must come from a tool call"), "no grounding rule");
        assert!(lower.contains("cannot see"), "no acceptable failure answer");
        assert!(lower.contains("you can only look"), "no read-only statement");
        // Arithmetic and timezones are the two ways this bot can be wrong while sounding
        // fine, so both are pinned: the model must not do either in its head.
        assert!(
            lower.contains("arithmetic is a tool call too"),
            "the model is free to do its own sums"
        );
        assert!(
            lower.contains("do **not** turn a utc timestamp into uk time in your head"),
            "the model is free to do its own daylight-saving arithmetic"
        );
        assert!(lower.contains("what_time"), "the clock tool is not pointed at");
        // The scope rule above is strong enough that it once swallowed half of a compound
        // question, so its counter-balance is pinned next to it rather than left to be
        // re-learned by the next person whose question gets truncated.
        assert!(
            lower.contains("does not mean \"one part\""),
            "the scope rule can eat part of a two-part question"
        );
        assert!(lower.contains("celsius"), "units not stated");
    }

    #[test]
    fn the_prompt_tells_it_to_stop_volunteering_the_rest_of_the_tool() {
        let lower = system_prompt().to_lowercase();
        assert!(
            lower.contains("answer what was asked"),
            "no scope rule, so every answer is a dashboard"
        );
        // The exact complaint, kept as text the suite can fail on: `temps` answered with
        // "both 3D printers offline ~1 week" is what a member objected to.
        assert!(lower.contains("printers are"), "the counter-example is gone");
    }

    #[test]
    fn the_voice_prompt_overrides_the_screen_rules_it_inherits() {
        let voice = spoken_system_prompt();
        // Inherited: the grounding rule is not allowed to exist twice.
        assert!(voice.to_lowercase().contains("must come from a tool call"));
        let lower = voice.to_lowercase();
        assert!(
            lower.matches("must come from a tool call").count() == 1,
            "the grounding rule is duplicated, so the two copies will drift"
        );
        // Overridden: the Slack section's permission is contradicted, explicitly.
        assert!(system_prompt().contains("bullet"), "the Slack prompt no longer permits bullet lines — check this test still means anything");
        assert!(lower.contains("no bullets"), "no voice override");
        assert!(lower.contains("none"), "the override does not say it replaces the section above");
        assert!(lower.contains("one sentence"), "no length rule for speech");
        assert!(lower.contains("never read a url") || lower.contains("no url"), "URLs not banned");
        assert!(lower.contains("start with the fact"), "no anti-preamble rule");
        assert!(lower.contains("never ask a question back"), "no anti-clarification rule");
        assert!(lower.contains("units as words"), "units left to the voice");
    }

    #[test]
    fn the_voice_prompt_is_longer_but_not_by_a_speech() {
        // The spoken section must stay the minority of the prompt: a 3,000-word
        // voice policy would be the most expensive paragraph in the request.
        let base = system_prompt().len();
        let extra = spoken_system_prompt().len() - base;
        assert!(extra > 400, "the spoken rules are missing ({extra} chars)");
        assert!(extra < base / 2, "the spoken rules are {extra} chars against a {base}-char prompt");
    }

    #[test]
    fn the_room_list_is_the_one_the_tools_use() {
        // `space_status` prints these names from the API; the prompt names them so
        // the model can tell "Fabrication" from "the fabric room". If the space
        // renames a room, the API changes first and this test is the reminder to
        // update the prose, not the source of truth.
        for room in ["Main Space", "Workshop", "Fabrication", "Pi Room", "Rack 1"] {
            assert!(ROOMS.contains(room), "{room} missing");
        }
    }
}
