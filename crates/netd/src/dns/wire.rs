//! The DNS wire format, as much of it as a resolver needs: questions, resource
//! records with name compression, and just enough EDNS to read a client's UDP
//! size and to advertise ours.
//!
//! Written rather than pulled in because this crate takes no external
//! dependencies (AGENTS.md rule 1 / `crates/wan-manager`), and because the
//! failure modes that matter here are the ones a parser has to answer
//! *defensively*: a compression pointer that points at itself, a name that does
//! not fit in 255 bytes, a label longer than 63. Each of them is a panic in a
//! naive parser and a FORMERR in this one.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub const TYPE_A: u16 = 1;
pub const TYPE_NS: u16 = 2;
pub const TYPE_SOA: u16 = 6;
pub const TYPE_PTR: u16 = 12;
pub const TYPE_MX: u16 = 15;
pub const TYPE_TXT: u16 = 16;
pub const TYPE_AAAA: u16 = 28;
pub const TYPE_OPT: u16 = 41;
pub const TYPE_ANY: u16 = 255;
pub const CLASS_IN: u16 = 1;

pub const RCODE_NOERROR: u8 = 0;
pub const RCODE_FORMERR: u8 = 1;
pub const RCODE_SERVFAIL: u8 = 2;
pub const RCODE_NXDOMAIN: u8 = 3;
pub const RCODE_NOTIMP: u8 = 4;
pub const RCODE_REFUSED: u8 = 5;

pub const FLAG_QR: u16 = 0x8000;
pub const FLAG_AA: u16 = 0x0400;
pub const FLAG_TC: u16 = 0x0200;
pub const FLAG_RD: u16 = 0x0100;
pub const FLAG_RA: u16 = 0x0080;
const OPCODE_MASK: u16 = 0x7800;

#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    /// Canonical: lower-case, no trailing dot. The zone is keyed the same way.
    pub name: String,
    pub qtype: u16,
    pub qclass: u16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub name: String,
    pub rtype: u16,
    pub class: u16,
    pub ttl: u32,
    pub rdata: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edns {
    pub udp_size: u16,
    pub do_bit: bool,
}

#[derive(Clone, Debug)]
pub struct Message {
    pub id: u16,
    pub flags: u16,
    pub questions: Vec<Question>,
    pub answers: Vec<Record>,
    pub authority: Vec<Record>,
    pub additional: Vec<Record>,
    pub edns: Option<Edns>,
}

impl Message {
    pub fn rcode(&self) -> u8 {
        (self.flags & 0x000f) as u8
    }

    pub fn opcode(&self) -> u16 {
        (self.flags & OPCODE_MASK) >> 11
    }

    pub fn is_response(&self) -> bool {
        self.flags & FLAG_QR != 0
    }

    /// The size a UDP answer may be: 512 without EDNS, the client's advertised
    /// size (clamped) with it. dnsmasq's own clamp is 4096 and that is the one
    /// value here that is a policy rather than a protocol constant.
    pub fn udp_payload_size(&self) -> usize {
        match self.edns {
            Some(e) => (e.udp_size as usize).clamp(512, 4096),
            None => 512,
        }
    }

    pub fn parse(bytes: &[u8]) -> Result<Message, String> {
        if bytes.len() < 12 {
            return Err("shorter than a DNS header".into());
        }
        let id = u16::from_be_bytes([bytes[0], bytes[1]]);
        let flags = u16::from_be_bytes([bytes[2], bytes[3]]);
        let qd = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
        let an = u16::from_be_bytes([bytes[6], bytes[7]]) as usize;
        let ns = u16::from_be_bytes([bytes[8], bytes[9]]) as usize;
        let ar = u16::from_be_bytes([bytes[10], bytes[11]]) as usize;
        // A response with thousands of records is either a compression bomb or
        // not a response we asked for; the cap keeps the allocation honest.
        if qd > 64 || an > 4096 || ns > 4096 || ar > 4096 {
            return Err(format!(
                "implausible counts qd={qd} an={an} ns={ns} ar={ar}"
            ));
        }
        let mut pos = 12;
        let mut questions = Vec::with_capacity(qd);
        for _ in 0..qd {
            let (name, used) = read_name(bytes, pos)?;
            pos += used;
            if pos + 4 > bytes.len() {
                return Err("question truncated".into());
            }
            questions.push(Question {
                name,
                qtype: u16::from_be_bytes([bytes[pos], bytes[pos + 1]]),
                qclass: u16::from_be_bytes([bytes[pos + 2], bytes[pos + 3]]),
            });
            pos += 4;
        }
        let (answers, p) = read_records(bytes, pos, an)?;
        pos = p;
        let (authority, p) = read_records(bytes, pos, ns)?;
        pos = p;
        let (additional, _) = read_records(bytes, pos, ar)?;
        let mut edns = None;
        for r in &additional {
            if r.rtype == TYPE_OPT {
                // RFC 6891: class is the responder's UDP size, TTL packs
                // extended rcode, version and the DO bit.
                edns = Some(Edns {
                    udp_size: r.class,
                    do_bit: r.ttl & 0x8000 != 0,
                });
            }
        }
        Ok(Message {
            id,
            flags,
            questions,
            answers,
            authority,
            additional,
            edns,
        })
    }

