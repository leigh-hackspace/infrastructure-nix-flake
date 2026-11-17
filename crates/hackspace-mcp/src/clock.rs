//! `clock` — epoch seconds ↔ the handful of text forms these tools need.
//!
//! `chrono` is in the workspace's lock file (gocardless-dashboard uses it), so
//! taking it here would cost no *crate*. It still costs a compile unit and, more
//! usefully, it would hide the one thing worth testing in this file: that a
//! timestamp from `space.json`, from Frigate, or from an RFC 3339 calendar entry
//! all land on the same wall clock. That is four small pure functions and their
//! tests; a dependency would make them someone else's problem in a way nobody
//! reads.
//!
//! Everything here is **UTC on purpose** — with one named exception, below.
//!
//! The hackspace is in Europe/London and a
//! reader in the space wants local time, but the boundary where that matters is
//! the *display* of an event's start, and the calendar API already carries its own
//! offset (`2026-11-16T18:30:00+00:00`) — so [`iso_epoch`] honours the offset it is
//! given and [`utc_text`] renders in UTC, labelled as such. A bot that quietly
//! switched to server-local time would print "18:30" and mean 18:30±1 depending on
//! the date, which is the kind of bug that survives forever because the answer is
//! plausible either way.
//!
//! The exception is [`london_offset`], and it exists because "what time is it" is a
//! different question from "when is the CTF": the first asks about **now**, there is
//! no published timestamp to honour, and the alternative is the prompt telling a
//! 30-billion-parameter quantised coder to add an hour in its head whenever the
//! clocks change. So the zone rule is stated once, here, with the transitions tested,
//! and every string it produces is **labelled** (`14:32 BST (UTC+1)`) — because the
//! failure this file warns about is not converting, it is converting silently.

/// Days from the civil epoch (1970-01-01) for a proleptic-Gregorian Y/M/D.
///
/// The usual Howard Hinnant algorithm: it is eleven lines, exact, and has no month
/// table to get wrong.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) as i64 + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// The inverse, for rendering.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `2026-11-16 08:44 UTC`, the form a Slack answer wants.
pub fn utc_text(epoch: f64) -> String {
    let secs = epoch as i64;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        tod / 3600,
        (tod % 3600) / 60
    )
}

/// `2026-11-16T08:44:00+00:00` — the form Home Assistant's history API wants.
pub fn iso_utc(epoch: f64) -> String {
    let secs = epoch as i64;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}+00:00",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

/// "now" as an f64 epoch, in one place so the tools agree on it.
pub fn now_epoch() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// "3d 4h ago", "in 12m", "just now" — a relative span a human reads first.
pub fn ago_text(delta_secs: f64) -> String {
    let future = delta_secs < 0.0;
    let s = delta_secs.abs();
    let body = if s < 90.0 {
        "now".to_string()
    } else {
        let units = [
            (365.0 * 86_400.0, "y"),
            (30.0 * 86_400.0, "mo"),
            (7.0 * 86_400.0, "w"),
            (86_400.0, "d"),
            (3600.0, "h"),
            (60.0, "m"),
        ];
        let mut left = s;
        let mut parts = Vec::new();
        for (size, label) in units {
            if left >= size {
                let n = (left / size) as i64;
                left -= n as f64 * size;
                parts.push(format!("{n}{label}"));
                if parts.len() == 2 {
                    break;
                }
            }
        }
        if parts.is_empty() {
            "now".to_string()
        } else {
            parts.join(" ")
        }
    };
    if body == "now" {
        "just now".to_string()
    } else if future {
        format!("in {body}")
    } else {
        format!("{body} ago")
    }
}

