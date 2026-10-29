//! Tickets: the Gatekeeper's signed permission to send one return.
//!
//! `POST /api/tickets`, signed by a device, answers a compact JWS (`EdDSA`,
//! `typ: buwiz-ticket+jwt`) that expires in five minutes and binds the
//! account, device, TIN, form, period, the SHA-256 of the plaintext XML,
//! the confirmation email and a `jti`. The Sender checks it before it opens
//! the sealed return, and a ticket works once.

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use ed25519_dalek::{SigningKey, VerifyingKey};
use http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::devices::Signed;
use crate::error::ApiError;
use crate::jws;
use crate::state::AppState;
use crate::submissions::parse_filename;
use crate::tin::Tin;

pub const TYP: &str = "buwiz-ticket+jwt";
pub const AUDIENCE: &str = "buwiz-sender";
pub const TTL_SECONDS: i64 = 300;
/// Tickets per account (and per device) in an hour.
const HOURLY_QUOTA: i64 = 30;
/// Tickets per TIN in a day.
const DAILY_TIN_QUOTA: i64 = 50;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketClaims {
    pub iss: String,
    pub aud: String,
    pub sub: Uuid,
    pub account_id: Uuid,
    pub device_id: Uuid,
    /// 14 digits: the 9-digit root and the 5-digit branch.
    pub tin: String,
    pub form: String,
    pub period: String,
    /// SHA-256 of the plaintext XML, lowercase hex.
    pub payload_sha256: String,
    pub email: String,
    pub jti: Uuid,
    pub iat: i64,
    pub exp: i64,
}

impl TicketClaims {
    /// The eBIRForms file name the ticket allows.
    pub fn filename(&self) -> String {
        format!("{}-{}-{}#{}#.xml", self.tin, self.form, self.period, self.email)
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum TicketError {
    #[error("the ticket is not valid: {0}")]
    Invalid(String),
    #[error("the ticket expired; ask for a new one")]
    Expired,
}

pub fn sign(key: &SigningKey, claims: &TicketClaims) -> String {
    jws::sign(key, TYP, &serde_json::to_value(claims).expect("claims serialize"))
}

/// Signature, type, audience and expiry. Single use is the caller's.
pub fn verify(keys: &[VerifyingKey], ticket: &str, now: i64) -> Result<TicketClaims, TicketError> {
    let value = jws::verify(keys, TYP, ticket).map_err(|error| TicketError::Invalid(error.to_string()))?;
    let claims: TicketClaims =
        serde_json::from_value(value).map_err(|_| TicketError::Invalid("claims are missing".into()))?;
    if claims.aud != AUDIENCE || claims.sub != claims.account_id {
        return Err(TicketError::Invalid("wrong audience".into()));
    }
    if claims.exp <= now {
        return Err(TicketError::Expired);
    }
    Ok(claims)
}

#[derive(Deserialize)]
pub struct TicketRequest {
    tin: String,
    form: String,
    period: String,
    payload_sha256: String,
    email: String,
    idempotency_key: String,
}

#[derive(sqlx::FromRow)]
struct Previous {
    device_id: Uuid,
    claim_id: Uuid,
    form_type: String,
    period: String,
    payload_sha256: Vec<u8>,
    email: String,
    jws: String,
    expires_at: OffsetDateTime,
    submission_status: Option<String>,
    used_at: Option<OffsetDateTime>,
}

/// `POST /api/tickets`.
pub async fn issue(State(state): State<AppState>, signed: Signed<TicketRequest>) -> Result<Response, ApiError> {
    let Signed { bearer, device, body: request } = signed;
    let tin = Tin::parse_combined(&request.tin)
        .map_err(|_| ApiError::bad_request("bad_tin", "tin must be the 9-digit root and the branch code"))?;
    let tin_digits = format!("{}{}", tin.root(), tin.branch());
    let filename = format!("{tin_digits}-{}-{}#{}#.xml", request.form, request.period, request.email);
    parse_filename(&filename)
        .map_err(|_| ApiError::bad_request("bad_return", "form, period or email is not valid for an eBIRForms file"))?;
    let sha = request.payload_sha256.to_ascii_lowercase();
    let digest = (sha.len() == 64 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| (0..32).map(|i| u8::from_str_radix(&sha[2 * i..2 * i + 2], 16).expect("hex")).collect::<Vec<u8>>())
        .ok_or(ApiError::bad_request("bad_payload_sha256", "payload_sha256 must be 64 hex digits"))?;
    let key = request.idempotency_key.as_str();
    if key.is_empty() || key.len() > 100 {
        return Err(ApiError::bad_request("bad_idempotency_key", "idempotency_key must be 1 to 100 characters"));
    }

    let claim_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM tin_claims WHERE account_id = $1 AND tin_root = $2 AND branch_code = $3 AND status = 'approved'",
    )
    .bind(bearer.account.id)
    .bind(tin.root())
    .bind(tin.branch())
    .fetch_optional(&state.db)
    .await?;
    let Some(claim_id) = claim_id else {
        return Err(ApiError::forbidden("tin_not_active", "this TIN is not active on your account"));
    };

    let previous: Option<Previous> = sqlx::query_as(
        "SELECT t.device_id, t.claim_id, t.form_type, t.period, t.payload_sha256, t.email, t.jws, t.expires_at,
                s.status AS submission_status, t.used_at
         FROM tickets t LEFT JOIN submissions s ON s.id = t.submission_id
         WHERE t.account_id = $1 AND t.idempotency_key = $2 ORDER BY t.created_at DESC LIMIT 1",
    )
    .bind(bearer.account.id)
    .bind(key)
    .fetch_optional(&state.db)
    .await?;
    if let Some(previous) = previous {
        let same = previous.claim_id == claim_id
            && previous.form_type == request.form
            && previous.period == request.period
            && previous.payload_sha256 == digest
            && previous.email == request.email;
        if !same {
            return Err(ApiError::conflict(
                "idempotency_mismatch",
                "this idempotency_key was used for a different return",
            ));
        }
        let live = previous.expires_at > OffsetDateTime::now_utc();
        // A send that failed, expired, or never started before its ticket
        // ran out may be tried again with a new ticket. Anything else may
        // have reached BIR.
        let retryable = match previous.submission_status.as_deref() {
            Some("failed" | "expired") => true,
            Some("ready") => !live,
            Some(_) => false,
            None => previous.used_at.is_none(),
        };
        if !retryable {
            return Err(ApiError::conflict(
                "already_submitted",
                "this return was already handed over; check its status instead of sending again",
            ));
        }
        if previous.used_at.is_none() && live && previous.device_id == device.id {
            return Ok(Json(ticket_answer(&previous.jws, previous.expires_at)).into_response());
        }
    }

    let quotas: (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE account_id = $1 AND created_at > now() - interval '1 hour'),
                count(*) FILTER (WHERE device_id = $2 AND created_at > now() - interval '1 hour'),
                count(*) FILTER (WHERE claim_id = $3 AND created_at > now() - interval '1 day')
         FROM tickets WHERE account_id = $1 AND created_at > now() - interval '1 day'",
    )
    .bind(bearer.account.id)
    .bind(device.id)
    .bind(claim_id)
    .fetch_one(&state.db)
    .await?;
    if quotas.0 >= HOURLY_QUOTA || quotas.1 >= HOURLY_QUOTA || quotas.2 >= DAILY_TIN_QUOTA {
        return Err(ApiError::Client {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "quota_exceeded",
            message: "too many sends in a short time; try again later",
        });
    }

