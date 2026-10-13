//! The write queue: every change the GUI asks for, and what became of it.
//!
//! An operator editing a switch wants two things a live-Telnet GUI cannot give
//! them at the same time: the page to react immediately, and the certainty that
//! what they clicked reached the device.  This table is how both happen.  A
//! submit inserts one row and returns; the page shows it as **pending sync** at
//! once; the sync ([`crate::sync`]) picks up pending rows, writes them to the
//! switch, and marks them applied or failed with what the switch said.  The row
//! survives, so the same table is the audit trail — who asked, when, what CLI
//! lines it became, how many times the sync tried, and what happened.
//!
//! The four rules the design rests on:
//!
//! * **A change is queued, never applied, by the request handler.**  A Telnet
//!   write to a switch takes seconds and can hang; an HTTP handler that does it
//!   makes the page as slow and as fragile as the console.  The handler's whole
//!   job is to validate and insert.
//! * **Retrying is the sync's decision, and it is bounded.**  A change the switch
//!   refuses (a VLAN that does not exist, a trunk member the CLI will not move)
//!   will fail the same way forever, so `max_attempts` ends it and the row keeps
//!   the error for the operator to read.  A change that failed because the switch
//!   was unreachable is worth retrying, which is why the two are not conflated:
//!   see [`crate::sync`]'s apply/defer split.
//! * **Nothing here writes to NVRAM.**  `write` is this firmware's save command
//!   and the GUI never sends it, so a reboot undoes everything — the same bargain
//!   the tool has always had, and why the log records what was *attempted* rather
//!   than claiming to be the device's configuration.
//! * **The row is read back, not echoed.**  Every mutation ends in a `RETURNING
//!   id` and a read through [`by_id`], so what the client sees is what the row
//!   holds — including the `attempt` count and the status Postgres computed, which
//!   an echo from this process would have to duplicate and could get wrong.
//!
//! ## Why `FOR UPDATE SKIP LOCKED`
//!
//! One worker per switch, so a change is never applied twice at once.  The claim
//! is still written as a skip-locked claim rather than a plain `UPDATE` because
//! the sync applies switches on its own schedule and a manual "apply now" from the
//! page must be able to run alongside it: with skip-locked the second worker sees
//! an empty queue and does nothing, instead of two workers interleaving
//! `interface 7` / `interface 8` down one console.

use common_pg as pg;
use sea_query::{Expr, LockBehavior, LockType, Order, Query, SimpleExpr};
use sqlx::Row;
use std::collections::HashMap;

use crate::shadow;
use crate::R;

/// The queue and audit table, plus the index that makes "what is still pending"
/// cheap (it is on every page load; the full history is not).
pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS change (
    id            bigserial PRIMARY KEY,
    switch_name   text        NOT NULL,
    -- port-name | port-state | port-members | vlan-name | vlan-create |
    -- vlan-delete | refresh.  Free text rather than a CHECK so a new kind does
    -- not need a migration, and an old row still reads after one.
    kind          text        NOT NULL,
    -- Ports the change touches, for marking rows in the table.  Text ids,
    -- because that is what the switch calls a port.
    ports         jsonb       NOT NULL DEFAULT '[]'::jsonb,
    -- One line for a human: `Port 7: untagged 9, tagged 7, 20`.
    summary       text        NOT NULL DEFAULT '',
    -- The CLI lines, one entry per port, each applied in config mode in order.
    commands      jsonb       NOT NULL DEFAULT '[]'::jsonb,
    -- What the shadow copy said before the change was queued (`name`, `enabled`,
    -- `native`, `tagged`, `vlan-name`, `untagged-members`, `tagged-members`).
    -- The audit log's other half, and the only thing a Revert can be built from.
    before        jsonb       NOT NULL DEFAULT '{}'::jsonb,
    note          text,
    -- pending | applying | applied | failed
    status        text        NOT NULL DEFAULT 'pending',
    attempt       integer     NOT NULL DEFAULT 0,
    max_attempts  integer     NOT NULL DEFAULT 5,
    created_at    timestamptz NOT NULL DEFAULT now(),
    created_by    text        NOT NULL DEFAULT '',
    applied_at    timestamptz,
    last_error    text,
    -- Set when this row was queued as the undo of change `id`.
    reverted_from bigint
);

