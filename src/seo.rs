//! What search engines and browsers read about the site: `robots.txt` and
//! the sitemap, which depend on the host (see [`crate::canonical`]), and the
//! web app manifest.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use serde_json::json;

use crate::canonical::{Site, is_app_host};
use crate::config::Config;
use crate::downloads::Release;
use crate::state::AppState;

/// Marketing pages in the sitemap: (path, last changed). Add new public
/// pages here.
const PAGES: &[(&str, &str)] = &[("/", "2026-10-30")];

/// The Bot site's pages in its sitemap, likewise.
const BOT_PAGES: &[(&str, &str)] = &[("/", "2026-11-01"), ("/mini-apps", "2026-11-01")];

/// Signed-in, sign-in and API paths, kept out of search results.
const PRIVATE_PATHS: &[&str] =
    &["/tins", "/staff", "/login", "/signup", "/device", "/oauth", "/api", "/auth", "/me", "/mcp"];

/// AI search and answer crawlers, named so they are let in on purpose.
const AI_SEARCH_CRAWLERS: &[&str] =
    &["OAI-SearchBot", "ChatGPT-User", "PerplexityBot", "Perplexity-User", "Claude-SearchBot", "Claude-User"];

pub async fn robots(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let host = host(&headers);
    text(robots_txt(host, &public_origin(&state.config, host)), "text/plain; charset=utf-8")
}

pub async fn sitemap(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let host = host(&headers);
    if is_app_host(host) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let body = match Site::for_host(host, state.config.site_split) {
        Site::Bot => sitemap_xml(&public_origin(&state.config, host), BOT_PAGES, None),
        _ => sitemap_xml(&state.config.forms_origin, PAGES, state.downloads.latest().await.as_deref()),
    };
    text(body, "application/xml; charset=utf-8")
}

/// The public address of the site on `host`: the Bot host's own, or else
/// the filing site's, whichever host a request came in on.
pub fn public_origin(config: &Config, host: &str) -> String {
    match Site::for_host(host, config.site_split) {
        Site::Bot => format!("https://{host}"),
        _ => config.forms_origin.clone(),
    }
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

/// `origin` is the [`public_origin`] whose sitemap is named.
fn robots_txt(host: &str, origin: &str) -> String {
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
    body.push_str(&format!("Sitemap: {origin}/sitemap.xml\n"));
    body
}

/// `pages` at `origin`, and the download page once a version is released.
fn sitemap_xml(origin: &str, pages: &[(&str, &str)], release: Option<&Release>) -> String {
    let download = release.map(|release| {
        let lastmod = release.published_at.map_or_else(|| pages[0].1.to_owned(), |at| at.date().to_string());
        ("/download", lastmod)
    });
    let urls: String = pages
        .iter()
        .map(|(path, lastmod)| (*path, (*lastmod).to_owned()))
        .chain(download)
        .map(|(path, lastmod)| {
            format!("  <url>\n    <loc>{origin}{path}</loc>\n    <lastmod>{lastmod}</lastmod>\n  </url>\n")
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
        let robots = robots_txt("buwiz.com", "https://buwiz.com");
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
            assert_eq!(robots_txt(host, "https://buwiz.com"), "User-agent: *\nDisallow: /\n", "{host}");
        }
    }

    #[test]
    fn the_sitemap_lists_the_home_page() {
        let sitemap = sitemap_xml("https://buwiz.com", PAGES, None);
        assert!(sitemap.contains("<loc>https://buwiz.com/</loc>\n    <lastmod>2026-10-30</lastmod>"), "{sitemap}");
        assert!(!sitemap.contains("/download"), "{sitemap}");
    }

    #[test]
    fn the_sitemap_lists_the_download_page_once_released() {
        let manifest = br#"{"version":"1.2.0","published_at":"2026-11-02T01:00:00Z","files":[{"platform":"android","arch":"universal","kind":"apk","file":"Buwiz.apk","size":1,"sha256":"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"}]}"#;
        let release = Release::parse(manifest, "https://downloads.buwiz.com").unwrap();
        let sitemap = sitemap_xml("https://buwiz.com", PAGES, Some(&release));
        assert!(
            sitemap.contains("<loc>https://buwiz.com/download</loc>\n    <lastmod>2026-11-02</lastmod>"),
            "{sitemap}"
        );
    }

    #[test]
    fn the_bot_site_lists_its_pages() {
        let robots = robots_txt("buwiz.com", "https://buwiz.com");
        assert!(robots.starts_with("User-agent: *\nAllow: /\n"));
        assert!(robots.ends_with("Sitemap: https://buwiz.com/sitemap.xml\n"));
        let sitemap = sitemap_xml("https://buwiz.com", BOT_PAGES, None);
        assert_eq!(sitemap.matches("<loc>").count(), 2, "{sitemap}");
        assert!(sitemap.contains("<loc>https://buwiz.com/</loc>\n    <lastmod>2026-11-01</lastmod>"), "{sitemap}");
        assert!(sitemap.contains("<loc>https://buwiz.com/mini-apps</loc>"), "{sitemap}");
    }
}
