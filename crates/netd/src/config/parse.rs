//! Reading the document, with a path in every error.
//!
//! The reader is deliberately dumb and strict: no defaults for anything that
//! changes behaviour, no ignored keys, and every error names the field
//! (`dhcp.pools[3].from: …`) so a generator can be fixed from the message
//! alone. Whether the assembled document makes *sense* is `validate.rs`'s job;
//! this file only decides that each field is the right shape.

use super::schema::{
    canonical_name, client_id_parse, hex_parse, mac_parse, Api, Conditional, Config, Dhcp,
    DhcpOption, Dns, Named, Netboot, OptionValue, Pool, Ra, RaIface, RaNet, Reservation, Soa,
    V4Net,
};
use common_json::Json;
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;

/// A cursor into the document that knows where it is.
pub(super) struct At<'a> {
    pub(super) v: &'a Json,
    pub(super) at: String,
}

impl<'a> At<'a> {
    fn new(v: &'a Json, at: &str) -> At<'a> {
        At {
            v,
            at: at.to_string(),
        }
    }

    fn err<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("{}: {what}", self.at))
    }

    fn obj(&self) -> Result<&'a BTreeMap<String, Json>, String> {
        match self.v {
            Json::Obj(m) => Ok(m),
            _ => self.err("expected an object"),
        }
    }

    /// Refuse a key nobody reads. This is the typo guard: a generator that
    /// writes `forwarder` instead of `forwarders` must not produce a router
    /// that resolves nothing while everything looks green.
    fn only(&self, allowed: &[&str]) -> Result<(), String> {
        let map = self.obj()?;
        let unknown: Vec<&str> = map
            .keys()
            .map(String::as_str)
            .filter(|k| !allowed.contains(k))
            .collect();
        if unknown.is_empty() {
            Ok(())
        } else {
            self.err(&format!(
                "unknown key(s) {} — this schema is strict on purpose; known keys: {}",
                unknown
                    .iter()
                    .map(|k| format!("`{k}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
                allowed.join(", ")
            ))
        }
    }

    fn get(&self, k: &str) -> Result<Option<At<'a>>, String> {
        match self.obj()?.get(k) {
            None | Some(Json::Null) => Ok(None),
            Some(v) => Ok(Some(At::new(v, &format!("{}.{k}", self.at)))),
        }
    }

    fn req(&self, k: &str) -> Result<At<'a>, String> {
        self.get(k)?
            .ok_or_else(|| format!("{}.{k}: required", self.at))
    }

    fn str(&self) -> Result<&'a str, String> {
        match self.v {
            Json::Str(s) => Ok(s),
            _ => self.err("expected a string"),
        }
    }

    fn u64(&self) -> Result<u64, String> {
        match self.v {
            Json::Num(n) if *n >= 0.0 && n.fract() == 0.0 => Ok(*n as u64),
            _ => self.err("expected a non-negative integer"),
        }
    }

    fn bool(&self) -> Result<bool, String> {
        match self.v {
            Json::Bool(b) => Ok(*b),
            _ => self.err("expected true or false"),
        }
    }

    fn arr(&self) -> Result<Vec<At<'a>>, String> {
        match self.v {
            Json::Arr(a) => Ok(a
                .iter()
                .enumerate()
                .map(|(i, v)| At::new(v, &format!("{}[{i}]", self.at)))
                .collect()),
            _ => self.err("expected an array"),
        }
    }

    /// An optional array: absent is an empty list, which is what a generator
    /// that has nothing to say should write.
    fn opt_arr(&self, k: &str) -> Result<Vec<At<'a>>, String> {
        self.get(k)?
            .map(|a| a.arr())
            .transpose()
            .map(Option::unwrap_or_default)
    }

    fn str_list(&self) -> Result<Vec<String>, String> {
        self.arr()?
            .iter()
            .map(|a| Ok(a.str()?.to_string()))
            .collect()
    }

    fn opt_str_list(&self, k: &str) -> Result<Vec<String>, String> {
        self.get(k)?
            .map(|a| a.str_list())
            .transpose()
            .map(Option::unwrap_or_default)
    }

    fn ip(&self) -> Result<IpAddr, String> {
        let s = self.str()?;
        s.parse()
            .map_err(|_| format!("{}: `{s}` is not an IP address", self.at))
    }

    fn ipv4(&self) -> Result<Ipv4Addr, String> {
        match self.ip()? {
            IpAddr::V4(v) => Ok(v),
            IpAddr::V6(_) => self.err("expected an IPv4 address"),
        }
    }

    fn ipv4_list(&self) -> Result<Vec<Ipv4Addr>, String> {
        self.arr()?.iter().map(At::ipv4).collect()
    }

    fn ip_list(&self) -> Result<Vec<IpAddr>, String> {
        self.arr()?.iter().map(At::ip).collect()
    }

    fn v4net(&self) -> Result<V4Net, String> {
        let s = self.str()?;
        let (addr, prefix) = s
            .split_once('/')
            .ok_or_else(|| format!("{}: a CIDR needs a length, e.g. 10.3.0.0/16", self.at))?;
        let addr: Ipv4Addr = addr
            .parse()
            .map_err(|_| format!("{}: `{addr}` is not an IPv4 address", self.at))?;
        let prefix: u8 = prefix
            .parse()
            .map_err(|_| format!("{}: `{prefix}` is not a prefix length", self.at))?;
        if prefix > 32 {
            return self.err("a prefix length is 0..32");
        }
        let mask = Ipv4Addr::from(if prefix == 0 {
            0
        } else {
            u32::MAX << (32 - prefix as u32)
        });
        V4Net::from_mask(addr, mask).ok_or_else(|| format!("{}: bad prefix length", self.at))
    }

    /// An IPv6 CIDR, for the DNS access list's v6 half.
    fn v6net(&self) -> Result<(Ipv6Addr, u8), String> {
        let s = self.str()?;
        let (addr, prefix) = s
            .split_once('/')
            .ok_or_else(|| format!("{}: a CIDR needs a length, e.g. fd00::/8", self.at))?;
        Ok((
            addr.parse()
                .map_err(|_| format!("{}: `{addr}` is not an IPv6 address", self.at))?,
            prefix
                .parse()
                .map_err(|_| format!("{}: `{prefix}` is not a prefix length", self.at))?,
        ))
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, String> {
        let doc = common_json::parse(text).map_err(|e| format!("config is not JSON: {e}"))?;
        let root = At::new(&doc, "config");
        root.only(&["version", "zone", "state_dir", "api", "dns", "dhcp", "ra"])?;
        let version = root.req("version")?.u64()?;
        if version != 1 {
            return Err(format!(
                "config.version: this daemon understands schema version 1, not {version}"
            ));
        }
        let zone = canonical_name(root.req("zone")?.str()?);
        let state_dir = PathBuf::from(root.req("state_dir")?.str()?);
        let api = read_api(&root.req("api")?)?;
        let dns = read_dns(&root.req("dns")?)?;
        let dhcp = read_dhcp(&root.req("dhcp")?)?;
        let ra = read_ra(&root.req("ra")?)?;
        let cfg = Config {
            version,
            zone,
            state_dir,
            api,
            dns,
            dhcp,
            ra,
        };
        cfg.validate()?;
        Ok(cfg)
    }
}

