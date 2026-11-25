//! In-app email sign-in for the eBIRForms apps (docs/accounts.md, "In-app
//! sign-in"): the app asks for a code by email, the person types it in the
//! app, and the app gets the same tokens the device flow gives.
//!
//! - `POST /auth/email/start` makes a request (`app_sign_ins`) and mails a
//!   code;
//! - `POST /auth/email/resend` mails a new one for the same request;
//! - `POST /auth/email/verify` checks it and answers the tokens.
//!
//! All three take JSON; the web's form posts to the first two paths stay
//! the web's ([`crate::account::web`] tells them apart by content type).
//! Each request has its own code ([`accounts::issue_code_as`], purpose
//! `app_sign_in`), which works with that request only, under the web's
//! limits and counters. A request belongs to the device that made it: the
//! app makes a secret, sends only its SHA-256 at the start, and must show
//! the secret itself to resend or verify. A code lasts
//! `APP_SIGN_IN_MINUTES` (5), and no new one is mailed for the request,
//! or for the address to the same install, until it has expired.

use std::borrow::Cow;

use axum::Json;
use axum::extract::Request;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::account::accounts::{self, CODE_ATTEMPTS, CodeError, IssuedCode, Purpose, Tried};
use crate::account::guard::{Counted, Refused};
use crate::account::mail::AppCodeWords;
use crate::account::oauth::{self, SCOPE_APP};
use crate::account::roles;
use crate::base::crypto;
use crate::base::state::AppState;

const CODE_LENGTH: usize = 6;
/// How long after its start a request may still be resent.
const RESEND_WITHIN: time::Duration = time::Duration::hours(1);
/// The largest body these endpoints read.
const BODY_LIMIT: usize = 16 * 1024;

/// The testing cap: requests without an app check (a Turnstile token),
/// across every address, in an hour and in a day. Above the web's own
/// limits, which every request passes either way.
const UNCHECKED_PER_HOUR: i64 = 20;
const UNCHECKED_PER_DAY: i64 = 50;
/// Staff are mailed once a day when this many unchecked requests came in
/// a day.
const UNCHECKED_ALERT_AT: i64 = 25;
/// The advisory lock the cap is counted and added under, so two requests
/// at once cannot both take the last place.
const UNCHECKED_LOCK: i64 = 0x6275_7769_7a02;
/// How long a refusal by a per-hour limit asks to wait, when the wait is
/// not known: the hour.
const HOUR_S: i64 = 3600;

/// Reserved: the refusal of a request without a Turnstile token from a
/// platform that requires one (`403`). Not used yet: every platform may
/// still go without, under the testing cap. A platform's 1.0.0 requires
/// the token (docs/accounts.md, "The app check and the testing cap").
pub const TURNSTILE_REQUIRED: &str = "turnstile_required";

/// Whether a request is for these endpoints rather than the web's forms.
pub fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"))
}

/// A refusal: `{"error": code, "message": sentence}`, with `retry_after`
/// (also as `Retry-After`) or `attempts_left` where they apply.
#[derive(Debug)]
pub struct Refusal {
    status: StatusCode,
    code: &'static str,
    message: Cow<'static, str>,
    retry_after: Option<i64>,
    attempts_left: Option<i32>,
}

impl Refusal {
    const fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self { status, code, message: Cow::Borrowed(message), retry_after: None, attempts_left: None }
    }

    const fn bad(code: &'static str, message: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    fn too_many(retry_after: i64) -> Self {
        Self {
            retry_after: Some(retry_after.max(1)),
            ..Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_requests",
                "Too many codes were asked for. Wait, then try again.",
            )
        }
    }

    fn internal(error: impl Into<anyhow::Error>) -> Self {
        tracing::error!(error = ?error.into(), "in-app sign-in failed");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong. Try again.")
    }
}

const INVALID_REQUEST: Refusal =
    Refusal::bad("invalid_request", "This sign-in request is not valid any more. Start again.");

