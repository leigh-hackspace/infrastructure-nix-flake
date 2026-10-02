//! HTTP server (tiny_http): the JSON API, the OIDC endpoints and the
//! embedded static SPA.  This is an internal LAN-only operator dashboard
//! fronted by nginx (forceSSL), so the server itself is plain HTTP on
//! 127.0.0.1.

use std::collections::HashMap;
use std::sync::Arc;

use tiny_http::{Header, Response, Server};

use crate::auth;
use crate::db;
use crate::sync;
use crate::Shared;

pub fn run(shared: Arc<Shared>) -> ! {
    let addr = format!("{}:{}", shared.cfg.bind, shared.cfg.port);
    let server = Server::http(&addr).unwrap_or_else(|e| {
        eprintln!("cannot bind {addr}: {e}");
        std::process::exit(1);
    });
    eprintln!("listening on {addr}");

    for request in server.incoming_requests() {
        let shared = shared.clone();
        std::thread::spawn(move || {
            let url = request.url().to_string();
            let (path, query) = match url.split_once('?') {
                Some((p, q)) => (p.to_string(), q.to_string()),
                None => (url.clone(), String::new()),
            };
            let method = request.method().to_string();
            let headers: HashMap<String, String> = request
                .headers()
                .iter()
                .map(|h| (h.field.as_str().to_string(), h.value.as_str().to_string()))
                .collect();

            // Housekeeping piggy-backed on traffic (1 in 4096 connections).
            if getrandom_int() % 4096 == 0 {
                auth::purge_expired(&shared);
            }

            let (status, resp_headers, body) =
                route(&shared, &method, &path, &query, &headers);
            let resp_headers: Vec<Header> = resp_headers
                .into_iter()
                .map(|(k, v)| Header::from_bytes(k, v).unwrap())
                .collect();
            let len = body.len();
            let resp = Response::new(
                tiny_http::StatusCode(status),
                resp_headers,
                std::io::Cursor::new(body),
                Some(len),
                None,
            );
            let _ = request.respond(resp);
        });
    }
    unreachable!()
}

fn getrandom_int() -> u32 {
    let mut b = [0u8; 4];
    let _ = getrandom::getrandom(&mut b);
    u32::from_le_bytes(b)
}

fn json_body(v: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_else(|_| b"{}".to_vec())
}

fn json_headers() -> Vec<(String, String)> {
    vec![("content-type".to_string(), "application/json".to_string())]
}

fn html_headers() -> Vec<(String, String)> {
    vec![("content-type".to_string(), "text/html; charset=utf-8".to_string())]
}

fn query_param(query: &str, name: &str) -> Option<String> {
    for kv in query.split('&') {
        if let Some((k, v)) = kv.split_once('=') {
            if k == name {
                return urlencoding::decode(v).ok().map(|s| s.into_owned());
            }
        }
    }
    None
}

