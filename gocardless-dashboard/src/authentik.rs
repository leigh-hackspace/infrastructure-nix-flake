//! Authentik `Members` group sync — replaces the old Python
//! `active_members_to_authentik.py` (gocardless-tools) which had two bugs:
//! it only ever read the *first page* (20) of authentik users, and its
//! user-creation always failed (username derived from the email collided
//! with existing usernames).
//!
//! Rules:
//!  - A GoCardless customer is a *valid member* when they hold an
//!    **active** subscription whose name contains "membership"
//!    (case-insensitive).
//!  - Valid members are added to the authentik `Members` group, matched to
//!    an authentik user by email (lowercase).  A member with no authentik
//!    account gets one created, like the old script tried to do.
//!  - authentik users *known to GoCardless* (whose
//!    `leighhack.org/gocardless-customer-id` attribute points at a current
//!    customer) who are no longer valid members are removed from the group.
//!  - Users *unknown* to GoCardless (no attribute, or an attribute that no
//!    longer matches a customer — e.g. manually added accounts) are left
//!    alone, per the membership policy.
//!  - The `leighhack.org/gocardless-customer-id` attribute and the display
//!    name are kept in sync (attribute updates avoid the multi-record
//!    emails where GoCardless keeps several customers per email).
//!
//! Runs daily from the `gocardless-authentik-sync` systemd timer (oneshot,
//! `--authentik-sync`) and on demand from the GUI.  Reads the local
//! Postgres snapshot (the daemon syncs GoCardless every 15 min), never the
//! GoCardless API directly.  Every run and every per-user action is written
//! to `authentik_sync_log`, which the GUI shows.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use sqlx::postgres::PgPool;

use crate::db::{self, DbData};

/// The authentik user attribute that records the GoCardless customer id.
pub const CUSTOMER_ID_ATTR: &str = "leighhack.org/gocardless-customer-id";
/// Case-insensitive marker in a subscription name that makes it a membership.
pub const MEMBERSHIP_MARKER: &str = "membership";

pub struct AkOpts {
    pub token: String,
    /// e.g. https://id.leighhack.org
    pub base_url: String,
    /// e.g. Members
    pub group_name: String,
}

// ---------------------------------------------------------------------------
// authentik API (tiny reqwest helpers; the API is plain DRF JSON)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct AkGroupRef {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct AkUser {
    pk: i64,
    #[serde(default)]
    username: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    is_active: bool,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    attributes: serde_json::Value,
    #[serde(default, rename = "groups_obj")]
    groups_obj: Vec<AkGroupRef>,
}

impl AkUser {
    fn group_names(&self) -> Vec<&str> {
        self.groups_obj.iter().map(|g| g.name.as_str()).collect()
    }

    /// The gocardless customer id attribute, if set to a string.
    fn customer_id(&self) -> Option<&str> {
        self.attributes.get(CUSTOMER_ID_ATTR).and_then(|v| v.as_str())
    }

    fn lower_email(&self) -> Option<String> {
        self.email
            .as_deref()
            .map(|e| e.trim().to_lowercase())
            .filter(|e| !e.is_empty())
    }
}

#[derive(Debug, Deserialize)]
struct AkList<T> {
    #[serde(default)]
    results: Vec<T>,
    #[serde(default)]
    pagination: AkPagination,
}

#[derive(Debug, Default, Deserialize)]
struct AkPagination {
    /// 0 when there is no next page, otherwise the next page number.
    #[serde(default)]
    next: i64,
}

/// Extract a short, log-worthy error from a DRF error body.
fn api_error(body: &str) -> String {
    let body = body.trim();
    let short = if body.len() > 200 {
        &body[..body.len().min(200)]
    } else {
        body
    };
    // DRF field errors look like {"username":["This field must be unique."]}
    match serde_json::from_str::<serde_json::Value>(short) {
        Ok(v) => v
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, val)| {
                        let msgs = val
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|x| x.as_str())
                                    .collect::<Vec<_>>()
                                    .join("; ")
                            })
                            .or_else(|| val.as_str().map(|s| s.to_string()))
                            .unwrap_or_default();
                        format!("{k}: {msgs}")
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|| short.to_string()),
        Err(_) => short.to_string(),
    }
}

