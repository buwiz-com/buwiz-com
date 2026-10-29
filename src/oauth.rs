//! Sign-in for the Buwiz desktop apps: RFC 8628 device authorization, the
//! token endpoint (device code and rotating refresh tokens), `/me`, and
//! sign-out.

use axum::extract::{FromRequestParts, State};
use axum::{Form, Json};
use http::request::Parts;
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::accounts::Account;
use crate::crypto;
use crate::error::{ApiError, OAuthError, OAuthFailure};
use crate::state::AppState;

/// Public clients allowed to sign in with the device flow, with the scope
/// each gets. They hold no secret. Only the BIR app (`app`) may file and
/// see full TINs.
pub const CLIENTS: &[(&str, &str, &str)] =
    &[("buwiz-desktop", "Buwiz", SCOPE_APP), ("buwiz-books", "Buwiz Books", SCOPE_BOOKS)];

const DEVICE_TTL: Duration = Duration::minutes(15);
const DEVICE_INTERVAL: i32 = 5;
const ACCESS_TTL: Duration = Duration::minutes(15);
const REFRESH_TTL: Duration = Duration::days(90);
/// No vowels, so a code never spells a word; no 0/O or 1/I lookalikes.
const USER_CODE_ALPHABET: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ";

pub fn client_name(client_id: &str) -> Option<&'static str> {
    CLIENTS.iter().find(|(id, ..)| *id == client_id).map(|(_, name, _)| *name)
}

fn client_scope(client_id: &str) -> &'static str {
    CLIENTS.iter().find(|(id, ..)| *id == client_id).map_or(SCOPE_MCP, |(.., scope)| *scope)
}

fn known_client(client_id: &str) -> Result<(), OAuthError> {
    match client_name(client_id) {
        Some(_) => Ok(()),
        None => Err(OAuthError {
            status: http::StatusCode::UNAUTHORIZED,
            error: "invalid_client",
            description: "unknown client_id",
        }),
    }
}

/// `ABCD-EFGH` from the user-code alphabet.
fn new_user_code() -> String {
    let mut rng = rand::rng();
    let mut code = String::with_capacity(9);
    for index in 0..8 {
        if index == 4 {
            code.push('-');
        }
        code.push(USER_CODE_ALPHABET[rng.random_range(0..USER_CODE_ALPHABET.len())] as char);
    }
    code
}

/// What a person typed, as stored: uppercase, `ABCD-EFGH`.
pub fn normalize_user_code(raw: &str) -> Option<String> {
    let letters: String = raw.chars().filter(char::is_ascii_alphanumeric).map(|ch| ch.to_ascii_uppercase()).collect();
    (letters.len() == 8).then(|| format!("{}-{}", &letters[..4], &letters[4..]))
}

#[derive(Deserialize)]
pub struct DeviceStart {
    client_id: String,
}

