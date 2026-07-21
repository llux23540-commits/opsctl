//! RFC 6238 TOTP (HMAC-SHA1, 6 digits, 30s step) — compatible with Google
//! Authenticator / Authy / 1Password. Secrets are base32 (RFC 4648, no pad).

use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, Mac};
use sha1::Sha1;

type HmacSha1 = Hmac<Sha1>;

const STEP: i64 = 30;
const DIGITS: u32 = 6;

/// Generate a new random base32 secret (160 bits, uuid-backed randomness).
pub fn gen_secret() -> String {
    let mut bytes = Vec::with_capacity(20);
    while bytes.len() < 20 {
        bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    bytes.truncate(20);
    BASE32_NOPAD.encode(&bytes)
}

/// otpauth:// provisioning URI for QR codes / manual entry.
pub fn provisioning_uri(secret: &str, account: &str, issuer: &str) -> String {
    format!(
        "otpauth://totp/{issuer}:{account}?secret={secret}&issuer={issuer}&algorithm=SHA1&digits=6&period=30"
    )
}

/// The 6-digit code for `secret` at unix time `now`.
pub fn code_at(secret: &str, now: i64) -> Option<String> {
    let key = BASE32_NOPAD.decode(secret.as_bytes()).ok()?;
    let counter = (now / STEP) as u64;
    let mut mac = HmacSha1::new_from_slice(&key).ok()?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let bin = ((digest[offset] as u32 & 0x7f) << 24)
        | ((digest[offset + 1] as u32) << 16)
        | ((digest[offset + 2] as u32) << 8)
        | (digest[offset + 3] as u32);
    let code = bin % 10u32.pow(DIGITS);
    Some(format!("{code:0width$}", width = DIGITS as usize))
}

/// Verify `code` against `secret` allowing ±1 step of clock skew.
pub fn verify(secret: &str, code: &str, now: i64) -> bool {
    let code = code.trim();
    for skew in [-STEP, 0, STEP] {
        if let Some(expected) = code_at(secret, now + skew) {
            if constant_eq(expected.as_bytes(), code.as_bytes()) {
                return true;
            }
        }
    }
    false
}

/// Length-then-content compare; avoids early-exit timing on the digits.
fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_and_window() {
        let s = gen_secret();
        let now = 1_700_000_000;
        let c = code_at(&s, now).unwrap();
        assert_eq!(c.len(), 6);
        assert!(verify(&s, &c, now));
        assert!(verify(&s, &c, now + 20)); // within same/next step window
        assert!(!verify(&s, "000000", now) || c == "000000");
    }
    #[test]
    fn rfc6238_known_vector() {
        // RFC 6238 test secret "12345678901234567890" (ASCII) → base32
        let secret = BASE32_NOPAD.encode(b"12345678901234567890");
        // at T=59s the SHA1 TOTP (8-digit) is 94287082 → 6-digit = 287082
        assert_eq!(code_at(&secret, 59).unwrap(), "287082");
    }
}