async fn get_group_id(http: &reqwest::Client, opts: &AkOpts) -> Result<String, String> {
    let url = format!(
        "{}/api/v3/core/groups/?name={}",
        opts.base_url.trim_end_matches('/'),
        urlencoding::encode(&opts.group_name)
    );
    let resp = http
        .get(&url)
        .bearer_auth(&opts.token)
        .send()
        .await
        .map_err(|e| format!("groups request failed: {e}"))?;
    let status = resp.status();
    let body: AkList<serde_json::Value> = resp
        .json()
        .await
        .map_err(|e| format!("bad groups response: {e}"))?;
    if !status.is_success() {
        return Err(format!("groups endpoint: HTTP {status}"));
    }
    body.results
        .first()
        .and_then(|g| g.get("pk").and_then(|p| p.as_str().map(str::to_string)))
        .ok_or_else(|| format!("group '{}' not found in authentik", opts.group_name))
}

/// All authentik users, following pagination (the old Python script only
/// ever read the first page — 20 users — of which there are ~90).
async fn get_users(http: &reqwest::Client, opts: &AkOpts) -> Result<Vec<AkUser>, String> {
    let base = format!("{}/api/v3/core/users/", opts.base_url.trim_end_matches('/'));
    let mut users = Vec::new();
    let mut page = 1i64;
    // Hard cap so a misbehaving `next` can never loop forever.
    for _ in 0..100 {
        let url = format!("{base}?path=users&page_size=100&page={page}");
        let resp = http
            .get(&url)
            .bearer_auth(&opts.token)
            .send()
            .await
            .map_err(|e| format!("users request failed: {e}"))?;
        let status = resp.status();
        let body: AkList<AkUser> = resp
            .json()
            .await
            .map_err(|e| format!("bad users response: {e}"))?;
        if !status.is_success() {
            return Err(format!("users endpoint: HTTP {status}"));
        }
        users.extend(body.results);
        if body.pagination.next == 0 {
            break;
        }
        page = body.pagination.next;
    }
    Ok(users)
}

