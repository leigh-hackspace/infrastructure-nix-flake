//! The decision: given one request, what is the answer and why?
//!
//! Everything that can be got wrong about DHCP — an offer for an address
//! somebody else holds, a reservation that loses to a pool, a NAK that should
//! have been an ACK or an ACK that should have been silence — is decided here
//! as a pure function of (config, lease table, request). The socket code only
//! moves bytes.

use super::wire::*;
use crate::config::{
    client_id_hex, ClientId, Config, DhcpOption, Mac, Netboot, OptionValue, Pool, Reservation,
};
use crate::lease::{Lease, LeaseDb};
use std::collections::BTreeSet;
use std::net::Ipv4Addr;

// ---------------------------------------------------------------------------
// the request, as a value
// ---------------------------------------------------------------------------

/// What a request says about itself, extracted once. The socket code never
/// looks at a raw option again.
#[derive(Clone, Debug)]
pub struct Client {
    pub mac: Mac,
    pub client_id: Option<ClientId>,
    pub hostname: Option<String>,
    pub requested_ip: Option<Ipv4Addr>,
    pub server_id: Option<Ipv4Addr>,
    pub ciaddr: Ipv4Addr,
    pub msg_type: u8,
    pub architectures: Vec<u16>,
    pub user_class: Option<String>,
    pub max_size: u16,
    /// `giaddr != 0`: this came through a relay agent. There are none on this
    /// network (every VLAN terminates on the router), so a relayed request can
    /// only be a misconfiguration or a spoof.
    pub relayed: bool,
}

impl Client {
    pub fn read(req: &Message, msg_type: u8) -> Result<Client, String> {
        let mut architectures = Vec::new();
        for v in req.opts(OPT_CLIENT_ARCH) {
            for pair in v.as_chunks::<2>().0 {
                architectures.push(u16::from_be_bytes([pair[0], pair[1]]));
            }
        }
        Ok(Client {
            mac: req.mac()?,
            client_id: req.opt(OPT_CLIENT_ID).map(|v| v.to_vec()),
            hostname: req
                .opt_str(OPT_HOSTNAME)
                .map(|h| sanitise_hostname(&h))
                .filter(|h| !h.is_empty()),
            requested_ip: req.opt_ip(OPT_REQUESTED_IP),
            server_id: req.opt_ip(OPT_SERVER_ID),
            ciaddr: req.ciaddr,
            msg_type,
            architectures,
            user_class: req.opt_str(OPT_USER_CLASS),
            max_size: req.opt_u16(OPT_MAX_SIZE).unwrap_or(576).max(300),
            relayed: req.giaddr != Ipv4Addr::UNSPECIFIED,
        })
    }

    pub fn describe(&self) -> String {
        format!(
            "{} (client-id {}){}",
            crate::config::mac_to_string(&self.mac),
            self.client_id
                .as_ref()
                .map(client_id_hex)
                .unwrap_or_else(|| "none".into()),
            self.hostname
                .as_ref()
                .map(|h| format!(" hostname {h}"))
                .unwrap_or_default(),
        )
    }
}

/// A DHCP hostname is a DNS label or nothing. Clients do send spaces, quotes,
/// underscores and whole sentences, and `dhcp-fqdn` puts this into the
/// resolver's zone — so the filter belongs here rather than in the DNS code.
pub fn sanitise_hostname(raw: &str) -> String {
    let first = raw.split('.').next().unwrap_or("");
    let cleaned: String = first
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    cleaned.trim_matches('-').to_string()
}

// ---------------------------------------------------------------------------
// the answer
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Offer,
    Ack,
    Nak,
    InformAck,
    Ignore,
}

impl Kind {
    pub fn msg_type(self) -> Option<u8> {
        match self {
            Kind::Offer => Some(MSG_OFFER),
            Kind::Ack | Kind::InformAck => Some(MSG_ACK),
            Kind::Nak => Some(MSG_NAK),
            Kind::Ignore => None,
        }
    }
}

/// Something the caller must do to the lease table. Kept as its own field
/// rather than inferred from the message type, so a caller cannot forget that a
/// DECLINE changes state while an Ignore does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    Release,
    Decline,
}

