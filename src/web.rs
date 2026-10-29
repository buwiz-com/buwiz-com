//! The HTTP surface: JSON and OAuth endpoints for the apps, form posts from
//! the account site, and the Topcoat pages as the fallback.

// Form handlers return early with the redirect or refusal to send, so
// `Result<_, Response>` is the point, not an accident.
#![allow(clippy::result_large_err)]

use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use serde::Deserialize;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use crate::accounts::{self, Account, CodeError};
use crate::agents;
use crate::claims::{self, DecideError, DocumentKind, SubmitError, Upload};
use crate::devices;
use crate::egress;
use crate::mcp;
use crate::notice::Notice;
use crate::oauth;
use crate::state::AppState;
use crate::submissions;
use crate::tickets;
use crate::tin::Tin;

/// Room for the largest claim: two required files and five supporting
/// ones, 10 MB each, plus form overhead. Each file is checked on its own.
const CLAIM_BODY_LIMIT: usize = 41 * 1024 * 1024;
const CLAIM_TOTAL_BYTES: usize = 40 * 1024 * 1024;

pub fn router(state: AppState, pages: topcoat::router::Router) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/assets/site.css", get(stylesheet))
        .route("/assets/home.css", get(home_stylesheet))
        .route("/assets/screens/{name}", get(screenshot))
        .route("/assets/demo.js", get(demo_script))
        .route("/assets/demo.css", get(demo_stylesheet))
        .route("/assets/{dir}/{name}", get(asset))
        // Desktop app sign-in (device flow) and tokens.
        .route("/auth/device/start", post(oauth::device_start))
        .route("/oauth/device/code", post(oauth::device_start))
        .route("/auth/device/poll", post(oauth::device_poll))
        .route("/oauth/token", post(oauth::token))
        .route("/me", get(oauth::me))
        .route("/api/auth/logout", post(oauth::logout))
        .route("/api/tax-profiles", get(claims::tax_profiles))
        // Submission relay.
        .route(
            "/api/submissions",
            post(submissions::create).get(submissions::list).layer(DefaultBodyLimit::max(submissions::BODY_LIMIT)),
        )
        .route("/api/submissions/{id}", get(submissions::show))
        .route("/api/submissions/{id}/relay", get(submissions::relay))
        .route("/.well-known/jwks.json", get(submissions::jwks))
        // Submission v2: device keys, tickets, sealed returns.
        .route("/api/devices", post(devices::register).get(devices::list))
        .route("/api/devices/{id}/revoke", post(devices::revoke_api))
        .route("/api/tickets", post(tickets::issue))
        .route(
            "/api/v2/submissions",
            post(submissions::create_v2).layer(DefaultBodyLimit::max(submissions::BODY_LIMIT)),
        )
        .route("/.well-known/buwiz-hpke.json", get(submissions::hpke_key))
        .route("/.well-known/buwiz-egress.json", get(egress::list))
        // MCP and OAuth for AI agents.
        .route("/mcp", post(mcp::handle))
        .route("/.well-known/oauth-protected-resource", get(agents::protected_resource))
        .route("/.well-known/oauth-protected-resource/mcp", get(agents::protected_resource))
        .route("/.well-known/oauth-authorization-server", get(agents::authorization_server))
        .route("/oauth/register", post(agents::register))
        .route("/oauth/authorize/decision", post(agents::decide))
        // Account site forms.
        .route("/auth/email/start", post(email_start))
        .route("/auth/email/verify", post(email_verify))
        .route("/logout", post(logout))
        .route("/device/decision", post(device_decision))
        .route("/tins/claims", post(submit_claim).layer(DefaultBodyLimit::max(CLAIM_BODY_LIMIT)))
        .route("/tins/claims/{claim_id}/withdraw", post(withdraw_claim))
        .route("/devices/{device_id}/revoke", post(revoke_device))
        .route("/admin/claims/{claim_id}/decision", post(decide_claim))
        .route("/admin/documents/{document_id}", get(admin_document))
        .fallback(move |request: axum::extract::Request| {
            let pages = pages.clone();
            async move { render_page(&pages, request).await }
        })
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn stylesheet() -> Response {
    css(include_str!("site.css"))
}

async fn home_stylesheet() -> Response {
    css(include_str!("home.css"))
}

