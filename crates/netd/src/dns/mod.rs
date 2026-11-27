//! The resolver: a zone it owns, a cache, and a forwarder. No socket code here
//! — `dns_server.rs` owns the listeners.
//!
//! ## What it answers, and what it refuses
//!
//! The old dnsmasq configuration is the specification
//! (`machines/gw/dns.nix`), and every directive in it maps to one of four
//! behaviours:
//!
//!   * **authoritative for `int.leighhack.org`** — `domain=`,
//!     `local=/int.leighhack.org/`, two `addn-hosts` files and `dhcp-fqdn`. Here
//!     it is `dns/zone.rs` over the config's records *and the live lease table*;
//!   * **forwarding** to four upstreams, `strict-order` (the first that answers
//!     wins, in the order the table lists them), plus a conditional forwarder for
//!     `ts.leighhack.org` so the tailnet's names never leak to a public resolver;
//!   * **refusals** — `local-service`, `domain-needed` (a name with no dot is
//!     never forwarded), `stop-dns-rebind` and `rebind-localhost-ok` with an
//!     exception for `ts.leighhack.org`;
//!   * **a cache** — `cache-size=10000`, `local-ttl=1`.
//!
//! The pieces dnsmasq uses that this deliberately does not: DNS-over-TLS, DNSSEC
//! validation (the upstreams do none of it for us either — `edns0 trust-ad` in
//! the box's own `resolv.conf` is about the stub, not validation), and
//! `dns-forward-max`'s queue. That last one is replaced by a concurrency cap
//! that *refuses* rather than queues (`dns_server.rs`), because a queue is how a
//! resolver turns one slow upstream into a box that answers nothing.

pub mod wire;
pub mod zone;

// The module's public surface. `cargo check` does not see the tests, so the
// items only the tests read would otherwise be warnings here.
#[allow(unused_imports)]
pub use wire::{
    a_record, aaaa_record, ns_record, ptr_record, rdata_address, reverse_v4, reverse_v6,
    soa_record, type_name, Edns, Message, Question, Record, CLASS_IN, FLAG_AA, FLAG_QR, FLAG_RA,
    FLAG_RD, FLAG_TC, RCODE_FORMERR, RCODE_NOERROR, RCODE_NOTIMP, RCODE_NXDOMAIN, RCODE_REFUSED,
    RCODE_SERVFAIL, TYPE_A, TYPE_AAAA, TYPE_ANY, TYPE_NS, TYPE_OPT, TYPE_PTR, TYPE_SOA,
};
#[allow(unused_imports)]
pub use zone::{answer, ZoneAnswer};

#[cfg(test)]
mod tests;
