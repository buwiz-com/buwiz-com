//! Which host serves what. `www.` hosts redirect to the bare domain:
//! www.buwiz.com/x goes to https://buwiz.com/x, so every page has one
//! address. The home page lives on the bare domain; the account host's root
//! is the signed-in dashboard, and the API host's root says what it is.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use http::header;
use serde_json::json;

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
}
