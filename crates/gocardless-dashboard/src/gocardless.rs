//! GoCardless Pro API client (read-only): paginated listing of customers,
//! mandates, subscriptions, payments, refunds and payouts, decoded straight
//! into the shared dto types.

use gdash_dto::{Customer, Fetched, Mandate, Payment, Payout, Refund, Subscription};
use serde::de::DeserializeOwned;

/// Fetch one fully-paginated list from the GoCardless Pro API.
pub async fn list_all<T>(
    http: &reqwest::Client,
    api: &str,
    token: &str,
    path: &str,
) -> Result<Vec<T>, String>
where
    T: DeserializeOwned + Send,
{
    let mut out: Vec<T> = Vec::new();
    let mut after: Option<String> = None;
    loop {
        // List responses wrap items under the resource key ("customers",
        // "mandates", ...) and paginate with a cursor: the next page is
        // requested with ?after=<meta.cursors.after> (mirrors the official
        // SDK's Paginator).
        let url = match &after {
            Some(a) => format!("{api}/{path}?after={}", urlencoding::encode(a)),
            None => format!("{api}/{path}"),
        };
        let resp = http
            .get(&url)
            .bearer_auth(token)
            .header("accept", "application/json")
            // Required by the Pro API (the SDK sends this; without it the
            // endpoint answers 400 missing_version_header).
            .header("GoCardless-Version", "2015-07-06")
            .send()
            .await
            .map_err(|e| format!("request {url}: {e}"))?;
        let status = resp.status();
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("bad JSON from {url}: {e}"))?;
        if !status.is_success() {
            return Err(format!("GET {url}: {} {}", status.as_u16(), v));
        }
        let items: Vec<serde_json::Value> = v
            .get(path)
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        for item in items {
            out.push(serde_json::from_value(item).map_err(|e| format!("decode {path}: {e}"))?);
        }
        match v
            .pointer("/meta/cursors/after")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string())
        {
            Some(cursor) => after = Some(cursor),
            None => break,
        }
    }
    Ok(out)
}

pub async fn fetch_all(http: &reqwest::Client, api: &str, token: &str) -> Result<Fetched, String> {
    let customers = list_all::<Customer>(http, api, token, "customers").await?;
    let mandates = list_all::<Mandate>(http, api, token, "mandates").await?;
    let subscriptions = list_all::<Subscription>(http, api, token, "subscriptions").await?;
    let payments = list_all::<Payment>(http, api, token, "payments").await?;
    let refunds = list_all::<Refund>(http, api, token, "refunds").await?;
    let payouts = list_all::<Payout>(http, api, token, "payouts").await?;
    Ok(Fetched {
        customers,
        mandates,
        subscriptions,
        payments,
        refunds,
        payouts,
    })
}
