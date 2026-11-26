//! Accounts, email sign-in codes, and browser sessions.

use rand::Rng;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::account::notice::Notice;
use crate::account::roles;
use crate::base::crypto;

pub const CODE_TTL: Duration = Duration::minutes(10);
const CODES_PER_HOUR: i64 = 5;
/// Codes mailed to addresses without an account, all of them together, in
/// an hour and in a day. Whoever gets past the form's other checks
/// ([`crate::account::guard`]) can still mail strangers no more than this;
/// an address that has an account is never held back by it.
const NEW_ADDRESS_CODES_PER_HOUR: i64 = 20;
const NEW_ADDRESS_CODES_PER_DAY: i64 = 60;
/// The advisory lock that counting and adding a code for an address
/// without an account are done under, so two posts at once cannot both
/// take the last place.
const NEW_ADDRESS_LOCK: i64 = 0x6275_7769_7a01;
/// Codes the apps mail without an app check (Turnstile) to addresses
/// without an account, together: a budget of their own, so token-less
/// requests never use up the web's ([`NewAddressBudget::Unchecked`]).
const UNCHECKED_NEW_ADDRESS_CODES_PER_HOUR: i64 = 10;
const UNCHECKED_NEW_ADDRESS_CODES_PER_DAY: i64 = 30;
const UNCHECKED_NEW_ADDRESS_LOCK: i64 = 0x6275_7769_7a03;
pub(crate) const CODE_ATTEMPTS: i32 = 5;
pub const SESSION_TTL: Duration = Duration::days(30);
/// How long sign-in records are kept: codes from when they were made,
/// device and agent sign-ins from when they expired ([`upkeep`]).
const SIGN_IN_RECORDS_DAYS: i32 = 30;

/// What a code in `email_codes` is for. A code works only for its own
/// purpose: a sign-in code never deletes an account, and a deletion code
/// never signs anyone in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    SignIn,
    /// Deleting the account ([`crate::account::deletion`]).
    Delete,
    /// Signing in from an app ([`crate::account::app_sign_in`]): it works
    /// only with the request it was made for ([`try_code_row`]).
    AppSignIn,
    /// Resetting a forgotten app PIN ([`crate::account::pin_reset`]): it
    /// works only with the request it was made for ([`try_code_row`]).
    PinReset,
}

impl Purpose {
    fn as_str(self) -> &'static str {
        match self {
            Self::SignIn => "sign_in",
            Self::Delete => "delete",
            Self::AppSignIn => "app_sign_in",
            Self::PinReset => "pin_reset",
        }
    }
}

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
    /// The limit for addresses without an account is reached. The person
    /// is not told: the answer must not differ from an account's.
    #[error("Too many addresses without an account asked for a code.")]
    Busy,
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

/// How a sign-in code is stored: in `email_codes.code_hash`, and an
/// account's fixed code in `accounts.fixed_code_hash`, so the one can be
/// copied into the other.
pub(crate) fn code_hash(email: &str, code: &str) -> Vec<u8> {
    crypto::hash(&format!("{email}:{code}"))
}

/// A code [`issue_code`] stored.
pub enum IssuedCode {
    /// A random code, for the caller to mail.
    Mail(String),
    /// The account's fixed code ([`roles::fixed_code_sql`]): its hash was stored,
    /// its holder knows the code, and nothing is mailed.
    Fixed,
}

impl std::fmt::Debug for IssuedCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mail(_) => f.write_str("Mail([hidden])"),
            Self::Fixed => f.write_str("Fixed"),
        }
    }
}

