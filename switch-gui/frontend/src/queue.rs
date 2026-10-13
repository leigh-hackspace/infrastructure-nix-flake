//! The queue, as the operator sees it: what is waiting for the switch, and what
//! has already happened to it.
//!
//! This panel is the reason the queue is a table and not a function call.  A write
//! to a switch takes seconds and can fail in ways that are not the operator's
//! fault (the VLAN was not created yet, someone was logged into the console), so
//! the page has to answer "did it happen yet?" without them watching the journal —
//! and when it says *failed* it has to show what the switch said, because that
//! sentence is the diagnosis (`Invalid input` is a different problem from
//! `Can't modify VLAN membership on a trunk port`).
//!
//! It polls on its own, faster than the page around it: the queue is the part that
//! actually moves, while the shadow copy the rest of the page shows is only
//! re-read every [`crate::POLL_SECS`] by the sync anyway.

use dioxus::prelude::*;
use switch_gui_dto as dto;

use crate::{enc, get_json, post_json};

/// How often the queue panel re-reads itself, in seconds.
const QUEUE_SECS: u64 = 5;

/// How many closed rows to show under the open ones.
const HISTORY: usize = 20;

/// The queue for one switch: the open rows, then the closed ones.
///
/// Every action's result goes to the page's banner (`note`), because a Retry that
/// quietly re-queued a row looks exactly like a Retry that did nothing.
#[component]
pub fn Queue(name: String, note: Signal<Option<(bool, String)>>) -> Element {
    let mut tick = use_signal(|| 0u64);
    use_effect(move || {
        spawn(async move {
            loop {
                gloo_timers::future::sleep(std::time::Duration::from_secs(QUEUE_SECS)).await;
                tick += 1;
            }
        });
    });

    // One resource for both halves.  They are fetched back to back rather than in
    // parallel so a row that moves between the two queries cannot appear twice —
    // the pair is consistent with one moment, which is what a log needs to be.
    let rows = use_resource({
        let name = name.clone();
        move || {
            let _ = tick();
            let n = name.clone();
            async move {
                let open =
                    get_json::<dto::Changes>(&format!("/api/pending?switch={}", enc(&n))).await?;
                let closed =
                    get_json::<dto::Changes>(&format!("/api/changes?switch={}", enc(&n))).await?;
                Ok::<_, String>((open.changes, closed.changes))
            }
        }
    });

    let (open, closed) = rows
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok().cloned())
        .unwrap_or_default();
    let error = rows
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().err().cloned())
        .unwrap_or_default();

    rsx! {
        if !error.is_empty() {
            div { class: "note err", "{error}" }
        }
        if open.is_empty() {
            p { class: "muted", "Nothing queued for this switch." }
        } else {
            table {
                thead {
                    tr {
                        th { "#" }
                        th { "What" }
                        th { "State" }
                        th { "Tries" }
                        th { "Queued" }
                        th { "By" }
                        th { "What the switch said" }
                        th {}
                    }
                }
                tbody {
                    for c in open.iter() {
                        {
                            let id = c.id;
                            let summary = c.summary.clone();
                            // The CLI lines, one command group per line.  A tooltip
                            // rather than a column: they are what an operator reads
                            // when a row looks wrong, and a column of
                            // `vlan 225 tagged 7` would push everything else off.
                            let commands = c
                                .commands
                                .iter()
                                .map(|g| g.join(" "))
                                .collect::<Vec<_>>()
                                .join("\n");
                            let status = c.status.clone();
                            let said = c.last_error.clone().unwrap_or_default();
                            let attempt = c.attempt;
                            let max = c.max_attempts;
                            let queued = c.created_at.clone();
                            let by = c.created_by.clone();
                            let mut note = note;
                            rsx! {
                                tr {
                                    td { class: "n muted", "{id}" }
                                    td { title: "{commands}", "{summary}" }
                                    td { class: "{state_class(&status)}", "{status}" }
                                    td { class: "n", title: "the sync gives up after {max} tries", "{attempt}/{max}" }
                                    td { class: "muted", "{stamp(&queued)}" }
                                    td { class: "muted", "{by}" }
                                    td { class: "err", "{said}" }
                                    td {
                                        div { class: "row",
                                            button {
                                                title: "put it back at the front of the queue and start the retry ladder again",
                                                onclick: move |_| {
                                                    spawn(async move {
                                                        let r = act("/api/change/retry", id).await;
                                                        note.set(Some(r));
                                                    });
                                                },
                                                "Retry"
                                            }
                                            button {
                                                class: "danger",
                                                title: "drop it from the queue without writing anything",
                                                onclick: move |_| {
                                                    spawn(async move {
                                                        let r = act("/api/change/forget", id).await;
                                                        note.set(Some(r));
                                                    });
                                                },
                                                "Discard"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if !closed.is_empty() {
            p { class: "muted", "Last {HISTORY} closed changes — the audit log." }
            table {
                thead {
                    tr {
                        th { "#" }
                        th { "What" }
                        th { "State" }
                        th { "Tries" }
                        th { "Done" }
                        th { "By" }
                        th { "Note / what the switch said" }
                        th {}
                    }
                }
                tbody {
                    for c in closed.iter().take(HISTORY) {
                        {
                            let id = c.id;
                            let kind = c.kind.clone();
                            let summary = c.summary.clone();
                            let status = c.status.clone();
                            let said = c.last_error.clone().unwrap_or_default();
                            let note_text = c.note.clone().unwrap_or_default();
                            let attempt = c.attempt;
                            let done = c
                                .applied_at
                                .clone()
                                .or_else(|| Some(c.created_at.clone()))
                                .unwrap_or_default();
                            let by = c.created_by.clone();
                            // Revert needs the row's `before` map, and a refresh has
                            // no state to go back to; a row that *is* a revert should
                            // not invite an undo of an undo.
                            let revertable = c.reverted_from.is_none() && kind != "refresh";
                            let mut note = note;
                            rsx! {
                                tr {
                                    td { class: "n muted", "{id}" }
                                    td { "{summary}" }
                                    td { class: "{state_class(&status)}", "{status}" }
                                    td { class: "n", "{attempt}" }
                                    td { class: "muted", "{stamp(&done)}" }
                                    td { class: "muted", "{by}" }
                                    td {
                                        if !note_text.is_empty() {
                                            div { "{note_text}" }
                                        }
                                        if !said.is_empty() {
                                            div { class: "err", "{said}" }
                                        }
                                    }
                                    td {
                                        if revertable {
                                            button {
                                                title: "queue the opposite change, from what the switch held before this one",
                                                onclick: move |_| {
                                                    spawn(async move {
                                                        let r = act("/api/change/revert", id).await;
                                                        note.set(Some(r));
                                                    });
                                                },
                                                "Revert"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The class for a status cell.  `failed` is the only one that is red: `applying`
/// is in flight, and painting it red would teach the operator to ignore red.
fn state_class(status: &str) -> &'static str {
    match status {
        "applied" => "up",
        "failed" => "err",
        "applying" => "warn",
        _ => "muted",
    }
}

/// `to_char(... AT TIME ZONE 'UTC')` gives `2026-10-13T12:34:56Z`; the date half
/// of a row the operator is reading now is noise, so show the time and keep the
/// whole stamp in the tooltip.
fn stamp(ts: &str) -> String {
    match ts.split_once('T') {
        Some((_, rest)) => rest.to_string(),
        None => ts.to_string(),
    }
}

/// Retry / Discard / Revert one row.
///
/// All three take `{"id":N}` and answer with [`dto::Ack`].  A `false` here is not
/// a transport failure: it is the queue's own verdict (`change 12 is not retryable
/// (it is applied, or it is gone)`), which is the sentence the operator needs and
/// is why these endpoints answer 200 with a refusal rather than an HTTP status the
/// SPA would have to guess at.
async fn act(path: &str, id: i64) -> (bool, String) {
    #[derive(serde::Serialize)]
    struct Id {
        id: i64,
    }
    match post_json::<Id, dto::Ack>(path, &Id { id }).await {
        Ok(a) if a.ok => (
            true,
            match a.change {
                Some(c) => format!("{}: {} ({})", path, c.summary, c.status),
                None => format!("{path} #{id}"),
            },
        ),
        Ok(a) => (false, a.error.unwrap_or_else(|| "refused".to_string())),
        Err(e) => (false, e),
    }
}
