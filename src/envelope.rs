//! Sealed returns: HPKE (RFC 9180) base mode with DHKEM(X25519,
//! HKDF-SHA256), HKDF-SHA256 and ChaCha20-Poly1305. The app seals the
//! plaintext XML to the Sender's public key; the Gatekeeper stores the
//! envelope without being able to open it.
//!
//! The HPKE `info` binds the envelope to one ticket:
//! `buwiz-submission-v1:` followed by the ticket's `jti`.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hpke::aead::ChaCha20Poly1305;
use hpke::kdf::HkdfSha256;
use hpke::kem::X25519HkdfSha256;
use hpke::{Deserializable, Kem as _, OpModeR, OpModeS, Serializable};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const KEM: &str = "DHKEM(X25519, HKDF-SHA256)";
pub const KDF: &str = "HKDF-SHA256";
pub const AEAD: &str = "ChaCha20Poly1305";

type Kem = X25519HkdfSha256;

/// What the app posts: `{"kid","enc","ct"}`, the last two base64url.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub kid: String,
    pub enc: String,
    pub ct: String,
}

/// The HPKE `info` for a ticket.
pub fn info(jti: &str) -> Vec<u8> {
    format!("buwiz-submission-v1:{jti}").into_bytes()
}

/// The Sender's X25519 key pair.
pub struct SealingKey {
    private: <Kem as hpke::Kem>::PrivateKey,
    public: <Kem as hpke::Kem>::PublicKey,
}

impl SealingKey {
    pub fn from_bytes(secret: &[u8; 32]) -> anyhow::Result<Self> {
        let private = <Kem as hpke::Kem>::PrivateKey::from_bytes(secret)
            .map_err(|_| anyhow::anyhow!("not an X25519 private key"))?;
        let public = Kem::sk_to_pk(&private);
        Ok(Self { private, public })
    }

    pub fn public_bytes(&self) -> Vec<u8> {
        self.public.to_bytes().to_vec()
    }

    /// The first 8 bytes of SHA-256 of the public key, hex.
    pub fn kid(&self) -> String {
        crate::crypto::hex(&Sha256::digest(self.public_bytes())[..8])
    }

    /// `/.well-known/buwiz-hpke.json`.
    pub fn public_document(&self) -> Value {
        json!({ "keys": [{
            "kid": self.kid(),
            "kem": KEM,
            "kdf": KDF,
            "aead": AEAD,
            "public_key": URL_SAFE_NO_PAD.encode(self.public_bytes()),
        }] })
    }

    pub fn open(&self, envelope: &Envelope, jti: &str) -> Result<Vec<u8>, &'static str> {
        if envelope.kid != self.kid() {
            return Err("the return was sealed to another key");
        }
        let enc = URL_SAFE_NO_PAD.decode(&envelope.enc).map_err(|_| "the envelope is malformed")?;
        let ct = URL_SAFE_NO_PAD.decode(&envelope.ct).map_err(|_| "the envelope is malformed")?;
        let encapped = <Kem as hpke::Kem>::EncappedKey::from_bytes(&enc).map_err(|_| "the envelope is malformed")?;
        hpke::single_shot_open::<ChaCha20Poly1305, HkdfSha256, Kem>(
            &OpModeR::Base,
            &self.private,
            &encapped,
            &info(jti),
            &ct,
            b"",
        )
        .map_err(|_| "the envelope does not open")
    }
}

/// Seals `plaintext` to `public` (32 bytes) for ticket `jti`. The server
/// only uses this in tests; apps have their own copy.
pub fn seal(public: &[u8], kid: &str, jti: &str, plaintext: &[u8]) -> anyhow::Result<Envelope> {
    let public = <Kem as hpke::Kem>::PublicKey::from_bytes(public).map_err(|_| anyhow::anyhow!("bad public key"))?;
    let (encapped, ct) = hpke::single_shot_seal::<ChaCha20Poly1305, HkdfSha256, Kem>(
        &OpModeS::Base,
        &public,
        &info(jti),
        plaintext,
        b"",
    )
    .map_err(|_| anyhow::anyhow!("sealing failed"))?;
    Ok(Envelope {
        kid: kid.to_owned(),
        enc: URL_SAFE_NO_PAD.encode(encapped.to_bytes()),
        ct: URL_SAFE_NO_PAD.encode(ct),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_envelope_opens_only_for_its_ticket_and_untouched() {
        let key = SealingKey::from_bytes(&[5; 32]).unwrap();
        let envelope = seal(&key.public_bytes(), &key.kid(), "jti-1", b"<xml/>").unwrap();
        assert_eq!(key.open(&envelope, "jti-1").unwrap(), b"<xml/>");
        assert_eq!(key.open(&envelope, "jti-2"), Err("the envelope does not open"));

        let mut ct = URL_SAFE_NO_PAD.decode(&envelope.ct).unwrap();
        ct[0] ^= 1;
        let tampered = Envelope { ct: URL_SAFE_NO_PAD.encode(ct), ..envelope.clone() };
        assert_eq!(key.open(&tampered, "jti-1"), Err("the envelope does not open"));

        let other = Envelope { kid: "0000000000000000".into(), ..envelope };
        assert_eq!(key.open(&other, "jti-1"), Err("the return was sealed to another key"));
    }
}
