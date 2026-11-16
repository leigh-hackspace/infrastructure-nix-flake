//! Home Assistant — everything in the building that is not on the Space API.
//!
//! The split between this file and `tools_space.rs` is the split between the two
//! systems' jobs, not an accident of which box runs which: the Space API is the
//! hackspace's *published* view (rooms, open/closed, printers, members) and Home
//! Assistant is its *nervous system* (every entity, every automation, history of
//! both). Vulcan never read HA at all, so these two tools are the whole reason the
//! replacement can answer things the old regexes could not.
//!
//! ## Read-only, and enforced here rather than in a prompt
//!
//! Only `GET /api/states` and `GET /api/history/period/…` are issued. There is no
//! `POST /api/services/…` anywhere in this crate, so "the bot must not be able to
//! turn things on" (`docs/slack-agent-2026-11-16.md` §7) is a property of the code
//! and of the unit, not an instruction to a model that can be argued out of.
//!
//! ## Why these tools *filter* rather than dump
//!
//! `/api/states` on this instance is a few hundred entities and tens of
//! kilobytes. Handing that to a model that prefills at ~240 tok/s and is being
//! asked "is it cold" is the difference between a 3-second answer and a
//! 40-second one, and between an answer and a truncated one. So `search=` and
//! `domain=` are the tool's job, the truncation is explicit ("+137 more, narrow
//! it with search="), and the model is *told in the output* how to narrow — which
//! is also the only way it can learn that it is possible.

use crate::args;
use crate::clock;
use crate::sources::Sources;
use crate::tools::{clamp_usize, register};
use common_json::Json;
use common_mcp::{object_schema, param};
use std::sync::Arc;

pub fn register_tools(srv: &mut common_mcp::Server, src: &Arc<Sources>) {
    register(
        srv,
        "home_assistant_states",
        "Live entity states from the hackspace's Home Assistant: lights, locks, switches, \
         binary sensors (doors, motion, occupancy), plugs, power/energy sensors, climate, \
         weather, and any automation. Filter it down: without arguments it lists the first 40 \
         entities and tells you how many more matched. \
         domain= is an HA domain such as sensor, light, switch, lock, binary_sensor, climate, \
         cover, automation, weather. \
         search= is a case-insensitive substring matched against the entity id AND its display \
         name — try \"temp\", \"door\", \"heat\", \"light\", \"motion\", \"plug\", \"power\". \
         If the space API is not enough for a question about the building, this is the tool to \
         try, and if it says it matched nothing, say what you could not see.",
        object_schema(
            vec![
                ("domain", param("string", "HA domain to restrict to, e.g. sensor, light, switch, lock, binary_sensor, climate, weather.")),
                ("search", param("string", "Case-insensitive substring of the entity id or its friendly name, e.g. workshop, heat, front door.")),
                ("limit", param("integer", "Maximum rows to return. Default 40, maximum 150.")),
            ],
            Vec::new(),
        ),
        src,
        states,
    );

    register(
        srv,
        "home_assistant_history",
        "How one Home Assistant numeric or on/off entity has varied over the last hours: \
         min, max, latest, mean for numbers; the distinct values and how often they changed for \
         switches and binary sensors. Use this for \"has it been cold all night\", \"did anyone \
         open the door overnight\", \"how long was the heater on\". Needs an entity id — get it \
         from home_assistant_states first if you do not have one.",
        object_schema(
            vec![
                ("entity_id", param("string", "One HA entity id, e.g. sensor.workshop_temperature or binary_sensor.front_door.")),
                ("hours", param("integer", "How far back, in hours. Default 24, maximum 168 (a week).")),
            ],
            vec!["entity_id"],
        ),
        src,
        history,
    );
}

fn states(src: &Sources, arguments: &Json) -> Result<String, String> {
    let domain = args::str_arg(arguments, "domain");
    let search = args::str_arg(arguments, "search").map(|s| s.to_lowercase());
    let limit = clamp_usize(args::int_arg(arguments, "limit", 1, 150).unwrap_or(None), 40, 150);

    let doc = src.fetch_hass("/api/states")?;
    let rows = doc.as_array().ok_or("Home Assistant /api/states did not return an array")?;

    let mut matched: Vec<String> = Vec::new();
    let mut seen_domains: Vec<String> = Vec::new();
    for e in rows {
        let Some(id) = e.get("entity_id").and_then(|v| v.as_str()) else { continue };
        let this_domain = id.split('.').next().unwrap_or("");
        if !seen_domains.contains(&this_domain.to_string()) {
            seen_domains.push(this_domain.to_string());
        }
        if let Some(d) = &domain {
            if !id.starts_with(&format!("{d}.")) {
                continue;
            }
        }
        let friendly = e
            .at(&["attributes", "friendly_name"])
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if let Some(s) = &search {
            if !id.to_lowercase().contains(s) && !friendly.to_lowercase().contains(s) {
                continue;
            }
        }
        let state = e.get("state").and_then(|v| v.as_str()).unwrap_or("?");
        let unit = e
            .at(&["attributes", "unit_of_measurement"])
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let label = if friendly.is_empty() || friendly.eq_ignore_ascii_case(id) {
            String::new()
        } else {
            format!(" ({friendly})")
        };
        matched.push(if unit.is_empty() {
            format!("{id} = {state}{label}")
        } else {
            format!("{id} = {state} {unit}{label}")
        });
    }

    if matched.is_empty() {
        let mut out = format!(
            "Home Assistant has {} entities but none matched{}{}.",
            rows.len(),
            if let Some(d) = &domain {
                format!(" domain {d}")
            } else {
                String::new()
            },
            if let Some(s) = &search {
                format!(" containing \"{s}\"")
            } else {
                String::new()
            }
        );
        // The domains that *do* exist: the one thing that lets a model correct a
        // wrong guess (\"domain: temp\") instead of reporting that HA is empty.
        let mut doms = seen_domains;
        doms.sort();
        out.push_str(&format!("\nDomains present: {}.", doms.join(", ")));
        return Ok(out);
    }
    let total = matched.len();
    let shown = matched.drain(..limit.min(matched.len())).collect::<Vec<_>>();
    let mut out = format!("Home Assistant states ({total} matched):\n");
    for line in &shown {
        out.push_str(&format!("  {line}\n"));
    }
    if total > shown.len() {
        out.push_str(&format!(
            "  (+{} more not shown — narrow with search= or domain=)\n",
            total - shown.len()
        ));
    }
    Ok(out.trim_end().to_string())
}

