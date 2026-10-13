//! The shadow copy: the Postgres state the GUI reads instead of the switches.
//!
//! Reading three ProCurve switches takes seconds — six-plus `show` commands each,
//! against a console from the 1990s that pages its own output — so the page must
//! not be the thing that does it.  A background sync ([`crate::sync`]) owns the
//! Telnet sessions and writes what it reads here; every HTTP handler in
//! [`crate::routes`] answers from here, which is what makes the GUI load in
//! milliseconds and lets a second browser tab see the same state.
//!
//! Three tables, one snapshot per switch: a `switch` row (the device, its
//! `show system` identity, when it was last read and what went wrong if it was
//! not), a `vlan` row per VLAN and a `port` row per port or trunk group.  A
//! snapshot is written in one transaction, so a reader never sees half a switch:
//! the old snapshot is replaced by the new one atomically, and a failed read
//! leaves the previous snapshot in place with an error recorded instead.  That is
//! the same rule the in-memory cache it replaces used — a failed refresh must not
//! destroy the last good read — now durable across a restart of the service.
//!
//! Queries are built with sea-query through [`common_pg`], the same as
//! gocardless-dashboard, including the four places that need Postgres' own
//! vocabulary (`to_char`, `EXTRACT(EPOCH …)`, a jsonb `::text` cast and a `@>`
//! containment test) — those are [`common_pg::ts_utc`] and friends, kept in the
//! shared crate so the wire format of a timestamp is defined once.
//!
//! Timestamps leave this module as strings Postgres formatted in UTC, and ages
//! leave it as seconds Postgres computed: the browser's clock is never compared to
//! a switch's read time (see `dto/src/lib.rs`).

use common_pg as pg;
use sea_query::extension::postgres::PgExpr;
use sea_query::{Alias, ColumnRef, Expr, OnConflict, Order, SimpleExpr};
use sqlx::Row;
use std::collections::{BTreeMap, HashMap};

use crate::model::{Port, Snapshot, Vlan};
use crate::R;

/// The shadow-copy schema.  Idempotent: it runs on every service start.
pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS switch (
    name         text PRIMARY KEY,
    address      text        NOT NULL DEFAULT '',
    system_name  text        NOT NULL DEFAULT '',
    location     text        NOT NULL DEFAULT '',
    software     text        NOT NULL DEFAULT '',
    uptime       text        NOT NULL DEFAULT '',
    serial       text        NOT NULL DEFAULT '',
    -- Manager / Operator / unknown: a property of the live console, recorded by
    -- whichever sync last held a session.  The page shows it because every write
    -- the GUI queues needs Manager, and a failed change is the worst time to
    -- find that out.
    privilege    text        NOT NULL DEFAULT 'unknown',
    fetched_at   timestamptz,
    error        text,
    warnings     jsonb       NOT NULL DEFAULT '[]'::jsonb
);

CREATE TABLE IF NOT EXISTS vlan (
    switch_name  text   NOT NULL REFERENCES switch(name) ON DELETE CASCADE,
    id           bigint NOT NULL,
    name         text   NOT NULL DEFAULT '',
    status       text   NOT NULL DEFAULT '',
    jumbo        text   NOT NULL DEFAULT '',
    PRIMARY KEY (switch_name, id)
);

CREATE TABLE IF NOT EXISTS port (
    switch_name    text    NOT NULL REFERENCES switch(name) ON DELETE CASCADE,
    -- The switch's own identity for a port: `1`..`48`, or `Trk1`.  Text because
    -- the CLI takes and prints text, and a trunk group is not a number.
    id             text    NOT NULL,
    name           text    NOT NULL DEFAULT '',
    media          text    NOT NULL DEFAULT '',
    stp_state      text    NOT NULL DEFAULT '',
    stp_state_from text    NOT NULL DEFAULT '',
    -- NULL means "in no VLAN at all", which is not the same as `false` (in a
    -- VLAN, link down), and the page renders them differently.
    link_up        boolean,
    trunk_group    text    NOT NULL DEFAULT '',
    trunk_type     text    NOT NULL DEFAULT '',
    native_vlan    bigint,
    tagged         jsonb   NOT NULL DEFAULT '[]'::jsonb,
    enabled        boolean NOT NULL DEFAULT true,
    has_counters   boolean NOT NULL DEFAULT false,
    errors_rx      text    NOT NULL DEFAULT '',
    drops_rx       text    NOT NULL DEFAULT '',
    PRIMARY KEY (switch_name, id)
);
"#;

/// The `switch` columns every page query needs.
///
/// A function rather than a `&str` so the two readers cannot drift: `page()` and
/// `index()` must agree on what "this switch's age" means.
fn switch_cols() -> Vec<ColumnRef> {
    vec![
        pg::col("name"),
        pg::col("address"),
        pg::col("system_name"),
        pg::col("location"),
        pg::col("software"),
        pg::col("uptime"),
        pg::col("serial"),
        pg::col("privilege"),
        pg::col("error"),
    ]
}

