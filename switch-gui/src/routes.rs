//! The HTTP surface: the JSON API the SPA calls, the SPA itself, and sign-in.
//!
//! Every handler here answers from Postgres.  None of them opens a console, and
//! that is the whole point of the rewrite: the request path is a handful of
//! indexed queries, so the page is instant, and a switch that is down or a login
//! that takes twenty seconds is the sync's problem, not the browser's.  The one
//! thing a handler may do to a switch is *queue* something
//! ([`crate::intent::plan`] → [`crate::changes::queue`]).
//!
//! Handlers return a [`common_web::Reply`] rather than writing to the socket, so
//! the routing table reads as a table and the tests can assert on a response
//! without a listener.
//!
//! ## Sign-in
//!
//! Unchanged in contract from the pre-database GUI, and deliberately so: either
//! nginx-sso has already authenticated the request (`X-WEBAUTH-USER`, injected by
//! the LAN-only vhost) or the request carries this app's own session cookie from
//! the authentik OIDC dance in [`common_oidc`].  The two are checked in that order
//! because the vhost is the production path and the cookie is what makes the tool
//! runnable on a loopback bind without authentik.

use crate::changes;
use crate::dto;
use crate::intent;
use crate::shadow;
use crate::R;
use common_web::{cookie_value, query_params, Reply};
use std::collections::HashMap;

/// What a handler may reach.  The HTTP threads get one of these; the sync has its
/// own handle to the same pool and devices.
pub struct Shared {
    /// The async runtime, for `block_on` of the database and OIDC futures from a
    /// plain std thread (the house pattern: gocardless-dashboard does the same).
    pub rt: tokio::runtime::Handle,
    pub pool: sqlx::PgPool,
    /// The authentik login, when one is configured.  `None` with `dev_user` set is
    /// the testing escape hatch; `None` with neither means nothing is served
    /// (there is no anonymous view of a switch's configuration).
    pub oidc: Option<common_oidc::Oidc>,
    pub dev_user: Option<String>,
    /// `--no-auth`: every request is the local operator, with no name to go with
    /// it.  Testing only, and refused on a non-loopback bind in `main` — the same
    /// argument the audit log makes about trusting `X-WEBAUTH-USER`: there is no
    /// path to this socket that does not go through nginx.
    pub no_auth: bool,
    /// `--read-only`: refuse a write where the operator is, rather than queueing a
    /// row for the sync to fail.  The sync refuses too — a row queued by hand into
    /// the database still cannot reach a switch — but a refusal in the response is
    /// the one the browser shows.
    pub read_only: bool,
    /// The attempt ladder for a queued change (see [`crate::changes::queue`]).
    pub max_attempts: i32,
}

impl Shared {
    pub fn cookie_name(&self) -> &str {
        self.oidc
            .as_ref()
            .map(|o| o.cookie_name())
            .unwrap_or("switch_session")
    }

    /// The signed-in user, if any.
    ///
    /// `webauth` is the `X-WEBAUTH-USER` header nginx-sso injects.  It is trusted
    /// because the vhost is LAN-only and the app binds loopback: there is no path
    /// to this socket that does not go through nginx (the same reasoning the
    /// pre-database GUI documented, and the reason `--no-auth` is refused on a
    /// non-loopback bind).
    pub fn user(&self, webauth: &str, cookie_header: &str) -> Option<String> {
        if !webauth.is_empty() {
            return Some(webauth.to_string());
        }
        if self.no_auth {
            return Some(NO_AUTH_USER.to_string());
        }
        if let Some(user) = &self.dev_user {
            return Some(user.clone());
        }
        let oidc = self.oidc.as_ref()?;
        let token = cookie_value(cookie_header, oidc.cookie_name())?;
        oidc.session_for_cookie(&token).map(|s| s.username)
    }
}

