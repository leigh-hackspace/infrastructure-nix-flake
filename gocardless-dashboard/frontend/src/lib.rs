//! gocardless-dashboard web UI — a Dioxus (wasm) SPA.
//!
//! Shows a GoCardless overview: customer counts, active/stale breakdown,
//! 30-day payment/refund totals, and a customer table (sortable, filterable,
//! text-searchable).  Clicking a customer opens their detail view (mandates,
//! subscriptions, payments) and pushes a `/customers/<id>` route so the
//! browser back/forward buttons work.  Login is an OIDC redirect to
//! authentik (Infra group only).
//!
//! rsx notes (dioxus 0.7): text interpolation only captures identifiers in
//! scope (no trailing `name = expr` args), and `for` loop bodies / `match`
//! arms must each be a single node — so per-row locals are bound in a braced
//! block ending in one `rsx!`.

use dioxus::prelude::*;
use gloo_net::http::Request;
use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;

/// Navigate the whole page (used for the OIDC login/logout redirects, which
/// must round-trip through the server, not the SPA).
fn navigate(url: &str) {
    web_sys::window().expect("window").location().set_href(url).ok();
}

// ---------------------------------------------------------------------------
// History integration (browser back/forward).
//
// The SPA maps a URL path to the view: "/" is the customer list,
// "/customers/<id>" is that customer's detail.  Forward navigation (clicking
// a row) pushes a history entry and sets the signal; the back/forward buttons
// fire popstate, which re-derives the selected customer from the URL.  This
// also makes detail pages reload/deep-link correctly.
// ---------------------------------------------------------------------------

fn current_path() -> String {
    web_sys::window()
        .and_then(|w| w.location().pathname().ok())
        .unwrap_or_else(|| "/".into())
}