fn read_api(at: &At<'_>) -> Result<Api, String> {
    at.only(&["bind", "port"])?;
    Ok(Api {
        bind: at.req("bind")?.ip()?,
        port: u16::try_from(at.req("port")?.u64()?)
            .map_err(|_| "api.port: must be 1..65535".to_string())?,
    })
}

fn read_dns(at: &At<'_>) -> Result<Dns, String> {
    at.only(&[
        "listen",
        "acl",
        "records",
        "forwarders",
        "conditional",
        "cache_size",
        "ttl",
        "forward_max",
        "domain_needed",
        "stop_rebind",
        "rebind_ok",
        "rebind_localhost_ok",
        "soa",
    ])?;

    let mut records = Vec::new();
    for r in at.req("records")?.arr()? {
        r.only(&["name", "addresses"])?;
        let name = canonical_name(r.req("name")?.str()?);
        let addresses = r.req("addresses")?.ip_list()?;
        if addresses.is_empty() {
            return Err(format!(
                "{}.addresses: empty — a name with no address answers nothing",
                r.at
            ));
        }
        records.push(Named { name, addresses });
    }

    let mut conditional = Vec::new();
    for c in at.opt_arr("conditional")? {
        c.only(&["zone", "servers"])?;
        conditional.push(Conditional {
            zone: canonical_name(c.req("zone")?.str()?),
            servers: c.req("servers")?.ip_list()?,
        });
    }

    // One list for both families, so `common/config.nix`'s ACL can be passed
    // through as it is written (`allow 10.3.1.0/24;` lines become these).
    let (mut acl, mut acl6) = (Vec::new(), Vec::new());
    for a in at.req("acl")?.arr()? {
        if a.str()?.contains(':') {
            acl6.push(a.v6net()?);
        } else {
            acl.push(a.v4net()?);
        }
    }

    let soa_at = at.req("soa")?;
    soa_at.only(&["mname", "rname", "refresh", "retry", "expire", "minimum"])?;
    let n = |k: &str, default: u64| -> Result<u32, String> {
        Ok(match soa_at.get(k)? {
            Some(v) => u32::try_from(v.u64()?).unwrap_or(default as u32),
            None => default as u32,
        })
    };
    let soa = Soa {
        mname: canonical_name(soa_at.req("mname")?.str()?),
        rname: canonical_name(soa_at.req("rname")?.str()?),
        refresh: n("refresh", 3600)?,
        retry: n("retry", 600)?,
        expire: n("expire", 604_800)?,
        minimum: n("minimum", 10)?,
    };

    Ok(Dns {
        listen: at.req("listen")?.str_list()?,
        acl,
        acl6,
        records,
        forwarders: at.req("forwarders")?.ip_list()?,
        conditional,
        cache_size: at.req("cache_size")?.u64()? as usize,
        ttl: at.req("ttl")?.u64()? as u32,
        forward_max: at.req("forward_max")?.u64()? as usize,
        domain_needed: at.req("domain_needed")?.bool()?,
        stop_rebind: at.req("stop_rebind")?.bool()?,
        rebind_ok: at
            .opt_str_list("rebind_ok")?
            .iter()
            .map(|s| canonical_name(s))
            .collect(),
        rebind_localhost_ok: at.req("rebind_localhost_ok")?.bool()?,
        soa,
    })
}