async fn demo_script() -> Response {
    text(include_str!("demo.js"), "text/javascript; charset=utf-8")
}

async fn demo_stylesheet() -> Response {
    css(include_str!("demo.css"))
}

fn css(body: &'static str) -> Response {
    text(body, "text/css; charset=utf-8")
}

fn text(body: &'static str, content_type: &'static str) -> Response {
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=300"));
    response
}

/// Other files the pages load: the Buwiz logo, and for the home page's AI
/// agent demos the two printed 1601-C documents their Computer screen shows
/// and the font those use. (path under /assets, content type, bytes)
pub(crate) const ASSETS: &[(&str, &str, &[u8])] = &[
    ("brand/buwiz_logo_light.webp", "image/webp", include_bytes!("assets/brand/buwiz_logo_light.webp")),
    ("brand/buwiz_logo_dark.webp", "image/webp", include_bytes!("assets/brand/buwiz_logo_dark.webp")),
    ("demo/1601c-october-2026.html", "text/html; charset=utf-8", include_bytes!("assets/demo/1601c-october-2026.html")),
    (
        "demo/1601c-october-2026-confirmation.html",
        "text/html; charset=utf-8",
        include_bytes!("assets/demo/1601c-october-2026-confirmation.html"),
    ),
    ("fonts/tinos-latin-400-normal.woff2", "font/woff2", include_bytes!("assets/fonts/tinos-latin-400-normal.woff2")),
];

async fn asset(Path((dir, name)): Path<(String, String)>) -> Response {
    let path = format!("{dir}/{name}");
    let Some((_, content_type, bytes)) = ASSETS.iter().find(|(known, _, _)| *known == path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400"));
    response
}

/// Screenshots of the Buwiz app for the home page's tour, light and dark.
pub(crate) const SCREENSHOTS: &[(&str, &[u8])] = &[
    ("side_by_side_light.webp", include_bytes!("assets/shell_side_by_side_light.webp")),
    ("side_by_side_dark.webp", include_bytes!("assets/shell_side_by_side_dark.webp")),
    ("form_2550q_light.webp", include_bytes!("assets/shell_form_2550q_light.webp")),
    ("form_2550q_dark.webp", include_bytes!("assets/shell_form_2550q_dark.webp")),
    ("templates_light.webp", include_bytes!("assets/shell_templates_light.webp")),
    ("templates_dark.webp", include_bytes!("assets/shell_templates_dark.webp")),
    ("panel_profile_light.webp", include_bytes!("assets/shell_panel_profile_light.webp")),
    ("panel_profile_dark.webp", include_bytes!("assets/shell_panel_profile_dark.webp")),
    ("calendar_light.webp", include_bytes!("assets/shell_calendar_light.webp")),
    ("calendar_dark.webp", include_bytes!("assets/shell_calendar_dark.webp")),
    ("palette_light.webp", include_bytes!("assets/shell_palette_light.webp")),
    ("palette_dark.webp", include_bytes!("assets/shell_palette_dark.webp")),
];

async fn screenshot(Path(name): Path<String>) -> Response {
    let Some((_, bytes)) = SCREENSHOTS.iter().find(|(known, _)| *known == name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/webp"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400"));
    response
}