/// The selected-customer id encoded in `path` (Some on /customers/<id>).
fn path_to_selected(path: &str) -> Option<String> {
    path.strip_prefix("/customers/")
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Push a new history entry for `path` (forward navigation only; does not
/// fire popstate, so the caller also sets the signal directly).
fn push_path(path: &str) {
    if let Some(w) = web_sys::window() {
        if let Ok(h) = w.history() {
            let _ = h.push_state_with_url(&JsValue::NULL, "", Some(path));
        }
    }
}

/// Install the popstate listener once and seed the selection from the URL we
/// actually landed on (handles reloads and deep-links onto a detail page).
fn init_popstate(mut selected: Signal<Option<String>>) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    selected.set(path_to_selected(&current_path()));
    let closure = wasm_bindgen::closure::Closure::wrap(Box::new(move || {
        selected.set(path_to_selected(&current_path()));
    }) as Box<dyn FnMut()>);
    // addEventListener wants the underlying JS Function; leak the Rust closure
    // so its Drop (which invalidates the JS function) never runs for the page's
    // lifetime.
    let listener: js_sys::Function = closure.as_js_value().clone().unchecked_into();
    let _ = window.add_event_listener_with_callback("popstate", &listener);
    std::mem::forget(closure);
}

#[derive(Clone, Debug, Deserialize)]
struct Session {
    username: String,
}

// Shared API types come from the gdash-dto crate — the same definitions the
// backend decodes GoCardless JSON into and serves (CustomerView is what the
// customer list/detail endpoints return).
use gdash_dto::{CustomerDetail as Detail, CustomerList, CustomerView, Summary};

async fn get_json<T: for<'de> Deserialize<'de>>(url: &str) -> Result<T, String> {
    let resp = Request::get(url).send().await.map_err(|e| format!("{url}: {e}"))?;
    if !resp.ok() {
        return Err(format!("{url}: HTTP {}", resp.status()));
    }
    resp.json::<T>().await.map_err(|e| format!("json: {e}"))
}

async fn post(url: &str) -> Result<(), String> {
    let req = Request::post(url)
        .header("content-type", "application/json")
        .body("{}".to_string())
        .map_err(|e| format!("{url}: {e}"))?;
    let resp = req.send().await.map_err(|e| format!("{url}: {e}"))?;
    if !resp.ok() {
        return Err(format!("{url}: HTTP {}", resp.status()));
    }
    Ok(())
}

fn gbp(cents: i64) -> String {
    let neg = cents < 0;
    let c = cents.unsigned_abs();
    format!(
        "{}£{},{:02}",
        if neg { "-" } else { "" },
        c / 100,
        c % 100
    )
}

fn date(ts: &Option<String>) -> String {
    ts.as_ref()
        .map(|t| t.chars().take(10).collect())
        .unwrap_or_else(|| "—".into())
}

fn charge_str(charge: Option<i64>, currency: Option<String>) -> String {
    match (charge, currency) {
        (Some(c), Some(cur)) => format!("{}/{}", gbp(c), cur),
        (Some(c), None) => gbp(c),
        _ => "—".into(),
    }
}

fn status_pill(status: &Option<String>) -> Element {
    let cls = match status.as_deref() {
        Some("active") | Some("paid_out") | Some("succeeded") => "ok",
        Some("stale") => "stale",
        Some("revoked") | Some("failed") => "bad",
        _ => "mute",
    };
    let label = status.clone().unwrap_or_else(|| "unknown".into());
    rsx! { div { class: "pill {cls}", {label} } }
}

// ---------------------------------------------------------------------------
// Client-side filtering + sorting (pure helpers; the component reads the
// signals, so it re-renders whenever they change).
// ---------------------------------------------------------------------------

fn filter_customers(rows: &[CustomerView], filter: &str, query: &str) -> Vec<CustomerView> {
    let q = query.trim().to_lowercase();
    rows.iter()
        .filter(|c| {
            let status_ok = match filter {
                "active" => c.active,
                "stale" => !c.active,
                _ => true,
            };
            if !status_ok {
                return false;
            }
            if q.is_empty() {
                return true;
            }
            // Search over name, email and the GoCardless ids (customer id and
            // mandate ids) — the ids are what disambiguate same-name records.
            let hay = format!(
                "{} {} {} {}",
                c.name,
                c.email.as_deref().unwrap_or(""),
                c.id,
                c.mandate_ids.join(" "),
            )
            .to_lowercase();
            hay.contains(&q)
        })
        .cloned()
        .collect()
}

fn sort_customers(mut rows: Vec<CustomerView>, key: &str, desc: bool) -> Vec<CustomerView> {
    use std::cmp::Ordering;
    rows.sort_by(|a, b| {
        let ord = match key {
            "customer" => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            // Natural order: active first, then name (matches the backend).
            "status" => b
                .active
                .cmp(&a.active)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            "payments" => a.payment_count.cmp(&b.payment_count),
            "last_payment" => a.last_payment_at.cmp(&b.last_payment_at),
            "total_paid" => a.total_paid.cmp(&b.total_paid),
            _ => Ordering::Equal,
        };
        if desc { ord.reverse() } else { ord }
    });
    rows
}

/// The unauthenticated login screen: a single button that redirects to
/// authentik (the server's /auth/login issues the OIDC redirect).
#[component]
fn Login() -> Element {
    rsx! {
        div { class: "login",
            h1 { "GoCardless" }
            p { "Sign in with authentik (Infra group)." }
            button {
                class: "primary",
                onclick: move |_| navigate("/auth/login"),
                "Sign in with authentik"
            }
        }
    }
}

/// A single stat card in the overview strip.
#[component]
fn StatCard(label: String, value: String, tone: Option<&'static str>) -> Element {
    let cls = format!("card {tone}", tone = tone.unwrap_or(""));
    rsx! { div { class: "{cls}",
        div { class: "stat-label", "{label}" }
        div { class: "stat-value", "{value}" }
    } }
}

/// A sortable column header: label plus an arrow indicating the current sort.
#[component]
fn SortHeader(
    field: String,
    label: &'static str,
    cls: &'static str,
    sort_key: Signal<String>,
    sort_dir: Signal<bool>,
) -> Element {
    let on = *sort_key.read() == field;
    let arrow = if !on {
        ""
    } else if *sort_dir.read() {
        "↓"
    } else {
        "↑"
    };
    let mut sk = sort_key.clone();
    let mut sd = sort_dir.clone();
    rsx! {
        th {
            class: "sort {cls}",
            onclick: move |_| {
                if on {
                    let cur = *sd.read();
                    sd.set(!cur);
                } else {
                    sk.set(field.clone());
                    sd.set(false);
                }
            },
            "{label} {arrow}"
        }
    }
}

/// The customer table with a status filter (tabs), a free-text search box,
/// sortable column headers, and the GoCardless customer id shown on each row.
#[component]
fn CustomerTable(selected: Signal<Option<String>>) -> Element {
    let filter = use_signal(|| "all".to_string());
    let query = use_signal(|| String::new());
    let sort_key = use_signal(|| "status".to_string());
    let sort_dir = use_signal(|| false);

    let list = use_resource(move || async move {
        get_json::<CustomerList>("/api/customers?filter=all").await
    });

    let res = list.value();
    let guard = res.read();
    let state = guard.as_ref();

    let rows = match state {
        Some(Ok(page)) => sort_customers(
            filter_customers(&page.customers, &filter.read(), &query.read()),
            &sort_key.read(),
            *sort_dir.read(),
        ),
        _ => Vec::new(),
    };

    // Each handler needs its own clone of the signal: a signal can't be moved
    // into several closures while still being read for the `class:`/`rows`.
    let mut f_all = filter.clone();
    let mut f_active = filter.clone();
    let mut f_stale = filter.clone();
    let mut q_search = query.clone();

    rsx! {
        div { class: "filters",
            button {
                class: if *filter.read() == "all" { "on" } else { "" },
                onclick: move |_| f_all.set("all".to_string()),
                "All"
            }
            button {
                class: if *filter.read() == "active" { "on" } else { "" },
                onclick: move |_| f_active.set("active".to_string()),
                "Active"
            }
            button {
                class: if *filter.read() == "stale" { "on" } else { "" },
                onclick: move |_| f_stale.set("stale".to_string()),
                "Stale"
            }
            input {
                r#type: "text",
                class: "search",
                placeholder: "Search name, email, customer or mandate id…",
                value: "{query}",
                oninput: move |e| q_search.set(e.value()),
            }
            span { class: "count", "{rows.len()} shown" }
        }

        table {
            thead {
                tr {
                    SortHeader { field: "customer".to_string(), label: "Customer", cls: "", sort_key: sort_key.clone(), sort_dir: sort_dir.clone() }
                    SortHeader { field: "status".to_string(), label: "Status", cls: "", sort_key: sort_key.clone(), sort_dir: sort_dir.clone() }
                    th { "Mandate" }
                    th { "Mandate id" }
                    th { "Subscription" }
                    SortHeader { field: "payments".to_string(), label: "Payments", cls: "num", sort_key: sort_key.clone(), sort_dir: sort_dir.clone() }
                    SortHeader { field: "last_payment".to_string(), label: "Last payment", cls: "", sort_key: sort_key.clone(), sort_dir: sort_dir.clone() }
                    SortHeader { field: "total_paid".to_string(), label: "Total paid", cls: "num", sort_key: sort_key.clone(), sort_dir: sort_dir.clone() }
                }
            }
            tbody {
                if let Some(Err(e)) = state {
                    tr { td { colspan: "8", div { class: "empty", "failed to load: {e}" } } }
                } else if rows.is_empty() {
                    tr { td { colspan: "8", div { class: "empty", "no customers" } } }
                } else {
                    for c in rows.iter() {
                        {
                            let id = c.id.clone();
                            let name = c.name.clone();
                            let email = c.email.clone().unwrap_or_default();
                            let active = c.active;
                            let mandate = c.mandate_status.clone();
                            let has_active_mandate = c.has_active_mandate;
                            let mandate_ids = c.mandate_ids.join(", ");
                            let desc = c.sub_description.clone();
                            let charge = charge_str(c.sub_charge, c.sub_currency.clone());
                            let count = c.payment_count;
                            let last_at = date(&c.last_payment_at);
                            let last_amt = c.last_payment_amount.map(gbp);
                            let total = gbp(c.total_paid);
                            let mandate_pill = status_pill(&mandate);
                            let active_pill = if active {
                                rsx! { div { class: "pill ok", "active" } }
                            } else {
                                rsx! { div { class: "pill stale", "stale" } }
                            };
                            let mut sel = selected.clone();
                            rsx! {
                                tr {
                                    class: "row",
                                    onclick: move |_| {
                                        push_path(&format!("/customers/{id}"));
                                        sel.set(Some(id.clone()));
                                    },
                                    td {
                                        div { "{name}" }
                                        div { class: "sub", "{email}" }
                                        div { class: "id", "GC {id}" }
                                    }
                                    td {
                                        {active_pill}
                                        if !active && has_active_mandate {
                                            div { class: "sub hint", "active mandate, no active subscription" }
                                        }
                                    }
                                    td { {mandate_pill} }
                                    td {
                                        if mandate_ids.is_empty() {
                                            div { class: "sub", "none" }
                                        } else {
                                            for mid in mandate_ids.split(", ") {
                                                span { class: "id", style: "margin-right:5px", "{mid}" }
                                            }
                                        }
                                    }
                                    td {
                                        if let Some(d) = desc {
                                            "{d}"
                                        } else {
                                            div { class: "sub", "none" }
                                        }
                                    }
                                    td { class: "num", "{charge}" }
                                    td { class: "num", "{count}" }
                                    td {
                                        if let Some(a) = last_amt {
                                            "{last_at}"
                                            div { class: "sub", "{a}" }
                                        } else {
                                            "{last_at}"
                                        }
                                    }
                                    td { class: "num", "{total}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// "← all customers": use the browser back when we can (so the forward
/// button returns here); otherwise (deep-linked, no history) replace the URL
/// with the list route and reset the selection.
fn go_back_list(back: &mut Signal<Option<String>>) {
    if let Some(w) = web_sys::window() {
        if let Ok(h) = w.history() {
            if h.length().unwrap_or(0) > 1 {
                let _ = h.back();
                return;
            }
            let _ = h.replace_state_with_url(&JsValue::NULL, "", Some("/"));
        }
    }
    back.set(None);
}

/// A customer's detail view: mandates, subscriptions and payments, each with
/// its GoCardless id, plus a plain-English note on what "active" means.
#[component]
fn DetailView(mut back: Signal<Option<String>>) -> Element {
    let id = back.read().clone().unwrap_or_default();
    let detail = use_resource(move || {
        let id = id.clone();
        async move { get_json::<Detail>(&format!("/api/customers/{id}")).await }
    });

    let res = detail.value();
    let guard = res.read();
    let state = guard.as_ref();

    rsx! {
        div {
            span {
                class: "back",
                onclick: move |_| go_back_list(&mut back),
                "← all customers"
            }
            if let Some(Ok(d)) = state {
                {
                    let name = d.customer.name.clone();
                    let active = d.customer.active;
                    let email = d.customer.email.clone();
                    let created = d.customer.created_at.clone();
                    let cust_id = d.customer.id.clone();
                    let has_active_mandate = d.customer.has_active_mandate;
                    let n_mandates = d.mandates.len();
                    let n_subscriptions = d.subscriptions.len();
                    let n_payments = d.payments.len();
                    rsx! {
                        div { class: "detailhead",
                            h2 { "{name}" }
                            if active {
                                div { class: "pill ok", "active" }
                            } else {
                                div { class: "pill stale", "stale" }
                            }
                            span { class: "id", "GC {cust_id}" }
                            if let Some(e) = email {
                                span { class: "sub", "{e}" }
                            }
                            if let Some(ca) = created {
                                {
                                    let dt = date(&Some(ca));
                                    rsx! { span { class: "sub", "customer since {dt}" } }
                                }
                            }
                        }

                        // Clarify "active mandate but stale customer".
                        if !active && has_active_mandate {
                            div { class: "note",
                                "This customer is ",
                                b { "stale" },
                                " because although they hold a mandate that is itself ",
                                b { "active" },
                                ", none of their subscriptions on that mandate are currently active — so they are not being charged. ",
                                "“Active” (the status column) means a live mandate with at least one active subscription."
                            }
                        }

                        div { class: "subhead", "Mandates ({n_mandates})" }
                        table {
                            thead { tr { th { "Status" } th { "Mandate id" } th { "Created" } th { "Revoked" } } }
                            tbody {
                                if n_mandates == 0 {
                                    tr { td { colspan: "4", div { class: "sub", "none" } } }
                                } else {
                                    for m in d.mandates.iter() {
                                        {
                                            let status = m.status.clone();
                                            let mid = m.id.clone();
                                            let created = date(&m.created_at);
                                            let revoked = date(&m.revoked_at);
                                            let pill = status_pill(&status);
                                            rsx! {
                                                tr {
                                                    td { {pill} }
                                                    td { span { class: "id", "{mid}" } }
                                                    td { "{created}" }
                                                    td { "{revoked}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        div { class: "subhead", "Subscriptions ({n_subscriptions})" }
                        table {
                            thead {
                                tr {
                                    th { "Status" }
                                    th { "Subscription id" }
                                    th { "Description" }
                                    th { class: "num", "Charge" }
                                    th { "Next charge" }
                                    th { "Created" }
                                    th { "End" }
                                }
                            }
                            tbody {
                                if n_subscriptions == 0 {
                                    tr { td { colspan: "7", div { class: "sub", "none" } } }
                                } else {
                                    for s in d.subscriptions.iter() {
                                        {
                                            let status = s.status.clone();
                                            let sid = s.id.clone();
                                            let desc = s.name.clone();
                                            let charge = charge_str(s.amount, s.currency.clone());
                                            let next = date(&s.next_charge_date());
                                            let created = date(&s.created_at);
                                            let cancelled = date(&s.end_date);
                                            let pill = status_pill(&status);
                                            rsx! {
                                                tr {
                                                    td { {pill} }
                                                    td { span { class: "id", "{sid}" } }
                                                    td {
                                                        if let Some(d2) = desc {
                                                            "{d2}"
                                                        } else {
                                                            "—"
                                                        }
                                                    }
                                                    td { class: "num", "{charge}" }
                                                    td { "{next}" }
                                                    td { "{created}" }
                                                    td { "{cancelled}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        div { class: "subhead", "Payments ({n_payments})" }
                        table {
                            thead {
                                tr {
                                    th { "Status" }
                                    th { "Payment id" }
                                    th { "Reference" }
                                    th { class: "num", "Amount" }
                                    th { "Created" }
                                    th { "Charge date" }
                                }
                            }
                            tbody {
                                if n_payments == 0 {
                                    tr { td { colspan: "6", div { class: "sub", "none" } } }
                                } else {
                                    for p in d.payments.iter() {
                                        {
                                            let status = p.status.clone();
                                            let pid = p.id.clone();
                                            let reference = p.reference.clone();
                                            let amount = p.amount.map(|a| {
                                                match p.currency.clone() {
                                                    Some(c) => format!("{} {c}", gbp(a)),
                                                    None => gbp(a),
                                                }
                                            }).unwrap_or_else(|| "—".into());
                                            let created = date(&p.created_at);
                                            let paid = date(&p.charge_date);
                                            let pill = status_pill(&status);
                                            rsx! {
                                                tr {
                                                    td { {pill} }
                                                    td { span { class: "id", "{pid}" } }
                                                    td {
                                                        if let Some(r) = reference {
                                                            "{r}"
                                                        } else {
                                                            "—"
                                                        }
                                                    }
                                                    td { class: "num", "{amount}" }
                                                    td { "{created}" }
                                                    td { "{paid}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else if let Some(Err(e)) = state {
                div { class: "empty", "failed to load: {e}" }
            } else {
                div { class: "empty", "loading…" }
            }
        }
    }
}

#[component]
fn App() -> Element {
    let session = use_signal(|| None::<Session>);
    let summary = use_signal(|| None::<Summary>);
    let selected = use_signal(|| None::<String>);
    let loaded = use_signal(|| false);

    // Session check on mount.
    use_effect(move || {
        let mut session = session.clone();
        spawn(async move {
            match get_json::<Session>("/api/session").await {
                Ok(s) => session.set(Some(s)),
                Err(_) => session.set(None),
            }
        });
    });

    // Wire the browser back/forward buttons to the selected-customer view
    // (runs once: no reactive reads inside).
    {
        let popstate_selected = selected.clone();
        use_effect(move || {
            init_popstate(popstate_selected);
        });
    }

    // Once logged in, poll the summary (drives sync progress + stats).
    let mut polling_started = use_signal(|| false);
    use_effect(move || {
        if session.read().is_none() {
            return;
        }
        if *polling_started.read() {
            return;
        }
        polling_started.set(true);
        let mut summary = summary.clone();
        let mut loaded = loaded.clone();
        spawn(async move {
            loop {
                if let Ok(s) = get_json::<Summary>("/api/summary").await {
                    summary.set(Some(s));
                    loaded.set(true);
                }
                gloo_timers::future::sleep(std::time::Duration::from_secs(15)).await;
            }
        });
    });

    if session.read().is_none() {
        return rsx! { Login {} };
    }

    let sum = summary.read().clone();

    rsx! {
        div { class: "app",
            header {
                h1 { "GoCardless" }
                div { class: "statusline",
                    if let Some(s) = &sum {
                        if s.syncing {
                            "syncing…"
                        } else if let Some(ts) = s.last_sync.clone() {
                            {
                                let t = date(&Some(ts));
                                rsx! { "last sync {t}" }
                            }
                        } else {
                            "waiting for first sync"
                        }
                        if let Some(e) = s.last_error.clone() {
                            span { class: "err", " · {e}" }
                        }
                    }
                }
                div { class: "spacer",
                    button {
                        class: "action",
                        disabled: sum.as_ref().map(|s| s.syncing).unwrap_or(false),
                        onclick: {
                            let mut summary = summary;
                            move |_| {
                                spawn(async move {
                                    let _ = post("/api/sync").await;
                                    summary.set(None);
                                });
                            }
                        },
                        "Sync now"
                    }
                    if let Some(sess) = session.read().clone() {
                        span { class: "user", "{sess.username}" }
                    }
                    button {
                        class: "action",
                        onclick: move |_| navigate("/auth/logout"),
                        "Log out"
                    }
                }
            }

            if let Some(s) = &sum {
                div { class: "stats",
                    StatCard { label: "customers".to_string(), value: s.customers.to_string(), tone: None }
                    StatCard { label: "active".to_string(), value: s.active.to_string(), tone: Some("ok") }
                    StatCard { label: "stale".to_string(), value: s.stale.to_string(), tone: Some("warn") }
                    StatCard { label: "collected · 30d".to_string(), value: gbp(s.succeeded_30d), tone: Some("ok") }
                    StatCard { label: "failed · 30d".to_string(), value: s.failed_30d.to_string(), tone: Some("warn") }
                    StatCard { label: "refunded · 30d".to_string(), value: gbp(s.refunds_30d), tone: None }
                }
            }

            if let Some(e) = sum.as_ref().and_then(|s| s.last_error.clone()) {
                div { class: "errline", "sync error: {e}" }
            }

            if selected.read().is_none() {
                if !*loaded.read() {
                    div { class: "empty", "loading…" }
                } else {
                    div { class: "scroller",
                        CustomerTable { selected }
                    }
                }
            } else {
                div { class: "scroller",
                    DetailView { back: selected }
                }
            }
        }
    }
}

/// Wasm entry point: runs when the module initialises (the JS glue's init()).
#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen(start))]
pub fn start() {
    dioxus::launch(App);
}

/// Host entry point (so `cargo run` still type-checks off-wasm).
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    dioxus::launch(App);
}