impl From<sqlx::Error> for Refusal {
    fn from(error: sqlx::Error) -> Self {
        Self::internal(error)
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let mut body = json!({ "error": self.code, "message": self.message });
        if let Some(retry_after) = self.retry_after {
            body["retry_after"] = json!(retry_after);
        }
        if let Some(attempts_left) = self.attempts_left {
            body["attempts_left"] = json!(attempts_left);
        }
        let mut response = (self.status, Json(body)).into_response();
        let headers = response.headers_mut();
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        if let Some(retry_after) = self.retry_after {
            headers.insert(header::RETRY_AFTER, HeaderValue::from(retry_after));
        }
        response
    }
}

/// A JSON answer that is not kept.
fn answer(body: impl serde::Serialize) -> Response {
    let mut response = Json(body).into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// The device an app says it runs on, for the mail's words only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Ios,
    Ipados,
    Android,
    Macos,
    Windows,
    Linux,
}

impl Platform {
    const ALL: [Self; 6] = [Self::Ios, Self::Ipados, Self::Android, Self::Macos, Self::Windows, Self::Linux];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ios => "ios",
            Self::Ipados => "ipados",
            Self::Android => "android",
            Self::Macos => "macos",
            Self::Windows => "windows",
            Self::Linux => "linux",
        }
    }

    /// A known value, or `None` for anything else (never an error).
    pub fn parse(value: Option<&Value>) -> Option<Self> {
        let value = value?.as_str()?;
        Self::ALL.into_iter().find(|platform| platform.as_str() == value)
    }

    /// What the mail calls the app on this device: fixed words only.
    pub fn device(platform: Option<Self>) -> &'static str {
        match platform {
            Some(Self::Ios) => "eBIRForms on iPhone",
            Some(Self::Ipados) => "eBIRForms on iPad",
            Some(Self::Android) => "eBIRForms on Android",
            Some(Self::Macos) => "eBIRForms on Mac",
            Some(Self::Windows) => "eBIRForms on Windows",
            Some(Self::Linux) => "eBIRForms on Linux",
            None => "eBIRForms",
        }
    }
}

/// Reads the JSON body, or refuses it as `invalid_request`.
async fn body<T: DeserializeOwned>(request: Request) -> Result<T, Refusal> {
    let bytes = axum::body::to_bytes(request.into_body(), BODY_LIMIT)
        .await
        .map_err(|_| Refusal::bad("invalid_request", "The request body is too large."))?;
    serde_json::from_slice(&bytes).map_err(|_| Refusal::bad("invalid_request", "The request is not the JSON expected."))
}

/// While the server is stopping: `503 restarting`, before anything starts.
fn refuse_while_stopping(state: &AppState) -> Result<(), Refusal> {
    if state.stopping.is_on() {
        return Err(Refusal {
            retry_after: Some(i64::from(crate::account::web::STOPPING_RETRY_S)),
            ..Refusal::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "restarting",
                "Buwiz is restarting. Nothing was sent; try again in a moment.",
            )
        });
    }
    Ok(())
}

/// SHA-256 of `secret`, if it is a secret as the app makes one: 32
/// random bytes, base64url without padding.
fn secret_hash(secret: &str) -> Option<Vec<u8>> {
    let bytes = URL_SAFE_NO_PAD.decode(secret).ok()?;
    (bytes.len() == 32).then(|| Sha256::digest(secret.as_bytes()).to_vec())
}

/// Compares two hashes in constant time.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

fn whole_seconds(duration: time::Duration) -> i64 {
    let seconds = duration.whole_seconds();
    if duration > time::Duration::seconds(seconds) { seconds + 1 } else { seconds }
}

/// How long an in-app code works, and so how long before a new one.
fn ttl(state: &AppState) -> time::Duration {
    time::Duration::minutes(state.config.app_sign_in_minutes)
}

