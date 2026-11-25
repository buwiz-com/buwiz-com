//! OAuth for AI agents, as the MCP authorization spec expects: discovery
//! metadata (RFC 9728, RFC 8414), dynamic client registration (RFC 7591)
//! and the authorization code flow with PKCE (S256 only).
//!
//! Agents are public clients. The person approves each one on the account
//! site. Their tokens carry the `mcp` scope: they read through `/mcp` but
//! can never file returns.
//!
//! The same endpoints sign the eBIRForms apps in ("Sign in with Buwiz",
//! docs/accounts.md): a built-in client ([`oauth::CLIENTS`]) with a fixed
//! list of redirect addresses ([`app_redirects`]), never registered, whose
//! code carries the `app` scope and the app's install.

use std::collections::HashMap;

use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Form, Json};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use url::Url;
use uuid::Uuid;

use crate::account::app_sign_in::{self, Platform};
use crate::account::notice::Notice;
use crate::account::oauth::{self, SCOPE_APP, SCOPE_MCP, TokenResponse};
use crate::base::config::Config;
use crate::base::crypto;
use crate::base::error::{OAuthError, OAuthFailure};
use crate::base::state::AppState;

const CODE_TTL: Duration = Duration::minutes(10);
const MAX_REDIRECT_URIS: usize = 5;
/// The shortest `state` an app may send: it must be the app's own random
/// value, not a constant.
const MIN_APP_STATE: usize = 16;
/// The eBIRForms apps' own URL scheme. No agent may register it, so a code
/// sent there is always the app's.
const APP_SCHEME: &str = "ebirforms";

/// A redirect address a built-in app client may use. Fixed here; never
/// registered, never in `oauth_clients`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppRedirect {
    /// This string exactly.
    Exact(&'static str),
    /// `http://127.0.0.1:<port>/callback`: the loopback address (RFC 8252
    /// §7.3) on any port, which must be written out; nothing else.
    Loopback,
}

/// The redirect addresses of a built-in client. Only the eBIRForms app
/// (`buwiz-desktop`) has any; the other built-in clients cannot use
/// `/oauth/authorize`.
pub fn app_redirects(client_id: &str) -> &'static [AppRedirect] {
    match client_id {
        "buwiz-desktop" => &[AppRedirect::Exact("ebirforms://signin-callback"), AppRedirect::Loopback],
        _ => &[],
    }
}

impl AppRedirect {
    pub fn matches(self, requested: &str) -> bool {
        match self {
            Self::Exact(address) => requested == address,
            Self::Loopback => requested
                .strip_prefix("http://127.0.0.1:")
                .and_then(|rest| rest.strip_suffix("/callback"))
                .is_some_and(|port| {
                    (1..=5).contains(&port.len())
                        && port.bytes().all(|byte| byte.is_ascii_digit())
                        && !port.starts_with('0')
                        && port.parse::<u16>().is_ok()
                }),
        }
    }
}

/// The origin this request came in on, if it is one of ours; otherwise the
/// account site. Discovery documents use it so a client sees the URLs it
/// reached us by.
pub fn request_origin(config: &Config, headers: &HeaderMap) -> String {
    let host = headers.get(http::header::HOST).and_then(|value| value.to_str().ok()).unwrap_or_default();
    std::iter::once(&config.account_origin)
        .chain(&config.extra_origins)
        .find(|origin| origin.split_once("://").is_some_and(|(_, authority)| authority == host))
        .unwrap_or(&config.account_origin)
        .clone()
}

/// `GET /.well-known/oauth-protected-resource[/mcp]` (RFC 9728).
pub async fn protected_resource(State(state): State<AppState>, headers: HeaderMap) -> Json<Value> {
    let origin = request_origin(&state.config, &headers);
    Json(json!({
        "resource": format!("{origin}/mcp"),
        "authorization_servers": [state.config.account_origin],
        "scopes_supported": [SCOPE_MCP],
        "bearer_methods_supported": ["header"],
        "resource_name": "Buwiz",
    }))
}

