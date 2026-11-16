//! The watching tools: cameras, fleet metrics, and who is on the network.
//!
//! These three exist because the two curated sources are not enough for the
//! questions that actually get asked in a hackspace channel. "Is anyone about"
//! is a camera question and a DHCP question; "is aibox cooking" is a Prometheus
//! question. None of them is a question the Space API can answer, and each is
//! summarised here rather than passed through, for the reason spelled out at the
//! top of `tools_hass.rs`.
//!
//! ## One measurement worth keeping in mind
//!
//! `docs/camera-wifi-cells-2026-10-31.md` established that a camera's feed quality
//! on this network is a property of the radio cell it joined, not of the camera —
//! two identical units at the same signal strength delivered 22× apart in
//! throughput. That is why this tool surfaces `stalls_last_hour`,
//! `reconnects_last_hour` and `connection_quality` per camera: the honest answer
//! to "is cam5 broken" is usually "cam5's stream is stalling and here is what the
//! capture side says", and the model should not have to guess which of the two it
//! is being told.
//!
//! `/api/events/summary` is deliberately **not** used: measured at over 10 s to
//! answer on this instance, which is longer than the whole budget of a turn.

use crate::args;
use crate::clock;
use crate::sources::{urlencode, Sources};
use crate::tools::{join_short, register};
use common_json::Json;
use common_mcp::{object_schema, param};
use std::collections::BTreeMap;
use std::sync::Arc;

pub fn register_tools(srv: &mut common_mcp::Server, src: &Arc<Sources>) {
    register(
        srv,
        "camera_activity",
        "The CCTV/Frigate cameras: how many motion/person/other detections each has recorded \
         recently, how long ago the most recent was, and which cameras are stalling or \
         reconnecting (a stalled camera means the feed is not reaching the detector, which is a \
         fault, not an empty room). Use this for \"is anyone in the space\", \"has anyone been \
         in the workshop tonight\", \"are the cameras working\". Note the space can be OPEN with \
         no detections if everyone is sitting still in one room.",
        object_schema(
            vec![
                ("hours", param("integer", "How far back to count detections. Default 12, maximum 168.")),
                ("camera", param("string", "Optional camera name or substring, e.g. main_space, woodwork, workshop, pi_room, social_space, rack_corridor, cnc, cam8.")),
            ],
            Vec::new(),
        ),
        src,
        camera_activity,
    );

    register(
        srv,
        "fleet_metric",
        "One instant PromQL query against the fleet's Prometheus — the servers, switches, NAS, \
         printers, UPS and exporters, NOT the hackspace rooms (there are no room temperature \
         series here; use space_status for those). Useful for CPU/memory/disk/temperature of a \
         box, whether a service is up, 3D-printer nozzle and bed temperatures \
         (moonraker_heater_temperature), node_hwmon_temp_celsius, UPS state, bandwidth. \
         Returns up to 10 series. If you get nothing back, the query is wrong or the metric \
         does not exist — say so rather than guessing a number.",
        object_schema(
            vec![
                ("query", param("string", "An instant PromQL expression, e.g. node_load1{instance=\"aibox:9100\"} or moonraker_heater_temperature.")),
            ],
            vec!["query"],
        ),
        src,
        fleet_metric,
    );

    register(
        srv,
        "network_clients",
        "How many devices currently hold a DHCP lease on the hackspace network, by VLAN, and \
         optionally the ones matching a name. This is the closest thing to \"how many people are \
         here\": vlan225 is the wired/server block, vlan226 is the camera/iot block, vlan227 the \
         member WiFi-ish block. Device names are what the devices call themselves. \
         Use for \"how busy is it tonight\", \"is my laptop on the network\", \"what is connected\".",
        object_schema(
            vec![
                ("search", param("string", "Optional substring matched against device name, MAC or IP, e.g. \"iphone\", \"cam\", \"10.3.2.1\".")),
            ],
            Vec::new(),
        ),
        src,
        network_clients,
    );
}

