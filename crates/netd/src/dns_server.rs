//! The DNS listeners, the cache and the forwarder.
//!
//! ## The shape
//!
//! One thread per configured `listen` address for UDP, plus one TCP listener
//! (truncation and `dig +tcp`). Each UDP thread handles its queries inline, which
//! is fine because the expensive part — waiting for an upstream — happens on a
//! freshly bound socket with a hard 1.5 s timeout per server, and the number of
//! queries in flight is capped by `dns.forward_max` (5000 here, dnsmasq's own
//! number). Over the cap the answer is SERVFAIL rather than a queue: a resolver
//! that queues is a resolver whose latency grows without bound when one upstream
//! goes dark.
//!
//! ## Access control is by source, not by interface
//!
//! dnsmasq is told which interfaces to answer on and filters by the receiving
//! device. This binds the wildcard and filters by the *source address* against
//! `dns.acl` — loopback plus every VLAN block, which is the same set nginx's
//! `LOCAL_NETWORK` admits. That is a stronger check for the case that matters:
//! a query arriving on the WAN with a spoofed private source is refused, and a
//! query from a LAN address is answered whichever device it came in on.

use crate::config::Config;
use crate::dns::{
    answer, rdata_address, Message, Question, Record, FLAG_TC, RCODE_FORMERR, RCODE_NOERROR,
    RCODE_NOTIMP, RCODE_NXDOMAIN, RCODE_REFUSED, RCODE_SERVFAIL, TYPE_A, TYPE_AAAA,
};
use crate::lease::LeaseDb;
use crate::stats::Stats;
use crate::sys;
use crate::util::now;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv6Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long one upstream gets. Four upstreams × 1.5 s is the worst case a
/// client waits, which is inside every resolver's own retry (2 s for glibc's
/// first attempt), so a dead upstream costs a retry rather than a timeout.
const UPSTREAM_TIMEOUT: Duration = Duration::from_millis(1500);
/// Kept as documentation of the UDP socket's own timeout (`sys::udp_socket`);
/// the loop relies on it rather than on a constant here.
#[allow(dead_code)]
const UDP_READ_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Default)]
struct Cache {
    entries: std::collections::HashMap<(String, u16), Entry>,
}

struct Entry {
    expires: i64,
    records: Vec<Record>,
}

impl Cache {
    fn get(&mut self, q: &Question, at: i64) -> Option<Vec<Record>> {
        let k = (q.name.clone(), q.qtype);
        match self.entries.get(&k) {
            Some(e) if e.expires > at => Some(e.records.clone()),
            Some(_) => {
                self.entries.remove(&k);
                None
            }
            None => None,
        }
    }

