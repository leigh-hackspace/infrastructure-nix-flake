//! The DHCP server loop: attach to each configured interface, answer what
//! `decision.rs` decided, and never exit because a device is missing.
//!
//! ## A missing interface is not a startup failure
//!
//! dnsmasq learned this the hard way here, and the lesson is written into
//! `machines/gw/dns.nix`: with `bind-interfaces`, a VLAN device that
//! systemd-networkd has not created yet is a **fatal** error at startup, and
//! the never-give-up policy turns that into a restart loop on the box that
//! answers DNS and DHCP for the building. The old box appeared healthy
//! (`systemctl is-system-running` said `running`) because a router that serves
//! no DHCP has no failed unit.
//!
//! So this loop reconciles: every second it looks at what the config asks for
//! and what `/sys/class/net` has, opens a socket for anything that appeared,
//! drops one for anything that went away, and logs the transition **once**.
//! A VLAN that appears ten minutes after boot starts answering leases the
//! second it appears.

use crate::config::Config;
use crate::dhcp::{
    boot_fields, decide, lease_from, reply_options, Client, Effect, Kind, Message, Reply,
    FLAG_BROADCAST,
};
use crate::lease::LeaseDb;
use crate::stats::Stats;
use crate::sys;
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DHCP_SERVER_PORT: u16 = 67;
const DHCP_CLIENT_PORT: u16 = 68;
/// How often the poll round runs. DHCP's own timers are in seconds, so this is
/// about not being *late* rather than being fast; 20 ms costs one wakeup per
/// 50 ms of nothing on a box with four threads.
const POLL: Duration = Duration::from_millis(20);
const RECONCILE: Duration = Duration::from_secs(1);

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

struct Bound {
    socket: UdpSocket,
}

