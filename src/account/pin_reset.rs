//! Resetting a forgotten app PIN by email code (docs/accounts.md,
//! "Resetting a forgotten app PIN"). The PIN lives on the device and the
//! server never sees it: the server checks that whoever asks can read the
//! account's mail, on this device, and tells the account.
//!
//! - `POST /api/pin-reset/start` mails a code (purpose `pin_reset`) to the
//!   account's own address and answers the request's id;
//! - `POST /api/pin-reset/verify` takes the code and writes one audit line
//!   in one transaction, commits, and only then mails the account a notice.
//!   It answers `200` only once the mail service took that notice; the app
//!   changes the PIN only after that `200`.
//!
//! Both take the BIR app's bearer only ([`Bearer::require_app`]). A request
//! belongs to the device it was made on: the live device row of the token's
//! sign-in, without which a real account is refused `409
//! device_not_registered`. A practice account (a store reviewer's, with a
//! fixed code, [`roles::fixed_code_sql`]) registers no device, so its
//! request belongs to the token's sign-in (its app grant) instead, and its
//! fixed code is taken at verify, as at sign-in; it is mailed no code, but
//! the notice all the same.

use std::borrow::Cow;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use http::{HeaderValue, StatusCode, header};
use serde::Deserialize;
use serde_json::json;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::account::accounts::{self, CODE_ATTEMPTS, CodeError, IssuedCode, NewAddressBudget, Planned, Purpose, Tried};
use crate::account::admin::{self, Line, Source};
use crate::account::app_sign_in::{begin_waiting, is_lock_timeout, mail_staff};
use crate::account::oauth::Bearer;
use crate::account::roles;
use crate::base::error::ApiError;
use crate::base::state::AppState;
use crate::uptime::keys::{Bucket, RateLimiter};

/// Starts one device (a practice account: one sign-in) may make in an
/// hour, and one account across its devices. Kept in memory.
pub const STARTS_PER_DEVICE: u32 = 3;
pub const STARTS_PER_ACCOUNT: u32 = 5;
/// Starts one account may make in a day, counted in the database
/// (`pin_resets`) under the account's lock, so a restart does not reset it.
pub const STARTS_PER_DAY: i64 = 10;
/// Tries at verify one device (or sign-in) may make in an hour, whatever
/// comes of them. Kept in memory.
pub const VERIFIES_PER_DEVICE: u32 = 10;
const WINDOW: Duration = Duration::hours(1);
/// Once a limiter's map is full (100,000 keys in an hour), the keys new in
/// that hour share this many times one key's limit. Each needs a signed-in
/// token, so real use never gets there.
const PAST_FULL: u32 = 100;
/// How long verify waits for the mail service to take the notice before it
/// answers `503 mail_unavailable`.
pub const NOTICE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// The longest `device_name` the app may report at start.
pub const DEVICE_NAME_CHARS: usize = 100;
/// What a practice account's notice calls the device when the app reported
/// no name.
pub const THIS_DEVICE: &str = "this device";
/// What `503 busy` asks to wait.
const BUSY_RETRY_S: i64 = 5;
/// What the account's five codes an hour ask to wait.
const HOUR_S: i64 = 3600;

/// The limits kept in memory ([`RateLimiter`]): a restart starts them
/// again. The day's count per account is in the database.
pub struct PinResetLimits {
    /// Starts per device (or practice sign-in).
    starts_by_device: RateLimiter<Uuid>,
    /// Starts per account, across its devices.
    starts_by_account: RateLimiter<Uuid>,
    /// Tries at verify per device (or practice sign-in).
    verifies: RateLimiter<Uuid>,
}

impl Default for PinResetLimits {
    fn default() -> Self {
        let limiter = |limit: u32| RateLimiter::with_overflow_budget(limit, WINDOW, limit * PAST_FULL);
        Self {
            starts_by_device: limiter(STARTS_PER_DEVICE),
            starts_by_account: limiter(STARTS_PER_ACCOUNT),
            verifies: limiter(VERIFIES_PER_DEVICE),
        }
    }
}

/// A refusal, in the shape of every API error, with `Retry-After` where it
/// applies. Never cached.
#[derive(Debug)]
enum Refusal {
    Api(ApiError),
    /// A refusal that asks to wait: `code_already_sent`, `busy`.
    Wait {
        status: StatusCode,
        code: &'static str,
        message: Cow<'static, str>,
        retry_after: i64,
    },
}

