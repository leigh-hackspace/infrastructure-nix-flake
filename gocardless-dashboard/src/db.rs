//! Postgres persistence: one table per GoCardless entity (each row stores
//! the full API JSON document plus the few scalar columns worth indexing)
//! and a `meta` key/value table.  Queries are built with sea-query and
//! executed through sqlx-postgres.  Every sync writes a full snapshot
//! (truncate + reinsert in one transaction) — the GoCardless Pro dataset
//! for a small organisation is tiny.
//!
//! sea-query 0.32 drops the sqlx runtime adapter, so each statement is
//! built to `(sql, params)` and the parameters are inlined (escaped) with
//! `inject_parameters` before handing a single string to sqlx.

use std::collections::HashMap;

use gdash_dto::{Customer, Fetched, Mandate, Payment, Payout, Refund, Subscription};
use sea_query::{
    prepare::inject_parameters, ColumnRef, DynIden, Expr, IntoIden, OnConflict,
    PostgresQueryBuilder, Query, SimpleExpr, TableRef,
};
use serde::de::DeserializeOwned;
use sqlx::postgres::PgPool;

fn tbl(name: &'static str) -> TableRef {
    TableRef::Table(name.into_iden())
}

/// A raw column identifier (for `insert().columns(..)` which takes `IntoIden`).
fn iden(name: &'static str) -> DynIden {
    name.into_iden()
}

/// A column reference (for `select().column(..)` which takes `IntoColumnRef`).
fn col(name: &'static str) -> ColumnRef {
    ColumnRef::Column(name.into_iden())
}

// sea-query `SimpleExpr` constructors (the `values_panic` array must be uniform).
fn ev(s: &str) -> SimpleExpr {
    Expr::value(s.to_string())
}
fn evo(o: Option<&str>) -> SimpleExpr {
    Expr::value(o.map(str::to_string))
}
fn evi(o: Option<i64>) -> SimpleExpr {
    Expr::value(o)
}

/// Build a sea-query statement, inline (escaped) its parameters, and return
/// the final SQL string.
fn to_sql(sql: String, values: sea_query::Values) -> String {
    inject_parameters(&sql, values, &PostgresQueryBuilder)
}

/// Build the final (parameter-inlined) SQL from a sea-query insert
/// statement.  Synchronous on purpose: sea-query's builders are not
/// `Send`, so the builder must be moved into a sync function rather than
/// kept alive across the `await` points in the caller's future.
fn finalize_insert(ins: sea_query::InsertStatement) -> String {
    let (sql, values) = ins.build_any(&PostgresQueryBuilder);
    to_sql(sql, values)
}