/// The formatted pair every reader wants alongside the columns: when, and how
/// stale.  Aliased to the names the DTO fields serialise to.
fn switch_freshness() -> Vec<(SimpleExpr, &'static str)> {
    vec![
        (pg::ts_utc("fetched_at"), "fetched_at"),
        (pg::age_secs("fetched_at"), "age_secs"),
        (pg::as_text("warnings"), "warnings"),
    ]
}

/// The `port` columns, in the order [`model_port`] reads them.
fn port_cols() -> Vec<ColumnRef> {
    vec![
        pg::col("id"),
        pg::col("name"),
        pg::col("media"),
        pg::col("stp_state"),
        pg::col("stp_state_from"),
        pg::col("link_up"),
        pg::col("trunk_group"),
        pg::col("trunk_type"),
        pg::col("native_vlan"),
        pg::col("enabled"),
        pg::col("has_counters"),
        pg::col("errors_rx"),
        pg::col("drops_rx"),
    ]
}

/// Apply the schema and make sure a row exists for every configured switch.
///
/// The row is inserted before anything has been read, so the index page lists a
/// switch that has never answered as a switch that has never answered rather than
/// not listing it at all — and so a switch dropped from `common/hosts.nix` keeps
/// its history until someone deletes the row deliberately.
pub async fn migrate(pool: &sqlx::PgPool, switches: &[(String, String)]) -> R<()> {
    pg::apply_schema(pool, SCHEMA).await?;
    for (name, address) in switches {
        let sql = pg::insert_sql(|ins| {
            ins.into_table(pg::tbl("switch"))
                .columns(vec![pg::iden("name"), pg::iden("address")])
                .values_panic(vec![pg::ev(name), pg::ev(address)]);
            // Re-running a switch must refresh the address (a box that moved is the
            // whole reason `common/hosts.nix` is the single table of addresses) and
            // nothing else: `fetched_at`, the identity columns and the error from the
            // last read are history the page shows.
            ins.on_conflict(
                OnConflict::column(pg::iden("name"))
                    .update_column(pg::iden("address"))
                    .to_owned(),
            );
        });
        sqlx::query(&sql)
            .execute(pool)
            .await
            .map_err(|e| format!("ensure switch {name}: {e}"))?;
    }
    Ok(())
}

/// Write one snapshot, replacing the switch's previous VLAN and port rows.
///
/// One transaction: a reader must never see the new snapshot's VLANs with the old
/// one's ports, which is what statements outside a transaction allow.  The
/// `switch` row is written in the same transaction so `fetched_at` commits with
/// the rows it describes — the age on the page is the age of what is on screen.
///
/// `privilege` is deliberately not touched here: it belongs to the console, not to
/// the read, and [`note_privilege`] records it.
pub async fn store_snapshot(pool: &sqlx::PgPool, snap: &Snapshot) -> R<()> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| format!("begin snapshot: {e}"))?;

    let sql = pg::update_sql(|upd| {
        upd.table(pg::tbl("switch"))
            .value(pg::iden("system_name"), pg::ev(&snap.system.name))
            .value(pg::iden("location"), pg::ev(&snap.system.location))
            .value(pg::iden("software"), pg::ev(&snap.system.software))
            .value(pg::iden("uptime"), pg::ev(&snap.system.uptime))
            .value(pg::iden("serial"), pg::ev(&snap.system.serial))
            // `now()`, not a timestamp from this process: the age the page shows is
            // measured against the same clock that stores it, so a sync running on a
            // box whose clock drifts cannot make a snapshot look older than it is.
            .value(pg::iden("fetched_at"), Expr::cust("now()"))
            .value(pg::iden("error"), Expr::cust("NULL"))
            .value(pg::iden("warnings"), pg::ev(&jsonb_array(&snap.warnings)))
            .and_where(cond("name").eq(snap.switch.as_str()));
    });
    let res = sqlx::query(&sql)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("update switch row: {e}"))?;
    if res.rows_affected() == 0 {
        // `migrate` inserts a row for every configured switch, so zero rows means
        // the snapshot names a switch nobody configured — a bug in the sync's
        // table, not a condition to paper over by inserting here (that would
        // create a switch row with no address).
        return Err(format!(
            "snapshot for `{}`: no such switch row (is it in the switch list?)",
            snap.switch
        ));
    }

    delete_all(&mut *tx, "vlan", &snap.switch).await?;
    for v in &snap.vlans {
        let sql = pg::insert_sql(|ins| {
            ins.into_table(pg::tbl("vlan"))
                .columns(vec![
                    pg::iden("switch_name"),
                    pg::iden("id"),
                    pg::iden("name"),
                    pg::iden("status"),
                    pg::iden("jumbo"),
                ])
                .values_panic(vec![
                    pg::ev(&snap.switch),
                    pg::evi(Some(i64::from(v.id))),
                    pg::ev(&v.name),
                    pg::ev(&v.status),
                    pg::ev(&v.jumbo),
                ]);
        });
        sqlx::query(&sql)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("insert vlan {}: {e}", v.id))?;
    }

    delete_all(&mut *tx, "port", &snap.switch).await?;
    for p in snap.sorted_ports() {
        // VLAN ids go in as JSON **numbers**, because that is the shape both
        // readers of this column ask for: [`number_list`] deserialises a
        // `Vec<i64>`, and [`vlan_membership`]'s `@>` test compares against
        // `[225]`. Storing them as strings (which this call site used to do by
        // reusing [`jsonb_array`]) made both read "no tagged VLANs" — every
        // tagged membership vanished from the page while sitting intact in the
        // database, and a membership diff built from it could not see a
        // membership to remove.
        let tagged = jsonb_int_array(
            &p.tagged_vlans
                .iter()
                .map(|v| i64::from(*v))
                .collect::<Vec<_>>(),
        );
        let sql = pg::insert_sql(|ins| {
            ins.into_table(pg::tbl("port"))
                .columns(vec![
                    pg::iden("switch_name"),
                    pg::iden("id"),
                    pg::iden("name"),
                    pg::iden("media"),
                    pg::iden("stp_state"),
                    pg::iden("stp_state_from"),
                    pg::iden("link_up"),
                    pg::iden("trunk_group"),
                    pg::iden("trunk_type"),
                    pg::iden("native_vlan"),
                    pg::iden("tagged"),
                    pg::iden("enabled"),
                    pg::iden("has_counters"),
                    pg::iden("errors_rx"),
                    pg::iden("drops_rx"),
                ])
                .values_panic(vec![
                    pg::ev(&snap.switch),
                    pg::ev(&p.id),
                    pg::ev(&p.name),
                    pg::ev(&p.media),
                    pg::ev(&p.stp_state),
                    pg::ev(&p.stp_state_from),
                    sea_query::Expr::value(p.link_up),
                    pg::ev(&p.trunk_group),
                    pg::ev(&p.trunk_type),
                    pg::evi(p.untagged_vlan.map(i64::from)),
                    // The jsonb column takes a text literal cast; sea-query has no
                    // jsonb value type, and `::jsonb` after a bound string is how
                    // every jsonb write in this repo works.
                    sea_query::Expr::cust(format!("'{}'::jsonb", tagged.replace('\'', "''"))),
                    sea_query::Expr::value(p.enabled),
                    sea_query::Expr::value(p.counters.is_some()),
                    pg::ev(
                        &p.counters
                            .as_ref()
                            .map(|c| c.errors_rx.clone())
                            .unwrap_or_default(),
                    ),
                    pg::ev(
                        &p.counters
                            .as_ref()
                            .map(|c| c.drops_rx.clone())
                            .unwrap_or_default(),
                    ),
                ]);
        });
        sqlx::query(&sql)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("insert port {}: {e}", p.id))?;
    }

    tx.commit()
        .await
        .map_err(|e| format!("commit snapshot of {}: {e}", snap.switch))?;
    Ok(())
}

