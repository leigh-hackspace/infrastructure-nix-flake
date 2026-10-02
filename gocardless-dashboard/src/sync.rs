//! The sync loop: fetch every GoCardless entity, persist a full snapshot to
//! Postgres, compute the in-memory views the API serves (active/stale
//! classification, per-customer payment stats) and publish them.
//!
//! "Active" mirrors the old active_members_to_authentik.py: a customer is
//! active when they have a mandate in active/pending_submission/submitted
//! that has at least one active subscription.  Everyone else is stale.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gdash_dto::{Customer, CustomerDetail, CustomerView, Fetched, Mandate, Payment, Subscription, Summary};

use crate::db::{self, DbData};
use crate::gocardless;
use crate::Shared;

/// Mandate statuses that count toward "active".
const ACTIVE_MANDATE_STATUSES: &[&str] = &["active", "pending_submission", "submitted"];

pub struct SyncState {
    pub last_sync: Option<String>,
    pub last_error: Option<String>,
    pub in_flight: bool,
}

pub struct Snapshot {
    pub customers: Vec<CustomerView>,
    /// customer id -> detail
    pub by_id: HashMap<String, CustomerDetail>,
    pub summary: Summary,
    pub last_sync: Option<String>,
    pub last_error: Option<String>,
}

impl Snapshot {
    pub fn empty() -> Arc<Self> {
        Arc::new(Self {
            customers: Vec::new(),
            by_id: HashMap::new(),
            summary: Summary::default(),
            last_sync: None,
            last_error: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Date handling (chrono): GoCardless timestamps are RFC3339 UTC.
// ---------------------------------------------------------------------------

fn ts_to_dt(ts: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|d| d.with_timezone(&chrono::Utc))
}

/// True when the (RFC3339) timestamp is within `window_days` of now.
fn in_window(ts: Option<&str>, window_days: i64) -> bool {
    match ts.and_then(ts_to_dt) {
        Some(dt) => dt + chrono::Duration::days(window_days) >= chrono::Utc::now(),
        None => false,
    }
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

// ---------------------------------------------------------------------------
// Snapshot construction
// ---------------------------------------------------------------------------

fn best_mandate_status(mandates: &[&Mandate]) -> Option<String> {
    let rank = |s: &str| match s {
        "active" => 0,
        "submitted" => 1,
        "pending_submission" => 2,
        "revoked" => 3,
        "declined" => 4,
        "expired" => 5,
        _ => 6,
    };
    mandates
        .iter()
        .filter_map(|m| m.status.as_ref())
        .min_by_key(|s| (rank(s), *s))
        .cloned()
}

fn build_customer_view(
    c: &Customer,
    mandates: &[&Mandate],
    subs: &[&Subscription],
    pays: &[&Payment],
    active: bool,
) -> CustomerView {
    let active_subs: Vec<&Subscription> = subs
        .iter()
        .filter(|s| s.status.as_deref() == Some("active"))
        .copied()
        .collect();
    let first_active = active_subs.first().copied();

    // GC mandate ids held by this customer (shown in the UI so that distinct
    // records that happen to share a name/email are tellable apart), and
    // whether any of them is itself active (independent of `active`).
    let mandate_ids: Vec<String> = mandates.iter().map(|m| (*m).id.clone()).collect();
    let has_active_mandate = mandates
        .iter()
        .any(|m| ACTIVE_MANDATE_STATUSES.contains(&m.status.as_deref().unwrap_or("")));

    // "Succeeded" for GoCardless Pro is `paid_out` (funds received).
    let succeeded: Vec<&Payment> = pays
        .iter()
        .filter(|p| p.status.as_deref() == Some("paid_out"))
        .copied()
        .collect();
    // No paid_at field in the API: use the charge date, falling back to the
    // creation timestamp.
    fn payment_ts(p: &Payment) -> Option<String> {
        p.charge_date.clone().or_else(|| p.created_at.clone())
    }
    let last_paid = succeeded.iter().filter_map(|p| payment_ts(*p)).max();
    let last_payment = last_paid.as_ref().and_then(|lp| {
        succeeded
            .iter()
            .find(|p| payment_ts(**p).as_deref() == Some(lp.as_str()))
            .and_then(|p| p.amount_cents())
    });
    let total_paid: i64 = succeeded
        .iter()
        .filter_map(|p| p.amount_cents())
        .sum();

    CustomerView {
        id: c.id.clone(),
        name: c.display_name(),
        email: c.email.clone(),
        created_at: c.created_at.clone(),
        active,
        mandate_status: best_mandate_status(mandates),
        mandate_ids,
        has_active_mandate,
        active_subscriptions: active_subs.len() as u32,
        sub_description: first_active.and_then(|s| s.name.clone()),
        sub_charge: first_active.and_then(|s| s.amount_cents()),
        sub_currency: first_active.and_then(|s| s.currency.clone()),
        last_payment_at: last_paid,
        last_payment_amount: last_payment,
        total_paid,
        payment_count: pays.len() as u32,
    }
}

pub fn build_snapshot(data: &DbData) -> (Arc<Snapshot>, Option<String>) {
    let mut active_customers: Vec<&String> = Vec::new();

    // mandates -> customer, and which mandate ids are "live"
    let mandates_by_customer: HashMap<String, Vec<&Mandate>> =
        db::index_by_customer(&data.mandates, |m| m.links.customer.as_deref());
    // The mandate is the customer-resolution hub: the API leaves
    // links.customer null on payments and subscriptions, so resolve those via
    // their linked mandate (see db::resolve_customer).
    let mandate_customer = db::mandate_customer_index(&data.mandates);
    let live_mandates: Vec<&Mandate> = data
        .mandates
        .iter()
        .filter(|m| ACTIVE_MANDATE_STATUSES.contains(&m.status.as_deref().unwrap_or("")))
        .collect();

    for m in &live_mandates {
        if let Some(cid) = &m.links.customer {
            // A live mandate only counts if it actually has an active
            // subscription (same rule as the old Python script).
            let has_active_sub = data.subscriptions.iter().any(|s| {
                s.links.mandate.as_deref() == Some(m.id.as_str())
                    && s.status.as_deref() == Some("active")
            });
            if has_active_sub && !active_customers.iter().any(|a| **a == **cid) {
                active_customers.push(cid);
            }
        }
    }

    // Subscriptions and payments carry no direct customer link; resolve each
    // through its mandate (100% of them have a links.mandate).
    let mut subs_by_customer: HashMap<String, Vec<&Subscription>> = HashMap::new();
    for s in &data.subscriptions {
        if let Some(c) = db::resolve_customer(&s.links, &mandate_customer) {
            subs_by_customer.entry(c).or_default().push(s);
        }
    }
    let mut pays_by_customer: HashMap<String, Vec<&Payment>> = HashMap::new();
    for p in &data.payments {
        if let Some(c) = db::resolve_customer(&p.links, &mandate_customer) {
            pays_by_customer.entry(c).or_default().push(p);
        }
    }

    let mut customers = Vec::with_capacity(data.customers.len());
    let mut by_id: HashMap<String, CustomerDetail> = HashMap::new();

    for c in &data.customers {
        let is_active = active_customers.iter().any(|a| **a == c.id);
        let view = build_customer_view(
            c,
            mandates_by_customer.get(c.id.as_str()).map(|v| v.as_slice()).unwrap_or(&[]),
            subs_by_customer.get(c.id.as_str()).map(|v| v.as_slice()).unwrap_or(&[]),
            pays_by_customer.get(c.id.as_str()).map(|v| v.as_slice()).unwrap_or(&[]),
            is_active,
        );
        let detail = CustomerDetail {
            customer: view.clone(),
            mandates: mandates_by_customer
                .get(c.id.as_str())
                .map(|v| v.iter().map(|m| (*m).clone()).collect())
                .unwrap_or_default(),
            subscriptions: subs_by_customer
                .get(c.id.as_str())
                .map(|v| v.iter().map(|s| (*s).clone()).collect())
                .unwrap_or_default(),
            payments: pays_by_customer
                .get(c.id.as_str())
                .map(|v| v.iter().map(|p| (*p).clone()).collect())
                .unwrap_or_default(),
        };
        by_id.insert(c.id.clone(), detail);
        customers.push(view);
    }
    customers.sort_by(|a, b| b.active.cmp(&a.active).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));

    let active = customers.iter().filter(|c| c.active).count();
    let succeeded_30d: i64 = data
        .payments
        .iter()
        .filter(|p| {
            p.status.as_deref() == Some("paid_out") && in_window(p.created_at.as_deref(), 30)
        })
        .filter_map(|p| p.amount_cents())
        .sum();
    let failed_30d: u32 = data
        .payments
        .iter()
        .filter(|p| {
            p.status.as_deref() == Some("failed") && in_window(p.created_at.as_deref(), 30)
        })
        .count() as u32;
    let refunds_30d: i64 = data
        .refunds
        .iter()
        .filter(|r| {
            r.status.as_deref() == Some("succeeded") && in_window(r.created_at.as_deref(), 30)
        })
        .filter_map(|r| r.amount_cents())
        .sum();

    let total = customers.len() as u32;
    (
        Arc::new(Snapshot {
            customers,
            by_id,
            summary: Summary {
                customers: total,
                active: active as u32,
                stale: total - active as u32,
                succeeded_30d,
                failed_30d,
                refunds_30d,
                last_sync: data.last_sync.clone(),
                last_error: None,
                syncing: false,
            },
            last_sync: data.last_sync.clone(),
            last_error: None,
        }),
        data.last_sync.clone(),
    )
}

// ---------------------------------------------------------------------------
// The sync itself + loop
// ---------------------------------------------------------------------------

async fn run_sync_inner(shared: &Shared) -> Result<String, String> {
    let fetched: Fetched =
        gocardless::fetch_all(&shared.http, &shared.cfg.gocardless_api, &shared.cfg.gocardless_token).await?;

    let now = now_rfc3339();
    db::write_all(&shared.pool, &fetched, &now).await?;

    // Rebuild the snapshot from what we just wrote (load_all gives us the
    // typed data plus last_sync from meta).
    let data = db::load_all(&shared.pool).await?;
    let snap = build_snapshot(&data).0;
    let last_sync = data.last_sync.clone();

    *shared.snapshot.write().unwrap() = snap;
    let mut st = shared.sync_state.lock().unwrap();
    st.last_sync = last_sync;
    st.last_error = None;
    Ok(now)
}

/// Spawn-friendly wrapper: takes ownership of the Arc so the returned
/// future is 'static (no lifetime tied to a caller's local).
pub fn run_sync_owned(shared: Arc<Shared>) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(async move { run_sync(&shared).await })
}

pub async fn run_sync(shared: &Shared) {
    {
        let mut st = shared.sync_state.lock().unwrap();
        if st.in_flight {
            return;
        }
        st.in_flight = true;
    }
    match run_sync_inner(shared).await {
        Ok(ts) => eprintln!("sync ok at {ts}"),
        Err(e) => {
            eprintln!("sync failed: {e}");
            shared.sync_state.lock().unwrap().last_error = Some(e);
        }
    }
    shared.sync_state.lock().unwrap().in_flight = false;
}

pub fn loop_sync(shared: Arc<Shared>) {
    let interval = Duration::from_secs(shared.cfg.sync_interval);
    // The GoCardless sync loop runs on its own thread (started from the
    // async main) with its own single-threaded runtime; the shared handle in
    // Shared belongs to the main runtime and is used by the HTTP handlers.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build sync loop runtime");
    loop {
        rt.block_on(run_sync(&shared));
        std::thread::sleep(interval);
    }
}