fn history(src: &Sources, arguments: &Json) -> Result<String, String> {
    let entity = args::required_str(arguments, "entity_id", "one entity id, e.g. sensor.workshop_temperature")?;
    let hours = args::int_arg_default(arguments, "hours", 24, 1, 168)?;
    let now = clock::now_epoch();
    let start = now - hours as f64 * 3600.0;
    let url = format!(
        "/api/history/period/{}?filter_entity_id={}&end_time={}",
        clock::iso_utc(start),
        crate::sources::urlencode(&entity),
        crate::sources::urlencode(&clock::iso_utc(now))
    );
    let doc = src.fetch_hass(&url)?;
    let series = doc
        .as_array()
        .ok_or("Home Assistant history did not return a list of series")?;
    let points = series.first().and_then(|s| s.as_array()).map(|a| a.to_vec()).unwrap_or_default();
    if points.is_empty() {
        return Ok(format!(
            "No recorded history for {entity} in the last {hours}h. Either that entity does not \
             exist, or Home Assistant is not recording it (check home_assistant_states for the \
             exact id)."
        ));
    }

    // Numeric vs stateful: the two questions a person asks about history, decided
    // by what the recording actually holds rather than by what the entity id says.
    let mut numbers: Vec<(f64, f64)> = Vec::new();
    let mut cats: Vec<(f64, String)> = Vec::new();
    for p in &points {
        let Some(at) = p.get("last_changed").and_then(|v| v.as_str()).and_then(clock::iso_epoch) else {
            continue;
        };
        if let Some(s) = p.get("state").and_then(|v| v.as_str()) {
            match s.trim().parse::<f64>() {
                Ok(n) if n.is_finite() => numbers.push((at, n)),
                _ => cats.push((at, s.to_string())),
            }
        }
    }

    let window = format!("last {hours}h for {entity}");
    if !numbers.is_empty() {
        let vals: Vec<f64> = numbers.iter().map(|(_, v)| *v).collect();
        let min = vals.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let mean = vals.iter().sum::<f64>() / vals.len() as f64;
        let (latest_at, latest) = numbers[numbers.len() - 1];
        let unit = points
            .last()
            .and_then(|p| p.at(&["attributes", "unit_of_measurement"]))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let samples = sparkline(&numbers, 12);
        return Ok(format!(
            "History, {window}: {} points\n  latest {}{unit} at {} · min {}{unit} · max {}{unit} · mean {}{unit}\n  {}\n  first {} · last {}",
            vals.len(),
            crate::sources::value(latest),
            clock::utc_text(latest_at),
            crate::sources::value(min),
            crate::sources::value(max),
            crate::sources::value(mean),
            samples,
            clock::utc_text(numbers[0].0),
            clock::utc_text(latest_at),
        ));
    }

    // On/off or otherwise categorical.
    let mut changes = 0usize;
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut prev: Option<String> = None;
    for (_, v) in &cats {
        match &prev {
            Some(p) if p == v => {}
            _ => changes += 1,
        }
        prev = Some(v.clone());
        match counts.iter_mut().find(|(k, _)| k == v) {
            Some((_, n)) => *n += 1,
            None => counts.push((v.clone(), 1)),
        }
    }
    counts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let tally = counts
        .iter()
        .take(8)
        .map(|(k, n)| format!("{k} ×{n}"))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "History, {window}: {} state changes, {} recorded states.\n  values: {}\n  first seen {} · last seen {}",
        changes.saturating_sub(1),
        cats.len(),
        tally,
        clock::utc_text(cats.first().map(|(t, _)| *t).unwrap_or(0.0)),
        clock::utc_text(cats.last().map(|(t, _)| *t).unwrap_or(0.0)),
    ))
}

/// Up to `want` samples of a numeric series as `HH:MM=value` pairs, evenly spaced.
///
/// Evenly spaced rather than the most recent N, because the shape of "cold all
/// night, warm at noon" is the thing a person is asking about and the tail is not
/// it. Fewer samples than `want` are returned verbatim, never interpolated.
fn sparkline(points: &[(f64, f64)], want: usize) -> String {
    if points.is_empty() {
        return String::new();
    }
    let step = (points.len() as f64 / want as f64).ceil().max(1.0) as usize;
    let mut parts = Vec::new();
    let mut i = 0;
    while i < points.len() {
        let (at, v) = points[i];
        let t = clock::utc_text(at);
        parts.push(format!("{}={}", &t[11..16], crate::sources::value(v)));
        i += step;
    }
    // Always finish on the last point: a series that ends at 03:00 when the
    // question was asked at 08:00 is itself part of the answer.
    let (at, v) = points[points.len() - 1];
    let tail = format!("{}={}", &clock::utc_text(at)[11..16], crate::sources::value(v));
    if parts.last().map(String::as_str) != Some(tail.as_str()) {
        parts.push(tail);
    }
    parts.join(" ")
}
