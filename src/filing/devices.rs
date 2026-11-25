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

use crate::account::app_sign_in::Platform;
use crate::account::oauth::{Bearer, SCOPE_APP, SigningOut};
use crate::base::crypto;
use crate::base::error::ApiError;
use crate::base::state::AppState;
use crate::filing::apple_models;

pub const HEADER_DEVICE: &str = "buwiz-device";
pub const HEADER_TIMESTAMP: &str = "buwiz-timestamp";
pub const HEADER_NONCE: &str = "buwiz-nonce";
pub const HEADER_SIGNATURE: &str = "buwiz-signature";
pub const MAX_SKEW_SECONDS: i64 = 60;
const MAX_BODY: usize = crate::filing::submissions::BODY_LIMIT;

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
    let input =
        signing_input(parts.method.as_str(), path, &crypto::hex(&crypto::sha256(body)), proof.timestamp, proof.nonce);
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
    /// The app's own id for its install: one row per account and install.
    #[serde(default)]
    install_id: Option<String>,
    /// `ios`, `ipados`, `android`, `macos`, `windows` or `linux`; anything
    /// else is kept as nothing.
    #[serde(default)]
    platform: Option<Value>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    os_version: Option<String>,
    #[serde(default)]
    computer_name: Option<String>,
}

/// A free-text field of a device: trimmed, at most `max` characters and no
/// control characters; empty is none.
fn text_field(
    value: Option<&str>,
    max: usize,
    code: &'static str,
    message: &'static str,
) -> Result<Option<String>, ApiError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > max || value.chars().any(char::is_control) {
        return Err(ApiError::bad_request(code, message));
    }
    Ok(Some(value.to_owned()))
}

