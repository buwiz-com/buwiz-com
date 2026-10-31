//! Which host serves what. `www.` hosts redirect to the bare domain:
//! www.buwiz.com/x goes to https://buwiz.com/x, so every page has one
//! address. The home page lives on the bare domain; the account host's root
//! is the signed-in dashboard, and the API host's root says what it is.
//! Only the bare domain is for search engines: everything on the account
//! and API hosts is sent with `X-Robots-Tag: noindex`.
//!
//! With `SITE_SPLIT=true` the bare domain is the Buwiz Bot site instead, and
//! the filing site lives on its own host (`FORMS_ORIGIN`); see [`Site`].

use std::sync::Arc;

use axum::Json;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use http::{HeaderValue, Method, StatusCode, header};
use serde_json::json;

use crate::state::AppState;

/// The site a host serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Site {
    /// The Buwiz Bot page, on the bare domain once the site is split.
    Bot,
    /// The filing (eBIRForms) site.
    Forms,
    Account,
    Api,
}

impl Site {
    /// Told apart by the first label. Until `split`, every other host is
    /// the filing site, as before.
    pub fn for_host(host: &str, split: bool) -> Self {
        let host = host.to_ascii_lowercase();
        match host.split_once('.').map(|(label, _)| label) {
            Some("account") => Site::Account,
            Some("api") => Site::Api,
            Some("forms" | "ebirforms") => Site::Forms,
            _ if split => Site::Bot,
            _ => Site::Forms,
        }
    }
}

pub async fn bare_domain(request: Request, next: Next) -> Response {
    let host = request.headers().get(header::HOST).and_then(|value| value.to_str().ok());
    let target = host.and_then(|host| bare_domain_url(host, request.uri().path_and_query().map(|pq| pq.as_str())));
    match target {
        Some(target) => Redirect::permanent(&target).into_response(),
        None => next.run(request).await,
    }
}

/// Where a request to `host` should go instead, or `None` to serve it.
fn bare_domain_url(host: &str, path_and_query: Option<&str>) -> Option<String> {
    let bare = host.strip_prefix("www.").filter(|bare| bare.contains('.'))?;
    Some(format!("https://{bare}{}", path_and_query.unwrap_or("/")))
}

/// Serves `/` on the account and API hosts; every other request passes on.
/// The state is the account origin, e.g. `https://account.buwiz.com`.
pub async fn host_roots(State(account_origin): State<Arc<str>>, request: Request, next: Next) -> Response {
    let host = request.headers().get(header::HOST).and_then(|value| value.to_str().ok()).unwrap_or_default();
    match root_for(host, request.uri().path()) {
        Some(Root::Account) => Redirect::to("/account").into_response(),
        Some(Root::Api) => Json(json!({
            "service": "Buwiz API",
            "sign_in": format!("{account_origin}/login"),
        }))
        .into_response(),
        None => next.run(request).await,
    }
}

/// On the Bot site, sends the filing site's pages to [`Config::forms_origin`]
/// and the sign-in pages to the account host, both with a 301. Everything
/// else, and every host while the site is not split, passes on.
///
/// [`Config::forms_origin`]: crate::config::Config::forms_origin
pub async fn split_site(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let config = &state.config;
    let host = request.headers().get(header::HOST).and_then(|value| value.to_str().ok()).unwrap_or_default();
    let reading = request.method() == Method::GET || request.method() == Method::HEAD;
    if !reading || Site::for_host(host, config.site_split) != Site::Bot {
        return next.run(request).await;
    }
    let target = match moved_to(request.uri().path()) {
        Some(Site::Forms) => &config.forms_origin,
        Some(_) => &config.account_origin,
        None => return next.run(request).await,
    };
    let path_and_query = request.uri().path_and_query().map_or("/", |pq| pq.as_str());
    let location = HeaderValue::from_str(&format!("{target}{path_and_query}")).expect("an origin and a request path");
    (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, location)]).into_response()
}

/// Which site a path on the Bot host belongs to, if it moved off it.
fn moved_to(path: &str) -> Option<Site> {
    let under = |prefix: &str| path == prefix || path.starts_with(&format!("{prefix}/"));
    if under("/download")
        || path == "/install-linux.sh"
        || path.starts_with("/assets/demo")
        || path.starts_with("/assets/screens/")
    {
        Some(Site::Forms)
    } else if path == "/login" || path == "/signup" || path == "/login/verify" || under("/tins") {
        Some(Site::Account)
    } else {
        None
    }
}