/// Record the privilege level the sync's session reached.
///
/// Separate from [`store_snapshot`] because it is a property of the console, not
/// of the read: a read that failed halfway still learned what context it was in,
/// and that is worth keeping when the snapshot it produced is not.
pub async fn note_privilege(pool: &sqlx::PgPool, name: &str, privilege: &str) -> R<()> {
    let sql = pg::update_sql(|upd| {
        upd.table(pg::tbl("switch"))
            .value(pg::iden("privilege"), pg::ev(privilege))
            .and_where(cond("name").eq(name));
    });
    sqlx::query(&sql)
        .execute(pool)
        .await
        .map_err(|e| format!("record privilege: {e}"))?;
    Ok(())
}

/// Record a failed read, keeping the last good snapshot.
///
/// `error` is what the page shows as "this switch is down"; `warnings` are the
/// `show` commands that failed on a read that half-worked.  Both are stored rather
/// than only logged, because the operator needs to see *how* out of date what is
/// on screen is — and because after a restart the page should still say which
/// switch was unreachable rather than pretend everything is fine.
///
/// Note it does not touch `fetched_at`: a failed read is not a new snapshot, and
/// resetting the age would turn an outage into a lie about freshness.
pub async fn store_failure(
    pool: &sqlx::PgPool,
    name: &str,
    address: &str,
    error: &str,
    warnings: &[String],
) -> R<()> {
    let sql = pg::insert_sql(|ins| {
        ins.into_table(pg::tbl("switch"))
            .columns(vec![
                pg::iden("name"),
                pg::iden("address"),
                pg::iden("error"),
                pg::iden("warnings"),
            ])
            .values_panic(vec![
                pg::ev(name),
                pg::ev(address),
                pg::ev(error),
                pg::ev(&jsonb_array(warnings)),
            ]);
        // The upsert rather than an update: a switch that has never been read has a
        // row from `migrate`, but a switch whose *first ever* read failed must still
        // record why, and this is the one place that knows both cases.
        ins.on_conflict(
            OnConflict::column(pg::iden("name"))
                .update_columns(vec![pg::iden("error"), pg::iden("warnings")])
                .to_owned(),
        );
    });
    sqlx::query(&sql)
        .execute(pool)
        .await
        .map_err(|e| format!("record read failure: {e}"))?;
    Ok(())
}