/// Parse the RFC 3339 forms these APIs emit into epoch seconds.
///
/// Accepted: `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM:SS[.fff]`, with a trailing `Z` or a
/// `±HH:MM` offset, and a bare `HH:MM:SS` ignored when the date is absent.
/// Returns `None` rather than guessing, because a silently-mis-parsed event time
/// shows up as the bot telling someone the CTF is on a different day.
pub fn iso_epoch(s: &str) -> Option<f64> {
    let s = s.trim();
    let bytes = s.as_bytes();
    if s.len() < 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month: u32 = s[5..7].parse().ok()?;
    let day: u32 = s[8..10].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut epoch = days_from_civil(year, month, day) as f64 * 86_400.0;
    let rest = &s[10..];
    if rest.is_empty() {
        return Some(epoch);
    }
    // `T`, `t` or a space separate the time from the date; anything else is not a
    // timestamp.
    let sep = rest.as_bytes()[0];
    if !matches!(sep, b'T' | b't' | b' ') {
        return None;
    }
    let mut t = &rest[1..];
    if t.len() < 5 || t.as_bytes()[2] != b':' {
        return None;
    }
    let hh: u32 = t[0..2].parse().ok()?;
    let mm: u32 = t[3..5].parse().ok()?;
    t = &t[5..];
    let mut ss: u32 = 0;
    // Seconds are optional in RFC 3339's `partial-time`; the separator is what
    // tells us they follow.  (The first version of this looked for a colon at the
    // *third* byte of the remainder and so silently dropped every `:SS`, which is
    // how a calendar event lands on the wrong minute.)
    if t.as_bytes().first() == Some(&b':') {
        let body = &t[1..];
        if body.len() < 2 {
            return None;
        }
        ss = body[0..2].parse().ok()?;
        t = &body[2..];
        if t.as_bytes().first() == Some(&b'.') {
            let frac: String = t[1..].chars().take_while(|c| c.is_ascii_digit()).collect();
            if !frac.is_empty() {
                if let Ok(v) = frac.parse::<f64>() {
                    epoch += v / 10f64.powi(frac.len() as i32);
                }
                t = &t[1 + frac.len()..];
            }
        }
    }
    if !(0..=23).contains(&hh) || !(0..=59).contains(&mm) || !(0..=60).contains(&ss) {
        return None;
    }
    epoch += hh as f64 * 3600.0 + mm as f64 * 60.0 + ss as f64;
    // A time without a zone is not something to guess at.  These APIs are the
    // space's own calendar and Home Assistant's recorder, and both always qualify;
    // assuming UTC for a `18:30` that meant Europe/London is exactly the
    // plausible-but-wrong answer that outlives every other kind of bug.
    match t.as_bytes().first() {
        None => None,
        Some(b'Z') | Some(b'z') if t.len() == 1 => Some(epoch),
        Some(&sign @ (b'+' | b'-')) => {
            let body = &t[1..];
            let oh: i64 = body.get(0..2)?.parse().ok()?;
            let om: i64 = match body.as_bytes().get(2) {
                Some(b':') => body.get(3..5)?.parse().ok()?,
                _ if body.len() == 2 => 0,
                _ => return None,
            };
            let off = oh * 3600 + om * 60;
            if !(t.len() == 3 || t.len() == 6) {
                return None;
            }
            Some(if sign == b'+' { epoch - off as f64 } else { epoch + off as f64 })
        }
        Some(_) => None,
    }
}

/// The **published wall clock** of a timestamp: the date and time as the calendar wrote
/// them, plus the offset it claimed.
///
/// This exists because a calendar entry is not a measurement, it is an *announcement*:
/// `2026-11-16T18:30:00+00:00` means "Code Club, half six, come along", and rendering it
/// through `utc_text` would print a different number the week the clocks change — while
/// `18:30` is what is on the door of the building. Internal arithmetic stays in epoch
/// seconds throughout (sorting, windows, comparisons), so this is a *display* helper only,
/// and the distinction is the one the module header used to summarize as "UTC on purpose".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wall {
    /// `2026-11-16`
    pub date: String,
    /// `18:30`, or `None` for an all-day entry (a bare `YYYY-MM-DD`).
    pub time: Option<String>,
    /// Offset in seconds from UTC, as the entry itself declared it.
    pub offset: i64,
}

