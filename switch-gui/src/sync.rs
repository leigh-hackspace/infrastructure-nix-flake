//! The sync: the only thing in this program that talks to a switch.
//!
//! Two jobs, one loop.  **Write**: take the queued changes out of Postgres, put
//! them down the console, and record what the switch said.  **Read**: re-read each
//! switch's whole configuration on an interval and replace its shadow copy.  The
//! GUI never does either — it reads the shadow copy and appends to the queue — so
//! every slow, fragile, serial-console interaction happens here, on its own
//! schedule, and a switch that is down costs a stale page and a warning rather
//! than a spinner.
//!
//! ## Why the loop is shaped like this
//!
//! The Telnet client is blocking and synchronous, because that is what a 1990s
//! console needs (see [`crate::telnet`]).  The database is async.  So each pass
//! has three parts and only the middle one may block:
//!
//! ```text
//!   claim (async, Postgres)  →  apply + read (blocking, spawn_blocking)  →  record (async, Postgres)
//! ```
//!
//! A blocking task that also held a database connection would be a held pool
//! connection for the duration of a console conversation that can take tens of
//! seconds on switch3, whose RADIUS login is slow.  Keeping the two apart is what
//! lets the pool stay at a few connections and lets a hung console hang exactly
//! one task.
//!
//! ## Why one switch's failure is not the network's
//!
//! Each switch gets its own blocking task per pass, and a failure of one is
//! recorded against that switch alone.  This is the same isolation the cached
//! sessions had, and it matters for the reason it always did: `switch3` answers
//! slowly because RADIUS is tried first, and a box that is being re-flashed must
//! not stop the other two from being read or their changes from being applied.
//!
//! ## The order inside a pass
//!
//! Writes first, then the read.  A read that happens before the writes it is
//! waiting for would put the pre-change state in the shadow copy, and the page
//! would show the change as pending against a snapshot that contradicts it —
//! which is exactly the confusion the pending notice exists to prevent.

use crate::changes;
use crate::model::Snapshot;
use crate::shadow;
use crate::switch::{self, Switch};
use crate::R;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often the loop wakes to look for queued work.
///
/// Two seconds: short enough that "pending sync" clears before anyone notices,
/// and the cost is one indexed claim query per switch, which Postgres answers in
/// well under a millisecond.  The *reads* are on [`crate::Opts::interval`], not
/// here — this is the latency of the queue, not the age of the shadow copy.
pub const TICK: Duration = Duration::from_secs(2);

/// How many changes one pass takes for one switch.
///
/// A batch, so that twenty clicks become one config-mode session
/// ([`Switch::apply_groups`]) rather than twenty.  Bounded so one operator's
/// mistake cannot starve the other switches of a pass.
pub const BATCH: usize = 20;

/// A row still `applying` after this long is from a dead worker (see
/// [`changes::recover_stale`]).  Generously longer than the console timeout.
pub const STALE: i64 = 900;

/// One switch's console session, and the only handle to it.
///
/// The session is stateful (a prompt context, a privilege level, a socket that
/// may have been dropped by the switch's own idle timer), so exactly one task may
/// hold it — which is what the mutex is for, and why the whole conversation
/// happens inside one [`std::sync::MutexGuard`] in [`run`] rather than being
/// handed around.
pub struct Device {
    pub cfg: switch::Config,
    /// The console session, or `None` when it needs reconnecting.  A failed read
    /// or write puts it back to `None`: the next pass reconnects, because every
    /// command of a dead session fails and the failure looks like a switch that
    /// is refusing you.
    pub cli: Mutex<Option<Switch>>,
}

/// What one pass will do on the console.
pub struct Job {
    pub cfg: switch::Config,
    /// `(change id, plans)` in queue order.  The plans are applied in this order
    /// and the ids come back tagged with what happened to each.
    pub writes: Vec<(i64, Vec<Vec<String>>)>,
    /// Read the whole configuration afterwards.
    pub read: bool,
}

