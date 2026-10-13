//! Postgres plumbing shared by the services that keep state in the network's
//! one Postgres (`gocardless-dashboard`, `switch-gui`).
//!
//! Two things live here: the pool/schema helpers, and the sea-query→sqlx glue.
//! The reasons are in `Cargo.toml`; the short version is that the glue has a
//! sharp edge (sea-query 0.32 has no sqlx adapter, so parameters must be
//! *inlined* into the SQL string before sqlx will run it) and both apps were
//! filing the same edge by hand.
//!
//! ```no_run
//! use common_pg as pg;
//!
//! # async fn demo() -> Result<(), String> {
//! let pool = pg::connect("postgresql://switch_gui@127.0.0.1/switch_gui", 4).await?;
//! pg::apply_schema(&pool, "CREATE TABLE IF NOT EXISTS t (a int)").await?;
//! let sql = pg::finalize_select(
//!     sea_query::Query::select().column(pg::col("a")).from(pg::tbl("t")).clone(),
//! );
//! # let _ = sql; Ok(()) }
//! ```

use sea_query::{
    prepare::inject_parameters, ColumnRef, DynIden, Expr, IntoIden, PostgresQueryBuilder,
    SimpleExpr, TableRef,
};
use sqlx::postgres::PgPool;

/// The pool handle.  A type alias rather than a wrapper: every sqlx call takes
/// `&PgPool`, and a newtype would only add a deref to every call site.
pub type Pool = PgPool;

/// Open a pool.
///
/// `max_connections` is small on purpose in both apps that use it: this is one
/// Postgres shared with other services on the same box, and neither dashboard
/// needs more than a handful of concurrent statements.
///
/// A failure names the DSN with its password removed ([`redact`]) — a startup
/// failure that prints the URL puts the password in the journal.
pub async fn connect(url: &str, max_connections: u32) -> Result<Pool, String> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(url)
        .await
        .map_err(|e| format!("connect to postgres at {}: {e}", redact(url)))
}

/// Run a schema: each `;`-separated statement on its own.
///
/// Postgres will not prepare multiple statements in one query, hence the split
/// rather than one `raw_sql` call.  The cost is that a schema string must not
/// contain a `;` inside a literal or a function body — true of every schema in
/// this repo, and worth knowing before adding a `DO $$ ... $$` block.
///
/// Idempotency is the caller's job (`CREATE TABLE IF NOT EXISTS`,
/// `ADD COLUMN IF NOT EXISTS`), because this runs on every service start.
pub async fn apply_schema(pool: &Pool, schema: &str) -> Result<(), String> {
    for stmt in split_statements(schema) {
        sqlx::query(stmt)
            .execute(pool)
            .await
            .map(|_| ())
            .map_err(|e| format!("{e}: {stmt}"))?;
    }
    Ok(())
}