async fn patch_user(
    http: &reqwest::Client,
    opts: &AkOpts,
    pk: i64,
    body: &serde_json::Value,
) -> Result<(), String> {
    let url = format!(
        "{}/api/v3/core/users/{pk}/",
        opts.base_url.trim_end_matches('/')
    );
    let resp = http
        .patch(&url)
        .bearer_auth(&opts.token)
        .json(body)
        .send()
        .await
        .map_err(|e| format!("PATCH user {pk} failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("PATCH user {pk}: HTTP {status} {}", api_error(&text)));
    }
    Ok(())
}

/// Replace the user's attributes with the given object (the API takes the
/// whole object, so the caller merges existing keys first).
async fn set_attributes(
    http: &reqwest::Client,
    opts: &AkOpts,
    u: &AkUser,
    attr_key: &str,
    attr_value: &str,
) -> Result<(), String> {
    let mut attrs = if u.attributes.is_object() {
        u.attributes.clone()
    } else {
        serde_json::json!({})
    };
    attrs.as_object_mut().unwrap().insert(attr_key.to_string(), serde_json::json!(attr_value));
    patch_user(http, opts, u.pk, &serde_json::json!({ "attributes": attrs })).await
}

async fn group_action(
    http: &reqwest::Client,
    opts: &AkOpts,
    group_id: &str,
    pk: i64,
    action: &str, // "add_user" | "remove_user"
) -> Result<(), String> {
    let url = format!(
        "{}/api/v3/core/groups/{group_id}/{action}/",
        opts.base_url.trim_end_matches('/')
    );
    let resp = http
        .post(&url)
        .bearer_auth(&opts.token)
        .json(&serde_json::json!({ "pk": pk }))
        .send()
        .await
        .map_err(|e| format!("group {action} failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("group {action} user {pk}: HTTP {status} {}", api_error(&text)));
    }
    Ok(())
}

async fn create_user(
    http: &reqwest::Client,
    opts: &AkOpts,
    email: &str,
    name: &str,
    customer_id: &str,
) -> Result<i64, String> {
    let url = format!("{}/api/v3/core/users/", opts.base_url.trim_end_matches('/'));
    let username = email.split('@').next().unwrap_or(email);
    let resp = http
        .post(&url)
        .bearer_auth(&opts.token)
        .json(&serde_json::json!({
            "email": email,
            "username": username,
            "name": name,
            "is_active": true,
            "attributes": { CUSTOMER_ID_ATTR: customer_id }
        }))
        .send()
        .await
        .map_err(|e| format!("create user failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {status} {}", api_error(&text)));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("bad create response: {e}"))?;
    v.get("pk")
        .and_then(|p| p.as_i64())
        .ok_or_else(|| "no pk in create-user response".to_string())
}

// ---------------------------------------------------------------------------
// The sync
// ---------------------------------------------------------------------------

/// One audit-log row (owned; converted to db::AkLogInsert at write time).
struct Entry {
    action: &'static str,
    username: Option<String>,
    email: Option<String>,
    customer_id: Option<String>,
    detail: Option<String>,
}

impl Entry {
    fn new(action: &'static str) -> Self {
        Self {
            action,
            username: None,
            email: None,
            customer_id: None,
            detail: None,
        }
    }

    fn for_user(mut self, u: &AkUser) -> Self {
        self.username = Some(u.username.clone());
        self.email = u.email.clone();
        self
    }
}

/// Valid-member customer ids: a customer with an active subscription whose
/// name contains "membership" (case-insensitive).  Subscriptions carry no
/// direct customer link from the API — resolve through the mandate (see
/// db::resolve_customer).
pub fn valid_member_ids(data: &DbData) -> HashSet<String> {
    let mandate_customer = db::mandate_customer_index(&data.mandates);
    let mut members = HashSet::new();
    for s in &data.subscriptions {
        if s.status.as_deref() != Some("active") {
            continue;
        }
        let is_membership = s
            .name
            .as_deref()
            .map(|n| n.to_lowercase().contains(MEMBERSHIP_MARKER))
            .unwrap_or(false);
        if is_membership {
            if let Some(cid) = db::resolve_customer(&s.links, &mandate_customer) {
                members.insert(cid);
            }
        }
    }
    members
}

/// Run one authentik sync.  All audit rows (including the `run` summary
/// row, last) are written to `authentik_sync_log`.  Returns the counts for
/// the oneshot's console output; a run that cannot even fetch authentik is
/// still logged (action "run", detail "error: …").
pub async fn run(pool: &PgPool, http: &reqwest::Client, opts: &AkOpts) -> Result<String, String> {
    let data = db::load_all(pool).await?;
    if data.customers.is_empty() {
        let err = "no GoCardless data in the local DB yet — the dashboard sync has not run".to_string();
        log_entries(pool, &[Entry {
            action: "run",
            username: None,
            email: None,
            customer_id: None,
            detail: Some(format!("error: {err}")),
        }])
        .await?;
        return Err(err);
    }

    let members = valid_member_ids(&data);
    let customer_ids: HashSet<&str> = data.customers.iter().map(|c| c.id.as_str()).collect();
    let customer_by_id: HashMap<&str, &gdash_dto::Customer> =
        data.customers.iter().map(|c| (c.id.as_str(), c)).collect();
    // lowercase email -> customers with that email (GoCardless keeps
    // several customers per email for re-registrations)
    let mut by_email: HashMap<String, Vec<&gdash_dto::Customer>> = HashMap::new();
    for c in &data.customers {
        if let Some(e) = c.email.as_deref() {
            let e = e.trim().to_lowercase();
            if !e.is_empty() {
                by_email.entry(e).or_default().push(c);
            }
        }
    }

    let group_id = match get_group_id(http, opts).await {
        Ok(id) => id,
        Err(e) => {
            log_run_error(pool, &e).await?;
            return Err(e);
        }
    };
    let users = match get_users(http, opts).await {
        Ok(u) => u,
        Err(e) => {
            log_run_error(pool, &e).await?;
            return Err(e);
        }
    };

    let mut entries: Vec<Entry> = Vec::new();
    let mut counts: HashMap<&'static str, u32> = HashMap::new();
    let count = |counts: &mut HashMap<&'static str, u32>, k: &'static str| {
        *counts.entry(k).or_insert(0) += 1;
    };

    // --- per authentik user ---------------------------------------------
    for u in &users {
        let in_group = u.group_names().iter().any(|g| *g == opts.group_name);
        if !u.is_active {
            if in_group {
                entries.push(Entry::new("skipped_inactive").for_user(u));
                count(&mut counts, "skipped_inactive");
            }
            continue;
        }

        match u.customer_id() {
            Some(id) if customer_ids.contains(id) => {
                // Known to GoCardless: membership follows the customer.
                let is_member = members.contains(id);
                if is_member && !in_group {
                    match group_action(http, opts, &group_id, u.pk, "add_user").await {
                        Ok(()) => {
                            entries.push(Entry::new("add").for_user(u).with_customer(id));
                            count(&mut counts, "add");
                        }
                        Err(e) => {
                            entries.push(
                                Entry::new("error")
                                    .for_user(u)
                                    .with_customer(id)
                                    .with_detail(format!("add to group failed: {e}")),
                            );
                            count(&mut counts, "error");
                        }
                    }
                } else if !is_member && in_group {
                    match group_action(http, opts, &group_id, u.pk, "remove_user").await {
                        Ok(()) => {
                            entries.push(Entry::new("remove").for_user(u).with_customer(id));
                            count(&mut counts, "remove");
                        }
                        Err(e) => {
                            entries.push(
                                Entry::new("error")
                                    .for_user(u)
                                    .with_customer(id)
                                    .with_detail(format!("remove from group failed: {e}")),
                            );
                            count(&mut counts, "error");
                        }
                    }
                }

                let cust = customer_by_id.get(id).copied().unwrap();
                if is_member {
                    // Keep the authentik display name in step with GoCardless.
                    let want = cust.display_name();
                    if !want.is_empty() && u.name.as_deref() != Some(want.as_str()) {
                        match patch_user(
                            http,
                            opts,
                            u.pk,
                            &serde_json::json!({ "name": want }),
                        )
                        .await
                        {
                            Ok(()) => {
                                entries.push(
                                    Entry::new("name_updated")
                                        .for_user(u)
                                        .with_customer(id)
                                        .with_detail(format!("name -> {want}")),
                                );
                                count(&mut counts, "name_updated");
                            }
                            Err(e) => {
                                entries.push(
                                    Entry::new("error")
                                        .for_user(u)
                                        .with_customer(id)
                                        .with_detail(format!("name update failed: {e}")),
                                );
                                count(&mut counts, "error");
                            }
                        }
                    }
                }

                // Attribute accuracy: if this user's email belongs to exactly
                // one GoCardless customer and it is not the one recorded,
                // correct it.  Ambiguous emails (multiple customers) are left
                // alone — any correction would be a guess.
                if let Some(el) = u.lower_email() {
                    if let Some(cands) = by_email.get(&el) {
                        if cands.len() == 1 && cands[0].id != id {
                            let fixed = cands[0].id.as_str();
                            match set_attributes(http, opts, u, CUSTOMER_ID_ATTR, fixed).await {
                                Ok(()) => {
                                    entries.push(
                                        Entry::new("attr_updated")
                                            .for_user(u)
                                            .with_customer(id)
                                            .with_detail(format!(
                                                "{id} -> {fixed} (single customer for email)"
                                            )),
                                    );
                                    count(&mut counts, "attr_updated");
                                }
                                Err(e) => {
                                    entries.push(
                                        Entry::new("error")
                                            .for_user(u)
                                            .with_customer(id)
                                            .with_detail(format!("attribute update failed: {e}")),
                                    );
                                    count(&mut counts, "error");
                                }
                            }
                        }
                    }
                }
            }
            Some(id) => {
                // Attribute set, but no such customer anymore (or never was
                // GoCardless's): unknown to GoCardless -> leave alone.  Log
                // when it matters (the user is in the group) so operators can
                // spot stale manual/legacy entries.
                if in_group {
                    entries.push(
                        Entry::new("left_unknown")
                            .for_user(u)
                            .with_customer(id)
                            .with_detail(format!(
                                "attribute {id} matches no current GoCardless customer — left in group"
                            )),
                    );
                    count(&mut counts, "left_unknown");
                }
            }
            None => {
                // No attribute: email fallback (old-script behaviour), but
                // only when the email is unambiguous and the customer is a
                // valid member.  Manual users (email unknown to GoCardless)
                // are never touched.
                if let Some(el) = u.lower_email() {
                    if let Some(cands) = by_email.get(&el) {
                        if cands.len() == 1 {
                            let cid = cands[0].id.as_str();
                            if members.contains(cid) {
                                if let Err(e) =
                                    set_attributes(http, opts, u, CUSTOMER_ID_ATTR, cid).await
                                {
                                    entries.push(
                                        Entry::new("error")
                                            .for_user(u)
                                            .with_customer(cid)
                                            .with_detail(format!("attribute update failed: {e}")),
                                    );
                                    count(&mut counts, "error");
                                } else {
                                    entries.push(
                                        Entry::new("attr_updated")
                                            .for_user(u)
                                            .with_customer(cid)
                                            .with_detail("set from email match"),
                                    );
                                    count(&mut counts, "attr_updated");
                                }
                                if !in_group {
                                    match group_action(http, opts, &group_id, u.pk, "add_user").await {
                                        Ok(()) => {
                                            entries.push(
                                                Entry::new("add")
                                                    .for_user(u)
                                                    .with_customer(cid),
                                            );
                                            count(&mut counts, "add");
                                        }
                                        Err(e) => {
                                            entries.push(
                                                Entry::new("error")
                                                    .for_user(u)
                                                    .with_customer(cid)
                                                    .with_detail(format!("add to group failed: {e}")),
                                            );
                                            count(&mut counts, "error");
                                        }
                                    }
                                }
                            }
                        } else if in_group {
                            entries.push(
                                Entry::new("skipped_ambiguous")
                                    .for_user(u)
                                    .with_detail(format!(
                                        "{} GoCardless customers share this email — attribute left unset",
                                        cands.len()
                                    )),
                            );
                            count(&mut counts, "skipped_ambiguous");
                        }
                    }
                }
            }
        }
    }

    // --- valid members with no authentik account: create one ------------
    let mut seen_emails: HashSet<String> = HashSet::new();
    let mut seen_usernames: HashSet<String> = HashSet::new();
    for u in &users {
        if let Some(e) = u.lower_email() {
            seen_emails.insert(e);
        }
        seen_usernames.insert(u.username.to_lowercase());
    }
    for c in &data.customers {
        if !members.contains(&c.id) {
            continue;
        }
        let Some(email) = c.email.as_deref().map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty()) else {
            continue;
        };
        if seen_emails.contains(&email) {
            continue;
        }
        let name = c.display_name();
        // The username is the email local part; if that username is already
        // taken (a different account of the same person, e.g. an admin with
        // a work email) creating is guaranteed to 400 — log a skip instead
        // of an error and leave the person's account to manual handling.
        let username = email.split('@').next().unwrap_or(&email);
        if seen_usernames.contains(&username.to_lowercase()) {
            entries.push(
                Entry::new("skipped_existing_username")
                    .with_email(Some(email.clone()))
                    .with_customer(&c.id)
                    .with_detail(format!("authentik username '{username}' already taken by another account — not created")),
            );
            count(&mut counts, "skipped_existing_username");
            continue;
        }
        match create_user(http, opts, &email, &name, &c.id).await {
            Ok(pk) => match group_action(http, opts, &group_id, pk, "add_user").await {
                Ok(()) => entries.push(
                    Entry::new("user_created")
                        .with_email(Some(email.clone()))
                        .with_customer(&c.id)
                        .with_detail(format!("created authentik user '{name}', added to {}", opts.group_name)),
                ),
                Err(e) => entries.push(
                    Entry::new("error")
                        .with_email(Some(email.clone()))
                        .with_customer(&c.id)
                        .with_detail(format!("user created but group add failed: {e}")),
                ),
            },
            Err(e) => entries.push(
                Entry::new("user_create_failed")
                    .with_email(Some(email.clone()))
                    .with_customer(&c.id)
                    .with_detail(e),
            ),
        }
        count(
            &mut counts,
            if entries.last().map(|x| x.action == "user_created").unwrap_or(false) {
                "created"
            } else {
                "create_failed"
            },
        );
    }

    // --- run summary + persist the log -----------------------------------
    let c = |k: &str| counts.get(k).copied().unwrap_or(0);
    let summary = format!(
        "valid members: {} | authentik users: {} | added: {} | removed: {} | created: {} | attr updated: {} | name updated: {} | left unknown: {} | skipped: {} | errors: {}",
        members.len(),
        users.len(),
        c("add"),
        c("remove"),
        c("created"),
        c("attr_updated"),
        c("name_updated"),
        c("left_unknown"),
        c("skipped_inactive") + c("skipped_ambiguous") + c("skipped_existing_username"),
        c("error"),
    );
    entries.push(Entry {
        action: "run",
        username: None,
        email: None,
        customer_id: None,
        detail: Some(summary.clone()),
    });
    log_entries(pool, &entries).await?;
    Ok(summary)
}

