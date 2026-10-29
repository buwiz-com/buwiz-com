//! Compact JWS with EdDSA (Ed25519): receipts and tickets.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Key id: the first 8 bytes of SHA-256 of the public key, hex.
pub fn key_id(key: &VerifyingKey) -> String {
    crate::bir::hex(&Sha256::digest(key.as_bytes())[..8])
}

/// A compact JWS over `claims` with header `typ`.
pub fn sign(key: &SigningKey, typ: &str, claims: &Value) -> String {
    let header = json!({ "alg": "EdDSA", "kid": key_id(&key.verifying_key()), "typ": typ });
    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("json")),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).expect("json")),
    );
    let signature = key.sign(signing_input.as_bytes());
    format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum JwsError {
    #[error("not a compact JWS")]
    Malformed,
    #[error("wrong JWS header")]
    Header,
    #[error("signed by an unknown key")]
    UnknownKey,
    #[error("bad signature")]
    Signature,
}

/// Checks `jws` against `keys` (matched by `kid`) and its `typ`, and returns
/// the claims. Expiry and the claims themselves are the caller's to check.
pub fn verify(keys: &[VerifyingKey], typ: &str, jws: &str) -> Result<Value, JwsError> {
    let mut parts = jws.split('.');
    let (Some(header), Some(payload), Some(signature), None) = (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(JwsError::Malformed);
    };
    let decode = |part: &str| URL_SAFE_NO_PAD.decode(part).map_err(|_| JwsError::Malformed);
    let header: Value = serde_json::from_slice(&decode(header)?).map_err(|_| JwsError::Malformed)?;
    if header["alg"] != "EdDSA" || header["typ"] != typ {
        return Err(JwsError::Header);
    }
    let kid = header["kid"].as_str().ok_or(JwsError::Header)?;
    let key = keys.iter().find(|key| key_id(key) == kid).ok_or(JwsError::UnknownKey)?;
    let signature = Signature::from_slice(&decode(signature)?).map_err(|_| JwsError::Malformed)?;
    let input = &jws[..jws.rfind('.').expect("three parts")];
    key.verify(input.as_bytes(), &signature).map_err(|_| JwsError::Signature)?;
    serde_json::from_slice(&decode(payload)?).map_err(|_| JwsError::Malformed)
}

/// The claims of a JWS, unverified. Only for a JWS that came from a trusted
/// party over a trusted channel.
pub fn claims_unverified(jws: &str) -> Option<Value> {
    let payload = jws.split('.').nth(1)?;
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()
}

/// One Ed25519 key as a JWK.
pub fn jwk(key: &VerifyingKey) -> Value {
    json!({
        "kty": "OKP",
        "crv": "Ed25519",
        "x": URL_SAFE_NO_PAD.encode(key.as_bytes()),
        "kid": key_id(key),
        "alg": "EdDSA",
        "use": "sig",
    })
}

/// The Ed25519 key in a JWK, if it is one.
pub fn from_jwk(jwk: &Value) -> Option<VerifyingKey> {
    if jwk["kty"] != "OKP" || jwk["crv"] != "Ed25519" {
        return None;
    }
    let bytes: [u8; 32] = URL_SAFE_NO_PAD.decode(jwk["x"].as_str()?).ok()?.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signed_token_verifies_only_with_its_key_and_type() {
        let key = SigningKey::from_bytes(&[3; 32]);
        let other = SigningKey::from_bytes(&[4; 32]);
        let jws = sign(&key, "buwiz-ticket+jwt", &json!({ "jti": "x" }));
        let keys = [other.verifying_key(), key.verifying_key()];
        assert_eq!(verify(&keys, "buwiz-ticket+jwt", &jws).unwrap()["jti"], "x");
        assert_eq!(verify(&keys, "buwiz-receipt+jwt", &jws), Err(JwsError::Header));
        assert_eq!(verify(&[other.verifying_key()], "buwiz-ticket+jwt", &jws), Err(JwsError::UnknownKey));

        let mut tampered = jws.clone();
        let middle = jws.split('.').nth(1).unwrap();
        let forged = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({ "jti": "y" })).unwrap());
        tampered = tampered.replacen(middle, &forged, 1);
        assert_eq!(verify(&keys, "buwiz-ticket+jwt", &tampered), Err(JwsError::Signature));
        assert_eq!(from_jwk(&jwk(&key.verifying_key())), Some(key.verifying_key()));
    }
}
