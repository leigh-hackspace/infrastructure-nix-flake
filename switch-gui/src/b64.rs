//! base64url (RFC 4648 §5) without padding, plus random bytes — the two small
//! primitives the OIDC flow in [`crate::oidc`] needs that std does not provide.
//!
//! Only the URL-safe alphabet is here because that is all this flow needs:
//! RFC 7636 defines the PKCE code challenge in it, and authentik compares the
//! challenge **unpadded** — it recomputes
//! `urlsafe_b64encode(sha256(verifier)).replace("=","")` and tests for
//! equality, so a padded challenge is accepted by `/authorize/` and then
//! rejected by `/token/` with `invalid_grant`.  The client secret goes to the
//! token endpoint in an HTTP `Basic` header, which curl builds itself, so the
//! standard alphabet is never needed here.

use std::fs::File;
use std::io::Read;

use crate::R;

const URL_SAFE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// base64 with the URL-and-filename-safe alphabet and **no** `=` padding.
pub fn url_unpadded(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        // One character per whole input byte: a 2-byte tail emits 3 characters
        // and a 1-byte tail emits 2, which is what "unpadded" means.
        let whole = chunk.len() * 8 / 6 + usize::from(chunk.len() * 8 % 6 > 0);
        for i in 0..whole {
            out.push(URL_SAFE[(n >> (18 - 6 * i) as u32) as usize & 63] as char);
        }
    }
    out
}

/// `n` bytes from the OS CSPRNG.
///
/// `getrandom(2)` would be the tidy answer but it is a syscall, and reaching it
/// without the `getrandom` crate means `libc`; `/dev/urandom` is the same
/// entropy on Linux and needs no dependency.  These bytes become a PKCE
/// verifier, an OAuth `state` and a session cookie, so a failure here must
/// never be papered over — hence an error rather than a fallback.
pub fn random_bytes(n: usize) -> R<Vec<u8>> {
    let mut f = File::open("/dev/urandom").map_err(|e| format!("open /dev/urandom: {e}"))?;
    let mut buf = vec![0u8; n];
    let mut filled = 0;
    while filled < n {
        match f.read(&mut buf[filled..]) {
            Ok(0) => return Err("/dev/urandom returned EOF".to_string()),
            Ok(k) => filled += k,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("read /dev/urandom: {e}")),
        }
    }
    Ok(buf)
}

/// Lowercase hex of `n` random bytes — session ids and OAuth `state`.
pub fn random_hex(n: usize) -> R<String> {
    Ok(crate::sha256::hex(&random_bytes(n)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The RFC 4648 §10 vectors, with the padding removed: the alphabet is
    /// what changes for the last two characters (62 and 63 are `-` and `_`).
    #[test]
    fn rfc4648_vectors_without_padding() {
        assert_eq!(url_unpadded(b""), "");
        assert_eq!(url_unpadded(b"f"), "Zg");
        assert_eq!(url_unpadded(b"fo"), "Zm8");
        assert_eq!(url_unpadded(b"foo"), "Zm9v");
        assert_eq!(url_unpadded(b"foob"), "Zm9vYg");
        assert_eq!(url_unpadded(b"fooba"), "Zm9vYmE");
        assert_eq!(url_unpadded(b"foobar"), "Zm9vYmFy");
    }

    /// 0xFB 0xFF 0xBF makes indexes 62 and 63 appear, which is the only place
    /// the URL-safe alphabet differs from the standard one.
    #[test]
    fn the_url_safe_alphabet_is_used() {
        assert_eq!(url_unpadded(&[0xfb, 0xff, 0xbf]), "-_-_");
        assert!(!url_unpadded(&[0xfb, 0xff, 0xbf]).contains('+'));
    }

    #[test]
    fn never_pads() {
        for n in 0..12 {
            let s = url_unpadded(&vec![b'x'; n]);
            assert!(!s.contains('='), "{n} bytes produced padding: {s}");
        }
    }

    #[test]
    fn random_lengths() {
        assert_eq!(random_bytes(32).unwrap().len(), 32);
        assert_eq!(random_hex(16).unwrap().len(), 32);
    }

    #[test]
    fn random_bytes_are_not_all_zero() {
        // Not a proof of randomness; it catches a read loop that returns
        // without filling the buffer.
        assert!(random_bytes(64).unwrap().iter().any(|&b| b != 0));
    }

    #[test]
    fn random_bytes_are_distinct() {
        assert_ne!(random_bytes(32).unwrap(), random_bytes(32).unwrap());
    }
}
