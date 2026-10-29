//! Device keys. After device-flow sign-in the app registers an Ed25519
//! public key. Every request on the v2 endpoints then carries a signature
//! from it, DPoP-style, over the method, path, body hash, a timestamp and a
//! nonce:
//!
//! ```text
//! buwiz-device: <device id>          (absent when registering)
//! buwiz-timestamp: <unix seconds>
//! buwiz-nonce: <16-64 chars, [A-Za-z0-9_-]>
//! buwiz-signature: <base64url Ed25519 signature over signing_input()>
//! ```
//!
//! A timestamp more than 60 seconds off is refused, and so is a nonce the
//! key has used before. A revoked device is refused everywhere: its
//! signatures fail and the sign-in it was registered under is ended.

use axum::Json;
use axum::extract::{FromRequest, Path, Request, State};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use http::StatusCode;
use http::request::Parts;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::crypto;
use crate::error::ApiError;
use crate::oauth::Bearer;
use crate::state::AppState;

pub const HEADER_DEVICE: &str = "buwiz-device";
pub const HEADER_TIMESTAMP: &str = "buwiz-timestamp";
pub const HEADER_NONCE: &str = "buwiz-nonce";
pub const HEADER_SIGNATURE: &str = "buwiz-signature";
pub const MAX_SKEW_SECONDS: i64 = 60;
const MAX_BODY: usize = crate::submissions::BODY_LIMIT;

/// What the device signs.
pub fn signing_input(method: &str, path: &str, body_sha256_hex: &str, timestamp: i64, nonce: &str) -> String {
    format!("buwiz-request-v1\n{method}\n{path}\n{body_sha256_hex}\n{timestamp}\n{nonce}")
}

#[derive(Clone, Debug)]
pub struct Device {
    pub id: Uuid,
    pub account_id: Uuid,
    pub grant_id: Uuid,
}

struct Proof<'a> {
    timestamp: i64,
    nonce: &'a str,
    signature: Signature,
}

fn header<'a>(parts: &'a Parts, name: &str) -> Option<&'a str> {
    parts.headers.get(name).and_then(|value| value.to_str().ok())
}

fn unauthorized(code: &'static str, message: &'static str) -> ApiError {
    ApiError::Client { status: StatusCode::UNAUTHORIZED, code, message }
}

fn proof(parts: &Parts) -> Result<Proof<'_>, ApiError> {
    let missing = || unauthorized("signature_required", "sign this request with the device key");
    let timestamp: i64 = header(parts, HEADER_TIMESTAMP).ok_or_else(missing)?.parse().map_err(|_| missing())?;
    let nonce = header(parts, HEADER_NONCE).ok_or_else(missing)?;
    let nonce_ok = (16..=64).contains(&nonce.len())
        && nonce.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if !nonce_ok {
        return Err(unauthorized("bad_nonce", "the nonce must be 16 to 64 URL-safe characters"));
    }
    let signature = URL_SAFE_NO_PAD
        .decode(header(parts, HEADER_SIGNATURE).ok_or_else(missing)?)
        .ok()
        .and_then(|bytes| Signature::from_slice(&bytes).ok())
        .ok_or_else(|| unauthorized("bad_signature", "the device signature does not verify"))?;
    let skew = (OffsetDateTime::now_utc().unix_timestamp() - timestamp).abs();
    if skew > MAX_SKEW_SECONDS {
        return Err(unauthorized("clock_skew", "the request time is more than 60 seconds off; check the clock"));
    }
    Ok(Proof { timestamp, nonce, signature })
}

