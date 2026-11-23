//! Who may be answering, and what a previous turn left behind.
//!
//! The whole job of this file is the **serialisation** of a socket that has exactly one of
//! everything: one conversation transcript per channel, one engine, one warm piper. It was cut
//! out of `listen.rs` because the bug that found it was invisible there — a warm-up holding the
//! one turn lock made the turn it was warming lose with `another turn is in flight`, and the
//! reason nobody saw it in review is that the refusal, the lock, the warm-up and the transcript
//! were four paragraphs apart in a thousand-line file. Together they are one sentence:
//! **a turn may take the lock immediately; a warm-up may hold it, and then a turn waits for it**
//! — and the measurement behind that exception is on [`acquire_agent`].
//!
//! Also here: [`Kind`], which is the only thing that separates a Slack-shaped turn from a
//! spoken one (system prompt, token budget, and — deliberately — a separate transcript, so a
//! room's half-finished question cannot leak into a channel message and back), and the session
//! bookkeeping that expires a conversation after `--reset-after`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use common_json::Json;

use crate::prompt;
use crate::agent::{self, Agent, MAX_SPEECH_TOKENS};
use crate::listen::{epoch, Ctx};
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Kind {
    Text,
    Voice,
}

impl Kind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Voice => "voice",
        }
    }

    pub(crate) fn system(self) -> String {
        match self {
            Kind::Text => prompt::system_prompt(),
            // The spoken prompt *is* the Slack one plus the override; one grounding
            // rule, so the two can never disagree about what counts as a fact.
            Kind::Voice => prompt::spoken_system_prompt(),
        }
    }

    pub(crate) fn budget(self) -> u32 {
        match self {
            Kind::Text => agent::MAX_ANSWER_TOKENS,
            Kind::Voice => MAX_SPEECH_TOKENS,
        }
    }
}

pub(crate) struct Session {
    pub(crate) transcript: Vec<Json>,
    pub(crate) last: Instant,
}

/// The socket's one turn lock, taken with one exception that has a measurement behind it.
///
/// Two conversations must never answer at once — one transcript, one engine, and an answer
/// interleaved with another's tool calls is not an answer — so a second turn is **refused**,
/// not queued (`AGENTS.md`'s reason: the room has moved on). A **warm-up** is the one holder a
/// turn should wait for, because waiting is cheaper than the alternative: measured on
/// services1 2026-11-22, a warm-up of a 4 079-token prompt cost **4 629 ms** of the 3060 and
/// reported `cached 0`, and the turn that arrived during it was refused with "another turn is
/// in flight" — a warm-up that does not delay the turn it warms is a bug, and one that *fails*
/// it is an outage with a polite message.
///
/// So: try; if it is held and a warm-up is in flight, poll for up to
/// [`PREFILL_WAIT`] and take it when free; otherwise give up immediately, as before. The
/// bounded wait is deliberately the only queueing in this file, and `ask_wait_ms` says how
/// often it is used.
pub(crate) fn acquire_agent(ctx: &Arc<Ctx>) -> Option<MutexGuard<'_, Agent>> {
    if let Ok(g) = ctx.agent.try_lock() {
        return Some(g);
    }
    if !ctx.stats.prefill_busy.load(Ordering::Relaxed) {
        return None;
    }
    let started = Instant::now();
    while started.elapsed() < PREFILL_WAIT {
        std::thread::sleep(Duration::from_millis(50));
        if let Ok(g) = ctx.agent.try_lock() {
            let ms = started.elapsed().as_millis() as u64;
            ctx.stats.ask_waited_for_prefill.fetch_add(1, Ordering::Relaxed);
            ctx.stats.ask_wait_ms.fetch_add(ms, Ordering::Relaxed);
            eprintln!("turn waited {ms} ms for a warm-up to free the engine");
            return Some(g);
        }
        if !ctx.stats.prefill_busy.load(Ordering::Relaxed) {
            // The warm-up is over and the lock is held by something else — a real turn.
            return None;
        }
    }
    None
}

/// How long a turn waits for a warm-up's lock. Longer than a cold prefill of this fleet's
/// largest system prompt (4.6 s measured), shorter than a caller's patience, and bounded so a
/// wedged warm-up thread cannot turn a refusal into a hang.
pub(crate) const PREFILL_WAIT: Duration = Duration::from_secs(10);

/// Marks a warm-up as in flight for the duration of the borrow. RAII because the flag sticking
/// would convert every future refusal into a 10 s wait, which is exactly the failure this
/// helper exists to prevent.
pub(crate) struct PrefillBusy<'a>(&'a AtomicBool);

impl PrefillBusy<'_> {
    pub(crate) fn new(flag: &AtomicBool) -> PrefillBusy<'_> {
        flag.store(true, Ordering::Relaxed);
        PrefillBusy(flag)
    }
}

impl Drop for PrefillBusy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}
pub(crate) fn take_transcript(
    sessions: &Mutex<BTreeMap<Kind, Session>>,
    kind: Kind,
    reset_after: Duration,
) -> Vec<Json> {
    let mut sessions = sessions.lock().unwrap_or_else(|p| p.into_inner());
    match sessions.get_mut(&kind) {
        Some(s) if s.last.elapsed() <= reset_after => s.transcript.clone(),
        _ => agent::seed_with(&kind.system()),
    }
}

pub(crate) fn keep_transcript(
    sessions: &Mutex<BTreeMap<Kind, Session>>,
    kind: Kind,
    transcript: Vec<Json>,
) {
    sessions
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(kind, Session { transcript, last: Instant::now() });
}

pub(crate) fn record(ctx: &Arc<Ctx>, question: &str, answer: &str, secs: f64) {
    if !ctx.opts.record_questions {
        return;
    }
    let line = format!(
        "{} — {} → {} [{:.1} s]",
        epoch(),
        question.replace('\n', " "),
        answer.replace('\n', " "),
        secs
    );
    let mut recent = ctx.stats.recent.lock().unwrap_or_else(|p| p.into_inner());
    recent.push_back(line);
    while recent.len() > 20 {
        recent.pop_front();
    }
}