/// Stores a sign-in code for `email`: a random 6-digit one, returned for
/// mailing, or, for an account with a fixed code that is still a plain
/// test account ([`roles::fixed_code_sql`]; `admin_emails` is `ADMIN_EMAILS`), a
/// copy of that code's hash, with nothing to mail. Either way the code is
/// stored the same way after the same limits, so the ten minutes and the
/// five wrong guesses hold for both.
pub async fn issue_code(db: &PgPool, email: &str, admin_emails: &[String]) -> Result<IssuedCode, CodeError> {
    let mut tx = db.begin().await?;
    lock_address(&mut tx, email).await?;
    let (hash, issued) = match plan_code(&mut tx, email, admin_emails, NewAddressBudget::Web).await? {
        Planned::Code(hash, issued) => (hash, issued),
        // The web form stores nothing and answers as if mailed.
        Planned::Busy => return Err(CodeError::Busy),
    };
    add_code(&mut tx, email, hash, Purpose::SignIn, CODE_TTL, false, true).await?;
    tx.commit().await?;
    Ok(issued)
}

/// Which budget a code for an address without an account comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NewAddressBudget {
    /// The web's, 20 an hour and 60 a day: the sign-in form, and the apps
    /// with an app check.
    Web,
    /// The apps' without an app check, 10 an hour and 30 a day. Its codes
    /// are marked `unchecked` and do not count towards the web's.
    Unchecked,
}

/// Takes the address's lock for this transaction: every code for an
/// address, of any purpose, is counted and added under it, so requests at
/// once cannot all pass the count of five an hour.
pub(crate) async fn lock_address(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, email: &str) -> sqlx::Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('code ' || $1, 0))")
        .bind(email)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// What [`plan_code`] found a sign-in code for an address may be.
pub(crate) enum Planned {
    /// A code to store with this hash (for [`add_code`]) and mail, or not
    /// (a fixed code).
    Code(Vec<u8>, IssuedCode),
    /// The address has no account and `budget`'s mail to new addresses is
    /// used up: nothing may be mailed to it now.
    Busy,
}

/// The checks before a sign-in code for `email` is stored, in the caller's
/// transaction, which holds [`lock_address`]: the address's five codes an
/// hour ([`CodeError::TooMany`]), then, for an address without an account,
/// `budget`'s mail to new addresses, whose lock is held until the caller
/// commits. Writes nothing; [`add_code`] stores what it found.
pub(crate) async fn plan_code(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    email: &str,
    admin_emails: &[String],
    budget: NewAddressBudget,
) -> Result<Planned, CodeError> {
    if codes_this_hour(tx, email).await? >= CODES_PER_HOUR {
        return Err(CodeError::TooMany);
    }
    let known: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accounts WHERE email = $1)")
        .bind(email)
        .fetch_one(&mut **tx)
        .await?;
    if !known {
        let (lock, per_hour, per_day, unchecked) = match budget {
            NewAddressBudget::Web => (NEW_ADDRESS_LOCK, NEW_ADDRESS_CODES_PER_HOUR, NEW_ADDRESS_CODES_PER_DAY, false),
            NewAddressBudget::Unchecked => (
                UNCHECKED_NEW_ADDRESS_LOCK,
                UNCHECKED_NEW_ADDRESS_CODES_PER_HOUR,
                UNCHECKED_NEW_ADDRESS_CODES_PER_DAY,
                true,
            ),
        };
        sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(lock).execute(&mut **tx).await?;
        // Codes mailed: one held back ([`add_code`]'s `mailed`) is no mail.
        let (hour, day): (i64, i64) = sqlx::query_as(
            "SELECT count(*) FILTER (WHERE c.created_at > now() - interval '1 hour'), count(*)
             FROM email_codes c
             WHERE c.created_at > now() - interval '1 day' AND c.unchecked = $1 AND c.mailed
               AND NOT EXISTS (SELECT 1 FROM accounts a WHERE a.email = c.email)",
        )
        .bind(unchecked)
        .fetch_one(&mut **tx)
        .await?;
        if hour >= per_hour || day >= per_day {
            tracing::warn!(hour, day, ?budget, "sign-in codes for addresses without an account are at their limit");
            return Ok(Planned::Busy);
        }
    }
    let fixed: Option<Vec<u8>> = if known {
        sqlx::query_scalar::<_, Option<Vec<u8>>>(&format!(
            "SELECT a.fixed_code_hash FROM accounts a WHERE a.email = $1 AND {}",
            roles::fixed_code_sql("a", "$2")
        ))
        .bind(email)
        .bind(admin_emails)
        .fetch_optional(&mut **tx)
        .await?
        .flatten()
    } else {
        None
    };
    Ok(match fixed {
        Some(hash) => Planned::Code(hash, IssuedCode::Fixed),
        None => {
            let code = random_code();
            Planned::Code(code_hash(email, &code), IssuedCode::Mail(code))
        }
    })
}