/// Checks the signature with `key`, then spends the nonce.
async fn check(db: &PgPool, parts: &Parts, body: &[u8], key: &VerifyingKey) -> Result<(), ApiError> {
    let proof = proof(parts)?;
    let path = parts.uri.path_and_query().map_or_else(|| parts.uri.path(), |pq| pq.as_str());
    let input = signing_input(
        parts.method.as_str(),
        path,
        &crate::bir::hex(&crypto::sha256(body)),
        proof.timestamp,
        proof.nonce,
    );
    key.verify(input.as_bytes(), &proof.signature)
        .map_err(|_| unauthorized("bad_signature", "the device signature does not verify"))?;
    let fresh = sqlx::query("INSERT INTO request_nonces (public_key, nonce) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(key.as_bytes().as_slice())
        .bind(proof.nonce)
        .execute(db)
        .await?;
    if fresh.rows_affected() == 0 {
        return Err(unauthorized("nonce_reused", "this request was already made; sign it again with a new nonce"));
    }
    Ok(())
}

/// Verifies a request signed by a registered device of the bearer's
/// sign-in. `body` is the raw request body (empty for a GET).
pub async fn verify(db: &PgPool, parts: &Parts, bearer: &Bearer, body: &[u8]) -> Result<Device, ApiError> {
    let id: Uuid = header(parts, HEADER_DEVICE)
        .and_then(|raw| raw.parse().ok())
        .ok_or_else(|| unauthorized("signature_required", "sign this request with the device key"))?;
    let row: Option<(Uuid, Uuid, Vec<u8>, Option<OffsetDateTime>)> =
        sqlx::query_as("SELECT account_id, grant_id, public_key, revoked_at FROM devices WHERE id = $1")
            .bind(id)
            .fetch_optional(db)
            .await?;
    let Some((account_id, grant_id, public_key, revoked_at)) = row else {
        return Err(unauthorized("unknown_device", "register this device first"));
    };
    if account_id != bearer.account.id || grant_id != bearer.grant_id {
        return Err(unauthorized("unknown_device", "this device belongs to another sign-in"));
    }
    if revoked_at.is_some() {
        return Err(ApiError::forbidden("device_revoked", "this device was revoked; sign in again"));
    }
    let key = public_key
        .as_slice()
        .try_into()
        .ok()
        .and_then(|bytes| VerifyingKey::from_bytes(bytes).ok())
        .ok_or_else(|| anyhow::anyhow!("stored device key is invalid"))?;
    check(db, parts, body, &key).await?;
    sqlx::query("UPDATE devices SET last_used_at = now() WHERE id = $1").bind(id).execute(db).await?;
    Ok(Device { id, account_id, grant_id })
}

/// A JSON request from the BIR app, signed by one of its devices.
pub struct Signed<T> {
    pub bearer: Bearer,
    pub device: Device,
    pub body: T,
}

impl<T: serde::de::DeserializeOwned> FromRequest<AppState> for Signed<T> {
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &AppState) -> Result<Self, ApiError> {
        let (mut parts, body) = request.into_parts();
        let bearer =
            <Bearer as axum::extract::FromRequestParts<AppState>>::from_request_parts(&mut parts, state).await?;
        bearer.require_app()?;
        let bytes = read_body(body).await?;
        let device = verify(&state.db, &parts, &bearer, &bytes).await?;
        let body = parse(&bytes)?;
        Ok(Self { bearer, device, body })
    }
}

async fn read_body(body: axum::body::Body) -> Result<Bytes, ApiError> {
    axum::body::to_bytes(body, MAX_BODY)
        .await
        .map_err(|_| ApiError::bad_request("body_too_large", "the request body is too large"))
}

fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(bytes).map_err(|_| ApiError::bad_request("bad_request", "the request body is not valid"))
}

#[derive(Deserialize)]
struct NewDevice {
    public_key: String,
    name: String,
}