/// `POST /api/devices`: registers the key in the body, signed by that key.
/// A sign-in that carries an install id registers once, and an install
/// has one row per account: registering again (after a new sign-in)
/// updates it in place, with the new key and sign-in, and ends the sign-in
/// it had. A sign-in without one may register again, each time a new row.
///
/// A sign-in that named its install when it began (the in-app start, or
/// the device flow's `/oauth/device/code`) and was completed is proof of
/// the account, so its registration replaces that install's row whatever
/// sign-in held it: by design. Someone who knows a device's install id and
/// can complete a sign-in on the account replaces that device's sign-in;
/// when the sign-in replaced was still live, the account is mailed
/// ([`mail_replaced`]).
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
    let install = new.install_id.as_deref().map(str::trim);
    if install.is_some_and(|id| !crate::account::app_sign_in::is_install_id(id)) {
        return Err(ApiError::bad_request(
            "bad_install_id",
            "install_id must be 16 to 64 characters of A-Z, a-z, 0-9, - and _",
        ));
    }
    let platform = Platform::parse(new.platform.as_ref()).map(Platform::as_str);
    let model = text_field(new.model.as_deref(), 80, "bad_model", "model must be at most 80 characters")?;
    let os_version =
        text_field(new.os_version.as_deref(), 40, "bad_os_version", "os_version must be at most 40 characters")?;
    let computer_name = text_field(
        new.computer_name.as_deref(),
        80,
        "bad_computer_name",
        "computer_name must be at most 80 characters",
    )?;
    check(&state.db, &parts, &bytes, &key).await?;

    let mut tx = state.db.begin().await?;
    // The grant first, so two registrations of one sign-in go one at a time.
    let grant_install: Option<Option<String>> =
        sqlx::query_scalar("SELECT install_id FROM oauth_grants WHERE id = $1 AND revoked_at IS NULL FOR UPDATE")
            .bind(bearer.grant_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(grant_install) = grant_install else {
        return Err(ApiError::InvalidToken);
    };
    match (grant_install.as_deref(), install) {
        (Some(signed_in_as), asked) if asked != Some(signed_in_as) => {
            return Err(ApiError::bad_request("install_mismatch", "install_id is not the one this app signed in with"));
        }
        // The sign-in named this install when it began, and was completed:
        // that is proof of the account, so it takes the install's row below
        // (ending the sign-in that held it, with a mail when that one was
        // live). No check that the row's sign-in is over: that is the
        // designed behaviour, not a gap (docs/accounts.md, "Devices").
        (Some(_), Some(_)) => {}
        (Some(_), None) => unreachable!("refused as install_mismatch above"),
        (None, Some(install)) => {
            // An install another live sign-in of the account registered is
            // not this sign-in's to take: it would re-key that device and
            // end its sign-in. Once that sign-in is over, or its device
            // removed, it may.
            let in_use: bool = sqlx::query_scalar(
                "SELECT EXISTS (
                     SELECT 1 FROM devices d JOIN oauth_grants g ON g.id = d.grant_id
                     WHERE d.account_id = $1 AND d.install_id = $2 AND d.revoked_at IS NULL
                       AND d.grant_id <> $3 AND g.revoked_at IS NULL
                       AND (EXISTS (SELECT 1 FROM oauth_refresh_tokens r
                                    WHERE r.grant_id = g.id AND r.used_at IS NULL AND r.expires_at > now())
                            OR EXISTS (SELECT 1 FROM oauth_access_tokens t
                                       WHERE t.grant_id = g.id AND t.expires_at > now())))",
            )
            .bind(bearer.account.id)
            .bind(install)
            .bind(bearer.grant_id)
            .fetch_one(&mut *tx)
            .await?;
            if in_use {
                return Err(ApiError::conflict(
                    "install_in_use",
                    "another sign-in of this account holds this install's device",
                ));
            }
            sqlx::query("UPDATE oauth_grants SET install_id = $2 WHERE id = $1")
                .bind(bearer.grant_id)
                .bind(install)
                .execute(&mut *tx)
                .await?;
        }
        // Apps from before install ids.
        (None, None) => {}
    }
    // A sign-in with an install registers one device. One without (apps
    // from before install ids) may register again, as it always could:
    // each time is another row.
    let registered: bool = install.is_some()
        && sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM devices WHERE grant_id = $1 AND revoked_at IS NULL)")
            .bind(bearer.grant_id)
            .fetch_one(&mut *tx)
            .await?;
    if registered {
        return Err(ApiError::conflict("device_registered", "this sign-in already registered a device key"));
    }
    let known: Option<(Uuid, Uuid)> = match install {
        Some(install) => {
            sqlx::query_as("SELECT id, grant_id FROM devices WHERE account_id = $1 AND install_id = $2 FOR UPDATE")
                .bind(bearer.account.id)
                .bind(install)
                .fetch_optional(&mut *tx)
                .await?
        }
        None => None,
    };
    // Whether the sign-in this registration ends was still live: not
    // ended, with a refresh token that works.
    let mut replaced_live = false;
    let written: Result<(Uuid, OffsetDateTime), sqlx::Error> = match known {
        Some((id, old_grant)) => {
            if old_grant != bearer.grant_id {
                replaced_live = sqlx::query_scalar(&format!(
                    "SELECT revoked_at IS NULL AND {} FROM oauth_grants g WHERE g.id = $1 FOR UPDATE",
                    HAS_REFRESH
                ))
                .bind(old_grant)
                .fetch_optional(&mut *tx)
                .await?
                .unwrap_or(false);
                sqlx::query(
                    "UPDATE oauth_grants SET revoked_at = now(), revoke_reason = 'device_replaced'
                     WHERE id = $1 AND revoked_at IS NULL",
                )
                .bind(old_grant)
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query_as(
                "UPDATE devices SET public_key = $2, grant_id = $3, name = $4, platform = $5, model = $6,
                     os_version = $7, computer_name = $8, signed_in_at = now(), revoked_at = NULL
                 WHERE id = $1 RETURNING id, created_at",
            )
            .bind(id)
            .bind(key.as_bytes().as_slice())
            .bind(bearer.grant_id)
            .bind(name)
            .bind(platform)
            .bind(&model)
            .bind(&os_version)
            .bind(&computer_name)
            .fetch_one(&mut *tx)
            .await
        }
        None => {
            sqlx::query_as(
                "INSERT INTO devices
                     (account_id, grant_id, public_key, name, install_id, platform, model, os_version, computer_name)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id, created_at",
            )
            .bind(bearer.account.id)
            .bind(bearer.grant_id)
            .bind(key.as_bytes().as_slice())
            .bind(name)
            .bind(install)
            .bind(platform)
            .bind(&model)
            .bind(&os_version)
            .bind(&computer_name)
            .fetch_one(&mut *tx)
            .await
        }
    };
    let (id, created_at) = match written {
        Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
            return Err(ApiError::conflict("device_exists", "this key is already registered"));
        }
        other => other?,
    };
    tx.commit().await?;
    tracing::info!(device = %id, account = %bearer.account.id, again = known.is_some(), "device registered");
    if replaced_live {
        tokio::spawn(mail_replaced(state.clone(), bearer.account.id, bearer.account.email.clone(), id, platform));
    }
    let created_at = created_at.format(&time::format_description::well_known::Rfc3339).map_err(anyhow::Error::from)?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id, "name": name, "created_at": created_at }))).into_response())
}