/// Asks search engines not to index anything on the account and API hosts.
pub async fn no_index(request: Request, next: Next) -> Response {
    let app_host = request.headers().get(header::HOST).and_then(|value| value.to_str().ok()).is_some_and(is_app_host);
    let mut response = next.run(request).await;
    if app_host {
        response.headers_mut().insert("x-robots-tag", HeaderValue::from_static("noindex"));
    }
    response
}

/// The account and API hosts, as opposed to the public site.
pub fn is_app_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host.starts_with("account.") || host.starts_with("api.")
}

#[derive(Debug, PartialEq)]
enum Root {
    Account,
    Api,
}

/// Hosts are told apart by their first label, so a local server on one
/// host keeps the home page at its root.
fn root_for(host: &str, path: &str) -> Option<Root> {
    if path != "/" {
        return None;
    }
    let host = host.to_ascii_lowercase();
    if host.starts_with("account.") {
        Some(Root::Account)
    } else if host.starts_with("api.") {
        Some(Root::Api)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn www_goes_to_the_bare_domain_keeping_the_path() {
        assert_eq!(
            bare_domain_url("www.buwiz.com", Some("/login?next=%2Faccount")).unwrap(),
            "https://buwiz.com/login?next=%2Faccount"
        );
        assert_eq!(bare_domain_url("www.buwiz.com", None).unwrap(), "https://buwiz.com/");
    }

    #[test]
    fn other_hosts_are_served() {
        for host in ["buwiz.com", "account.buwiz.com", "localhost:3008", "www.", "wwwbuwiz.com"] {
            assert_eq!(bare_domain_url(host, Some("/")), None, "{host}");
        }
    }

    #[test]
    fn the_account_and_api_roots_are_not_the_home_page() {
        assert_eq!(root_for("account.buwiz.com", "/"), Some(Root::Account));
        assert_eq!(root_for("Account.Buwiz.com", "/"), Some(Root::Account));
        assert_eq!(root_for("api.buwiz.com", "/"), Some(Root::Api));
        assert_eq!(root_for("buwiz.com", "/"), None);
        assert_eq!(root_for("account.buwiz.com", "/login"), None);
        assert_eq!(root_for("api.buwiz.com", "/oauth/device/code"), None);
        // A local server keeps the home page at its root.
        assert_eq!(root_for("localhost:3008", "/"), None);
    }

    #[test]
    fn hosts_name_their_site() {
        for split in [false, true] {
            assert_eq!(Site::for_host("account.buwiz.com", split), Site::Account);
            assert_eq!(Site::for_host("API.buwiz.com", split), Site::Api);
            assert_eq!(Site::for_host("ebirforms.buwiz.com", split), Site::Forms);
            assert_eq!(Site::for_host("forms.localhost:3008", split), Site::Forms);
        }
        // Until the split, every other host is the filing site.
        for host in ["buwiz.com", "localhost:3008", "", "accounting.buwiz.com"] {
            assert_eq!(Site::for_host(host, false), Site::Forms, "{host}");
            assert_eq!(Site::for_host(host, true), Site::Bot, "{host}");
        }
    }

    #[test]
    fn the_filing_and_sign_in_pages_move_off_the_bot_site() {
        for path in [
            "/download",
            "/download/mac",
            "/install-linux.sh",
            "/assets/demo.js",
            "/assets/demo.css",
            "/assets/screens/palette_dark.webp",
        ] {
            assert_eq!(moved_to(path), Some(Site::Forms), "{path}");
        }
        for path in ["/login", "/signup", "/login/verify", "/tins", "/tins/new", "/tins/claims"] {
            assert_eq!(moved_to(path), Some(Site::Account), "{path}");
        }
        for path in [
            "/",
            "/downloads",
            "/api/devices",
            "/mcp",
            "/oauth/authorize",
            "/.well-known/jwks.json",
            "/auth/email/start",
            "/device",
            "/assets/site.css",
            "/assets/brand/icon-512.png",
            "/robots.txt",
        ] {
            assert_eq!(moved_to(path), None, "{path}");
        }
    }

    #[test]
    fn only_the_account_and_api_hosts_are_app_hosts() {
        assert!(is_app_host("account.buwiz.com"));
        assert!(is_app_host("API.buwiz.com"));
        assert!(!is_app_host("buwiz.com"));
        assert!(!is_app_host("localhost:3008"));
    }
}