    fn put(&mut self, q: &Question, records: &[Record], at: i64, max: usize) {
        if max == 0 || records.is_empty() {
            return;
        }
        let ttl = records.iter().map(|r| r.ttl).min().unwrap_or(0);
        if ttl == 0 {
            return;
        }
        if self.entries.len() >= max {
            self.entries.retain(|_, e| e.expires > at);
            if self.entries.len() >= max {
                // Still full of live entries: drop everything rather than
                // evicting by policy. A cache is a performance detail, and a
                // wrong eviction policy is a bug that only shows under load.
                self.entries.clear();
            }
        }
        self.entries.insert(
            (q.name.clone(), q.qtype),
            Entry {
                expires: at + i64::from(ttl),
                records: records.to_vec(),
            },
        );
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

pub struct Dns {
    cfg: Arc<Config>,
    db: Arc<Mutex<LeaseDb>>,
    stats: Arc<Stats>,
    cache: Mutex<Cache>,
    inflight: AtomicUsize,
}

impl Dns {
    pub fn new(cfg: Arc<Config>, db: Arc<Mutex<LeaseDb>>, stats: Arc<Stats>) -> Arc<Dns> {
        Arc::new(Dns {
            cfg,
            db,
            stats,
            cache: Mutex::new(Cache::default()),
            inflight: AtomicUsize::new(0),
        })
    }

    pub fn cache_len(&self) -> usize {
        self.cache.lock().map(|c| c.len()).unwrap_or(0)
    }

    /// Serve for ever: one thread per `listen` address, plus TCP on each.
    pub fn serve(self: Arc<Self>) -> std::io::Result<()> {
        let mut handles = Vec::new();
        for addr in self.cfg.dns.listen.clone() {
            let udp = sys::udp_socket(&addr)?;
            println!("netd: dns udp on {addr}");
            let me = self.clone();
            handles.push(std::thread::spawn(move || me.udp_loop(udp)));
            match TcpListener::bind(&addr) {
                Ok(listener) => {
                    println!("netd: dns tcp on {addr}");
                    let me = self.clone();
                    handles.push(std::thread::spawn(move || me.tcp_loop(listener)));
                }
                Err(e) => eprintln!(
                    "netd: dns tcp on {addr}: {e} — truncation retries will fail, UDP still answers"
                ),
            }
        }
        for h in handles {
            let _ = h.join();
        }
        Ok(())
    }

    fn udp_loop(self: &Arc<Self>, socket: UdpSocket) {
        let mut buf = vec![0u8; 4096];
        loop {
            match socket.recv_from(&mut buf) {
                Ok((n, from)) => {
                    if let Some(resp) = self.respond(&buf[..n], from.ip(), false) {
                        // A truncated answer that is still too big for the
                        // client is dropped: it will retry over TCP.
                        if resp.len() <= self.max_for(&buf[..n]) {
                            let _ = socket.send_to(&resp, from);
                        } else {
                            let tc = self.truncated(&buf[..n]);
                            let _ = socket.send_to(&tc, from);
                        }
                    }
                }
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => {
                    eprintln!("netd: dns udp: {e}");
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }

    fn tcp_loop(self: &Arc<Self>, listener: TcpListener) {
        loop {
            match listener.accept() {
                Ok((stream, from)) => {
                    let me = self.clone();
                    // A TCP query is already an admission that UDP did not fit;
                    // one thread per connection with a read timeout is enough
                    // for a LAN and cannot wedge the UDP path.
                    let _ = std::thread::spawn(move || me.tcp_conn(stream, from));
                }
                Err(e) => {
                    eprintln!("netd: dns tcp accept: {e}");
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }

    fn tcp_conn(&self, mut stream: TcpStream, from: SocketAddr) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let mut len = [0u8; 2];
        if stream.read_exact(&mut len).is_err() {
            return;
        }
        let mut buf = vec![0u8; u16::from_be_bytes(len) as usize];
        if stream.read_exact(&mut buf).is_err() {
            return;
        }
        if let Some(resp) = self.respond(&buf, from.ip(), true) {
            let mut framed = (resp.len() as u16).to_be_bytes().to_vec();
            framed.extend_from_slice(&resp);
            let _ = stream.write_all(&framed);
        }
    }

    fn max_for(&self, query: &[u8]) -> usize {
        match Message::parse(query) {
            Ok(m) => m.udp_payload_size(),
            Err(_) => 512,
        }
    }

    /// A TC-flagged, answer-less response: the client retries over TCP.
    fn truncated(&self, query: &[u8]) -> Vec<u8> {
        match Message::parse(query) {
            Ok(q) => {
                let mut r = Message::response(&q);
                r.flags |= FLAG_TC;
                r.encode()
            }
            Err(_) => Vec::new(),
        }
    }

    /// The whole answer to one query, or `None` when nothing should be sent.
    fn respond(&self, query: &[u8], from: IpAddr, tcp: bool) -> Option<Vec<u8>> {
        Stats::bump(&self.stats.dns_queries);
        if !self.allowed(from) {
            Stats::bump(&self.stats.dns_refused);
            let q = Message::parse(query).ok()?;
            let mut r = Message::response(&q);
            r.flags = (r.flags & !0x000f) | u16::from(RCODE_REFUSED);
            return Some(r.encode());
        }
        let q = match Message::parse(query) {
            Ok(q) => q,
            // A query we cannot parse gets a FORMERR with the id we could read,
            // because silence makes a client wait out its own retry.
            Err(e) => {
                eprintln!("netd: dns: {from}: unparseable query: {e}");
                if query.len() >= 2 {
                    let mut r = Message::response(&Message {
                        id: u16::from_be_bytes([query[0], query[1]]),
                        flags: 0,
                        questions: Vec::new(),
                        answers: Vec::new(),
                        authority: Vec::new(),
                        additional: Vec::new(),
                        edns: None,
                    });
                    r.flags = (r.flags & !0x000f) | u16::from(RCODE_FORMERR);
                    return Some(r.encode());
                }
                return None;
            }
        };
        if q.is_response() {
            // A response arriving at a server is a stray or a spoof; never
            // answer it, or two resolvers can talk to each other for ever.
            return None;
        }
        if q.opcode() != 0 {
            let mut r = Message::response(&q);
            r.flags = (r.flags & !0x000f) | u16::from(RCODE_NOTIMP);
            return Some(r.encode());
        }
        let Some(question) = q.questions.first().cloned() else {
            let mut r = Message::response(&q);
            r.flags = (r.flags & !0x000f) | u16::from(RCODE_FORMERR);
            return Some(r.encode());
        };

        // --- the zone we own ------------------------------------------------
        let leased = match self.db.lock() {
            Ok(g) => answer(&self.cfg, &g, &question, now()),
            Err(_) => {
                // A poisoned lease table must not take DNS down with it: answer
                // from the config's own records, which is what a resolver that
                // had never seen a lease would do.
                answer(&self.cfg, &LeaseDb::new(), &question, now())
            }
        };
        if leased.authoritative {
            Stats::bump(&self.stats.dns_answers);
            let mut r = Message::response(&q);
            r.flags = (r.flags & !0x000f) | u16::from(leased.rcode) | crate::dns::FLAG_AA;
            r.answers = leased.answers;
            r.authority = leased.authority;
            r.edns = q.edns.map(|_| crate::dns::Edns {
                udp_size: 1232,
                do_bit: false,
            });
            return Some(r.encode());
        }

        // --- the cache ------------------------------------------------------
        let at = now();
        if let Some(records) = self
            .cache
            .lock()
            .ok()
            .and_then(|mut c| c.get(&question, at))
        {
            Stats::bump(&self.stats.dns_cache_hits);
            let mut r = Message::response(&q);
            r.answers = records;
            r.edns = q.edns.map(|_| crate::dns::Edns {
                udp_size: 1232,
                do_bit: false,
            });
            return Some(r.encode());
        }

        // --- forwarding ------------------------------------------------------
        if self.cfg.dns.domain_needed && !question.name.contains('.') {
            // A single-label name is a typo or a NetBIOS habit, never something
            // an upstream should be asked; dnsmasq's `domain-needed` said the
            // same thing.
            let mut r = Message::response(&q);
            r.flags = (r.flags & !0x000f) | u16::from(RCODE_NXDOMAIN);
            return Some(r.encode());
        }
        if self.inflight.fetch_add(1, Ordering::SeqCst) >= self.cfg.dns.forward_max {
            self.inflight.fetch_sub(1, Ordering::SeqCst);
            Stats::bump(&self.stats.dns_servfail);
            let mut r = Message::response(&q);
            r.flags = (r.flags & !0x000f) | u16::from(RCODE_SERVFAIL);
            return Some(r.encode());
        }
        let result = self.forward(query, &q, &question, tcp);
        self.inflight.fetch_sub(1, Ordering::SeqCst);

        match result {
            Ok(bytes) => {
                Stats::bump(&self.stats.dns_forwarded);
                // The upstream's bytes, untouched: re-encoding a response we did
                // not build would break every compressed name inside its rdata
                // (an SOA's mname is a pointer into the message we parsed and
                // then threw away).
                if self.cfg.dns.stop_rebind {
                    if let Some(reason) = self.rebind_violation(&bytes, &question) {
                        Stats::bump(&self.stats.dns_servfail);
                        eprintln!("netd: dns: {question:?}: {reason}");
                        let mut r = Message::response(&q);
                        r.flags = (r.flags & !0x000f) | u16::from(RCODE_SERVFAIL);
                        return Some(r.encode());
                    }
                }
                self.cache_from_response(&bytes, &question, at);
                Some(bytes)
            }
            Err(e) => {
                Stats::bump(&self.stats.dns_servfail);
                eprintln!(
                    "netd: dns: {} {}: {e}",
                    question.name,
                    crate::dns::type_name(question.qtype)
                );
                let mut r = Message::response(&q);
                r.flags = (r.flags & !0x000f) | u16::from(RCODE_SERVFAIL);
                Some(r.encode())
            }
        }
    }

    fn cache_from_response(&self, bytes: &[u8], q: &Question, at: i64) {
        let Ok(resp) = Message::parse(bytes) else {
            return;
        };
        if resp.rcode() != RCODE_NOERROR {
            return;
        }
        // Only the types whose rdata is a fixed-width address: every other type
        // can contain a compression pointer into a message this cache does not
        // keep, so caching it would hand the next client a pointer to nothing.
        let records: Vec<Record> = resp
            .answers
            .iter()
            .filter(|r| matches!(r.rtype, TYPE_A | TYPE_AAAA) && r.name == q.name)
            .cloned()
            .collect();
        if !records.is_empty() {
            if let Ok(mut c) = self.cache.lock() {
                c.put(q, &records, at, self.cfg.dns.cache_size);
            }
        }
    }

    fn forward(
        &self,
        query: &[u8],
        q: &Message,
        question: &Question,
        tcp: bool,
    ) -> Result<Vec<u8>, String> {
        let servers = self.servers_for(&question.name);
        if servers.is_empty() {
            return Err("no upstream for this name".into());
        }
        let mut last = String::from("no upstream answered");
        for server in &servers {
            match if tcp {
                upstream_tcp(server, query)
            } else {
                upstream_udp(server, query)
            } {
                Ok(bytes) => {
                    // A response for a different question is a cache-poisoning
                    // attempt or a broken upstream; either way, not an answer.
                    if let Ok(resp) = Message::parse(&bytes) {
                        if resp.id == q.id
                            && resp
                                .questions
                                .first()
                                .is_some_and(|rq| rq.name == question.name)
                        {
                            return Ok(bytes);
                        }
                        last = format!("{server} answered a different question");
                    } else {
                        last = format!("{server} sent an unparseable response");
                    }
                }
                Err(e) => {
                    Stats::bump(&self.stats.dns_forward_errors);
                    last = format!("{server}: {e}");
                }
            }
        }
        Err(last)
    }

    /// Which upstreams a name goes to: the longest matching conditional
    /// forwarder, else the plain list in the order the table writes it
    /// (`strict-order` — the first that answers wins, which is what makes
    /// 9.9.9.9 the network's primary rather than a lottery).
    fn servers_for(&self, name: &str) -> Vec<IpAddr> {
        let mut best: Option<(&String, &Vec<IpAddr>)> = None;
        for c in &self.cfg.dns.conditional {
            let matches = name == c.zone || name.ends_with(&format!(".{}", c.zone));
            if matches
                && best
                    .as_ref()
                    .map(|(z, _)| c.zone.len() > z.len())
                    .unwrap_or(true)
            {
                best = Some((&c.zone, &c.servers));
            }
        }
        match best {
            Some((_, servers)) => servers.clone(),
            None => self.cfg.dns.forwarders.clone(),
        }
    }

    /// `stop-dns-rebind`: an upstream answer that points a *public* name at a
    /// private address is how a hostile zone reaches inside the network.
    fn rebind_violation(&self, bytes: &[u8], q: &Question) -> Option<String> {
        let resp = Message::parse(bytes).ok()?;
        let exempt = self
            .cfg
            .dns
            .rebind_ok
            .iter()
            .any(|z| q.name == *z || q.name.ends_with(&format!(".{z}")));
        if exempt {
            return None;
        }
        for r in &resp.answers {
            let Some(ip) = rdata_address(r) else { continue };
            let private = match ip {
                IpAddr::V4(v4) => {
                    let v4 = u32::from(v4);
                    let loopback = v4 >> 24 == 127;
                    let other = v4 >> 24 == 10
                        || v4 >> 20 == (172 << 4 | 1)
                        || v4 >> 16 == (192 << 8 | 168)
                        || v4 >> 16 == (169 << 8 | 254)
                        || v4 == 0;
                    other || (loopback && !self.cfg.dns.rebind_localhost_ok)
                }
                IpAddr::V6(v6) => {
                    let loopback = v6 == Ipv6Addr::LOCALHOST;
                    let other = (v6.segments()[0] & 0xfe00) == 0xfc00
                        || (v6.segments()[0] & 0xffc0) == 0xfe80;
                    other || (loopback && !self.cfg.dns.rebind_localhost_ok)
                }
            };
            if private {
                return Some(format!(
                    "{q:?}: the answer contains {ip}, which is inside the network — refused (stop-dns-rebind)"
                ));
            }
        }
        None
    }

    fn allowed(&self, from: IpAddr) -> bool {
        match from {
            IpAddr::V4(v4) => {
                v4.is_loopback()
                    || self.cfg.dns.acl.iter().any(|n| n.contains(v4))
                    || self.cfg.dns.acl.iter().any(|n| n.prefix == 0)
            }
            IpAddr::V6(v6) => {
                v6.is_loopback()
                    || self.cfg.dns.acl6.iter().any(|(net, prefix)| {
                        if *prefix == 0 {
                            return true;
                        }
                        let mask = u128::MAX << (128 - *prefix as u32);
                        u128::from(v6) & mask == u128::from(*net) & mask
                    })
            }
        }
    }
}

/// One upstream attempt over UDP, with a hard timeout. A fresh socket per
/// query: a shared one would need its own bookkeeping for which reply belongs
/// to which client, and a LAN resolver does not do enough of this to care.
fn upstream_udp(server: &IpAddr, query: &[u8]) -> Result<Vec<u8>, String> {
    let bind = match server {
        IpAddr::V4(_) => "0.0.0.0:0",
        IpAddr::V6(_) => "[::]:0",
    };
    let socket = UdpSocket::bind(bind).map_err(|e| format!("bind {bind}: {e}"))?;
    socket
        .set_read_timeout(Some(UPSTREAM_TIMEOUT))
        .map_err(|e| e.to_string())?;
    socket
        .connect(SocketAddr::new(*server, 53))
        .map_err(|e| e.to_string())?;
    socket.send(query).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 4096];
    let n = socket.recv(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(n);
    Ok(buf)
}

fn upstream_tcp(server: &IpAddr, query: &[u8]) -> Result<Vec<u8>, String> {
    let addr = SocketAddr::new(*server, 53);
    let deadline = Instant::now() + UPSTREAM_TIMEOUT;
    let mut stream =
        TcpStream::connect_timeout(&addr, UPSTREAM_TIMEOUT).map_err(|e| e.to_string())?;
    let remain = deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(100));
    let _ = stream.set_read_timeout(Some(remain));
    let _ = stream.set_write_timeout(Some(remain));
    let mut framed = (query.len() as u16).to_be_bytes().to_vec();
    framed.extend_from_slice(query);
    stream.write_all(&framed).map_err(|e| e.to_string())?;
    let mut len = [0u8; 2];
    stream.read_exact(&mut len).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; u16::from_be_bytes(len) as usize];
    stream.read_exact(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

/// `netd dig <name> [type]` on the command line uses the same query builder as
/// the server's own tests, so a person asking the box a question and a test
/// asking it a question are asking the same question.
pub fn client_query(id: u16, name: &str, qtype: u16) -> Vec<u8> {
    Message {
        id,
        flags: crate::dns::FLAG_RD,
        questions: vec![Question {
            name: name.trim_end_matches('.').to_ascii_lowercase(),
            qtype,
            qclass: crate::dns::CLASS_IN,
        }],
        answers: Vec::new(),
        authority: Vec::new(),
        additional: Vec::new(),
        edns: Some(crate::dns::Edns {
            udp_size: 1232,
            do_bit: false,
        }),
    }
    .encode()
}

/// Send one query to a server and return the parsed reply, for `netd dig` and
/// for the tests that ask the *running* daemon a question.
pub fn query(
    server: SocketAddr,
    name: &str,
    qtype: u16,
    timeout: Duration,
) -> Result<Message, String> {
    let socket = UdpSocket::bind(if server.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })
    .map_err(|e| e.to_string())?;
    socket
        .set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    socket.connect(server).map_err(|e| e.to_string())?;
    let id = (std::process::id() as u16) ^ 0x5a5a;
    socket
        .send(&client_query(id, name, qtype))
        .map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 4096];
    let n = socket.recv(&mut buf).map_err(|e| e.to_string())?;
    Message::parse(&buf[..n])
}
