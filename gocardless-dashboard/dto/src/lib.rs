//! Shared API types for gocardless-dashboard.
//!
//! The backend decodes GoCardless Pro API responses straight into these
//! structs, persists them, and serves them (and the derived views) to the
//! Dioxus SPA, which deserialises the same types — one definition on both
//! sides of the wire.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// GoCardless Pro entity records (only the fields we care about; the full
// JSON document is what gets persisted).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Links {
    #[serde(default)]
    pub customer: Option<String>,
    #[serde(default)]
    pub subscription: Option<String>,
    #[serde(default)]
    pub mandate: Option<String>,
    #[serde(default)]
    pub payment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Customer {
    pub id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub given_name: Option<String>,
    #[serde(default)]
    pub family_name: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

impl Customer {
    /// Display name: the modern `name` field, falling back to the
    /// deprecated given/family pair.
    pub fn display_name(&self) -> String {
        if let Some(n) = &self.name {
            if !n.trim().is_empty() {
                return n.clone();
            }
        }
        format!(
            "{} {}",
            self.given_name.clone().unwrap_or_default(),
            self.family_name.clone().unwrap_or_default()
        )
        .trim()
        .to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mandate {
    pub id: String,
    #[serde(default)]
    pub links: Links,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub revoked_at: Option<String>,
}

/// A scheduled future charge of a subscription (the API only lists a few).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpcomingPayment {
    /// Date (YYYY-MM-DD) the charge will be taken.
    pub charge_date: String,
    #[serde(default)]
    pub amount: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub id: String,
    #[serde(default)]
    pub links: Links,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// Recurring charge in minor units (pence).
    #[serde(default)]
    pub amount: Option<i64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub interval: Option<i64>,
    #[serde(default)]
    pub interval_unit: Option<String>,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub end_date: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub upcoming_payments: Vec<UpcomingPayment>,
}

impl Subscription {
    /// Charge in minor units (pence), if present.
    pub fn amount_cents(&self) -> Option<i64> {
        self.amount
    }

    /// The next scheduled charge date, if the API gave one.
    pub fn next_charge_date(&self) -> Option<String> {
        self.upcoming_payments.first().map(|u| u.charge_date.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Payment {
    pub id: String,
    #[serde(default)]
    pub links: Links,
    /// Amount in minor units (pence).
    #[serde(default)]
    pub amount: Option<i64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub reference: Option<String>,
    /// Date (YYYY-MM-DD) the charge is/was taken.
    #[serde(default)]
    pub charge_date: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

impl Payment {
    /// Amount in minor units (pence), if present.
    pub fn amount_cents(&self) -> Option<i64> {
        self.amount
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Refund {
    pub id: String,
    #[serde(default)]
    pub links: Links,
    /// Amount in minor units (pence).
    #[serde(default)]
    pub amount: Option<i64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

impl Refund {
    pub fn amount_cents(&self) -> Option<i64> {
        self.amount
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Payout {
    pub id: String,
    /// Amount in minor units (pence).
    #[serde(default)]
    pub amount: Option<i64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub arrival_date: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

impl Payout {
    pub fn amount_cents(&self) -> Option<i64> {
        self.amount
    }
}

/// One full sync: every entity the GoCardless API hands back.
#[derive(Debug, Default)]
pub struct Fetched {
    pub customers: Vec<Customer>,
    pub mandates: Vec<Mandate>,
    pub subscriptions: Vec<Subscription>,
    pub payments: Vec<Payment>,
    pub refunds: Vec<Refund>,
    pub payouts: Vec<Payout>,
}

// ---------------------------------------------------------------------------
// API views served by the backend and consumed by the SPA.
// ---------------------------------------------------------------------------

/// One row of the customer table.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CustomerView {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub mandate_status: Option<String>,
    /// GoCardless mandate ids held by this customer.  GoCardless keeps
    /// several customers with the same name/email as *separate* records (see
    /// the phyushin case), so showing these ids is what tells two "identical"
    /// rows apart.
    #[serde(default)]
    pub mandate_ids: Vec<String>,
    /// True when the customer holds at least one mandate whose *own* status is
    /// active/pending_submission/submitted.  Distinct from `active`: a
    /// customer is `active` only when such a mandate also has an active
    /// subscription.  A customer can therefore have an `active` mandate yet be
    /// `stale` (an active mandate with no live subscription).
    #[serde(default)]
    pub has_active_mandate: bool,
    #[serde(default)]
    pub active_subscriptions: u32,
    #[serde(default)]
    pub sub_description: Option<String>,
    #[serde(default)]
    pub sub_charge: Option<i64>,
    #[serde(default)]
    pub sub_currency: Option<String>,
    #[serde(default)]
    pub last_payment_at: Option<String>,
    #[serde(default)]
    pub last_payment_amount: Option<i64>,
    #[serde(default)]
    pub total_paid: i64,
    #[serde(default)]
    pub payment_count: u32,
}

/// Everything for one customer (detail view).
#[derive(Debug, Serialize, Deserialize)]
pub struct CustomerDetail {
    pub customer: CustomerView,
    #[serde(default)]
    pub mandates: Vec<Mandate>,
    #[serde(default)]
    pub subscriptions: Vec<Subscription>,
    #[serde(default)]
    pub payments: Vec<Payment>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CustomerList {
    pub customers: Vec<CustomerView>,
}

/// Dashboard header stats + sync bookkeeping (`GET /api/summary`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Summary {
    #[serde(default)]
    pub customers: u32,
    #[serde(default)]
    pub active: u32,
    #[serde(default)]
    pub stale: u32,
    #[serde(default)]
    pub succeeded_30d: i64,
    #[serde(default)]
    pub failed_30d: u32,
    #[serde(default)]
    pub refunds_30d: i64,
    #[serde(default)]
    pub last_sync: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub syncing: bool,
}

/// OIDC session (`GET /api/session`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub username: String,
    #[serde(default)]
    pub email: Option<String>,
}