/// An `install_id` as the app makes one: 16 to 64 of `[A-Za-z0-9_-]`. An
/// identifier, not a secret.
fn install_id(raw: Option<&str>) -> Result<Option<&str>, Refusal> {
    match raw {
        None => Ok(None),
        Some(id)
            if (16..=64).contains(&id.len())
                && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_') =>
        {
            Ok(Some(id))
        }
        Some(_) => {
            Err(Refusal::bad("bad_install_id", "install_id must be 16 to 64 characters of A-Z, a-z, 0-9, - and _."))
        }
    }
}

#[derive(Deserialize)]
pub struct Start {
    client_id: String,
    email: String,
    #[serde(default)]
    intent: Option<Value>,
    secret_hash: String,
    #[serde(default)]
    platform: Option<Value>,
    #[serde(default)]
    install_id: Option<String>,
    #[serde(default)]
    turnstile: Option<String>,
    /// Reserved for a platform attestation; not read yet.
    #[serde(default)]
    #[allow(dead_code)]
    attestation: Option<Value>,
}

/// `POST /auth/email/start` (JSON).
pub async fn start(state: AppState, request: Request) -> Response {
    let headers = request.headers().clone();
    match start_inner(&state, &headers, request).await {
        Ok(response) => response,
        Err(refusal) => refusal.into_response(),
    }
}

async fn start_inner(state: &AppState, headers: &HeaderMap, request: Request) -> Result<Response, Refusal> {
    refuse_while_stopping(state)?;
    let asked: Start = body(request).await?;
    if oauth::client_scope_of(&asked.client_id) != Some(SCOPE_APP) {
        return Err(Refusal::bad("unknown_client", "This app is not one that can sign in here."));
    }
    let email = accounts::normalize_email(&asked.email)
        .map_err(|_| Refusal::bad("invalid_email", "That is not an email address."))?;
    let stored_hash = URL_SAFE_NO_PAD
        .decode(&asked.secret_hash)
        .ok()
        .filter(|hash| hash.len() == 32)
        .ok_or(Refusal::bad("invalid_request", "secret_hash must be the base64url SHA-256 of the secret."))?;
    let install = install_id(asked.install_id.as_deref())?;
    let create = asked.intent.as_ref().and_then(Value::as_str) == Some("create");
    let platform = Platform::parse(asked.platform.as_ref());
    let words = AppCodeWords { device: Platform::device(platform), create, minutes: state.config.app_sign_in_minutes };

    let checked = app_check(state, headers, asked.turnstile.as_deref()).await?;
    let fixed = accounts::has_fixed_code(&state.db, &email, &state.config.admin_emails).await?;
    // The request is made first, under a lock per address and install, so
    // two starts at once from one install cannot both mail a code.
    let mut tx = state.db.begin().await?;
    if let Some(install) = install {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('app sign-in ' || $1 || ' ' || $2, 0))")
            .bind(&email)
            .bind(install)
            .execute(&mut *tx)
            .await?;
        let live = "FROM app_sign_ins
                    WHERE email = $1 AND install_id = $2 AND used_at IS NULL AND expires_at > now()";
        if fixed {
            // Nothing is mailed to a test account with a fixed code, so it
            // is never held: its earlier request from this install ends.
            sqlx::query(&format!("UPDATE app_sign_ins SET used_at = now() WHERE id IN (SELECT id {live})"))
                .bind(&email)
                .bind(install)
                .execute(&mut *tx)
                .await?;
        } else {
            let ends: Option<OffsetDateTime> = sqlx::query_scalar(&format!("SELECT max(expires_at) {live}"))
                .bind(&email)
                .bind(install)
                .fetch_one(&mut *tx)
                .await?;
            if let Some(ends) = ends {
                return Err(already_sent(whole_seconds(ends - OffsetDateTime::now_utc())));
            }
        }
    }
    let request_id: Uuid = sqlx::query_scalar(
        "INSERT INTO app_sign_ins (email, client_id, secret_hash, intent, platform, install_id, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(&email)
    .bind(&asked.client_id)
    .bind(&stored_hash)
    .bind(if create { "create" } else { "sign_in" })
    .bind(platform.map(Platform::as_str))
    .bind(install)
    .bind(OffsetDateTime::now_utc() + ttl(state))
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    let issued = match issue(state, headers, checked, &email).await {
        Ok(issued) => issued,
        Err(refusal) => {
            // Nothing was mailed: the request holds nothing back.
            end(state, request_id).await?;
            return Err(refusal);
        }
    };
    if let Err(refusal) = mail(state, request_id, &email, issued, words).await {
        end(state, request_id).await?;
        return Err(refusal);
    }
    Ok(sent(state, request_id))
}