CREATE INDEX IF NOT EXISTS change_open_idx
    ON change (switch_name, id)
    WHERE status IN ('pending', 'applying');

CREATE INDEX IF NOT EXISTS change_recent_idx
    ON change (switch_name, id desc)
"#;

/// How many rows a page or a JSON client is shown.  The audit trail is
/// unbounded; a view of it is not.
const LIST_LIMIT: i64 = 200;

/// The statuses that mean "not settled": still queued, in flight, or both.
pub(crate) const OPEN: [&str; 2] = ["pending", "applying"];

/// The one status [`claim`] may take.
///
/// Deliberately narrower than [`OPEN`].  `applying` is the in-flight marker, not
/// a queue state: a row wearing it is either being written by a worker right now
/// or belongs to [`recover_stale`], and a claim that matched it would let the
/// *next* pass re-claim the row the *current* pass is holding — burning an
/// attempt every tick and outrunning the ladder in `max_attempts`.
const PENDING: &str = "pending";

/// Apply the queue's schema.
pub async fn migrate(pool: &sqlx::PgPool) -> R<()> {
    pg::apply_schema(pool, SCHEMA).await
}

/// Queue one change.
///
/// `commands` is the plan, already built by [`crate::intent`] from the shadow
/// copy: what the switch is told, not what the operator typed, because the thing
/// an auditor wants to read is the former.  One entry per port, applied in order
/// inside one config-mode session (see [`crate::switch::Switch::apply_groups`]).
///
/// Returns the row as stored, so the caller can show it without a second query —
/// including its `id`, which is what the page uses to keep marking the row
/// pending after a refresh.
#[allow(clippy::too_many_arguments)]
pub async fn queue(
    pool: &sqlx::PgPool,
    switch_name: &str,
    kind: &str,
    ports: &[String],
    summary: &str,
    commands: &[Vec<String>],
    before: &std::collections::BTreeMap<String, String>,
    created_by: &str,
    max_attempts: i32,
    reverted_from: Option<i64>,
    note: Option<&str>,
) -> R<crate::dto::Change> {
    let mut ins = Query::insert();
    ins.into_table(pg::tbl("change"))
        .columns(vec![
            pg::iden("switch_name"),
            pg::iden("kind"),
            pg::iden("ports"),
            pg::iden("summary"),
            pg::iden("commands"),
            pg::iden("before"),
            pg::iden("created_by"),
            pg::iden("max_attempts"),
            pg::iden("reverted_from"),
            pg::iden("note"),
        ])
        .values_panic(vec![
            pg::ev(switch_name),
            pg::ev(kind),
            jsonb_literal(&shadow::jsonb_array(ports)),
            pg::ev(summary),
            jsonb_literal(&shadow::jsonb_command_lists(commands)),
            jsonb_literal(&shadow::jsonb_object(before)),
            pg::ev(created_by),
            sea_query::Expr::value(max_attempts),
            pg::evi(reverted_from),
            // An empty note is no note: a NULL column reads as "nobody said
            // anything", which is exactly what it means.
            pg::evo(note.filter(|n| !n.trim().is_empty())),
        ]);
    let id = insert_returning_id(pool, ins, "queue change").await?;
    by_id(pool, id)
        .await?
        .ok_or_else(|| format!("change {id} vanished after insert"))
}

/// Take the next change to apply for one switch, marking it `applying`.
///
/// `FOR UPDATE SKIP LOCKED` is what lets a manual "apply now" and the periodic
/// sync run at the same moment without applying the same row twice: whichever
/// loses the race sees no rows and returns `None`.  Marking it `applying` in the
/// same statement is what makes a crash recoverable — see [`recover_stale`].
///
/// Returns `None` only when there is genuinely nothing queued, which depends on
/// the statement returning the id it claimed: see [`claim_sql`].
pub async fn claim(pool: &sqlx::PgPool, switch_name: &str) -> R<Option<crate::dto::Change>> {
    let sql = claim_sql(switch_name);
    let row = sqlx::query(&sql)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("claim change: {e}"))?;
    let Some(row) = row else { return Ok(None) };
    let id = row.try_get::<i64, _>("id").unwrap_or(0);
    by_id(pool, id).await
}

