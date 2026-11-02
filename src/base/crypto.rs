//! Random tokens, token hashes, and document encryption.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// A bearer secret: `prefix` + 32 random bytes, base64url. Only its
/// [`hash`] is stored.
pub fn new_token(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes))
}

pub fn hash(secret: &str) -> Vec<u8> {
    Sha256::digest(secret.as_bytes()).to_vec()
}

pub fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Encrypts `plaintext` with AES-256-GCM. `context` (the claim and document
/// ids) is bound as associated data, so a ciphertext cannot be moved to
/// another row. Returns `(nonce, ciphertext)`.
pub fn seal(key: &[u8; 32], context: &[u8], plaintext: &[u8]) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let mut nonce = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce);
    let ciphertext = Aes256Gcm::new(key.into())
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad: context })
        .map_err(|_| anyhow::anyhow!("document encryption failed"))?;
    Ok((nonce.to_vec(), ciphertext))
}

pub fn open(key: &[u8; 32], context: &[u8], nonce: &[u8], ciphertext: &[u8]) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(nonce.len() == 12, "bad nonce length");
    Aes256Gcm::new(key.into())
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ciphertext, aad: context })
        .map_err(|_| anyhow::anyhow!("document decryption failed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_prefixed_and_unique() {
        let first = new_token("bwa_");
        assert!(first.starts_with("bwa_"));
        assert_eq!(first.len(), 4 + 43);
        assert_ne!(first, new_token("bwa_"));
    }

    #[test]
    fn sealed_documents_open_only_in_their_own_context() {
        let key = [7u8; 32];
        let (nonce, ciphertext) = seal(&key, b"claim-1/doc-1", b"%PDF-1.7").unwrap();
        assert_eq!(open(&key, b"claim-1/doc-1", &nonce, &ciphertext).unwrap(), b"%PDF-1.7");
        assert!(open(&key, b"claim-2/doc-1", &nonce, &ciphertext).is_err());
        assert!(open(&[8u8; 32], b"claim-1/doc-1", &nonce, &ciphertext).is_err());
    }
}