/// `429 code_already_sent`: a code mailed to this install for this address
/// still works for `seconds`.
fn already_sent(seconds: i64) -> Refusal {
    let minutes = (seconds.max(1) + 59) / 60;
    Refusal {
        status: StatusCode::TOO_MANY_REQUESTS,
        code: "code_already_sent",
        message: Cow::Owned(format!(
            "A sign-in code was already sent to this address. Use it, or try again in {} minute{}.",
            minutes,
            if minutes == 1 { "" } else { "s" }
        )),
        retry_after: Some(seconds.max(1)),
        attempts_left: None,
    }
}

/// Ends a request that can no longer be used.
async fn end(state: &AppState, request_id: Uuid) -> sqlx::Result<()> {
    sqlx::query("UPDATE app_sign_ins SET used_at = now() WHERE id = $1").bind(request_id).execute(&state.db).await?;
    Ok(())
}

/// What start and resend answer, whether or not the address has an
/// account, and whether or not a code was mailed.
fn sent(state: &AppState, request_id: Uuid) -> Response {
    let seconds = ttl(state).whole_seconds();
    answer(json!({
        "request_id": request_id,
        "expires_in": seconds,
        "resend_after": seconds,
        "code_length": CODE_LENGTH,
    }))
}

/// The app check: whether a Turnstile token came and passed. A token
/// that fails is refused; none (or Turnstile off) is `false`, and the
/// request then counts towards the testing cap.
async fn app_check(state: &AppState, headers: &HeaderMap, turnstile: Option<&str>) -> Result<bool, Refusal> {
    match state.sign_in_guard.app_check(headers, turnstile).await {
        Some(true) => Ok(true),
        Some(false) => {
            Err(Refusal::new(StatusCode::FORBIDDEN, "turnstile_failed", "The app check did not pass. Try again."))
        }
        None => Ok(false),
    }
}

/// A code [`issue`] made: its row, and what to mail.
type Issued = Option<(Uuid, IssuedCode)>;

/// The checks after the app check, then the code: the testing cap
/// (unless `checked`), the network's hour, and the web's own limits in
/// [`accounts::issue_code_as`]. `None` when nothing is to be mailed and the
/// answer must not say so (the limit on addresses without an account).
async fn issue(state: &AppState, headers: &HeaderMap, checked: bool, email: &str) -> Result<Issued, Refusal> {
    let counted = match state.sign_in_guard.count_network(headers) {
        Ok(counted) => counted,
        Err(Refused::NetworkTooMany | Refused::NotHuman) => return Err(Refusal::too_many(HOUR_S)),
    };
    if !checked && let Err(refusal) = spend_unchecked(state).await {
        return Err(forgiven(state, &counted, refusal));
    }
    let admins = &state.config.admin_emails;
    match accounts::issue_code_as(&state.db, email, admins, Purpose::AppSignIn, ttl(state)).await {
        Ok(issued) => Ok(Some(issued)),
        Err(CodeError::TooMany) => Err(forgiven(state, &counted, Refusal::too_many(HOUR_S))),
        // As on the web: answered as if mailed, and the network not held
        // to it.
        Err(CodeError::Busy) => {
            state.sign_in_guard.forgive(&counted);
            Ok(None)
        }
        Err(error) => Err(forgiven(state, &counted, Refusal::internal(anyhow::anyhow!(error)))),
    }
}