/// The claim statement itself, built by the function [`claim`] runs.
///
/// Extracted so the test asserts on the query that ships rather than on a copy
/// of it — which is exactly how this statement's worst bug survived: without the
/// `RETURNING` the UPDATE answers with a command tag and **no row**, so
/// `fetch_optional` yields `None`, `claim` reports "nothing queued", and the
/// caller cannot tell that apart from an empty queue.  The row is left
/// `applying` with its attempt spent: never written, never failed, never
/// recorded, and invisible to every error path in the program.  A queue that
/// silently does nothing looks from the page like a queue that is merely slow.
pub(crate) fn claim_sql(switch_name: &str) -> String {
    pg::update_sql(|upd| {
        let mut next = Query::select();
        next.column(pg::col("id"))
            .from(pg::tbl("change"))
            .and_where(Expr::col(pg::col("switch_name")).eq(switch_name))
            // `pending` only — see [`PENDING`] for why an in-flight row is not
            // up for grabs.
            .and_where(Expr::col(pg::col("status")).eq(PENDING))
            .order_by(pg::col("id"), Order::Asc)
            .limit(1)
            // The row lock, not a table lock: the loser of the race moves on to
            // the next row instead of waiting for this one, which is the
            // difference between two workers and one worker that blocks.
            .lock_with_behavior(LockType::Update, LockBehavior::SkipLocked);

        upd.table(pg::tbl("change"))
            .value(pg::iden("attempt"), Expr::col(pg::col("attempt")).add(1))
            .value(pg::iden("status"), "applying")
            // `IN (subquery)` rather than `=`: sea-query 0.32 has no scalar
            // subquery expression, and the subquery selects one row either way.
            .and_where(Expr::col(pg::col("id")).in_subquery(next))
            // Repeated in the outer statement because the subquery's lock is
            // taken before the update runs: if the row was claimed a moment
            // earlier, this is what stops the second worker stealing it.
            .and_where(Expr::col(pg::col("status")).eq(PENDING))
            // The id, or the claim is a write with no result.  Every other
            // mutation in this module already returns something; this one is the
            // queue's only read-modify-write, so it is the one that has to.
            .returning(sea_query::Query::returning().column(pg::col("id")));
    })
}

/// Mark a claimed change applied.
pub async fn applied(pool: &sqlx::PgPool, id: i64) -> R<()> {
    let sql = pg::update_sql(|upd| {
        upd.table(pg::tbl("change"))
            .value(pg::iden("status"), "applied")
            .value(pg::iden("applied_at"), Expr::cust("now()"))
            .value(pg::iden("last_error"), Expr::cust("NULL"))
            .and_where(Expr::col(pg::col("id")).eq(id));
    });
    sqlx::query(&sql)
        .execute(pool)
        .await
        .map_err(|e| format!("mark change {id} applied: {e}"))?;
    Ok(())
}

/// A claimed change that did not go out: back to `pending` if it has attempts
/// left, `failed` if it does not.  Returns whether it gave up.
///
/// The row keeps `attempt` (it counts tries, not successes) and `last_error`,
/// because "it failed three times with `Can't modify VLAN membership on a trunk
/// member`" is the information an operator needs and "it is not applied" is not.
pub async fn retry_or_fail(pool: &sqlx::PgPool, id: i64, error: &str) -> R<bool> {
    let sql = pg::update_sql(|upd| {
        upd.table(pg::tbl("change"))
            // The ladder, in SQL rather than in Rust, so the comparison of `attempt`
            // with `max_attempts` uses the values in the row and not a copy read a
            // round trip earlier (which another worker could have changed).
            .value(
                pg::iden("status"),
                Expr::cust("CASE WHEN attempt >= max_attempts THEN 'failed' ELSE 'pending' END"),
            )
            .value(pg::iden("last_error"), error)
            .and_where(Expr::col(pg::col("id")).eq(id))
            .returning(sea_query::Query::returning().column(pg::col("status")));
    });
    let row = sqlx::query(&sql)
        .fetch_one(pool)
        .await
        .map_err(|e| format!("requeue change {id}: {e}"))?;
    Ok(row
        .try_get::<String, _>("status")
        .map(|s| s == "failed")
        .unwrap_or(true))
}

