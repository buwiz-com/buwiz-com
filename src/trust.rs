//! The relay trust list: what an app trusts when it sends a return.
//!
//! A compact JWS (`EdDSA`, `typ: buwiz-trust+jwt`) signed **offline** with
//! a key this server never holds (`buwiz-trust-sign`), over:
//!
//! ```json
//! {"hosts": ["host:port", …],
//!  "sealing_keys": [{"kid", "kem", "kdf", "aead", "public_key"}],
//!  "receipt_keys": [{"kty": "OKP", "crv": "Ed25519", "kid", "x", …}],
//!  "iat": …, "exp": …}
//! ```
//!
//! `hosts` is where the relay pipe may connect; `sealing_keys` are the
//! Sender's HPKE keys an app may seal to; `receipt_keys` verify receipts.
//! The server only serves the pre-signed blob from `TRUST_LIST_JWS`, so a
//! compromised server can neither widen the hosts nor swap in its own
//! sealing or receipt key: apps pin the list's public key.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use ed25519_dalek::SigningKey;
use http::{HeaderValue, StatusCode, header};
use serde_json::{Value, json};

use crate::error::ApiError;
use crate::jws;
use crate::state::AppState;

pub const TYP: &str = "buwiz-trust+jwt";

/// What the list vouches for.
#[derive(Clone, Debug, Default)]
pub struct Contents {
    pub hosts: Vec<String>,
    /// Entries of the Sender's `/.well-known/buwiz-hpke.json` `keys`.
    pub sealing_keys: Vec<Value>,
    /// Ed25519 JWKs of the Sender's receipt keys.
    pub receipt_keys: Vec<Value>,
}

impl Contents {
    /// Adds the keys in a Sender's public document
    /// (`{"receipt_keys":[…],"hpke":{"keys":[…]}}`, as `buwiz-sender public`
    /// prints and `/internal/public` answers).
    pub fn add_sender(&mut self, public: &Value) -> anyhow::Result<()> {
        let sealing = public["hpke"]["keys"].as_array().filter(|keys| !keys.is_empty());
        let receipt = public["receipt_keys"].as_array().filter(|keys| !keys.is_empty());
        let (Some(sealing), Some(receipt)) = (sealing, receipt) else {
            anyhow::bail!("a Sender's public keys need hpke.keys and receipt_keys");
        };
        self.sealing_keys.extend(sealing.iter().cloned());
        self.receipt_keys.extend(receipt.iter().cloned());
        Ok(())
    }
}

/// Signs `contents`, valid from `iat` to `exp` (Unix seconds).
pub fn sign_list(key: &SigningKey, contents: &Contents, iat: i64, exp: i64) -> anyhow::Result<String> {
    for host in &contents.hosts {
        let valid = host
            .rsplit_once(':')
            .is_some_and(|(name, port)| !name.is_empty() && port.parse::<u16>().is_ok_and(|port| port > 0));
        anyhow::ensure!(valid, "not host:port: {host}");
    }
    anyhow::ensure!(!contents.sealing_keys.is_empty(), "the list needs the Sender's sealing key");
    for jwk in &contents.receipt_keys {
        anyhow::ensure!(jws::from_jwk(jwk).is_some(), "not an Ed25519 JWK: {jwk}");
    }
    anyhow::ensure!(!contents.receipt_keys.is_empty(), "the list needs the Sender's receipt key");
    anyhow::ensure!(exp > iat, "the list must expire after it is issued");
    let claims = json!({
        "hosts": contents.hosts,
        "sealing_keys": contents.sealing_keys,
        "receipt_keys": contents.receipt_keys,
        "iat": iat,
        "exp": exp,
    });
    Ok(jws::sign(key, TYP, &claims))
}

/// `GET /.well-known/buwiz-trust.json`: the signed list, as configured.
pub async fn list(State(state): State<AppState>) -> Result<Response, ApiError> {
    let Some(list) = &state.config.trust_list else {
        return Err(ApiError::Client {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "trust_list_missing",
            message: "this server has no signed trust list",
        });
    };
    let mut response = list.clone().into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/jose"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=300"));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::SealingKey;

    fn sender_public() -> Value {
        let receipt = SigningKey::from_bytes(&[8; 32]);
        json!({
            "receipt_keys": [jws::jwk(&receipt.verifying_key())],
            "hpke": SealingKey::from_bytes(&[9; 32]).unwrap().public_document(),
        })
    }

    #[test]
    fn a_signed_list_carries_hosts_and_the_senders_keys() {
        let key = SigningKey::from_bytes(&[6; 32]);
        let mut contents = Contents { hosts: vec!["127.0.0.1:22".into()], ..Contents::default() };
        contents.add_sender(&sender_public()).unwrap();
        let jws = sign_list(&key, &contents, 10, 20).unwrap();
        let claims = jws::verify(&[key.verifying_key()], TYP, &jws).unwrap();
        assert_eq!(claims["hosts"][0], "127.0.0.1:22");
        assert_eq!(claims["sealing_keys"][0]["kid"], SealingKey::from_bytes(&[9; 32]).unwrap().kid());
        assert_eq!(claims["receipt_keys"][0]["crv"], "Ed25519");
        assert_eq!(claims["exp"], 20);
        let other = SigningKey::from_bytes(&[7; 32]);
        assert!(jws::verify(&[other.verifying_key()], TYP, &jws).is_err());
    }

    #[test]
    fn incomplete_lists_are_not_signed() {
        let key = SigningKey::from_bytes(&[6; 32]);
        let hosts_only = Contents { hosts: vec!["h:22".into()], ..Contents::default() };
        assert!(sign_list(&key, &hosts_only, 10, 20).is_err(), "no Sender keys");
        let mut contents = Contents { hosts: vec!["no-port".into()], ..Contents::default() };
        contents.add_sender(&sender_public()).unwrap();
        assert!(sign_list(&key, &contents, 10, 20).is_err());
        contents.hosts = vec!["h:22".into()];
        assert!(sign_list(&key, &contents, 20, 10).is_err());
        assert!(Contents::default().add_sender(&json!({})).is_err());
    }
}
