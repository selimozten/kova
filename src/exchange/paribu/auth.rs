use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Sign data with HMAC-SHA256, returning a base64-encoded signature.
pub fn sign(data: &str, secret: &str) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
    mac.update(data.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_base64() {
        let sig = sign("market=btc_tl&amount=1.0", "my_secret");
        assert!(!sig.is_empty());
        // SHA256 produces 32 bytes = 44 base64 chars (with padding)
        assert_eq!(sig.len(), 44);
    }

    #[test]
    fn test_sign_deterministic() {
        let sig1 = sign("test_data", "secret");
        let sig2 = sign("test_data", "secret");
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_sign_different_data() {
        let sig1 = sign("data1", "secret");
        let sig2 = sign("data2", "secret");
        assert_ne!(sig1, sig2);
    }
}
