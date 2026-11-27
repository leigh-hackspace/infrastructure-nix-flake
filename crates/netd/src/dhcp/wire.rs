//! The wire format: DHCPv4 packets and the option encoders, with no policy in
//! sight. `decision.rs` never sees a byte and this file never sees a config.

use crate::config::mac_to_string;
use std::net::Ipv4Addr;

// --- option codes, named ----------------------------------------------------

pub const OPT_PAD: u8 = 0;
pub const OPT_SUBNET_MASK: u8 = 1;
pub const OPT_ROUTER: u8 = 3;
pub const OPT_DNS: u8 = 6;
pub const OPT_HOSTNAME: u8 = 12;
pub const OPT_DOMAIN: u8 = 15;
pub const OPT_BROADCAST: u8 = 28;
pub const OPT_REQUESTED_IP: u8 = 50;
pub const OPT_LEASE_TIME: u8 = 51;
pub const OPT_OVERLOAD: u8 = 52;
pub const OPT_MSG_TYPE: u8 = 53;
pub const OPT_SERVER_ID: u8 = 54;
pub const OPT_PARAM_REQ: u8 = 55;
pub const OPT_MAX_SIZE: u8 = 57;
pub const OPT_CLIENT_ID: u8 = 61;
pub const OPT_TFTP_SERVER: u8 = 66;
pub const OPT_BOOTFILE: u8 = 67;
pub const OPT_USER_CLASS: u8 = 77;
pub const OPT_CLIENT_ARCH: u8 = 93;
pub const OPT_END: u8 = 255;

pub const MSG_DISCOVER: u8 = 1;
pub const MSG_OFFER: u8 = 2;
pub const MSG_REQUEST: u8 = 3;
pub const MSG_DECLINE: u8 = 4;
pub const MSG_ACK: u8 = 5;
pub const MSG_NAK: u8 = 6;
pub const MSG_RELEASE: u8 = 7;
pub const MSG_INFORM: u8 = 8;

pub const MAGIC: [u8; 4] = [99, 130, 83, 99];
pub const BOOTREQUEST: u8 = 1;
pub const BOOTREPLY: u8 = 2;
/// RFC 2131 §2: bit 15 of the flags field. Named because `flags & 0x8000` at a
/// call site reads as a magic number, and this has been wrong in more than one
/// DHCP server.
pub const FLAG_BROADCAST: u16 = 0x8000;
pub const HTYPE_ETHERNET: u8 = 1;
const HEADER_LEN: usize = 236;

/// A message type as a word, for the journal and `netd probe`.
pub fn msg_type_name(t: u8) -> &'static str {
    match t {
        MSG_DISCOVER => "DISCOVER",
        MSG_OFFER => "OFFER",
        MSG_REQUEST => "REQUEST",
        MSG_DECLINE => "DECLINE",
        MSG_ACK => "ACK",
        MSG_NAK => "NAK",
        MSG_RELEASE => "RELEASE",
        MSG_INFORM => "INFORM",
        _ => "unknown",
    }
}

// ---------------------------------------------------------------------------
// the packet
// ---------------------------------------------------------------------------

/// A BOOTP/DHCP message, parsed into fields rather than left as bytes.
#[derive(Clone, Debug)]
pub struct Message {
    pub op: u8,
    pub htype: u8,
    pub hlen: u8,
    pub hops: u8,
    pub xid: u32,
    pub secs: u16,
    pub flags: u16,
    pub ciaddr: Ipv4Addr,
    pub yiaddr: Ipv4Addr,
    pub siaddr: Ipv4Addr,
    pub giaddr: Ipv4Addr,
    pub chaddr: [u8; 16],
    pub options: Vec<(u8, Vec<u8>)>,
}

impl Message {
    pub fn parse(bytes: &[u8]) -> Result<Message, String> {
        if bytes.len() < HEADER_LEN + 4 {
            return Err(format!(
                "{} bytes is shorter than a BOOTP header",
                bytes.len()
            ));
        }
        let be16 = |o: usize| u16::from_be_bytes([bytes[o], bytes[o + 1]]);
        let be32 =
            |o: usize| u32::from_be_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        if bytes[HEADER_LEN..HEADER_LEN + 4] != MAGIC {
            return Err("no DHCP magic cookie — this is BOOTP or garbage".into());
        }
        let mut options = Vec::new();
        parse_options(&bytes[HEADER_LEN + 4..], &mut options)?;
        // Option 52: the message ran out of room and continued in `sname`
        // and/or `file`. A request whose client-id is in `file` is a request we
        // would otherwise answer for the wrong device.
        if let Some(v) = options
            .iter()
            .find(|(c, _)| *c == OPT_OVERLOAD)
            .and_then(|(_, v)| v.first())
            .copied()
        {
            if v & 1 != 0 {
                parse_options(&bytes[44..44 + 64], &mut options)?;
            }
            if v & 2 != 0 {
                parse_options(&bytes[108..108 + 128], &mut options)?;
            }
        }
        Ok(Message {
            op: bytes[0],
            htype: bytes[1],
            hlen: bytes[2],
            hops: bytes[3],
            xid: be32(4),
            secs: be16(8),
            flags: be16(10),
            ciaddr: Ipv4Addr::from(be32(12)),
            yiaddr: Ipv4Addr::from(be32(16)),
            siaddr: Ipv4Addr::from(be32(20)),
            giaddr: Ipv4Addr::from(be32(24)),
            chaddr: {
                let mut c = [0u8; 16];
                c.copy_from_slice(&bytes[28..44]);
                c
            },
            options,
        })
    }