fn camera_activity(src: &Sources, arguments: &Json) -> Result<String, String> {
    let hours = args::int_arg_default(arguments, "hours", 12, 1, 168)?;
    let wanted = args::str_arg(arguments, "camera").map(|s| s.to_lowercase());
    let now = clock::now_epoch();
    let after = (now - hours as f64 * 3600.0) as i64;

    // Two calls, and a failure of the second must not lose the first: the health
    // line is the answer to "are the cameras working" even when the event history
    // endpoint is unhappy.
    let stats = src.fetch_json(&format!("{}/api/stats", src.frigate))?;
    let events_url = format!("{}/api/events?limit=400&after={after}", src.frigate);
    let events = match src.fetch_json(&events_url) {
        Ok(Json::Arr(rows)) => Some(rows),
        _ => None,
    };

    let mut unhealthy: Vec<String> = Vec::new();
    let mut camera_count = 0usize;
    if let Some(cams) = stats.get("cameras").and_then(|c| c.as_object()) {
        for (name, c) in cams {
            if let Some(w) = &wanted {
                if !name.to_lowercase().contains(w.as_str()) {
                    continue;
                }
            }
            camera_count += 1;
            let stalls = c.get("stalls_last_hour").and_then(|v| v.as_u64()).unwrap_or(0);
            let reconnects = c.get("reconnects_last_hour").and_then(|v| v.as_u64()).unwrap_or(0);
            let quality = c.get("connection_quality").and_then(|v| v.as_str()).unwrap_or("?");
            if stalls > 0 || reconnects > 0 || !matches!(quality, "excellent" | "good") {
                unhealthy.push(format!(
                    "{name}: {quality}, {} stall{} and {} reconnect{} in the last hour",
                    stalls,
                    if stalls == 1 { "" } else { "s" },
                    reconnects,
                    if reconnects == 1 { "" } else { "s" }
                ));
            }
        }
    }

    // Per-camera detection counts and the most recent event, from the event list.
    let mut per_camera: BTreeMap<String, (BTreeMap<String, usize>, f64)> = BTreeMap::new();
    if let Some(rows) = &events {
        for ev in rows {
            let Some(cam) = ev.get("camera").and_then(|v| v.as_str()) else { continue };
            if let Some(w) = &wanted {
                if !cam.to_lowercase().contains(w.as_str()) {
                    continue;
                }
            }
            let label = ev.get("label").and_then(|v| v.as_str()).unwrap_or("?");
            let start = ev.get("start_time").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let entry = per_camera.entry(cam.to_string()).or_default();
            *entry.0.entry(label.to_string()).or_insert(0) += 1;
            if start > entry.1 {
                entry.1 = start;
            }
        }
    }

    let total = stats
        .get("detection_fps")
        .and_then(|v| v.as_f64())
        .map(|v| format!("{} det/s fleet-wide, ", crate::sources::value(v)))
        .unwrap_or_else(|| "det/s unknown, ".to_string());
    // The detector line is the "is the CPU swamped / is the Coral hot" answer, and
    // it is the one number in here a regular reader of this channel has opinions
    // about (docs/frigate-false-positives-2026-11-12.md measured the Coral at ~37 %
    // utilisation, which is why throughput is not the false-positive cause).
    let coral = stats
        .get("detectors")
        .and_then(|d| d.as_object())
        .and_then(|m| m.values().next())
        .map(|d| {
            let speed = d.get("inference_speed").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let temp = d.get("temperature").and_then(|v| v.as_f64());
            format!(
                "detector {} ms{}",
                crate::sources::value(speed),
                temp.map(|t| format!(" at {} C", crate::sources::value(t))).unwrap_or_default()
            )
        })
        .unwrap_or_else(|| "detector unknown".to_string());

    let mut out = format!(
        "Frigate ({} cameras{}): {total}{coral}\n",
        if camera_count == 0 {
            stats
                .get("cameras")
                .and_then(|c| c.as_object())
                .map(|m| m.len())
                .unwrap_or(0)
        } else {
            camera_count
        },
        if wanted.is_some() { ", filtered" } else { "" }
    );

    if events.is_none() {
        out.push_str("  (the detections endpoint did not answer, so counts below are unknown)\n");
    }
    if per_camera.is_empty() && events.is_some() {
        out.push_str(&format!("  no detections of any kind in the last {hours}h\n"));
    }
    for (cam, (labels, latest)) in &per_camera {
        let tally = labels
            .iter()
            .map(|(l, n)| format!("{l} ×{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let since = if *latest > 0.0 {
            format!(" · last {}", clock::ago_text(now - latest))
        } else {
            String::new()
        };
        out.push_str(&format!("  {cam}: {tally}{since}\n"));
    }
    if !unhealthy.is_empty() {
        out.push_str("  needs attention: ");
        out.push_str(&join_short(&unhealthy, 6));
        out.push('\n');
    }
    Ok(out.trim_end().to_string())
}

fn fleet_metric(src: &Sources, arguments: &Json) -> Result<String, String> {
    let query = args::required_str(arguments, "query", "a PromQL expression")?;
    let limit = 10usize;
    let url = format!(
        "{}/api/v1/query?query={}",
        src.prometheus,
        urlencode(&query)
    );
    let doc = src.fetch_json(&url)?;
    if let Some(err) = doc.get("error").and_then(|v| v.as_str()) {
        return Ok(format!(
            "Prometheus rejected that query: {err}\n(The query was: {query})"
        ));
    }
    let result = doc
        .at(&["data", "result"])
        .and_then(|r| r.as_array())
        .ok_or("Prometheus reply had no data.result array")?;
    if result.is_empty() {
        // A model that gets "no such metric" back will guess the next name, and the
        // one after that — measured on 2026-11-16, where a question about NAS disk
        // spent twelve calls and four rounds inventing names (`node_disk_free_bytes`,
        // `vfs_free_bytes`, `nas_disk_free_bytes`, …) and ended at the round cap.
        // The fix is not a shorter leash: it is to answer the question it was really
        // asking, which is "what is this metric called here?".
        let suggestions = suggest_metrics(src, &query).unwrap_or_default();
        let mut out = format!(
            "Prometheus has no series matching: {query}\nThat means the metric does not exist \
             here (or every matching series is stale). Do not guess a number."
        );
        if !suggestions.is_empty() {
            out.push_str(&format!("\nMetric names here that resemble it: {}", suggestions.join(", ")));
        }
        // …and the other half of a query that matches nothing: the right metric with
        // a guessed `instance=` label answers nothing too, which is indistinguishable
        // from "the box has no disk metrics".  Measured 2026-11-16: the model found
        // `node_filesystem_avail_bytes` (correct) and then filtered on an invented
        // `instance="10.3.1.6:9100"` and concluded it could not see the NAS.
        if let Some(instances) = known_instances(src) {
            out.push_str(&format!("\nScraped instances (for instance=…): {instances}"));
        }
        return Ok(out);
    }
    let mut out = format!(
        "Prometheus, {} series matching `{query}` ({} shown):\n",
        result.len(),
        result.len().min(limit)
    );
    for series in result.iter().take(limit) {
        let labels = series
            .get("metric")
            .and_then(|m| m.as_object())
            .map(|m| {
                let name = m
                    .get("__name__")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let rest: Vec<String> = m
                    .iter()
                    .filter(|(k, _)| k.as_str() != "__name__")
                    .map(|(k, v)| format!("{k}=\"{}\"", v.as_str().unwrap_or("")))
                    .collect();
                if name.is_empty() {
                    format!("{{{}}}", rest.join(","))
                } else if rest.is_empty() {
                    name.to_string()
                } else {
                    format!("{name}{{{}}}", rest.join(","))
                }
            })
            .unwrap_or_else(|| "(unlabelled)".to_string());
        // An instant query answers `value: [epoch, "string"]`; a range query in the
        // same endpoint would answer `values: [[…], …]`, so read the last of either.
        let value = series
            .get("value")
            .and_then(|v| v.as_array())
            .and_then(|a| a.get(1))
            .and_then(|v| v.as_str().map(|s| s.to_string()).or_else(|| v.as_f64().map(crate::sources::value)))
            .or_else(|| {
                series
                    .get("values")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.last())
                    .and_then(|pair| pair.as_array())
                    .and_then(|p| p.get(1))
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
            })
            .unwrap_or_else(|| "?".to_string());
        out.push_str(&format!("  {labels} = {value}\n"));
    }
    if result.len() > limit {
        out.push_str(&format!("  (+{} more series)\n", result.len() - limit));
    }
    Ok(out.trim_end().to_string())
}

