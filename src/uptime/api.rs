//! The read routes (docs/bir-uptime.md §4) and the `buwiz-bir` header on
//! responses apps already get (§3). They exist from `BIR_UPTIME=public`:
//! below it they are not routed at all, so every one answers the site's own
//! 404 and nothing else changes (`crate::account::web::router`).
//!
//! - `GET /api/bir/status`: the public document, the same bytes for
//!   everyone, with an `ETag` and `Cache-Control: public, max-age=15,
//!   stale-while-revalidate=30`. 120 a minute per IP address (an IPv6
//!   address by its /64), held in memory only.
//! - `GET /api/bir/status/mine`: a Bearer token of any scope, or the website
//!   session. The document with times to the minute, plus the last 10's
//!   tally and today's counts. 60 a minute per account. `private, no-cache`,
//!   no `ETag`, never `304`.
//! - `GET /api/bir/history`: the day lines and the last 24 hours, public,
//!   cacheable 5 minutes, limited per IP with the status.

use axum::Json;
use axum::extract::{FromRequestParts, RawQuery, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::request::Parts;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use serde::Serialize;
use time::{Duration, OffsetDateTime};

use super::history::{self, RangeError};
use super::keys::{client_ip, manila_day, manila_midnight};
use super::public::{self, Document, HEADER, Times};
use crate::account::accounts::Account;
use crate::account::oauth::Bearer;
use crate::account::web::session_account;
use crate::base::error::ApiError;
use crate::base::state::AppState;

/// The public document's caching: short, the same for everyone.
pub const PUBLIC_CACHE: &str = "public, max-age=15, stale-while-revalidate=30";
/// The history's.
pub const HISTORY_CACHE: &str = "public, max-age=300";
/// The signed-in detail's: never a shared cache.
pub const PRIVATE_CACHE: &str = "private, no-cache";

const RATE_LIMITED: &str = "Too many requests. Try again in a minute.";

/// `429 rate_limited` with `Retry-After`, never cached.
fn rate_limited(wait: Duration) -> Response {
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
fn check_ip(state: &AppState, headers: &HeaderMap, now: OffsetDateTime) -> Result<(), Duration> {
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
    let sign = state.uptime.live.sign();
    let etag = public::etag(&sign);
    let mut response = if matches(&headers, &etag) {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        Json(public::document(&sign, &state.config.forms_origin, Times::Public)).into_response()
    };
    let headers = response.headers_mut();
    headers.insert(header::ETAG, HeaderValue::from_str(&etag).expect("an ETag is ASCII"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(PUBLIC_CACHE));
    response
}

/// The signed-in reader: a Bearer token of any scope when the request has
/// one, else the website session.
async fn reader(state: &AppState, parts: &mut Parts) -> Result<Account, ApiError> {
    if parts.headers.contains_key(header::AUTHORIZATION) {
        return Ok(Bearer::from_request_parts(parts, state).await?.account);
    }
    session_account(state, &parts.headers).await?.ok_or(ApiError::InvalidToken)
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
    pub detail: Detail,
}

/// `GET /api/bir/status/mine`.
pub async fn status_mine(State(state): State<AppState>, mut parts: Parts) -> Response {
    let account = match reader(&state, &mut parts).await {
        Ok(account) => account,
        Err(error) => return no_store(error.into_response()),
    };
    let now = OffsetDateTime::now_utc();
    if let Err(wait) = state.uptime.reads_per_account.check(account.id, now) {
        return rate_limited(wait);
    }
    let sign = state.uptime.live.sign();
    let tally = state.uptime.live.last_ten();
    let today = match today(&state, now).await {
        Ok(today) => today,
        Err(error) => return no_store(ApiError::from(error).into_response()),
    };
    let mine = Mine {
        sign: public::document(&sign, &state.config.forms_origin, Times::Exact),
        detail: Detail { last_10: LastTen { got_in: tally.got_in, of: tally.of }, today },
    };
    let mut response = Json(mine).into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(PRIVATE_CACHE));
    response
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
    match history::history(&state.db, from, to, now).await {
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

/// Adds the compact sign, from memory, to a response apps already get
/// (whatever its status). Its body and its caching stay as they were.
pub async fn sign_header(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let compact = public::compact(&state.uptime.live.sign());
    if let Ok(value) = HeaderValue::from_str(&compact) {
        response.headers_mut().insert(HEADER, value);
    }
    response
}