/// Run for ever. `Err` is only "the daemon cannot run at all" (no configured
/// interface, a config that contradicts itself at runtime); everything else is
/// logged and retried.
pub fn run(
    cfg: Arc<Config>,
    db: Arc<Mutex<LeaseDb>>,
    stats: Arc<Stats>,
    verbose: bool,
) -> std::io::Result<()> {
    if cfg.dhcp.interfaces.is_empty() {
        println!("netd: no DHCP interfaces configured — the resolver is the only job");
    }
    let mut bound: BTreeMap<String, Bound> = BTreeMap::new();
    let mut last_error: BTreeMap<String, String> = BTreeMap::new();
    let mut next_reconcile = Instant::now();

    loop {
        if Instant::now() >= next_reconcile {
            reconcile(&cfg, &mut bound, &mut last_error);
            next_reconcile = Instant::now() + RECONCILE;
        }
        for (device, b) in bound.iter() {
            loop {
                let mut buf = [0u8; 1500];
                match b.socket.recv_from(&mut buf) {
                    Ok((n, from)) => {
                        let ts = now();
                        Stats::bump(&stats.dhcp_rx);
                        handle(
                            &cfg,
                            &db,
                            &stats,
                            device,
                            &b.socket,
                            &buf[..n],
                            from,
                            ts,
                            verbose,
                        );
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == ErrorKind::Interrupted => break,
                    Err(e) => {
                        // A socket error is per-device and usually means the
                        // device went away; the reconciler will drop it on the
                        // next pass, so this only records it.
                        Stats::bump(&stats.dhcp_errors);
                        eprintln!("netd: {device}: receive failed: {e}");
                        break;
                    }
                }
            }
        }
        std::thread::sleep(POLL);
    }
}

fn reconcile(
    cfg: &Config,
    bound: &mut BTreeMap<String, Bound>,
    last_error: &mut BTreeMap<String, String>,
) {
    for device in &cfg.dhcp.interfaces {
        let present = sys::interface_exists(device);
        match (present, bound.contains_key(device)) {
            (true, false) => match sys::dhcp_socket(device, DHCP_SERVER_PORT) {
                Ok(socket) => {
                    let _ = socket.set_nonblocking(true);
                    bound.insert(device.clone(), Bound { socket });
                    last_error.remove(device);
                    println!("netd: dhcp on {device} (bound {DHCP_SERVER_PORT}/udp)");
                }
                Err(e) => {
                    // Say it once per distinct message, not once a second.
                    let msg = e.to_string();
                    if last_error.get(device) != Some(&msg) {
                        eprintln!("netd: {device}: cannot open a DHCP socket yet: {msg}");
                        last_error.insert(device.clone(), msg);
                    }
                }
            },
            (false, false) => {
                let msg = format!(
                    "no such interface ({}); waiting for it to appear",
                    match sys::carrier(device) {
                        Some(true) => "carrier up",
                        Some(false) => "no carrier",
                        None => "no device",
                    }
                );
                if last_error.get(device) != Some(&msg) {
                    eprintln!("netd: {device}: {msg} — this is not fatal, the lease table still serves every other VLAN");
                    last_error.insert(device.clone(), msg);
                }
            }
            (true, true) => {}
            (false, true) => {
                bound.remove(device);
                println!(
                    "netd: {device}: gone, DHCP detached (it will reattach if the device returns)"
                );
            }
        }
    }
    // A device dropped from the config stops answering immediately.
    bound.retain(|device, _| {
        let keep = cfg.dhcp.interfaces.contains(device);
        if !keep {
            println!("netd: {device}: no longer configured, DHCP detached");
        }
        keep
    });
}

#[allow(clippy::too_many_arguments)]
fn handle(
    cfg: &Config,
    db: &Arc<Mutex<LeaseDb>>,
    stats: &Stats,
    device: &str,
    socket: &UdpSocket,
    bytes: &[u8],
    from: SocketAddr,
    now: i64,
    verbose: bool,
) {
    let req = match Message::parse(bytes) {
        Ok(m) => m,
        Err(e) => {
            Stats::bump(&stats.dhcp_errors);
            if verbose {
                eprintln!("netd: {device}: {from}: {e}");
            }
            return;
        }
    };
    let msg_type = req.msg_type().unwrap_or(0);
    let client = match Client::read(&req, msg_type) {
        Ok(c) => c,
        Err(e) => {
            Stats::bump(&stats.dhcp_errors);
            if verbose {
                eprintln!("netd: {device}: {from}: {e}");
            }
            return;
        }
    };

    let answer = {
        let guard = match db.lock() {
            Ok(g) => g,
            Err(_) => {
                eprintln!(
                    "netd: the lease table is poisoned; refusing to answer DHCP for {device}"
                );
                Stats::bump(&stats.dhcp_errors);
                return;
            }
        };
        decide(cfg, &guard, device, &client, now)
    };

    // Apply the effect first: a release must forget the row before we print
    // anything, and a decline must hold the address back before the next
    // request can be offered it.
    match answer.effect {
        Effect::Release => {
            if let Some(ip) = answer.ip {
                if let Ok(mut g) = db.lock() {
                    let dropped = g.release(&client.mac, client.client_id.as_ref(), ip);
                    if dropped {
                        Stats::bump(&stats.dhcp_releases);
                        println!("netd: {device} RELEASE {ip} {}", client.describe());
                    }
                }
            }
            return;
        }
        Effect::Decline => {
            if let Some(ip) = answer.ip {
                if let Ok(mut g) = db.lock() {
                    g.decline(ip, now + i64::from(cfg.dhcp.decline_quarantine));
                    Stats::bump(&stats.dhcp_declines);
                    println!(
                        "netd: {device} DECLINE {ip} {} — held back for {}s",
                        client.describe(),
                        cfg.dhcp.decline_quarantine
                    );
                }
            }
            return;
        }
        Effect::None => {}
    }

    match answer.kind {
        Kind::Ignore => {
            Stats::bump(&stats.dhcp_ignored);
            if verbose {
                println!(
                    "netd: {device} {} {} — {}",
                    crate::dhcp::msg_type_name(msg_type),
                    client.describe(),
                    answer.reason
                );
            }
            return;
        }
        Kind::Nak => Stats::bump(&stats.dhcp_naks),
        Kind::Offer => Stats::bump(&stats.dhcp_offers),
        Kind::Ack | Kind::InformAck => Stats::bump(&stats.dhcp_acks),
    }

    // The row is written on ACK, never on OFFER: the server commits on REQUEST
    // (RFC 2131), and a lease table full of offers is a table of addresses no
    // client ever took.
    if let Some(lease) = lease_from(&answer, &client, device, now) {
        if let Ok(mut g) = db.lock() {
            g.upsert(lease);
        }
    }

    let pool = cfg.pool_for(device);
    let opts = reply_options(
        cfg,
        pool,
        &answer.tags,
        answer.ttl,
        answer.netboot.as_ref(),
        answer.kind,
    );
    let mut reply = Reply::new(&req);
    let (siaddr, file) = boot_fields(answer.netboot.as_ref());
    reply.siaddr = siaddr;
    reply.file_name = file;
    reply.options = opts;
    if let Some(ip) = answer.ip {
        if answer.kind != Kind::InformAck {
            reply.yiaddr = ip;
        }
    }
    let body = reply.encode();
    // Option 57 is the client's own limit on how big a reply it can take. Ours
    // are a few hundred bytes and every option in one is load-bearing, so this
    // reports rather than drops — but a report is the difference between a
    // client ignoring one option and a client ignoring the lease.
    if body.len() > client.max_size as usize {
        eprintln!(
            "netd: {device}: reply to {} is {} bytes, over its option-57 limit of {}",
            client.describe(),
            body.len(),
            client.max_size
        );
    }

    // Where the reply goes. A client without an address cannot receive a
    // unicast packet, and asking it to answer ARP for an address it does not
    // have yet is the reason servers send raw frames; broadcasting is the
    // RFC-blessed alternative and the one this daemon uses. A renewal
    // (`ciaddr` set, and the flag clear) is unicast, because that client *does*
    // own the address.
    let target = if req.flags & FLAG_BROADCAST != 0 || reply.ciaddr.is_unspecified() {
        SocketAddr::from((Ipv4Addr::BROADCAST, DHCP_CLIENT_PORT))
    } else {
        SocketAddr::from((reply.ciaddr, DHCP_CLIENT_PORT))
    };
    match socket.send_to(&body, target) {
        Ok(_) => {
            let verb = match answer.kind {
                Kind::Offer => "OFFER",
                Kind::Ack => "ACK",
                Kind::InformAck => "ACK (inform)",
                Kind::Nak => "NAK",
                Kind::Ignore => "",
            };
            println!(
                "netd: {device} {verb} {} {} — {}",
                answer
                    .ip
                    .map(|i| i.to_string())
                    .unwrap_or_else(|| "no address".into()),
                client.describe(),
                answer.reason
            );
        }
        Err(e) => {
            Stats::bump(&stats.dhcp_errors);
            eprintln!(
                "netd: {device}: reply to {} ({}) failed: {e}",
                client.describe(),
                from
            );
        }
    }
}