/// Hands a request to the Topcoat router and converts its response back.
async fn render_page(pages: &topcoat::router::Router, request: axum::extract::Request) -> Response {
    let (parts, body) = request.into_parts();
    let request = http::Request::from_parts(parts, topcoat::router::Body::new(body));
    let mut response = pages.handle(request).await.map(axum::body::Body::new);
    let headers = response.headers_mut();
    headers.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static("no-store"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert("referrer-policy", HeaderValue::from_static("same-origin"));
    response
}

/// The signed-in person on the account site, from the session cookie.
pub async fn session_account(state: &AppState, headers: &HeaderMap) -> anyhow::Result<Option<Account>> {
    let Some(token) = accounts::cookie(headers, state.config.session_cookie()) else {
        return Ok(None);
    };
    Ok(accounts::session_account(&state.db, token).await?)
}

/// Form posts must come from our own pages. Browsers send `Origin` on every
/// POST; `Sec-Fetch-Site` covers the rare case they leave it out.
pub(crate) fn same_origin(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    let ok = match headers.get(header::ORIGIN).and_then(|value| value.to_str().ok()) {
        Some(origin) => state.config.is_own_origin(origin),
        None => headers.get("sec-fetch-site").is_some_and(|value| value == "same-origin"),
    };
    if ok { Ok(()) } else { Err((StatusCode::FORBIDDEN, "cross-site form post refused").into_response()) }
}

/// Only paths on this site, so `next` cannot send people elsewhere.
pub fn safe_next(raw: Option<&str>) -> String {
    match raw {
        Some(next) if next.starts_with('/') && !next.starts_with("//") && !next.contains('\\') => next.to_owned(),
        _ => "/account".to_owned(),
    }
}

/// `path?key=value&…`, with `notice` added when there is one.
pub fn link(path: &str, notice: Option<Notice>, params: &[(&str, &str)]) -> String {
    let mut query = form_urlencoded::Serializer::new(String::new());
    for (key, value) in params {
        query.append_pair(key, value);
    }
    if let Some(notice) = notice {
        query.append_pair("notice", notice.code());
    }
    let query = query.finish();
    if query.is_empty() { path.to_owned() } else { format!("{path}?{query}") }
}

fn see_other(location: String) -> Response {
    Redirect::to(&location).into_response()
}

fn session_cookie(state: &AppState, value: &str, max_age: i64) -> HeaderValue {
    let secure = if state.config.secure_cookies { "; Secure" } else { "" };
    let cookie =
        format!("{}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}", state.config.session_cookie());
    HeaderValue::from_str(&cookie).expect("cookie is ASCII")
}

fn internal(error: impl Into<anyhow::Error>) -> Response {
    tracing::error!(error = ?error.into(), "form post failed");
    (StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong. Try again.").into_response()
}

#[derive(Deserialize)]
struct EmailStart {
    email: String,
    next: Option<String>,
}

async fn email_start(State(state): State<AppState>, headers: HeaderMap, Form(form): Form<EmailStart>) -> Response {
    if let Err(refused) = same_origin(&state, &headers) {
        return refused;
    }
    let next = safe_next(form.next.as_deref());
    let email = match accounts::normalize_email(&form.email) {
        Ok(email) => email,
        Err(notice) => return see_other(link("/login", Some(notice), &[("next", &next)])),
    };
    let code = match accounts::issue_code(&state.db, &email).await {
        Ok(code) => code,
        Err(CodeError::TooMany) => {
            return see_other(link("/login", Some(Notice::CodeTooMany), &[("next", &next), ("email", &email)]));
        }
        Err(error) => return internal(anyhow::anyhow!(error)),
    };
    if let Err(error) = state.mailer.send_sign_in_code(&email, &code).await {
        return internal(error);
    }
    see_other(link("/login/verify", Some(Notice::CodeSent), &[("email", &email), ("next", &next)]))
}

#[derive(Deserialize)]
struct EmailVerify {
    email: String,
    code: String,
    next: Option<String>,
}

async fn email_verify(State(state): State<AppState>, headers: HeaderMap, Form(form): Form<EmailVerify>) -> Response {
    if let Err(refused) = same_origin(&state, &headers) {
        return refused;
    }
    let next = safe_next(form.next.as_deref());
    let Ok(email) = accounts::normalize_email(&form.email) else {
        return see_other(link("/login", Some(Notice::EmailInvalid), &[("next", &next)]));
    };
    let account = match accounts::redeem_code(&state.db, &email, &form.code).await {
        Ok(account) => account,
        Err(CodeError::Invalid | CodeError::TooMany) => {
            return see_other(link("/login/verify", Some(Notice::CodeInvalid), &[("email", &email), ("next", &next)]));
        }
        Err(CodeError::Internal(error)) => return internal(error),
    };
    let token = match accounts::start_session(&state.db, account.id).await {
        Ok(token) => token,
        Err(error) => return internal(error),
    };
    let mut response = see_other(next);
    response
        .headers_mut()
        .insert(header::SET_COOKIE, session_cookie(&state, &token, accounts::SESSION_TTL.whole_seconds()));
    response
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(refused) = same_origin(&state, &headers) {
        return refused;
    }
    if let Some(token) = accounts::cookie(&headers, state.config.session_cookie())
        && let Err(error) = accounts::end_session(&state.db, token).await
    {
        return internal(error);
    }
    let mut response = see_other(link("/login", Some(Notice::SignedOut), &[]));
    response.headers_mut().insert(header::SET_COOKIE, session_cookie(&state, "", 0));
    response
}

/// Signed-in account, or a redirect to sign in and come back to `back_to`.
async fn require_session(state: &AppState, headers: &HeaderMap, back_to: &str) -> Result<Account, Response> {
    match session_account(state, headers).await {
        Ok(Some(account)) => Ok(account),
        Ok(None) => Err(see_other(link("/login", Some(Notice::SignInFirst), &[("next", back_to)]))),
        Err(error) => Err(internal(error)),
    }
}

#[derive(Deserialize)]
struct DeviceDecision {
    user_code: String,
    decision: String,
}

async fn device_decision(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<DeviceDecision>,
) -> Response {
    if let Err(refused) = same_origin(&state, &headers) {
        return refused;
    }
    let back_to = link("/device", None, &[("user_code", &form.user_code)]);
    let account = match require_session(&state, &headers, &back_to).await {
        Ok(account) => account,
        Err(redirect) => return redirect,
    };
    let Some(user_code) = oauth::normalize_user_code(&form.user_code) else {
        return see_other(link("/device", Some(Notice::DeviceInvalid), &[]));
    };
    let approve = form.decision == "approve";
    match oauth::decide_device(&state.db, &user_code, account.id, approve).await {
        Ok(true) => {
            let notice = if approve { Notice::DeviceApproved } else { Notice::DeviceDenied };
            see_other(link("/device", Some(notice), &[]))
        }
        Ok(false) => see_other(link("/device", Some(Notice::DeviceInvalid), &[])),
        Err(error) => internal(error),
    }
}

async fn submit_claim(State(state): State<AppState>, headers: HeaderMap, multipart: Multipart) -> Response {
    if let Err(refused) = same_origin(&state, &headers) {
        return refused;
    }
    let account = match require_session(&state, &headers, "/tins/new").await {
        Ok(account) => account,
        Err(redirect) => return redirect,
    };
    let back = |notice: Notice| see_other(link("/tins/new", Some(notice), &[]));
    let form = match read_claim_form(multipart).await {
        Ok(form) => form,
        Err(notice) => return back(notice),
    };
    let tin = match Tin::parse_combined(&form.tin) {
        Ok(tin) => tin,
        Err(notice) => return back(notice),
    };
    let submitted = claims::submit(
        &state.db,
        &state.documents,
        &state.config.document_key,
        account.id,
        &tin,
        &form.registered_name,
        form.uploads,
    )
    .await;
    match submitted {
        Ok(_) => see_other(link("/account", Some(Notice::ClaimSubmitted), &[])),
        Err(SubmitError::Invalid(notice)) => back(notice),
        Err(SubmitError::Internal(error)) => internal(error),
    }
}

#[derive(Default)]
struct ClaimForm {
    tin: String,
    registered_name: String,
    uploads: Vec<Upload>,
}

async fn read_claim_form(mut multipart: Multipart) -> Result<ClaimForm, Notice> {
    let mut form = ClaimForm::default();
    let mut total = 0usize;
    while let Some(field) = multipart.next_field().await.map_err(|_| Notice::BadUpload)? {
        let name = field.name().unwrap_or_default().to_owned();
        let kind = match name.as_str() {
            "tin" => {
                form.tin = field.text().await.map_err(|_| Notice::BadUpload)?;
                continue;
            }
            "registered_name" => {
                form.registered_name = field.text().await.map_err(|_| Notice::BadUpload)?;
                continue;
            }
            "form_2303" => DocumentKind::Form2303,
            "government_id" => DocumentKind::GovernmentId,
            "supporting" => DocumentKind::Supporting,
            _ => continue,
        };
        let filename = field.file_name().unwrap_or_default().to_owned();
        let bytes = field.bytes().await.map_err(|_| Notice::FileTooBig)?;
        // An empty file input still sends a part with no name and no bytes.
        if bytes.is_empty() && filename.is_empty() {
            continue;
        }
        total += bytes.len();
        if bytes.len() > claims::MAX_FILE_BYTES || total > CLAIM_TOTAL_BYTES {
            return Err(Notice::FileTooBig);
        }
        form.uploads.push(Upload { kind, filename, bytes: bytes.to_vec() });
    }
    Ok(form)
}

async fn withdraw_claim(State(state): State<AppState>, headers: HeaderMap, Path(claim_id): Path<Uuid>) -> Response {
    if let Err(refused) = same_origin(&state, &headers) {
        return refused;
    }
    let account = match require_session(&state, &headers, "/account").await {
        Ok(account) => account,
        Err(redirect) => return redirect,
    };
    match claims::withdraw(&state.db, &state.documents, claim_id, account.id).await {
        Ok(true) => see_other(link("/account", Some(Notice::ClaimWithdrawn), &[])),
        Ok(false) => see_other(link("/account", Some(Notice::AlreadyDecided), &[])),
        Err(error) => internal(error),
    }
}

async fn revoke_device(State(state): State<AppState>, headers: HeaderMap, Path(device_id): Path<Uuid>) -> Response {
    if let Err(refused) = same_origin(&state, &headers) {
        return refused;
    }
    let account = match require_session(&state, &headers, "/account").await {
        Ok(account) => account,
        Err(redirect) => return redirect,
    };
    match devices::revoke(&state.db, account.id, device_id).await {
        Ok(true) => see_other(link("/account", Some(Notice::DeviceRevoked), &[])),
        Ok(false) => see_other(link("/account", None, &[])),
        Err(error) => internal(error),
    }
}

/// Signed-in staff member, or the response to send instead.
async fn require_admin(state: &AppState, headers: &HeaderMap, back_to: &str) -> Result<Account, Response> {
    let account = require_session(state, headers, back_to).await?;
    if state.config.is_admin(&account.email) {
        Ok(account)
    } else {
        Err((StatusCode::NOT_FOUND, "not found").into_response())
    }
}

#[derive(Deserialize)]
struct Decision {
    decision: String,
    #[serde(default)]
    note: String,
}

async fn decide_claim(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(claim_id): Path<Uuid>,
    Form(form): Form<Decision>,
) -> Response {
    if let Err(refused) = same_origin(&state, &headers) {
        return refused;
    }
    let back_to = format!("/admin/claims/{claim_id}");
    let reviewer = match require_admin(&state, &headers, &back_to).await {
        Ok(reviewer) => reviewer,
        Err(response) => return response,
    };
    let approve = match form.decision.as_str() {
        "approve" => true,
        "reject" => false,
        _ => return (StatusCode::BAD_REQUEST, "unknown decision").into_response(),
    };
    match claims::decide(&state.db, &state.documents, claim_id, reviewer.id, approve, &form.note).await {
        Ok(()) => see_other(link("/admin/claims", Some(Notice::ClaimDecided), &[])),
        Err(DecideError::Refused(notice)) => see_other(link(&back_to, Some(notice), &[])),
        Err(DecideError::Internal(error)) => internal(error),
    }
}

async fn admin_document(State(state): State<AppState>, headers: HeaderMap, Path(document_id): Path<Uuid>) -> Response {
    if let Err(response) = require_admin(&state, &headers, "/admin/claims").await {
        return response;
    }
    let document =
        match claims::open_document(&state.db, &state.documents, &state.config.document_key, document_id).await {
            Ok(Some(document)) => document,
            Ok(None) => return (StatusCode::GONE, "This document was deleted when its claim ended.").into_response(),
            Err(error) => return internal(error),
        };
    let disposition = format!("inline; filename=\"{}\"", document.filename);
    let mut response = document.bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(&document.content_type).expect("sniffed type"));
    headers.insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(&disposition).expect("clean filename"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    headers.insert("content-security-policy", HeaderValue::from_static("sandbox; default-src 'none'"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_stays_on_this_site() {
        assert_eq!(safe_next(Some("/device?user_code=BCDF-GHJK")), "/device?user_code=BCDF-GHJK");
        assert_eq!(safe_next(Some("//evil.example")), "/account");
        assert_eq!(safe_next(Some("https://evil.example")), "/account");
        assert_eq!(safe_next(Some("/\\evil.example")), "/account");
        assert_eq!(safe_next(None), "/account");
    }

    #[test]
    fn links_encode_params_and_notice() {
        assert_eq!(link("/login", None, &[]), "/login");
        assert_eq!(
            link("/login", Some(Notice::SignInFirst), &[("next", "/device?user_code=A B")]),
            "/login?next=%2Fdevice%3Fuser_code%3DA+B&notice=sign_in_first"
        );
    }
}