/// SQL: the grant `g` has a refresh token that still works.
const HAS_REFRESH: &str = "EXISTS (SELECT 1 FROM oauth_refresh_tokens r
                                   WHERE r.grant_id = g.id AND r.used_at IS NULL AND r.expires_at > now())";

/// Mails the account that a registration replaced a live sign-in on one of
/// its devices ([`register`]): after the commit, off the request. Not to a
/// test account with a fixed code (nothing is mailed to those). A failure
/// is logged with ids only; the registration stands.
async fn mail_replaced(state: AppState, account_id: Uuid, email: String, device_id: Uuid, platform: Option<&str>) {
    let fixed =
        crate::account::accounts::has_fixed_code(&state.db, &email, &state.config.admin_emails).await.unwrap_or_else(
            |error| {
                tracing::error!(%error, account = %account_id, device = %device_id, "could not read whether the account has a fixed code");
                true
            },
        );
    if fixed {
        return;
    }
    let device = Platform::device(platform.and_then(Platform::from_stored));
    // Ids only: a mail transport's error can quote the address.
    if state.mailer.send_sign_in_replaced(&email, device).await.is_err() {
        tracing::error!(account = %account_id, device = %device_id, "the mail about a replaced sign-in could not be sent");
    }
}

#[derive(Serialize, sqlx::FromRow)]
pub struct DeviceView {
    pub id: Uuid,
    pub name: String,
    pub client_id: String,
    #[serde(skip)]
    pub grant_id: Uuid,
    /// The caller's own sign-in's device ([`list`]).
    #[sqlx(skip)]
    pub current: bool,
    pub platform: Option<String>,
    /// As the app sent it.
    pub model: Option<String>,
    /// The name it is sold under, for an Apple code; else `model`.
    #[sqlx(skip)]
    pub model_name: Option<String>,
    pub os_version: Option<String>,
    pub computer_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub signed_in_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
    /// `live` (its sign-in has a refresh token that works), `expired` (its
    /// sign-in's tokens all ran out: signed out, though nobody removed
    /// it) or `removed`.
    pub state: String,
}

/// How long a removed device stays on the lists.
const REMOVED_SHOWN_DAYS: i32 = 30;

/// The account's devices, live first, without those removed more than 30
/// days ago.
pub async fn for_account(db: &PgPool, account_id: Uuid) -> sqlx::Result<Vec<DeviceView>> {
    let mut devices: Vec<DeviceView> = sqlx::query_as(&format!(
        "SELECT d.id, d.name, g.client_id, d.grant_id, d.platform, d.model, d.os_version, d.computer_name,
                d.created_at, coalesce(d.signed_in_at, d.created_at) AS signed_in_at, d.last_used_at, d.revoked_at,
                CASE WHEN d.revoked_at IS NOT NULL THEN 'removed'
                     WHEN g.revoked_at IS NULL AND {HAS_REFRESH} THEN 'live'
                     ELSE 'expired' END AS state
         FROM devices d JOIN oauth_grants g ON g.id = d.grant_id
         WHERE d.account_id = $1 AND (d.revoked_at IS NULL OR d.revoked_at > now() - make_interval(days => $2))
         ORDER BY d.revoked_at IS NOT NULL, coalesce(d.signed_in_at, d.created_at) DESC"
    ))
    .bind(account_id)
    .bind(REMOVED_SHOWN_DAYS)
    .fetch_all(db)
    .await?;
    for device in &mut devices {
        device.model_name = device.model.as_deref().map(|model| apple_models::name(model).into_owned());
    }
    Ok(devices)
}

