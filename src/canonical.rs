//! `www.` hosts redirect to the bare domain: www.buwiz.com/x goes to
//! https://buwiz.com/x, so every page has one address.

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use http::header;

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
}
