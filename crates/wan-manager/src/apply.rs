//! Writing the routing and NAT state the daemon has decided on.
//!
//! Every operation is idempotent and re-applied every tick. That is not
//! belt-and-braces: a route can be removed by hand, an nftables reload deletes
//! runtime set elements, and a `nixos-rebuild switch` reloads the whole ruleset.
//! The daemon is the thing that notices and puts its half back.
//!
//! NAT is keyed on an **interface set** (`ip routerNat wan_ifaces`) rather than
//! on named interfaces, which is what makes the flake contain no WAN name at
//! all. The set is declared in `machines/gw/firewall.nix`; this module fills it
//! with whatever discovery found.

use crate::config::Config;
use crate::net::Kind;
use crate::rank::{Candidate, Selection};
use crate::util::{run, route_dev};

const SET: &str = "wan_ifaces";

fn ip(args: &[&str]) -> bool {
    run(
        "ip",
        &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        5,
    )
    .map(|(c, _, _)| c == 0)
    .unwrap_or(false)
}

fn nft(args: &[&str]) -> bool {
    run(
        "nft",
        &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        5,
    )
    .map(|(c, _, _)| c == 0)
    .unwrap_or(false)
}

fn nft_out(args: &[&str]) -> Option<String> {
    run(
        "nft",
        &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        5,
    )
    .and_then(|(c, out, _)| if c == 0 { Some(out) } else { None })
}

pub fn ensure_forwarding() -> Vec<String> {
    let mut out = Vec::new();
    let path = "/proc/sys/net/ipv4/ip_forward";
    if crate::util::read_trim(path).as_deref() != Some("1") {
        if std::fs::write(path, "1\n").is_ok() {
            out.push("enabled net.ipv4.ip_forward".into());
        }
    }
    out
}

/// The two rules `machines/gw/networking.nix` declares. Re-adding them is how a
/// `switch` that fails to load networkd's files does not take the policy with
/// it; the declarative copy is still the source of truth when it is there.
pub fn ensure_policy_rules(cfg: &Config) -> Vec<String> {
    let mut out = Vec::new();
    let Some(text) = run(
        "ip",
        &["rule".to_string(), "show".to_string()],
        5,
    )
    .map(|(_, o, _)| o) else {
        return out;
    };
    let has = |mark: u32, table: u32| {
        text.lines().any(|l| {
            l.contains(&format!("fwmark 0x{mark:x}"))
                && l.contains(&format!("lookup {table}"))
        })
    };
    if !has(1, cfg.tunnel_table) {
        if ip(&[
            "rule",
            "add",
            "pref",
            "100",
            "fwmark",
            "1",
            "lookup",
            &cfg.tunnel_table.to_string(),
        ]) {
            out.push("added fwmark 1 -> table 100".into());
        }
    }
    if !has(2, cfg.hub_table) {
        if ip(&[
            "rule",
            "add",
            "pref",
            "200",
            "fwmark",
            "2",
            "lookup",
            &cfg.hub_table.to_string(),
        ]) {
            out.push("added fwmark 2 -> table 200".into());
        }
    }
    out
}

/// Remove only the routes this daemon installs. The devices' own DHCP defaults
/// (higher metrics, `proto dhcp`) and pppd's table-100 route are somebody else's
/// inputs and must survive a clear.
fn clear_ours(cfg: &Config, candidates: &[Candidate]) -> Vec<String> {
    let mut out = Vec::new();
    let metric = cfg.fallback_metric.to_string();
    let l2tp_metric = cfg.l2tp_metric.to_string();
    let table = cfg.hub_table.to_string();
    for c in candidates {
        if c.info.kind == Kind::Tunnel {
            continue;
        }
        let dev = c.info.name.clone();
        let _ = ip(&["-4", "route", "del", "default", "dev", &dev, "metric", &metric]);
        let _ = ip(&[
            "-4", "route", "del", "default", "dev", &dev, "table", &table, "metric", &metric,
        ]);
        let _ = ip(&[
            "-4",
            "route",
            "del",
            &format!("{}/32", cfg.l2tp_endpoint),
            "dev",
            &dev,
            "metric",
            &l2tp_metric,
        ]);
    }
    // Also remove an endpoint route that the candidate list no longer names.
    let _ = ip(&[
        "-4",
        "route",
        "del",
        &format!("{}/32", cfg.l2tp_endpoint),
        "metric",
        &l2tp_metric,
    ]);
    out.push("cleared daemon routes".into());
    out
}