/// The non-async half of [`apply_schema`]: the statements a schema string
/// contains, trimmed and with the empties removed.
pub fn split_statements(schema: &str) -> Vec<&str> {
    schema
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

/// A DSN with any password removed, for logging.
///
/// Handles the two forms a URL takes: `scheme://user:pass@host/db` and a
/// `key=value` connection string (`password=... host=...`).  Anything it does
/// not recognise is returned unchanged rather than dropped — a startup failure
/// you cannot diagnose is worse than one that quotes a URL with no secret in it.
pub fn redact(url: &str) -> String {
    // `scheme://user:pass@host` → keep `scheme://user`, drop `:pass`.
    if let Some(scheme_end) = url.find("://") {
        let rest = &url[scheme_end + 3..];
        // The authority ends at the first '/', '?' or ' '; the userinfo at '@'.
        let authority_end = rest.find(['/', '?', ' ']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(authority_end);
        if let Some(at) = authority.rfind('@') {
            let userinfo = &authority[..at];
            let host = &authority[at + 1..];
            // Only a userinfo that actually has a `:` carries a secret; a plain
            // `user@host` DSN is returned untouched rather than decorated with a
            // password that was never there.
            let shown = match userinfo.split_once(':') {
                Some((user, _)) => format!("{user}:***@"),
                None => format!("{userinfo}@"),
            };
            return format!("{}://{}{}{}", &url[..scheme_end], shown, host, tail);
        }
        return url.to_string();
    }

    // `key=value ...` form.
    if url.contains('=') {
        return url
            .split_whitespace()
            .map(|pair| match pair.split_once('=') {
                Some((k, _)) if k.eq_ignore_ascii_case("password") => {
                    format!("{k}=***")
                }
                _ => pair.to_string(),
            })
            .collect::<Vec<_>>()
            .join(" ");
    }

    url.to_string()
}

// ------------------------------------------------------ sea-query plumbing

/// A table reference from a name (`tbl("switch")`, not `Table::new(Alias::new(..))`).
///
/// `&'static str` because sea-query's dynamic identifiers are `Rc<dyn Iden>` and
/// the blanket `IntoIden` impl keeps whatever it was given: build one from a
/// `&str` with a short lifetime and the whole query has to borrow it.  Every
/// column and table name in this repo is a literal, so the bound costs nothing
/// and saves the caller a `to_string()` per column.
pub fn tbl(name: &'static str) -> TableRef {
    TableRef::Table(name.into_iden())
}

/// A raw column identifier, for `insert().columns(..)` which takes `IntoIden`.
pub fn iden(name: &'static str) -> DynIden {
    name.into_iden()
}

/// A column reference, for `select().column(..)` which takes `IntoColumnRef`.
pub fn col(name: &'static str) -> ColumnRef {
    ColumnRef::Column(name.into_iden())
}

// sea-query `SimpleExpr` constructors.  They are this short because every insert
// in both apps is a `values_panic` array that must stay uniform in type.
pub fn ev(s: &str) -> SimpleExpr {
    Expr::value(s.to_string())
}

pub fn evo(o: Option<&str>) -> SimpleExpr {
    Expr::value(o.map(str::to_string))
}

pub fn evi(o: Option<i64>) -> SimpleExpr {
    Expr::value(o)
}

pub fn evb(b: bool) -> SimpleExpr {
    Expr::value(b)
}

/// Build a statement and inline (escape) its parameters, returning the SQL.
///
/// This is the step sea-query 0.32 no longer does for sqlx.  Inlining is safe
/// because the values come from the server, not the wire — and because
/// `inject_parameters` escapes them the way the driver would.
pub fn to_sql(sql: String, values: sea_query::Values) -> String {
    inject_parameters(&sql, values, &PostgresQueryBuilder)
}

/// Final SQL from a sea-query insert.
///
/// Synchronous on purpose: sea-query's builders are not `Send`, so a builder
/// must be built and finished inside a sync function rather than held across an
/// `await` in the caller's future.  Every caller in this repo hits that.
pub fn finalize_insert(ins: sea_query::InsertStatement) -> String {
    let (sql, values) = ins.build_any(&PostgresQueryBuilder);
    to_sql(sql, values)
}

/// Final SQL from a sea-query select.
pub fn finalize_select(sel: sea_query::SelectStatement) -> String {
    let (sql, values) = sel.build_any(&PostgresQueryBuilder);
    to_sql(sql, values)
}

/// Final SQL from a sea-query update.
pub fn finalize_update(upt: sea_query::UpdateStatement) -> String {
    let (sql, values) = upt.build_any(&PostgresQueryBuilder);
    to_sql(sql, values)
}

/// Final SQL from a sea-query delete.
pub fn finalize_delete(del: sea_query::DeleteStatement) -> String {
    let (sql, values) = del.build_any(&PostgresQueryBuilder);
    to_sql(sql, values)
}

// ---------------------------------------------- builders that cannot outlive a call
//
// A sea-query statement holds its identifiers as `Rc<dyn Iden>`, so no statement
// is `Send` — and an `async fn` that keeps one alive across an `await` produces
// the least comprehensible error in this crate (`future cannot be sent between
// threads safely`, pointing at the `spawn`, three files away from the statement).
// Building the SQL inside a call that returns only the string makes that
// unrepresentable: the statement is created, filled and dropped here, and the
// caller is left holding a `String`.

/// Build a select and return its SQL.
///
/// ```
/// # use common_pg as pg;
/// let sql = pg::select_sql(|sel| {
///     sel.column(pg::col("id")).from(pg::tbl("switch"));
/// });
/// assert!(sql.starts_with("SELECT"));
/// ```
pub fn select_sql(build: impl FnOnce(&mut sea_query::SelectStatement)) -> String {
    let mut sel = sea_query::Query::select();
    build(&mut sel);
    finalize_select(sel)
}

/// Build an update and return its SQL.
pub fn update_sql(build: impl FnOnce(&mut sea_query::UpdateStatement)) -> String {
    let mut upd = sea_query::Query::update();
    build(&mut upd);
    finalize_update(upd)
}

/// Build an insert and return its SQL.
pub fn insert_sql(build: impl FnOnce(&mut sea_query::InsertStatement)) -> String {
    let mut ins = sea_query::Query::insert();
    build(&mut ins);
    finalize_insert(ins)
}

/// Build a delete and return its SQL.
pub fn delete_sql(build: impl FnOnce(&mut sea_query::DeleteStatement)) -> String {
    let mut del = sea_query::Query::delete();
    build(&mut del);
    finalize_delete(del)
}

/// Quote a `LIKE`/`ILIKE` pattern operand.
pub fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '%' | '_' => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

/// `%pattern%` with the operand escaped (see [`like_escape`]).
pub fn like_contains(s: &str) -> String {
    format!("%{}%", like_escape(s))
}

// ------------------------------------------- the escape hatch, kept in one place
//
// sea-query has no vocabulary for three things every schema in this repo needs,
// and each has exactly one honest way to write it.  They are here so the
// `Expr::cust` calls — the only ones in the repo — are reviewed once rather than
// re-invented per app, and so a column's *wire format* is defined by the crate
// the DTOs' contract lives next to.

/// A timestamp column as a UTC string: `2026-10-13T10:20:30Z`.
///
/// Why not a `DateTime<Utc>`: sqlx' chrono support is a feature neither app
/// enables, and a timestamp that arrives as an instant gets re-rendered in the
/// browser's zone, so the audit log and the port table would disagree about when
/// a change landed.  Formatting in Postgres with `AT TIME ZONE 'UTC'` and a
/// literal `Z` means the string the client sees is the string the row holds.
pub fn ts_utc(column: &str) -> SimpleExpr {
    Expr::cust(format!(
        "to_char({column} AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')"
    ))
}

/// How many seconds ago a timestamp column was, by the **server's** clock.
///
/// The client must not compute this: a browser whose clock is three minutes off
/// reports a snapshot as from the future, and a page that shows "-180 s ago" is
/// worse than one that shows a wrong number.  `EXTRACT(EPOCH …)` returns numeric,
/// which sqlx would hand back as a string, hence the cast.
pub fn age_secs(column: &str) -> SimpleExpr {
    Expr::cust(format!("EXTRACT(EPOCH FROM (now() - {column}))::bigint"))
}

/// A jsonb column as text.
///
/// sqlx' json support is a feature no service here enables (it adds a `Json`
/// wrapper to every column for one use), so jsonb crosses as text and is parsed
/// with the serde_json the API layer already has.  Casting in the select keeps
/// that invisible to the row reader.
pub fn as_text(column: &str) -> SimpleExpr {
    Expr::cust(format!("{column}::text"))
}

/// `count(*)`, for a select that has nothing else to name.
pub fn count_star() -> SimpleExpr {
    Expr::cust("count(*)")
}

/// A jsonb array literal built from a number, for a `@>` containment test.
///
/// `tagged @> '[7]'::jsonb` is how "is port 7 tagged on VLAN 7" is asked of a
/// jsonb array column; the value is bound, only the cast is text.
pub fn jsonb_array_of(value: i64) -> SimpleExpr {
    cust_with_value("($1)::text::jsonb", format!("[{value}]"))
}

/// A custom expression with one bound value, in the shape sea-query wants.
///
/// `Expr::cust_with_values` takes an iterator of values and its type parameters
/// make callers spell `vec![...]` and an explicit `Into<Value>` bound at every
/// call site; this is the one-value case, which is every case in this repo.  The
/// `$1` in `sql` is where the value goes, and it is escaped rather than
/// concatenated — which is the entire reason not to `format!` a value into a
/// custom expression.
pub fn cust_with_value<T>(sql: &str, value: T) -> SimpleExpr
where
    T: Into<sea_query::Value>,
{
    Expr::cust_with_values(sql, vec![value])
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_query::{Alias, Query};

    #[test]
    fn redact_takes_the_password_out_of_a_url() {
        assert_eq!(
            redact("postgresql://switch_gui:s3cr3t@127.0.0.1:5432/switch_gui"),
            "postgresql://switch_gui:***@127.0.0.1:5432/switch_gui"
        );
        // No password at all: unchanged, so a socket DSN still reads usefully.
        assert_eq!(
            redact("postgresql://switch_gui@127.0.0.1/switch_gui"),
            "postgresql://switch_gui@127.0.0.1/switch_gui"
        );
        // No userinfo either.
        assert_eq!(redact("postgres://h/db"), "postgres://h/db");
    }

    /// The `key=value` form is what libpq tools use, and `password=` is the
    /// secret in it.
    #[test]
    fn redact_takes_the_password_out_of_a_key_value_dsn() {
        assert_eq!(
            redact("host=127.0.0.1 user=switch_gui password=s3cr3t dbname=switch_gui"),
            "host=127.0.0.1 user=switch_gui password=*** dbname=switch_gui"
        );
        assert_eq!(
            redact("PASSWORD=s3cr3t host=/run/postgresql"),
            "PASSWORD=*** host=/run/postgresql"
        );
    }

    /// A DSN that is not either recognised form is returned as-is: a failure you
    /// cannot diagnose is worse than a URL with no secret in it.
    #[test]
    fn redact_leaves_an_unknown_form_alone() {
        assert_eq!(redact("switch_gui"), "switch_gui");
        assert_eq!(redact(""), "");
    }

    #[test]
    fn a_password_with_at_signs_still_redacts() {
        // `rfind('@')`: the *last* @ separates the host, so an @ in the password
        // must not make the host look like part of the secret.
        assert_eq!(
            redact("postgresql://u:p@ss@host/db"),
            "postgresql://u:***@host/db"
        );
    }

    #[test]
    fn a_schema_splits_into_statements() {
        let sql = "
            CREATE TABLE IF NOT EXISTS a (x int);
            CREATE INDEX IF NOT EXISTS b ON a (x);

            -- a comment line
            CREATE TABLE IF NOT EXISTS c (y int);
        ";
        let stmts = split_statements(sql);
        assert_eq!(stmts.len(), 3, "{stmts:?}");
        assert!(stmts[0].starts_with("CREATE TABLE IF NOT EXISTS a"));
        assert!(stmts[2].contains("CREATE TABLE IF NOT EXISTS c"));
        assert!(split_statements("").is_empty());
        assert!(split_statements(";;").is_empty());
    }

    #[test]
    fn like_patterns_escape_the_wildcards() {
        assert_eq!(like_escape("plain"), "plain");
        assert_eq!(like_escape("100%"), r"100\%");
        assert_eq!(like_escape("a_b"), r"a\_b");
        assert_eq!(like_escape(r"back\slash"), r"back\\slash");
        assert_eq!(like_contains("50%"), r"%50\%%");
        // The order matters: a backslash added while escaping `%` must not then
        // be escaped again, so `\%` is four characters out, not five.
        assert_eq!(like_escape(r"\%"), r"\\\%");
    }

    /// The inlining step, proven on a statement with a value that would break
    /// the SQL if it were concatenated instead.
    #[test]
    fn parameters_are_inlined_not_concatenated() {
        let ins = sea_query::Query::insert()
            .into_table(tbl("change"))
            .columns(vec![iden("switch_name"), iden("note")])
            .values_panic(vec![
                ev("switch1").into(),
                ev("'); DROP TABLE change;--").into(),
            ])
            .clone();
        let sql = finalize_insert(ins);
        assert!(sql.starts_with("INSERT INTO \"change\""), "{sql}");
        // The quote is escaped (`E'\''` style), so the injected statement stays
        // inside the string literal and the result is still one insert.
        assert!(sql.contains(r"E'\'); DROP TABLE change;--'"), "{sql}");
        assert!(sql.contains("'switch1'"), "{sql}");

        let sel = sea_query::Query::select()
            .column(col("id"))
            .from(tbl("change"))
            .and_where(Expr::col(col("switch_name")).eq("switch1"))
            .clone();
        let sql = finalize_select(sel);
        assert!(sql.contains("WHERE \"switch_name\" = 'switch1'"), "{sql}");
    }

    /// The three escape hatches must render exactly the SQL the row readers
    /// expect, including the literal `Z` (a real timezone offset would let a
    /// browser shift the value again).
    #[test]
    fn the_wire_formats_render_as_documented() {
        let sel = Query::select()
            .expr_as(ts_utc("fetched_at"), Alias::new("fetched_at"))
            .expr_as(age_secs("fetched_at"), Alias::new("age_secs"))
            .expr_as(as_text("tagged"), Alias::new("tagged"))
            .from(tbl("switch"))
            .clone();
        let sql = finalize_select(sel);
        assert!(
            sql.contains(r#"to_char(fetched_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"')"#),
            "{sql}"
        );
        assert!(
            sql.contains("EXTRACT(EPOCH FROM (now() - fetched_at))::bigint"),
            "{sql}"
        );
        assert!(sql.contains("tagged::text"), "{sql}");
    }

    /// `@>` on a jsonb array: the containment test a VLAN-membership query needs,
    /// with the operand bound rather than concatenated.
    #[test]
    fn jsonb_containment_binds_its_operand() {
        use sea_query::extension::postgres::PgExpr;
        let sel = Query::select()
            .column(col("id"))
            .from(tbl("port"))
            .and_where(Expr::col(col("tagged")).contains(jsonb_array_of(7)))
            .clone();
        let sql = finalize_select(sel);
        assert!(sql.contains("@>"), "{sql}");
        assert!(sql.contains("'[7]'"), "{sql}");
    }

    #[test]
    fn count_star_is_a_plain_star() {
        let sel = Query::select()
            .expr_as(count_star(), Alias::new("n"))
            .from(tbl("change"))
            .clone();
        assert!(finalize_select(sel).contains("count(*)"));
    }

    /// The one-value escape hatch must escape, not concatenate.
    #[test]
    fn a_bound_custom_value_is_escaped() {
        let expr = cust_with_value("$1::jsonb", r#"["a'"]"#.to_string());
        let ins = sea_query::Query::insert()
            .into_table(tbl("change"))
            .columns(vec![iden("ports")])
            .values_panic(vec![expr.into()])
            .clone();
        let sql = finalize_insert(ins);
        assert!(sql.contains("::jsonb"), "{sql}");
        // The single quote inside the JSON is escaped by the inliner.
        assert!(sql.contains(r#"a\'"#) || sql.contains("''"), "{sql}");
    }

    /// A builder helper must not leave a statement alive for the caller: the
    /// whole point is that only the SQL string comes back, which is what keeps an
    /// `async fn` that calls it `Send`.
    #[test]
    fn the_builder_helpers_return_only_a_string() {
        let sql = select_sql(|sel| {
            sel.column(col("name"))
                .from(tbl("switch"))
                .and_where(Expr::col(col("name")).eq("switch1"));
        });
        assert!(sql.contains("FROM \"switch\""), "{sql}");
        assert!(sql.contains("'switch1'"), "{sql}");

        let sql = update_sql(|upd| {
            upd.table(tbl("change"))
                .value(iden("status"), "pending")
                .and_where(Expr::col(col("id")).eq(7));
        });
        assert!(sql.starts_with("UPDATE"), "{sql}");

        let sql = insert_sql(|ins| {
            ins.into_table(tbl("vlan"))
                .columns(vec![iden("switch_name"), iden("id")])
                .values_panic(vec![ev("switch1").into(), evi(Some(225)).into()]);
        });
        assert!(sql.contains("INSERT INTO \"vlan\""), "{sql}");

        let sql = delete_sql(|del| {
            del.from_table(tbl("port"))
                .and_where(Expr::col(col("switch_name")).eq("switch1"));
        });
        assert!(sql.starts_with("DELETE FROM"), "{sql}");
    }
}