/// The `instance` label values Prometheus actually has, so a selector is chosen
/// rather than invented.  Capped, with the count, because the fleet grows.
fn known_instances(src: &Sources) -> Option<String> {
    let doc = src
        .fetch_json(&format!("{}/api/v1/label/instance/values", src.prometheus))
        .ok()?;
    // Prometheus wraps everything in `{"status":…,"data":…}` — reading the array
    // off the envelope reads as "no instances", quietly, which is the shape of bug
    // that makes a helpful feature look like it never fires.
    let values: Vec<String> = doc
        .get("data")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str())
        .map(|s| s.to_string())
        .collect();
    if values.is_empty() {
        return None;
    }
    let head = crate::tools::join_short(&values, 18);
    Some(if values.len() > 18 {
        format!("{head} — narrow by job name instead of guessing an address")
    } else {
        head
    })
}

/// Metric names in this Prometheus that look like something in the failed query.
///
/// One extra upstream call, only on the path that already failed, in exchange for
/// the difference between a model that corrects itself and one that retries guesses
/// until the round budget runs out.
fn suggest_metrics(src: &Sources, query: &str) -> Result<Vec<String>, String> {
    let doc = src.fetch_json(&format!("{}/api/v1/label/__name__/values", src.prometheus))?;
    let names = doc
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or("the label-values reply had no `data` array")?
        .iter()
        .filter_map(|v| v.as_str())
        .map(|s| s.to_string())
        .collect::<Vec<String>>();
    // Score on shared words, and on substring overlap for a single long token:
    // `node_disk_free_bytes` should find `node_filesystem_avail_bytes` on "node"
    // and "bytes" rather than on nothing at all.
    let mut words: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .map(|w| w.to_string())
        .collect();
    if words.is_empty() {
        words.push(query.to_lowercase());
    }
    let mut scored: Vec<(usize, String)> = names
        .iter()
        .map(|n| {
            let low = n.to_lowercase();
            let score = words
                .iter()
                .map(|w| {
                    if low == *w {
                        4
                    } else if low.contains(w.as_str()) || w.contains(low.as_str()) {
                        2
                    } else {
                        0
                    }
                })
                .sum::<usize>();
            (score, n.clone())
        })
        .filter(|(score, _)| *score > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    Ok(scored.into_iter().take(8).map(|(_, n)| n).collect())
}

fn network_clients(src: &Sources, arguments: &Json) -> Result<String, String> {
    let search = args::str_arg(arguments, "search").map(|s| s.to_lowercase());
    let doc = src.fetch_json(&format!("{}/api/leases", src.leases))?;
    let total = doc.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
    let stale = doc.get("stale").and_then(|v| v.as_bool()).unwrap_or(false);

    if let Some(s) = &search {
        let rows = doc.get("leases").and_then(|l| l.as_array()).map(|a| a.to_vec()).unwrap_or_default();
        let hits: Vec<String> = rows
            .iter()
            .filter(|l| {
                let name = l.get("name").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                let mac = l.get("mac").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                let ip = l.get("ip").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                name.contains(s.as_str()) || mac.contains(s.as_str()) || ip.contains(s.as_str())
            })
            .take(12)
            .map(|l| {
                format!(
                    "{} {} [{} {}]",
                    l.get("ip").and_then(|v| v.as_str()).unwrap_or("?"),
                    l.get("name").and_then(|v| v.as_str()).unwrap_or("(unnamed)"),
                    l.get("mac").and_then(|v| v.as_str()).unwrap_or("?"),
                    l.get("vlan").and_then(|v| v.as_str()).unwrap_or("?"),
                )
            })
            .collect();
        return Ok(if hits.is_empty() {
            format!("No DHCP lease matches \"{s}\" out of {total} leases right now.")
        } else {
            format!(
                "{total} leases; {} match \"{s}\":\n  {}",
                hits.len(),
                hits.join("\n  ")
            )
        });
    }

    let by_vlan = doc
        .get("vlans")
        .and_then(|v| v.as_object())
        .map(|m| {
            let mut pairs: Vec<(String, u64)> = m
                .iter()
                .map(|(k, v)| (k.clone(), v.as_u64().unwrap_or(0)))
                .collect();
            pairs.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
            pairs
                .iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect::<Vec<_>>()
                .join(" · ")
        })
        .unwrap_or_else(|| "(no VLAN breakdown)".to_string());
    let expired = doc.get("expired").and_then(|v| v.as_u64()).unwrap_or(0);
    let mut out = format!(
        "{total} DHCP leases on the network now ({expired} expired), by VLAN: {by_vlan}\n"
    );
    if stale {
        // Rule 29's lesson, applied: a table of zeroes must not read as a quiet
        // network, so say the channel is dead rather than the count.
        out.push_str("  WARNING: the router says this lease table is STALE — the counts above may \
                      be hours old, treat them as unknown.\n");
    }
    let named: Vec<String> = doc
        .get("leases")
        .and_then(|l| l.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|l| l.get("name").and_then(|v| v.as_str()))
                .filter(|n| !n.is_empty())
                .take(20)
                .map(|n| n.to_string())
                .collect()
        })
        .unwrap_or_default();
    if !named.is_empty() {
        out.push_str(&format!("  names: {}", join_short(&named, 20)));
    }
    Ok(out.trim_end().to_string())
}