/// Run a statement that selects exactly one `json` column, decoding each
/// row into the given type.
async fn fetch_json<T: DeserializeOwned>(pool: &PgPool, sql: String) -> Result<Vec<T>, String> {
    let rows: Vec<(String,)> = sqlx::query_as(&sql)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;
    rows.into_iter()
        .map(|(j,)| serde_json::from_str(&j).map_err(|e| format!("bad stored json: {e}")))
        .collect()
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS customers (
    id     TEXT PRIMARY KEY,
    email  TEXT,
    name   TEXT,
    json   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS mandates (
    id           TEXT PRIMARY KEY,
    customer_id  TEXT,
    status       TEXT,
    json         TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS subscriptions (
    id            TEXT PRIMARY KEY,
    customer_id   TEXT,
    mandate_id    TEXT,
    status        TEXT,
    json          TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS payments (
    id              TEXT PRIMARY KEY,
    customer_id     TEXT,
    subscription_id TEXT,
    amount          BIGINT,
    currency        TEXT,
    status          TEXT,
    json            TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS refunds (
    id         TEXT PRIMARY KEY,
    payment_id TEXT,
    amount     BIGINT,
    currency   TEXT,
    status     TEXT,
    json       TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS payouts (
    id       TEXT PRIMARY KEY,
    amount   BIGINT,
    currency TEXT,
    status   TEXT,
    json     TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS authentik_sync_log (
    id          BIGSERIAL PRIMARY KEY,
    ts          TIMESTAMPTZ NOT NULL DEFAULT now(),
    action      TEXT NOT NULL,
    username    TEXT,
    email       TEXT,
    customer_id TEXT,
    detail      TEXT
);
CREATE INDEX IF NOT EXISTS authentik_sync_log_ts_idx   ON authentik_sync_log(ts);
CREATE INDEX IF NOT EXISTS authentik_sync_log_act_idx  ON authentik_sync_log(action);
CREATE INDEX IF NOT EXISTS mandates_customer_idx        ON mandates(customer_id);
CREATE INDEX IF NOT EXISTS subscriptions_customer_idx   ON subscriptions(customer_id);
CREATE INDEX IF NOT EXISTS subscriptions_mandate_idx    ON subscriptions(mandate_id);
CREATE INDEX IF NOT EXISTS payments_customer_idx        ON payments(customer_id);
"#;

pub async fn init_schema(pool: &PgPool) -> Result<(), String> {
    // Postgres won't run multiple statements in one prepared query, so
    // execute each one separately (the DDL above has no embedded ';').
    for stmt in SCHEMA.split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        sqlx::query(stmt)
            .execute(pool)
            .await
            .map(|_| ())
            .map_err(|e| format!("{e}: {stmt}"))?;
    }
    Ok(())
}

pub struct DbData {
    pub customers: Vec<Customer>,
    pub mandates: Vec<Mandate>,
    pub subscriptions: Vec<Subscription>,
    pub payments: Vec<Payment>,
    pub refunds: Vec<Refund>,
    pub payouts: Vec<Payout>,
    pub last_sync: Option<String>,
}

/// Load every entity table back into dto structs (used to rebuild the
/// snapshot after a restart, before the next sync runs).
pub async fn load_all(pool: &PgPool) -> Result<DbData, String> {
    let select = |table: &'static str| {
        let (sql, values) = Query::select()
            .column(col("json"))
            .from(tbl(table))
            .build_any(&PostgresQueryBuilder);
        to_sql(sql, values)
    };

    let customers: Vec<Customer> = fetch_json(pool, select("customers")).await?;
    let mandates: Vec<Mandate> = fetch_json(pool, select("mandates")).await?;
    let subscriptions: Vec<Subscription> = fetch_json(pool, select("subscriptions")).await?;
    let payments: Vec<Payment> = fetch_json(pool, select("payments")).await?;
    let refunds: Vec<Refund> = fetch_json(pool, select("refunds")).await?;
    let payouts: Vec<Payout> = fetch_json(pool, select("payouts")).await?;

    let (sql, values) = Query::select()
        .column(col("value"))
        .from(tbl("meta"))
        .and_where(sea_query::Expr::col(col("key")).eq("last_sync"))
        .build_any(&PostgresQueryBuilder);
    let sql = to_sql(sql, values);
    let rows: Vec<(Option<String>,)> = sqlx::query_as(&sql)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;
    let last_sync = rows.into_iter().next().and_then(|r| r.0);

    Ok(DbData {
        customers,
        mandates,
        subscriptions,
        payments,
        refunds,
        payouts,
        last_sync,
    })
}

/// Build an `INSERT` for one entity table.  Synchronous on purpose:
/// sea-query's builders are not `Send` (they hold `Rc<dyn Iden>`), and
/// rustc's liveness check treats a builder touched inside a `for` loop
/// as live across later `await` points — so all builder work happens in
/// sync functions and only the finished SQL string crosses an await.
fn build_insert(
    table: &'static str,
    columns: &'static [&'static str],
    rows: Vec<Vec<SimpleExpr>>,
) -> String {
    // An empty table would produce `INSERT ... VALUES` with no rows, which
    // Postgres rejects; the caller already truncated the table, so a no-op
    // is exactly right.
    if rows.is_empty() {
        return "select 1".to_string();
    }
    let mut ins = Query::insert();
    ins.into_table(tbl(table));
    ins.columns(columns.iter().copied().map(iden));
    for row in rows {
        ins.values_panic(row);
    }
    finalize_insert(ins)
}

/// Truncate and reinsert everything in one transaction, and stamp
/// meta.last_sync with the given RFC3339 timestamp.
pub async fn write_all(pool: &PgPool, data: &Fetched, now_rfc3339: &str) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

    for table in [
        "customers",
        "mandates",
        "subscriptions",
        "payments",
        "refunds",
        "payouts",
    ] {
        let sql = sea_query::Table::truncate()
            .table(tbl(table))
            .build_any(&PostgresQueryBuilder);
        sqlx::query(&sql)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
    }

    // Customer resolution for payments/subscriptions (see resolve_customer):
    // the API leaves links.customer null on those, so resolve via mandate.
    let mandate_customer = mandate_customer_index(&data.mandates);

    // customers
    let rows = data.customers.iter().map(|c| {
        let name = c.display_name();
        let name_opt = if name.is_empty() { None } else { Some(name) };
        let json = serde_json::to_string(c).unwrap();
        vec![ev(&c.id), evo(c.email.as_deref()), evo(name_opt.as_deref()), ev(&json)]
    }).collect();
    let sql = build_insert("customers", &COLS_CUSTOMERS, rows);
    sqlx::query(&sql)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    // mandates
    let rows = data.mandates.iter().map(|m| {
        let json = serde_json::to_string(m).unwrap();
        vec![ev(&m.id), evo(m.links.customer.as_deref()), evo(m.status.as_deref()), ev(&json)]
    }).collect();
    let sql = build_insert("mandates", &COLS_MANDATES, rows);
    sqlx::query(&sql)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    // subscriptions
    let rows = data.subscriptions.iter().map(|s| {
        let json = serde_json::to_string(s).unwrap();
        let customer = resolve_customer(&s.links, &mandate_customer);
        vec![
            ev(&s.id),
            evo(customer.as_deref()),
            evo(s.links.mandate.as_deref()),
            evo(s.status.as_deref()),
            ev(&json),
        ]
    }).collect();
    let sql = build_insert("subscriptions", &COLS_SUBSCRIPTIONS, rows);
    sqlx::query(&sql)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    // payments
    let rows = data.payments.iter().map(|p| {
        let json = serde_json::to_string(p).unwrap();
        let customer = resolve_customer(&p.links, &mandate_customer);
        vec![
            ev(&p.id),
            evo(customer.as_deref()),
            evo(p.links.subscription.as_deref()),
            evi(p.amount_cents()),
            evo(p.currency.as_deref()),
            evo(p.status.as_deref()),
            ev(&json),
        ]
    }).collect();
    let sql = build_insert("payments", &COLS_PAYMENTS, rows);
    sqlx::query(&sql)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    // refunds
    let rows = data.refunds.iter().map(|r| {
        let json = serde_json::to_string(r).unwrap();
        vec![
            ev(&r.id),
            evo(r.links.payment.as_deref()),
            evi(r.amount_cents()),
            evo(r.currency.as_deref()),
            evo(r.status.as_deref()),
            ev(&json),
        ]
    }).collect();
    let sql = build_insert("refunds", &COLS_REFUNDS, rows);
    sqlx::query(&sql)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    // payouts
    let rows = data.payouts.iter().map(|p| {
        let json = serde_json::to_string(p).unwrap();
        vec![ev(&p.id), evi(p.amount_cents()), evo(p.currency.as_deref()), evo(p.status.as_deref()), ev(&json)]
    }).collect();
    let sql = build_insert("payouts", &COLS_PAYOUTS, rows);
    sqlx::query(&sql)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    // meta.last_sync upsert
    let sql = {
        let mut up = Query::insert();
        up.into_table(tbl("meta"));
        up.columns([iden("key"), iden("value")]);
        up.values_panic([ev("last_sync"), evo(Some(now_rfc3339))]);
        up.on_conflict(
            OnConflict::column("key")
                .update_column("value")
                .to_owned(),
        );
        finalize_insert(up)
    };
    sqlx::query(&sql)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())
}

const COLS_CUSTOMERS: &[&str] = &["id", "email", "name", "json"];
const COLS_MANDATES: &[&str] = &["id", "customer_id", "status", "json"];
const COLS_SUBSCRIPTIONS: &[&str] =
    &["id", "customer_id", "mandate_id", "status", "json"];
const COLS_PAYMENTS: &[&str] =
    &["id", "customer_id", "subscription_id", "amount", "currency", "status", "json"];
const COLS_REFUNDS: &[&str] =
    &["id", "payment_id", "amount", "currency", "status", "json"];
const COLS_PAYOUTS: &[&str] = &["id", "amount", "currency", "status", "json"];

/// One row of the authentik sync audit log (also the wire type for the GUI;
/// see gdash_dto::AkLogRow — kept here as a plain tuple-friendly struct for
/// the insert).
pub struct AkLogInsert<'a> {
    pub action: &'a str,
    pub username: Option<&'a str>,
    pub email: Option<&'a str>,
    pub customer_id: Option<&'a str>,
    pub detail: Option<&'a str>,
}

/// Append rows to the authentik sync audit log in one transaction.
pub async fn write_ak_log(pool: &PgPool, rows: &[AkLogInsert<'_>]) -> Result<(), String> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    for r in rows {
        let sql = {
            let mut ins = Query::insert();
            ins.into_table(tbl("authentik_sync_log"));
            ins.columns([
                iden("action"),
                iden("username"),
                iden("email"),
                iden("customer_id"),
                iden("detail"),
            ]);
            ins.values_panic([
                ev(r.action),
                evo(r.username),
                evo(r.email),
                evo(r.customer_id),
                evo(r.detail),
            ]);
            finalize_insert(ins)
        };
        sqlx::query(&sql)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
    }
    tx.commit().await.map_err(|e| e.to_string())
}

/// The most recent authentik sync log rows (newest first), decoded into the
/// shared wire type.  `limit` is a small constant-sized number (inlined by
/// hand, so it never goes through parameter binding).
pub async fn read_ak_log(pool: &PgPool, limit: u32) -> Result<Vec<gdash_dto::AkLogRow>, String> {
    // limit is a u32 we control (inlined by hand, never bound as a parameter).
    let sql = format!(
        "SELECT id, to_char(ts AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"'), action, \n         username, email, customer_id, detail \n         FROM authentik_sync_log \n         ORDER BY id DESC LIMIT {limit}"
    );
    let rows: Vec<(i64, String, String, Option<String>, Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as(&sql)
            .fetch_all(pool)
            .await
            .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|(id, ts, action, username, email, customer_id, detail)| gdash_dto::AkLogRow {
            id,
            ts,
            action,
            username,
            email,
            customer_id,
            detail,
        })
        .collect())
}