fn read_dhcp(at: &At<'_>) -> Result<Dhcp, String> {
    at.only(&[
        "interfaces",
        "lease_ttl",
        "decline_quarantine",
        "ignored_addresses",
        "ignored_names",
        "pools",
        "reservations",
        "options",
        "netboot",
    ])?;

    let mut pools = Vec::new();
    for p in at.req("pools")?.arr()? {
        p.only(&[
            "interface",
            "tag",
            "from",
            "to",
            "netmask",
            "server_ip",
            "ttl",
        ])?;
        let from = p.req("from")?.ipv4()?;
        let to = p.req("to")?.ipv4()?;
        let server_ip = p.req("server_ip")?.ipv4()?;
        let netmask = p.req("netmask")?.ipv4()?;
        let net = V4Net::from_mask(server_ip, netmask).ok_or_else(|| {
            format!(
                "{}.netmask: {netmask} is not a contiguous mask — the pool's network cannot be derived",
                p.at
            )
        })?;
        if u32::from(from) > u32::from(to) {
            return Err(format!("{}.from: {from} is above `to` ({to})", p.at));
        }
        if !net.contains(from) || !net.contains(to) {
            return Err(format!(
                "{}.from: the range {from}-{to} is not inside {} (server_ip + netmask)",
                p.at,
                net.cidr()
            ));
        }
        pools.push(Pool {
            interface: p.req("interface")?.str()?.to_string(),
            tag: p.req("tag")?.str()?.to_string(),
            from,
            to,
            net,
            server_ip,
            ttl: p.req("ttl")?.u64()? as u32,
        });
    }

    let mut reservations = Vec::new();
    for r in at.req("reservations")?.arr()? {
        r.only(&["name", "address", "macs", "client_ids"])?;
        let macs = r
            .opt_str_list("macs")?
            .iter()
            .map(|m| mac_parse(m))
            .collect::<Result<Vec<_>, _>>()?;
        let client_ids = r
            .opt_str_list("client_ids")?
            .iter()
            .map(|c| client_id_parse(c))
            .collect::<Result<Vec<_>, _>>()?;
        reservations.push(Reservation {
            name: r.req("name")?.str()?.to_string(),
            address: r.req("address")?.ipv4()?,
            macs,
            client_ids,
        });
    }

    let mut options = Vec::new();
    for o in at.opt_arr("options")? {
        o.only(&["tag", "code", "value", "hex"])?;
        let code = u8::try_from(o.req("code")?.u64()?)
            .map_err(|_| format!("{}.code: a DHCP option code is 0..255", o.at))?;
        let tag = o
            .get("tag")?
            .map(|t| t.str())
            .transpose()?
            .map(str::to_string);
        let value = match o.get("hex")? {
            Some(h) => OptionValue::Hex(hex_parse(h.str()?)?),
            None => read_option_value(&o, code)?,
        };
        options.push(DhcpOption { tag, code, value });
    }

    let mut netboot = Vec::new();
    for n in at.opt_arr("netboot")? {
        n.only(&[
            "tag",
            "match_option",
            "match_values",
            "match_contains",
            "when_tags",
            "boot",
            "server",
        ])?;
        let mut match_values = Vec::new();
        for m in n.opt_arr("match_values")? {
            match_values.push(
                u16::try_from(m.u64()?)
                    .map_err(|_| format!("{}: does not fit in a DHCP option value", m.at))?,
            );
        }
        let entry = Netboot {
            tag: n.req("tag")?.str()?.to_string(),
            match_option: u8::try_from(n.req("match_option")?.u64()?)
                .map_err(|_| format!("{}.match_option: 0..255", n.at))?,
            match_values,
            match_contains: n
                .get("match_contains")?
                .map(|c| c.str())
                .transpose()?
                .map(str::to_string),
            when_tags: n.opt_str_list("when_tags")?,
            boot: n.req("boot")?.str()?.to_string(),
            server: n.req("server")?.ipv4()?,
        };
        if entry.match_values.is_empty() && entry.match_contains.is_none() {
            return Err(format!(
                "{}.match_*: a netboot variant with no match is a boot menu offered to every device",
                n.at
            ));
        }
        netboot.push(entry);
    }

    Ok(Dhcp {
        interfaces: at.req("interfaces")?.str_list()?,
        lease_ttl: at.req("lease_ttl")?.u64()? as u32,
        decline_quarantine: at.req("decline_quarantine")?.u64()? as u32,
        ignored_addresses: at
            .opt_arr("ignored_addresses")?
            .iter()
            .map(At::ipv4)
            .collect::<Result<BTreeSet<_>, _>>()?,
        ignored_names: at
            .opt_str_list("ignored_names")?
            .iter()
            .map(|s| canonical_name(s))
            .collect(),
        pools,
        reservations,
        options,
        netboot,
    })
}