fn forgiven(state: &AppState, counted: &Counted, refusal: Refusal) -> Refusal {
    state.sign_in_guard.forgive(counted);
    refusal
}

/// Ties what [`issue`] made to the request and mails it, with a link
/// ([`take_link`]). A fixed code (a
/// test account) is not mailed: its holder has it.
async fn mail(
    state: &AppState,
    request_id: Uuid,
    email: &str,
    issued: Issued,
    words: AppCodeWords,
) -> Result<(), Refusal> {
    // A one-time link to the request goes with a mailed code; only its
    // hash is kept, and a new code replaces it.
    let token = matches!(issued, Some((_, IssuedCode::Mail(_)))).then(|| crypto::new_token(""));
    sqlx::query("UPDATE app_sign_ins SET code_id = $2, link_token_hash = $3 WHERE id = $1")
        .bind(request_id)
        .bind(issued.as_ref().map(|(code_id, _)| *code_id))
        .bind(token.as_deref().map(crypto::hash))
        .execute(&state.db)
        .await?;
    match issued {
        Some((_, IssuedCode::Mail(code))) => {
            let link =
                token.map(|token| format!("{}/app-sign-in#r={request_id}&t={token}", state.config.account_origin));
            state.mailer.send_app_sign_in_code(email, &code, link.as_deref(), words).await.map_err(|error| {
                tracing::error!(error = ?error, "an app's sign-in code could not be mailed");
                Refusal::new(StatusCode::BAD_GATEWAY, "mail_failed", "The code could not be mailed. Try again.")
            })
        }
        Some((_, IssuedCode::Fixed)) => {
            tracing::info!("an app asked a code for a test account with a fixed code: nothing is mailed");
            Ok(())
        }
        None => Ok(()),
    }
}

/// Counts a request without an app check against the testing cap, or
/// refuses it with how long to wait. Staff are mailed once a Manila day
/// when the day's count reaches [`UNCHECKED_ALERT_AT`].
async fn spend_unchecked(state: &AppState) -> Result<(), Refusal> {
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(UNCHECKED_LOCK).execute(&mut *tx).await?;
    let (hour, day, hour_wait, day_wait): (i64, i64, Option<f64>, Option<f64>) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE created_at > now() - interval '1 hour'), count(*),
                extract(epoch FROM min(created_at) FILTER (WHERE created_at > now() - interval '1 hour')
                                  + interval '1 hour' - now())::float8,
                extract(epoch FROM min(created_at) + interval '1 day' - now())::float8
         FROM app_sign_in_unchecked WHERE created_at > now() - interval '1 day'",
    )
    .fetch_one(&mut *tx)
    .await?;
    let wait = |seconds: Option<f64>| seconds.map_or(HOUR_S, |seconds| seconds.ceil() as i64);
    if day >= UNCHECKED_PER_DAY {
        tracing::warn!(day, "in-app sign-in: the day's requests without an app check are at the cap");
        return Err(Refusal::too_many(wait(day_wait)));
    }
    if hour >= UNCHECKED_PER_HOUR {
        tracing::warn!(hour, "in-app sign-in: the hour's requests without an app check are at the cap");
        return Err(Refusal::too_many(wait(hour_wait)));
    }
    sqlx::query("INSERT INTO app_sign_in_unchecked DEFAULT VALUES").execute(&mut *tx).await?;
    let first_alert = day + 1 >= UNCHECKED_ALERT_AT
        && sqlx::query(
            "INSERT INTO app_sign_in_alerts (day) VALUES ((now() AT TIME ZONE 'Asia/Manila')::date)
             ON CONFLICT DO NOTHING",
        )
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
    tx.commit().await?;
    if first_alert {
        alert_staff(state, day + 1).await;
    }
    Ok(())
}

