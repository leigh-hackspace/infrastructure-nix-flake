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
         have; it is one call and it answers all of those.",
        object_schema(Vec::new(), Vec::new()),
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

fn space_status(src: &Sources, _args: &Json) -> Result<String, String> {
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
    let mut out = format!("Leigh Hackspace is {state}");
    if let Some(since) = doc.at(&["state", "lastchange"]).and_then(|v| v.as_f64()) {
        out.push_str(&format!(", since {} ({})", utc_text(since), ago_text(clock::now_epoch() - since)));
    }
    out.push('\n');

    // Temperature, with the two derived facts a person asks for next.
    let temps = sensor_rows(&doc, "temperature", "location");
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
