//! `netd probe`: a complete DHCP lease cycle from the box, against whatever
//! server is answering on that interface.
//!
//! This exists for the cutover (`docs/netd-2026-11-26.md`): the only honest way
//! to prove a replacement hands out the same leases as dnsmasq is to be the same
//! client twice. It sends a DISCOVER with a throwaway MAC, prints the OFFER's
//! options, REQUESTs the address, confirms the ACK, and RELEASEs it — so
//! running it against the *live* dnsmasq leaves the lease table as it was.
//!
//! It binds port 68 (the client port) on the named device: that is where a
//! server sends an OFFER to a client that has no address yet. The router itself
//! is not a DHCP client on any VLAN (its uplink is ppp0), so the port is free;
//! if something does hold it, this exits with that error rather than fighting
//! for it.

use crate::config::mac_parse;
use crate::dhcp::*;
use crate::sys;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CLIENT_PORT: u16 = 68;
const SERVER_PORT: u16 = 67;

pub struct ProbeArgs {
    pub interface: String,
    pub mac: String,
    pub timeout: Duration,
    pub verbose: bool,
}

pub fn run(args: &ProbeArgs) -> Result<(), String> {
    if !sys::CAN_SERVE {
        return Err(sys::describe_unsupported_platform().into());
    }
    let mac = mac_parse(&args.mac)?;
    let socket = sys::dhcp_socket(&args.interface, CLIENT_PORT)
        .map_err(|e| format!("{}: {e}", args.interface))?;
    socket
        .set_read_timeout(Some(args.timeout))
        .map_err(|e| e.to_string())?;
    socket.set_broadcast(true).map_err(|e| format!("{e}"))?;

    let xid = seed();
    let hostname = "netd-probe";
    let mut opts = vec![
        (OPT_MSG_TYPE, vec![MSG_DISCOVER]),
        (OPT_CLIENT_ID, {
            let mut v = vec![0x01];
            v.extend_from_slice(&mac);
            v
        }),
        (OPT_HOSTNAME, enc_str(hostname)),
        (OPT_MAX_SIZE, 1500u16.to_be_bytes().to_vec()),
        (OPT_PARAM_REQ, vec![1, 3, 6, 15, 28, 51, 58, 59, 66, 67]),
    ];
    if args.verbose {
        opts.push((OPT_USER_CLASS, b"netd probe".to_vec()));
    }
    let discover = {
        let mut p = Reply::request(xid, &mac);
        p.options = opts.clone();
        p.encode()
    };
    println!(
        "netd probe: DISCOVER on {} as {} (xid {xid:#x})",
        args.interface, args.mac
    );
    broadcast(&socket, &discover)?;

    let offer = wait_for(&socket, xid, MSG_OFFER, args.timeout)?;
    print_packet("OFFER", &offer);
    let yiaddr = offer.yiaddr;
    if yiaddr.is_unspecified() {
        return Err("the server's OFFER carried no address".into());
    }
    let server = offer.opt_ip(OPT_SERVER_ID).ok_or(
        "the OFFER carried no server identifier (option 54) — no REQUEST can be addressed to it",
    )?;

    let request = {
        let mut p = Reply::request(xid, &mac);
        p.options = opts.clone();
        p.options[0] = (OPT_MSG_TYPE, vec![MSG_REQUEST]);
        p.options.push((OPT_REQUESTED_IP, enc_ip(yiaddr)));
        p.options.push((OPT_SERVER_ID, enc_ip(server)));
        p.encode()
    };
    println!("netd probe: REQUEST {yiaddr} from server {server}");
    broadcast(&socket, &request)?;
    let ack = wait_for(&socket, xid, MSG_ACK, args.timeout)?;
    print_packet("ACK", &ack);

    // Give the lease back, so asking the live server a question does not change
    // its answer: a probe that leaves a lease behind would be a table entry
    // nobody can explain.
    let mut release = Reply::request(xid ^ 0xffff, &mac);
    release.ciaddr = yiaddr;
    release.options = vec![
        (OPT_MSG_TYPE, vec![MSG_RELEASE]),
        (OPT_SERVER_ID, enc_ip(server)),
        (OPT_CLIENT_ID, {
            let mut v = vec![0x01];
            v.extend_from_slice(&mac);
            v
        }),
    ];
    let release = release.encode();
    if let Err(e) = socket.send_to(&release, SocketAddr::from((server, SERVER_PORT))) {
        eprintln!("netd probe: note — the RELEASE did not send ({e}); the address stays leased for its TTL");
    } else {
        println!("netd probe: RELEASE {yiaddr}");
    }
    Ok(())
}

fn broadcast(socket: &UdpSocket, packet: &[u8]) -> Result<(), String> {
    socket
        .send_to(packet, SocketAddr::from((Ipv4Addr::BROADCAST, SERVER_PORT)))
        .map(|_| ())
        .map_err(|e| format!("sending to 255.255.255.255:{SERVER_PORT}: {e}"))
}

/// Read until a packet with our xid and the wanted message type arrives. Other
/// xids are a real thing on a busy VLAN (another client's conversation), so
/// they are skipped rather than mistaken for an answer.
fn wait_for(socket: &UdpSocket, xid: u32, want: u8, timeout: Duration) -> Result<Message, String> {
    let deadline = SystemTime::now() + timeout;
    let mut buf = [0u8; 1500];
    loop {
        if SystemTime::now() > deadline {
            return Err(format!(
                "no {} within {:?} — is a DHCP server answering on this interface?",
                msg_type_name(want),
                timeout
            ));
        }
        match socket.recv_from(&mut buf) {
            Ok((n, _from)) => {
                let Ok(m) = Message::parse(&buf[..n]) else {
                    continue;
                };
                if m.xid == xid && m.msg_type() == Some(want) {
                    return Ok(m);
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(format!("receive: {e}")),
        }
    }
}

fn print_packet(label: &str, m: &Message) {
    println!(
        "netd probe: {label} yiaddr {} siaddr {}",
        m.yiaddr, m.siaddr
    );
    if let Some(f) = m.opt(OPT_BOOTFILE) {
        println!("  bootfile  {}", String::from_utf8_lossy(f));
    }
    if let Some(f) = m.opt(OPT_TFTP_SERVER) {
        println!("  next-server {}", String::from_utf8_lossy(f));
    }
    for (name, value) in describe_request(m) {
        match name.as_str() {
            "op" | "xid" | "mac" | "yiaddr" | "siaddr" | "message-type" => continue,
            _ => println!("  {name:24} {value}"),
        }
    }
}

/// A stable-per-invocation xid without pulling in a random number source: the
/// nanosecond clock, mixed. It only has to not collide with another client's
/// conversation on the same VLAN for a few seconds.
fn seed() -> u32 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(1);
    nanos ^ (std::process::id() << 16) ^ 0x5eed_1234
}
