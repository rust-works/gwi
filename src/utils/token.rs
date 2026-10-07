//! Random session tokens and OAuth state/verifier values.
//!
//! Forked from omni-dev's `browser::auth::generate_token` (rust-works/omni-dev#2203).

use base64::Engine;
use rand::Rng;

/// Bytes of entropy in a generated token (256 bits).
const TOKEN_BYTES: usize = 32;

/// Generates a fresh random token (256 bits, URL-safe base64).
#[must_use]
pub fn generate_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_url_safe_and_distinct() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 43, "32 bytes encode to 43 unpadded base64 chars");
        assert!(a
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }
}
