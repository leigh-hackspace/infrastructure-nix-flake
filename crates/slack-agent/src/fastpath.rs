//! `fastpath` — the Slack-side answers that need no model.
//!
//! Three shapes live here: the calendar list (`space_events`, answered this way since phase 1
//! because the calendar already has the rows in the right order and a model only risks
//! dropping one), the one-tool questions `Agent::quick_answer` recognises, and the argument
//! builder they share. The recognition itself is `triage::quick`; the sentence is the tool's;
//! what is *here* is only "post it to Slack the way a reply is posted", which is the part
//! that is this harness's business.
//!
//! `docs/slack-agent-2026-11-16.md` §15 is the measurement that put this in a module of its
//! own: the same questions cost 5-18 s of engine and tens of milliseconds through these paths.

use crate::agent::{self, Agent};
use crate::slack::{self, Slack};
use crate::{Config, Metrics};
use common_json::{self as json, Json};
use std::sync::atomic::Ordering;
use std::time::Instant;

/// Days ahead for one deterministic calendar answer, from the words in the message.
pub(crate) fn calendar_args(days: i64) -> Json {
    json::object(vec![(
        "days".to_string(),
        json::number_from_int(days.clamp(1, 60) as u64),
    )])
}

/// Answer "what's on" straight from the tool. Returns `Err` when anything in the path fails,
/// and the caller then asks the model instead — a slow answer beats none.
///
/// No `eyes` reaction either: that one exists because a model turn takes ~17 s and a person
/// deserves to know they were not ignored. This takes the length of one HTTP GET.
pub(crate) fn answer_events(
    agent: &mut Agent,
    slack: &Slack,
    config: &Config,
    metrics: &Metrics,
    mention: &slack::Mention,
    days: i64,
) -> Result<(), String> {
    let started = Instant::now();
    let text = agent.call("space_events", calendar_args(days))?;
    let (thread_ts, at) = slack::reply_target(
        mention.thread_ts.as_deref(),
        config.thread_replies,
        &mention.user,
    );
    let body = match at {
        Some(at) => format!("{at} {}", slack::escape(&text)),
        None => slack::escape(&text),
    };
    slack.post(&mention.channel, thread_ts.as_deref(), &body)?;
    metrics.turns.fetch_add(1, Ordering::Relaxed);
    metrics.tool_calls.fetch_add(1, Ordering::Relaxed);
    metrics
        .last_turn_ms
        .store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
    println!(
        "deterministic: space_events(days={days}) answered {} in {} ms (no model)",
        mention.user,
        started.elapsed().as_millis()
    );
    Ok(())
}

/// Post a fast-path answer. A sibling of `answer_events`, deliberately not folded into it:
/// they share three steps and differ in everything else (one takes a tool name and arguments,
/// the other a days-window; one reacts, one does not), and the seam between two callers that
/// are "almost the same" is where a boolean parameter is born and someone later reads it
/// backwards.
pub(crate) fn answer_quick(
    slack: &Slack,
    config: &Config,
    metrics: &Metrics,
    mention: &slack::Mention,
    hit: &agent::QuickHit,
    ms: u64,
) -> Result<(), String> {
    let (thread_ts, at) = slack::reply_target(
        mention.thread_ts.as_deref(),
        config.thread_replies,
        &mention.user,
    );
    let body = match at {
        Some(at) => format!("{at} {}", slack::escape(&hit.text)),
        None => slack::escape(&hit.text),
    };
    slack.post(&mention.channel, thread_ts.as_deref(), &body)?;
    metrics.turns.fetch_add(1, Ordering::Relaxed);
    metrics.tool_calls.fetch_add(1, Ordering::Relaxed);
    metrics.last_turn_ms.store(ms, Ordering::Relaxed);
    println!(
        "deterministic: {} answered {} in {ms} ms (no model)",
        hit.label, mention.user
    );
    Ok(())
}