/// The whole answer to one request: what to say, to whom, with which options,
/// and — the part an operator needs — *why*. `reason` is what `netd explain`
/// prints and what the journal line carries, so the daemon never makes a
/// decision a person cannot read back.
#[derive(Clone, Debug)]
pub struct Answer {
    pub kind: Kind,
    pub effect: Effect,
    pub ip: Option<Ipv4Addr>,
    pub ttl: u32,
    pub tags: BTreeSet<String>,
    pub reserved: bool,
    pub netboot: Option<Netboot>,
    pub reason: String,
}

impl Answer {
    pub fn ignore(reason: impl Into<String>) -> Answer {
        Answer {
            kind: Kind::Ignore,
            effect: Effect::None,
            ip: None,
            ttl: 0,
            tags: BTreeSet::new(),
            reserved: false,
            netboot: None,
            reason: reason.into(),
        }
    }

    fn with_effect(mut self, effect: Effect) -> Answer {
        self.effect = effect;
        self
    }

    fn with_ip(mut self, ip: Option<Ipv4Addr>) -> Answer {
        self.ip = ip;
        self
    }
}

/// Decide what to do about one request.
///
/// `interface` is the device the request arrived on, which is how a request
/// carries a VLAN at all: there is no VLAN tag in a DHCP packet, and dnsmasq
/// derived its tags from the interface name exactly as this does
/// (`machines/gw/lib/network-tables.nix`'s `matchTag`).
pub fn decide(cfg: &Config, db: &LeaseDb, interface: &str, c: &Client, now: i64) -> Answer {
    if c.relayed {
        return Answer::ignore(
            "relayed request (giaddr set): every VLAN terminates on this box, so a relay is a misconfiguration and the answer would be for the wrong network",
        );
    }
    if c.msg_type == 0 {
        return Answer::ignore(
            "no message type (option 53) — plain BOOTP, which this server does not answer",
        );
    }
    if !c.ciaddr.is_unspecified()
        && !matches!(
            c.msg_type,
            MSG_REQUEST | MSG_RELEASE | MSG_DECLINE | MSG_INFORM
        )
    {
        return Answer::ignore(format!(
            "ciaddr {} is set on a {} — that field is only legal on REQUEST, RELEASE, DECLINE and INFORM",
            c.ciaddr,
            msg_type_name(c.msg_type)
        ));
    }
    if let Some(h) = &c.hostname {
        if cfg.ignored_name(h) {
            return Answer::ignore(format!(
                "hostname `{h}` is in dhcp.ignored_names — a device asking to be called wpad gets no lease, which is what dnsmasq's dhcp-ignore-names did here"
            ));
        }
    }

    let pool = cfg.pool_for(interface);
    let reservation = cfg.reservation_for(&c.mac, c.client_id.as_ref());
    let mut tags = cfg.tags_for(interface);
    let netboot = match_netboot(cfg, interface, c, &mut tags);

    // A release or a decline is the client correcting our table; it is not a
    // licence request, so it never goes through the offer/ack logic. The address
    // still travels in `ip`, because the caller has to act on it — a DECLINE with
    // no address to quarantine is a declined address nobody can avoid handing
    // out again.
    match c.msg_type {
        MSG_RELEASE => {
            let ip = (!c.ciaddr.is_unspecified()).then_some(c.ciaddr);
            return Answer::ignore(match ip {
                Some(ip) => format!("RELEASE of {ip}"),
                None => "RELEASE with no ciaddr".to_string(),
            })
            .with_ip(ip)
            .with_effect(Effect::Release);
        }
        MSG_DECLINE => {
            return Answer::ignore(match c.requested_ip {
                Some(ip) => format!("DECLINE of {ip}"),
                None => "DECLINE with no address".to_string(),
            })
            .with_ip(c.requested_ip)
            .with_effect(Effect::Decline);
        }
        _ => {}
    }

    match reservation {
        Some(r) => reserved(cfg, c, pool, r, tags, netboot),
        None => pooled(cfg, db, interface, pool, c, now, tags, netboot),
    }
}