fn route_default(dev: &str, metric: u32, table: Option<u32>) -> bool {
    let m = metric.to_string();
    match table {
        Some(t) => ip(&[
            "-4", "route", "replace", "default", "dev", dev, "metric", &m, "table", &t.to_string(),
        ]),
        None => ip(&["-4", "route", "replace", "default", "dev", dev, "metric", &m]),
    }
}

fn route_via(gw: &str, dev: &str, metric: u32, table: Option<u32>) -> bool {
    let m = metric.to_string();
    match table {
        Some(t) => ip(&[
            "-4", "route", "replace", "default", "via", gw, "dev", dev, "metric", &m, "table",
            &t.to_string(),
        ]),
        None => ip(&[
            "-4", "route", "replace", "default", "via", gw, "dev", dev, "metric", &m,
        ]),
    }
}

/// The whole routing decision, in an order that never leaves the box with no
/// default while it is rewriting one.
pub fn apply_routes(cfg: &Config, candidates: &[Candidate], sel: &Selection) -> Vec<String> {
    let mut actions = Vec::new();
    // Never remove a working default just because this tick has not found a
    // non-tunnel replacement yet. On a switch the old path is still carrying
    // the network while the daemon makes its first, slower discovery pass, and
    // on a handback `networkd` may briefly drop the DHCP route. Clearing first
    // is how the first deploy took the LAN down even though the box had a
    // perfectly good route. The tunnel-only case is included: verifying table
    // 100 is useful, but it must not delete the main/table-200 defaults that
    // are still carrying traffic.
    if sel.best.is_some() {
        actions.extend(clear_ours(cfg, candidates));
    } else {
        actions.push("no non-tunnel egress yet; leaving existing default routes untouched".into());
    }
    actions.extend(ensure_policy_rules(cfg));
    actions.extend(ensure_forwarding());

    // The tunnel's table is pppd's, but pppd's ip-up can have been skipped by a
    // crash between IPCP and the script. Verify it, and remove it when the
    // tunnel is present but not viable so VLAN 225 falls through to the working
    // path rather than into a dead ppp0.
    let tunnel_route = cfg.tunnel_table;
    match sel.tunnel {
        Some(i) => {
            let dev = candidates[i].info.name.as_str();
            let present = run(
                "ip",
                &[
                    "-4".to_string(),
                    "route".to_string(),
                    "show".to_string(),
                    "table".to_string(),
                    tunnel_route.to_string(),
                    "default".to_string(),
                ],
                5,
            )
            .map(|(c, o, _)| c == 0 && o.lines().any(|l| l.trim_start().starts_with("default")))
            .unwrap_or(false);
            if !present && route_default(dev, 0, Some(tunnel_route)) {
                actions.push(format!("installed tunnel default {dev} table {tunnel_route}"));
            }
        }
        None => {
            // A tunnel that exists and is still up is not "down" because one
            // internet probe failed. Losing table 100 for a tick sends VLAN 225
            // out the fallback and back again, and every move rewrites the NAT
            // source of every open connection. Hold it for three failed ticks,
            // then let VLAN 225 fall through to the working egress.
            let alive = candidates.iter().any(|c| {
                c.info.kind == Kind::Tunnel && c.info.link_up && c.failures < 3
            });
            if !alive
                && ip(&[
                    "-4",
                    "route",
                    "del",
                    "default",
                    "table",
                    &tunnel_route.to_string(),
                ])
            {
                actions.push(format!("removed dead tunnel default from table {tunnel_route}"));
            } else if alive {
                actions.push("holding tunnel default through transient probe failure".into());
            }
        }
    }

    if let Some(i) = sel.best {
        let c = &candidates[i];
        if let Some(gw) = c.info.gateway.as_deref() {
            if route_via(gw, &c.info.name, cfg.fallback_metric, None) {
                actions.push(format!(
                    "selected {} via {gw} (main, metric {})",
                    c.info.name, cfg.fallback_metric
                ));
            }
            if route_via(gw, &c.info.name, cfg.fallback_metric, Some(cfg.hub_table)) {
                actions.push(format!(
                    "selected {} via {gw} (table {}, metric {})",
                    c.info.name, cfg.hub_table, cfg.fallback_metric
                ));
            }
            // The tunnel dials *through* this path; without the /32 a dead hub
            // takes public services and IPv6 with it even while a backup works.
            let _ = ip(&[
                "-4",
                "route",
                "replace",
                &format!("{}/32", cfg.l2tp_endpoint),
                "via",
                gw,
                "dev",
                &c.info.name,
                "metric",
                &cfg.l2tp_metric.to_string(),
            ]);
        } else if c.info.kind.is_gatewayless() {
            // A relay is a point-to-point link to a box that owns the uplink, so
            // there is no next hop to name: `dev <relay>` is the whole route. It
            // is written to both tables for the same reason the `via` form is —
            // the main table carries this box's sockets and table 200 carries
            // every VLAN, and the two disagreeing is a failover that half-works.
            let dev = c.info.name.as_str();
            if route_default(dev, cfg.fallback_metric, None) {
                actions.push(format!(
                    "selected relay {dev} (main, metric {})",
                    cfg.fallback_metric
                ));
            }
            if route_default(dev, cfg.fallback_metric, Some(cfg.hub_table)) {
                actions.push(format!(
                    "selected relay {dev} (table {}, metric {})",
                    cfg.hub_table, cfg.fallback_metric
                ));
            }
            // The tunnel still has to dial *through* the relay: name it as the
            // path to AAISP's LNS so public addresses and IPv6 do not fall away
            // while the relay is the egress.
            let _ = ip(&[
                "-4",
                "route",
                "replace",
                &format!("{}/32", cfg.l2tp_endpoint),
                "dev",
                dev,
                "metric",
                &cfg.l2tp_metric.to_string(),
            ]);
        }
    } else if let Some(i) = sel.tunnel {
        // Nothing but the tunnel: it can carry the router's own traffic, and
        // the `mark 1` table is already pointing at it. Do not write a table
        // 200 route: there is no non-tunnel gateway to NAT a VLAN to, and a
        // `dev ppp0` default there would be a lie about what can be NATed.
        let dev = candidates[i].info.name.as_str();
        if route_default(dev, cfg.fallback_metric, None) {
            actions.push(format!("tunnel-only: main default dev {dev}"));
        }
    }
    let _ = run("ip", &["route".to_string(), "flush".to_string(), "cache".to_string()], 5);
    actions
}