/// The index page's rows: every switch, its freshness, and how much is queued.
///
/// Four queries rather than one with correlated subqueries.  Each is a plain
/// grouped count sea-query can build, the merge is a `HashMap` in Rust, and the
/// result is that a switch with 50 ports costs four round trips in a millisecond
/// instead of one query the query builder cannot express.  (Correlated counts
/// would need `Expr::cust` around a subquery, which is the thing this module
/// avoids by keeping the counts uncorrelated.)
pub async fn index(pool: &sqlx::PgPool) -> R<Vec<crate::dto::SwitchSummary>> {
    let sql = pg::select_sql(|sel| {
        for c in switch_cols() {
            sel.column(c);
        }
        for (expr, alias) in switch_freshness() {
            sel.expr_as(expr, Alias::new(alias));
        }
        sel.from(pg::tbl("switch"))
            .order_by(pg::col("name"), Order::Asc);
    });
    let rows = sqlx::query(&sql)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("index: {e}"))?;

    let ports = counts(pool, "port").await?;
    let vlans = counts(pool, "vlan").await?;
    let pending = crate::changes::open_counts(pool).await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let name = text(&r, "name");
        out.push(crate::dto::SwitchSummary {
            address: text(&r, "address"),
            system_name: text(&r, "system_name"),
            location: text(&r, "location"),
            software: text(&r, "software"),
            ports: counts_of(&ports, &name),
            vlans: counts_of(&vlans, &name),
            pending: counts_of(&pending, &name),
            privilege: text(&r, "privilege"),
            // Whether *this process* refuses writes is stamped on by the route,
            // which is the only place that knows the command line.
            read_only: false,
            fetched_at: opt_text(&r, "fetched_at"),
            age_secs: opt_integer(&r, "age_secs"),
            error: opt_text(&r, "error"),
            warnings: string_list(&text(&r, "warnings")),
            name,
        });
    }
    Ok(out)
}

/// `SELECT switch_name, count(*) FROM <table> GROUP BY switch_name`.
async fn counts(pool: &sqlx::PgPool, table: &'static str) -> R<HashMap<String, i64>> {
    let sql = pg::select_sql(|sel| {
        sel.column(pg::col("switch_name"))
            .expr_as(pg::count_star(), Alias::new("n"))
            .from(pg::tbl(table))
            .group_by_col(pg::col("switch_name"));
    });
    let rows = sqlx::query(&sql)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("count {table}: {e}"))?;
    Ok(rows
        .iter()
        .map(|r| (text(r, "switch_name"), integer(r, "n")))
        .collect())
}

fn counts_of(map: &HashMap<String, i64>, name: &str) -> i64 {
    map.get(name).copied().unwrap_or(0)
}

/// One switch's shadow copy, without its queue (see [`crate::changes::open`]).
///
/// `None` when no such switch is configured, which the routes turn into a 404
/// rather than an empty page: a typo'd switch name should say so.
pub async fn page(pool: &sqlx::PgPool, name: &str) -> R<Option<crate::dto::Page>> {
    let sql = pg::select_sql(|sel| {
        for c in switch_cols() {
            sel.column(c);
        }
        for (expr, alias) in switch_freshness() {
            sel.expr_as(expr, Alias::new(alias));
        }
        sel.from(pg::tbl("switch")).and_where(cond("name").eq(name));
    });
    let row = sqlx::query(&sql)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("switch row: {e}"))?;
    let Some(row) = row else { return Ok(None) };

    let mut page = crate::dto::Page {
        address: text(&row, "address"),
        system_name: text(&row, "system_name"),
        location: text(&row, "location"),
        software: text(&row, "software"),
        uptime: text(&row, "uptime"),
        serial: text(&row, "serial"),
        fetched_at: opt_text(&row, "fetched_at"),
        age_secs: opt_integer(&row, "age_secs"),
        error: opt_text(&row, "error"),
        warnings: string_list(&text(&row, "warnings")),
        privilege: text(&row, "privilege"),
        name: text(&row, "name"),
        ..Default::default()
    };

    let sql = pg::select_sql(|sel| {
        sel.columns(vec![
            pg::col("id"),
            pg::col("name"),
            pg::col("status"),
            pg::col("jumbo"),
        ])
        .from(pg::tbl("vlan"))
        .and_where(cond("switch_name").eq(name))
        .order_by(pg::col("id"), Order::Asc);
    });
    let rows = sqlx::query(&sql)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("vlans: {e}"))?;
    page.vlans = rows
        .iter()
        .map(|r| crate::dto::Vlan {
            id: integer(r, "id"),
            name: text(r, "name"),
            status: text(r, "status"),
            jumbo: text(r, "jumbo"),
        })
        .collect();

    page.ports = port_rows(pool, name, None)
        .await?
        .iter()
        .map(dto_port)
        .collect();
    sort_ports(&mut page.ports);
    Ok(Some(page))
}

