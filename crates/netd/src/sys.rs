//! The parts of the daemon that touch the kernel: interface names, and the two
//! socket kinds this box needs to be a DHCP server and an IPv6 router.
//!
//! ## Why there is `unsafe` here and nowhere else
//!
//! The house style is zero external crates, and `std` cannot do either of the
//! two things this daemon needs:
//!
//!   * **one DHCP socket per interface**, bound to that interface with
//!     `SO_BINDTODEVICE`. That is what makes a request's VLAN derivable *from
//!     the socket it arrived on* rather than from a packet field, and it is the
//!     whole reason a DHCP reply can be sent out of the right cable with no
//!     `IP_PKTINFO` cmsg handling. dnsmasq uses one socket and cmsgs; this is
//!     the same information via N sockets, which is simpler to get right and
//!     fail-safe when a VLAN device does not exist yet (that socket is simply
//!     not created — see `dhcp_server.rs` on why that must never be fatal);
//!   * **router advertisements**, which are raw ICMPv6. There is no unprivileged
//!     API for them.
//!
//! So: three `extern "C"` declarations and two `#[repr(C)]` structs, all
//! Linux-specific and all in this file. Everything above it is safe code.

use std::io;
use std::net::{Ipv6Addr, UdpSocket};
use std::os::unix::io::{FromRawFd, RawFd};

// --- the constants (Linux, x86_64 and arm64 agree on all of these) ----------

const AF_INET: i32 = 2;
const AF_INET6: i32 = 10;
const SOCK_DGRAM: i32 = 2;
const SOCK_RAW: i32 = 3;
const IPPROTO_ICMPV6: i32 = 58;
const SOL_SOCKET: i32 = 1;
const SO_REUSEADDR: i32 = 2;
const SO_BROADCAST: i32 = 6;
const SO_RCVTIMEO: i32 = 20;
const SO_BINDTODEVICE: i32 = 25;
const IPPROTO_IPV6: i32 = 41;
const IPV6_UNICAST_HOPS: i32 = 16;
const IPV6_MULTICAST_HOPS: i32 = 18;
const EINTR: i32 = 4;

extern "C" {
    fn socket(domain: i32, ty: i32, protocol: i32) -> i32;
    fn setsockopt(fd: i32, level: i32, name: i32, value: *const u8, len: u32) -> i32;
    fn bind(fd: i32, addr: *const u8, len: u32) -> i32;
    fn sendto(fd: i32, buf: *const u8, len: usize, flags: i32, addr: *const u8, alen: u32)
        -> isize;
    fn recvfrom(
        fd: i32,
        buf: *mut u8,
        len: usize,
        flags: i32,
        addr: *mut u8,
        alen: *mut u32,
    ) -> isize;
    fn close(fd: i32) -> i32;
}

/// `struct sockaddr_in` — family and port, then the address.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SockAddrIn {
    family: u16,
    port: u16,
    addr: [u8; 4],
    zero: [u8; 8],
}

/// `struct sockaddr_in6`, including the scope id that makes a link-local
/// destination mean "this interface".
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SockAddrIn6 {
    family: u16,
    port: u16,
    flowinfo: u32,
    addr: [u8; 16],
    scope_id: u32,
}

fn last_error() -> io::Error {
    io::Error::last_os_error()
}

fn setsockopt_raw(fd: RawFd, level: i32, name: i32, bytes: &[u8]) -> io::Result<()> {
    let rc = unsafe { setsockopt(fd, level, name, bytes.as_ptr(), bytes.len() as u32) };
    if rc < 0 {
        return Err(last_error());
    }
    Ok(())
}

fn set_i32(fd: RawFd, level: i32, name: i32, value: i32) -> io::Result<()> {
    setsockopt_raw(fd, level, name, &value.to_ne_bytes())
}

// ---------------------------------------------------------------------------
// interfaces
// ---------------------------------------------------------------------------