/// Write audit entries (and only audit entries) to the log table.
async fn log_entries(pool: &PgPool, entries: &[Entry]) -> Result<(), String> {
    let rows: Vec<db::AkLogInsert<'_>> = entries
        .iter()
        .map(|e| db::AkLogInsert {
            action: e.action,
            username: e.username.as_deref(),
            email: e.email.as_deref(),
            customer_id: e.customer_id.as_deref(),
            detail: e.detail.as_deref(),
        })
        .collect();
    db::write_ak_log(pool, &rows).await
}

/// Log a failed run (fetch errors) as a single `run` row.
async fn log_run_error(pool: &PgPool, err: &str) -> Result<(), String> {
    log_entries(
        pool,
        &[Entry {
            action: "run",
            username: None,
            email: None,
            customer_id: None,
            detail: Some(format!("error: {err}")),
        }],
    )
    .await
}

// builder helpers (kept separate so call sites stay readable)
impl Entry {
    fn with_customer(mut self, id: &str) -> Self {
        self.customer_id = Some(id.to_string());
        self
    }
    fn with_detail(mut self, d: impl Into<String>) -> Self {
        self.detail = Some(d.into());
        self
    }
    fn with_email(mut self, e: Option<String>) -> Self {
        self.email = e;
        self
    }
}