/// `POST /auth/device/start` (also `/oauth/device/code`).
pub async fn device_start(
    State(state): State<AppState>,
    Json(request): Json<DeviceStart>,
) -> Result<Json<Value>, OAuthFailure> {
    known_client(&request.client_id)?;
    let device_code = crypto::new_token("bwd_");
    let user_code = new_user_code();
    sqlx::query(
        "INSERT INTO device_authorizations (client_id, device_code_hash, user_code, interval_seconds, expires_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&request.client_id)
    .bind(crypto::hash(&device_code))
    .bind(&user_code)
    .bind(DEVICE_INTERVAL)
    .bind(OffsetDateTime::now_utc() + DEVICE_TTL)
    .execute(&state.db)
    .await?;
    let origin = &state.config.account_origin;
    let query: String = form_urlencoded::Serializer::new(String::new()).append_pair("user_code", &user_code).finish();
    Ok(Json(json!({
        "device_code": device_code,
        "user_code": user_code,
        "verification_uri": format!("{origin}/device"),
        "verification_uri_complete": format!("{origin}/device?{query}"),
        "expires_in": DEVICE_TTL.whole_seconds(),
        "interval": DEVICE_INTERVAL,
    })))
}

#[derive(Deserialize)]
pub struct DevicePoll {
    client_id: String,
    device_code: String,
}

/// `POST /auth/device/poll`: the JSON form of the device-code grant.
pub async fn device_poll(
    State(state): State<AppState>,
    Json(request): Json<DevicePoll>,
) -> Result<Json<TokenResponse>, OAuthFailure> {
    redeem_device_code(&state.db, &request.client_id, &request.device_code).await.map(Json)
}

#[derive(sqlx::FromRow)]
struct DeviceRow {
    id: Uuid,
    client_id: String,
    interval_seconds: i32,
    expires_at: OffsetDateTime,
    last_polled_at: Option<OffsetDateTime>,
    account_id: Option<Uuid>,
    decision: Option<String>,
    consumed_at: Option<OffsetDateTime>,
}

async fn redeem_device_code(db: &PgPool, client_id: &str, device_code: &str) -> Result<TokenResponse, OAuthFailure> {
    known_client(client_id)?;
    let mut tx = db.begin().await?;
    let row: Option<DeviceRow> = sqlx::query_as(
        "SELECT id, client_id, interval_seconds, expires_at, last_polled_at, account_id, decision, consumed_at
         FROM device_authorizations WHERE device_code_hash = $1 FOR UPDATE",
    )
    .bind(crypto::hash(device_code))
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Err(OAuthError::new("invalid_grant", "unknown device code").into());
    };
    if row.client_id != client_id {
        return Err(OAuthError::new("invalid_grant", "this device code belongs to another client").into());
    }
    if row.consumed_at.is_some() {
        return Err(OAuthError::new("invalid_grant", "this device code was already used").into());
    }
    let now = OffsetDateTime::now_utc();
    if row.expires_at <= now {
        return Err(OAuthError::new("expired_token", "the sign-in request expired; start again").into());
    }
    match (row.decision.as_deref(), row.account_id) {
        (Some("denied"), _) => {
            return Err(OAuthError::new("access_denied", "the sign-in was declined").into());
        }
        (Some("approved"), Some(account_id)) => {
            sqlx::query("UPDATE device_authorizations SET consumed_at = now() WHERE id = $1")
                .bind(row.id)
                .execute(&mut *tx)
                .await?;
            let tokens = issue_grant(&mut tx, account_id, client_id, client_scope(client_id)).await?;
            tx.commit().await?;
            return Ok(tokens);
        }
        _ => {}
    }
    let too_soon =
        row.last_polled_at.is_some_and(|last| now - last < Duration::seconds(i64::from(row.interval_seconds)));
    if too_soon {
        // RFC 8628 §3.5: a client that polls too fast waits 5 s longer.
        sqlx::query(
            "UPDATE device_authorizations SET last_polled_at = now(), interval_seconds = interval_seconds + 5 WHERE id = $1",
        )
        .bind(row.id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        return Err(OAuthError::new("slow_down", "poll less often").into());
    }
    sqlx::query("UPDATE device_authorizations SET last_polled_at = now() WHERE id = $1")
        .bind(row.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Err(OAuthError::new("authorization_pending", "waiting for the person to approve").into())
}

/// The person on the account site approves or declines a code.
pub async fn decide_device(db: &PgPool, user_code: &str, account_id: Uuid, approve: bool) -> sqlx::Result<bool> {
    let decision = if approve { "approved" } else { "denied" };
    let updated = sqlx::query(
        "UPDATE device_authorizations SET decision = $1, account_id = $2
         WHERE user_code = $3 AND decision IS NULL AND consumed_at IS NULL AND expires_at > now()",
    )
    .bind(decision)
    .bind(account_id)
    .bind(user_code)
    .execute(db)
    .await?;
    Ok(updated.rows_affected() == 1)
}

/// The app asking to sign in, if `user_code` is still open.
pub async fn pending_device(db: &PgPool, user_code: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "SELECT client_id FROM device_authorizations
         WHERE user_code = $1 AND decision IS NULL AND consumed_at IS NULL AND expires_at > now()",
    )
    .bind(user_code)
    .fetch_optional(db)
    .await
}

/// What a token may do. The Buwiz apps get `app`; AI agents get `mcp`,
/// which reads but never files.
pub const SCOPE_APP: &str = "app";
pub const SCOPE_MCP: &str = "mcp";
/// Buwiz Books: reads profiles by opaque id; never files or sees TINs.
pub const SCOPE_BOOKS: &str = "books";

#[derive(Serialize)]
pub struct TokenResponse {
    access_token: String,
    token_type: &'static str,
    expires_in: i64,
    refresh_token: String,
    scope: String,
}

pub(crate) async fn issue_grant(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account_id: Uuid,
    client_id: &str,
    scope: &str,
) -> sqlx::Result<TokenResponse> {
    let grant_id: Uuid =
        sqlx::query_scalar("INSERT INTO oauth_grants (account_id, client_id, scope) VALUES ($1, $2, $3) RETURNING id")
            .bind(account_id)
            .bind(client_id)
            .bind(scope)
            .fetch_one(&mut **tx)
            .await?;
    issue_tokens(tx, grant_id, scope).await
}

async fn issue_tokens(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    grant_id: Uuid,
    scope: &str,
) -> sqlx::Result<TokenResponse> {
    let now = OffsetDateTime::now_utc();
    let access_token = crypto::new_token("bwa_");
    let refresh_token = crypto::new_token("bwr_");
    sqlx::query("INSERT INTO oauth_access_tokens (grant_id, token_hash, expires_at) VALUES ($1, $2, $3)")
        .bind(grant_id)
        .bind(crypto::hash(&access_token))
        .bind(now + ACCESS_TTL)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO oauth_refresh_tokens (grant_id, token_hash, expires_at) VALUES ($1, $2, $3)")
        .bind(grant_id)
        .bind(crypto::hash(&refresh_token))
        .bind(now + REFRESH_TTL)
        .execute(&mut **tx)
        .await?;
    Ok(TokenResponse {
        access_token,
        token_type: "Bearer",
        expires_in: ACCESS_TTL.whole_seconds(),
        refresh_token,
        scope: scope.to_owned(),
    })
}

#[derive(Deserialize)]
pub struct TokenRequest {
    grant_type: String,
    client_id: String,
    refresh_token: Option<String>,
    device_code: Option<String>,
    code: Option<String>,
    redirect_uri: Option<String>,
    code_verifier: Option<String>,
}

/// `POST /oauth/token` (form-encoded): `refresh_token`, the RFC 8628
/// `device_code` grant, and `authorization_code` with PKCE for agents.
pub async fn token(
    State(state): State<AppState>,
    Form(request): Form<TokenRequest>,
) -> Result<Json<TokenResponse>, OAuthFailure> {
    match request.grant_type.as_str() {
        "refresh_token" => {
            let Some(refresh_token) = request.refresh_token else {
                return Err(OAuthError::new("invalid_request", "refresh_token is required").into());
            };
            refresh(&state.db, &request.client_id, &refresh_token).await.map(Json)
        }
        "urn:ietf:params:oauth:grant-type:device_code" => {
            let Some(device_code) = request.device_code else {
                return Err(OAuthError::new("invalid_request", "device_code is required").into());
            };
            redeem_device_code(&state.db, &request.client_id, &device_code).await.map(Json)
        }
        "authorization_code" => {
            let (Some(code), Some(redirect_uri), Some(verifier)) =
                (request.code, request.redirect_uri, request.code_verifier)
            else {
                return Err(
                    OAuthError::new("invalid_request", "code, redirect_uri and code_verifier are required").into()
                );
            };
            crate::agents::redeem_code(&state.db, &request.client_id, &code, &redirect_uri, &verifier).await.map(Json)
        }
        _ => {
            Err(OAuthError::new("unsupported_grant_type", "use authorization_code, refresh_token or device_code")
                .into())
        }
    }
}

async fn refresh(db: &PgPool, client_id: &str, refresh_token: &str) -> Result<TokenResponse, OAuthFailure> {
    let mut tx = db.begin().await?;
    #[derive(sqlx::FromRow)]
    struct Stored {
        token_id: Uuid,
        grant_id: Uuid,
        grant_client: String,
        scope: String,
        expires_at: OffsetDateTime,
        used_at: Option<OffsetDateTime>,
        revoked_at: Option<OffsetDateTime>,
    }
    let row: Option<Stored> = sqlx::query_as(
        "SELECT r.id AS token_id, g.id AS grant_id, g.client_id AS grant_client, g.scope, r.expires_at, r.used_at,
                g.revoked_at
         FROM oauth_refresh_tokens r JOIN oauth_grants g ON g.id = r.grant_id
         WHERE r.token_hash = $1 FOR UPDATE OF r, g",
    )
    .bind(crypto::hash(refresh_token))
    .fetch_optional(&mut *tx)
    .await?;
    let invalid = || OAuthError::new("invalid_grant", "sign in again").into();
    let Some(Stored { token_id, grant_id, grant_client, scope, expires_at, used_at, revoked_at }) = row else {
        return Err(invalid());
    };
    if grant_client != client_id || revoked_at.is_some() || expires_at <= OffsetDateTime::now_utc() {
        return Err(invalid());
    }
    if used_at.is_some() {
        // A rotated-out token came back: someone else may hold a copy. End
        // the whole sign-in.
        revoke_grant(&mut tx, grant_id, "refresh_token_reuse").await?;
        tx.commit().await?;
        tracing::warn!(%grant_id, "refresh token reuse; grant revoked");
        return Err(invalid());
    }
    sqlx::query("UPDATE oauth_refresh_tokens SET used_at = now() WHERE id = $1")
        .bind(token_id)
        .execute(&mut *tx)
        .await?;
    let tokens = issue_tokens(&mut tx, grant_id, &scope).await?;
    tx.commit().await?;
    Ok(tokens)
}

async fn revoke_grant(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    grant_id: Uuid,
    reason: &str,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE oauth_grants SET revoked_at = now(), revoke_reason = $2 WHERE id = $1 AND revoked_at IS NULL")
        .bind(grant_id)
        .bind(reason)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// The account behind a valid `Authorization: Bearer` access token.
pub struct Bearer {
    pub account: Account,
    pub grant_id: Uuid,
    pub scope: String,
}

impl Bearer {
    /// Refuses every token but the BIR app's: Books and AI agents never file.
    pub fn require_app(&self) -> Result<(), ApiError> {
        if self.scope == SCOPE_APP {
            Ok(())
        } else {
            Err(ApiError::forbidden("insufficient_scope", "only the Buwiz BIR app can file returns"))
        }
    }
}

impl FromRequestParts<AppState> for Bearer {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let token = parts
            .headers
            .get(http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or(ApiError::InvalidToken)?;
        let row: Option<(Uuid, String, Uuid, String)> = sqlx::query_as(
            "SELECT g.id, g.scope, a.id, a.email
             FROM oauth_access_tokens t
             JOIN oauth_grants g ON g.id = t.grant_id
             JOIN accounts a ON a.id = g.account_id
             WHERE t.token_hash = $1 AND t.expires_at > now() AND g.revoked_at IS NULL",
        )
        .bind(crypto::hash(token.trim()))
        .fetch_optional(&state.db)
        .await?;
        let (grant_id, scope, id, email) = row.ok_or(ApiError::InvalidToken)?;
        Ok(Self { account: Account { id, email }, grant_id, scope })
    }
}

/// `GET /me`.
pub async fn me(bearer: Bearer) -> Json<Value> {
    Json(json!({ "user_id": bearer.account.id, "email": bearer.account.email }))
}

/// `POST /api/auth/logout`: ends this app's sign-in.
pub async fn logout(State(state): State<AppState>, bearer: Bearer) -> Result<Json<Value>, ApiError> {
    let mut tx = state.db.begin().await?;
    revoke_grant(&mut tx, bearer.grant_id, "signed_out").await?;
    tx.commit().await?;
    Ok(Json(json!({})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_codes_are_two_groups_of_four_from_the_alphabet() {
        for _ in 0..100 {
            let code = new_user_code();
            assert_eq!(code.len(), 9);
            assert_eq!(&code[4..5], "-");
            assert!(code.bytes().filter(|byte| *byte != b'-').all(|byte| USER_CODE_ALPHABET.contains(&byte)));
        }
    }

    #[test]
    fn typed_user_codes_normalize() {
        assert_eq!(normalize_user_code("bcdf ghjk").as_deref(), Some("BCDF-GHJK"));
        assert_eq!(normalize_user_code("BCDF-GHJK").as_deref(), Some("BCDF-GHJK"));
        assert_eq!(normalize_user_code("BCD"), None);
    }
}