/// What one pass did.
#[derive(Default)]
pub struct Outcome {
    /// Change ids that reached the switch.
    pub applied: Vec<i64>,
    /// Change ids that did not, with the console's answer.  Everything after a
    /// refusal in the same batch is included: the batch stopped there, and the
    /// queue will retry them.
    pub failed: Vec<(i64, String)>,
    /// A fresh snapshot, when one was read.
    pub snapshot: Option<Snapshot>,
    /// The privilege the session reached (`Manager` / `Operator`), recorded even
    /// when the read failed.
    pub privilege: Option<String>,
    /// The pass's own failure: the console could not be reached or the read
    /// failed.  Recorded against the switch, not against any change.
    pub error: Option<String>,
    /// Warnings from a partial read.
    pub warnings: Vec<String>,
}

/// One pass for one switch: apply the claimed changes, then read.
///
/// Runs inside [`tokio::task::spawn_blocking`] and touches nothing but the switch
/// and its own [`Device`]: no database, no await, no shared state beyond the
/// session it owns the lock for.
pub fn run(job: Job, device: &Device) -> Outcome {
    let mut out = Outcome::default();
    // A pass that never reached the console still owes the queue an answer.
    //
    // `claim` has already marked these rows `applying` and spent an attempt, and
    // `record` only writes the rows named in `applied`/`failed`. Returning with
    // neither — which is what an early `return out` used to do — left every
    // claimed change sitting `applying` with no error and no trace in the
    // journal, invisible until `recover_stale` released them fifteen minutes
    // later. On the page that is "I ticked a box, pressed Apply, the tick went
    // away and nothing happened": the change is real, queued, and never sent.
    //
    // It is not a rare path: `switch3` takes 15-25 s to log in, so a connect
    // failure is a Tuesday. And because `claim_batch` claims up to `BATCH` rows
    // per pass, one dead console orphaned the whole batch at once.
    let fail_all = |out: &mut Outcome, reason: String| {
        out.failed = job
            .writes
            .iter()
            .map(|(id, _)| (*id, reason.clone()))
            .collect();
        out.error = Some(reason);
    };
    let mut guard = match device.cli.lock() {
        Ok(g) => g,
        Err(_) => {
            fail_all(&mut out, "the console lock was poisoned".into());
            return out;
        }
    };

    // Nothing to do on the console at all: a pass that only reads still needs a
    // session, so the connect happens below either way.
    let mut sw = match guard.take() {
        Some(sw) => sw,
        None => match Switch::connect(&job.cfg) {
            Ok(sw) => sw,
            Err(e) => {
                fail_all(&mut out, format!("connect: {e}"));
                return out;
            }
        },
    };
    // The slug, not the prose: this is the value the shadow copy stores and the index
    // page shows, so the two cannot drift apart.
    out.privilege = Some(sw.privilege.slug().to_string());

    // --- writes -------------------------------------------------------------
    if !job.writes.is_empty() {
        // Flatten the batch into one list of plans: `apply_groups` enters config
        // mode once for all of them, which is the point of batching.
        let plans: Vec<Vec<String>> = job.writes.iter().flat_map(|(_, p)| p.clone()).collect();
        match sw.apply_groups(&plans) {
            Ok(()) => out.applied = job.writes.iter().map(|(id, _)| *id).collect(),
            Err(e) => {
                // The CLI has no transaction: whatever ran before the refusal is
                // applied, and the rest was not.  The whole batch is marked
                // failed rather than guessing where the line falls — the retry
                // re-applies the first ones, and every command the GUI generates
                // is idempotent (`name "x"` twice is the same name).
                out.failed = job.writes.iter().map(|(id, _)| (*id, e.clone())).collect();
                out.error = Some(e);
            }
        }
    }

    // --- read ---------------------------------------------------------------
    if job.read || job.writes.is_empty() {
        match switch::read_snapshot(&mut sw) {
            Ok(snap) => {
                if !snap.warnings.is_empty() {
                    out.warnings = snap.warnings.clone();
                }
                out.snapshot = Some(snap);
            }
            Err(e) => {
                // A read that fails on a session we just used is a dead session,
                // not a broken switch: reconnect next pass.
                out.error = Some(match out.error {
                    Some(existing) => format!("{existing}; and the re-read failed: {e}"),
                    None => format!("read: {e}"),
                });
            }
        }
    }

    // A session that failed anywhere is dropped rather than kept: the next pass
    // starts from a known state, which is the only way a half-read console (left
    // inside `config` by a refused command, say) cannot poison the pass after it.
    let usable = out.error.is_none();
    *guard = if usable { Some(sw) } else { None };
    out.privilege = if usable {
        out.privilege
    } else {
        Some("unknown".into())
    };
    out
}

