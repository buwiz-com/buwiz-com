//! What search engines and browsers read about the site: `robots.txt` and
//! the sitemap, which depend on the host (see [`crate::canonical`]), and the
//! web app manifest.

use axum::response::{IntoResponse, Response};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use serde_json::json;

use crate::canonical::is_app_host;

/// The public site, whichever host a request came in on.
pub const SITE: &str = "https://buwiz.com";

/// Marketing pages in the sitemap: (path, last changed). Add new public
/// pages here.
const PAGES: &[(&str, &str)] = &[("/", "2026-10-30")];

/// Signed-in, sign-in and API paths, kept out of search results.
const PRIVATE_PATHS: &[&str] =
    &["/tins", "/staff", "/login", "/signup", "/device", "/oauth", "/api", "/auth", "/me", "/mcp"];

/// AI search and answer crawlers, named so they are let in on purpose.
const AI_SEARCH_CRAWLERS: &[&str] =
    &["OAI-SearchBot", "ChatGPT-User", "PerplexityBot", "Perplexity-User", "Claude-SearchBot", "Claude-User"];

pub async fn robots(headers: HeaderMap) -> Response {
    text(robots_txt(host(&headers)), "text/plain; charset=utf-8")
}

pub async fn sitemap(headers: HeaderMap) -> Response {
    if is_app_host(host(&headers)) {
        return StatusCode::NOT_FOUND.into_response();
    }
    text(sitemap_xml(), "application/xml; charset=utf-8")
}

pub async fn manifest() -> Response {
    let manifest = json!({
        "name": "Buwiz",
        "short_name": "Buwiz",
        "start_url": "/",
        "display": "standalone",
        "theme_color": "#0f5b46",
        "background_color": "#f7f6f2",
        "icons": [
            {"src": "/assets/brand/icon-192.png", "sizes": "192x192", "type": "image/png", "purpose": "any"},
            {"src": "/assets/brand/icon-512.png", "sizes": "512x512", "type": "image/png", "purpose": "any"},
            {"src": "/assets/brand/icon-maskable-512.png", "sizes": "512x512", "type": "image/png", "purpose": "maskable"},
        ],
    });
    text(manifest.to_string(), "application/manifest+json")
}

fn host(headers: &HeaderMap) -> &str {
    headers.get(header::HOST).and_then(|value| value.to_str().ok()).unwrap_or_default()
}

fn robots_txt(host: &str) -> String {
    if is_app_host(host) {
        return "User-agent: *\nDisallow: /\n".to_owned();
    }
    let rules: String = PRIVATE_PATHS.iter().map(|path| format!("Disallow: {path}\n")).collect();
    let mut body = format!("User-agent: *\nAllow: /\n{rules}\n");
    for crawler in AI_SEARCH_CRAWLERS {
        body.push_str(&format!("User-agent: {crawler}\nAllow: /\n{rules}\n"));
    }
    // Training crawlers (GPTBot, ClaudeBot, Google-Extended) fall under `*`
    // and are allowed for now. Whether to let them in is a business
    // decision to revisit.
    body.push_str(&format!("Sitemap: {SITE}/sitemap.xml\n"));
    body
}

fn sitemap_xml() -> String {
    let urls: String = PAGES
        .iter()
        .map(|(path, lastmod)| {
            format!("  <url>\n    <loc>{SITE}{path}</loc>\n    <lastmod>{lastmod}</lastmod>\n  </url>\n")
        })
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n{urls}</urlset>\n"
    )
}

fn text(body: String, content_type: &'static str) -> Response {
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=3600"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_public_site_keeps_private_paths_out_and_names_the_sitemap() {
        let robots = robots_txt("buwiz.com");
        assert!(robots.starts_with("User-agent: *\nAllow: /\n"));
        for path in PRIVATE_PATHS {
            assert_eq!(robots.matches(&format!("Disallow: {path}\n")).count(), 1 + AI_SEARCH_CRAWLERS.len(), "{path}");
        }
        for crawler in AI_SEARCH_CRAWLERS {
            assert!(robots.contains(&format!("User-agent: {crawler}\nAllow: /\n")), "{crawler}");
        }
        assert!(!robots.contains("Disallow: /\n"));
        assert!(robots.ends_with("Sitemap: https://buwiz.com/sitemap.xml\n"));
    }

    #[test]
    fn the_account_and_api_hosts_are_closed_to_crawlers() {
        for host in ["account.buwiz.com", "api.buwiz.com"] {
            assert_eq!(robots_txt(host), "User-agent: *\nDisallow: /\n", "{host}");
        }
    }

    #[test]
    fn the_sitemap_lists_the_home_page() {
        let sitemap = sitemap_xml();
        assert!(sitemap.contains("<loc>https://buwiz.com/</loc>\n    <lastmod>2026-10-30</lastmod>"), "{sitemap}");
    }
}