/// A real code for `email`, made and stored as a mailed one is (the same
/// digits' generator and hash), that is never mailed: for an app's request
/// when [`plan_code`] found the address [`Planned::Busy`], so the request
/// has a code and is answered and held exactly as one that was mailed.
/// Nobody knows its digits; it can only be guessed, at a mailed code's odds.
pub(crate) fn unmailed_code(email: &str) -> Vec<u8> {
    code_hash(email, &random_code())
}

/// Stores a code for deleting the account of `email` and returns it, for
/// mailing. Always a random one, mailed to the address: a test account's
/// fixed code never deletes it. It counts towards the address's five codes
/// an hour, as a sign-in code does, and the newest one asked for is the
/// only one that works.
pub(crate) async fn issue_deletion_code(db: &PgPool, email: &str) -> Result<String, CodeError> {
    let mut tx = db.begin().await?;
    lock_address(&mut tx, email).await?;
    if codes_this_hour(&mut tx, email).await? >= CODES_PER_HOUR {
        return Err(CodeError::TooMany);
    }
    let code = random_code();
    add_code(&mut tx, email, code_hash(email, &code), Purpose::Delete, CODE_TTL, false, true).await?;
    tx.commit().await?;
    Ok(code)
}

/// Whether the account of `email` signs in with a fixed code
/// ([`roles::fixed_code_sql`]), so nothing is mailed to it.
pub(crate) async fn has_fixed_code(
    db: impl sqlx::PgExecutor<'_>,
    email: &str,
    admin_emails: &[String],
) -> sqlx::Result<bool> {
    let fixed: Option<bool> =
        sqlx::query_scalar(&format!("SELECT {} FROM accounts a WHERE a.email = $1", roles::fixed_code_sql("a", "$2")))
            .bind(email)
            .bind(admin_emails)
            .fetch_optional(db)
            .await?;
    Ok(fixed.unwrap_or(false))
}

fn random_code() -> String {
    format!("{:06}", rand::rng().random_range(0..1_000_000))
}

/// Codes of any purpose made for `email` in the last hour.
async fn codes_this_hour(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, email: &str) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM email_codes WHERE email = $1 AND created_at > now() - interval '1 hour'")
        .bind(email)
        .fetch_one(&mut **tx)
        .await
}

/// Stores a code for `email` and returns its id. `unchecked`: an app asked
/// for it without an app check. `mailed`: false for an
/// [`unmailed_code`], which counts towards the address's five an hour as
/// every code does, and towards no budget of mail (the new addresses', the
/// apps' testing cap).
pub(crate) async fn add_code(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    email: &str,
    hash: Vec<u8>,
    purpose: Purpose,
    ttl: Duration,
    unchecked: bool,
    mailed: bool,
) -> sqlx::Result<Uuid> {
    // Dated when it is added, not when the transaction began: the code
    // added last (after waiting for the lock) must be the newest, because
    // only the newest can be redeemed.
    sqlx::query_scalar(
        "INSERT INTO email_codes (email, code_hash, expires_at, created_at, purpose, unchecked, mailed)
         VALUES ($1, $2, $3, clock_timestamp(), $4, $5, $6) RETURNING id",
    )
    .bind(email)
    .bind(hash)
    .bind(OffsetDateTime::now_utc() + ttl)
    .bind(purpose.as_str())
    .bind(unchecked)
    .bind(mailed)
    .fetch_one(&mut **tx)
    .await
}