// ---------------------------------------------------------------------------
// NAT and the input-side interface set
// ---------------------------------------------------------------------------

const TABLES: [(&str, &str); 3] = [
    ("ip", "routerNat"),
    ("ip", "routerIn"),
    ("ip6", "router6"),
];

fn set_exists_named(family: &str, table: &str, set: &str) -> bool {
    nft_out(&["list", "set", family, table, set]).is_some()
}

fn set_exists(family: &str, table: &str) -> bool {
    set_exists_named(family, table, SET)
}

/// The NAT table is the one this module may create: a LAN with no NAT is worse
/// than a missing firewall, and the emergency script says the same. The input
/// tables belong to the flake and are not recreated here.
fn ensure_nat_table() -> bool {
    if set_exists("ip", "routerNat") {
        return true;
    }
    let _ = nft(&["add", "table", "ip", "routerNat"]);
    let _ = nft(&[
        "add", "chain", "ip", "routerNat", "srcnat", "{", "type", "nat", "hook", "postrouting",
        "priority", "srcnat;", "}",
    ]);
    nft(&[
        "add", "set", "ip", "routerNat", SET, "{", "type", "ifname;", "}",
    ])
}

fn ensure_rule(cfg: &Config) {
    let Some(chain) = nft_out(&["list", "chain", "ip", "routerNat", "srcnat"]) else {
        return;
    };
    if chain.contains(&format!("@{SET}")) {
        return;
    }
    for src in &cfg.nat_sources {
        let _ = nft(&[
            "add", "rule", "ip", "routerNat", "srcnat", "oifname", &format!("@{SET}"), "ip",
            "saddr", src, "masquerade",
        ]);
    }
}

