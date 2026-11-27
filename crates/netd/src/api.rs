//! The HTTP API: the replacement for reading a lease file.
//!
//! ## Why this and not a file
//!
//! Everything that read `dnsmasq.leases` was reading a channel between two
//! processes: the dashboard's `/api/leases`, its names for the connection table,
//! the `lease-reader.path` grant, the five-minute `lease-reader-watch`, and the
//! gatus row that asserted the whole chain was alive. The chain existed because
//! only dnsmasq may write that file. Here the lease table *is* this process's
//! memory, so the dashboard asks a value — `GET /v1/leases` — and there is no
//! inode to be replaced under anybody's descriptor.
//!
//! ## The contract
//!
//! Documented in `docs/netd-2026-11-26.md` and versioned in the body
//! (`"version": 1`), because `network-status` is a second program in this repo
//! and a shape change has to be visible to it:
//!
//!   * `GET /v1/leases` — `{version, now, total, leases: [...]}`, one row per
//!     lease with `ip, name, mac, client_id, interface, tag, issued, expires,
//!     left, reserved`. Additive changes only in version 1.
//!   * `GET /v1/status` — what the daemon is doing: uptime, the interfaces it is
//!     attached to, the cache size, the counters, and the state of the snapshot
//!     file.
//!   * `GET /metrics` — Prometheus text.
//!   * `GET /healthz` — `ok`, for a supervisor that only wants liveness.
//!
//! ## Access control
//!
//! None here, and that is a decision inherited from `network-status`: it binds
//! the router's LAN address, the front door's LAN ACL is what admits a client,
//! and the same audience already sees the conntrack table. Binding loopback
//! would be *stricter* and is what this daemon will do if the dashboard is ever
//! local to it; until then the address is `api.bind`, which the Nix module sets
//! to the VLAN 225 address.

use crate::config::Config;
use crate::lease::LeaseDb;
use crate::stats::Stats;
use crate::util::{human_duration, now};
use common_json::{array, number_from_int, object, quoted, string};
use common_web::{read_request, Reply};

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Api {
    pub cfg: Arc<Config>,
    pub db: Arc<Mutex<LeaseDb>>,
    pub stats: Arc<Stats>,
    pub dns: Arc<crate::dns_server::Dns>,
}

