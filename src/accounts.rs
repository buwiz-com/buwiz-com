//! Accounts, email sign-in codes, and browser sessions.

use rand::Rng;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::crypto;
use crate::notice::Notice;

const CODE_TTL: Duration = Duration::minutes(10);
const CODES_PER_HOUR: i64 = 5;
const CODE_ATTEMPTS: i32 = 5;
pub const SESSION_TTL: Duration = Duration::days(30);

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Account {
    pub id: Uuid,
    pub email: String,
}

/// Lowercased, trimmed address, or why it is not one.
pub fn normalize_email(raw: &str) -> Result<String, Notice> {
    let email = raw.trim().to_lowercase();
    let Some((local, domain)) = email.split_once('@') else {
        return Err(Notice::EmailInvalid);
    };
    let valid = !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && email.len() <= 254
        && !email.chars().any(|ch| ch.is_whitespace() || ch.is_control());
    if valid { Ok(email) } else { Err(Notice::EmailInvalid) }
}

#[derive(Debug, thiserror::Error)]
pub enum CodeError {
    #[error("Too many codes were sent to this address. Try again in an hour.")]
    TooMany,
    #[error("That code is wrong or has expired. Request a new one.")]
    Invalid,
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for CodeError {
    fn from(error: sqlx::Error) -> Self {
        Self::Internal(error.into())
    }
}

fn code_hash(email: &str, code: &str) -> Vec<u8> {
    crypto::hash(&format!("{email}:{code}"))
}

/// Creates a 6-digit code for `email` and returns it for mailing.
pub async fn issue_code(db: &PgPool, email: &str) -> Result<String, CodeError> {
    let recent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM email_codes WHERE email = $1 AND created_at > now() - interval '1 hour'",
    )
    .bind(email)
    .fetch_one(db)
    .await?;
    if recent >= CODES_PER_HOUR {
        return Err(CodeError::TooMany);
    }
    let code = format!("{:06}", rand::rng().random_range(0..1_000_000));
    sqlx::query("INSERT INTO email_codes (email, code_hash, expires_at) VALUES ($1, $2, $3)")
        .bind(email)
        .bind(code_hash(email, &code))
        .bind(OffsetDateTime::now_utc() + CODE_TTL)
        .execute(db)
        .await?;
    Ok(code)
}

/// Checks the newest open code for `email`. A match signs the person in,
/// creating the account the first time.
pub async fn redeem_code(db: &PgPool, email: &str, code: &str) -> Result<Account, CodeError> {
    let code: String = code.chars().filter(char::is_ascii_digit).collect();
    let mut tx = db.begin().await?;
    let row: Option<(Uuid, Vec<u8>, i32)> = sqlx::query_as(
        "SELECT id, code_hash, attempts FROM email_codes
         WHERE email = $1 AND consumed_at IS NULL AND expires_at > now()
         ORDER BY created_at DESC LIMIT 1 FOR UPDATE",
    )
    .bind(email)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((code_id, stored, attempts)) = row else {
        return Err(CodeError::Invalid);
    };
    if attempts >= CODE_ATTEMPTS {
        return Err(CodeError::Invalid);
    }
    if code.len() != 6 || stored != code_hash(email, &code) {
        sqlx::query("UPDATE email_codes SET attempts = attempts + 1 WHERE id = $1")
            .bind(code_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Err(CodeError::Invalid);
    }
    sqlx::query("UPDATE email_codes SET consumed_at = now() WHERE id = $1").bind(code_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO accounts (email) VALUES ($1) ON CONFLICT (email) DO NOTHING")
        .bind(email)
        .execute(&mut *tx)
        .await?;
    let account =
        sqlx::query_as("SELECT id, email FROM accounts WHERE email = $1").bind(email).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(account)
}

/// Starts a browser session and returns its cookie value.
pub async fn start_session(db: &PgPool, account_id: Uuid) -> sqlx::Result<String> {
    let token = crypto::new_token("bws_");
    sqlx::query("INSERT INTO web_sessions (account_id, token_hash, expires_at) VALUES ($1, $2, $3)")
        .bind(account_id)
        .bind(crypto::hash(&token))
        .bind(OffsetDateTime::now_utc() + SESSION_TTL)
        .execute(db)
        .await?;
    Ok(token)
}

pub async fn session_account(db: &PgPool, token: &str) -> sqlx::Result<Option<Account>> {
    sqlx::query_as(
        "SELECT a.id, a.email FROM web_sessions s JOIN accounts a ON a.id = s.account_id
         WHERE s.token_hash = $1 AND s.revoked_at IS NULL AND s.expires_at > now()",
    )
    .bind(crypto::hash(token))
    .fetch_optional(db)
    .await
}

pub async fn end_session(db: &PgPool, token: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE web_sessions SET revoked_at = now() WHERE token_hash = $1 AND revoked_at IS NULL")
        .bind(crypto::hash(token))
        .execute(db)
        .await?;
    Ok(())
}

/// Ends every sign-in of an account: browser sessions, app and agent
/// grants (which ends their access and refresh tokens), and sign-ins
/// approved but not yet picked up by the app or agent.
pub async fn sign_out_everywhere(db: &PgPool, account_id: Uuid) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query("UPDATE web_sessions SET revoked_at = now() WHERE account_id = $1 AND revoked_at IS NULL")
        .bind(account_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE oauth_grants SET revoked_at = now(), revoke_reason = 'signed_out_everywhere'
         WHERE account_id = $1 AND revoked_at IS NULL",
    )
    .bind(account_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE device_authorizations SET consumed_at = now() WHERE account_id = $1 AND consumed_at IS NULL")
        .bind(account_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE oauth_codes SET used_at = now() WHERE account_id = $1 AND used_at IS NULL")
        .bind(account_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    tracing::info!(%account_id, "signed out everywhere");
    Ok(())
}

/// Ends one app's or agent's sign-in on the account.
pub async fn sign_out_app(db: &PgPool, account_id: Uuid, grant_id: Uuid) -> sqlx::Result<bool> {
    let updated = sqlx::query(
        "UPDATE oauth_grants SET revoked_at = now(), revoke_reason = 'signed_out_by_staff'
         WHERE id = $1 AND account_id = $2 AND revoked_at IS NULL",
    )
    .bind(grant_id)
    .bind(account_id)
    .execute(db)
    .await?;
    Ok(updated.rows_affected() == 1)
}

/// The value of cookie `name` in a `Cookie` header map.
pub fn cookie<'a>(headers: &'a http::HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(http::header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_checks_addresses() {
        assert_eq!(normalize_email("  Juan@Example.PH ").unwrap(), "juan@example.ph");
        assert!(normalize_email("juan").is_err());
        assert!(normalize_email("@example.ph").is_err());
        assert!(normalize_email("juan@example").is_err());
        assert!(normalize_email("ju an@example.ph").is_err());
    }

    #[test]
    fn reads_one_cookie_among_many() {
        let mut headers = http::HeaderMap::new();
        headers.append(http::header::COOKIE, "a=1; buwiz_session=bws_x".parse().unwrap());
        assert_eq!(cookie(&headers, "buwiz_session"), Some("bws_x"));
        assert_eq!(cookie(&headers, "missing"), None);
    }
}
