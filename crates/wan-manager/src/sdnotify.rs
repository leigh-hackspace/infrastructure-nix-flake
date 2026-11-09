//! `sd_notify(3)` without a dependency.
//!
//! `Type=notify` + `WatchdogSec=` is the half of "can never fail to start and
//! recovers even when it is wedged" that systemd can enforce: a process that
//! stops petting the watchdog is killed and restarted by the supervisor. The
//! protocol is one datagram to `$NOTIFY_SOCKET`, so it does not need a crate.
//! If the socket is absent (running by hand) every call is a no-op.

#[cfg(target_os = "linux")]
pub fn notify(msg: &str) {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};
    let Ok(path) = std::env::var("NOTIFY_SOCKET") else {
        return;
    };
    let Ok(sock) = UnixDatagram::unbound() else {
        return;
    };
    if let Some(rest) = path.strip_prefix('@') {
        if let Ok(addr) = SocketAddr::from_abstract_name(rest.as_bytes()) {
            let _ = sock.send_to_addr(msg.as_bytes(), &addr);
        }
    } else {
        let _ = sock.send_to(msg.as_bytes(), &path);
    }
}

#[cfg(not(target_os = "linux"))]
pub fn notify(_msg: &str) {}

pub fn ready() {
    notify("READY=1\nSTATUS=watching every WAN path");
}

pub fn watchdog() {
    notify("WATCHDOG=1");
}

pub fn status(text: &str) {
    notify(&format!("STATUS={text}"));
}