impl Api {
    pub fn serve(self: Arc<Self>) -> std::io::Result<()> {
        let addr = SocketAddr::new(self.cfg.api.bind, self.cfg.api.port);
        let listener = TcpListener::bind(addr).map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("api {addr}: {e} (is another copy of netd running?)"),
            )
        })?;
        println!("netd: api on {addr}");
        loop {
            match listener.accept() {
                Ok((stream, _from)) => {
                    let me = self.clone();
                    // Nothing here blocks on a lock for long; one thread per
                    // connection with a short timeout is enough for a handful of
                    // pollers and keeps a stalled client from wedging the rest.
                    let _ = std::thread::spawn(move || me.conn(stream));
                }
                Err(e) => {
                    eprintln!("netd: api accept: {e}");
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }

    fn conn(&self, mut stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let Some(req) = read_request(&mut stream) else {
            return;
        };
        let path = req.path().to_string();
        let reply = match path.as_str() {
            "/healthz" => Reply::text(200, "ok\n"),
            "/v1/leases" => Reply::json(200, &self.leases()),
            "/v1/status" => Reply::json(200, &self.status()),
            "/metrics" => Reply::text(
                200,
                &self
                    .stats
                    .metrics(now().saturating_sub(self.stats.started())),
            ),
            // A one-line index: this API is not a UI, and every wrong path
            // should say where the right ones are rather than 404 into a blank.
            "/" => Reply::json(200, INDEX),
            _ => Reply::json(404, "{\"error\":\"no such endpoint\"}"),
        };
        let _ = reply.send(&mut stream);
    }

    pub fn leases(&self) -> String {
        let at = now();
        let (rows, saved_at, save_error) = match self.db.lock() {
            Ok(g) => {
                let rows: Vec<common_json::Json> = g.all().iter().map(|l| l.json(at)).collect();
                (rows, g.saved_at, g.save_error.clone())
            }
            Err(_) => (
                Vec::new(),
                0,
                Some("the lease table is poisoned".to_string()),
            ),
        };
        let expired = self
            .db
            .lock()
            .map(|g| g.expired_count(at) as u64)
            .unwrap_or(0);
        let mut fields = vec![
            ("version".to_string(), number_from_int(1)),
            ("now".to_string(), number_from_int(at.max(0) as u64)),
            ("total".to_string(), number_from_int(rows.len() as u64)),
            ("expired".to_string(), number_from_int(expired)),
            (
                "lease_ttl".to_string(),
                number_from_int(u64::from(self.cfg.dhcp.lease_ttl)),
            ),
            (
                "state_saved_at".to_string(),
                number_from_int(saved_at.max(0) as u64),
            ),
            (
                "state_error".to_string(),
                match &save_error {
                    Some(e) => string(e.clone()),
                    None => common_json::Json::Null,
                },
            ),
            ("leases".to_string(), array(rows)),
        ];
        fields.sort_by(|a, b| a.0.cmp(&b.0));
        emit_object(fields)
    }

    pub fn status(&self) -> String {
        let at = now();
        let (cfg_interfaces, dns_listen) = (&self.cfg.dhcp.interfaces, &self.cfg.dns.listen);
        let attached: Vec<String> = cfg_interfaces
            .iter()
            .filter(|i| crate::sys::interface_exists(i))
            .cloned()
            .collect();
        let missing: Vec<String> = cfg_interfaces
            .iter()
            .filter(|i| !crate::sys::interface_exists(i))
            .cloned()
            .collect();
        let leases = self.db.lock().map(|g| g.len()).unwrap_or(0);
        let started = self.stats.started();
        let counters = object(vec![
            (
                "dhcp_acks".to_string(),
                number_from_int(
                    self.stats
                        .dhcp_acks
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
            ),
            (
                "dhcp_errors".to_string(),
                number_from_int(
                    self.stats
                        .dhcp_errors
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
            ),
            (
                "dhcp_ignored".to_string(),
                number_from_int(
                    self.stats
                        .dhcp_ignored
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
            ),
            (
                "dhcp_naks".to_string(),
                number_from_int(
                    self.stats
                        .dhcp_naks
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
            ),
            (
                "dhcp_offers".to_string(),
                number_from_int(
                    self.stats
                        .dhcp_offers
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
            ),
            (
                "dhcp_rx".to_string(),
                number_from_int(
                    self.stats
                        .dhcp_rx
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
            ),
            (
                "dns_answers".to_string(),
                number_from_int(
                    self.stats
                        .dns_answers
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
            ),
            (
                "dns_cache_hits".to_string(),
                number_from_int(
                    self.stats
                        .dns_cache_hits
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
            ),
            (
                "dns_forwarded".to_string(),
                number_from_int(
                    self.stats
                        .dns_forwarded
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
            ),
            (
                "dns_queries".to_string(),
                number_from_int(
                    self.stats
                        .dns_queries
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
            ),
            (
                "dns_refused".to_string(),
                number_from_int(
                    self.stats
                        .dns_refused
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
            ),
            (
                "dns_servfail".to_string(),
                number_from_int(
                    self.stats
                        .dns_servfail
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
            ),
            (
                "ra_errors".to_string(),
                number_from_int(
                    self.stats
                        .ra_errors
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
            ),
            (
                "ra_sent".to_string(),
                number_from_int(
                    self.stats
                        .ra_sent
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
            ),
            (
                "ra_solicited".to_string(),
                number_from_int(
                    self.stats
                        .ra_solicited
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
            ),
        ]);
        let mut fields = vec![
            ("version".to_string(), number_from_int(1)),
            ("now".to_string(), number_from_int(at.max(0) as u64)),
            (
                "started".to_string(),
                number_from_int(started.max(0) as u64),
            ),
            (
                "uptime".to_string(),
                number_from_int(at.saturating_sub(started).max(0) as u64),
            ),
            (
                "uptime_human".to_string(),
                string(human_duration(at.saturating_sub(started))),
            ),
            ("zone".to_string(), string(self.cfg.zone.clone())),
            (
                "dhcp_interfaces".to_string(),
                array(self.cfg.dhcp.interfaces.iter().map(|i| string(i.clone()))),
            ),
            (
                "dhcp_attached".to_string(),
                array(attached.into_iter().map(string)),
            ),
            (
                "dhcp_missing".to_string(),
                array(missing.into_iter().map(string)),
            ),
            (
                "dns_listen".to_string(),
                array(dns_listen.iter().map(|l| string(l.clone()))),
            ),
            (
                "dns_cache_entries".to_string(),
                number_from_int(self.dns.cache_len() as u64),
            ),
            ("leases".to_string(), number_from_int(leases as u64)),
            (
                "api".to_string(),
                string(format!("{}:{}", self.cfg.api.bind, self.cfg.api.port)),
            ),
            ("counters".to_string(), counters),
        ];
        fields.sort_by(|a, b| a.0.cmp(&b.0));
        emit_object(fields)
    }
}

/// `common_json::emit` without the pretty printing: an API polled every five
/// seconds by a dashboard and a scrape does not need whitespace, and one line is
/// what `curl | head -c` shows usefully.
fn emit_object(pairs: Vec<(String, common_json::Json)>) -> String {
    let parts: Vec<String> = pairs
        .into_iter()
        .map(|(k, v)| format!("{}:{}", quoted(&k), compact(&v)))
        .collect();
    format!("{{{}}}", parts.join(","))
}

fn compact(v: &common_json::Json) -> String {
    use common_json::Json;
    match v {
        Json::Obj(m) => {
            let parts: Vec<String> = m
                .iter()
                .map(|(k, v)| format!("{}:{}", quoted(k), compact(v)))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Json::Arr(a) => format!("[{}]", a.iter().map(compact).collect::<Vec<_>>().join(",")),
        // Scalars: the shared emitter already writes these correctly, and
        // re-implementing number/string escaping here is how two encoders drift.
        other => common_json::emit(other, 0).trim().to_string(),
    }
}

/// The one-line index: this API is not a UI, and every wrong path should say
/// where the right ones are rather than 404 into a blank.
const INDEX: &str = "{\"service\":\"netd\",\"endpoints\":[\"/v1/leases\",\"/v1/status\",\"/metrics\",\"/healthz\"]}";

/// The address the API listens on, for the journal line and for `just netd-status`.
pub fn api_addr(cfg: &Config) -> SocketAddr {
    SocketAddr::new(cfg.api.bind, cfg.api.port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compact_encoder_agrees_with_the_shared_one_on_scalars() {
        use common_json::{boolean, number_from_int, string, Json};
        assert_eq!(compact(&string("a\"b")), "\"a\\\"b\"");
        assert_eq!(compact(&number_from_int(7)), "7");
        assert_eq!(compact(&boolean(true)), "true");
        assert_eq!(compact(&Json::Null), "null");
    }
}
