use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Sign a query string with HMAC-SHA256.
pub fn sign(query: &str, secret: &str) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
    mac.update(query.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign() {
        let sig = sign("symbol=BTCUSDT&timestamp=1234567890", "my_secret");
        assert!(!sig.is_empty());
        assert_eq!(sig.len(), 64); // SHA256 produces 32 bytes = 64 hex chars
    }
}