impl From<ApiError> for Refusal {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

impl From<sqlx::Error> for Refusal {
    fn from(error: sqlx::Error) -> Self {
        if is_lock_timeout(&error) {
            tracing::warn!("PIN reset: a lock or a connection was not had in time; answered busy");
            return busy();
        }
        Self::Api(error.into())
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let mut response = match self {
            Self::Api(error) => error.into_response(),
            Self::Wait { status, code, message, retry_after } => {
                let mut response =
                    (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response();
                response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(retry_after.max(1)));
                response
            }
        };
        response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}

fn busy() -> Refusal {
    Refusal::Wait {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "busy",
        message: Cow::Borrowed("Buwiz is busy. Nothing was done; try again in a few seconds."),
        retry_after: BUSY_RETRY_S,
    }
}

fn refused(status: StatusCode, code: &'static str, message: &'static str) -> Refusal {
    Refusal::Api(ApiError::Client { status, code, message })
}

const BAD_START: &str = "Send nothing, {}, or {\"device_name\": \"...\"} of at most 100 characters.";
const BAD_VERIFY: &str = "Send a JSON object with request_id and code.";

fn not_registered() -> Refusal {
    refused(
        StatusCode::CONFLICT,
        "device_not_registered",
        "This device is not added to your Buwiz account. Add it, then ask for a PIN reset code again.",
    )
}

fn code_invalid() -> Refusal {
    refused(StatusCode::BAD_REQUEST, "code_invalid", "That code is wrong. Check it and try again.")
}

fn code_expired() -> Refusal {
    refused(StatusCode::BAD_REQUEST, "code_expired", "This code expired. Ask for a new one.")
}

/// `503 mail_unavailable` at start: the code was not kept.
fn code_not_mailed() -> Refusal {
    refused(
        StatusCode::SERVICE_UNAVAILABLE,
        "mail_unavailable",
        "The code could not be mailed. Nothing was kept; ask for a new code.",
    )
}

/// `503 mail_unavailable` at verify: the reset is verified and logged, but
/// the account was not told, so the app must not change the PIN.
fn notice_not_mailed() -> Refusal {
    refused(
        StatusCode::SERVICE_UNAVAILABLE,
        "mail_unavailable",
        "The code was right, but the email telling your account could not be sent. Your PIN was not changed: \
         ask for a new code.",
    )
}

/// An answer that is not kept.
fn answer(body: serde_json::Value) -> Response {
    let mut response = Json(body).into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// What a request is bound to: a real account's live device, or a practice
/// account's sign-in.
#[derive(Clone, Debug)]
enum Bound {
    Device { id: Uuid, name: String },
    Grant(Uuid),
}

impl Bound {
    /// The key the per-device limits count under.
    fn key(&self) -> Uuid {
        match self {
            Self::Device { id, .. } => *id,
            Self::Grant(grant) => *grant,
        }
    }

    /// The `pin_resets` row's `device_id`.
    fn device_id(&self) -> Option<Uuid> {
        match self {
            Self::Device { id, .. } => Some(*id),
            Self::Grant(_) => None,
        }
    }
}

/// SQL: the `pin_resets` row `r` is bound to `$2` (a device id; NULL for a
/// practice account) and, for a practice account, made with the sign-in
/// `$3`.
const BOUND_SQL: &str = "(CASE WHEN $2::uuid IS NULL THEN r.device_id IS NULL AND r.grant_id = $3
                              ELSE r.device_id = $2 END)";

/// In the caller's transaction: locks the account (released at its end),
/// then finds what a request of this token is bound to. `None`: a real
/// account whose sign-in has no live device. A practice account is one with
/// a working fixed code ([`roles::fixed_code_sql`]), as `GET /me` tells the
/// app.
async fn lock_and_bind(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    state: &AppState,
    bearer: &Bearer,
) -> Result<Option<Bound>, Refusal> {
    let practice: Option<bool> = sqlx::query_scalar(&format!(
        "SELECT {} FROM accounts a WHERE a.id = $1 FOR UPDATE",
        roles::fixed_code_sql("a", "$2")
    ))
    .bind(bearer.account.id)
    .bind(&state.config.admin_emails)
    .fetch_optional(&mut **tx)
    .await?;
    // Deleted since the token was read.
    let practice = practice.ok_or(Refusal::Api(ApiError::InvalidToken))?;
    if practice {
        return Ok(Some(Bound::Grant(bearer.grant_id)));
    }
    let device: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, name FROM devices WHERE grant_id = $1 AND account_id = $2 AND revoked_at IS NULL
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(bearer.grant_id)
    .bind(bearer.account.id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(device.map(|(id, name)| Bound::Device { id, name }))
}

/// The transaction of a start or verify: a connection and every lock are
/// waited for at most a few seconds, then `503 busy`.
async fn begin(state: &AppState) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, Refusal> {
    Ok(begin_waiting(&state.db).await?)
}

#[derive(Deserialize)]
struct StartBody {
    #[serde(default)]
    device_name: Option<String>,
}

/// The `device_name` of a start's body, trimmed (`None` when empty or not
/// sent), or `400 bad_request`.
fn reported_name(body: &[u8]) -> Result<Option<String>, Refusal> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let bad = || refused(StatusCode::BAD_REQUEST, "bad_request", BAD_START);
    let asked: StartBody = serde_json::from_slice(body).map_err(|_| bad())?;
    let Some(name) = asked.device_name else {
        return Ok(None);
    };
    let name = name.trim();
    if name.chars().count() > DEVICE_NAME_CHARS || name.chars().any(char::is_control) {
        return Err(bad());
    }
    Ok((!name.is_empty()).then(|| name.to_owned()))
}

/// `POST /api/pin-reset/start`: mails a PIN reset code to the account's
/// own address and answers `{"request_id", "expires_in"}`. The body is
/// empty, `{}`, or `{"device_name": "..."}` (at most
/// [`DEVICE_NAME_CHARS`]), which only a practice account's notice uses.
///
/// Under the account's lock: the device (`409 device_not_registered`), a
/// code of this device's that still works (`429 code_already_sent`, with
/// `Retry-After` until it expires), the limits ([`STARTS_PER_DEVICE`] and
/// [`STARTS_PER_ACCOUNT`] an hour in memory, [`STARTS_PER_DAY`] a day in
/// the database, and the address's five codes an hour: `429 rate_limited`
/// with `Retry-After`), then the code and the request, committed before the
/// code is mailed. A code that could not be mailed is taken back (`503
/// mail_unavailable`), and its start no longer counts.
pub async fn start(State(state): State<AppState>, bearer: Bearer, body: Bytes) -> Response {
    match start_inner(&state, &bearer, &body).await {
        Ok(response) => response,
        Err(refusal) => refusal.into_response(),
    }
}

/// The starts counted in memory for one start, given back if it comes to
/// nothing.
struct Counted<'a> {
    limits: &'a PinResetLimits,
    device: Uuid,
    account: Uuid,
    at: OffsetDateTime,
    buckets: (Bucket, Bucket),
}

impl Counted<'_> {
    fn release(self) {
        self.limits.starts_by_device.release(&self.device, self.at, self.buckets.0);
        self.limits.starts_by_account.release(&self.account, self.at, self.buckets.1);
    }
}