/// Who an unauthenticated request is when `--no-auth` is on: not anonymous (there
/// is no anonymous view of a switch), and not the operator's name either, because
/// the audit log must not attribute a test write to a person who was never asked
/// to sign in.
pub const NO_AUTH_USER: &str = "local";

/// Route one request.
pub fn handle(req: &common_web::Request, shared: &Shared) -> R<Reply> {
    let path = req.path().to_string();
    let query = query_params(&req.raw_path);
    let cookie = req.header("cookie").to_string();
    let webauth = req.header("x-webauth-user").to_string();

    // Reachable signed out: the OIDC dance and the health check.
    if path.starts_with("/auth/") || path == "/healthz" {
        return auth(&path, &query, &cookie, shared);
    }

    let Some(user) = shared.user(&webauth, &cookie) else {
        return Ok(if shared.oidc.is_some() {
            // No `next` parameter is honoured: there is one page, so there is
            // nothing worth preserving across sign-in, and refusing it is also
            // what keeps this from being an open redirect.
            Reply::redirect("/auth/login", &[])
        } else {
            Reply::text(
                401,
                "sign-in is not configured on this instance (set the OIDC client pair, or use --dev-user/--no-auth on a loopback bind)",
            )
        });
    };

    if path == "/logout" {
        if let (Some(oidc), Some(c)) = (&shared.oidc, cookie_value(&cookie, shared.cookie_name())) {
            oidc.drop_session(&c);
        }
        return Ok(Reply::redirect(
            "/",
            &[format!("{}=\"\"; Path=/; Max-Age=0", shared.cookie_name())],
        ));
    }

    match (req.method.as_str(), path.as_str()) {
        // The SPA.  One document, several URLs: `/switch/switch2` is a route the
        // client takes from `location.pathname`, and every unmatched GET below
        // the root serves the same document so a refresh or a pasted link lands
        // where it says.
        ("GET", "/") | ("GET", "/index.html") => Ok(spa()),
        ("GET", p) if p.starts_with("/switch/") => Ok(spa()),
        ("GET", p) if p.starts_with("/assets/") => Ok(asset(p)),

        ("GET", "/api/whoami") => Ok(json(&dto::Whoami { user })),
        ("GET", "/api/state") => Ok(state(shared, &user)),
        ("GET", "/api/switch") => Ok(switch_page(shared, &query)),
        ("GET", "/api/pending") => Ok(pending(shared, &query)),
        ("GET", "/api/changes") => Ok(history(shared, &query)),
        ("GET", "/api/export") => Ok(export(shared, &query)),

        ("POST", "/api/intent") => Ok(post_intent(shared, req, &user)),
        ("POST", "/api/change/retry") => Ok(post_change_action(shared, req, Action::Retry)),
        ("POST", "/api/change/forget") => Ok(post_change_action(shared, req, Action::Forget)),
        ("POST", "/api/change/revert") => Ok(post_change_action(shared, req, Action::Revert)),

        ("GET", "/api/audit") => Ok(history(shared, &query)),
        _ => Ok(Reply::text(404, "not found")),
    }
}

/// What a change-row action does.  Three separate endpoints rather than a flag on
/// one, because "delete this history" and "try again" must not be one typo apart.
enum Action {
    Retry,
    Forget,
    Revert,
}

// ------------------------------------------------------------------ sign-in