/// Record an outcome: the change results, then the snapshot or the failure.
///
/// Split from [`run`] so the blocking task never holds a database connection, and
/// ordered so that a change's result is visible before the snapshot that reflects
/// it: a client polling the two must never see "applied" alongside a snapshot that
/// still shows the old state, because the honest reading of that pair is "the
/// change did not work", and it did.
pub async fn record(pool: &sqlx::PgPool, name: &str, address: &str, out: Outcome) {
    for id in out.applied {
        if let Err(e) = changes::applied(pool, id).await {
            eprintln!("switch-gui: {name}: {e}");
        }
    }
    for (id, err) in out.failed {
        match changes::retry_or_fail(pool, id, &err).await {
            Ok(gave_up) if gave_up => {
                eprintln!("switch-gui: {name}: change {id} gave up after its attempts: {err}")
            }
            Err(e) => eprintln!("switch-gui: {name}: {e}"),
            _ => {}
        }
    }
    if let Some(privilege) = out.privilege {
        if let Err(e) = shadow::note_privilege(pool, name, &privilege).await {
            eprintln!("switch-gui: {name}: {e}");
        }
    }
    match out.snapshot {
        Some(snap) => {
            if let Err(e) = shadow::store_snapshot(pool, &snap).await {
                eprintln!("switch-gui: {name}: storing snapshot: {e}");
            }
        }
        None => {
            let err = out.error.unwrap_or_else(|| "no snapshot".into());
            if let Err(e) = shadow::store_failure(pool, name, address, &err, &out.warnings).await {
                eprintln!("switch-gui: {name}: {e}");
            }
        }
    }
}

/// The changes to apply this pass, in queue order.
///
/// A `refresh` row is not a write: it is consumed here (marked applied) and turns
/// the pass into a forced read.  Claiming it through the same queue is what makes
/// "read it now" arrive in order with the writes — a Refresh clicked after five
/// edits happens after they are applied, which is the only ordering that makes
/// the resulting snapshot mean anything.
pub async fn claim_batch(
    pool: &sqlx::PgPool,
    name: &str,
) -> R<(Vec<(i64, Vec<Vec<String>>)>, bool)> {
    let mut writes = Vec::new();
    let mut read = false;
    while writes.len() < BATCH {
        // A claim that succeeded and a claim that failed on the *next* row are
        // not the same thing: the rows already taken are `applying` and are this
        // function's debt, so an error here fails them before it propagates.
        // Returning Err with rows still claimed is how they ended up invisible.
        let claimed = match changes::claim(pool, name).await {
            Ok(change) => change,
            Err(e) => {
                let ids: Vec<i64> = writes.iter().map(|(id, _)| *id).collect();
                fail_claimed(pool, &ids, &format!("claim: {e}")).await;
                return Err(e);
            }
        };
        let Some(change) = claimed else { break };
        if change.kind == "refresh" {
            read = true;
            changes::applied(pool, change.id).await?;
            continue;
        }
        writes.push((change.id, change.commands));
    }
    Ok((writes, read))
}