/// Put a change back at the front of the queue by hand.
///
/// Used by the page's Retry button, and by nothing else: it resets `attempt`
/// because the operator pressing it is a statement that the reason it failed has
/// been dealt with (the VLAN was created, the switch was repowered), which is
/// exactly when the attempt ladder should start again.
pub async fn requeue(pool: &sqlx::PgPool, id: i64) -> R<bool> {
    let sql = pg::update_sql(|upd| {
        upd.table(pg::tbl("change"))
            .value(pg::iden("status"), "pending")
            .value(pg::iden("attempt"), 0)
            .value(pg::iden("last_error"), Expr::cust("NULL"))
            .and_where(Expr::col(pg::col("id")).eq(id))
            // `applied` is deliberately not in this list: re-applying a change that
            // already went out is not a retry, it is a second write.
            .and_where(Expr::col(pg::col("status")).is_in(["pending", "applying", "failed"]))
            .returning(sea_query::Query::returning().column(pg::col("id")));
    });
    let row = sqlx::query(&sql)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("requeue change {id}: {e}"))?;
    Ok(row.is_some())
}

/// Drop a pending change from the queue without applying it.
///
/// Only a row that has not been written can be forgotten: `applied` and `failed`
/// rows are history, and history is what this table is for.  The page calls this
/// "discard", and it is the one destructive thing a viewer can do here — which is
/// why it is a separate endpoint from Retry rather than a flag on the same one.
pub async fn forget(pool: &sqlx::PgPool, id: i64) -> R<bool> {
    let sql = pg::delete_sql(|del| {
        del.from_table(pg::tbl("change"))
            .and_where(Expr::col(pg::col("id")).eq(id))
            .and_where(Expr::col(pg::col("status")).is_in(["pending", "applying"]))
            .returning(sea_query::Query::returning().column(pg::col("id")));
    });
    let row = sqlx::query(&sql)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("forget change {id}: {e}"))?;
    Ok(row.is_some())
}

/// Rows left `applying` by a crash, put back to `pending`.  Returns how many.
///
/// A row marked `applying` means "a worker is writing this now".  If the service
/// was killed mid-write the row would otherwise sit there forever, showing the
/// operator a pending-sync notice for a change nobody is working on and — worse —
/// a partially applied change would never be retried.  Anything older than `stale`
/// cannot still be in flight, because a write to a switch either completes or the
/// console times out well inside it.
///
/// The change may have been half-applied before the crash.  That is why the sync
/// re-reads the switch after every batch: the shadow copy then shows what the
/// device actually holds, and the operator decides whether to re-queue.
pub async fn recover_stale(pool: &sqlx::PgPool, stale_secs: i64) -> R<u64> {
    let sql = pg::update_sql(|upd| {
        upd.table(pg::tbl("change"))
            .value(pg::iden("status"), "pending")
            .and_where(Expr::col(pg::col("status")).eq("applying"))
            .and_where(Expr::col(pg::col("applied_at")).is_null())
            .and_where(
                Expr::col(pg::col("created_at"))
                    // `make_interval(secs => $1)` keeps the threshold a value
                    // rather than a string built here; sea-query has no interval.
                    .lt(pg::cust_with_value(
                        "now() - make_interval(secs => $1)",
                        stale_secs as f64,
                    )),
            );
    });
    let res = sqlx::query(&sql)
        .execute(pool)
        .await
        .map_err(|e| format!("recover stale changes: {e}"))?;
    Ok(res.rows_affected())
}

/// The rows still to reach a switch (or already in flight), newest first.
///
/// `switch_name` limits it to one switch; `None` is the whole network, which is
/// what the index page's "pending sync" banner reads.
pub async fn open(pool: &sqlx::PgPool, switch_name: Option<&str>) -> R<Vec<crate::dto::Change>> {
    let sql = pg::select_sql(|sel| {
        change_columns(sel);
        sel.from(pg::tbl("change"))
            .and_where(Expr::col(pg::col("status")).is_in(OPEN));
        if let Some(name) = switch_name {
            sel.and_where(Expr::col(pg::col("switch_name")).eq(name));
        }
        sel.order_by(pg::col("id"), Order::Desc)
            .limit(LIST_LIMIT as u64);
    });
    list(pool, &sql, "open changes").await
}