/// What trying a code came to ([`try_code_row`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tried {
    /// It was the code, which is now used up.
    Taken,
    /// It was wrong, or the code's tries were used up already: the tries
    /// it has left (0 when none).
    Wrong { attempts_left: i32 },
    /// The code is not open (used, expired or gone).
    NoCode,
}

/// Uses up the newest open code of `purpose` for `email` if `code` is it.
/// Only the newest counts, so a code asked for earlier stops working once
/// a new one is asked for. A wrong code counts towards that code's five
/// tries; the caller commits either way, so the try stays counted.
pub(crate) async fn take_code(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    email: &str,
    purpose: Purpose,
    code: &str,
) -> sqlx::Result<bool> {
    let row: Option<(Uuid, Vec<u8>, i32)> = sqlx::query_as(
        "SELECT id, code_hash, attempts FROM email_codes
         WHERE email = $1 AND purpose = $2 AND consumed_at IS NULL AND expires_at > now()
         ORDER BY created_at DESC LIMIT 1 FOR UPDATE",
    )
    .bind(email)
    .bind(purpose.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    Ok(try_open(tx, email, row, code).await? == Tried::Taken)
}

/// Uses up the code `code_id` (an app's, for `email`, of `purpose`) if
/// `code` is it and it is open: only that one code is tried, whatever else
/// is open for the address, and a code of another purpose is no code here.
/// A wrong code counts towards its five tries, as in [`take_code`].
pub(crate) async fn try_code_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    code_id: Uuid,
    email: &str,
    purpose: Purpose,
    code: &str,
) -> sqlx::Result<Tried> {
    let row: Option<(Uuid, Vec<u8>, i32)> = sqlx::query_as(
        "SELECT id, code_hash, attempts FROM email_codes
         WHERE id = $1 AND email = $2 AND purpose = $3 AND consumed_at IS NULL AND expires_at > now() FOR UPDATE",
    )
    .bind(code_id)
    .bind(email)
    .bind(purpose.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    try_open(tx, email, row, code).await
}

/// Tries `code` on an open code row (id, hash, tries so far).
async fn try_open(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    email: &str,
    row: Option<(Uuid, Vec<u8>, i32)>,
    code: &str,
) -> sqlx::Result<Tried> {
    let code: String = code.chars().filter(char::is_ascii_digit).collect();
    let Some((code_id, stored, attempts)) = row else {
        return Ok(Tried::NoCode);
    };
    if attempts >= CODE_ATTEMPTS {
        return Ok(Tried::Wrong { attempts_left: 0 });
    }
    if code.len() != 6 || !crypto::same(&stored, &code_hash(email, &code)) {
        sqlx::query("UPDATE email_codes SET attempts = attempts + 1 WHERE id = $1")
            .bind(code_id)
            .execute(&mut **tx)
            .await?;
        return Ok(Tried::Wrong { attempts_left: CODE_ATTEMPTS - attempts - 1 });
    }
    sqlx::query("UPDATE email_codes SET consumed_at = now() WHERE id = $1").bind(code_id).execute(&mut **tx).await?;
    Ok(Tried::Taken)
}

/// The account of `email`, made now if there is none: what a right
/// sign-in code leads to.
pub(crate) async fn account_for(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, email: &str) -> sqlx::Result<Account> {
    sqlx::query("INSERT INTO accounts (email) VALUES ($1) ON CONFLICT (email) DO NOTHING")
        .bind(email)
        .execute(&mut **tx)
        .await?;
    sqlx::query_as("SELECT id, email FROM accounts WHERE email = $1").bind(email).fetch_one(&mut **tx).await
}

/// Checks the newest open sign-in code for `email`, in `tx` (the caller
/// sets its lock timeout). A match signs the person in, creating the
/// account the first time.
pub async fn redeem_code(
    mut tx: sqlx::Transaction<'_, sqlx::Postgres>,
    email: &str,
    code: &str,
) -> Result<Account, CodeError> {
    if !take_code(&mut tx, email, Purpose::SignIn, code).await? {
        tx.commit().await?;
        return Err(CodeError::Invalid);
    }
    let account = account_for(&mut tx, email).await?;
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

/// The account of a website session. A banned account has none: a ban
/// ends its sessions, and this refuses any that is left.
pub async fn session_account(db: &PgPool, token: &str) -> sqlx::Result<Option<Account>> {
    sqlx::query_as(
        "SELECT a.id, a.email FROM web_sessions s JOIN accounts a ON a.id = s.account_id
         WHERE s.token_hash = $1 AND s.revoked_at IS NULL AND s.expires_at > now() AND a.banned_at IS NULL",
    )
    .bind(crypto::hash(token))
    .fetch_optional(db)
    .await
}

/// The account of a website session, banned or not: only for deleting the
/// account (`/delete`), which a banned account may do. Every other page
/// asks [`session_account`].
pub async fn deletion_session_account(db: &PgPool, token: &str) -> sqlx::Result<Option<Account>> {
    sqlx::query_as(
        "SELECT a.id, a.email FROM web_sessions s JOIN accounts a ON a.id = s.account_id
         WHERE s.token_hash = $1 AND s.revoked_at IS NULL AND s.expires_at > now()",
    )
    .bind(crypto::hash(token))
    .fetch_optional(db)
    .await
}

pub async fn is_banned(db: &PgPool, account_id: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT banned_at IS NOT NULL FROM accounts WHERE id = $1").bind(account_id).fetch_one(db).await
}

pub async fn end_session(db: &PgPool, token: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE web_sessions SET revoked_at = now() WHERE token_hash = $1 AND revoked_at IS NULL")
        .bind(crypto::hash(token))
        .execute(db)
        .await?;
    Ok(())
}

/// Ends an account's browser sessions and the sign-ins approved but not yet
/// picked up by an app or agent; with `grants`, also every app and agent
/// grant (which ends their access and refresh tokens). Staff sign an account
/// out everywhere with `grants`; a ban leaves the grants (the API refuses a
/// banned account's calls anyway).
pub(crate) async fn end_sign_ins(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account_id: Uuid,
    grants: bool,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE web_sessions SET revoked_at = now() WHERE account_id = $1 AND revoked_at IS NULL")
        .bind(account_id)
        .execute(&mut **tx)
        .await?;
    if grants {
        sqlx::query(
            "UPDATE oauth_grants SET revoked_at = now(), revoke_reason = 'signed_out_everywhere'
             WHERE account_id = $1 AND revoked_at IS NULL",
        )
        .bind(account_id)
        .execute(&mut **tx)
        .await?;
    }
    sqlx::query("UPDATE device_authorizations SET consumed_at = now() WHERE account_id = $1 AND consumed_at IS NULL")
        .bind(account_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE oauth_codes SET used_at = now() WHERE account_id = $1 AND used_at IS NULL")
        .bind(account_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// The hourly upkeep: deletes sign-in records once nothing needs them.
/// Codes go 30 days after they were made (the limits per address look back
/// an hour and a day), and device, in-app and agent sign-ins and PIN reset
/// requests 30 days after they expired (nothing reads one after that). The
/// in-app testing cap's alert days and the new-device mails' record go
/// after 30.
pub async fn upkeep(db: &PgPool) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM email_codes WHERE created_at < now() - make_interval(days => $1)")
        .bind(SIGN_IN_RECORDS_DAYS)
        .execute(db)
        .await?;
    for table in ["device_authorizations", "oauth_codes", "app_sign_ins", "pin_resets"] {
        sqlx::query(&format!("DELETE FROM {table} WHERE expires_at < now() - make_interval(days => $1)"))
            .bind(SIGN_IN_RECORDS_DAYS)
            .execute(db)
            .await?;
    }
    for table in ["app_sign_in_alerts", "new_device_mails"] {
        sqlx::query(&format!("DELETE FROM {table} WHERE sent_at < now() - make_interval(days => $1)"))
            .bind(SIGN_IN_RECORDS_DAYS)
            .execute(db)
            .await?;
    }
    Ok(())
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