    pub fn opts(&self, code: u8) -> Vec<&[u8]> {
        self.options
            .iter()
            .filter(|(c, _)| *c == code)
            .map(|(_, v)| v.as_slice())
            .collect()
    }

    pub fn opt(&self, code: u8) -> Option<&[u8]> {
        self.options
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, v)| v.as_slice())
    }

    pub fn msg_type(&self) -> Option<u8> {
        self.opt(OPT_MSG_TYPE).and_then(|v| v.first().copied())
    }

    pub fn opt_ip(&self, code: u8) -> Option<Ipv4Addr> {
        let v = self.opt(code)?;
        (v.len() == 4).then(|| Ipv4Addr::new(v[0], v[1], v[2], v[3]))
    }

    pub fn opt_u16(&self, code: u8) -> Option<u16> {
        let v = self.opt(code)?;
        (v.len() == 2).then(|| u16::from_be_bytes([v[0], v[1]]))
    }

    pub fn opt_str(&self, code: u8) -> Option<String> {
        let v = self.opt(code)?;
        Some(String::from_utf8_lossy(v).trim_matches('\0').to_string())
    }

    /// The client's hardware address, when the packet says it has one.
    pub fn mac(&self) -> Result<crate::config::Mac, String> {
        if self.htype != HTYPE_ETHERNET || self.hlen != 6 {
            return Err(format!(
                "htype {} hlen {} is not an Ethernet client",
                self.htype, self.hlen
            ));
        }
        let mut m = [0u8; 6];
        m.copy_from_slice(&self.chaddr[..6]);
        Ok(m)
    }
}

fn parse_options(bytes: &[u8], out: &mut Vec<(u8, Vec<u8>)>) -> Result<(), String> {
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            OPT_PAD => i += 1,
            OPT_END => return Ok(()),
            code => {
                let len = *bytes.get(i + 1).ok_or_else(|| {
                    format!("option {code} has no length byte — the packet is truncated")
                })? as usize;
                let end = i + 2 + len;
                if end > bytes.len() {
                    return Err(format!(
                        "option {code} claims {len} bytes and the packet ends first"
                    ));
                }
                out.push((code, bytes[i + 2..end].to_vec()));
                i = end;
            }
        }
    }
    Ok(())
}

/// A packet being built — a reply in the server's path, a request in `probe`'s.
pub struct Reply {
    pub op: u8,
    pub xid: u32,
    pub flags: u16,
    pub ciaddr: Ipv4Addr,
    pub yiaddr: Ipv4Addr,
    pub siaddr: Ipv4Addr,
    pub chaddr: [u8; 16],
    pub file_name: Option<String>,
    pub options: Vec<(u8, Vec<u8>)>,
}

impl Reply {
    pub fn new(req: &Message) -> Reply {
        Reply {
            op: BOOTREPLY,
            xid: req.xid,
            flags: req.flags,
            ciaddr: req.ciaddr,
            yiaddr: Ipv4Addr::UNSPECIFIED,
            siaddr: Ipv4Addr::UNSPECIFIED,
            chaddr: req.chaddr,
            file_name: None,
            options: Vec::new(),
        }
    }

