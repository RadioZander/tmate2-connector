//! TCI-X login (§4.3): the password never crosses the link. The server sends
//! a fresh nonce; the client answers with an HMAC of it, keyed with a key
//! derived from the password by scrypt.
//!
//! ```text
//! key   = scrypt(password, salt = "tcix-login:" + user, N = 2^14, r = 8, p = 1, 32 bytes)
//! proof = HMAC-SHA256(key, nonce as its 32 hex digits) as 64 lower-case hex digits
//! ```

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// The login key for an account. Slow on purpose (scrypt): compute it once
/// per user and password and keep it.
pub fn key(user: &str, password: &str) -> [u8; 32] {
    let params = scrypt::Params::new(14, 8, 1).expect("valid scrypt parameters");
    let salt = format!("tcix-login:{user}");
    let mut out = [0u8; 32];
    scrypt::scrypt(password.as_bytes(), salt.as_bytes(), &params, &mut out).expect("32-byte output is valid");
    out
}

/// The proof for one connection's nonce.
pub fn proof(key: &[u8; 32], nonce: &str) -> String {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(nonce.trim().to_ascii_lowercase().as_bytes());
    hex(&mac.finalize().into_bytes())
}

/// The `login:` line for an account and a nonce.
pub fn login_line(user: &str, key: &[u8; 32], nonce: &str) -> String {
    format!("login:{},{};", crate::tci::pct_encode(user), proof(key, nonce))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worked example in TCI-X §4.3.
    #[test]
    fn matches_the_spec_example() {
        let k = key("g0kad", "correct horse");
        assert_eq!(
            hex(&k),
            "7b9d9b088f60009828eceb149a2fc01b8c3e925ba0a82f1e51be70ed21b3d157"
        );
        assert_eq!(
            proof(&k, "00112233445566778899aabbccddeeff"),
            "6a83548bc3fdc8eb8f11f1d33bee6884231769102e96a05c0d188d3b2ec66974"
        );
        assert_eq!(
            login_line("g0kad", &k, "00112233445566778899aabbccddeeff"),
            "login:g0kad,6a83548bc3fdc8eb8f11f1d33bee6884231769102e96a05c0d188d3b2ec66974;"
        );
    }
}