fn reserved(
    cfg: &Config,
    c: &Client,
    pool: Option<&Pool>,
    r: &Reservation,
    tags: BTreeSet<String>,
    netboot: Option<Netboot>,
) -> Answer {
    if cfg.dhcp.ignored_addresses.contains(&r.address) {
        return Answer::ignore(format!(
            "`{}` is reserved at {}, which is also in ignored_addresses — the table contradicts itself",
            r.name, r.address
        ));
    }
    let ttl = cfg.dhcp.lease_ttl;
    let base = |kind: Kind, ip: Option<Ipv4Addr>, reason: String| Answer {
        kind,
        effect: Effect::None,
        ip,
        ttl,
        tags: tags.clone(),
        reserved: true,
        netboot: netboot.clone(),
        reason,
    };
    match c.msg_type {
        MSG_DISCOVER => base(
            Kind::Offer,
            Some(r.address),
            format!("fixed reservation `{}` holds {}", r.name, r.address),
        ),
        MSG_REQUEST => {
            // A client naming another server is not ours to answer (RFC 2131
            // §4.3.2); a client asking *us* for the wrong address can act on a
            // NAK, so it gets one.
            if let (Some(sid), Some(p)) = (c.server_id, pool) {
                if sid != p.server_ip {
                    return Answer::ignore(format!(
                        "REQUEST names server {sid}, and this interface is {}",
                        p.server_ip
                    ));
                }
            }
            let asked = c
                .requested_ip
                .or((!c.ciaddr.is_unspecified()).then_some(c.ciaddr));
            match asked {
                Some(a) if a != r.address => base(
                    Kind::Nak,
                    None,
                    format!(
                        "REQUEST for {a}, but `{}` is reserved at {}",
                        r.name, r.address
                    ),
                ),
                _ => base(
                    Kind::Ack,
                    Some(r.address),
                    format!("fixed reservation `{}` at {}", r.name, r.address),
                ),
            }
        }
        MSG_INFORM => base(Kind::InformAck, None, format!("INFORM from `{}`", r.name)),
        other => Answer::ignore(format!("{} from a reserved client", msg_type_name(other))),
    }
}

#[allow(clippy::too_many_arguments)] // config, table, interface, pool, client, clock,
                                     // tags, boot — the whole decision.s inputs
fn pooled(
    cfg: &Config,
    db: &LeaseDb,
    interface: &str,
    pool: Option<&Pool>,
    c: &Client,
    now: i64,
    tags: BTreeSet<String>,
    netboot: Option<Netboot>,
) -> Answer {
    let Some(pool) = pool else {
        return Answer::ignore(format!(
            "no dynamic pool on `{interface}` — this interface hands out nothing (a VLAN with no dynFrom/dynTo in common/vlans.nix exists to have no pool)",
        ));
    };
    let base = |kind: Kind, ip: Option<Ipv4Addr>, reason: String| Answer {
        kind,
        effect: Effect::None,
        ip,
        ttl: pool.ttl,
        tags: tags.clone(),
        reserved: false,
        netboot: netboot.clone(),
        reason,
    };
    match c.msg_type {
        MSG_INFORM => base(
            Kind::InformAck,
            None,
            format!(
                "INFORM on {}/{} — options only, no address",
                pool.server_ip, pool.net.prefix
            ),
        ),
        MSG_DISCOVER => match choose(cfg, db, pool, c, now) {
            Ok((ip, why)) => base(Kind::Offer, Some(ip), why),
            Err(why) => Answer::ignore(why),
        },
        MSG_REQUEST => {
            if let Some(sid) = c.server_id {
                if sid != pool.server_ip {
                    return Answer::ignore(format!(
                        "REQUEST is for server {sid}, and this interface is {} — the client picked another offer, so silence is the only correct reply",
                        pool.server_ip
                    ));
                }
            }
            let asked = c
                .requested_ip
                .or((!c.ciaddr.is_unspecified()).then_some(c.ciaddr));
            let Some(asked) = asked else {
                return Answer::ignore("REQUEST with no requested address and no ciaddr");
            };
            if !is_usable(cfg, pool, asked) {
                return base(
                    Kind::Nak,
                    None,
                    format!(
                        "{asked} is not a usable address on {}/{} (outside the pool, reserved, ignored, or the network/broadcast address)",
                        pool.server_ip, pool.net.prefix
                    ),
                );
            }
            if db.held_by_other(asked, &c.mac, c.client_id.as_ref()) {
                let holder = db
                    .by_ip(asked)
                    .map(|l| crate::config::mac_to_string(&l.mac))
                    .unwrap_or_default();
                return base(Kind::Nak, None, format!("{asked} is held by {holder}"));
            }
            base(
                Kind::Ack,
                Some(asked),
                format!(
                    "{asked} confirmed on {}/{}",
                    pool.server_ip, pool.net.prefix
                ),
            )
        }
        other => Answer::ignore(format!("{} on a pooled interface", msg_type_name(other))),
    }
}