/// Counts a start for `device` and `account`, or refuses it with how long
/// to wait (counting nothing).
fn count_start(limits: &PinResetLimits, device: Uuid, account: Uuid) -> Result<Counted<'_>, Refusal> {
    let at = OffsetDateTime::now_utc();
    let by_device = limits.starts_by_device.hit(device, at).map_err(ApiError::rate_limited)?;
    let by_account = match limits.starts_by_account.hit(account, at) {
        Ok(bucket) => bucket,
        Err(wait) => {
            limits.starts_by_device.release(&device, at, by_device);
            return Err(ApiError::rate_limited(wait).into());
        }
    };
    Ok(Counted { limits, device, account, at, buckets: (by_device, by_account) })
}

/// How long until `ends`, in whole seconds rounded up.
fn seconds_until(ends: OffsetDateTime) -> i64 {
    let left = ends - OffsetDateTime::now_utc();
    let seconds = left.whole_seconds();
    if left > Duration::seconds(seconds) { seconds + 1 } else { seconds }
}

async fn start_inner(state: &AppState, bearer: &Bearer, body: &[u8]) -> Result<Response, Refusal> {
    bearer.require_app()?;
    let reported = reported_name(body)?;
    let account = &bearer.account;
    let ttl = Duration::minutes(state.config.app_sign_in_minutes);

    let mut tx = begin(state).await?;
    let bound = lock_and_bind(&mut tx, state, bearer).await?.ok_or_else(not_registered)?;
    // Every code of the address is counted and added under its lock.
    accounts::lock_address(&mut tx, &account.email).await?;
    let open: Option<OffsetDateTime> = sqlx::query_scalar(&format!(
        "SELECT max(c.expires_at) FROM pin_resets r JOIN email_codes c ON c.id = r.code_id
         WHERE r.account_id = $1 AND {BOUND_SQL} AND r.used_at IS NULL AND r.expires_at > now()
           AND c.consumed_at IS NULL AND c.expires_at > now() AND c.attempts < $4"
    ))
    .bind(account.id)
    .bind(bound.device_id())
    .bind(bearer.grant_id)
    .bind(CODE_ATTEMPTS)
    .fetch_one(&mut *tx)
    .await?;
    if let Some(ends) = open {
        let seconds = seconds_until(ends).max(1);
        let minutes = (seconds + 59) / 60;
        return Err(Refusal::Wait {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "code_already_sent",
            message: Cow::Owned(format!(
                "A PIN reset code was already sent for this device. Use it, or try again in {minutes} minute{}.",
                if minutes == 1 { "" } else { "s" }
            )),
            retry_after: seconds,
        });
    }
    let counted = count_start(&state.pin_reset_limits, bound.key(), account.id)?;
    let stored = add_request(&mut tx, state, bearer, &bound, reported.as_deref(), ttl).await;
    let (request_id, code_id, code) = match stored {
        Ok(stored) => stored,
        Err(refusal) => {
            counted.release();
            return Err(refusal);
        }
    };
    if let Err(error) = tx.commit().await {
        counted.release();
        return Err(error.into());
    }
    // Only after the commit, and only a random code: a practice account's
    // fixed code is its holder's already.
    if let IssuedCode::Mail(code) = &code
        && let Err(error) =
            state.mailer.send_pin_reset_code(&account.email, code, state.config.app_sign_in_minutes).await
    {
        tracing::error!(error = ?error, %request_id, "a PIN reset code could not be mailed");
        take_back(state, request_id, code_id).await;
        counted.release();
        return Err(code_not_mailed());
    }
    tracing::info!(%request_id, account = %account.id, practice = matches!(bound, Bound::Grant(_)), "PIN reset asked for");
    Ok(answer(json!({ "request_id": request_id, "expires_in": ttl.whole_seconds() })))
}