    /// A client's request, for `netd probe`: same encoder, so a packet the probe
    /// sends and a packet the server parses cannot disagree about the format.
    pub fn request(xid: u32, mac: &crate::config::Mac) -> Reply {
        let mut chaddr = [0u8; 16];
        chaddr[..6].copy_from_slice(mac);
        Reply {
            op: BOOTREQUEST,
            xid,
            flags: FLAG_BROADCAST,
            ciaddr: Ipv4Addr::UNSPECIFIED,
            yiaddr: Ipv4Addr::UNSPECIFIED,
            siaddr: Ipv4Addr::UNSPECIFIED,
            chaddr,
            file_name: None,
            options: Vec::new(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(300);
        out.push(self.op);
        out.push(HTYPE_ETHERNET);
        out.push(6);
        out.push(0); // hops: this box is never a relay
        out.extend_from_slice(&self.xid.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes()); // secs
        out.extend_from_slice(&self.flags.to_be_bytes());
        out.extend_from_slice(&self.ciaddr.octets());
        out.extend_from_slice(&self.yiaddr.octets());
        out.extend_from_slice(&self.siaddr.octets());
        out.extend_from_slice(&[0, 0, 0, 0]); // giaddr
        out.extend_from_slice(&self.chaddr);
        out.extend_from_slice(&[0u8; 64]); // sname
        let mut file = [0u8; 128];
        if let Some(f) = &self.file_name {
            let b = f.as_bytes();
            let n = b.len().min(127);
            file[..n].copy_from_slice(&b[..n]);
        }
        out.extend_from_slice(&file);
        out.extend_from_slice(&MAGIC);
        for (code, value) in &self.options {
            let n = value.len().min(255);
            out.push(*code);
            out.push(n as u8);
            out.extend_from_slice(&value[..n]);
        }
        out.push(OPT_END);
        // RFC 1542's minimum; old PXE ROMs are happier with the padding, and
        // dnsmasq pads too.
        while out.len() < 300 {
            out.push(OPT_PAD);
        }
        out
    }
}

// --- option encoders --------------------------------------------------------

pub fn enc_ip(ip: Ipv4Addr) -> Vec<u8> {
    ip.octets().to_vec()
}

pub fn enc_ips(ips: &[Ipv4Addr]) -> Vec<u8> {
    ips.iter().flat_map(|i| i.octets()).collect()
}

pub fn enc_u32(v: u32) -> Vec<u8> {
    v.to_be_bytes().to_vec()
}

pub fn enc_str(s: &str) -> Vec<u8> {
    s.as_bytes().to_vec()
}

/// Option 119 (domain search), RFC 3397: length-prefixed labels, each name
/// terminated by a zero. Worth implementing rather than approximating because a
/// wrong encoding here silently gives clients a search list of garbage.
pub fn enc_domain_search(names: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    for name in names {
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
    out
}

/// Every option that mattered, decoded, for `netd probe` and the journal.
pub fn describe_request(req: &Message) -> Vec<(String, String)> {
    let mut rows = vec![
        (
            "op".to_string(),
            if req.op == BOOTREQUEST {
                "request".to_string()
            } else {
                format!("op {}", req.op)
            },
        ),
        ("xid".to_string(), format!("{:#x}", req.xid)),
        ("hops".to_string(), req.hops.to_string()),
        ("secs".to_string(), req.secs.to_string()),
        (
            "mac".to_string(),
            mac_to_string(&req.chaddr[..6].try_into().unwrap_or([0; 6])),
        ),
        ("yiaddr".to_string(), req.yiaddr.to_string()),
        ("siaddr".to_string(), req.siaddr.to_string()),
    ];
    for (code, value) in &req.options {
        let name = match *code {
            OPT_MSG_TYPE => "message-type",
            OPT_SERVER_ID => "server-id",
            OPT_REQUESTED_IP => "requested-ip",
            OPT_CLIENT_ID => "client-id",
            OPT_HOSTNAME => "hostname",
            OPT_LEASE_TIME => "lease-time",
            OPT_PARAM_REQ => "param-request",
            OPT_MAX_SIZE => "max-size",
            OPT_CLIENT_ARCH => "client-arch",
            OPT_USER_CLASS => "user-class",
            OPT_SUBNET_MASK => "subnet-mask",
            OPT_ROUTER => "router",
            OPT_DNS => "dns",
            OPT_DOMAIN => "domain",
            OPT_BOOTFILE => "bootfile",
            OPT_TFTP_SERVER => "tftp-server",
            _ => "",
        };
        let decoded = match *code {
            OPT_MSG_TYPE => match value.first() {
                Some(v) => msg_type_name(*v).to_string(),
                None => "empty".into(),
            },
            OPT_SERVER_ID | OPT_REQUESTED_IP | OPT_SUBNET_MASK | OPT_ROUTER | OPT_DNS
                if value.len() == 4 =>
            {
                Ipv4Addr::new(value[0], value[1], value[2], value[3]).to_string()
            }
            OPT_CLIENT_ID => value
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(":"),
            OPT_CLIENT_ARCH => value
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| u16::from_be_bytes(*p).to_string())
                .collect::<Vec<_>>()
                .join(","),
            OPT_HOSTNAME | OPT_USER_CLASS | OPT_DOMAIN | OPT_BOOTFILE | OPT_TFTP_SERVER => {
                String::from_utf8_lossy(value).to_string()
            }
            _ => value
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(":"),
        };
        let label = if name.is_empty() {
            format!("option {code}")
        } else {
            format!("{name} ({code})")
        };
        rows.push((label, decoded));
    }
    rows
}