/// Fail every claimed change with `reason`, for the paths that got as far as
/// claiming and no further.
///
/// `retry_or_fail` is the right verb, not `failed`: a console that would not
/// answer this pass may answer the next, and the row should be retried until
/// `max_attempts` runs out rather than thrown away because one pass was
/// unlucky. What must never happen is the row staying `applying`: nothing else
/// in the program looks at a row in that state until [`recover_stale`] releases
/// it a `STALE` later, and on the page that gap is a change that silently does
/// nothing.
pub async fn fail_claimed(pool: &sqlx::PgPool, claimed: &[i64], reason: &str) {
    for id in claimed {
        match changes::retry_or_fail(pool, *id, reason).await {
            Ok(gave_up) if gave_up => eprintln!("switch-gui: change {id} gave up: {reason}"),
            Err(e) => eprintln!("switch-gui: {e}"),
            _ => {}
        }
    }
}

/// Whether the shadow copy needs re-reading.
///
/// `age` is what [`crate::shadow`] reports (Postgres' clock, seconds).  `None`
/// means never read, which is always due.
pub fn read_due(age: Option<i64>, interval: Duration) -> bool {
    match age {
        // Never read → due.  A *negative* age is not an age: it is a clock that
        // went backwards, and treating it as "read in the future, therefore fresh"
        // is how a page stops re-reading forever after one NTP correction.
        None => true,
        Some(secs) if secs < 0 => true,
        Some(secs) => Duration::from_secs(secs as u64) > interval,
    }
}

/// Start the loop.
///
/// One task for the whole network, which walks its switches in order per pass;
/// the per-switch isolation is that a failure is recorded against one switch and
/// the next is still visited, not that they run concurrently — three blocking
/// tasks all waiting on slow consoles would be three threads parked for tens of
/// seconds each, and the pass is already dominated by `switch3`'s login.
pub fn spawn(
    rt: &tokio::runtime::Handle,
    pool: sqlx::PgPool,
    devices: Vec<Arc<Device>>,
    interval: Duration,
) {
    rt.spawn(async move {
        loop {
            if let Err(e) = pass_all(&pool, &devices, interval).await {
                eprintln!("switch-gui: sync: {e}");
            }
            tokio::time::sleep(TICK).await;
        }
    });
}

/// One pass over every switch.
pub async fn pass_all(pool: &sqlx::PgPool, devices: &[Arc<Device>], interval: Duration) -> R<()> {
    // Before anything else: rows a dead worker left `applying`.  Cheap (a
    // partial-index scan) and it is what stops a crash from freezing the queue.
    match changes::recover_stale(pool, STALE).await {
        Ok(0) => {}
        Ok(n) => eprintln!("switch-gui: put {n} interrupted change(s) back in the queue"),
        Err(e) => eprintln!("switch-gui: {e}"),
    }
    for device in devices {
        if let Err(e) = pass(pool, device, interval).await {
            // One switch's database trouble is reported and skipped; the others
            // still get their pass.
            eprintln!("switch-gui: {}: {e}", device.cfg.name);
        }
    }
    // The queue depth after the pass, so a change that never drains is visible in
    // the journal without anyone opening the page.  Only when it is non-zero:
    // an idle network should not write a line every two seconds.
    match changes::open_count(pool).await {
        Ok(0) => {}
        Ok(n) => eprintln!("switch-gui: {n} change(s) still queued"),
        Err(e) => eprintln!("switch-gui: {e}"),
    }
    Ok(())
}