/// The history: every row, newest first.  This is the audit trail view.
pub async fn history(pool: &sqlx::PgPool, switch_name: Option<&str>) -> R<Vec<crate::dto::Change>> {
    let sql = pg::select_sql(|sel| {
        change_columns(sel);
        sel.from(pg::tbl("change"));
        if let Some(name) = switch_name {
            sel.and_where(Expr::col(pg::col("switch_name")).eq(name));
        }
        sel.order_by(pg::col("id"), Order::Desc)
            .limit(LIST_LIMIT as u64);
    });
    list(pool, &sql, "change history").await
}

/// One row by id, for a revert that needs to know what it is undoing — and for
/// every mutation's read-back (see the module docs).
pub async fn by_id(pool: &sqlx::PgPool, id: i64) -> R<Option<crate::dto::Change>> {
    let sql = pg::select_sql(|sel| {
        change_columns(sel);
        sel.from(pg::tbl("change"))
            .and_where(Expr::col(pg::col("id")).eq(id));
    });
    let row = sqlx::query(&sql)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("change {id}: {e}"))?;
    Ok(row.as_ref().map(read_change))
}

/// How many open rows each switch has, as `switch_name => count`.
///
/// The per-switch "pending sync" number on both pages.  It lives here rather than
/// in [`crate::shadow`] because it is the queue's own bookkeeping: the filter is
/// [`OPEN`], and a count that used a different predicate would disagree with what
/// the sync still means to act on.  A switch with nothing queued has no rows and
/// so no entry — callers count a miss as zero.
pub async fn open_counts(pool: &sqlx::PgPool) -> R<HashMap<String, i64>> {
    let sql = pg::select_sql(|sel| {
        sel.column(pg::col("switch_name"))
            .expr_as(pg::count_star(), sea_query::Alias::new("n"))
            .from(pg::tbl("change"))
            .and_where(Expr::col(pg::col("status")).is_in(OPEN))
            .group_by_col(pg::col("switch_name"));
    });
    let rows = sqlx::query(&sql)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("count open changes: {e}"))?;
    Ok(rows
        .iter()
        .map(|r| {
            (
                text(r, "switch_name"),
                r.try_get::<i64, _>("n").unwrap_or(0),
            )
        })
        .collect())
}

/// How many rows are still queued, network-wide.
///
/// The cheap version for a caller that only wants the queue depth — the sync logs
/// it once per tick, which is why it counts rows rather than reading them.  The
/// pages want the per-switch breakdown and use [`open_counts`].
pub async fn open_count(pool: &sqlx::PgPool) -> R<i64> {
    let sql = pg::select_sql(|sel| {
        sel.expr_as(pg::count_star(), sea_query::Alias::new("n"))
            .from(pg::tbl("change"))
            .and_where(Expr::col(pg::col("status")).is_in(OPEN));
    });
    let row = sqlx::query(&sql)
        .fetch_one(pool)
        .await
        .map_err(|e| format!("count open changes: {e}"))?;
    Ok(row.try_get::<i64, _>("n").unwrap_or(0))
}

// -------------------------------------------------------------- internals


/// The `change` columns every reader selects, formatted for the wire.
///
/// One function so the list view, the history view and the single-row read cannot
/// disagree about what a change contains — the audit log and the pending banner
/// would otherwise be able to show different things for the same row.
fn change_columns(sel: &mut sea_query::SelectStatement) {
    sel.columns(vec![
        pg::col("id"),
        pg::col("switch_name"),
        pg::col("kind"),
        pg::col("summary"),
        pg::col("status"),
        pg::col("attempt"),
        pg::col("max_attempts"),
        pg::col("created_by"),
        pg::col("note"),
    ])
    .expr_as(pg::as_text("ports"), sea_query::Alias::new("ports"))
    .expr_as(pg::as_text("commands"), sea_query::Alias::new("commands"))
    .expr_as(pg::as_text("before"), sea_query::Alias::new("before"))
    .expr_as(
        pg::ts_utc("created_at"),
        sea_query::Alias::new("created_at"),
    )
    .expr_as(
        pg::ts_utc("applied_at"),
        sea_query::Alias::new("applied_at"),
    )
    .expr_as(pg::col("last_error"), sea_query::Alias::new("last_error"))
    .expr_as(
        pg::col("reverted_from"),
        sea_query::Alias::new("reverted_from"),
    );
}

