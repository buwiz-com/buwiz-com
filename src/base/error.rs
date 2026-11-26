//! Errors returned by the JSON and OAuth endpoints.

use axum::Json;
use axum::response::{IntoResponse, Response};
use http::{HeaderValue, StatusCode, header};
use serde_json::json;

/// An error with the shape the Buwiz apps read:
/// `{"error":{"code":"…","message":"…"}}`.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{message}")]
    Client { status: StatusCode, code: &'static str, message: &'static str },
    /// The same shape, with words made for this request (a claim's fix
    /// reasons, say).
    #[error("{message}")]
    Refused { status: StatusCode, code: &'static str, message: String },
    /// Missing, expired or revoked access token. Answers 401 with
    /// `WWW-Authenticate`.
    #[error("sign in again")]
    InvalidToken,
    /// Over a limit: `429 rate_limited`, with `Retry-After` in seconds and
    /// the minutes to wait in the message. Never cached.
    #[error("too many requests")]
    RateLimited { retry_after: time::Duration },
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl ApiError {
    pub fn bad_request(code: &'static str, message: &'static str) -> Self {
        Self::Client { status: StatusCode::BAD_REQUEST, code, message }
    }

    pub fn forbidden(code: &'static str, message: &'static str) -> Self {
        Self::Client { status: StatusCode::FORBIDDEN, code, message }
    }

    pub fn conflict(code: &'static str, message: &'static str) -> Self {
        Self::Client { status: StatusCode::CONFLICT, code, message }
    }

    /// [`ApiError::RateLimited`]: try again after `wait`.
    pub fn rate_limited(wait: time::Duration) -> Self {
        Self::RateLimited { retry_after: wait }
    }

    pub fn not_found() -> Self {
        Self::Client { status: StatusCode::NOT_FOUND, code: "not_found", message: "not found" }
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Client { status, code, message } => {
                (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
            }
            Self::Refused { status, code, message } => {
                (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
            }
            Self::InvalidToken => {
                let body = json!({ "error": { "code": "invalid_token", "message": "sign in again" } });
                let mut response = (StatusCode::UNAUTHORIZED, Json(body)).into_response();
                response
                    .headers_mut()
                    .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static(r#"Bearer error="invalid_token""#));
                response
            }
            Self::RateLimited { retry_after } => {
                // At least a second, and whole minutes rounded up in words.
                let seconds = retry_after.whole_seconds().max(1);
                let minutes = (seconds + 59) / 60;
                let unit = if minutes == 1 { "minute" } else { "minutes" };
                let message = format!("Too many requests. Try again in {minutes} {unit}.");
                let body = json!({ "error": { "code": "rate_limited", "message": message } });
                let mut response = (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response();
                let headers = response.headers_mut();
                headers.insert(header::RETRY_AFTER, HeaderValue::from(seconds));
                headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                response
            }
            Self::Internal(error) => {
                tracing::error!(error = ?error, "request failed");
                let body = json!({ "error": { "code": "internal", "message": "something went wrong" } });
                (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
            }
        }
    }
}

/// An RFC 6749 / RFC 8628 error: `{"error":"…","error_description":"…"}`.
#[derive(Debug)]
pub struct OAuthError {
    pub status: StatusCode,
    pub error: &'static str,
    pub description: &'static str,
}

impl OAuthError {
    pub const fn new(error: &'static str, description: &'static str) -> Self {
        Self { status: StatusCode::BAD_REQUEST, error, description }
    }
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        let body = json!({ "error": self.error, "error_description": self.description });
        let mut response = (self.status, Json(body)).into_response();
        response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}

/// OAuth endpoints answer database failures as a plain 500.
pub enum OAuthFailure {
    OAuth(OAuthError),
    Internal(anyhow::Error),
}

impl From<OAuthError> for OAuthFailure {
    fn from(error: OAuthError) -> Self {
        Self::OAuth(error)
    }
}

impl From<sqlx::Error> for OAuthFailure {
    fn from(error: sqlx::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl IntoResponse for OAuthFailure {
    fn into_response(self) -> Response {
        match self {
            Self::OAuth(error) => error.into_response(),
            Self::Internal(error) => ApiError::Internal(error).into_response(),
        }
    }
}