/// Index a slice by the GoCardless customer id (via links.customer).
pub fn index_by_customer<T>(
    items: &[T],
    get: impl Fn(&T) -> Option<&str>,
) -> HashMap<String, Vec<&T>> {
    let mut map: HashMap<String, Vec<&T>> = HashMap::new();
    for item in items {
        if let Some(c) = get(item) {
            map.entry(c.to_string()).or_default().push(item);
        }
    }
    map
}

// ---------------------------------------------------------------------------
// Customer resolution.
//
// The GoCardless Pro API leaves `links.customer` null on *payments* and
// *subscriptions* — it only populates `links.customer` on mandates.  Payments
// and subscriptions do carry `links.mandate`, and every mandate carries
// `links.customer`, so the customer for any entity is resolved through the
// linked mandate.  (Verified against live data: 100% of payments and
// subscriptions resolve this way; 0 carry a direct customer link.)
// ---------------------------------------------------------------------------

/// GoCardless mandate id -> customer id (from `mandate.links.customer`).
pub fn mandate_customer_index(mandates: &[Mandate]) -> HashMap<String, String> {
    mandates
        .iter()
        .filter_map(|m| m.links.customer.clone().map(|c| (m.id.clone(), c)))
        .collect()
}

/// Resolve the customer id for an entity from its `Links`.
///
/// Prefers the direct `links.customer` (set on mandates); otherwise follows
/// `links.mandate` to the mandate's customer (the path used by payments and
/// subscriptions).
pub fn resolve_customer(links: &gdash_dto::Links, mandate_customer: &HashMap<String, String>) -> Option<String> {
    if let Some(c) = &links.customer {
        return Some(c.clone());
    }
    links
        .mandate
        .as_deref()
        .and_then(|m| mandate_customer.get(m))
        .cloned()
}
