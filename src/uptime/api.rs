//! The read routes (docs/bir-uptime.md §4) and the `buwiz-bir` header on
//! responses apps already get (§3). They exist from `BIR_UPTIME=public`:
//! below it they are not routed at all, so every one answers the site's own
//! 404 and nothing else changes (`crate::account::web::router`).
//!
//! Everything public here is made from the sign as it stood at the last
//! 5-minute mark ([`super::live::Live::public`]), so it can be up to 5
//! minutes behind and nobody polling it can time a single try:
//!
//! - `GET /api/bir/status`: the public document, the same bytes for
//!   everyone, with an `ETag` (a hash of the bytes) and `Cache-Control:
//!   public, max-age=15, stale-while-revalidate=30`. 120 a minute per IP
//!   address (an IPv6 address by its /64), held in memory only.
//! - `GET /api/bir/history`: the day lines and the last 24 hours, as at the
//!   mark, from memory ([`super::history::Cache`]), public, cacheable 5
//!   minutes, limited per IP with the status.
//! - The `buwiz-bir` header on responses that need no sign-in (the trust
//!   list, the sealing key).
//!
//! Signed-in reads carry the live sign:
//!
//! - `GET /api/bir/status/mine`: a Bearer token of any scope, or the website
//!   session (a banned account `403`). The live sign with public times; for
//!   staff (Admin or Support) times to the minute, `last_news_at`, and
//!   `detail` (the last 10's tally and today's counts). 60 a minute per
//!   account. `no-store`, no `ETag`, never `304`.
//! - The `buwiz-bir` header on tickets, submissions and tax profiles.

use axum::Json;
use axum::extract::{FromRequestParts, RawQuery, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::request::Parts;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use serde::Serialize;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::history::{self, RangeError};
use super::keys::{client_ip, manila_day, manila_midnight};
use super::public::{self, Document, HEADER, Times};
use crate::account::accounts::{self, Account};
use crate::account::oauth::{BANNED_MESSAGE, Bearer};
use crate::account::roles;
use crate::base::crypto;
use crate::base::error::ApiError;
use crate::base::state::AppState;

/// The public document's caching: short, the same for everyone.
pub const PUBLIC_CACHE: &str = "public, max-age=15, stale-while-revalidate=30";
/// The history's.
pub const HISTORY_CACHE: &str = "public, max-age=300";
/// The signed-in reads': never kept anywhere.
pub const PRIVATE_CACHE: &str = "no-store";

const RATE_LIMITED: &str = "Too many requests. Try again in a minute.";

/// `429 rate_limited` with `Retry-After`, never cached.
pub fn rate_limited(wait: Duration) -> Response {
    let seconds = wait.whole_seconds().max(1);
    let body = serde_json::json!({ "error": { "code": "rate_limited", "message": RATE_LIMITED } });
    let mut response = (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response();
    let headers = response.headers_mut();
    headers.insert(header::RETRY_AFTER, HeaderValue::from(seconds));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Counts a public read from the caller's address (`CF-Connecting-IP`; a
/// request without it, which only a local server sees, counts as one
/// address).
pub fn check_ip(state: &AppState, headers: &HeaderMap, now: OffsetDateTime) -> Result<(), Duration> {
    let address = client_ip(headers).unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
    state.uptime.check_read_from(address, now)
}

/// Whether `If-None-Match` names `etag` (or `*`).
fn matches(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.split(',').map(str::trim).any(|tag| tag == "*" || tag.trim_start_matches("W/") == etag))
}

/// `GET /api/bir/status`.
pub async fn status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let now = OffsetDateTime::now_utc();
    if let Err(wait) = check_ip(&state, &headers, now) {
        return rate_limited(wait);
    }
    let published = state.uptime.published(&state.config.forms_origin, now);
    let mut response = if matches(&headers, &published.etag) {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let mut response = published.body.clone().into_response();
        response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        response
    };
    let headers = response.headers_mut();
    headers.insert(header::ETAG, HeaderValue::from_str(&published.etag).expect("an ETag is ASCII"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(PUBLIC_CACHE));
    response
}

/// The signed-in reader: a Bearer token of any scope when the request has
/// one, else the website session. A banned account is refused `403
/// account_banned` either way (a ban ends its sessions, so a session it
/// ended still says why).
async fn reader(state: &AppState, parts: &mut Parts) -> Result<Account, ApiError> {
    if parts.headers.contains_key(header::AUTHORIZATION) {
        return Ok(Bearer::from_request_parts(parts, state).await?.account);
    }
    let Some(token) = accounts::cookie(&parts.headers, state.config.session_cookie()) else {
        return Err(ApiError::InvalidToken);
    };
    let row: Option<(Uuid, String, bool, bool)> = sqlx::query_as(
        "SELECT a.id, a.email, a.banned_at IS NOT NULL, s.revoked_at IS NULL
           FROM web_sessions s JOIN accounts a ON a.id = s.account_id
          WHERE s.token_hash = $1 AND s.expires_at > now()",
    )
    .bind(crypto::hash(token))
    .fetch_optional(&state.db)
    .await?;
    match row {
        Some((_, _, true, _)) => Err(ApiError::forbidden("account_banned", BANNED_MESSAGE)),
        Some((id, email, false, true)) => Ok(Account { id, email }),
        _ => Err(ApiError::InvalidToken),
    }
}

/// The mailbox's last 10.
#[derive(Clone, Debug, Serialize)]
pub struct LastTen {
    pub got_in: usize,
    pub of: usize,
}

/// Today's counted tries, Buwiz-wide (Manila day).
#[derive(Clone, Debug, Serialize)]
pub struct Today {
    pub tries: i64,
    pub got_in: i64,
    pub accounts: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Detail {
    pub last_10: LastTen,
    pub today: Today,
}

/// The signed-in document.
#[derive(Clone, Debug, Serialize)]
pub struct Mine {
    #[serde(flatten)]
    pub sign: Document,
    /// Staff only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Detail>,
}

/// `GET /api/bir/status/mine`: the live sign. Everyone signed in gets it
/// with public times (5-minute marks) and no counts; staff (Admin or
/// Support) get times to the minute and the counts.
pub async fn status_mine(State(state): State<AppState>, mut parts: Parts) -> Response {
    let account = match reader(&state, &mut parts).await {
        Ok(account) => account,
        Err(error) => return no_store(error.into_response()),
    };
    let now = OffsetDateTime::now_utc();
    if let Err(wait) = state.uptime.reads_per_account.check(account.id, now) {
        return rate_limited(wait);
    }
    let staff = match roles::staff(&state.db, &state.config, &account).await {
        Ok(staff) => staff.is_some(),
        Err(error) => return no_store(ApiError::from(error).into_response()),
    };
    let sign = state.uptime.live.sign();
    let times = if staff { Times::Exact } else { Times::Public };
    let detail = if staff {
        let tally = state.uptime.live.last_ten();
        let today = match today(&state, now).await {
            Ok(today) => today,
            Err(error) => return no_store(ApiError::from(error).into_response()),
        };
        Some(Detail { last_10: LastTen { got_in: tally.got_in, of: tally.of }, today })
    } else {
        None
    };
    let mine = Mine { sign: public::document(&sign, &state.config.forms_origin, times), detail };
    no_store(Json(mine).into_response())
}

fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Today's counted tries and the accounts they came from: the larger of the
/// day's set in memory and the distinct keys within one key period.
async fn today(state: &AppState, now: OffsetDateTime) -> sqlx::Result<Today> {
    let day = manila_day(now);
    let start = manila_midnight(day);
    let (tries, got_in, accounts): (i64, i64, Option<i64>) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE counted), count(*) FILTER (WHERE counted AND outcome = 'sent'),
                (SELECT max(n) FROM (SELECT count(DISTINCT account_key) AS n FROM bir_tries
                                      WHERE counted AND ended_at >= $1 AND ended_at < $2 GROUP BY key_epoch) AS p)
           FROM bir_tries WHERE ended_at >= $1 AND ended_at < $2",
    )
    .bind(start)
    .bind(start + Duration::DAY)
    .fetch_one(&state.db)
    .await?;
    let in_memory = state.uptime.day_sets.counts(day).map_or(0, |(accounts, _)| accounts);
    let accounts = accounts.unwrap_or(0).max(i64::try_from(in_memory).unwrap_or(i64::MAX));
    Ok(Today { tries, got_in, accounts })
}

/// `from` and `to` of a query, each at most once (a second is refused, as
/// `bad_range`); other parameters are ignored.
fn range_query(query: Option<&str>) -> Result<(Option<String>, Option<String>), RangeError> {
    let (mut from, mut to) = (None, None);
    for (name, value) in form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        let slot = match &*name {
            "from" => &mut from,
            "to" => &mut to,
            _ => continue,
        };
        if slot.replace(value.into_owned()).is_some() {
            return Err(RangeError::Repeated);
        }
    }
    Ok((from, to))
}