/// Is this address one the pool may hand out at all? Not "is it free" — that is
/// the lease table's question, and a client renewing its own address must not
/// be asked to prove it.
fn is_usable(cfg: &Config, pool: &Pool, ip: Ipv4Addr) -> bool {
    in_pool(pool, ip)
        && !cfg.dhcp.ignored_addresses.contains(&ip)
        && cfg.reservation_at(ip).is_none()
        && ip != pool.net.addr
        && ip != pool.net.broadcast()
}

pub fn in_pool(pool: &Pool, ip: Ipv4Addr) -> bool {
    u32::from(ip) >= u32::from(pool.from) && u32::from(ip) <= u32::from(pool.to)
}

/// Pick an address for a client, and say how it was picked.
///
/// The order is the RFC's preference order as this network uses it: keep what
/// you had, then what you asked for, then the first free one. Keeping what you
/// had first is not politeness — a client whose lease lapsed while it was
/// asleep gets the same address back, so nothing else in the network has to
/// notice it was away.
fn choose(
    cfg: &Config,
    db: &LeaseDb,
    pool: &Pool,
    c: &Client,
    now: i64,
) -> Result<(Ipv4Addr, String), String> {
    let free = |ip: Ipv4Addr| -> bool {
        is_usable(cfg, pool, ip)
            && !db.held_by_other(ip, &c.mac, c.client_id.as_ref())
            && !db.is_declined(ip, now)
    };
    if let Some(l) = db.by_client(&c.mac, c.client_id.as_ref()) {
        if in_pool(pool, l.ip) && l.expires > now && free(l.ip) {
            return Ok((
                l.ip,
                format!("{} is this client's own unexpired lease", l.ip),
            ));
        }
    }
    if let Some(req) = c.requested_ip {
        if free(req) {
            return Ok((req, format!("{req} is the address the client asked for")));
        }
    }
    // Deterministic on purpose: a round-robin cursor makes "why did this device
    // get .173" unanswerable from the config alone, and scanning 104 addresses
    // costs nothing.
    let mut ip = u32::from(pool.from);
    let end = u32::from(pool.to);
    while ip <= end {
        let candidate = Ipv4Addr::from(ip);
        if free(candidate) {
            return Ok((
                candidate,
                format!(
                    "{candidate} is the first free address in {}-{}",
                    pool.from, pool.to
                ),
            ));
        }
        ip += 1;
    }
    Err(format!(
        "no free address in {}-{} on {}: the pool is exhausted",
        pool.from, pool.to, pool.interface
    ))
}

/// Netboot variant matching (dnsmasq's `dhcp-match`). The **last** match wins,
/// which is how dnsmasq applies repeated `dhcp-boot` lines and why the Pi
/// Room's `netboot-autoexec` (option 77) beats its `netboot-efi` (option 93):
/// an iPXE binary asks for its script, not for another loader.
fn match_netboot(
    cfg: &Config,
    interface: &str,
    c: &Client,
    tags: &mut BTreeSet<String>,
) -> Option<Netboot> {
    let mut chosen = None;
    for nb in &cfg.dhcp.netboot {
        if !nb.when_tags.is_empty() && !nb.when_tags.iter().any(|t| t == interface) {
            continue;
        }
        let hit = match nb.match_option {
            OPT_CLIENT_ARCH => c.architectures.iter().any(|a| nb.match_values.contains(a)),
            OPT_USER_CLASS => match (&nb.match_contains, &c.user_class) {
                (Some(needle), Some(class)) => class.contains(needle.as_str()),
                _ => false,
            },
            _ => false,
        };
        if hit {
            // dnsmasq's `set:<tag>` half: the request carries the tag from here
            // on, which is what makes the boot options apply to it and nothing
            // else (`netd explain` shows the tag set for this reason).
            tags.insert(nb.tag.clone());
            chosen = Some(nb.clone());
        }
    }
    chosen
}

// ---------------------------------------------------------------------------
// the options a reply carries
// ---------------------------------------------------------------------------