/// An app signed in to the account that has not registered a device yet
/// (the person has not tapped "Add device"): a live grant of a BIR app
/// with no live device row.
#[derive(Serialize, sqlx::FromRow)]
pub struct PendingSignIn {
    pub grant_id: Uuid,
    pub client_id: String,
    pub platform: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub signed_in_at: OffsetDateTime,
    #[sqlx(skip)]
    pub current: bool,
}

pub async fn pending_for_account(db: &PgPool, account_id: Uuid) -> sqlx::Result<Vec<PendingSignIn>> {
    sqlx::query_as(
        "SELECT g.id AS grant_id, g.client_id, g.platform, g.created_at AS signed_in_at
         FROM oauth_grants g
         WHERE g.account_id = $1 AND g.revoked_at IS NULL AND g.scope = $2
           AND NOT EXISTS (SELECT 1 FROM devices d WHERE d.grant_id = g.id AND d.revoked_at IS NULL)
         ORDER BY g.created_at DESC",
    )
    .bind(account_id)
    .bind(SCOPE_APP)
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

/// Ends one of the account's live BIR app sign-ins (and its device, if
/// it has one), by the person themselves. `false` when it is not this
/// account's, not a BIR app's, or already ended.
pub async fn sign_out_grant(db: &PgPool, account_id: Uuid, grant_id: Uuid) -> sqlx::Result<bool> {
    let mut tx = db.begin().await?;
    let ended = sqlx::query(
        "UPDATE oauth_grants SET revoked_at = now(), revoke_reason = 'signed_out'
         WHERE id = $1 AND account_id = $2 AND scope = $3 AND revoked_at IS NULL",
    )
    .bind(grant_id)
    .bind(account_id)
    .bind(SCOPE_APP)
    .execute(&mut *tx)
    .await?;
    if ended.rows_affected() == 0 {
        return Ok(false);
    }
    sqlx::query("UPDATE devices SET revoked_at = now() WHERE grant_id = $1 AND revoked_at IS NULL")
        .bind(grant_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// `GET /api/devices`: the devices, and the sign-ins without one.
pub async fn list(State(state): State<AppState>, bearer: Bearer) -> Result<Json<Value>, ApiError> {
    let mut devices = for_account(&state.db, bearer.account.id).await?;
    for device in &mut devices {
        device.current = device.grant_id == bearer.grant_id;
    }
    let mut pending = pending_for_account(&state.db, bearer.account.id).await?;
    for sign_in in &mut pending {
        sign_in.current = sign_in.grant_id == bearer.grant_id;
    }
    Ok(Json(json!({ "devices": devices, "pending": pending })))
}

/// `POST /api/sign-ins/{grant_id}/sign-out`: ends one of the account's app
/// sign-ins, this app's own included. A banned account's app may do it,
/// as it may sign out.
pub async fn sign_out_api(
    State(state): State<AppState>,
    signing_out: SigningOut,
    Path(grant_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let bearer = signing_out.bearer;
    bearer.require_app()?;
    if sign_out_grant(&state.db, bearer.account.id, grant_id).await? {
        Ok(Json(json!({ "signed_out": true })))
    } else {
        Err(ApiError::not_found())
    }
}

/// `POST /api/devices/{id}/revoke`. A banned account's app may still
/// remove its own device, as it may sign out.
pub async fn revoke_api(
    State(state): State<AppState>,
    signing_out: SigningOut,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let bearer = signing_out.bearer;
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
