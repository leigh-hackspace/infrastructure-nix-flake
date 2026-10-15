//! Minimal RFC 6455 WebSocket server: handshake, framing, and control frames.
//!
//! Server-side framing rules implemented here:
//!   - client frames MUST be masked (we reject unmasked frames)
//!   - server frames MUST NOT be masked
//!   - control frames are never fragmented and are capped at 125 bytes
//!   - ping is answered with a pong carrying the same payload
//!   - close is echoed, then the connection is torn down

use std::io::{self, Read, Write};
use std::net::TcpStream;

/// Magic GUID appended to Sec-WebSocket-Key before hashing (RFC 6455 §4.2.2).
pub const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

pub const OP_CONT: u8 = 0x0;
pub const OP_TEXT: u8 = 0x1;
pub const OP_BINARY: u8 = 0x2;
pub const OP_CLOSE: u8 = 0x8;
pub const OP_PING: u8 = 0x9;
pub const OP_PONG: u8 = 0xA;

/// Cap on a single frame payload (64 MiB) — audio appends are big but sane.
const MAX_FRAME_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug)]
pub enum WsError {
    Io(io::Error),
    Protocol(String),
    /// Peer sent (or we echo) a close frame.
    Closed { code: u16, reason: String },
}

impl From<io::Error> for WsError {
    fn from(e: io::Error) -> Self {
        WsError::Io(e)
    }
}

/// `Sec-WebSocket-Accept` for a client `Sec-WebSocket-Key`.
pub fn accept_key(key: &str) -> String {
    let mut data = key.as_bytes().to_vec();
    data.extend_from_slice(GUID.as_bytes());
    crate::base64::encode(&crate::sha1::sha1(&data))
}

/// One complete WebSocket message from the client (after de-fragmentation).
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
}

struct Frame {
    opcode: u8,
    fin: bool,
    payload: Vec<u8>,
}

fn read_frame(stream: &mut TcpStream) -> Result<Frame, WsError> {
    let mut hdr = [0u8; 2];
    stream.read_exact(&mut hdr)?;
    let fin = hdr[0] & 0x80 != 0;
    let opcode = hdr[0] & 0x0F;
    let masked = hdr[1] & 0x80 != 0;
    let mut len = u64::from(hdr[1] & 0x7F);
    if len == 126 {
        let mut b = [0u8; 2];
        stream.read_exact(&mut b)?;
        len = u64::from(u16::from_be_bytes(b));
    } else if len == 127 {
        let mut b = [0u8; 8];
        stream.read_exact(&mut b)?;
        len = u64::from_be_bytes(b);
    }
    if len > MAX_FRAME_BYTES {
        return Err(WsError::Protocol("frame too large".into()));
    }
    if opcode >= 0x8 && len > 125 {
        return Err(WsError::Protocol("control frame payload too long".into()));
    }
    if !masked {
        return Err(WsError::Protocol("client frame was not masked".into()));
    }
    let mut mask = [0u8; 4];
    stream.read_exact(&mut mask)?;
    let mut payload = vec![0u8; len as usize];
    stream.read_exact(&mut payload)?;
    for (i, b) in payload.iter_mut().enumerate() {
        *b ^= mask[i % 4];
    }
    Ok(Frame { opcode, fin, payload })
}

/// Read a full message, transparently answering pings and echoing closes.
pub fn read_message(stream: &mut TcpStream) -> Result<Message, WsError> {
    // (opcode, accumulated payload) for an in-flight fragmented message.
    let mut frag: Option<(u8, Vec<u8>)> = None;
    loop {
        let frame = read_frame(stream)?;
        match frame.opcode {
            OP_PING => {
                write_frame(stream, OP_PONG, &frame.payload)?;
            }
            OP_PONG => {}
            OP_CLOSE => {
                let (code, reason) = if frame.payload.len() >= 2 {
                    (
                        u16::from_be_bytes([frame.payload[0], frame.payload[1]]),
                        String::from_utf8_lossy(&frame.payload[2..]).to_string(),
                    )
                } else {
                    (1000, String::new())
                };
                // Echo the close frame back, then let the caller tear down.
                let _ = write_frame(stream, OP_CLOSE, &frame.payload);
                return Err(WsError::Closed { code, reason });
            }
            OP_TEXT | OP_BINARY => {
                if !frame.fin {
                    frag = Some((frame.opcode, frame.payload));
                    continue;
                }
                return finish_message(frame.opcode, frame.payload);
            }
            OP_CONT => {
                let (op, mut acc) = frag.take().ok_or_else(|| WsError::Protocol("continuation without start".into()))?;
                acc.extend_from_slice(&frame.payload);
                if frame.fin {
                    return finish_message(op, acc);
                }
                frag = Some((op, acc));
            }
            other => return Err(WsError::Protocol(format!("unknown opcode 0x{other:x}"))),
        }
    }
}

fn finish_message(opcode: u8, payload: Vec<u8>) -> Result<Message, WsError> {
    match opcode {
        OP_TEXT => {
            let s = String::from_utf8(payload)
                .map_err(|_| WsError::Protocol("text message is not valid UTF-8".into()))?;
            Ok(Message::Text(s))
        }
        OP_BINARY => Ok(Message::Binary(payload)),
        _ => Err(WsError::Protocol("bad message type".into())),
    }
}

/// Write a single (unmasked, FIN) server frame.
pub fn write_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8]) -> io::Result<()> {
    let mut out = Vec::with_capacity(payload.len() + 10);
    out.push(0x80 | opcode);
    let len = payload.len();
    if len < 126 {
        out.push(len as u8);
    } else if len <= 0xFFFF {
        out.push(126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    out.extend_from_slice(payload);
    stream.write_all(&out)?;
    stream.flush()
}

pub fn write_text(stream: &mut TcpStream, s: &str) -> io::Result<()> {
    write_frame(stream, OP_TEXT, s.as_bytes())
}

pub fn write_close(stream: &mut TcpStream, code: u16, reason: &str) -> io::Result<()> {
    let mut payload = code.to_be_bytes().to_vec();
    payload.extend_from_slice(reason.as_bytes());
    write_frame(stream, OP_CLOSE, &payload)
}