/// The shadow copy's `port` rows for one switch.
///
/// `only` restricts it to one port id; the same column list serves both readers,
/// which is what keeps `port()` and `page()` from describing the same port
/// differently.
async fn port_rows(
    pool: &sqlx::PgPool,
    name: &str,
    only: Option<&str>,
) -> R<Vec<sqlx::postgres::PgRow>> {
    let sql = pg::select_sql(|sel| {
        for c in port_cols() {
            sel.column(c);
        }
        sel.expr_as(pg::as_text("tagged"), Alias::new("tagged"));
        sel.from(pg::tbl("port"))
            .and_where(cond("switch_name").eq(name));
        if let Some(id) = only {
            sel.and_where(cond("id").eq(id));
        }
    });
    let rows = sqlx::query(&sql)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("ports: {e}"))?;
    Ok(rows)
}

/// The switch's own port order: physical ports numerically, then the trunk
/// groups.  The CLI's tables are already in that order, and a page that
/// reshuffles them by string comparison (`1`, `11`, `2`) is unreadable next to a
/// patch panel.
fn sort_ports(ports: &mut [crate::dto::Port]) {
    ports.sort_by_key(|p| crate::model::port_key(&p.id));
}

/// One port as the shadow copy has it — the state a queued membership change is
/// diffed against.
///
/// This is the call that replaced reading the switch to answer a form submit.
/// The rule it keeps from the old code is the one that matters: with no shadow
/// row there is nothing honest to compare against, and the answer must then be an
/// error telling the operator to let the sync read the switch, not a guess.
pub async fn port(pool: &sqlx::PgPool, name: &str, id: &str) -> R<Option<Port>> {
    let rows = port_rows(pool, name, Some(id)).await?;
    Ok(rows.first().map(model_port))
}

/// Every port of one switch as the shadow copy has it.
///
/// Only the revert of a VLAN membership change for a list of ports needs the
/// whole set at once; a single-port revert uses [`port`].
pub async fn ports(pool: &sqlx::PgPool, name: &str) -> R<BTreeMap<String, Port>> {
    let rows = port_rows(pool, name, None).await?;
    Ok(rows
        .iter()
        .map(|r| {
            let p = model_port(r);
            (p.id.clone(), p)
        })
        .collect())
}

/// A `port` row as the app's model.  Shared by both readers so the shadow copy
/// cannot mean two different things depending on which one ran.
fn model_port(r: &sqlx::postgres::PgRow) -> Port {
    let mut port = Port::new(&text(r, "id"));
    port.name = text(r, "name");
    port.media = text(r, "media");
    port.stp_state = text(r, "stp_state");
    port.stp_state_from = text(r, "stp_state_from");
    port.link_up = r.try_get::<Option<bool>, _>("link_up").unwrap_or(None);
    port.trunk_group = text(r, "trunk_group");
    port.trunk_type = text(r, "trunk_type");
    port.untagged_vlan = opt_integer(r, "native_vlan").and_then(|v| u32::try_from(v).ok());
    port.tagged_vlans = number_list(&text(r, "tagged"))
        .into_iter()
        .filter_map(|v| u32::try_from(v).ok())
        .collect();
    // A missing column reads as enabled, which is the switch's default: the
    // wrong-way round of this bug would show every port as shut.
    port.enabled = r.try_get::<bool, _>("enabled").unwrap_or(true);
    port
}

/// A `port` row as the wire type.
fn dto_port(r: &sqlx::postgres::PgRow) -> crate::dto::Port {
    let m = model_port(r);
    crate::dto::Port {
        id: m.id,
        name: m.name,
        media: m.media,
        stp_state: m.stp_state,
        stp_state_from: m.stp_state_from,
        link_up: m.link_up,
        trunk_group: m.trunk_group,
        trunk_type: m.trunk_type,
        native: m.untagged_vlan.map(i64::from),
        tagged: m.tagged_vlans.iter().map(|v| i64::from(*v)).collect(),
        enabled: m.enabled,
        counters: if r.try_get::<bool, _>("has_counters").unwrap_or(false) {
            Some(crate::dto::Counters {
                errors_rx: text(r, "errors_rx"),
                drops_rx: text(r, "drops_rx"),
            })
        } else {
            None
        },
    }
}

/// One VLAN row, so a message can name a VLAN rather than only its id.
pub async fn vlan(pool: &sqlx::PgPool, name: &str, id: u32) -> R<Option<Vlan>> {
    let sql = pg::select_sql(|sel| {
        sel.columns(vec![
            pg::col("id"),
            pg::col("name"),
            pg::col("status"),
            pg::col("jumbo"),
        ])
        .from(pg::tbl("vlan"))
        .and_where(cond("switch_name").eq(name))
        .and_where(cond("id").eq(i64::from(id)));
    });
    let row = sqlx::query(&sql)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("vlan row: {e}"))?;
    Ok(row.map(|r| Vlan {
        id: u32::try_from(integer(&r, "id")).unwrap_or(0),
        name: text(&r, "name"),
        status: text(&r, "status"),
        jumbo: text(&r, "jumbo"),
    }))
}