    /// Encode as a response: same id, `QR` set, our flags decided by the caller.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(512);
        out.extend_from_slice(&self.id.to_be_bytes());
        out.extend_from_slice(&self.flags.to_be_bytes());
        out.extend_from_slice(&(self.questions.len() as u16).to_be_bytes());
        out.extend_from_slice(&(self.answers.len() as u16).to_be_bytes());
        out.extend_from_slice(&(self.authority.len() as u16).to_be_bytes());
        let ar = self.additional.len() + usize::from(self.edns.is_some());
        out.extend_from_slice(&(ar as u16).to_be_bytes());
        for q in &self.questions {
            write_name(&mut out, &q.name);
            out.extend_from_slice(&q.qtype.to_be_bytes());
            out.extend_from_slice(&q.qclass.to_be_bytes());
        }
        for r in self
            .answers
            .iter()
            .chain(self.authority.iter())
            .chain(self.additional.iter())
        {
            write_name(&mut out, &r.name);
            out.extend_from_slice(&r.rtype.to_be_bytes());
            out.extend_from_slice(&r.class.to_be_bytes());
            out.extend_from_slice(&r.ttl.to_be_bytes());
            out.extend_from_slice(&(r.rdata.len() as u16).to_be_bytes());
            out.extend_from_slice(&r.rdata);
        }
        if let Some(e) = self.edns {
            out.push(0); // root name
            out.extend_from_slice(&TYPE_OPT.to_be_bytes());
            out.extend_from_slice(&e.udp_size.max(512).to_be_bytes());
            let ttl: u32 = if e.do_bit { 0x8000 } else { 0 };
            out.extend_from_slice(&ttl.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
        }
        out
    }

    /// An empty response to `q`, carrying the question back (which clients
    /// require) and nothing else.
    pub fn response(q: &Message) -> Message {
        Message {
            id: q.id,
            flags: FLAG_QR | (q.flags & (FLAG_RD | OPCODE_MASK)) | FLAG_RA,
            questions: q.questions.clone(),
            answers: Vec::new(),
            authority: Vec::new(),
            additional: Vec::new(),
            edns: Some(Edns {
                udp_size: 1232,
                do_bit: false,
            }),
        }
    }
}

/// Read a possibly-compressed name. Returns the canonical form and how many
/// bytes of the *original* stream the name occupied (a pointer consumes two,
/// wherever it points).
pub fn read_name(bytes: &[u8], mut pos: usize) -> Result<(String, usize), String> {
    let mut labels: Vec<String> = Vec::new();
    let mut consumed = 0usize;
    let mut jumped = false;
    let mut hops = 0;
    let mut total = 0usize;
    loop {
        let len = *bytes
            .get(pos)
            .ok_or("name runs past the end of the message")? as usize;
        if len == 0 {
            if !jumped {
                consumed += 1;
            }
            break;
        }
        if len & 0xc0 == 0xc0 {
            let second = *bytes
                .get(pos + 1)
                .ok_or("compression pointer runs past the end")? as usize;
            let target = ((len & 0x3f) << 8) | second;
            if !jumped {
                consumed += 2;
            }
            hops += 1;
            // A pointer that points forward, or a chain that never terminates,
            // is a malformed message — and a parser that follows it is a
            // resolver that hangs.
            if hops > 32 || target >= bytes.len() || target >= pos {
                return Err("compression pointer loops or points forward".into());
            }
            pos = target;
            jumped = true;
            continue;
        }
        if len > 63 {
            return Err(format!("label length {len} is over 63"));
        }
        let end = pos + 1 + len;
        let label = bytes
            .get(pos + 1..end)
            .ok_or("label runs past the end of the message")?;
        total += len + 1;
        if total > 255 {
            return Err("name is over 255 bytes".into());
        }
        labels.push(String::from_utf8_lossy(label).to_ascii_lowercase());
        if !jumped {
            consumed += len + 1;
        }
        pos = end;
    }
    Ok((labels.join("."), consumed))
}

fn read_records(
    bytes: &[u8],
    mut pos: usize,
    count: usize,
) -> Result<(Vec<Record>, usize), String> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let (name, used) = read_name(bytes, pos)?;
        pos += used;
        if pos + 10 > bytes.len() {
            return Err("record header truncated".into());
        }
        let rtype = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]);
        let class = u16::from_be_bytes([bytes[pos + 2], bytes[pos + 3]]);
        let ttl = u32::from_be_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]);
        let rdlen = u16::from_be_bytes([bytes[pos + 8], bytes[pos + 9]]) as usize;
        pos += 10;
        let rdata = bytes
            .get(pos..pos + rdlen)
            .ok_or("record data runs past the end of the message")?
            .to_vec();
        pos += rdlen;
        out.push(Record {
            name,
            rtype,
            class,
            ttl,
            rdata,
        });
    }
    Ok((out, pos))
}

