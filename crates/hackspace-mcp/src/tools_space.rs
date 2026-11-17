//! The hackspace Space API — the one upstream that answers most of "what is the
//! building doing", and the same one Vulcan read.
//!
//! It is a single 2 KiB document (`space.json`) carrying the open/closed state,
//! seven temperature and seven humidity sensors, the two 3D printers and the
//! member count, all pushed into it by whatever publishes to it on `apps1`. Two
//! things about it shaped these tools:
//!
//! - **It is already curated.** The rooms are named the way people in the space
//!   name them (`Main Space`, `Fabrication`, `Pi Room`, `Rack 1`), so the tool's
//!   job is to make it shorter and to do the arithmetic, not to translate it.
//! - **Its sensor arrays are keyed by type and hold `location`s**, so "which room
//!   is coldest" is a real question here and answering it in the tool is what
//!   stops a quantised model doing the arithmetic in prose and getting it wrong.

use crate::args;
use crate::clock::{self, ago_text, utc_text};
use crate::sources::Sources;
use crate::tools::{clamp_usize, join_short, register};
use common_json::Json;
use common_mcp::{object_schema, param};
use std::sync::Arc;

pub fn register_tools(srv: &mut common_mcp::Server, src: &Arc<Sources>) {
    register(
        srv,
        "space_status",
        "The hackspace building, live: whether it is open now and since when, every room's \
         temperature and humidity, the two 3D printers, and how many members are subscribed. \
         Temperatures are Celsius and this is an old stone mill with no heating to speak of, so \
         10-14 C is genuinely cold, 14-18 C is cool, and anything above 20 C is warm here. Rooms: \
         Main Space, Workshop, Fabrication, Craft Space, Dark Room, Pi Room, Rack 1. Call this \
         for any question about the space being cold, hot, open, closed, or how many members we \
         have; it is one call and it answers all of those. \
         For a one-line answer to a single question, pass focus: \"open\" for open/closed, \
         \"coldest\" or \"warmest\" for which room, or a room name such as \"workshop\" or \"cnc\"
         for that room alone. With no focus it returns the whole picture, which is what an \
         answer that has to make a judgement wants.",
        object_schema(
            vec![(
                "focus",
                param("string", "Optional: \"open\", \"coldest\", \"warmest\", or one room name. Returns one sentence about just that."),
            )],
            Vec::new(),
        ),
        src,
        space_status,
    );

    register(
        srv,
        "space_events",
        "Upcoming events on the hackspace calendar (the public calendar and the members \
         calendar). Times are the event's own, as published. Use this for \"what's on\", \
         \"is there a social tonight\", \"when is the CTF\".",
        object_schema(
            vec![
                (
                    "days",
                    param("integer", "How many days ahead to list. Default 7, maximum 60."),
                ),
                (
                    "calendar",
                    param(
                        "string",
                        "public, members or both. Default both. Public is events open to anyone.",
                    ),
                ),
            ],
            Vec::new(),
        ),
        src,
        space_events,
    );
}