/// The OIDC endpoints and the health check.
fn auth(path: &str, query: &HashMap<String, String>, cookie: &str, shared: &Shared) -> R<Reply> {
    if path == "/healthz" {
        // Liveness only, and deliberately not "can I reach a switch": the unit is
        // healthy while a switch is down, and a probe that conflates the two would
        // restart a working service over a dead device.
        return Ok(Reply::text(200, "ok"));
    }

    let Some(oidc) = &shared.oidc else {
        return Ok(Reply::text(
            400,
            "no OIDC client configured: sign in with --dev-user or set the env file",
        ));
    };

    match path {
        "/auth/login" => {
            // `begin_login` returns the /authorize/ URL and the state it will
            // expect back.  The state goes out as a cookie so the callback can
            // check it arrived from the same browser it left: that is the CSRF
            // check the PKCE flow relies on and the one thing the query string
            // cannot provide on its own.
            let (url, state) = oidc.begin_login();
            let mut reply = Reply::redirect(&url, &[]);
            reply.headers.push((
                "Set-Cookie".to_string(),
                format!("{LOGIN_COOKIE}={state}; Path=/; Max-Age=600; HttpOnly; SameSite=Lax"),
            ));
            Ok(reply)
        }
        "/auth/callback" => {
            let code = match query.get("code") {
                Some(c) => c.clone(),
                None => {
                    let why = query
                        .get("error")
                        .cloned()
                        .or_else(|| query.get("error_description").cloned())
                        .unwrap_or_else(|| "no code".to_string());
                    return Ok(Reply::html(400, &error_page(&why)));
                }
            };
            let state = query.get("state").cloned().unwrap_or_default();
            let expected = cookie_value(cookie, LOGIN_COOKIE).unwrap_or_default();
            if !expected.is_empty() && expected != state {
                return Ok(Reply::html(
                    400,
                    &error_page("the login state did not match this browser — start again"),
                ));
            }
            match shared.rt.block_on(oidc.finish_login(&code, &state)) {
                Ok((session_cookie, _session)) => {
                    let mut reply = Reply::redirect("/", &[]);
                    reply.headers.push((
                        "Set-Cookie".to_string(),
                        format!(
                            "{}={}; Path=/; Max-Age=43200; HttpOnly; SameSite=Lax",
                            oidc.cookie_name(),
                            session_cookie
                        ),
                    ));
                    // The login-state cookie has one use.
                    reply.headers.push((
                        "Set-Cookie".to_string(),
                        format!("{LOGIN_COOKIE}=\"\"; Path=/; Max-Age=0"),
                    ));
                    Ok(reply)
                }
                Err(e) => Ok(Reply::html(401, &error_page(&e.0))),
            }
        }
        other => Ok(Reply::text(404, &format!("no such auth endpoint: {other}"))),
    }
}

/// The cookie that carries the OIDC `state` between /auth/login and the callback.
const LOGIN_COOKIE: &str = "switch_login";

/// The one page a signed-out or refused visitor ever sees.
fn error_page(why: &str) -> String {
    format!(
        "<!doctype html><meta charset=utf-8><title>switch-gui</title>\
         <body style=\"font:16px system-ui;margin:3rem\"><h1>Sign-in failed</h1>\
         <p>{why}</p><p><a href=\"/auth/login\">Try again</a></p>"
    )
}

// -------------------------------------------------------------------- API

/// `GET /api/state` — the index.
fn state(shared: &Shared, user: &str) -> Reply {
    match shared.rt.block_on(shadow::index(&shared.pool)) {
        Ok(mut switches) => {
            let ready = switches.iter().any(|s| s.fetched_at.is_some());
            // `--read-only` is a property of this process, not of the rows in the
            // database, so it is stamped on after the read rather than stored.
            for s in switches.iter_mut() {
                s.read_only = shared.read_only;
            }
            json(&dto::State {
                user: user.to_string(),
                ready,
                switches,
            })
        }
        Err(e) => server_error(&e),
    }
}