    let now = OffsetDateTime::now_utc();
    let jti = Uuid::new_v4();
    let claims = TicketClaims {
        iss: state.config.account_origin.clone(),
        aud: AUDIENCE.to_owned(),
        sub: bearer.account.id,
        account_id: bearer.account.id,
        device_id: device.id,
        tin: tin_digits,
        form: request.form.clone(),
        period: request.period.clone(),
        payload_sha256: sha,
        email: request.email.clone(),
        jti,
        iat: now.unix_timestamp(),
        exp: now.unix_timestamp() + TTL_SECONDS,
    };
    let ticket = sign(&state.ticket_key, &claims);
    let expires_at = OffsetDateTime::from_unix_timestamp(claims.exp).expect("valid time");
    sqlx::query(
        "INSERT INTO tickets (jti, account_id, device_id, claim_id, form_type, period, payload_sha256, email,
             idempotency_key, jws, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(jti)
    .bind(bearer.account.id)
    .bind(device.id)
    .bind(claim_id)
    .bind(&request.form)
    .bind(&request.period)
    .bind(&digest)
    .bind(&request.email)
    .bind(key)
    .bind(&ticket)
    .bind(expires_at)
    .execute(&state.db)
    .await?;
    tracing::info!(%jti, account = %bearer.account.id, device = %device.id, form = %request.form, "ticket issued");
    Ok((StatusCode::CREATED, Json(ticket_answer(&ticket, expires_at))).into_response())
}

fn ticket_answer(ticket: &str, expires_at: OffsetDateTime) -> serde_json::Value {
    let claims = jws::claims_unverified(ticket).unwrap_or_default();
    json!({ "ticket": ticket, "jti": claims["jti"], "expires_at": expires_at.unix_timestamp() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(exp: i64) -> TicketClaims {
        let account = Uuid::new_v4();
        TicketClaims {
            iss: "http://localhost".into(),
            aud: AUDIENCE.into(),
            sub: account,
            account_id: account,
            device_id: Uuid::new_v4(),
            tin: "00012345300000".into(),
            form: "1701Qv2018".into(),
            period: "2026Q1".into(),
            payload_sha256: "00".repeat(32),
            email: "juana@example.com".into(),
            jti: Uuid::new_v4(),
            iat: exp - TTL_SECONDS,
            exp,
        }
    }

    #[test]
    fn tickets_expire_and_bind_their_file_name() {
        let key = SigningKey::from_bytes(&[1; 32]);
        let fresh = claims(2_000);
        let ticket = sign(&key, &fresh);
        assert_eq!(verify(&[key.verifying_key()], &ticket, 1_000).unwrap(), fresh);
        assert_eq!(verify(&[key.verifying_key()], &ticket, 2_000), Err(TicketError::Expired));
        assert!(parse_filename(&fresh.filename()).is_ok(), "{}", fresh.filename());
        let other = SigningKey::from_bytes(&[2; 32]);
        assert!(matches!(verify(&[other.verifying_key()], &ticket, 1_000), Err(TicketError::Invalid(_))));
    }
}