pub fn wall(s: &str) -> Option<Wall> {
    let s = s.trim();
    if s.len() < 10 || s.as_bytes().get(4) != Some(&b'-') || s.as_bytes().get(7) != Some(&b'-') {
        return None;
    }
    let date = s[0..10].to_string();
    let rest = &s[10..];
    if rest.is_empty() {
        return Some(Wall { date, time: None, offset: 0 });
    }
    if !matches!(rest.as_bytes()[0], b'T' | b't' | b' ') {
        return None;
    }
    let t = &rest[1..];
    if t.len() < 5 || t.as_bytes().get(2) != Some(&b':') {
        return None;
    }
    let (hh, mm) = (t[0..2].parse::<u32>().ok()?, t[3..5].parse::<u32>().ok()?);
    if hh > 23 || mm > 59 {
        return None;
    }
    // The offset is whatever follows the time; `iso_epoch` is the strict parser and this
    // must not be more lenient than it, so ask it rather than re-reading the tail.
    let epoch = iso_epoch(s)?;
    // The declared offset is the difference between the civil time printed in the string
    // and the absolute instant the strict parser computed from it.
    let naive = {
        let (y, m, d) = (date[0..4].parse::<i64>().ok()?, date[5..7].parse::<u32>().ok()?, date[8..10].parse::<u32>().ok()?);
        days_from_civil(y, m, d) * 86_400 + hh as i64 * 3600 + mm as i64 * 60
    };
    let offset = naive - (epoch as i64);
    Some(Wall { date, time: Some(format!("{hh:02}:{mm:02}")), offset })
}

/// `Mon`, `Tue`, … from a `YYYY-MM-DD`. Three letters because a Slack list wants to scan.
pub fn weekday(date: &str) -> Option<&'static str> {
    if date.len() < 10 {
        return None;
    }
    let y = date[0..4].parse::<i64>().ok()?;
    let m = date[5..7].parse::<u32>().ok()?;
    let d = date[8..10].parse::<u32>().ok()?;
    // Index into a Sunday-first week: day 0 of the civil epoch was a Thursday, which
    // is what makes the `+4` land. The suite pins four dates rather than trusting this
    // line, because an off-by-one here renames every event in the digest.
    let dow = (days_from_civil(y, m, d) + 4).rem_euclid(7);
    Some(["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][dow as usize])
}

/// `2026-11-16 18:30`, no zone label, from an epoch and an offset — the form the grouped
/// event list wants, since the group heading already says which day.
pub fn civil_text(epoch: f64, offset: i64) -> String {
    let secs = epoch as i64 + offset;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", tod / 3600, (tod % 3600) / 60)
}

/// `2026-11-17` → `17 Nov 2026`, because the hyphenated form is **spoken** with the word
/// "dash" in it. Measured through the same espeak-ng front end piper uses (`-x -q`):
///
/// ```text
/// 2026-11-17  → t'u: T'aUz@nd @n tw'Enti s'Iks d'aS Il'Ev@n d'aS s'Ev@nt,i:n
/// 17 Nov 2026 → s'Ev@nt,i:n n'0v t'u: T'aUz@nd @n tw'Enti s'Iks
/// ```
///
/// So a tool that prints an ISO date is fine on a screen and says "dash" out loud, and the
/// voice channel reads tool text straight through — `what_time` is why this exists, and the
/// event list is the caller that still has not been moved to it (`docs/follow-ups.md`).
pub fn nice_date(date: &str) -> String {
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let Ok(y) = date[0..4].parse::<i64>() else {
        return date.to_string();
    };
    let Ok(m) = date[5..7].parse::<usize>() else {
        return date.to_string();
    };
    let Ok(d) = date[8..10].parse::<u32>() else {
        return date.to_string();
    };
    match month.get(m.saturating_sub(1)) {
        Some(name) => format!("{} {name} {y}", d),
        None => date.to_string(),
    }
}

/// Seconds to add to UTC to get **Europe/London** wall time at this instant.
///
/// The rule is the statutory one, not a table: British Summer Time runs from 01:00 UTC on
/// the last Sunday in March to 01:00 UTC on the last Sunday in October. It is eleven lines
/// because the *arithmetic* is easy; what is not easy is getting it silently wrong across a
/// transition, so the suite pins the four minutes that matter (one either side of both
/// changeovers) and the whole function is then checked against the system tzdata over
/// sixteen years — see the crate README.
///
/// Deliberately a *named* zone rather than the host's: services1 could be set to UTC (it
/// effectively is in winter, where the two agree) and the answer would still be the space's.
pub fn london_offset(epoch: i64) -> i64 {
    let year = civil_from_days(epoch.div_euclid(86_400)).0;
    if epoch >= dst_start(year) && epoch < dst_end(year) {
        3_600
    } else {
        0
    }
}

/// `GMT` or `BST`, for putting next to the time so the number cannot be misread.
pub fn london_abbr(epoch: i64) -> &'static str {
    if london_offset(epoch) == 0 {
        "GMT"
    } else {
        "BST"
    }
}