/// `GET /api/switch?name=` — one switch: the shadow copy and its open queue.
///
/// Two queries, and the queue is joined in Rust rather than in SQL so the page
/// can show a change that names a port the shadow copy does not have (a change
/// queued against a switch that has since been re-read, or a typo that got through
/// before the check existed) instead of silently dropping it.
fn switch_page(shared: &Shared, query: &HashMap<String, String>) -> Reply {
    let name = query.get("switch").cloned().unwrap_or_default();
    if name.is_empty() {
        return Reply::json(
            400,
            &format!(
                "{}",
                serde_json::json!({"error": "which switch? (?switch=switch1)"})
            ),
        );
    }
    let both = shared.rt.block_on(async {
        let page = shadow::page(&shared.pool, &name).await?;
        let open = changes::open(&shared.pool, Some(&name)).await?;
        Ok::<_, String>((page, open))
    });
    match both {
        Ok((Some(mut page), open)) => {
            page.pending = open;
            // As in `state`: a fact about this process, not about the row.
            page.read_only = shared.read_only;
            json(&page)
        }
        Ok((None, _)) => Reply::json(
            404,
            &serde_json::json!({"error": format!("no switch named `{name}`")}).to_string(),
        ),
        Err(e) => server_error(&e),
    }
}

/// `GET /api/pending?switch=` — the open queue, for the banner.
fn pending(shared: &Shared, query: &HashMap<String, String>) -> Reply {
    let name = query.get("switch").cloned();
    match shared
        .rt
        .block_on(changes::open(&shared.pool, name.as_deref()))
    {
        Ok(changes) => json(&dto::Changes { changes }),
        Err(e) => server_error(&e),
    }
}

/// `GET /api/changes?switch=` (and the old `/api/audit`) — the audit trail.
fn history(shared: &Shared, query: &HashMap<String, String>) -> Reply {
    let name = query.get("switch").cloned();
    match shared
        .rt
        .block_on(changes::history(&shared.pool, name.as_deref()))
    {
        Ok(changes) => json(&dto::Changes { changes }),
        Err(e) => server_error(&e),
    }
}

/// `GET /api/export?switch=` — the shadow copy and its history as one document.
///
/// The JSON view the pre-database GUI exposed for scripting, kept because the
/// scripts exist.  It is the same data the page renders, which is the point: one
/// reader, one shape, no second implementation to drift.
fn export(shared: &Shared, query: &HashMap<String, String>) -> Reply {
    let name = query.get("switch").cloned();
    let out = shared.rt.block_on(async {
        let switches = shadow::index(&shared.pool).await?;
        let mut doc = serde_json::Map::new();
        doc.insert(
            "switches".into(),
            serde_json::to_value(&switches).map_err(|e| e.to_string())?,
        );
        let wanted: Vec<String> = match &name {
            Some(n) => vec![n.clone()],
            None => switches.iter().map(|s| s.name.clone()).collect(),
        };
        let mut pages = Vec::new();
        for n in wanted {
            if let Some(p) = shadow::page(&shared.pool, &n).await? {
                pages.push(serde_json::to_value(p).map_err(|e| e.to_string())?);
            }
        }
        doc.insert("shadow".into(), serde_json::Value::Array(pages));
        doc.insert(
            "changes".into(),
            serde_json::to_value(changes::history(&shared.pool, name.as_deref()).await?)
                .map_err(|e| e.to_string())?,
        );
        Ok::<_, String>(serde_json::Value::Object(doc))
    });
    match out {
        Ok(v) => Reply::json(200, &v.to_string()),
        Err(e) => server_error(&e),
    }
}