/// The members of one VLAN, split the way the switch itself splits them.
///
/// A flat list of ids would answer "is this port in this VLAN?", which is not the
/// question a log line answers: a change that moves an untagged VLAN and one that
/// adds a tagged membership are different acts, and an audit line that says
/// "members" for both is the same half-sentence the CLI's own output would be.
///
/// The delete guard ("a VLAN with members is not deletable") does **not** read
/// this: [`crate::commands::vlan_delete`] builds that message from the snapshot the
/// plan was diffed against, and one fact wants one source.
pub async fn vlan_membership(
    pool: &sqlx::PgPool,
    name: &str,
    id: u32,
) -> R<(Vec<String>, Vec<String>)> {
    let sql = pg::select_sql(|sel| {
        sel.column(pg::col("id"))
            .column(pg::col("native_vlan"))
            .expr_as(pg::as_text("tagged"), Alias::new("tagged"))
            .from(pg::tbl("port"))
            .and_where(cond("switch_name").eq(name))
            .and_where(
                cond("native_vlan")
                    .eq(i64::from(id))
                    .or(cond("tagged").contains(pg::jsonb_array_of(i64::from(id)))),
            )
            .order_by(pg::col("id"), Order::Asc);
    });
    let rows = sqlx::query(&sql)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("vlan membership: {e}"))?;
    let mut untagged = Vec::new();
    let mut tagged = Vec::new();
    for r in &rows {
        let port = text(r, "id");
        if opt_integer(r, "native_vlan").map(|v| v as u32) == Some(id) {
            untagged.push(port);
        } else {
            tagged.push(port);
        }
    }
    Ok((untagged, tagged))
}

/// How old one switch's shadow copy is, in seconds, for the sync's own decision.
///
/// `None` means never read (or no such switch), which the sync treats as due.
/// Postgres does the subtraction so the comparison is against the same clock that
/// wrote the timestamp.
pub async fn age_secs(pool: &sqlx::PgPool, name: &str) -> R<Option<i64>> {
    let sql = pg::select_sql(|sel| {
        sel.expr_as(pg::age_secs("fetched_at"), Alias::new("age_secs"))
            .from(pg::tbl("switch"))
            .and_where(cond("name").eq(name));
    });
    let row = sqlx::query(&sql)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("age of {name}: {e}"))?;
    Ok(row.and_then(|r| opt_integer(&r, "age_secs")))
}

/// Remove every row of one child table for one switch.
///
/// Generic over the executor so it runs inside [`store_snapshot`]'s transaction:
/// sqlx implements `Executor` for `&'c mut Transaction<'c, _>` with both
/// lifetimes the same, which a helper taking `&mut Transaction<'a, _>` cannot
/// express — hence the bound here rather than a concrete type.
async fn delete_all<'e, E>(exec: E, table: &'static str, switch_name: &str) -> R<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let sql = pg::delete_sql(|del| {
        del.from_table(pg::tbl(table))
            .and_where(cond("switch_name").eq(switch_name));
    });
    exec.execute(sqlx::query(&sql))
        .await
        .map_err(|e| format!("clear {table}: {e}"))?;
    Ok(())
}

/// The whole shadow copy as a [`Snapshot`] — the shape the CLI readers and the
/// command builders were written against.
///
/// This exists so `commands::vlan_delete` (which refuses a VLAN that still has
/// members) and anything else that wants "the switch as the tool models it" keep
/// working unchanged now that the model lives in Postgres instead of in a cache.
/// It is a read of three tables, not a read of the switch: nothing here touches
/// the console.
pub async fn snapshot(pool: &sqlx::PgPool, name: &str) -> R<Option<Snapshot>> {
    let Some(page) = page(pool, name).await? else {
        return Ok(None);
    };
    let mut snap = Snapshot {
        switch: page.name.clone(),
        warnings: page.warnings.clone(),
        ..Default::default()
    };
    snap.system.name = page.system_name.clone();
    snap.system.location = page.location.clone();
    snap.system.software = page.software.clone();
    snap.system.uptime = page.uptime.clone();
    snap.system.serial = page.serial.clone();
    snap.vlans = page
        .vlans
        .iter()
        .map(|v| Vlan {
            id: u32::try_from(v.id).unwrap_or(0),
            name: v.name.clone(),
            status: v.status.clone(),
            jumbo: v.jumbo.clone(),
        })
        .collect();
    snap.ports = ports(pool, name).await?;
    // The snapshot carries no timestamp of its own: the page's age comes from
    // `age_secs`, which Postgres computes from the row's own `fetched_at`, so a
    // reconstructed snapshot is as fresh as the rows it was read from.
    Ok(Some(snap))
}

// -------------------------------------------------------------- row helpers

/// A `text` column, `""` when NULL.
fn text(row: &sqlx::postgres::PgRow, column: &str) -> String {
    row.try_get::<String, _>(column).unwrap_or_default()
}

fn opt_text(row: &sqlx::postgres::PgRow, column: &str) -> Option<String> {
    row.try_get::<Option<String>, _>(column).ok().flatten()
}

fn integer(row: &sqlx::postgres::PgRow, column: &str) -> i64 {
    row.try_get::<i64, _>(column).unwrap_or(0)
}

