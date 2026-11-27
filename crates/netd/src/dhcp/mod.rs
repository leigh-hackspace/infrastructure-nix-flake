//! The DHCPv4 server: the wire format and the decision, with no socket in
//! sight.
//!
//! The split is the same one `crates/voice-assistant` uses for its device
//! boundary: everything that can be got wrong about the protocol is a pure
//! function of (config, lease table, request), and the socket code in
//! `dhcp_server.rs` only moves bytes. A test can therefore ask the whole
//! question without root, a network, or a capture file.
//!
//! ## What this replaces, and what it deliberately does not implement
//!
//! This network uses: dynamic pools per VLAN, fixed reservations keyed on a MAC
//! *or* a client-id, per-VLAN options, the UniFi vendor blob, and the Pi Room's
//! three bootfile variants. Everything else dnsmasq can do is an "ignore, and
//! say why in the journal":
//!
//!   * **relays** (`giaddr != 0`): there are none — every VLAN terminates on
//!     this box (`common/vlans.nix`), so a relayed request can only be a
//!     misconfiguration or a spoof;
//!   * **DHCPv6**: the network's IPv6 is advertised (`ra.rs`), not leased. The
//!     old config's `option6:23,[::]` meant "there is no DHCPv6 server"; a
//!     daemon that never opens the socket says the same thing without the
//!     option;
//!   * **`wpad`**: suppressed by name, as dnsmasq's `dhcp-ignore-names` did.
//!     Every Windows device asks, and answering puts a phantom device in the
//!     lease table (and, when the firewall's VLAN sets were generated from
//!     leases, in a rule).

mod decision;
mod wire;

// The module's public surface. `cargo check` does not see the tests, so the
// items only the tests read would otherwise be warnings here.
#[allow(unused_imports)]
pub use decision::{
    boot_fields, decide, in_pool, lease_from, option_bytes, reply_options, sanitise_hostname,
    Answer, Client, Effect, Kind,
};
#[allow(unused_imports)]
pub use wire::{
    describe_request, enc_domain_search, enc_ip, enc_ips, enc_str, enc_u32, msg_type_name, Message,
    Reply, BOOTREPLY, BOOTREQUEST, FLAG_BROADCAST, MSG_ACK, MSG_DECLINE, MSG_DISCOVER, MSG_INFORM,
    MSG_NAK, MSG_OFFER, MSG_RELEASE, MSG_REQUEST, OPT_BOOTFILE, OPT_BROADCAST, OPT_CLIENT_ARCH,
    OPT_CLIENT_ID, OPT_DNS, OPT_DOMAIN, OPT_END, OPT_HOSTNAME, OPT_LEASE_TIME, OPT_MAX_SIZE,
    OPT_MSG_TYPE, OPT_OVERLOAD, OPT_PAD, OPT_PARAM_REQ, OPT_REQUESTED_IP, OPT_ROUTER,
    OPT_SERVER_ID, OPT_SUBNET_MASK, OPT_TFTP_SERVER, OPT_USER_CLASS,
};

#[cfg(test)]
mod tests;