/// Mails Admins and Support (as BIR uptime's alerts do) that the testing
/// cap is filling. A failure is logged; it is not tried again that day.
async fn alert_staff(state: &AppState, count: i64) {
    let to = match roles::staff_emails(&state.db, &state.config.admin_emails).await {
        Ok(to) => to,
        Err(error) => {
            tracing::error!(%error, "in-app sign-in: staff to alert could not be read");
            return;
        }
    };
    let subject = "In-app sign-in: many requests without an app check today";
    let text = format!(
        "{count} in-app sign-in requests came without an app check (Turnstile) in the last day.\n\n\
         Requests without one are capped at {UNCHECKED_PER_HOUR} an hour and {UNCHECKED_PER_DAY} a day \
         across every address while the token is optional. Past the cap the apps are told to wait. \
         This is mailed at most once a day.\n\n\
         See docs/accounts.md, \"In-app sign-in\"."
    );
    for address in &to {
        if let Err(error) = state.mailer.send_notice(address, subject, &text).await {
            tracing::error!(error = ?error, "in-app sign-in: a staff alert could not be sent to one address");
        }
    }
}

/// A request as stored.
#[derive(sqlx::FromRow)]
struct Stored {
    email: String,
    client_id: String,
    secret_hash: Vec<u8>,
    intent: String,
    platform: Option<String>,
    code_id: Option<Uuid>,
    link_token_hash: Option<Vec<u8>>,
    attempts: i32,
    created_at: OffsetDateTime,
    expires_at: OffsetDateTime,
    used_at: Option<OffsetDateTime>,
}

impl Stored {
    /// Whether `secret` is the one this request was started with.
    fn belongs_to(&self, secret: &str) -> bool {
        secret_hash(secret).is_some_and(|hash| same(&hash, &self.secret_hash))
    }

    fn words(&self, minutes: i64) -> AppCodeWords {
        let platform = self.platform.as_ref().map(|platform| Value::String(platform.clone()));
        AppCodeWords {
            device: Platform::device(Platform::parse(platform.as_ref())),
            create: self.intent == "create",
            minutes,
        }
    }
}

const STORED: &str =
    "SELECT email, client_id, secret_hash, intent, platform, code_id, link_token_hash, attempts, created_at,
                             expires_at, used_at
                      FROM app_sign_ins WHERE id = $1";

#[derive(Deserialize)]
pub struct Resend {
    request_id: Uuid,
    secret: String,
    #[serde(default)]
    turnstile: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    attestation: Option<Value>,
}

/// `POST /auth/email/resend`.
pub async fn resend(state: AppState, request: Request) -> Response {
    let headers = request.headers().clone();
    match resend_inner(&state, &headers, request).await {
        Ok(response) => response,
        Err(refusal) => refusal.into_response(),
    }
}

async fn resend_inner(state: &AppState, headers: &HeaderMap, request: Request) -> Result<Response, Refusal> {
    refuse_while_stopping(state)?;
    let asked: Resend = body(request).await?;
    let stored: Option<Stored> = sqlx::query_as(STORED).bind(asked.request_id).fetch_optional(&state.db).await?;
    let now = OffsetDateTime::now_utc();
    let Some(stored) = stored.filter(|stored| {
        stored.used_at.is_none() && stored.created_at + RESEND_WITHIN > now && stored.belongs_to(&asked.secret)
    }) else {
        return Err(INVALID_REQUEST);
    };
    // No new code while the request's own still works.
    if stored.expires_at > now {
        return Err(Refusal::too_many(whole_seconds(stored.expires_at - now)));
    }
    // The app check before the request is taken, so a token that fails
    // costs nothing; then the request, so two resends at once mail one
    // code.
    let checked = app_check(state, headers, asked.turnstile.as_deref()).await?;
    let claimed = sqlx::query(
        "UPDATE app_sign_ins SET expires_at = $2
         WHERE id = $1 AND used_at IS NULL AND expires_at <= now()",
    )
    .bind(asked.request_id)
    .bind(now + ttl(state))
    .execute(&state.db)
    .await?;
    if claimed.rows_affected() == 0 {
        return Err(Refusal::too_many(ttl(state).whole_seconds()));
    }
    let issued = match issue(state, headers, checked, &stored.email).await {
        Ok(issued) => issued,
        Err(refusal) => {
            // Nothing was mailed: the request may be resent again.
            sqlx::query("UPDATE app_sign_ins SET expires_at = now() WHERE id = $1")
                .bind(asked.request_id)
                .execute(&state.db)
                .await?;
            return Err(refusal);
        }
    };
    let words = stored.words(state.config.app_sign_in_minutes);
    if let Err(refusal) = mail(state, asked.request_id, &stored.email, issued, words).await {
        end(state, asked.request_id).await?;
        return Err(refusal);
    }
    // A new code: the request's tries start again with it, and its
    // time from now.
    sqlx::query("UPDATE app_sign_ins SET expires_at = $2, attempts = 0 WHERE id = $1")
        .bind(asked.request_id)
        .bind(OffsetDateTime::now_utc() + ttl(state))
        .execute(&state.db)
        .await?;
    Ok(sent(state, asked.request_id))
}