fn current_members_named(family: &str, table: &str, set: &str) -> Vec<String> {
    let Some(out) = nft_out(&["list", "set", family, table, set]) else {
        return Vec::new();
    };
    // `elements = { "wlp0s29u1u2", "ppp0" }`
    let Some(elems) = out.split("elements").nth(1) else {
        return Vec::new();
    };
    elems
        .split('"')
        .skip(1)
        .step_by(2)
        .map(|s| s.to_string())
        .collect()
}

fn sync_named_set(
    family: &str,
    table: &str,
    set: &str,
    desired: &[String],
    actions: &mut Vec<String>,
) {
    if !set_exists_named(family, table, set) {
        return;
    }
    let current = current_members_named(family, table, set);
    for name in desired {
        if !current.contains(name)
            && nft(&["add", "element", family, table, set, "{", name, "}"])
        {
            actions.push(format!("{table} {set} += {name}"));
        }
    }
    for name in &current {
        if !desired.contains(name)
            && nft(&["delete", "element", family, table, set, "{", name, "}"])
        {
            actions.push(format!("{table} {set} -= {name}"));
        }
    }
}

fn sync_set(family: &str, table: &str, desired: &[String], actions: &mut Vec<String>) {
    sync_named_set(family, table, SET, desired, actions)
}

pub fn apply_nat(cfg: &Config, candidates: &[Candidate], sel: &Selection) -> Vec<String> {
    let mut actions = Vec::new();
    if !ensure_nat_table() {
        return actions;
    }
    ensure_rule(cfg);
    // Desired: every candidate that could actually carry a packet, not just the
    // selected one. A route change then needs no NAT change, which is the
    // window where a failover would otherwise lose the LAN.
    let mut desired: Vec<String> = candidates
        .iter()
        .filter(|c| c.info.link_up)
        .filter(|c| match c.info.kind {
            Kind::Tunnel => c
                .info
                .addresses
                .iter()
                .any(|(a, _)| !crate::net::is_lan_ip(cfg, *a)),
            // A relay carries whatever the far end can NAT; its own address is
            // in no source block, so the address test the tunnel uses would
            // exclude it and the LAN would lose NAT on the tick it is chosen.
            Kind::Relay => c.info.link_up,
            _ => c.info.gateway.is_some(),
        })
        .map(|c| c.info.name.clone())
        .collect();
    // A selected path that did not meet the filter is still the one carrying the
    // network; forcing it into the set is what keeps NAT from depending on a
    // probe that has not run yet.
    for i in [sel.best, sel.tunnel].into_iter().flatten() {
        desired.push(candidates[i].info.name.clone());
    }
    desired.sort();
    desired.dedup();
    for (family, table) in TABLES {
        sync_set(family, table, &desired, &mut actions);
    }
    // The mark rules in `machines/gw/firewall.nix` exclude destinations that are
    // the uplink's *own* connected subnet, so traffic to the hub's address keeps
    // its connected route. The manager fills that set from the DHCP addresses it
    // found — the hub's subnet is discovered, never written down.
    sync_wan_local(candidates, &mut actions);
    actions
}

/// `10.120.243.17/24` → `10.120.243.0/24`. The set is `flags interval`, so a
/// prefix is an element; an address would exclude only the gateway itself.
fn net_prefix(ip: std::net::Ipv4Addr, mask: u8) -> String {
    let m = if mask == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(mask))
    };
    let n = u32::from(ip) & m;
    format!("{}/{}", std::net::Ipv4Addr::from(n), mask)
}

fn sync_wan_local(candidates: &[Candidate], actions: &mut Vec<String>) {
    let Some(_) = nft_out(&["list", "set", "ip", "router", "wan_local_v4"]) else {
        return;
    };
    let mut desired: Vec<String> = candidates
        .iter()
        .filter(|c| c.info.kind != Kind::Tunnel)
        .flat_map(|c| c.info.addresses.iter().map(|(ip, mask)| net_prefix(*ip, *mask)))
        .collect();
    desired.sort();
    desired.dedup();
    sync_named_set("ip", "router", "wan_local_v4", &desired, actions);
}

/// Whether the kernel's own default is the one the daemon installed. Used by
/// the status file as the "did anything else remove it" question.
pub fn main_default_dev() -> Option<String> {
    let text = run(
        "ip",
        &["-4".to_string(), "route".to_string(), "show".to_string(), "default".to_string()],
        5,
    )
    .map(|(_, o, _)| o)?;
    text.lines().find_map(route_dev)
}