/// `GET /.well-known/oauth-authorization-server` (RFC 8414).
pub async fn authorization_server(State(state): State<AppState>) -> Json<Value> {
    let issuer = &state.config.account_origin;
    Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/oauth/authorize"),
        "token_endpoint": format!("{issuer}/oauth/token"),
        "registration_endpoint": format!("{issuer}/oauth/register"),
        "device_authorization_endpoint": format!("{issuer}/oauth/device/code"),
        "scopes_supported": [SCOPE_MCP],
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token", "urn:ietf:params:oauth:grant-type:device_code"],
        "token_endpoint_auth_methods_supported": ["none"],
        "code_challenge_methods_supported": ["S256"],
        "authorization_response_iss_parameter_supported": true,
    }))
}

/// A redirect URI an agent may register: HTTPS, HTTP on the loopback
/// interface (RFC 8252 §7.3), or a private-use app scheme (§7.1). Never a
/// fragment, never a scheme that runs code in the browser.
pub fn redirect_uri_allowed(raw: &str) -> bool {
    let Ok(uri) = Url::parse(raw) else {
        return false;
    };
    if uri.fragment().is_some() || !uri.username().is_empty() || uri.password().is_some() {
        return false;
    }
    match uri.scheme() {
        APP_SCHEME => false,
        "https" => uri.host().is_some(),
        "http" => is_loopback(&uri),
        "javascript" | "data" | "file" | "vbscript" | "blob" | "about" | "ftp" | "ws" | "wss" => false,
        _ => true,
    }
}