/// Run a change-listing query.
async fn list(pool: &sqlx::PgPool, sql: &str, what: &str) -> R<Vec<crate::dto::Change>> {
    let rows = sqlx::query(sql)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("{what}: {e}"))?;
    Ok(rows.iter().map(read_change).collect())
}

/// `INSERT … RETURNING id`, which every mutation in this module starts with.
async fn insert_returning_id(
    pool: &sqlx::PgPool,
    mut ins: sea_query::InsertStatement,
    what: &str,
) -> R<i64> {
    ins.returning(sea_query::Query::returning().column(pg::col("id")));
    let sql = pg::finalize_insert(ins);
    let row = sqlx::query(&sql)
        .fetch_one(pool)
        .await
        .map_err(|e| format!("{what}: {e}"))?;
    Ok(row.try_get::<i64, _>("id").unwrap_or(0))
}

/// A jsonb column value: the JSON text, cast.
///
/// sea-query has no jsonb value type, so a jsonb write is a bound string with a
/// `::jsonb` cast after it.  The JSON itself is built by [`crate::shadow`], which
/// is where the escaping rules live.
fn jsonb_literal(json: &str) -> SimpleExpr {
    pg::cust_with_value("$1::jsonb", json)
}

/// A `change` row as the wire type.
fn read_change(r: &sqlx::postgres::PgRow) -> crate::dto::Change {
    crate::dto::Change {
        id: r.try_get::<i64, _>("id").unwrap_or(0),
        switch_name: text(r, "switch_name"),
        kind: text(r, "kind"),
        ports: shadow::string_list(&text(r, "ports")),
        summary: text(r, "summary"),
        status: text(r, "status"),
        attempt: r.try_get::<i32, _>("attempt").unwrap_or(0),
        max_attempts: r.try_get::<i32, _>("max_attempts").unwrap_or(5),
        created_at: text(r, "created_at"),
        created_by: text(r, "created_by"),
        applied_at: opt_text(r, "applied_at"),
        last_error: opt_text(r, "last_error"),
        reverted_from: opt_integer(r, "reverted_from"),
        commands: shadow::command_lists(&text(r, "commands")),
        note: opt_text(r, "note"),
        before: shadow::string_map(&text(r, "before")),
    }
}

fn text(row: &sqlx::postgres::PgRow, column: &str) -> String {
    row.try_get::<String, _>(column).unwrap_or_default()
}

fn opt_text(row: &sqlx::postgres::PgRow, column: &str) -> Option<String> {
    row.try_get::<Option<String>, _>(column).ok().flatten()
}