/// An option's value is typed by its **code**, not by what the generator felt
/// like emitting: code 6 is a list of addresses and code 43 is a vendor blob,
/// and a schema that let either be a string would put a lease on the wire no
/// client can read.
fn read_option_value(o: &At<'_>, code: u8) -> Result<OptionValue, String> {
    let v = o
        .get("value")?
        .ok_or_else(|| format!("{}.value: required (or `hex`)", o.at))?;
    match code {
        1 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 28 | 42 | 44 | 45 | 48 | 49 | 54 => match v.v {
            Json::Arr(_) => Ok(OptionValue::Ipv4(v.ipv4_list()?)),
            Json::Str(_) => Ok(OptionValue::Ipv4(vec![v.ipv4()?])),
            _ => v.err("expected an IPv4 address or a list of them"),
        },
        13 | 22 | 26 | 51 | 57 | 58 | 59 | 93 => {
            Ok(OptionValue::U32(u32::try_from(v.u64()?).unwrap_or(0)))
        }
        _ => match v.v {
            Json::Str(s) => Ok(OptionValue::Text(s.clone())),
            _ => v.err("expected a string, or use `hex` for a raw value"),
        },
    }
}

fn read_ra(at: &At<'_>) -> Result<Ra, String> {
    at.only(&["interval", "interfaces"])?;
    let mut interfaces = Vec::new();
    for r in at.req("interfaces")?.arr()? {
        r.only(&[
            "interface",
            "prefixes",
            "router_lifetime",
            "valid_lifetime",
            "preferred_lifetime",
            "mtu",
        ])?;
        let mut prefixes = Vec::new();
        for p in r.req("prefixes")?.arr()? {
            prefixes.push(RaNet::parse(p.str()?)?);
        }
        interfaces.push(RaIface {
            interface: r.req("interface")?.str()?.to_string(),
            prefixes,
            router_lifetime: u16::try_from(r.req("router_lifetime")?.u64()?)
                .map_err(|_| format!("{}.router_lifetime: 0..65535 seconds", r.at))?,
            valid_lifetime: r.req("valid_lifetime")?.u64()? as u32,
            preferred_lifetime: r.req("preferred_lifetime")?.u64()? as u32,
            mtu: r
                .get("mtu")?
                .map(|m| m.u64())
                .transpose()?
                .map(|m| m as u32),
        })
    }
    Ok(Ra {
        interval: at.req("interval")?.u64()?,
        interfaces,
    })
}