fn opt_integer(row: &sqlx::postgres::PgRow, column: &str) -> Option<i64> {
    row.try_get::<Option<i64>, _>(column).ok().flatten()
}

/// A column in a condition: `cond("name").eq(x)` rather than
/// `Expr::col(pg::col("name")).eq(x)`.  The two helpers exist because sea-query
/// wants a `ColumnRef` in a select list and an `Expr` in a `WHERE` clause, and
/// writing the wrapper out at every call site made the queries unreadable.
fn cond(column: &'static str) -> Expr {
    Expr::col(pg::col(column))
}

/// A jsonb array of strings, read out of a `::text` cast.
pub fn string_list(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

/// A jsonb array of numbers, read out of a `::text` cast.
///
/// Tolerant of the two shapes this column has had: the numbers
/// [`jsonb_int_array`] writes now, and the quoted ids an older build wrote with
/// [`jsonb_array`]. Reading is cheap to get wrong and silent about it — a
/// `Vec<i64>` deserialize of `["225"]` is an error, and the fallback is an empty
/// list, so a shape mismatch here shows up as "this port is in no tagged
/// VLANs", which is indistinguishable from a true answer on the page. The
/// column is rewritten from the switch on every sync, so the string form is on
/// its way out on its own; this is what keeps it readable until it is.
fn number_list(json: &str) -> Vec<i64> {
    serde_json::from_str::<Vec<serde_json::Value>>(json)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| match v {
            serde_json::Value::Number(n) => n.as_i64(),
            serde_json::Value::String(s) => s.trim().parse().ok(),
            _ => None,
        })
        .collect()
}

/// A JSON array of numbers, for a jsonb column holding ids.
///
/// The number form is not cosmetic: Postgres' `@>` containment is exact about
/// element type, so `tagged @> '[225]'::jsonb` never matches a `tagged` stored
/// as `["225"]` (see [`vlan_membership`]).
pub fn jsonb_int_array(items: &[i64]) -> String {
    let parts: Vec<String> = items.iter().map(|v| v.to_string()).collect();
    format!("[{}]", parts.join(","))
}

/// A JSON array of strings, for a jsonb column.
pub fn jsonb_array(items: &[String]) -> String {
    let escaped: Vec<String> = items
        .iter()
        .map(|s| format!("\"{}\"", json_escape(s)))
        .collect();
    format!("[{}]", escaped.join(","))
}

/// The JSON string escape, for the places this crate builds JSON by hand.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// A jsonb array of string arrays: a change's commands, one entry per port.
pub fn jsonb_command_lists(groups: &[Vec<String>]) -> String {
    let parts: Vec<String> = groups.iter().map(|g| jsonb_array(g)).collect();
    format!("[{}]", parts.join(","))
}

/// A jsonb object of string pairs: a change's `before` map.
///
/// Flat on purpose (see `dto::Change::before`): the audit log prints it and Revert
/// reads it back, and a nested document would need a schema per kind.
pub fn jsonb_object(map: &BTreeMap<String, String>) -> String {
    let parts: Vec<String> = map
        .iter()
        .map(|(k, v)| format!("\"{}\":\"{}\"", json_escape(k), json_escape(v)))
        .collect();
    format!("{{{}}}", parts.join(","))
}

/// The inverse of [`jsonb_object`].
pub fn string_map(json: &str) -> BTreeMap<String, String> {
    serde_json::from_str(json).unwrap_or_default()
}