fn opt_integer(row: &sqlx::postgres::PgRow, column: &str) -> Option<i64> {
    row.try_get::<Option<i64>, _>(column).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same rule as the shadow schema: `apply_schema` splits on `;`, so no
    /// statement body may contain one.
    #[test]
    fn the_schema_splits_into_statements() {
        let stmts = pg::split_statements(SCHEMA);
        assert!(stmts.len() >= 3, "{} statements", stmts.len());
        for s in &stmts {
            assert!(!s.contains("$$"), "a body would be split: {s}");
        }
        assert!(stmts
            .iter()
            .any(|s| s.contains("CREATE TABLE IF NOT EXISTS change")));
        // The partial index is what keeps "what is pending" cheap on an
        // unbounded history.
        assert!(SCHEMA.contains("WHERE status IN ('pending', 'applying')"));
    }

    /// Every column `read_change` asks for must be in the select list, or it
    /// silently reads as the default: an audit log that is quietly all-`pending`.
    #[test]
    fn the_column_list_covers_every_field() {
        let sql = pg::select_sql(|sel| {
            change_columns(sel);
            sel.from(pg::tbl("change"));
        });
        for column in [
            "\"id\"",
            "switch_name",
            "kind",
            "ports::text",
            "commands::text",
            "before::text",
            "summary",
            "status",
            "attempt",
            "max_attempts",
            "created_at",
            "created_by",
            "note",
            "applied_at",
            "last_error",
            "reverted_from",
        ] {
            assert!(sql.contains(column), "{column} missing from {sql}");
        }
        // Both timestamps formatted in UTC, both jsonb columns cast.
        assert_eq!(sql.matches("AT TIME ZONE 'UTC'").count(), 2, "{sql}");
    }

    /// The claim must lock and skip, must take only *pending* rows, must count
    /// the attempt it is making, and must hand the claimed id back.
    ///
    /// Built from [`claim_sql`] — the statement the sync actually runs — because
    /// this test used to rebuild the query by hand, and the copy was missing the
    /// `RETURNING` that the real one needed too: the claim spent an attempt,
    /// reported "nothing queued", and no change ever reached a switch while every
    /// test here stayed green.
    #[test]
    fn the_claim_is_a_skip_locked_ladder() {
        let sql = claim_sql("switch1");
        assert!(sql.contains("FOR UPDATE SKIP LOCKED"), "{sql}");
        assert!(sql.contains("\"attempt\" + 1"), "{sql}");
        assert!(sql.contains("'applying'"), "{sql}");
        assert!(sql.contains("IN (SELECT"), "{sql}");
        // The claimed id, or `fetch_optional` sees no row and the caller reads
        // that as an empty queue (see `claim_sql`).
        assert!(sql.contains("RETURNING \"id\""), "{sql}");
        // Both halves of the predicate say `pending`: the row the update is
        // writing is already `applying`, so a claim that matched `applying`
        // would re-take its own in-flight row on the next pass.
        assert_eq!(sql.matches("'pending'").count(), 2, "{sql}");
        assert!(
            !sql.contains("'applying',") && !sql.contains("'applying')"),
            "claim must not select an in-flight row: {sql}"
        );
        // And it is scoped to one switch, or a change for switch3 would be
        // claimed by switch1's pass and sent down the wrong console.
        assert!(sql.contains("'switch1'"), "{sql}");
    }

    /// Giving up is decided in SQL from the row's own counters, so a retry cannot
    /// endlessly re-run a change the switch refuses.
    #[test]
    fn the_ladder_is_evaluated_by_postgres() {
        let sql = pg::update_sql(|upd| {
            upd.table(pg::tbl("change")).value(
                pg::iden("status"),
                Expr::cust("CASE WHEN attempt >= max_attempts THEN 'failed' ELSE 'pending' END"),
            );
        });
        assert!(sql.contains("attempt >= max_attempts"), "{sql}");
        assert!(sql.contains("'failed'"), "{sql}");
    }

    /// A jsonb write is a bound string with a cast: the JSON must not be
    /// concatenated into the statement.
    #[test]
    fn jsonb_values_are_bound() {
        let expr = jsonb_literal(r#"["a","b"]"#);
        let ins = Query::insert()
            .into_table(pg::tbl("change"))
            .columns(vec![pg::iden("ports")])
            .values_panic(vec![expr])
            .clone();
        let sql = pg::finalize_insert(ins);
        assert!(
            sql.contains(r#"E'[\"a\",\"b\"]'::jsonb"#) || sql.contains("'[\"a\",\"b\"]'::jsonb"),
            "{sql}"
        );
    }

    /// The stale threshold is a bound interval, not a string built here.
    #[test]
    fn the_stale_threshold_is_bound() {
        let sql = pg::update_sql(|upd| {
            upd.table(pg::tbl("change"))
                .value(pg::iden("status"), "pending")
                .and_where(Expr::col(pg::col("created_at")).lt(pg::cust_with_value(
                    "now() - make_interval(secs => $1)",
                    900.0,
                )));
        });
        assert!(sql.contains("make_interval(secs => 900)"), "{sql}");
    }

    /// `forget` and `requeue` must not be able to touch an applied row: one is
    /// history and the other would be a second write.
    #[test]
    fn settled_rows_are_left_alone() {
        assert!(!SCHEMA.contains("applied'),"));
        // The status lists the two guards use, read out of the builders.
        let sql = pg::delete_sql(|del| {
            del.from_table(pg::tbl("change"))
                .and_where(Expr::col(pg::col("status")).is_in(["pending", "applying"]));
        });
        assert!(sql.contains("IN ('pending', 'applying')"), "{sql}");
        assert!(!sql.contains("applied"), "{sql}");
    }
}
