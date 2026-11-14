//! Generation and hashing of enrolment keys and agent credentials.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng as _;
use sha2::{Digest as _, Sha256};

/// Prefix of enrolment keys.
pub const ENROLLMENT_KEY_PREFIX: &str = "cbx_ek_";
/// Prefix of agent credentials.
pub const AGENT_CREDENTIAL_PREFIX: &str = "cbx_ac_";

/// Generates a token: the prefix followed by 32 random bytes, base64url without padding.
pub fn generate(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// SHA-256 of the token; only this hash is ever stored.
pub fn hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_unique_and_hash_is_stable() {
        let a = generate(ENROLLMENT_KEY_PREFIX);
        let b = generate(ENROLLMENT_KEY_PREFIX);
        assert_ne!(a, b);
        assert!(a.starts_with("cbx_ek_"));
        assert_eq!(hash("a"), hash("a"));
        assert_ne!(hash("a"), hash("b"));
    }
}
