use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use ulid::Ulid;

const MACHINE_ID_NAMESPACE: &[u8] = b"remote-control-for-windows/machine-id/v1";

pub fn new_request_id() -> String {
    Ulid::new().to_string()
}

pub fn new_session_id() -> String {
    Ulid::new().to_string()
}

pub fn new_session_token() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn new_host_id() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("host_{}", URL_SAFE_NO_PAD.encode(bytes))
}

pub fn short_machine_id(stable_material: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(MACHINE_ID_NAMESPACE);
    hasher.update(stable_material);
    let digest = hasher.finalize();
    let hex = hex::encode_upper(&digest[..6]);
    format!("{}-{}-{}", &hex[..4], &hex[4..8], &hex[8..12])
}

pub fn token_label(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    format!("token:{}", &hex::encode(&digest[..4]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_machine_id_is_stable_and_redacted() {
        let first = short_machine_id(b"machine-guid");
        let second = short_machine_id(b"machine-guid");
        assert_eq!(first, second);
        assert_eq!(first.len(), 14);
        assert!(first.contains('-'));
        assert!(!first.contains("machine-guid"));
    }

    #[test]
    fn host_id_is_random_url_safe_and_prefixed() {
        let first = new_host_id();
        let second = new_host_id();
        assert!(first.starts_with("host_"));
        assert!(second.starts_with("host_"));
        assert_ne!(first, second);
        assert!(!first.contains('/'));
        assert!(!first.contains('+'));
        assert!(!first.contains('='));
    }
}
