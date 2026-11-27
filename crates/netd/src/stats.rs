//! Counters, as atomics: the API reads them, the workers bump them, and nothing
//! waits for anything.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

#[derive(Debug, Default)]
pub struct Stats {
    pub started: AtomicI64,
    pub dns_queries: AtomicU64,
    pub dns_cache_hits: AtomicU64,
    pub dns_forwarded: AtomicU64,
    pub dns_refused: AtomicU64,
    pub dns_servfail: AtomicU64,
    pub dns_forward_errors: AtomicU64,
    pub dns_answers: AtomicU64,
    pub dhcp_rx: AtomicU64,
    pub dhcp_offers: AtomicU64,
    pub dhcp_acks: AtomicU64,
    pub dhcp_naks: AtomicU64,
    pub dhcp_ignored: AtomicU64,
    pub dhcp_declines: AtomicU64,
    pub dhcp_releases: AtomicU64,
    pub dhcp_errors: AtomicU64,
    pub ra_sent: AtomicU64,
    pub ra_solicited: AtomicU64,
    pub ra_errors: AtomicU64,
}

impl Stats {
    pub fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn started(&self) -> i64 {
        self.started.load(Ordering::Relaxed)
    }

    pub fn set_started(&self, now: i64) {
        self.started.store(now, Ordering::Relaxed);
    }

    /// Prometheus text. The names are the ones a dashboard can rely on; the
    /// `# HELP` lines are sentences because that is what `prometheus` shows on
    /// a graph's hover text.
    pub fn metrics(&self, uptime: i64) -> String {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        let mut out = String::with_capacity(2048);
        out.push_str("# HELP netd_uptime_seconds Seconds since the daemon started serving.\n");
        out.push_str("# TYPE netd_uptime_seconds gauge\n");
        out.push_str(&format!("netd_uptime_seconds {uptime}\n"));
        let series: &[(&str, &str, &AtomicU64)] = &[
            (
                "netd_dns_queries_total",
                "DNS queries received (after the ACL: a refused source is counted in netd_dns_refused_total instead).",
                &self.dns_queries,
            ),
            (
                "netd_dns_cache_hits_total",
                "Queries answered from the cache without asking an upstream.",
                &self.dns_cache_hits,
            ),
            (
                "netd_dns_forwarded_total",
                "Queries sent to an upstream resolver.",
                &self.dns_forwarded,
            ),
            (
                "netd_dns_refused_total",
                "Queries refused because the source is outside dns.acl.",
                &self.dns_refused,
            ),
            (
                "netd_dns_servfail_total",
                "Queries we could not answer and could not forward (every upstream failed, or a rebind was stopped).",
                &self.dns_servfail,
            ),
            (
                "netd_dns_forward_errors_total",
                "Individual upstream attempts that failed, including timeouts.",
                &self.dns_forward_errors,
            ),
            (
                "netd_dns_answers_total",
                "Answers returned from this box's own zone data.",
                &self.dns_answers,
            ),
            (
                "netd_dhcp_received_total",
                "DHCP packets received on every configured interface.",
                &self.dhcp_rx,
            ),
            (
                "netd_dhcp_offers_total",
                "DHCPOFFERs sent.",
                &self.dhcp_offers,
            ),
            ("netd_dhcp_acks_total", "DHCPACKs sent.", &self.dhcp_acks),
            ("netd_dhcp_naks_total", "DHCPNAKs sent.", &self.dhcp_naks),
            (
                "netd_dhcp_ignored_total",
                "Requests deliberately not answered (no pool, wpad, a relay, another server's offer, a decline).",
                &self.dhcp_ignored,
            ),
            (
                "netd_dhcp_declines_total",
                "Addresses a client reported as already in use.",
                &self.dhcp_declines,
            ),
            (
                "netd_dhcp_releases_total",
                "Leases a client gave back.",
                &self.dhcp_releases,
            ),
            (
                "netd_dhcp_errors_total",
                "Packets that could not be parsed or decoded.",
                &self.dhcp_errors,
            ),
            (
                "netd_ra_sent_total",
                "Router advertisements sent, solicited and unsolicited.",
                &self.ra_sent,
            ),
            (
                "netd_ra_solicited_total",
                "Router advertisements sent in answer to a solicitation.",
                &self.ra_solicited,
            ),
            (
                "netd_ra_errors_total",
                "Router advertisements that could not be sent (a missing device, a missing capability).",
                &self.ra_errors,
            ),
        ];
        for (name, help, counter) in series {
            out.push_str(&format!(
                "# HELP {name} {help}\n# TYPE {name} counter\n{name} {}\n",
                get(counter)
            ));
        }
        out
    }
}