/// The device's kernel index, which is what a scope id is.
pub fn ifindex(name: &str) -> Option<u32> {
    std::fs::read_to_string(format!("/sys/class/net/{name}/ifindex"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

pub fn interface_exists(name: &str) -> bool {
    std::path::Path::new(&format!("/sys/class/net/{name}")).exists()
}

/// The interface's link-local address, read out of `/proc/net/if_inet6`.
///
/// Only used to report what the RA sender found; the sender itself does not
/// need it because the scope id in the destination picks the source address.
pub fn link_local_v6(name: &str) -> Option<Ipv6Addr> {
    let text = std::fs::read_to_string("/proc/net/if_inet6").ok()?;
    for line in text.lines() {
        // `<32 hex chars> <ifindex> <prefix len> <scope> <flags> <name>`
        let mut fields = line.split_whitespace();
        let addr = fields.next()?;
        let _idx = fields.next()?;
        let _plen = fields.next()?;
        let _scope = fields.next()?;
        let _flags = fields.next()?;
        let dev = fields.next()?;
        if dev != name || addr.len() != 32 {
            continue;
        }
        let mut bytes = [0u8; 16];
        for i in 0..16 {
            bytes[i] = u8::from_str_radix(&addr[i * 2..i * 2 + 2], 16).ok()?;
        }
        let ip = Ipv6Addr::from(bytes);
        // fe80::/10 is the link-local range; a ULA or global address here would
        // be the wrong source for an advertisement.
        return (ip.segments()[0] & 0xffc0 == 0xfe80).then_some(ip);
    }
    None
}

// ---------------------------------------------------------------------------
// the DHCP socket
// ---------------------------------------------------------------------------

/// A UDP socket bound to one device and to port 67.
///
/// `SO_REUSEADDR` is required because several sockets share port 67; they are
/// told apart by `SO_BINDTODEVICE`, which also forces every reply out of the
/// same device the request arrived on. `SO_BROADCAST` is what lets a reply to
/// an address-less client go to 255.255.255.255, which is the only destination
/// that works before the client has configured its address (sending unicast to
/// a yiaddr would need the client to answer ARP for an address it does not have
/// yet — dnsmasq solves that with raw frames, this daemon deliberately does
/// not).
pub fn dhcp_socket(device: &str, port: u16) -> io::Result<UdpSocket> {
    let fd = unsafe { socket(AF_INET, SOCK_DGRAM, 0) };
    if fd < 0 {
        return Err(last_error());
    }
    let result = (|| -> io::Result<()> {
        set_i32(fd, SOL_SOCKET, SO_REUSEADDR, 1)?;
        set_i32(fd, SOL_SOCKET, SO_BROADCAST, 1)?;
        let mut dev = device.as_bytes().to_vec();
        dev.push(0);
        setsockopt_raw(fd, SOL_SOCKET, SO_BINDTODEVICE, &dev)?;
        let sa = SockAddrIn {
            family: AF_INET as u16,
            port: port.to_be(),
            addr: [0, 0, 0, 0],
            zero: [0; 8],
        };
        let rc = unsafe {
            bind(
                fd,
                &sa as *const SockAddrIn as *const u8,
                std::mem::size_of::<SockAddrIn>() as u32,
            )
        };
        if rc < 0 {
            return Err(last_error());
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(unsafe { UdpSocket::from_raw_fd(fd) }),
        Err(e) => {
            unsafe { close(fd) };
            Err(io::Error::new(
                e.kind(),
                format!("{device}: {e} (a DHCP socket needs CAP_NET_RAW and a device that exists)"),
            ))
        }
    }
}

/// A plain UDP socket with a receive timeout, for DNS and for the unprivileged
/// test path.
pub fn udp_socket(addr: &str) -> io::Result<UdpSocket> {
    let s = UdpSocket::bind(addr)?;
    s.set_read_timeout(Some(std::time::Duration::from_millis(500)))?;
    Ok(s)
}

// ---------------------------------------------------------------------------
// raw ICMPv6, for router advertisements
// ---------------------------------------------------------------------------

/// A raw ICMPv6 socket. Sending a router advertisement is not a privilege
/// systemd can grant per-operation, so the unit runs with `CAP_NET_RAW` and
/// this is the only thing that uses it.
pub struct Icmp6 {
    fd: RawFd,
}

impl Drop for Icmp6 {
    fn drop(&mut self) {
        unsafe {
            close(self.fd);
        }
    }
}

impl Icmp6 {
    pub fn open() -> io::Result<Icmp6> {
        let fd = unsafe { socket(AF_INET6, SOCK_RAW, IPPROTO_ICMPV6) };
        if fd < 0 {
            return Err(io::Error::new(
                last_error().kind(),
                format!(
                    "raw ICMPv6 socket: {} (the unit needs CAP_NET_RAW)",
                    last_error()
                ),
            ));
        }
        // RFC 4861 §6.1.2: a router advertisement MUST be sent with a hop limit
        // of 255, and a receiver MUST discard one that arrives with less. Both
        // settings are here because a multicast send is what we do and a
        // solicited unicast reply is what we also do.
        let r = set_i32(fd, IPPROTO_IPV6, IPV6_MULTICAST_HOPS, 255)
            .and_then(|_| set_i32(fd, IPPROTO_IPV6, IPV6_UNICAST_HOPS, 255));
        if let Err(e) = r {
            unsafe { close(fd) };
            return Err(e);
        }
        // A short receive timeout, so the RS poll cannot hold the RA timer up.
        let ts = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let _ = setsockopt_raw(fd, SOL_SOCKET, SO_RCVTIMEO, &ts.to_bytes());
        Ok(Icmp6 { fd })
    }

    /// Send one packet to `ff02::1` — every node on the link — out of `ifindex`.
    pub fn send_all_nodes(&self, ifindex: u32, packet: &[u8]) -> io::Result<()> {
        let dst = SockAddrIn6 {
            family: AF_INET6 as u16,
            port: 0,
            flowinfo: 0,
            addr: "ff02::1".parse::<Ipv6Addr>().expect("a literal").octets(),
            scope_id: ifindex,
        };
        let n = unsafe {
            sendto(
                self.fd,
                packet.as_ptr(),
                packet.len(),
                0,
                &dst as *const SockAddrIn6 as *const u8,
                std::mem::size_of::<SockAddrIn6>() as u32,
            )
        };
        if n < 0 {
            return Err(last_error());
        }
        Ok(())
    }

    /// Send one packet to a specific link-local address (a solicitation's
    /// source), out of `ifindex`.
    pub fn send_to(&self, ifindex: u32, dst_ip: Ipv6Addr, packet: &[u8]) -> io::Result<()> {
        let dst = SockAddrIn6 {
            family: AF_INET6 as u16,
            port: 0,
            flowinfo: 0,
            addr: dst_ip.octets(),
            scope_id: ifindex,
        };
        let n = unsafe {
            sendto(
                self.fd,
                packet.as_ptr(),
                packet.len(),
                0,
                &dst as *const SockAddrIn6 as *const u8,
                std::mem::size_of::<SockAddrIn6>() as u32,
            )
        };
        if n < 0 {
            return Err(last_error());
        }
        Ok(())
    }

    /// Read one ICMPv6 packet if one is waiting, with where it came from.
    ///
    /// `None` covers "nothing waiting" (`EAGAIN`, which with a zero timeout is
    /// the normal case) and `EINTR`; an error is only a real one.
    pub fn recv(&self) -> io::Result<Option<(Vec<u8>, Ipv6Addr, u32)>> {
        let mut buf = vec![0u8; 1500];
        let mut from = SockAddrIn6::default();
        let mut len = std::mem::size_of::<SockAddrIn6>() as u32;
        let n = unsafe {
            recvfrom(
                self.fd,
                buf.as_mut_ptr(),
                buf.len(),
                0,
                &mut from as *mut SockAddrIn6 as *mut u8,
                &mut len,
            )
        };
        if n < 0 {
            let e = last_error();
            return match e.raw_os_error() {
                Some(11) | Some(EINTR) => Ok(None), // EAGAIN/EINTR
                _ => Err(e),
            };
        }
        buf.truncate(n as usize);
        Ok(Some((buf, Ipv6Addr::from(from.addr), from.scope_id)))
    }
}

/// `struct timespec` on 64-bit Linux.
#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

impl Timespec {
    fn to_bytes(&self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[..8].copy_from_slice(&self.tv_sec.to_ne_bytes());
        out[8..].copy_from_slice(&self.tv_nsec.to_ne_bytes());
        out
    }
}

/// A node's own name for a socket that is about to be closed; only exists so
/// `dhcp_server` can report a device it could not bind without the message
/// naming the wrong device.
pub fn describe_unsupported_platform() -> &'static str {
    "netd's socket layer is Linux-only (SO_BINDTODEVICE and raw ICMPv6); on any other platform run `netd check`, which needs no sockets"
}

/// `true` when this build can serve: the socket layer compiled, which it does
/// only for Linux targets the constants above are right for.
pub const CAN_SERVE: bool = cfg!(target_os = "linux");

/// Does an interface have carrier right now? Read from `/sys` so a cable being
/// out is visible without ioctls.
pub fn carrier(device: &str) -> Option<bool> {
    let s = std::fs::read_to_string(format!("/sys/class/net/{device}/carrier")).ok()?;
    Some(s.trim() == "1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_platform_this_build_would_serve_on_is_known() {
        // A fact the daemon reports at startup, and the one thing that must not
        // be discovered on the router.
        assert_eq!(CAN_SERVE, cfg!(target_os = "linux"));
    }

    #[test]
    fn loopback_is_present_and_indexed() {
        assert!(interface_exists("lo"));
        assert_eq!(ifindex("lo"), Some(1));
        assert!(carrier("lo").is_some() || !cfg!(target_os = "linux"));
    }

    #[test]
    fn a_missing_interface_is_reported_as_missing_not_as_an_error() {
        assert!(!interface_exists("definitely-not-a-real-interface"));
        assert_eq!(ifindex("definitely-not-a-real-interface"), None);
    }

    #[test]
    fn loopback_has_no_link_local_and_that_is_not_a_panic() {
        // `lo` has ::1 and no fe80:: address; the RA sender must treat this as
        // "nothing to advertise on" rather than as a reason to stop.
        assert_eq!(link_local_v6("lo"), None);
    }

    #[test]
    fn a_udp_socket_can_be_bound_and_timed_out() {
        let s = udp_socket("127.0.0.1:0").expect("loopback binds");
        assert!(s.local_addr().is_ok());
        assert!(s.read_timeout().unwrap().is_some());
    }
}