fn cookie_value(headers: &HashMap<String, String>, name: &str) -> Option<String> {
    let header = headers.get("cookie")?;
    for kv in header.split(';') {
        if let Some((k, v)) = kv.trim().split_once('=') {
            if k.trim() == name {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn login_page(error_html: &str) -> Vec<u8> {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
<title>gocardless-dashboard</title>
<style>body{{background:#0f1216;color:#dbe1e8;font-family:system-ui;display:flex;align-items:center;justify-content:center;height:100vh;margin:0}}
.box{{background:#171c23;border:1px solid #242c36;border-radius:12px;padding:28px 34px;max-width:420px;text-align:center}}
.err{{color:#e07a7a;margin-top:10px}}</style></head>
<body><div class="box">
<h1 style="margin-top:0;font-size:18px">GoCardless dashboard</h1>
{error_html}
<a href="/auth/login" style="display:inline-block;margin-top:18px;padding:10px 22px;background:#25527e;color:#fff;border-radius:8px;text-decoration:none">Sign in with Authentik (Infra)</a>
</div></body></html>"#
    )
    .into_bytes()
}

/// Returns (status, headers, body).
fn route(
    shared: &Arc<Shared>,
    method: &str,
    path: &str,
    query: &str,
    headers: &HashMap<String, String>,
) -> (u16, Vec<(String, String)>, Vec<u8>) {
    // --- OIDC ---
    if path == "/auth/login" && method == "GET" {
        let (url, _state) = auth::begin_login(shared);
        return (302, vec![("location".into(), url)], Vec::new());
    }
    if path == "/auth/logout" {
        if let Some(cookie) = cookie_value(headers, auth::COOKIE_NAME) {
            shared.sessions.lock().unwrap().remove(&cookie);
        }
        return (
            302,
            vec![
                ("location".into(), "/".into()),
                (
                    "set-cookie".into(),
                    format!("{}=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax", auth::COOKIE_NAME),
                ),
            ],
            Vec::new(),
        );
    }
    if path == "/auth/callback" && method == "GET" {
        let err = query_param(query, "error").or_else(|| {
            query_param(query, "error_description").map(|d| format!("error: {d}"))
        });
        let code = query_param(query, "code");
        let state = query_param(query, "state");
        return match (err, code, state) {
            (None, None, _) => (
                400,
                html_headers(),
                login_page("<p class=err>missing code — start login again</p>"),
            ),
            (Some(e), _, _) => (
                403,
                html_headers(),
                login_page(&format!("<p class=err>{}</p>", escape_html(&e))),
            ),
            (_, Some(_c), None) => (
                400,
                html_headers(),
                login_page("<p class=err>missing state — start login again</p>"),
            ),
            (_, Some(c), Some(s)) => {
                match shared
                    .rt
                    .block_on(auth::finish_login(shared, &c, &s))
                {
                    Ok((cookie, _session)) => (
                        302,
                        vec![
                            ("location".into(), "/".into()),
                            (
                                "set-cookie".into(),
                                format!(
                                    "{}={cookie}; Max-Age=43200; Path=/; HttpOnly; SameSite=Lax",
                                    auth::COOKIE_NAME
                                ),
                            ),
                        ],
                        Vec::new(),
                    ),
                    Err(e) => (
                        403,
                        html_headers(),
                        login_page(&format!("<p class=err>{}</p>", escape_html(&e.0))),
                    ),
                }
            }
        };
    }

    // --- JSON API (session-gated) ---
    if path.starts_with("/api/") {
        let cookie = match cookie_value(headers, auth::COOKIE_NAME) {
            Some(c) => c,
            None => {
                return (
                    401,
                    json_headers(),
                    json_body(&serde_json::json!({"error": "unauthenticated"})),
                )
            }
        };
        if auth::session_for_cookie(shared, &cookie).is_none() {
            return (
                401,
                json_headers(),
                json_body(&serde_json::json!({"error": "unauthenticated"})),
            );
        }

        if path == "/api/session" {
            let sess = auth::session_for_cookie(shared, &cookie).unwrap();
            return (
                200,
                json_headers(),
                json_body(&serde_json::json!({
                    "username": sess.username,
                    "email": sess.email,
                })),
            );
        }
        if path == "/api/summary" {
            let snap = shared.snapshot.read().unwrap();
            let (last_error, in_flight) = {
                let st = shared.sync_state.lock().unwrap();
                (st.last_error.clone().or(snap.last_error.clone()), st.in_flight)
            };
            let mut s = snap.summary.clone();
            s.last_error = last_error;
            s.syncing = in_flight;
            return (
                200,
                json_headers(),
                json_body(&serde_json::to_value(&s).unwrap()),
            );
        }
        if path == "/api/customers" {
            let filter = query_param(query, "filter").unwrap_or_else(|| "all".into());
            let snap = shared.snapshot.read().unwrap();
            let list: Vec<&gdash_dto::CustomerView> = snap
                .customers
                .iter()
                .filter(|c| match filter.as_str() {
                    "active" => c.active,
                    "stale" => !c.active,
                    _ => true,
                })
                .collect();
            return (
                200,
                json_headers(),
                json_body(&serde_json::json!({"customers": list})),
            );
        }
        if let Some(rest) = path.strip_prefix("/api/customers/") {
            let id = rest.split('/').next().unwrap_or("");
            let snap = shared.snapshot.read().unwrap();
            return match snap.by_id.get(id) {
                Some(detail) => (200, json_headers(), json_body(&serde_json::json!(detail))),
                None => (
                    404,
                    json_headers(),
                    json_body(&serde_json::json!({"error": "unknown customer"})),
                ),
            };
        }
        if path == "/api/sync" && method == "POST" {
            {
                let st = shared.sync_state.lock().unwrap();
                if st.in_flight {
                    return (
                        202,
                        json_headers(),
                        json_body(&serde_json::json!({"status": "already syncing"})),
                    );
                }
            }
            shared.rt.spawn(sync::run_sync_owned(shared.clone()));
            return (
                202,
                json_headers(),
                json_body(&serde_json::json!({"status": "started"})),
            );
        }
        if path == "/api/authentik-sync" && method == "GET" {
            // The latest run (action="run") plus the most recent log rows.
            return match shared.rt.block_on(db::read_ak_log(&shared.pool, 500)) {
                Ok(rows) => {
                    let last_run = rows.iter().find(|r| r.action == "run").cloned();
                    (
                        200,
                        json_headers(),
                        json_body(&serde_json::json!({
                            "last_run": last_run,
                            "rows": rows,
                        })),
                    )
                }
                Err(e) => (
                    500,
                    json_headers(),
                    json_body(&serde_json::json!({"error": e})),
                ),
            };
        }
        if path == "/api/authentik-sync" && method == "POST" {
            // On-demand run (daily timer is the normal path).  The handler
            // thread is a plain std thread, so block_on is fine here.
            let opts = match shared.cfg.authentik_token.clone() {
                Some(token) => crate::authentik::AkOpts {
                    token,
                    base_url: shared.cfg.authentik_url.clone(),
                    group_name: shared.cfg.authentik_group.clone(),
                },
                None => {
                    return (
                        500,
                        json_headers(),
                        json_body(&serde_json::json!({"error": "AUTHENTIK_TOKEN not configured"})),
                    );
                }
            };
            return match shared
                .rt
                .block_on(crate::authentik::run(&shared.pool, &shared.http, &opts))
            {
                Ok(summary) => (
                    200,
                    json_headers(),
                    json_body(&serde_json::json!({"status": "ok", "summary": summary})),
                ),
                Err(e) => (
                    500,
                    json_headers(),
                    json_body(&serde_json::json!({"error": e})),
                ),
            };
        }
        return (
            404,
            json_headers(),
            json_body(&serde_json::json!({"error": "not found"})),
        );
    }

    // --- static SPA ---
    let asset_name = if path == "/" { "index.html" } else { path.trim_start_matches('/') };
    if let Some(a) = crate::find_asset(asset_name) {
        return (
            200,
            vec![
                ("content-type".to_string(), a.mime.to_string()),
            ],
            a.data.to_vec(),
        );
    }
    // SPA fallback: paths that don't look like file requests (no extension in
    // the final segment) are client-side routes (e.g. /customers/<id>).  Serve
    // index.html for GETs so a refresh or direct load lands on the SPA and the
    // router restores the view, instead of a 404.
    let is_file_path = path
        .rsplit('/')
        .next()
        .is_some_and(|seg| seg.contains('.'));
    if method == "GET" && !is_file_path {
        if let Some(a) = crate::find_asset("index.html") {
            return (
                200,
                vec![("content-type".to_string(), a.mime.to_string())],
                a.data.to_vec(),
            );
        }
    }
    (
        404,
        html_headers(),
        b"<h1>404</h1>".to_vec(),
    )
}