/// The sensor arrays are `[{value, location, unit}]` for temperature and humidity
/// and `[{name, state, lastchange}]` for printers, so one helper reads a row and
/// the callers say which key holds the label.
fn sensor_rows<'a>(doc: &'a Json, group: &str, label_key: &str) -> Vec<(&'a Json, String)> {
    doc.at(&["sensors", group])
        .and_then(|s| s.as_array())
        .map(|rows| {
            rows.iter()
                .map(|r| {
                    (
                        r,
                        r.get(label_key)
                            .and_then(|v| v.as_str())
                            .unwrap_or("(unnamed)")
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One sentence, from the state and temperatures it was handed. **Pure on purpose**: it is
/// the only way to test what a person is told about their own building without a network.
///
/// Two formatting decisions are load-bearing, because this string is read **aloud** as well
/// as posted:
///
/// * **no absolute dates.** `2026-11-17 15:34` is read by the speech front end with the word
///   "dash" in it (measured; `docs/follow-ups.md`), so the focused answer gives a time of day
///   and a relative span, and past a day it says "2 days ago". Nothing to misread either way.
/// * **no bare unit letters.** "12.1 C" is read as "twelve point one cee". The voice prompt
///   tells the model to write units as words, and a tool answering without a model has the
///   same obligation to the same listener. Hence "degrees".
fn focused(
    state: &str,
    since: Option<f64>,
    now: f64,
    temps: &[(String, Option<f64>)],
    focus: &str,
) -> Result<String, String> {
    let f = focus.trim().to_lowercase().replace(['_', '-'], " ");
    let f = f.split_whitespace().collect::<Vec<_>>().join(" ");
    let aged = since.map(|s| ago_text(now - s)).unwrap_or_else(|| "a while".into());
    let since_phrase = match since {
        Some(s) if now - s < 86_400.0 => format!(
            ", since {} UTC ({aged})",
            utc_text(s).split(' ').nth(1).unwrap_or("??")
        ),
        Some(_) => format!(", for {aged}"),
        None => String::new(),
    };
    if matches!(
        f.as_str(),
        "open" | "opened" | "closed" | "close" | "status" | "state" | "now" | "space"
    ) {
        return Ok(format!("Leigh Hackspace is {state}{since_phrase}."));
    }
    let values: Vec<(&String, f64)> = temps
        .iter()
        .filter_map(|(l, v)| v.map(|v| (l, v)))
        .collect();
    let rank = |worst: bool| -> Result<String, String> {
        let Some((loc, v)) = (if worst {
            values.iter().min_by(|a, b| a.1.total_cmp(&b.1))
        } else {
            values.iter().max_by(|a, b| a.1.total_cmp(&b.1))
        }) else {
            return Err("the space API gave no temperature to rank".into());
        };
        Ok(format!(
            "{loc} is the {} room at {} degrees.",
            if worst { "coldest" } else { "warmest" },
            crate::sources::value(*v)
        ))
    };
    if values.is_empty() {
        return Err("the space API returned no temperatures to answer that from".into());
    }
    if f.starts_with("cold") || f.starts_with("cool") || f.starts_with("freez") || f == "chilly" {
        return rank(true);
    }
    if f.starts_with("warm") || f.starts_with("hot") {
        return rank(false);
    }
    if f.is_empty() || matches!(f.as_str(), "rooms" | "room" | "spread" | "extremes" | "which room") {
        let min = values.iter().min_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        let max = values.iter().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        return Ok(format!(
            "{coldest} is the coldest at {} degrees and {warmest} the warmest at {}, a spread              of {} degrees across {} rooms.",
            crate::sources::value(min.1),
            crate::sources::value(max.1),
            crate::sources::value(max.1 - min.1),
            values.len(),
            coldest = min.0,
            warmest = max.0,
        ));
    }
    // A room, matched loosely in both directions: the API's location is `main_space` and a
    // person types "main space", and neither of those is the other one's mistake.
    match values.iter().find(|(loc, _)| {
        let l = loc.to_lowercase().replace(['_', '-'], " ");
        l.contains(&f) || f.contains(&l)
    }) {
        Some((loc, v)) => Ok(format!(
            "{loc} is {} degrees.",
            crate::sources::value(*v)
        )),
        // An **error**, not an answer: "I cannot see a room called aibox" is a refusal, and
        // the harness fast path treats a refusal as its cue to ask the model instead — the
        // model may know aibox is a server and reach for a different tool. A model that
        // passes a bad focus also gets the list of what does exist, which it can correct.
        None => Err(format!(
            "no room called \"{focus}\". The temperatures I can see are: {}",
            join_short(&temps.iter().map(|(l, _)| l.clone()).collect::<Vec<_>>(), 12)
        )),
    }
}

fn space_status(src: &Sources, arguments: &Json) -> Result<String, String> {
    let doc = src.fetch_json(&format!("{}/space.json", src.space))?;

    let open = doc.at(&["state", "open"]).and_then(|v| v.as_bool());
    let state = match open {
        Some(true) => "OPEN",
        Some(false) => "CLOSED",
        // Not "closed": a space API that answers without a state is a different
        // fact from one that says closed, and the bot should not be able to tell
        // someone the door is locked on the strength of a missing field.
        None => "UNKNOWN (the space API gave no open/closed field)",
    };
    let since = doc.at(&["state", "lastchange"]).and_then(|v| v.as_f64());
    let now = clock::now_epoch();
    let mut out = format!("Leigh Hackspace is {state}");
    if let Some(since) = since {
        out.push_str(&format!(", since {} ({})", utc_text(since), ago_text(now - since)));
    }
    out.push('\n');

    // Temperature, with the two derived facts a person asks for next.
    let temps = sensor_rows(&doc, "temperature", "location");

    // **One question, one sentence, no model.** `focus` exists so the harness can answer the
    // questions that need no judgement — "is it open", "how warm is the workshop" — straight
    // from this call, in tens of milliseconds instead of the 5-18 s of engine the same
    // question costs today (`docs/slack-agent-2026-11-16.md` §15 is the measurement that sent
    // us here). The sentence is rendered **here** rather than by the caller for the same
    // reason the calendar prose is: the digest, the voice answer and the Slack answer must
    // not each own a copy of "how we say the temperature of a room".
    if let Some(focus) = args::str_arg(arguments, "focus") {
        let rows: Vec<(String, Option<f64>)> = temps
            .iter()
            .map(|(row, loc)| ((*loc).clone(), row.get("value").and_then(|v| v.as_f64())))
            .collect();
        return focused(state, since, now, &rows, &focus);
    }
    if temps.is_empty() {
        out.push_str("Temperature: the API returned no temperature sensors\n");
    } else {
        let listed: Vec<String> = temps
            .iter()
            .map(|(row, loc)| {
                let n = row.get("value").and_then(|v| v.as_f64());
                let unit = row.get("unit").and_then(|v| v.as_str()).unwrap_or("C");
                format!("{loc} {}", n.map(|n| crate::sources::with_unit(n, unit)).unwrap_or_else(|| "?".into()))
            })
            .collect();
        out.push_str(&format!("Temperature: {}\n", join_short(&listed, 12)));
        let with_values: Vec<(f64, &String)> = temps
            .iter()
            .filter_map(|(row, loc)| row.get("value").and_then(|v| v.as_f64()).map(|n| (n, loc)))
            .collect();
        if let (Some(min), Some(max)) = (
            with_values.iter().min_by(|a, b| a.0.total_cmp(&b.0)),
            with_values.iter().max_by(|a, b| a.0.total_cmp(&b.0)),
        ) {
            out.push_str(&format!(
                "  coldest: {loc} {v} · warmest: {wloc} {wv} · spread {} C\n",
                crate::sources::value(max.0 - min.0),
                loc = min.1,
                v = crate::sources::value(min.0),
                wloc = max.1,
                wv = crate::sources::value(max.0),
            ));
        }
    }

    let hums = sensor_rows(&doc, "humidity", "location");
    if !hums.is_empty() {
        let listed: Vec<String> = hums
            .iter()
            .map(|(row, loc)| {
                let n = row.get("value").and_then(|v| v.as_f64());
                format!("{loc} {}", n.map(crate::sources::value).unwrap_or_else(|| "?".into()))
            })
            .collect();
        out.push_str(&format!("Humidity %: {}\n", join_short(&listed, 12)));
    }

    let printers = sensor_rows(&doc, "ext_3d_printers", "name");
    if !printers.is_empty() {
        let listed: Vec<String> = printers
            .iter()
            .map(|(row, name)| {
                let st = row.get("state").and_then(|v| v.as_str()).unwrap_or("?");
                match row.get("lastchange").and_then(|v| v.as_f64()) {
                    Some(t) => format!("{name} {st} ({} ago)", ago_text(clock::now_epoch() - t)),
                    None => format!("{name} {st}"),
                }
            })
            .collect();
        out.push_str(&format!("3D printers: {}\n", join_short(&listed, 8)));
    }

    if let Some(rows) = doc.at(&["sensors", "total_member_count"]).and_then(|v| v.as_array()) {
        if let Some(first) = rows.first() {
            if let Some(n) = first.get("value").and_then(|v| v.as_f64()) {
                out.push_str(&format!(
                    "Members: {} {}\n",
                    crate::sources::value(n),
                    first.get("name").and_then(|v| v.as_str()).unwrap_or("subscribed")
                ));
            }
        }
    }

    Ok(out.trim_end().to_string())
}

fn space_events(src: &Sources, arguments: &Json) -> Result<String, String> {
    let days = args::int_arg_default(arguments, "days", 7, 1, 60)?;
    let calendar = args::str_arg(arguments, "calendar").unwrap_or_else(|| "both".to_string());
    let calendars: Vec<&str> = match calendar.as_str() {
        "public" => vec!["public"],
        "members" => vec!["members"],
        "both" => vec!["public", "members"],
        other => {
            return Err(format!(
                "argument \"calendar\" must be public, members or both, got {other}"
            ))
        }
    };

    let now = clock::now_epoch();
    let until = now + days as f64 * 86_400.0;
    let mut rows: Vec<Row> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut fetched = 0usize;

    for cal in calendars {
        let url = format!("{}/events?calendar={cal}", src.space);
        let parts = match src.fetch_json(&url) {
            Ok(Json::Arr(rows)) => rows,
            Ok(_) => return Err(format!("{url}: expected an array of events")),
            // One calendar being down is a partial answer, not a failed tool.
            Err(e) => {
                errors.push(format!(
                    "  (the {cal} calendar did not answer: {})",
                    e.lines().next().unwrap_or("")
                ));
                continue;
            }
        };
        fetched += 1;
        for ev in parts {
            let Some(start_raw) = ev
                .at(&["start", "dateTime"])
                .or_else(|| ev.at(&["start", "date"]))
                .and_then(|v| v.as_str())
            else {
                continue;
            };
            let Some(start) = clock::iso_epoch(start_raw) else {
                continue;
            };
            // The API returns a rolling window that includes the past; the caller
            // asked what is *coming*.
            if start < now || start > until {
                continue;
            }
            // The event's own published wall clock, not ours: `18:30+00:00` is "half six"
            // to the person reading this list, and converting to UTC would print a
            // different number the week the clocks change.
            let wall = clock::wall(start_raw);
            let date = wall
                .as_ref()
                .map(|w| w.date.clone())
                .unwrap_or_else(|| clock::utc_text(start)[..10].to_string());
            let offset = wall.as_ref().map(|w| w.offset).unwrap_or(0);
            let when = match wall.as_ref().and_then(|w| w.time.clone()) {
                None => "all day".to_string(),
                Some(t0) => {
                    let end = ev
                        .at(&["end", "dateTime"])
                        .or_else(|| ev.at(&["end", "date"]))
                        .and_then(|v| v.as_str())
                        .and_then(clock::iso_epoch);
                    match end {
                        // Same published day → "18:30–21:30"; crossing midnight or an
                        // all-day end → the far date too, because "18:00-02:00" on a
                        // Saturday is a different plan than "18:00-02:00 Sunday".
                        Some(e) if clock::civil_text(e, offset)[..10] == date => {
                            format!("{t0}–{}", &clock::civil_text(e, offset)[11..16])
                        }
                        Some(e) => format!("{t0}–{} ({})", &clock::civil_text(e, offset)[11..16], short_date(&clock::civil_text(e, offset)[..10], offset)),
                        None => t0,
                    }
                }
            };
            rows.push(Row {
                epoch: start,
                date,
                offset,
                when,
                summary: ev
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(untitled)")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                cal: cal_tag(ev.get("calendar").and_then(|v| v.as_str()).unwrap_or(cal)),
            });
        }
    }
    // The one thing the old bot never did: it listed events in whatever order the API
    // returned them, so "Monday's social" could appear under Thursday. Sorting is on the
    // absolute instant, which is the only order that is correct across offsets.
    rows.sort_by(|a, b| a.epoch.total_cmp(&b.epoch));
    let shown = clamp_usize(args::int_arg(arguments, "limit", 1, 50).unwrap_or(None), 20, 50);
    if rows.is_empty() {
        let mut none = format!(
            "No upcoming events in the next {days} day{} on the {} calendar.",
            if days == 1 { "" } else { "s" },
            calendar
        );
        if fetched == 0 {
            none.push_str(" No calendar answered, so this is unknown rather than empty.");
        }
        if !errors.is_empty() {
            none.push('\n');
            none.push_str(&errors.join("\n"));
        }
        return Ok(none);
    }

    let mut out = format!(
        "Upcoming events — next {days} day{} on the {} calendar, soonest first ({} found):\n",
        if days == 1 { "" } else { "s" },
        calendar,
        rows.len()
    );
    let mut day: Option<String> = None;
    for row in rows.iter().take(shown) {
        if day.as_deref() != Some(row.date.as_str()) {
            day = Some(row.date.clone());
            out.push_str(&format!("\n{}\n", day_heading(&row.date, row.offset, now)));
        }
        let tag = if calendar == "both" { format!("  [{}]", row.cal) } else { String::new() };
        out.push_str(&format!("  {:<12} {}{tag}\n", row.when, row.summary));
    }
    if rows.len() > shown {
        out.push_str(&format!("\n(+{} later ones not shown)\n", rows.len() - shown));
    }
    if !errors.is_empty() {
        out.push('\n');
        out.push_str(&errors.join("\n"));
        out.push('\n');
    }
    Ok(out.trim_end().to_string())
}

/// One event, already reduced to what a human reads. Kept as a struct rather than a
/// pre-rendered line because the list groups by day, and grouping over strings is how the
/// old output managed to interleave two days.
struct Row {
    epoch: f64,
    date: String,
    offset: i64,
    when: String,
    summary: String,
    cal: String,
}

/// `Today (Mon 16 Nov)` / `Tomorrow (Tue 17 Nov)` / `Sat 21 Nov`.
///
/// "Today" is computed in the *event's* declared timezone, because that is the day the
/// announcement means: an event at 00:30+00:00 when the space is on +00:00 and a server
/// clock sitting in UTC are the same day, and neither is "yesterday".
fn day_heading(date: &str, offset: i64, now: f64) -> String {
    let short = short_date(date, offset);
    let local_today = clock::civil_text(now, offset);
    let local_tomorrow = clock::civil_text(now + 86_400.0, offset);
    if date == &local_today[..10] {
        format!("Today ({short})")
    } else if date == &local_tomorrow[..10] {
        format!("Tomorrow ({short})")
    } else {
        short
    }
}

/// `Mon 16 Nov` — day before month, because this is a UK hackspace and the calendar it
/// mirrors is written that way. The year is left off on purpose: a seven-day list never
/// spans a year boundary in a way that matters, and a digest headed "Mon 16 Nov 2026" reads
/// like a legal notice.
fn short_date(date: &str, _offset: i64) -> String {
    const MONTHS: [&str; 13] = [
        "", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let Some(wd) = clock::weekday(date) else {
        return date.to_string();
    };
    let (Ok(m), Ok(d)) = (date[5..7].parse::<usize>(), date[8..10].parse::<usize>()) else {
        return date.to_string();
    };
    format!("{wd} {d} {}", MONTHS.get(m).copied().unwrap_or("?"))
}

/// The API's own calendar names are `calendar.public_events` and `calendar.member_events`,
/// which is a database column, not something to show a person. Anything else passes through
/// untouched, because a new calendar is a fact about the world and not a typo to swallow.
fn cal_tag(raw: &str) -> String {
    match raw {
        "calendar.public_events" | "public" | "public_events" => "public".to_string(),
        "calendar.member_events" | "members" | "member_events" => "members".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_focused_answer_is_one_sentence_about_the_thing_asked() {
        let temps = vec![
            ("workshop".to_string(), Some(12.1)),
            ("main_space".to_string(), Some(15.4)),
            ("cnc".to_string(), Some(13.0)),
            ("sensor_down".to_string(), None),
        ];
        let now = 1_794_900_000.0;
        let since = now - 48.0 * 60.0;

        let open = focused("OPEN", Some(since), now, &temps, "open").unwrap();
        assert!(open.starts_with("Leigh Hackspace is OPEN, since "), "{open}");
        assert!(open.contains("48m ago"), "{open}");
        // A time of day, never a date: the speech front end says "dash".
        assert!(!open.contains("2026-"), "{open}");

        assert_eq!(
            focused("CLOSED", Some(since), now, &temps, "workshop").unwrap(),
            "workshop is 12.1 degrees."
        );
        // The API writes `main_space`; a person writes "main space". Both find it.
        assert_eq!(
            focused("OPEN", None, now, &temps, "main space").unwrap(),
            "main_space is 15.4 degrees."
        );
        assert_eq!(
            focused("OPEN", None, now, &temps, "coldest").unwrap(),
            "workshop is the coldest room at 12.1 degrees."
        );
        assert_eq!(
            focused("OPEN", None, now, &temps, "warmest").unwrap(),
            "main_space is the warmest room at 15.4 degrees."
        );
        let spread = focused("OPEN", None, now, &temps, "").unwrap();
        assert!(spread.contains("3.3 degrees across 3 rooms"), "{spread}");
        // A sensor with no value is not a room at zero degrees.
        assert!(!spread.contains("4 rooms"), "{spread}");
    }

    #[test]
    fn a_room_that_is_not_there_says_what_is_rather_than_guessing() {
        let temps = vec![
            ("workshop".to_string(), Some(12.1)),
            ("cnc".to_string(), Some(13.0)),
        ];
        let e = focused("OPEN", None, 1.0, &temps, "woodshop").unwrap_err();
        assert!(e.contains("no room called \"woodshop\""), "{e}");
        assert!(e.contains("workshop") && e.contains("cnc"), "{e}");
        // And no temperatures at all is a sentence the caller can read, not an empty answer.
        assert!(focused("OPEN", None, 1.0, &[], "coldest").is_err());
    }

    #[test]
    fn a_day_old_reads_as_a_span_and_not_a_clock_time() {
        let temps = vec![("workshop".to_string(), Some(12.1))];
        let now = 1_794_900_000.0;
        let r = focused("CLOSED", Some(now - 2.5 * 86_400.0), now, &temps, "open").unwrap();
        assert!(r.contains("for 2d"), "{r}");
        assert!(!r.contains("UTC"), "{r}");
    }
}
