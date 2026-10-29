//! OAuth for AI agents, as the MCP authorization spec expects: discovery
//! metadata (RFC 9728, RFC 8414), dynamic client registration (RFC 7591)
//! and the authorization code flow with PKCE (S256 only).
//!
//! Agents are public clients. The person approves each one on the account
//! site. Their tokens carry the `mcp` scope: they read through `/mcp` but
//! can never file returns.

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

use crate::config::Config;
use crate::crypto;
use crate::error::{OAuthError, OAuthFailure};
use crate::oauth::{self, SCOPE_MCP, TokenResponse};
use crate::state::AppState;

const CODE_TTL: Duration = Duration::minutes(10);
const MAX_REDIRECT_URIS: usize = 5;

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

/// `POST /oauth/register` (RFC 7591). Public clients only.
pub async fn register(State(state): State<AppState>, Json(request): Json<Registration>) -> Response {
    if request.redirect_uris.is_empty() || request.redirect_uris.len() > MAX_REDIRECT_URIS {
        return registration_error("invalid_redirect_uri", "register 1 to 5 redirect_uris");
    }
    if !request.redirect_uris.iter().all(|uri| uri.len() <= 2000 && redirect_uri_allowed(uri)) {
        return registration_error(
            "invalid_redirect_uri",
            "redirect_uris must be https, http on localhost, or an app scheme, without a fragment",
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
    })
}

/// Where a redirect URI leads, for the consent page: its host, or its
/// scheme for an app.
pub fn redirect_host(redirect_uri: &str) -> String {
    match Url::parse(redirect_uri) {
        Ok(url) => url.host_str().map_or_else(|| format!("the {} app", url.scheme()), str::to_owned),
        Err(_) => redirect_uri.to_owned(),
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

/// The person approved: store a one-time code and send them back.
async fn approve(db: &PgPool, request: &AuthorizeRequest, account_id: Uuid, issuer: &str) -> sqlx::Result<String> {
    let code = crypto::new_token("bwc_");
    sqlx::query(
        "INSERT INTO oauth_codes (code_hash, client_id, account_id, redirect_uri, code_challenge, scope, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(crypto::hash(&code))
    .bind(&request.client_id)
    .bind(account_id)
    .bind(&request.redirect_uri)
    .bind(&request.code_challenge)
    .bind(SCOPE_MCP)
    .bind(OffsetDateTime::now_utc() + CODE_TTL)
    .execute(db)
    .await?;
    let mut params = vec![("code", code.as_str()), ("iss", issuer)];
    if let Some(state) = &request.state {
        params.push(("state", state));
    }
    Ok(redirect_with(&request.redirect_uri, &params))
}

/// `POST /oauth/authorize/decision`: the consent form.
pub async fn decide(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    if let Err(refused) = crate::web::same_origin(&state, &headers) {
        return refused;
    }
    let account = match crate::web::session_account(&state, &headers).await {
        Ok(Some(account)) => account,
        Ok(None) => return (StatusCode::FORBIDDEN, "Sign in first.").into_response(),
        Err(error) => {
            tracing::error!(error = ?error, "consent failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let issuer = &state.config.account_origin;
    let request = match check_authorize(&state.db, &state.config, &form).await {
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
        Ok(location) => {
            tracing::info!(client_id = %request.client_id, account = %account.id, "agent approved");
            Redirect::to(&location).into_response()
        }
        Err(error) => {
            tracing::error!(error = ?error, "consent failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// The `authorization_code` grant: one use, ten minutes, same client and
/// redirect URI, and a verifier that hashes to the challenge.
pub async fn redeem_code(
    db: &PgPool,
    client_id: &str,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<TokenResponse, OAuthFailure> {
    let invalid = || OAuthFailure::from(OAuthError::new("invalid_grant", "the code is not valid"));
    let mut tx = db.begin().await?;
    let row: Option<(String, Uuid, String, String, OffsetDateTime, Option<OffsetDateTime>)> = sqlx::query_as(
        "SELECT client_id, account_id, redirect_uri, code_challenge, expires_at, used_at
         FROM oauth_codes WHERE code_hash = $1 FOR UPDATE",
    )
    .bind(crypto::hash(code))
    .fetch_optional(&mut *tx)
    .await?;
    let Some((code_client, account_id, code_redirect, challenge, expires_at, used_at)) = row else {
        return Err(invalid());
    };
    sqlx::query("UPDATE oauth_codes SET used_at = now() WHERE code_hash = $1")
        .bind(crypto::hash(code))
        .execute(&mut *tx)
        .await?;
    let verifier_ok = (43..=128).contains(&verifier.len())
        && URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) == challenge;
    let valid = used_at.is_none()
        && expires_at > OffsetDateTime::now_utc()
        && code_client == client_id
        && code_redirect == redirect_uri
        && verifier_ok;
    if !valid {
        tx.commit().await?;
        return Err(invalid());
    }
    let tokens = oauth::issue_grant(&mut tx, account_id, client_id, SCOPE_MCP).await?;
    tx.commit().await?;
    Ok(tokens)
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
    fn loopback_redirects_may_change_port_others_must_match() {
        assert!(redirect_uri_matches("http://127.0.0.1:1000/cb", "http://127.0.0.1:2000/cb"));
        assert!(!redirect_uri_matches("http://127.0.0.1:1000/cb", "http://127.0.0.1:2000/other"));
        assert!(!redirect_uri_matches("https://a.example/cb", "https://a.example:8443/cb"));
        assert!(redirect_uri_matches("https://a.example/cb", "https://a.example/cb"));
    }

    #[test]
    fn redirects_keep_existing_query_and_encode_values() {
        assert_eq!(
            redirect_with("https://a.example/cb?x=1", &[("code", "c d"), ("state", "s&t")]),
            "https://a.example/cb?x=1&code=c+d&state=s%26t"
        );
    }
}