#[derive(Deserialize)]
pub struct Verify {
    request_id: Uuid,
    secret: String,
    /// The code typed in the app, or ...
    #[serde(default)]
    code: Option<String>,
    /// ... the token of the link in the mail, which the app got from
    /// `ebirforms://signin?r=…&t=…`.
    #[serde(default)]
    link_token: Option<String>,
}

/// The link's token, checked against the request: it uses up the
/// request's code as the code itself would. A token that is not the
/// request's newest (a resend replaced it) is `link_used`.
async fn take_link(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    stored: &Stored,
    token: &str,
) -> Result<Tried, Refusal> {
    let matches = stored.link_token_hash.as_deref().is_some_and(|hash| same(&crypto::hash(token), hash));
    let (true, Some(code_id)) = (matches, stored.code_id) else {
        return Err(Refusal::bad("link_used", "This link was used or replaced by a newer code."));
    };
    match accounts::spend_code_row(tx, code_id).await? {
        Tried::NoCode => Err(Refusal::bad("link_used", "This link was used or replaced by a newer code.")),
        tried => Ok(tried),
    }
}

/// `POST /auth/email/verify`: the tokens, as the device flow gives them.
pub async fn verify(state: AppState, request: Request) -> Response {
    match verify_inner(&state, request).await {
        Ok(response) => response,
        Err(refusal) => refusal.into_response(),
    }
}