/// RFC 2131 defaults (the ones dnsmasq derived from the interface this request
/// arrived on), then the config, with the config winning a collision.
///
/// The defaults are not invented: they are what the running dnsmasq sends here,
/// reconstructed from its configuration (`machines/gw/dns.nix`: no
/// `dhcp-option=3`, so the router is the interface's own address; `domain=`
/// supplies option 15) and confirmed against its lease file on 2026-11-27.
pub fn reply_options(
    cfg: &Config,
    pool: Option<&Pool>,
    tags: &BTreeSet<String>,
    ttl: u32,
    netboot: Option<&Netboot>,
    kind: Kind,
) -> Vec<(u8, Vec<u8>)> {
    let mut opts: Vec<(u8, Vec<u8>)> = Vec::new();
    fn set(opts: &mut Vec<(u8, Vec<u8>)>, code: u8, value: Vec<u8>) {
        opts.retain(|(c, _)| *c != code);
        opts.push((code, value));
    }
    if let Some(t) = kind.msg_type() {
        set(&mut opts, OPT_MSG_TYPE, vec![t]);
    }
    if let Some(p) = pool {
        set(&mut opts, OPT_SERVER_ID, enc_ip(p.server_ip));
        if matches!(kind, Kind::Offer | Kind::Ack) {
            set(&mut opts, OPT_LEASE_TIME, enc_u32(ttl));
            set(&mut opts, OPT_SUBNET_MASK, enc_ip(p.net.mask()));
            set(&mut opts, OPT_BROADCAST, enc_ip(p.net.broadcast()));
            set(&mut opts, OPT_ROUTER, enc_ip(p.server_ip));
            set(&mut opts, OPT_DNS, enc_ip(p.server_ip));
            set(&mut opts, OPT_DOMAIN, enc_str(&cfg.zone));
        }
    }
    if kind == Kind::Nak {
        // A NAK carries nothing but the reason, and clients ignore options on
        // one anyway.
        return opts;
    }
    for o in &cfg.dhcp.options {
        let applies = match &o.tag {
            None => true,
            Some(t) => tags.contains(t),
        };
        if applies {
            set(&mut opts, o.code, option_bytes(o));
        }
    }
    if let Some(nb) = netboot {
        set(&mut opts, OPT_BOOTFILE, enc_str(&nb.boot));
        // The TFTP server name is meaningful for a file; for an HTTP boot the
        // URL is the bootfile and `siaddr` is what older ROMs use to find
        // anything at all, so it is answered with the table's server either way.
        set(&mut opts, OPT_TFTP_SERVER, enc_str(&nb.server.to_string()));
    }
    opts
}

/// One configured option's bytes.
pub fn option_bytes(o: &DhcpOption) -> Vec<u8> {
    match &o.value {
        OptionValue::Ipv4(list) => enc_ips(list),
        OptionValue::U32(v) => enc_u32(*v),
        OptionValue::Text(s) if o.code == 119 => enc_domain_search(
            &s.split(',')
                .map(|x| x.trim().to_string())
                .collect::<Vec<_>>(),
        ),
        OptionValue::Text(s) => enc_str(s),
        OptionValue::Hex(bytes) => bytes.clone(),
    }
}

/// The `siaddr`/`file` fields of a reply, which is where a PXE ROM looks before
/// it looks at options 66/67.
pub fn boot_fields(netboot: Option<&Netboot>) -> (Ipv4Addr, Option<String>) {
    match netboot {
        Some(nb) => (nb.server, Some(nb.boot.clone())),
        None => (Ipv4Addr::UNSPECIFIED, None),
    }
}

/// The lease row an ACK creates. `None` for anything that did not promise an
/// address (NAK, INFORM, ignore), so a caller cannot record a lease it did not
/// make.
pub fn lease_from(answer: &Answer, c: &Client, interface: &str, now: i64) -> Option<Lease> {
    // An offer is not a promise: the row is written when the client asks for it
    // (RFC 2131 — the server commits on REQUEST). A NAK, an INFORM and an
    // ignore promise nothing at all.
    if answer.kind != Kind::Ack {
        return None;
    }
    let ip = answer.ip?;
    Some(Lease {
        ip,
        mac: c.mac,
        client_id: c.client_id.clone(),
        hostname: c.hostname.clone(),
        interface: interface.to_string(),
        tag: answer
            .tags
            .iter()
            .find(|t| t.as_str() != interface)
            .or_else(|| answer.tags.iter().next())
            .cloned()
            .unwrap_or_else(|| interface.to_string()),
        issued: now,
        expires: now + i64::from(answer.ttl),
        reserved: answer.reserved,
    })
}