/// 01:00 UTC on the last Sunday in March.
fn dst_start(year: i64) -> i64 {
    last_sunday_utc(year, 3, 31)
}

/// 01:00 UTC on the last Sunday in October.
fn dst_end(year: i64) -> i64 {
    last_sunday_utc(year, 10, 31)
}

/// The instant (01:00 UTC) of the last Sunday of a month, which is the shape every EU and
/// UK daylight-saving rule takes. `last` is that month's last day number.
fn last_sunday_utc(year: i64, month: u32, last: u32) -> i64 {
    // Same Sunday-first weekday as [`weekday`]: day 0 of the civil epoch was a Thursday.
    // So `dow` is *also* the number of days back from the last day of the month to the last
    // Sunday — 0 when the month itself ends on a Sunday. (This was `(dow + 6) % 7` first,
    // which is the formula for a Monday-first week and landed every transition six days
    // early; the suite catches it because `2026-03-29` stops being BST at midnight.)
    let dow = (days_from_civil(year, month, last) + 4).rem_euclid(7);
    let day = last as i64 - dow;
    days_from_civil(year, month, day as u32) * 86_400 + 3_600
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_epochs_render_as_known_clock_times() {
        assert_eq!(utc_text(0.0), "1970-01-01 00:00 UTC");
        assert_eq!(utc_text(1_794_820_163.0), "2026-11-16 09:09 UTC");
        assert_eq!(utc_text(1_794_690_788.0), "2026-11-14 21:13 UTC");
        // A leap day, both directions.
        assert_eq!(utc_text(iso_epoch("2024-02-29T23:59:00Z").unwrap()), "2024-02-29 23:59 UTC");
    }

    #[test]
    fn the_day_arithmetic_round_trips_across_a_year() {
        let z = days_from_civil(2024, 1, 1);
        for day in 0..400 {
            let (y, m, d) = civil_from_days(z + day);
            assert_eq!(days_from_civil(y, m, d), z + day, "{y}-{m}-{d}");
        }
    }

    #[test]
    fn an_iso_timestamp_with_an_offset_becomes_the_right_instant() {
        // The real fixture: a calendar entry from the space API.
        let e = iso_epoch("2026-11-16T18:30:00+00:00").unwrap();
        assert_eq!(utc_text(e), "2026-11-16 18:30 UTC");
        // Same wall time, five hours behind UTC = five hours later as an instant.
        let w = iso_epoch("2026-11-16T18:30:00-05:00").unwrap();
        assert!((w - (e + 5.0 * 3600.0)).abs() < 0.001);
        // And east of UTC.
        let w = iso_epoch("2026-11-16T18:30:00+02:00").unwrap();
        assert!((w - (e - 2.0 * 3600.0)).abs() < 0.001);
    }

    #[test]
    fn a_date_only_event_and_a_fractional_zulu_both_parse() {
        assert_eq!(iso_epoch("2026-11-16"), Some(iso_epoch("2026-11-16T00:00:00Z").unwrap()));
        let with_frac = iso_epoch("2026-11-16T18:30:00.500Z").unwrap();
        assert!((with_frac - iso_epoch("2026-11-16T18:30:00Z").unwrap() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn nonsense_is_refused_rather_than_guessed() {
        assert_eq!(iso_epoch(""), None);
        assert_eq!(iso_epoch("Monday"), None);
        assert_eq!(iso_epoch("2026-13-01T00:00:00Z"), None); // month 13
        // A time with no zone is not ours to invent: 18:30 in Europe/London is not
        // 18:30 UTC and the answer would be plausible either way.
        assert_eq!(iso_epoch("2026-11-16 18:30"), None);
        assert_eq!(iso_epoch("2026-11-16T18:30"), None);
        // A bare date is unambiguous (midnight, as the calendar API means it).
        assert!(iso_epoch("2026-11-16").is_some());
        // And a zone that is not `Z` or `±HH[:]MM` is a typo, not a timestamp.
        assert_eq!(iso_epoch("2026-11-16T18:30:00+000"), None);
        assert_eq!(iso_epoch("2026-11-16T99:00:00Z"), None);
    }

    #[test]
    fn a_relative_span_reads_the_way_a_person_says_it() {
        assert_eq!(ago_text(10.0), "just now");
        assert_eq!(ago_text(120.0), "2m ago");
        assert_eq!(ago_text(-(3600.0 * 5.0)), "in 5h");
        assert_eq!(ago_text(86_400.0 * 3.0 + 3600.0 * 4.0), "3d 4h ago");
        assert_eq!(ago_text(7_776_000.0), "3mo ago");
    }

    #[test]
    fn a_calendar_entry_is_displayed_as_the_time_it_published() {
        // The whole point: 18:30+01:00 is "half six" to the person reading the list, and
        // rendering it in UTC would say 17:30 and put the CTF on the wrong evening the week
        // the clocks change.
        let w = wall("2026-11-16T18:30:00+01:00").unwrap();
        assert_eq!(w.date, "2026-11-16");
        assert_eq!(w.time.as_deref(), Some("18:30"));
        assert_eq!(w.offset, 3600);
        assert_eq!(civil_text(iso_epoch("2026-11-16T18:30:00+01:00").unwrap(), 3600), "2026-11-16 18:30");
        // UTC entries are unchanged, and `Z` is the same thing as `+00:00`.
        assert_eq!(wall("2026-11-16T18:30:00Z").unwrap().time.as_deref(), Some("18:30"));
        assert_eq!(wall("2026-11-16T18:30:00Z").unwrap().offset, 0);
        // An all-day entry has no time to get wrong.
        let all_day = wall("2026-11-22").unwrap();
        assert_eq!(all_day.date, "2026-11-22");
        assert_eq!(all_day.time, None);
        // And the strict parser still gates it: no offset means no wall clock, because
        // `2026-11-16T18:30` is somebody's local time that we are not entitled to name.
        assert_eq!(wall("2026-11-16T18:30"), None);
        assert_eq!(wall("Monday"), None);
    }

    #[test]
    fn weekday_names_come_from_the_calendar_not_a_table_of_guesses() {
        assert_eq!(weekday("2026-11-16"), Some("Mon")); // the day this was written, checked against the journal
        assert_eq!(weekday("2026-11-21"), Some("Sat"));
        assert_eq!(weekday("2026-11-22"), Some("Sun"));
        assert_eq!(weekday("1970-01-01"), Some("Thu")); // the epoch's own weekday
        assert_eq!(weekday("nope"), None);
    }

    #[test]
    fn the_history_api_wants_its_window_in_the_shape_it_accepts() {
        assert_eq!(iso_utc(1_794_820_163.0), "2026-11-16T09:09:23+00:00");
    }

    #[test]
    fn the_rule_matches_the_system_tzdata_for_every_year_we_might_run_in() {
        // One assertion per year, at the exact second the offset changes and the second
        // before it. tzdata owns the rule; this only has to agree with it, and if the UK ever
        // changes the dates the table says so and the test fails here rather than in a room.
        // Generated from the system tzdata (`Europe/London`) on 2026-11-17: for each year,
        // the two instants the offset changes, both of which tzdata puts at 01:00 UTC.
        const TRANSITIONS: &[(i64, i64, i64)] = &[
            (2020, 1585443600, 1603587600),
            (2021, 1616893200, 1635642000),
            (2022, 1648342800, 1667091600),
            (2023, 1679792400, 1698541200),
            (2024, 1711846800, 1729990800),
            (2025, 1743296400, 1761440400),
            (2026, 1774746000, 1792890000),
            (2027, 1806195600, 1824944400),
            (2028, 1837645200, 1856394000),
            (2029, 1869094800, 1887843600),
            (2030, 1901149200, 1919293200),
            (2031, 1932598800, 1950742800),
            (2032, 1964048400, 1982797200),
            (2033, 1995498000, 2014246800),
            (2034, 2026947600, 2045696400),
            (2035, 2058397200, 2077146000),
            (2036, 2090451600, 2108595600),
            (2037, 2121901200, 2140045200),
            (2038, 2153350800, 2172099600),
            (2039, 2184800400, 2203549200),
            (2040, 2216250000, 2234998800),
        ];
        for &(year, start, end) in TRANSITIONS {
            assert_eq!(london_offset(start - 1), 0, "{year}: GMT until the second of the changeover");
            assert_eq!(london_offset(start), 3_600, "{year}: BST from the changeover");
            assert_eq!(london_offset(end - 1), 3_600, "{year}: BST until the second of the changeover");
            assert_eq!(london_offset(end), 0, "{year}: GMT from the changeover");
            assert_eq!(london_abbr(start), "BST");
            assert_eq!(london_abbr(end), "GMT");
            // And the changeover is at one o'clock UTC by definition, which is the fact the
            // `dst_start`/`dst_end` arithmetic rests on.
            assert_eq!(dst_start(year), start, "{year} spring");
            assert_eq!(dst_end(year), end, "{year} autumn");
        }
    }

    #[test]
    fn a_date_the_voice_can_say_is_not_the_one_a_database_writes() {
        assert_eq!(nice_date("2026-11-17"), "17 Nov 2026");
        assert_eq!(nice_date("2026-01-01"), "1 Jan 2026");
        assert_eq!(nice_date("2028-12-31"), "31 Dec 2028", "a leap year's last day");
        // Anything we cannot read comes back untouched rather than as a guess.
        assert_eq!(nice_date("not a date"), "not a date");
        assert_eq!(nice_date("2026-13-01"), "2026-13-01", "month 13 is not December's cousin");
    }

    #[test]
    fn london_crosses_the_clocks_at_one_utc_on_the_right_sundays() {
        // Four minutes that matter, pinned from tzdata rather than from belief: 2026's
        // changeovers are Sun 29 Mar and Sun 25 Oct.
        let into_bst = iso_epoch("2026-03-29T01:00:00Z").unwrap() as i64;
        let out_of_bst = iso_epoch("2026-10-25T01:00:00Z").unwrap() as i64;
        assert_eq!(london_offset(into_bst - 1), 0, "still GMT a second before");
        assert_eq!(london_offset(into_bst), 3_600, "BST from 01:00 UTC");
        assert_eq!(london_offset(out_of_bst - 1), 3_600, "still BST a second before");
        assert_eq!(london_offset(out_of_bst), 0, "GMT again from 01:00 UTC");
        // The wall clock at those instants, which is what a person in the room sees:
        // 02:00 on the way into summer, 01:00 (twice) on the way out.
        assert_eq!(civil_text(into_bst as f64, london_offset(into_bst)), "2026-03-29 02:00");
        assert_eq!(civil_text(out_of_bst as f64, london_offset(out_of_bst)), "2026-10-25 01:00");
        assert_eq!(london_abbr(into_bst), "BST");
        assert_eq!(london_abbr(out_of_bst), "GMT");
        // A deep-winter and a deep-summer date, so a broken comparison cannot pass both.
        assert_eq!(london_offset(iso_epoch("2026-01-15T12:00:00Z").unwrap() as i64), 0);
        assert_eq!(london_offset(iso_epoch("2026-07-15T12:00:00Z").unwrap() as i64), 3_600);
    }
}