/// `GET /api/bir/history?from=YYYY-MM-DD&to=YYYY-MM-DD`.
pub async fn history(State(state): State<AppState>, headers: HeaderMap, RawQuery(query): RawQuery) -> Response {
    let now = OffsetDateTime::now_utc();
    if let Err(wait) = check_ip(&state, &headers, now) {
        return rate_limited(wait);
    }
    let (from, to) = match range_query(query.as_deref()) {
        Ok(asked) => asked,
        Err(error) => return bad_range(error),
    };
    let (from, to) = match history::range(from.as_deref(), to.as_deref(), now) {
        Ok(range) => range,
        Err(error) => return bad_range(error),
    };
    // From the history in memory, as at the last 5-minute mark; a range
    // older than it (no today in it) reads only the day lines.
    let answer = match state.uptime.history.get(&state.db, now).await {
        Ok(cached) => match history::slice(&cached, from, to) {
            Some(history) => Ok(history),
            None => history::history(&state.db, from, to, public::round_down(now), Some(cached.last_24h.clone())).await,
        },
        Err(error) => Err(error),
    };
    match answer {
        Ok(history) => {
            let mut response = Json(history).into_response();
            response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(HISTORY_CACHE));
            response
        }
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn bad_range(error: RangeError) -> Response {
    let refused = ApiError::Client { status: StatusCode::BAD_REQUEST, code: "bad_range", message: error.message() };
    no_store(refused.into_response())
}

/// Adds the compact live sign to a signed-in response apps already get (a
/// ticket, a submission, tax profiles), whatever its status. Its body and
/// its caching stay as they were.
pub async fn sign_header(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let compact = public::compact(&state.uptime.live.sign());
    if let Ok(value) = HeaderValue::from_str(&compact) {
        response.headers_mut().insert(HEADER, value);
    }
    response
}

/// Adds the compact sign of the last 5-minute mark to a response that needs
/// no sign-in (the trust list, the sealing key), as [`sign_header`] does.
pub async fn public_sign_header(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let published = state.uptime.published(&state.config.forms_origin, OffsetDateTime::now_utc());
    if let Ok(value) = HeaderValue::from_str(&published.compact) {
        response.headers_mut().insert(HEADER, value);
    }
    response
}