async fn verify_inner(state: &AppState, request: Request) -> Result<Response, Refusal> {
    refuse_while_stopping(state)?;
    let asked: Verify = body(request).await?;
    let mut tx = state.db.begin().await?;
    // The account before the request, in the order a deletion takes them
    // (it locks the account, then deletes the requests): read whose it is
    // without a lock, lock that account if there is one, then the row.
    let email: Option<String> = sqlx::query_scalar("SELECT email FROM app_sign_ins WHERE id = $1")
        .bind(asked.request_id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(email) = email else {
        return Err(INVALID_REQUEST);
    };
    sqlx::query("SELECT 1 FROM accounts WHERE email = $1 FOR SHARE").bind(&email).execute(&mut *tx).await?;
    let stored: Option<Stored> =
        sqlx::query_as(&format!("{STORED} FOR UPDATE")).bind(asked.request_id).fetch_optional(&mut *tx).await?;
    let Some(stored) = stored.filter(|stored| stored.used_at.is_none() && stored.email == email) else {
        return Err(INVALID_REQUEST);
    };
    if !stored.belongs_to(&asked.secret) {
        return Err(INVALID_REQUEST);
    }
    if stored.expires_at <= OffsetDateTime::now_utc() {
        return Err(Refusal::bad("code_expired", "This code expired. Ask for a new one."));
    }
    // Only the request's own code: the same digits from another request,
    // or a code from the web, are wrong here.
    let tried = match (&asked.code, &asked.link_token, stored.code_id) {
        (Some(code), None, Some(code_id)) => accounts::try_code_row(&mut tx, code_id, &email, code).await?,
        (Some(_), None, None) => Tried::NoCode,
        (None, Some(token), _) => take_link(&mut tx, &stored, token).await?,
        _ => return Err(Refusal::bad("invalid_request", "Send either the code or the link's token.")),
    };
    let attempts_left = match tried {
        Tried::Taken => None,
        Tried::Wrong { attempts_left } => Some(attempts_left),
        // No code is open (none was mailed, the limit on addresses without
        // an account): the tries are counted on the request instead, so
        // the answer is the one a wrong code gets.
        Tried::NoCode => {
            sqlx::query("UPDATE app_sign_ins SET attempts = attempts + 1 WHERE id = $1")
                .bind(asked.request_id)
                .execute(&mut *tx)
                .await?;
            Some((CODE_ATTEMPTS - stored.attempts - 1).max(0))
        }
    };
    if let Some(attempts_left) = attempts_left {
        tx.commit().await?;
        if attempts_left == 0 {
            return Err(Refusal::bad("too_many_attempts", "That code was tried too many times. Ask for a new one."));
        }
        return Err(Refusal {
            attempts_left: Some(attempts_left),
            ..Refusal::bad("invalid_code", "That code is wrong. Check it and try again.")
        });
    }
    let account = accounts::account_for(&mut tx, &email).await?;
    sqlx::query("UPDATE app_sign_ins SET used_at = now() WHERE id = $1")
        .bind(asked.request_id)
        .execute(&mut *tx)
        .await?;
    if oauth::banned_now(&mut tx, account.id).await? {
        tx.commit().await?;
        return Err(Refusal::new(StatusCode::FORBIDDEN, "account_banned", oauth::BANNED_MESSAGE));
    }
    let tokens = oauth::issue_grant(&mut tx, account.id, &stored.client_id, SCOPE_APP).await?;
    tx.commit().await?;
    Ok(answer(tokens))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_platforms_name_a_device() {
        for (value, device) in [
            ("ios", "eBIRForms on iPhone"),
            ("ipados", "eBIRForms on iPad"),
            ("android", "eBIRForms on Android"),
            ("macos", "eBIRForms on Mac"),
            ("windows", "eBIRForms on Windows"),
            ("linux", "eBIRForms on Linux"),
        ] {
            assert_eq!(Platform::device(Platform::parse(Some(&json!(value)))), device);
        }
        for odd in [json!("iOS"), json!("<b>Windows</b>"), json!(7), json!(null)] {
            assert_eq!(Platform::device(Platform::parse(Some(&odd))), "eBIRForms");
        }
        assert_eq!(Platform::device(Platform::parse(None)), "eBIRForms");
    }

    #[test]
    fn install_ids_are_16_to_64_url_safe_characters() {
        assert_eq!(install_id(None).unwrap(), None);
        for good in ["abcdefghijklmnop", "A1_-A1_-A1_-A1_-", &"x".repeat(64)] {
            assert_eq!(install_id(Some(good)).unwrap(), Some(good));
        }
        for bad in ["short", &"x".repeat(65), "abcdefghijklmnop!", "abcdefgh ijklmnop"] {
            assert!(install_id(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_secret_is_32_bytes_in_base64url() {
        let secret = URL_SAFE_NO_PAD.encode([7u8; 32]);
        assert_eq!(secret_hash(&secret).unwrap(), Sha256::digest(secret.as_bytes()).to_vec());
        assert!(secret_hash(&URL_SAFE_NO_PAD.encode([7u8; 31])).is_none());
        assert!(secret_hash("not base64!").is_none());
        assert!(same(&[1, 2], &[1, 2]) && !same(&[1, 2], &[1, 3]) && !same(&[1], &[1, 2]));
    }
}