/// The inverse of [`jsonb_command_lists`].
pub fn command_lists(json: &str) -> Vec<Vec<String>> {
    serde_json::from_str(json).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The schema must stay free of a `;` inside a literal: [`common_pg::apply_schema`]
    /// splits on it, and a `DO $$ ...; ... $$` block would be cut in half.
    #[test]
    fn the_schema_splits_into_statements() {
        let stmts = pg::split_statements(SCHEMA);
        assert!(stmts.len() >= 3, "{} statements", stmts.len());
        for s in &stmts {
            assert!(
                !s.contains("$$") && !s.to_lowercase().contains("create function"),
                "a multi-statement body would be split by `apply_schema`: {s}"
            );
        }
        for table in ["switch", "vlan", "port"] {
            assert!(
                stmts
                    .iter()
                    .any(|s| s.contains(&format!("CREATE TABLE IF NOT EXISTS {table}"))),
                "no {table} table"
            );
        }
    }

    /// Both port readers must ask for the same columns, or `port()` and `page()`
    /// describe the same port differently.
    #[test]
    fn one_column_list_for_both_port_readers() {
        let cols = port_cols();
        let sql = pg::select_sql(|sel| {
            sel.exprs(cols)
                .expr_as(pg::as_text("tagged"), Alias::new("tagged"))
                .from(pg::tbl("port"));
        });
        for column in [
            "\"id\"",
            "\"name\"",
            "\"media\"",
            "\"stp_state\"",
            "\"stp_state_from\"",
            "\"link_up\"",
            "\"trunk_group\"",
            "\"trunk_type\"",
            "\"native_vlan\"",
            "\"enabled\"",
            "\"has_counters\"",
            "tagged::text",
        ] {
            assert!(sql.contains(column), "{column} missing from {sql}");
        }
    }

    /// The freshness expressions must be in every reader's column list: an
    /// absent `age_secs` reads as `None`, which the page renders as "never read"
    /// for a switch that was read a second ago.
    #[test]
    fn every_switch_reader_selects_its_freshness() {
        let sql = pg::select_sql(|sel| {
            for c in switch_cols() {
                sel.column(c);
            }
            for (expr, alias) in switch_freshness() {
                sel.expr_as(expr, Alias::new(alias));
            }
            sel.from(pg::tbl("switch"))
                .order_by(pg::col("name"), Order::Asc);
        });
        for column in ["fetched_at", "age_secs", "warnings", "privilege", "error"] {
            assert!(sql.contains(column), "{column} missing from {sql}");
        }
        assert!(sql.contains("AT TIME ZONE 'UTC'"), "{sql}");
    }

    #[test]
    fn json_arrays_escape() {
        assert_eq!(jsonb_array(&[]), "[]");
        assert_eq!(jsonb_array(&["1".into(), "2".into()]), r#"["1","2"]"#);
        assert_eq!(
            jsonb_array(&["a\"b\\c".into()]),
            r#"["a\"b\\c"]"#,
            "a port name with a quote must not break the column"
        );
        assert_eq!(
            jsonb_command_lists(&[vec!["interface 7".into()], vec!["interface 8".into()]]),
            r#"[["interface 7"],["interface 8"]]"#
        );
    }

    #[test]
    fn json_arrays_read_back() {
        assert_eq!(
            string_list(r#"["a","b"]"#),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(number_list("[7,20]"), vec![7, 20]);
        // The shape this column used to be written in (see `number_list`).
        assert_eq!(number_list(r#"["7","20"]"#), vec![7, 20]);
        assert_eq!(number_list("[ 7 , 20 ]"), vec![7, 20]);
        assert_eq!(
            command_lists(r#"[["interface 7","shutdown"]]"#),
            vec![vec!["interface 7".to_string(), "shutdown".to_string()]]
        );
        // Garbage and an absent value both read as nothing rather than failing
        // the whole page.
        assert!(string_list("").is_empty());
        assert!(number_list("null").is_empty());
        assert!(string_list("not json").is_empty());
        assert!(command_lists("null").is_empty());
    }

    /// The round trip the page depends on: the tagged list the sync writes is the
    /// tagged list the page reads.  This was broken for a build — the write reused
    /// [`jsonb_array`], so the column held `["225"]` while [`number_list`] parsed a
    /// `Vec<i64>` and fell back to empty, and every port on every switch looked
    /// like an access port with no tagged memberships.  Nothing failed loudly: the
    /// data was in the database, correct, and unreadable.
    #[test]
    fn tagged_vlans_survive_the_jsonb_round_trip() {
        let ids: Vec<i64> = [1u32, 225, 1337].iter().map(|v| i64::from(*v)).collect();
        let json = jsonb_int_array(&ids);
        assert_eq!(json, "[1,225,1337]");
        // A JSON array of numbers, which is what `@> '[225]'::jsonb` matches.
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert!(parsed
            .as_array()
            .expect("an array")
            .iter()
            .all(serde_json::Value::is_number));
        assert_eq!(number_list(&json), vec![1, 225, 1337]);
        // And an empty set stays an empty set, not `null`.
        assert_eq!(jsonb_int_array(&[]), "[]");
        assert!(number_list(&jsonb_int_array(&[])).is_empty());
    }

    /// The port order the page shows is the switch's, not a string sort: `Trk1`
    /// belongs after `48`, and `11` after `9`.
    #[test]
    fn ports_sort_the_way_an_operator_reads_a_patch_panel() {
        let mut ports = vec!["48", "11", "Trk1", "9", "1", "Trk10"]
            .into_iter()
            .map(|id| crate::dto::Port {
                id: id.to_string(),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        sort_ports(&mut ports);
        let ids: Vec<&str> = ports.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["1", "9", "11", "48", "Trk1", "Trk10"]);
    }

    /// A port name with a quote must survive the jsonb literal cast: the value is
    /// inlined by `inject_parameters`, and a `'` in a name would otherwise end the
    /// literal early.  (The CLI refuses quotes in names, so this is defence in
    /// depth against a hand-edited row, not a live path.)
    #[test]
    fn a_jsonb_literal_survives_a_quote() {
        let json = jsonb_array(&["NAS2's port".into()]);
        let expr = sea_query::Expr::cust(format!("'{}'::jsonb", json.replace('\'', "''")));
        let sql = pg::select_sql(|sel| {
            sel.expr_as(expr, Alias::new("tagged"))
                .from(pg::tbl("port"));
        });
        // The `'` became `''`, so the literal still holds the whole array and the
        // cast still applies to the value rather than to a fragment of it.
        assert!(sql.contains(r#"["NAS2''s port"]'::jsonb"#), "{sql}");
        assert_eq!(sql.matches("::jsonb").count(), 1, "{sql}");
    }
}