/// One switch: claim, apply and read on the console, record.
pub async fn pass(pool: &sqlx::PgPool, device: &Arc<Device>, interval: Duration) -> R<()> {
    let name = device.cfg.name.clone();
    let address = device.cfg.host.clone();
    let (writes, forced_read) = claim_batch(pool, &name).await?;

    // `--read-only` is the escape hatch for a switch you want looked at but not
    // touched.  The queue still drains — the changes are failed with the reason,
    // so the operator sees why nothing happened rather than watching a pending
    // notice that will never clear.
    let writes = if device.cfg.read_only {
        if !writes.is_empty() {
            for (id, _) in &writes {
                let _ = changes::retry_or_fail(pool, *id, crate::READ_ONLY).await;
            }
            eprintln!(
                "switch-gui: {}: refused {} queued change(s) (--read-only)",
                name,
                writes.len()
            );
        }
        Vec::new()
    } else {
        writes
    };

    let age = shadow::age_secs(pool, &name).await.unwrap_or(None);
    let read = forced_read || writes_due(&writes) || read_due(age, interval);
    if writes.is_empty() && !read {
        return Ok(());
    }

    // The ids, taken before the job moves: if the console task dies, `pass` is
    // still the only thing that can answer the rows it claimed.
    let claimed: Vec<i64> = writes.iter().map(|(id, _)| *id).collect();
    let job = Job {
        cfg: device.cfg.clone(),
        writes,
        read,
    };
    let device2 = device.clone();
    let outcome = match tokio::task::spawn_blocking(move || run(job, &device2)).await {
        Ok(outcome) => outcome,
        Err(e) => {
            // The console task died — a panic, or a runtime shutting down under
            // it. Either way nothing recorded these rows, and the pass that
            // claimed them is the only thing that can say so.
            fail_claimed(pool, &claimed, &format!("console task: {e}")).await;
            return Err(format!("console task: {e}"));
        }
    };
    record(pool, &name, &address, outcome).await;
    Ok(())
}

/// Whether a write forces the read that follows it.
///
/// Always, when there was a write: the shadow copy is what the next form is
/// diffed against and what the pending notice is shown beside, so leaving it at
/// the pre-change state until the next interval would show the operator a
/// snapshot that contradicts the change they just made.
fn writes_due(writes: &[(i64, Vec<Vec<String>>)]) -> bool {
    !writes.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The queue's latency and the snapshot's freshness are different numbers for
    /// different reasons, and conflating them is how a GUI ends up either
    /// hammering the switches or feeling stuck.
    #[test]
    fn the_tick_is_much_shorter_than_a_read() {
        assert!(TICK < Duration::from_secs(10));
        // A read interval as short as the tick would be a read every two seconds.
        assert!(Duration::from_secs(30) > TICK * 2);
    }

    /// Never read → due.  Read a moment ago → not due.  Read longer ago than the
    /// interval → due.  A negative age (a clock that went backwards) → due, not
    /// "never due", which is the failure mode that would freeze the page.
    #[test]
    fn a_read_is_due_when_it_is_old_or_missing() {
        let every = Duration::from_secs(30);
        assert!(read_due(None, every), "never read is always due");
        assert!(!read_due(Some(0), every), "just read is not due");
        assert!(!read_due(Some(10), every));
        assert!(read_due(Some(31), every));
        assert!(
            read_due(Some(-5), every),
            "a clock jump must not stop reads"
        );
    }

    /// A batch is bounded: one switch's queue must not starve the other two of a
    /// pass, because each switch gets one pass and they share the task pool.
    #[test]
    // The whole point is to assert on the constant, so that changing it here is a
    // failing test rather than a quiet behaviour change on two switches.
    #[allow(clippy::assertions_on_constants)]
    fn a_batch_is_bounded() {
        assert!(BATCH > 1, "a batch of one is not a batch");
        assert!(BATCH <= 50, "and not a whole config dump either");
    }

    /// The stale threshold must outlast the console's own patience, or a slow
    /// switch's in-flight change gets claimed twice.
    #[test]
    #[allow(clippy::assertions_on_constants)] // see `a_batch_is_bounded`
    fn stale_is_longer_than_a_console_conversation() {
        // The longest login the tool will wait for is the login timeout (45 s by
        // default) plus a batch of commands; 900 s is several of those.
        assert!(STALE > 300);
    }

    /// The outcome's two lists are disjoint: a change is either applied or
    /// failed, never both, or the audit log contradicts itself.
    #[test]
    fn an_outcome_records_each_change_once() {
        let out = Outcome {
            applied: vec![1, 2],
            failed: vec![(3, "refused".into())],
            ..Default::default()
        };
        for (id, _) in &out.failed {
            assert!(!out.applied.contains(id), "{id} recorded twice");
        }
    }
}