/// Write a name with no compression. Compression would save bytes; it would
/// also mean a table of offsets whose entries can be invalidated by every
/// append, which is exactly the kind of bookkeeping a resolver does not need to
/// do to save 30 bytes on a LAN with a 1232-byte budget.
pub fn write_name(out: &mut Vec<u8>, name: &str) {
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() {
            continue;
        }
        let b = label.as_bytes();
        let n = b.len().min(63);
        out.push(n as u8);
        out.extend_from_slice(&b[..n]);
    }
    out.push(0);
}

// --- record constructors ----------------------------------------------------

pub fn a_record(name: &str, ip: Ipv4Addr, ttl: u32) -> Record {
    Record {
        name: name.to_string(),
        rtype: TYPE_A,
        class: CLASS_IN,
        ttl,
        rdata: ip.octets().to_vec(),
    }
}

pub fn aaaa_record(name: &str, ip: Ipv6Addr, ttl: u32) -> Record {
    Record {
        name: name.to_string(),
        rtype: TYPE_AAAA,
        class: CLASS_IN,
        ttl,
        rdata: ip.octets().to_vec(),
    }
}

pub fn ptr_record(name: &str, target: &str, ttl: u32) -> Record {
    let mut rdata = Vec::new();
    write_name(&mut rdata, target);
    Record {
        name: name.to_string(),
        rtype: TYPE_PTR,
        class: CLASS_IN,
        ttl,
        rdata,
    }
}

pub fn ns_record(name: &str, target: &str, ttl: u32) -> Record {
    let mut rdata = Vec::new();
    write_name(&mut rdata, target);
    Record {
        name: name.to_string(),
        rtype: TYPE_NS,
        class: CLASS_IN,
        ttl,
        rdata,
    }
}

#[allow(clippy::too_many_arguments)] // the SOA is five RFC numbers plus a name,
                                     // a mailbox, a serial and a TTL; grouping them
                                     // would be a struct nobody reads
pub fn soa_record(
    name: &str,
    mname: &str,
    rname: &str,
    serial: u32,
    refresh: u32,
    retry: u32,
    expire: u32,
    minimum: u32,
    ttl: u32,
) -> Record {
    let mut rdata = Vec::new();
    write_name(&mut rdata, mname);
    write_name(&mut rdata, rname);
    for n in [serial, refresh, retry, expire, minimum] {
        rdata.extend_from_slice(&n.to_be_bytes());
    }
    Record {
        name: name.to_string(),
        rtype: TYPE_SOA,
        class: CLASS_IN,
        ttl,
        rdata,
    }
}

/// `10.3.1.20` → `20.1.3.10.in-addr.arpa`, the name a PTR query asks for.
#[allow(dead_code)] // the inverse of `zone::reverse_to_ip`, used by the test that
                    // proves the pair round-trips; nothing else builds a
                    // reverse name because PTR queries arrive with one.
pub fn reverse_v4(ip: Ipv4Addr) -> String {
    let o = ip.octets();
    format!("{}.{}.{}.{}.in-addr.arpa", o[3], o[2], o[1], o[0])
}

/// An IPv6 address's `ip6.arpa` name: 32 nibbles, reversed.
#[allow(dead_code)]
pub fn reverse_v6(ip: Ipv6Addr) -> String {
    let mut out = String::with_capacity(72);
    for byte in ip.octets().iter().rev() {
        out.push_str(&format!("{:x}.{:x}.", byte & 0x0f, byte >> 4));
    }
    out.push_str("ip6.arpa");
    out
}

/// An `A`/`AAAA` record's address, for the rebind check and for logging.
pub fn rdata_address(r: &Record) -> Option<IpAddr> {
    match r.rtype {
        TYPE_A if r.rdata.len() == 4 => Some(IpAddr::V4(Ipv4Addr::new(
            r.rdata[0], r.rdata[1], r.rdata[2], r.rdata[3],
        ))),
        TYPE_AAAA if r.rdata.len() == 16 => {
            let mut b = [0u8; 16];
            b.copy_from_slice(&r.rdata);
            Some(IpAddr::V6(Ipv6Addr::from(b)))
        }
        _ => None,
    }
}

/// The type of a query, as the word an operator types (`netd explain --dns`).
pub fn type_name(t: u16) -> &'static str {
    match t {
        TYPE_A => "A",
        TYPE_NS => "NS",
        TYPE_SOA => "SOA",
        TYPE_PTR => "PTR",
        TYPE_MX => "MX",
        TYPE_TXT => "TXT",
        TYPE_AAAA => "AAAA",
        TYPE_OPT => "OPT",
        TYPE_ANY => "ANY",
        _ => "?",
    }
}