/// `POST /api/intent` — validate, plan, queue.  The only write path.
///
/// The response is the queued row, and the client's job after this is to keep
/// showing the port as pending until the queue says otherwise: nothing here waits
/// for the switch.
fn post_intent(shared: &Shared, req: &common_web::Request, user: &str) -> Reply {
    let intent: dto::Intent = match serde_json::from_str(&req.body_text()) {
        Ok(i) => i,
        Err(e) => {
            return json(&dto::Ack::refused(format!(
                "the request body must be one intent as JSON: {e}"
            )))
        }
    };

    // `--read-only` is the escape hatch for a switch you want looked at but not
    // touched.  Refuse here rather than queueing a row the sync will only fail:
    // the operator is looking at this response, and the journal is not.
    // A `refresh` is not a write, so it still works.
    if shared.read_only && intent.kind != "refresh" {
        return json(&dto::Ack::refused(crate::READ_ONLY));
    }

    let planned = match shared.rt.block_on(intent::plan(&shared.pool, &intent)) {
        Ok(p) => p,
        Err(e) => return json(&dto::Ack::refused(e)),
    };
    // "already in that state" is not a failure of the request; the SPA shows it
    // as a note rather than an error, and `ok: true` is what says so.  The `Err`
    // is the same sentence every time and the SPA is the place an operator reads
    // it, so it is not repeated back here.
    if intent::check_queueable(&planned).is_err() {
        return json(&dto::Ack::noop());
    }

    // The journal line for the click, naming the note if there was one: the
    // database has the audit row, and this is what ties it to a request in the
    // unit's log.  Built from the shadow copy the plan was diffed against, so the
    // line also says which snapshot the operator was looking at.
    let line = match shared
        .rt
        .block_on(shadow::snapshot(&shared.pool, &intent.switch_name))
    {
        Ok(Some(snap)) => intent::log_line(&planned, &snap, user),
        // No snapshot to name. `plan` could not have produced `planned` without
        // one, so this is a switch deleted between the plan and here; log what is
        // known rather than losing the line.
        _ => format!(
            "queued {} for {}: {}{} by {user}",
            planned.kind,
            intent.switch_name,
            planned.summary,
            note_note(planned.note.as_deref().unwrap_or(""))
        ),
    };
    eprintln!("switch-gui: {line}");
    let queued = shared.rt.block_on(changes::queue(
        &shared.pool,
        &intent.switch_name,
        planned.kind,
        &planned.ports,
        &planned.summary,
        &planned.commands,
        &planned.before,
        user,
        // The attempt ladder: five goes at a switch that is merely unreachable,
        // which is the common case, and the same five for a refusal that will not
        // change (the operator sees the count and the reason, and can Retry).
        shared.max_attempts,
        None,
        planned.note.as_deref(),
    ));
    match queued {
        Ok(change) => json(&dto::Ack::queued(change)),
        Err(e) => server_error(&e),
    }
}

/// The journal's rendering of an operator's note: set off, or nothing at all.
fn note_note(note: &str) -> String {
    let note = note.trim();
    if note.is_empty() {
        String::new()
    } else {
        format!(" — {note}")
    }
}

/// `POST /api/change/{retry,forget,revert}` — act on one queued row.
fn post_change_action(shared: &Shared, req: &common_web::Request, action: Action) -> Reply {
    // The same refusal as `POST /api/intent`, and for the same reason: a read-only
    // instance must not touch a switch, and a row requeued or reverted here would
    // be applied by the sync on its next pass.  `Forget` is allowed — deleting a
    // row writes nothing anywhere — so that an operator can clear a queue a
    // `--read-only` deployment should never have filled.
    if shared.read_only && !matches!(action, Action::Forget) {
        return Reply::json(
            200,
            &serde_json::json!({
                "ok": false,
                "error": format!("{}; nothing was queued", crate::READ_ONLY)
            })
            .to_string(),
        );
    }
    #[derive(serde::Deserialize)]
    struct Id {
        id: i64,
    }
    let id = match serde_json::from_str::<Id>(&req.body_text()) {
        Ok(i) => i.id,
        Err(e) => return json(&dto::Ack::refused(format!("expected {{\"id\":123}}: {e}"))),
    };

    match action {
        Action::Retry => match shared.rt.block_on(changes::requeue(&shared.pool, id)) {
            Ok(true) => json(&dto::Ack {
                ok: true,
                change: shared
                    .rt
                    .block_on(changes::by_id(&shared.pool, id))
                    .ok()
                    .flatten(),
                error: None,
            }),
            Ok(false) => json(&dto::Ack::refused(format!(
                "change {id} is not retryable (it is applied, or it is gone)"
            ))),
            Err(e) => server_error(&e),
        },
        Action::Forget => match shared.rt.block_on(changes::forget(&shared.pool, id)) {
            Ok(true) => json(&dto::Ack::noop()),
            Ok(false) => json(&dto::Ack::refused(format!(
                "change {id} has already been written or given up, so it stays in the \
                 audit log — only a change that has not reached the switch can be discarded"
            ))),
            Err(e) => server_error(&e),
        },
        Action::Revert => {
            let change = match shared.rt.block_on(changes::by_id(&shared.pool, id)) {
                Ok(Some(c)) => c,
                Ok(None) => return json(&dto::Ack::refused(format!("no change {id}"))),
                Err(e) => return server_error(&e),
            };
            let planned = match shared.rt.block_on(intent::revert(&shared.pool, &change)) {
                Ok(p) => p,
                Err(e) => return json(&dto::Ack::refused(e)),
            };
            let queued = shared.rt.block_on(changes::queue(
                &shared.pool,
                &change.switch_name,
                planned.kind,
                &planned.ports,
                &planned.summary,
                &planned.commands,
                &planned.before,
                // The operator who clicked Revert, not the author of the original:
                // the audit log records who did this thing.
                &revert_user(&change),
                shared.max_attempts,
                Some(change.id),
                planned.note.as_deref(),
            ));
            match queued {
                Ok(c) => json(&dto::Ack::queued(c)),
                Err(e) => server_error(&e),
            }
        }
    }
}