/// `POST /api/devices`: registers the key in the body, signed by that key.
pub async fn register(State(state): State<AppState>, request: Request) -> Result<Response, ApiError> {
    let (mut parts, body) = request.into_parts();
    let bearer = <Bearer as axum::extract::FromRequestParts<AppState>>::from_request_parts(&mut parts, &state).await?;
    bearer.require_app()?;
    let bytes = read_body(body).await?;
    let new: NewDevice = parse(&bytes)?;
    let key = URL_SAFE_NO_PAD
        .decode(new.public_key.trim())
        .ok()
        .and_then(|raw| <[u8; 32]>::try_from(raw).ok())
        .and_then(|raw| VerifyingKey::from_bytes(&raw).ok())
        .ok_or(ApiError::bad_request("bad_public_key", "public_key must be a base64url Ed25519 key"))?;
    let name = new.name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(ApiError::bad_request("bad_name", "name must be 1 to 80 characters"));
    }
    check(&state.db, &parts, &bytes, &key).await?;
    let inserted: Result<(Uuid, OffsetDateTime), sqlx::Error> = sqlx::query_as(
        "INSERT INTO devices (account_id, grant_id, public_key, name) VALUES ($1, $2, $3, $4) RETURNING id, created_at",
    )
    .bind(bearer.account.id)
    .bind(bearer.grant_id)
    .bind(key.as_bytes().as_slice())
    .bind(name)
    .fetch_one(&state.db)
    .await;
    let (id, created_at) = match inserted {
        Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
            return Err(ApiError::conflict("device_exists", "this key is already registered"));
        }
        other => other?,
    };
    tracing::info!(device = %id, account = %bearer.account.id, "device registered");
    let created_at = created_at.format(&time::format_description::well_known::Rfc3339).map_err(anyhow::Error::from)?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id, "name": name, "created_at": created_at }))).into_response())
}

#[derive(Serialize, sqlx::FromRow)]
pub struct DeviceView {
    pub id: Uuid,
    pub name: String,
    pub client_id: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
}

pub async fn for_account(db: &PgPool, account_id: Uuid) -> sqlx::Result<Vec<DeviceView>> {
    sqlx::query_as(
        "SELECT d.id, d.name, g.client_id, d.created_at, d.last_used_at, d.revoked_at
         FROM devices d JOIN oauth_grants g ON g.id = d.grant_id
         WHERE d.account_id = $1 ORDER BY d.revoked_at IS NOT NULL, d.created_at DESC",
    )
    .bind(account_id)
    .fetch_all(db)
    .await
}

/// Revokes the device and ends the sign-in it was registered under.
/// `false` when it is not this account's or was already revoked.
pub async fn revoke(db: &PgPool, account_id: Uuid, device_id: Uuid) -> sqlx::Result<bool> {
    let mut tx = db.begin().await?;
    let grant: Option<Uuid> = sqlx::query_scalar(
        "UPDATE devices SET revoked_at = now() WHERE id = $1 AND account_id = $2 AND revoked_at IS NULL
         RETURNING grant_id",
    )
    .bind(device_id)
    .bind(account_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(grant) = grant else {
        return Ok(false);
    };
    sqlx::query(
        "UPDATE oauth_grants SET revoked_at = now(), revoke_reason = 'device_revoked' WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(grant)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    tracing::info!(device = %device_id, account = %account_id, "device revoked");
    Ok(true)
}

/// `GET /api/devices`.
pub async fn list(State(state): State<AppState>, bearer: Bearer) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({ "devices": for_account(&state.db, bearer.account.id).await? })))
}

/// `POST /api/devices/{id}/revoke`.
pub async fn revoke_api(
    State(state): State<AppState>,
    bearer: Bearer,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    bearer.require_app()?;
    if revoke(&state.db, bearer.account.id, id).await? {
        Ok(Json(json!({ "revoked": true })))
    } else {
        Err(ApiError::not_found())
    }
}

/// Upkeep: nonces older than the skew window can never be accepted again.
pub async fn upkeep(db: &PgPool) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM request_nonces WHERE seen_at < now() - interval '10 minutes'").execute(db).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signing_input_covers_every_part() {
        assert_eq!(
            signing_input("POST", "/api/tickets", "ab", 1700000000, "nonce-0123456789"),
            "buwiz-request-v1\nPOST\n/api/tickets\nab\n1700000000\nnonce-0123456789"
        );
    }
}