fn is_loopback(uri: &Url) -> bool {
    matches!(uri.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
}

/// Exact match, except that a loopback redirect may use any port (RFC 8252
/// §7.3): native apps get a fresh port each run.
pub fn redirect_uri_matches(registered: &str, requested: &str) -> bool {
    if registered == requested {
        return true;
    }
    let (Ok(mut registered), Ok(mut requested)) = (Url::parse(registered), Url::parse(requested)) else {
        return false;
    };
    if registered.scheme() != "http" || !is_loopback(&registered) {
        return false;
    }
    let _ = registered.set_port(None);
    let _ = requested.set_port(None);
    registered == requested
}

#[derive(Deserialize)]
pub struct Registration {
    client_name: Option<String>,
    #[serde(default)]
    redirect_uris: Vec<String>,
    token_endpoint_auth_method: Option<String>,
}

fn registration_error(error: &'static str, description: &'static str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": error, "error_description": description }))).into_response()
}

/// `POST /oauth/register` (RFC 7591). Public clients only. The id is
/// always `agent_` and a fresh UUID, so it can never be a built-in
/// client's ([`oauth::CLIENTS`]).
pub async fn register(State(state): State<AppState>, Json(request): Json<Registration>) -> Response {
    if request.redirect_uris.is_empty() || request.redirect_uris.len() > MAX_REDIRECT_URIS {
        return registration_error("invalid_redirect_uri", "register 1 to 5 redirect_uris");
    }
    if !request.redirect_uris.iter().all(|uri| uri.len() <= 2000 && redirect_uri_allowed(uri)) {
        return registration_error(
            "invalid_redirect_uri",
            "redirect_uris must be https, http on localhost, or an app scheme of your own, without a fragment",
        );
    }
    if request.token_endpoint_auth_method.as_deref().is_some_and(|method| method != "none") {
        return registration_error("invalid_client_metadata", "only public clients (token_endpoint_auth_method none)");
    }
    let name: String = request.client_name.unwrap_or_default().chars().filter(|ch| !ch.is_control()).take(80).collect();
    let name = if name.trim().is_empty() { "AI agent".to_owned() } else { name.trim().to_owned() };
    let client_id = format!("agent_{}", Uuid::new_v4().simple());
    let inserted = sqlx::query("INSERT INTO oauth_clients (client_id, client_name, redirect_uris) VALUES ($1, $2, $3)")
        .bind(&client_id)
        .bind(&name)
        .bind(&request.redirect_uris)
        .execute(&state.db)
        .await;
    if let Err(error) = inserted {
        tracing::error!(error = ?error, "client registration failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    tracing::info!(%client_id, client_name = %name, "agent registered");
    let body = json!({
        "client_id": client_id,
        "client_id_issued_at": OffsetDateTime::now_utc().unix_timestamp(),
        "client_name": name,
        "redirect_uris": request.redirect_uris,
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "scope": SCOPE_MCP,
    });
    (StatusCode::CREATED, Json(body)).into_response()
}

/// A checked authorization request, ready to show or to approve.
#[derive(Clone, Debug)]
pub struct AuthorizeRequest {
    pub client_id: String,
    pub client_name: String,
    pub redirect_uri: String,
    pub state: Option<String>,
    pub code_challenge: String,
    /// Set for a built-in app client; `None` for an agent.
    pub app: Option<AppRequest>,
}

/// What an app's authorization request adds: its install and platform,
/// kept with the code for the grant.
#[derive(Clone, Debug)]
pub struct AppRequest {
    pub install_id: String,
    pub platform: Platform,
}

impl AuthorizeRequest {
    fn scope(&self) -> &'static str {
        if self.app.is_some() { SCOPE_APP } else { SCOPE_MCP }
    }
}

/// Why an authorization request cannot go on.
#[derive(Debug)]
pub enum AuthorizeProblem {
    /// The client or redirect URI cannot be trusted. Show an error page;
    /// never redirect.
    Untrusted(&'static str),
    /// Tell the client by redirecting with `error=…`.
    Redirect { redirect_uri: String, state: Option<String>, error: &'static str, description: &'static str },
}

/// Checks `/oauth/authorize` parameters against the registered client.
pub async fn check_authorize(
    db: &PgPool,
    config: &Config,
    params: &HashMap<String, String>,
) -> Result<AuthorizeRequest, AuthorizeProblem> {
    let param = |name: &str| params.get(name).map(String::as_str).filter(|value| !value.is_empty());
    let client_id = param("client_id").ok_or(AuthorizeProblem::Untrusted("The request has no client_id."))?;
    // A built-in client never reads `oauth_clients`: only its fixed list
    // of addresses counts, whatever a row there might say.
    if let Some(name) = oauth::client_name(client_id) {
        return check_app_authorize(client_id, name, params);
    }
    let client: Option<(String, Vec<String>)> =
        sqlx::query_as("SELECT client_name, redirect_uris FROM oauth_clients WHERE client_id = $1")
            .bind(client_id)
            .fetch_optional(db)
            .await
            .map_err(|_| AuthorizeProblem::Untrusted("Something went wrong. Try again."))?;
    let (client_name, registered) =
        client.ok_or(AuthorizeProblem::Untrusted("This app is not registered with Buwiz."))?;
    let redirect_uri = match param("redirect_uri") {
        Some(uri) if registered.iter().any(|known| redirect_uri_matches(known, uri)) => uri.to_owned(),
        None if registered.len() == 1 => registered[0].clone(),
        _ => return Err(AuthorizeProblem::Untrusted("The app sent a redirect address it did not register.")),
    };
    let state = param("state").map(str::to_owned);
    let refuse = |error, description| AuthorizeProblem::Redirect {
        redirect_uri: redirect_uri.clone(),
        state: state.clone(),
        error,
        description,
    };
    if param("response_type") != Some("code") {
        return Err(refuse("unsupported_response_type", "only response_type=code"));
    }
    let Some(code_challenge) = param("code_challenge") else {
        return Err(refuse("invalid_request", "PKCE is required: send code_challenge"));
    };
    if param("code_challenge_method") != Some("S256") || code_challenge.len() != 43 {
        return Err(refuse("invalid_request", "use code_challenge_method=S256"));
    }
    if let Some(scope) = param("scope")
        && scope.split_whitespace().any(|item| item != SCOPE_MCP)
    {
        return Err(refuse("invalid_scope", "agents may only ask for the mcp scope"));
    }
    if let Some(resource) = param("resource") {
        let ours = std::iter::once(&config.account_origin)
            .chain(&config.extra_origins)
            .any(|origin| resource.trim_end_matches('/') == format!("{origin}/mcp"));
        if !ours {
            return Err(refuse("invalid_target", "resource must be this server's /mcp"));
        }
    }
    Ok(AuthorizeRequest {
        client_id: client_id.to_owned(),
        client_name,
        redirect_uri,
        state,
        code_challenge: code_challenge.to_owned(),
        app: None,
    })
}

/// [`check_authorize`] for a built-in app client: one of its fixed
/// redirect addresses, PKCE, a `state` of at least 16 characters, its
/// install and platform; the scope is `app`.
fn check_app_authorize(
    client_id: &str,
    client_name: &str,
    params: &HashMap<String, String>,
) -> Result<AuthorizeRequest, AuthorizeProblem> {
    let param = |name: &str| params.get(name).map(String::as_str).filter(|value| !value.is_empty());
    let redirects = app_redirects(client_id);
    if redirects.is_empty() {
        return Err(AuthorizeProblem::Untrusted("This app cannot sign in here."));
    }
    let redirect_uri = match param("redirect_uri") {
        Some(uri) if redirects.iter().any(|known| known.matches(uri)) => uri.to_owned(),
        _ => return Err(AuthorizeProblem::Untrusted("The app sent a redirect address it may not use.")),
    };
    let state = param("state").map(str::to_owned);
    let refuse = |error, description| AuthorizeProblem::Redirect {
        redirect_uri: redirect_uri.clone(),
        state: state.clone(),
        error,
        description,
    };
    if param("response_type") != Some("code") {
        return Err(refuse("unsupported_response_type", "only response_type=code"));
    }
    let Some(code_challenge) = param("code_challenge") else {
        return Err(refuse("invalid_request", "PKCE is required: send code_challenge"));
    };
    if param("code_challenge_method") != Some("S256") || code_challenge.len() != 43 {
        return Err(refuse("invalid_request", "use code_challenge_method=S256"));
    }
    if state.as_ref().is_none_or(|state| state.chars().count() < MIN_APP_STATE) {
        return Err(refuse("invalid_request", "state is required: at least 16 characters"));
    }
    let Some(install_id) = param("install_id").filter(|id| app_sign_in::is_install_id(id)) else {
        return Err(refuse("invalid_request", "install_id must be 16 to 64 characters of A-Z, a-z, 0-9, - and _"));
    };
    let Some(platform) = param("platform").and_then(Platform::from_stored) else {
        return Err(refuse("invalid_request", "platform must be ios, ipados, android, macos, windows or linux"));
    };
    if param("scope").is_some_and(|scope| scope != SCOPE_APP) {
        return Err(refuse("invalid_scope", "this app's scope is app"));
    }
    Ok(AuthorizeRequest {
        client_id: client_id.to_owned(),
        client_name: client_name.to_owned(),
        redirect_uri,
        state,
        code_challenge: code_challenge.to_owned(),
        app: Some(AppRequest { install_id: install_id.to_owned(), platform }),
    })
}

/// Where a redirect URI leads, for the consent page: its full origin
/// (scheme, host and any port), or the scheme and host of an app link.
/// Registration checks neither the client's name nor its redirect, so this
/// is the one thing the person can judge the app by.
pub fn redirect_origin(redirect_uri: &str) -> String {
    let Ok(url) = Url::parse(redirect_uri) else {
        return redirect_uri.to_owned();
    };
    match (url.scheme(), url.host_str()) {
        ("https" | "http", Some(_)) => url.origin().ascii_serialization(),
        (scheme, Some(host)) if !host.is_empty() => format!("{scheme}://{host}"),
        (scheme, _) => format!("{scheme}:"),
    }
}

/// `redirect_uri` with `params` added to its query.
pub fn redirect_with(redirect_uri: &str, params: &[(&str, &str)]) -> String {
    let mut url = Url::parse(redirect_uri).expect("checked redirect URI");
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in params {
            query.append_pair(key, value);
        }
    }
    url.into()
}

pub fn problem_redirect(problem: &AuthorizeProblem, issuer: &str) -> Option<String> {
    let AuthorizeProblem::Redirect { redirect_uri, state, error, description } = problem else {
        return None;
    };
    let mut params = vec![("error", *error), ("error_description", *description), ("iss", issuer)];
    if let Some(state) = state {
        params.push(("state", state));
    }
    Some(redirect_with(redirect_uri, &params))
}

/// The person approved: store a one-time code and send them back. `None`
/// when the account was banned meanwhile: it is locked and checked at the
/// moment of writing.
async fn approve(
    db: &PgPool,
    request: &AuthorizeRequest,
    account_id: Uuid,
    issuer: &str,
) -> sqlx::Result<Option<String>> {
    let mut tx = db.begin().await?;
    if oauth::banned_now(&mut tx, account_id).await? {
        return Ok(None);
    }
    let code = crypto::new_token("bwc_");
    sqlx::query(
        "INSERT INTO oauth_codes
             (code_hash, client_id, account_id, redirect_uri, code_challenge, scope, expires_at, install_id, platform)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(crypto::hash(&code))
    .bind(&request.client_id)
    .bind(account_id)
    .bind(&request.redirect_uri)
    .bind(&request.code_challenge)
    .bind(request.scope())
    .bind(OffsetDateTime::now_utc() + CODE_TTL)
    .bind(request.app.as_ref().map(|app| app.install_id.as_str()))
    .bind(request.app.as_ref().map(|app| app.platform.as_str()))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let mut params = vec![("code", code.as_str()), ("iss", issuer)];
    if let Some(state) = &request.state {
        params.push(("state", state));
    }
    Ok(Some(redirect_with(&request.redirect_uri, &params)))
}

/// `POST /oauth/authorize/decision`: the consent form, and an app's
/// Continue or Cancel. Never shown in a frame.
pub async fn decide(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let mut response = decide_inner(&state, &headers, &form).await;
    crate::account::web::no_frames(response.headers_mut());
    response
}

async fn decide_inner(state: &AppState, headers: &HeaderMap, form: &HashMap<String, String>) -> Response {
    if let Err(refused) = crate::account::web::same_origin(state, headers) {
        return refused;
    }
    let account = match crate::account::web::session_account(state, headers).await {
        Ok(Some(account)) => account,
        Ok(None) => return (StatusCode::FORBIDDEN, "Sign in first.").into_response(),
        Err(error) => {
            tracing::error!(error = ?error, "consent failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let issuer = &state.config.account_origin;
    let request = match check_authorize(&state.db, &state.config, form).await {
        Ok(request) => request,
        Err(problem) => {
            return match problem_redirect(&problem, issuer) {
                Some(location) => Redirect::to(&location).into_response(),
                None => (StatusCode::BAD_REQUEST, "This sign-in request is not valid.").into_response(),
            };
        }
    };
    if form.get("decision").map(String::as_str) != Some("approve") {
        let mut params = vec![("error", "access_denied"), ("iss", issuer.as_str())];
        if let Some(state) = &request.state {
            params.push(("state", state));
        }
        return Redirect::to(&redirect_with(&request.redirect_uri, &params)).into_response();
    }
    match approve(&state.db, &request, account.id, issuer).await {
        Ok(None) => (StatusCode::FORBIDDEN, Notice::AccountBanned.message()).into_response(),
        Ok(Some(location)) => {
            let what = if request.app.is_some() { "app sign-in approved" } else { "agent approved" };
            tracing::info!(client_id = %request.client_id, account = %account.id, "{what}");
            Redirect::to(&location).into_response()
        }
        Err(error) => {
            tracing::error!(error = ?error, "consent failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// What redeeming a code gave: the tokens and, for an app, the install
/// it signed in, for the new-device mail.
pub struct Redeemed {
    pub tokens: TokenResponse,
    pub app: Option<SignedInApp>,
}

/// An app's sign-in made by a code.
pub struct SignedInApp {
    pub account_id: Uuid,
    pub grant_id: Uuid,
    pub platform: Option<Platform>,
    /// The address to mail about a new device, decided in the exchange's
    /// transaction: the account had no device row for the install then,
    /// and does not sign in with a fixed code. `None`: no mail.
    pub new_device_to: Option<String>,
}

/// The `authorization_code` grant: one use, ten minutes, same client and
/// redirect URI, and a verifier that hashes to the challenge. The grant
/// gets the scope stored with the code (`mcp` for an agent, `app` for an
/// app), and an app's install and platform.
///
/// Its transaction waits at most three seconds for a connection and for
/// each lock ([`app_sign_in::begin_waiting`]), as in-app sign-in's do;
/// then the answer is `503 temporarily_unavailable` and the code is not
/// used. `admin_emails` is `ADMIN_EMAILS`, for the fixed-code rule of the
/// new-device mail.
pub async fn redeem_code(
    db: &PgPool,
    admin_emails: &[String],
    client_id: &str,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<Redeemed, OAuthFailure> {
    match redeem_code_inner(db, admin_emails, client_id, code, redirect_uri, verifier).await {
        Err(OAuthFailure::Internal(error))
            if error.downcast_ref::<sqlx::Error>().is_some_and(app_sign_in::is_lock_timeout) =>
        {
            tracing::warn!("code exchange: a lock or a connection was not had in time; answered busy");
            Err(OAuthFailure::from(OAuthError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                error: "temporarily_unavailable",
                description: "Buwiz is busy; try again in a few seconds",
            }))
        }
        redeemed => redeemed,
    }
}

async fn redeem_code_inner(
    db: &PgPool,
    admin_emails: &[String],
    client_id: &str,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<Redeemed, OAuthFailure> {
    let invalid = || OAuthFailure::from(OAuthError::new("invalid_grant", "the code is not valid"));
    let mut tx = app_sign_in::begin_waiting(db).await?;
    // The account before the code row, in the order a ban takes them (it
    // locks the account, then ends the code rows). A code's account never
    // changes, so the unlocked read names the same one.
    let owner: Option<Uuid> = sqlx::query_scalar("SELECT account_id FROM oauth_codes WHERE code_hash = $1")
        .bind(crypto::hash(code))
        .fetch_optional(&mut *tx)
        .await?;
    let Some(owner) = owner else {
        return Err(invalid());
    };
    let banned = oauth::banned_now(&mut tx, owner).await?;
    #[derive(sqlx::FromRow)]
    struct Stored {
        client_id: String,
        account_id: Uuid,
        redirect_uri: String,
        code_challenge: String,
        scope: String,
        expires_at: OffsetDateTime,
        used_at: Option<OffsetDateTime>,
        install_id: Option<String>,
        platform: Option<String>,
    }
    let row: Option<Stored> = sqlx::query_as(
        "SELECT client_id, account_id, redirect_uri, code_challenge, scope, expires_at, used_at, install_id, platform
         FROM oauth_codes WHERE code_hash = $1 FOR UPDATE",
    )
    .bind(crypto::hash(code))
    .fetch_optional(&mut *tx)
    .await?;
    let Some(Stored {
        client_id: code_client,
        account_id,
        redirect_uri: code_redirect,
        code_challenge: challenge,
        scope,
        expires_at,
        used_at,
        install_id,
        platform,
    }) = row
    else {
        return Err(invalid());
    };
    sqlx::query("UPDATE oauth_codes SET used_at = now() WHERE code_hash = $1")
        .bind(crypto::hash(code))
        .execute(&mut *tx)
        .await?;
    let verifier_ok = (43..=128).contains(&verifier.len())
        && URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) == challenge;
    let valid = used_at.is_none()
        && account_id == owner
        && expires_at > OffsetDateTime::now_utc()
        && code_client == client_id
        && code_redirect == redirect_uri
        && verifier_ok;
    if !valid {
        tx.commit().await?;
        return Err(invalid());
    }
    // No new sign-in for a banned account, whatever was approved.
    if banned {
        tx.commit().await?;
        return Err(OAuthFailure::from(OAuthError::new("access_denied", "this account is banned")));
    }
    let from = oauth::GrantFrom { install_id: install_id.as_deref(), platform: platform.as_deref() };
    let (grant_id, tokens) = oauth::issue_grant_with_id(&mut tx, account_id, client_id, &scope, from).await?;
    let app = match (scope.as_str(), install_id.as_deref()) {
        (SCOPE_APP, Some(install_id)) => Some(SignedInApp {
            account_id,
            grant_id,
            platform: platform.as_deref().and_then(Platform::from_stored),
            new_device_to: new_device_to(&mut tx, account_id, install_id, admin_emails).await?,
        }),
        _ => None,
    };
    tx.commit().await?;
    Ok(Redeemed { tokens, app })
}

/// Whom to mail about a sign-in from `install_id`, in the exchange's
/// transaction: the account's address when it has no device row for the
/// install at this moment and does not sign in with a fixed code (a store
/// reviewer's). Decided here, not on the mail's task, so an app that
/// registers its device at once still has its new device mailed.
async fn new_device_to(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account_id: Uuid,
    install_id: &str,
    admin_emails: &[String],
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(&format!(
        "SELECT a.email FROM accounts a
         WHERE a.id = $1 AND NOT {}
           AND NOT EXISTS (SELECT 1 FROM devices d WHERE d.account_id = a.id AND d.install_id = $2)",
        crate::account::roles::fixed_code_sql("a", "$3")
    ))
    .bind(account_id)
    .bind(install_id)
    .bind(admin_emails)
    .fetch_optional(&mut **tx)
    .await
}

/// Mails "New sign-in to eBIRForms on …", with when, and where to log it
/// out, to the address the exchange decided on ([`new_device_to`]); none,
/// no mail. Runs on its own task after the exchange committed, so the
/// token answer never waits for it, and reads nothing from the database.
/// A mail that fails is logged, with ids only, and the sign-in stands.
pub fn mail_new_device(state: &AppState, app: SignedInApp) {
    let state = state.clone();
    tokio::spawn(async move {
        state.mailer.new_device_gate().await;
        new_device_mail(&state, &app).await;
        state.mailer.new_device_checked();
    });
}

async fn new_device_mail(state: &AppState, app: &SignedInApp) {
    let at = OffsetDateTime::now_utc();
    let Some(to) = &app.new_device_to else {
        return;
    };
    let device = Platform::device(app.platform);
    let tins = format!("{}/tins", state.config.account_origin);
    if state.mailer.send_new_device(to, device, at, &tins).await.is_err() {
        tracing::error!(account = %app.account_id, grant = %app.grant_id, "the new-device mail could not be sent");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_uris_are_https_loopback_or_app_schemes() {
        for ok in [
            "https://claude.ai/api/mcp/auth_callback",
            "http://localhost:33418/callback",
            "http://127.0.0.1/cb",
            "cursor://anysphere.cursor-retrieval/oauth/callback",
            "com.example.agent:/oauth",
        ] {
            assert!(redirect_uri_allowed(ok), "{ok}");
        }
        for bad in [
            "http://evil.example/cb",
            "https://claude.ai/cb#frag",
            "javascript:alert(1)",
            "data:text/html,hi",
            "https://user:pw@claude.ai/cb",
            "not a url",
        ] {
            assert!(!redirect_uri_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn consent_shows_the_full_redirect_origin() {
        assert_eq!(redirect_origin("https://mallory.example/cb?x=1"), "https://mallory.example");
        assert_eq!(
            redirect_origin("https://claude.ai.mallory.example:8443/cb"),
            "https://claude.ai.mallory.example:8443"
        );
        assert_eq!(redirect_origin("http://127.0.0.1:33418/callback"), "http://127.0.0.1:33418");
        assert_eq!(
            redirect_origin("cursor://anysphere.cursor-retrieval/oauth/callback"),
            "cursor://anysphere.cursor-retrieval"
        );
        assert_eq!(redirect_origin("com.example.agent:/oauth"), "com.example.agent:");
    }

    #[test]
    fn loopback_redirects_may_change_port_others_must_match() {
        assert!(redirect_uri_matches("http://127.0.0.1:1000/cb", "http://127.0.0.1:2000/cb"));
        assert!(!redirect_uri_matches("http://127.0.0.1:1000/cb", "http://127.0.0.1:2000/other"));
        assert!(!redirect_uri_matches("https://a.example/cb", "https://a.example:8443/cb"));
        assert!(redirect_uri_matches("https://a.example/cb", "https://a.example/cb"));
    }

    #[test]
    fn only_buwiz_desktop_has_app_redirects_and_they_are_strict() {
        assert!(app_redirects("buwiz-ebirforms").is_empty() && app_redirects("buwiz-books").is_empty());
        assert!(app_redirects("agent_x").is_empty());
        let ok = |uri: &str| app_redirects("buwiz-desktop").iter().any(|known| known.matches(uri));
        for good in ["ebirforms://signin-callback", "http://127.0.0.1:1/callback", "http://127.0.0.1:65535/callback"] {
            assert!(ok(good), "{good}");
        }
        for bad in [
            "ebirforms://signin-callback/",
            "http://127.0.0.1:080/callback",
            "http://127.0.0.1:0/callback",
            "http://127.0.0.1:99999/callback",
            "http://127.0.0.1:+80/callback",
            "http://127.0.0.1:80/callback?",
            "http://127.0.0.1:80/callback#",
        ] {
            assert!(!ok(bad), "{bad}");
        }
        assert!(!redirect_uri_allowed("ebirforms://signin-callback"));
    }

    #[test]
    fn no_frames_adds_to_a_policy_already_there() {
        let mut headers = HeaderMap::new();
        crate::account::web::no_frames(&mut headers);
        assert_eq!(headers["content-security-policy"], "frame-ancestors 'none'");
        assert_eq!(headers["x-frame-options"], "DENY");
        let mut headers = HeaderMap::new();
        headers.insert("content-security-policy", "default-src 'self';".parse().unwrap());
        crate::account::web::no_frames(&mut headers);
        assert_eq!(headers["content-security-policy"], "default-src 'self'; frame-ancestors 'none'");
        let mut headers = HeaderMap::new();
        headers.insert("content-security-policy", "frame-ancestors 'none'; img-src 'self'".parse().unwrap());
        crate::account::web::no_frames(&mut headers);
        assert_eq!(headers["content-security-policy"], "frame-ancestors 'none'; img-src 'self'");
    }

    #[test]
    fn redirects_keep_existing_query_and_encode_values() {
        assert_eq!(
            redirect_with("https://a.example/cb?x=1", &[("code", "c d"), ("state", "s&t")]),
            "https://a.example/cb?x=1&code=c+d&state=s%26t"
        );
    }
}