/// The `created_by` of a revert: the person who clicked, recorded as such.
///
/// The original author is already in the row being reverted, and `reverted_from`
/// points at it, so nothing is lost by naming the person responsible for *this*
/// write — but the suffix means a glance at the log never confuses the two.
fn revert_user(change: &dto::Change) -> String {
    format!("{} (revert of {})", change.created_by, change.id)
}

// --------------------------------------------------------------------- SPA

/// The single-page app: one document, which the client then routes.
///
/// `no-store` on the HTML and a long `max-age` on the hashed assets is the only
/// combination that is right: the document must always be revalidated (it is what
/// names the assets), and an asset's name cannot outlive its contents.
fn spa() -> Reply {
    match crate::find_asset("index.html") {
        Some(a) => Reply::html(200, std::str::from_utf8(a.data).unwrap_or(""))
            .header("Cache-Control", "no-store"),
        None => Reply::html(
            200,
            // No bundle in the binary: a `cargo build` without the frontend. The
            // API still works, so say that rather than 404 a page the operator
            // is expecting.
            "<!doctype html><meta charset=utf-8><title>switch-gui</title>\
             <body style=\"font:16px system-ui;margin:3rem\">\
             <h1>switch-gui</h1><p>No SPA bundle is embedded in this binary.</p>\
             <p>The JSON API is live: <a href=\"/api/state\">/api/state</a>.</p>",
        )
        .header("Cache-Control", "no-store"),
    }
}

/// One bundled asset, by name.
fn asset(path: &str) -> Reply {
    let name = path.trim_start_matches("/assets/");
    match crate::find_asset(name) {
        Some(a) => Reply::new(200, a.mime, a.data.to_vec())
            .header("Cache-Control", "public, max-age=604800, immutable"),
        None => Reply::text(404, "no such asset"),
    }
}

// ------------------------------------------------------------------ helpers

/// A 200 JSON reply for a serialisable value.
fn json<T: serde::Serialize>(value: &T) -> Reply {
    match serde_json::to_string(value) {
        Ok(body) => Reply::json(200, &body),
        Err(e) => Reply::json(500, &format!("{{\"error\":\"encode: {e}\"}}")),
    }
}

/// A 500 with the message.  Postgres errors are safe to show this way (they name
/// a table and a constraint, never a password) and are far more useful than a
/// bare "server error" to whoever reads the journal five minutes later.
fn server_error(e: &str) -> Reply {
    Reply::json(500, &serde_json::json!({"error": e}).to_string())
}
