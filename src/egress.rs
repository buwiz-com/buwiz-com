//! The egress list: where an app may let the relay pipe connect.
//!
//! A compact JWS (`EdDSA`, `typ: buwiz-egress+jwt`) over
//! `{"hosts": ["host:port", …], "iat", "exp"}`, signed **offline** with a
//! key this server never holds (`buwiz-egress-sign`). The server only
//! serves the pre-signed blob from `EGRESS_LIST_JWS`, so a compromised
//! server cannot widen the list: apps pin the public key and refuse any
//! list it did not sign.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use ed25519_dalek::SigningKey;
use http::{HeaderValue, StatusCode, header};
use serde_json::json;

use crate::error::ApiError;
use crate::jws;
use crate::state::AppState;

pub const TYP: &str = "buwiz-egress+jwt";

/// Signs `hosts` (`host:port` each) valid from `iat` to `exp` (Unix seconds).
pub fn sign_list(key: &SigningKey, hosts: &[String], iat: i64, exp: i64) -> anyhow::Result<String> {
    for host in hosts {
        let valid = host
            .rsplit_once(':')
            .is_some_and(|(name, port)| !name.is_empty() && port.parse::<u16>().is_ok_and(|port| port > 0));
        anyhow::ensure!(valid, "not host:port: {host}");
    }
    anyhow::ensure!(exp > iat, "the list must expire after it is issued");
    Ok(jws::sign(key, TYP, &json!({ "hosts": hosts, "iat": iat, "exp": exp })))
}

/// `GET /.well-known/buwiz-egress.json`: the signed list, as configured.
pub async fn list(State(state): State<AppState>) -> Result<Response, ApiError> {
    let Some(list) = &state.config.egress_list else {
        return Err(ApiError::Client {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "egress_list_missing",
            message: "this server has no signed egress list",
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

    #[test]
    fn a_signed_list_verifies_with_its_key_only() {
        let key = SigningKey::from_bytes(&[6; 32]);
        let jws = sign_list(&key, &["127.0.0.1:22".into()], 10, 20).unwrap();
        let claims = jws::verify(&[key.verifying_key()], TYP, &jws).unwrap();
        assert_eq!(claims["hosts"][0], "127.0.0.1:22");
        assert_eq!(claims["exp"], 20);
        let other = SigningKey::from_bytes(&[7; 32]);
        assert!(jws::verify(&[other.verifying_key()], TYP, &jws).is_err());
        assert!(sign_list(&key, &["no-port".into()], 10, 20).is_err());
        assert!(sign_list(&key, &["h:22".into()], 20, 10).is_err());
    }
}
