//! Base64, standard alphabet with padding, in 90 lines of stdlib Rust.
//!
//! The API's audio goes over JSON, so every PCM window and every spoken answer
//! crosses the socket as text: `application/wav` bodies would need a preflight for
//! nothing gained, and a browser's `FileReader`/`btoa` path is one line either way.
//!
//! Only what this service needs is implemented — encode, decode, and a decoder that
//! **refuses** anything it cannot prove is base64.  The refusal matters: the input
//! is attacker-reachable (any LAN client can POST to this socket), and a lenient
//! decoder that skips invalid characters will happily turn a truncated stream into
//! a different, shorter, still-parseable one — so a half-sent window would be
//! transcribed as if it were the whole phrase.  A hard error here reads as a bad
//! request, which is what it is.

/// 64 bits per character, in the standard (not URL-safe) alphabet, which is what
/// `btoa()` produces and what `atob()` reads.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Decode, or say why not.  Padding must be well-formed (`AAB=` is fine,
/// `AA==A` and `AA=A` are not), and a length that is not 0 mod 4 is rejected
/// rather than floored: 25 characters is not a thing.
pub fn decode(text: &str) -> Result<Vec<u8>, String> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    if !bytes.len().is_multiple_of(4) {
        return Err(format!(
            "base64 length {} is not a multiple of 4",
            bytes.len()
        ));
    }
    let mut table = [255u8; 256];
    for (i, c) in ALPHABET.iter().enumerate() {
        table[*c as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (chunk_no, chunk) in bytes.chunks(4).enumerate() {
        let last = chunk_no + 1 == bytes.len() / 4;
        let mut pad = 0usize;
        let mut vals = [0u8; 4];
        for (i, c) in chunk.iter().enumerate() {
            if *c == b'=' {
                // Padding only in the final group, only at the end of it, and at
                // most two of it — the three rules that make a valid string valid.
                if !last || i < 2 || chunk[i..].iter().any(|c| *c != b'=') {
                    return Err(format!("misplaced '=' in base64 at offset {}", chunk_no * 4 + i));
                }
                // The first '=' fixes the padding length; later ones must not
                // recompute it, or "Cw==" would be read as one pad instead of two
                // and every length-1 payload would decode to two bytes.
                if pad == 0 {
                    pad = 4 - i;
                }
                vals[i] = 0;
                continue;
            }
            let v = table[*c as usize];
            if v == 255 {
                return Err(format!(
                    "not a base64 byte: {:?} at offset {}",
                    *c as char,
                    chunk_no * 4 + i
                ));
            }
            vals[i] = v;
        }
        let n = (vals[0] as u32) << 18 | (vals[1] as u32) << 12 | (vals[2] as u32) << 6 | vals[3] as u32;
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_length() {
        // 0..=12 covers all three padding shapes twice over, which is where an
        // off-by-one in the chunking lives.
        for len in 0..13usize {
            let raw: Vec<u8> = (0..len as u64).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(decode(&encode(&raw)).unwrap(), raw, "len {len}");
        }
    }

    #[test]
    fn matches_the_known_vectors() {
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"foob"), "Zm9vYg==");
        assert_eq!(decode("Zm9vYmFy").unwrap(), b"foobar");
    }

    #[test]
    fn refuses_what_a_browser_would_have_silently_mangled() {
        assert!(decode("Zm9vY").is_err(), "length not a multiple of 4");
        assert!(decode("Zm9v*").is_err(), "byte outside the alphabet");
        assert!(decode("Z=9v").is_err(), "padding in the middle");
        assert!(decode("Zm=").is_err(), "one pad char with a bad length");
        assert!(decode("Zm==").is_ok());
        assert!(decode("Zm9=").is_ok());
    }

    #[test]
    fn a_full_worth_of_audio_bytes_round_trips() {
        // 3 s of 16 kHz mono PCM16 is the real payload; 96 640 is not a multiple of
        // 3, so this is the odd tail in production too.
        let raw: Vec<u8> = (0..96_640u64).map(|i| (i % 251) as u8).collect();
        let text = encode(&raw);
        assert_eq!(text.len(), raw.len().div_ceil(3) * 4);
        assert_eq!(decode(&text).unwrap().len(), raw.len());
    }
}
