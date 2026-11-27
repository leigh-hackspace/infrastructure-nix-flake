//! What `netd check` and `netd explain` print. Kept out of `main.rs` because a
//! dispatcher should be readable in one screen, and out of `config/` because it
//! is prose for an operator rather than a rule.

use crate::config::{client_id_parse, mac_parse, Config};
use crate::dhcp::{decide, reply_options, Client, Kind};
use crate::lease::LeaseDb;
use crate::util::table;
use std::net::Ipv4Addr;

/// Everything the daemon would do, in the order an operator asks about it.
pub fn summary(cfg: &Config) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} — {} — schema v{}\n\n",
        "netd", cfg.zone, cfg.version
    ));

    let mut rows: Vec<(String, String)> = Vec::new();
    rows.push(("api".into(), format!("{}:{}", cfg.api.bind, cfg.api.port)));
    rows.push(("state".into(), cfg.state_dir.display().to_string()));
    rows.push((
        "dns".into(),
        format!(
            "{} — {} record(s), {} forwarder(s), cache {}",
            cfg.dns.listen.join(", "),
            cfg.dns.records.len(),
            cfg.dns.forwarders.len(),
            cfg.dns.cache_size
        ),
    ));
    rows.push((
        "acl".into(),
        format!(
            "{} source net(s){}",
            cfg.dns.acl.len() + cfg.dns.acl6.len(),
            if cfg.dns.domain_needed {
                ", domain-needed"
            } else {
                ""
            }
        ),
    ));
    rows.push((
        "ra".into(),
        format!(
            "every {}s on {}",
            cfg.ra.interval,
            if cfg.ra.interfaces.is_empty() {
                "nothing".to_string()
            } else {
                cfg.ra
                    .interfaces
                    .iter()
                    .map(|r| {
                        format!(
                            "{} ({} prefix(es), lifetime {}s)",
                            r.interface,
                            r.prefixes.len(),
                            r.router_lifetime
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ),
    ));
    out.push_str(&table(&rows));
    out.push('\n');

    if !cfg.dhcp.pools.is_empty() {
        out.push_str("\nDHCP pools\n");
        let rows: Vec<(String, String)> = cfg
            .dhcp
            .pools
            .iter()
            .map(|p| {
                let used = (u32::from(p.to) - u32::from(p.from) + 1) as u64;
                (
                    p.interface.clone(),
                    format!(
                        "{}–{} ({} addresses) on {}, server {}",
                        p.from,
                        p.to,
                        used,
                        p.net.cidr(),
                        p.server_ip
                    ),
                )
            })
            .collect();
        out.push_str(&table(&rows));
        out.push('\n');
    }

    out.push_str(&format!("\n{}\n", describe_netboot(cfg)));
    out.push_str(&format!(
        "\n{} reservations, {} option rule(s), {} ignored name(s), {} ignored address(es)\n",
        cfg.dhcp.reservations.len(),
        cfg.dhcp.options.len(),
        cfg.dhcp.ignored_names.len(),
        cfg.dhcp.ignored_addresses.len()
    ));
    out
}

fn describe_netboot(cfg: &Config) -> String {
    if cfg.dhcp.netboot.is_empty() {
        return "boot: no netboot variants".into();
    }
    let rows: Vec<(String, String)> = cfg
        .dhcp
        .netboot
        .iter()
        .map(|n| {
            let when = if n.when_tags.is_empty() {
                "any interface".to_string()
            } else {
                n.when_tags.join(",")
            };
            let match_what = match n.match_option {
                c if c == crate::dhcp::OPT_CLIENT_ARCH => {
                    format!(
                        "arch {}",
                        n.match_values
                            .iter()
                            .map(|v| v.to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                }
                c if c == crate::dhcp::OPT_USER_CLASS => {
                    format!(
                        "class contains {:?}",
                        n.match_contains.clone().unwrap_or_default()
                    )
                }
                c => format!("option {c}"),
            };
            (
                format!("boot {}", n.tag),
                format!("{when}: {match_what} → {} via {}", n.boot, n.server),
            )
        })
        .collect();
    table(&rows)
}

/// The pieces of a request `netd explain` can be told about.
pub struct ExplainArgs {
    pub interface: String,
    pub mac: String,
    pub client_id: Option<String>,
    pub requested: Option<String>,
    pub hostname: Option<String>,
    pub request: bool,
}

pub fn explain(
    cfg: &Config,
    state: &std::path::Path,
    args: &ExplainArgs,
) -> Result<String, String> {
    let mac = mac_parse(&args.mac)?;
    let client_id = match &args.client_id {
        Some(c) => Some(client_id_parse(c)?),
        None => None,
    };
    let requested: Option<Ipv4Addr> = match &args.requested {
        Some(r) => Some(r.parse().map_err(|_| format!("{r}: not an address"))?),
        None => None,
    };
    let (db, warning) = LeaseDb::load(state.parent().unwrap_or(state));
    let c = Client {
        mac,
        client_id,
        hostname: args.hostname.clone(),
        requested_ip: requested,
        server_id: cfg.pool_for(&args.interface).map(|p| p.server_ip),
        ciaddr: requested.unwrap_or(Ipv4Addr::UNSPECIFIED),
        msg_type: if args.request {
            crate::dhcp::MSG_REQUEST
        } else {
            crate::dhcp::MSG_DISCOVER
        },
        architectures: Vec::new(),
        user_class: None,
        max_size: 1500,
        relayed: false,
    };
    let answer = decide(cfg, &db, &args.interface, &c, crate::util::now());

    let mut out = String::new();
    out.push_str(&format!(
        "request: {} on {} as {}\n\n",
        if args.request { "REQUEST" } else { "DISCOVER" },
        args.interface,
        c.describe()
    ));
    if let Some(w) = warning {
        out.push_str(&format!("note: {w}\n\n"));
    }
    let mut rows: Vec<(String, String)> = Vec::new();
    rows.push((
        "answer".into(),
        match answer.kind {
            Kind::Offer => "OFFER".into(),
            Kind::Ack => "ACK".into(),
            Kind::Nak => "NAK".into(),
            Kind::InformAck => "ACK (inform)".into(),
            Kind::Ignore => "no reply".into(),
        },
    ));
    rows.push((
        "address".into(),
        answer
            .ip
            .map(|i| i.to_string())
            .unwrap_or_else(|| "—".into()),
    ));
    if answer.kind != Kind::Ignore {
        rows.push(("lease".into(), format!("{}s", answer.ttl)));
        rows.push((
            "tags".into(),
            answer.tags.iter().cloned().collect::<Vec<_>>().join(", "),
        ));
        if let Some(nb) = &answer.netboot {
            rows.push(("boot".into(), format!("{} via {}", nb.boot, nb.server)));
        }
    }
    rows.push(("why".into(), answer.reason.clone()));
    out.push_str(&table(&rows));

    if answer.kind != Kind::Ignore {
        out.push_str("\n\noptions in the reply\n");
        let pool = cfg.pool_for(&args.interface);
        let opts = reply_options(
            cfg,
            pool,
            &answer.tags,
            answer.ttl,
            answer.netboot.as_ref(),
            answer.kind,
        );
        let rows: Vec<(String, String)> = opts
            .iter()
            .map(|(code, value)| (option_name(*code), decode_option(*code, value)))
            .collect();
        out.push_str(&table(&rows));
    }
    out.push('\n');
    Ok(out)
}

fn option_name(code: u8) -> String {
    let name = match code {
        1 => "subnet-mask",
        3 => "router",
        6 => "dns-server",
        15 => "domain-name",
        28 => "broadcast",
        43 => "vendor-specific",
        51 => "lease-time",
        53 => "message-type",
        54 => "server-id",
        66 => "tftp-server",
        67 => "bootfile",
        119 => "domain-search",
        _ => "",
    };
    if name.is_empty() {
        format!("option {code}")
    } else {
        format!("{name} ({code})")
    }
}

fn decode_option(code: u8, v: &[u8]) -> String {
    match code {
        53 => match v.first() {
            Some(t) => crate::dhcp::msg_type_name(*t).to_string(),
            None => "—".into(),
        },
        51 => u32::from_be_bytes([
            *v.first().unwrap_or(&0),
            *v.get(1).unwrap_or(&0),
            *v.get(2).unwrap_or(&0),
            *v.get(3).unwrap_or(&0),
        ])
        .to_string(),
        1 | 3 | 6 | 54 if v.len().is_multiple_of(4) => v
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| Ipv4Addr::from(*c).to_string())
            .collect::<Vec<_>>()
            .join(", "),
        15 | 66 | 67 => String::from_utf8_lossy(v).to_string(),
        _ => v
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(":"),
    }
}

/// A DNS answer as a table, for `netd query`: the questions, then the records
/// with the address decoded where the type has one.
pub fn describe_dns(m: &crate::dns::Message) -> String {
    let mut rows: Vec<(String, String)> = Vec::new();
    for q in &m.questions {
        rows.push((
            format!("{} {}", q.name, crate::dns::type_name(q.qtype)),
            "question".into(),
        ));
    }
    for r in m.answers.iter().chain(m.authority.iter()) {
        rows.push((
            format!("{} {}", r.name, crate::dns::type_name(r.rtype)),
            format!(
                "{}s {}",
                r.ttl,
                crate::dns::rdata_address(r)
                    .map(|a| a.to_string())
                    .unwrap_or_else(|| format!("{} bytes of rdata", r.rdata.len()))
            ),
        ));
    }
    table(&rows)
}