/// In the start's transaction, under the account's and the address's
/// locks: the day's count, the address's codes an hour, then the code
/// (purpose `pin_reset`) and its request. A refusal writes nothing.
async fn add_request(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    state: &AppState,
    bearer: &Bearer,
    bound: &Bound,
    reported: Option<&str>,
    ttl: Duration,
) -> Result<(Uuid, Uuid, IssuedCode), Refusal> {
    let account = &bearer.account;
    let (today, wait): (i64, Option<f64>) = sqlx::query_as(
        "SELECT count(*), extract(epoch FROM min(created_at) + interval '1 day' - now())::float8
         FROM pin_resets WHERE account_id = $1 AND created_at > now() - interval '1 day'",
    )
    .bind(account.id)
    .fetch_one(&mut **tx)
    .await?;
    if today >= STARTS_PER_DAY {
        let seconds = wait.map_or(HOUR_S, |seconds| seconds.ceil() as i64);
        return Err(ApiError::rate_limited(Duration::seconds(seconds)).into());
    }
    let (hash, code) =
        match accounts::plan_code(tx, &account.email, &state.config.admin_emails, NewAddressBudget::Web).await {
            Ok(Planned::Code(hash, code)) => (hash, code),
            Err(CodeError::TooMany) => return Err(ApiError::rate_limited(Duration::seconds(HOUR_S)).into()),
            // The account exists, so no budget for new addresses applies.
            Ok(Planned::Busy) => {
                return Err(ApiError::Internal(anyhow::anyhow!("an account's address planned as new")).into());
            }
            Err(error) => return Err(ApiError::Internal(anyhow::anyhow!(error)).into()),
        };
    let code_id = accounts::add_code(tx, &account.email, hash, Purpose::PinReset, ttl, false, true).await?;
    let device_name = match bound {
        Bound::Device { name, .. } => name.clone(),
        Bound::Grant(_) => reported.unwrap_or(THIS_DEVICE).to_owned(),
    };
    let request_id: Uuid = sqlx::query_scalar(
        "INSERT INTO pin_resets (account_id, device_id, grant_id, device_name, code_id, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(account.id)
    .bind(bound.device_id())
    .bind(bearer.grant_id)
    .bind(&device_name)
    .bind(code_id)
    .bind(OffsetDateTime::now_utc() + ttl)
    .fetch_one(&mut **tx)
    .await?;
    Ok((request_id, code_id, code))
}

/// After a code that was not mailed: the request and its code are deleted,
/// and with them their place in every count. If that fails the request is
/// left to expire unused (it cannot be verified without the code), and the
/// ids are logged.
async fn take_back(state: &AppState, request_id: Uuid, code_id: Uuid) {
    let deleted = async {
        let mut tx = begin_waiting(&state.db).await?;
        sqlx::query("DELETE FROM pin_resets WHERE id = $1").bind(request_id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM email_codes WHERE id = $1").bind(code_id).execute(&mut *tx).await?;
        tx.commit().await
    }
    .await;
    if let Err(error) = deleted {
        tracing::error!(%error, %request_id, %code_id, "an unmailed PIN reset request could not be taken back");
    }
}

#[derive(Deserialize)]
struct VerifyBody {
    request_id: Uuid,
    code: String,
}

/// A request as stored.
#[derive(sqlx::FromRow)]
struct Stored {
    device_name: String,
    code_id: Option<Uuid>,
    expires_at: OffsetDateTime,
    used_at: Option<OffsetDateTime>,
}

/// `POST /api/pin-reset/verify` `{"request_id", "code"}`: `200
/// {"verified": true, "device", "at"}` once the reset is verified, logged
/// and the account told.
///
/// In this order (the app relies on it): in one transaction, the account is
/// locked and the code taken (purpose `pin_reset`, the request this
/// account's and this device's, open and unexpired, the code with tries
/// left; a wrong try is counted and committed), the request marked used,
/// and one `account_audit` line written (`pin_reset_verified`, source
/// `app`, the account as actor, reason `device <name> (<id>)` or, for a
/// practice account, `grant <id>`); it commits. Then the notice is mailed
/// to the account's own address, waiting at most [`NOTICE_TIMEOUT`]: the
/// `200` comes only once the mail service took it. Otherwise `503
/// mail_unavailable` (the code stays spent, the line stays), staff are
/// alerted, and the app must not change the PIN. Nothing is mailed before
/// the commit.
///
/// Refusals: `400 bad_request`; `400 code_invalid` (a wrong code, an
/// unknown request, another device's, one used, a code of another
/// purpose: one answer for all); `400 code_expired`; `429 too_many_tries`
/// (the code had [`CODE_ATTEMPTS`] wrong tries); `429 rate_limited` past
/// [`VERIFIES_PER_DEVICE`] tries an hour; `409 device_not_registered`;
/// `503 busy` (nothing was taken).
pub async fn verify(State(state): State<AppState>, bearer: Bearer, body: Bytes) -> Response {
    match verify_inner(&state, &bearer, &body).await {
        Ok(response) => response,
        Err(refusal) => refusal.into_response(),
    }
}

async fn verify_inner(state: &AppState, bearer: &Bearer, body: &[u8]) -> Result<Response, Refusal> {
    bearer.require_app()?;
    let asked: VerifyBody =
        serde_json::from_slice(body).map_err(|_| refused(StatusCode::BAD_REQUEST, "bad_request", BAD_VERIFY))?;
    let account = &bearer.account;

    let mut tx = begin(state).await?;
    let bound = lock_and_bind(&mut tx, state, bearer).await?.ok_or_else(not_registered)?;
    state.pin_reset_limits.verifies.check(bound.key(), OffsetDateTime::now_utc()).map_err(ApiError::rate_limited)?;
    let stored: Option<Stored> = sqlx::query_as(&format!(
        "SELECT device_name, code_id, expires_at, used_at FROM pin_resets r
         WHERE r.id = $1 AND r.account_id = $4 AND {BOUND_SQL} FOR UPDATE"
    ))
    .bind(asked.request_id)
    .bind(bound.device_id())
    .bind(bearer.grant_id)
    .bind(account.id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(stored) = stored.filter(|stored| stored.used_at.is_none()) else {
        return Err(code_invalid());
    };
    if stored.expires_at <= OffsetDateTime::now_utc() {
        return Err(code_expired());
    }
    // Only the request's own code, and only of this purpose.
    let tried = match stored.code_id {
        Some(code_id) => {
            accounts::try_code_row(&mut tx, code_id, &account.email, Purpose::PinReset, &asked.code).await?
        }
        None => Tried::NoCode,
    };
    match tried {
        Tried::Taken => {}
        Tried::Wrong { attempts_left } => {
            // The try stays counted.
            tx.commit().await?;
            if attempts_left == 0 {
                return Err(refused(
                    StatusCode::TOO_MANY_REQUESTS,
                    "too_many_tries",
                    "That code was tried too many times. Ask for a new one.",
                ));
            }
            return Err(code_invalid());
        }
        // The code ended a moment before its request.
        Tried::NoCode => return Err(code_expired()),
    }
    let at: OffsetDateTime =
        sqlx::query_scalar("UPDATE pin_resets SET used_at = now() WHERE id = $1 RETURNING used_at")
            .bind(asked.request_id)
            .fetch_one(&mut *tx)
            .await?;
    let reason = match &bound {
        Bound::Device { id, name } => format!("device {name} ({id})"),
        Bound::Grant(grant) => format!("grant {grant}"),
    };
    let line = Line {
        account_id: account.id,
        action: "pin_reset_verified",
        old_value: None,
        new_value: None,
        source: Source::App,
        reason: Some(&reason),
    };
    admin::audit(&mut tx, account, line).await?;
    tx.commit().await?;

    // Only now, after the commit: the account is told, or no 200.
    let device = stored.device_name;
    let mailed = tokio::time::timeout(NOTICE_TIMEOUT, state.mailer.send_pin_reset_notice(&account.email, &device, at))
        .await
        .map_err(|_| anyhow::anyhow!("the mail service did not answer in {NOTICE_TIMEOUT:?}"))
        .and_then(|sent| sent);
    if let Err(error) = mailed {
        tracing::error!(error = ?error, request_id = %asked.request_id, account = %account.id,
            "a verified PIN reset's notice was not mailed; answered mail_unavailable");
        tokio::spawn(alert_staff(state.clone(), account.id, asked.request_id));
        return Err(notice_not_mailed());
    }
    tracing::info!(request_id = %asked.request_id, account = %account.id, "PIN reset verified and the account told");
    Ok(answer(json!({ "verified": true, "device": device, "at": at.unix_timestamp() })))
}

/// Mails staff that a verified reset's notice was not mailed: ids only, no
/// address. A failure is logged (it goes over the same mail service).
async fn alert_staff(state: AppState, account_id: Uuid, request_id: Uuid) {
    let text = format!(
        "A PIN reset code was verified in the eBIRForms app, but the mail service did not take the notice to the \
         account. The app was told not to change the PIN, and to ask for a new code. The audit line is written.\n\n\
         Account {account_id}, request {request_id}. Look at the mail service.\n\n\
         See docs/accounts.md, \"Resetting a forgotten app PIN\"."
    );
    mail_staff(&state, "PIN reset: the notice to the account was not mailed", &text).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_body_is_empty_an_empty_object_or_a_short_name() {
        for empty in ["", " \n", "{}", "{\"device_name\": null}", "{\"device_name\": \"  \"}", "{\"other\": 1}"] {
            assert_eq!(reported_name(empty.as_bytes()).unwrap(), None, "{empty}");
        }
        assert_eq!(reported_name(br#"{"device_name": " Ana's iPhone "}"#).unwrap().as_deref(), Some("Ana's iPhone"));
        let longest = format!(r#"{{"device_name": "{}"}}"#, "é".repeat(DEVICE_NAME_CHARS));
        assert!(reported_name(longest.as_bytes()).unwrap().is_some());
        let longer = format!(r#"{{"device_name": "{}"}}"#, "é".repeat(DEVICE_NAME_CHARS + 1));
        for bad in [longer.as_str(), "7", "nope", r#"{"device_name": 7}"#, "{\"device_name\": \"a\\u0007b\"}"] {
            assert!(reported_name(bad.as_bytes()).is_err(), "{bad}");
        }
    }
}
