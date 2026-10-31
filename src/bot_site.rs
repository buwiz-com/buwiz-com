//! The Buwiz Bot site, on the bare domain once the site is split (see
//! [`crate::canonical::Site`]). It is the Leptos app in `bot-site/`, built
//! with Trunk into `bot-site/dist/` (`bot-site/build.sh`) and embedded in
//! the binary: `/` is its `index.html`, and its bundle and images are under
//! `/bot/`, apart from the server's own `/assets/`.

use axum::response::{IntoResponse, Response};
use http::{HeaderValue, StatusCode, header};

use crate::config::Config;

#[derive(rust_embed::Embed)]
#[folder = "bot-site/dist/"]
struct Dist;

/// The page at `/`, its origin metas filled in from `config` so its
/// sign-in and eBIRForms links go to this server's hosts.
pub fn index(config: &Config) -> Response {
    let file = Dist::get("index.html").expect("bot-site/dist/index.html is built");
    let html = String::from_utf8_lossy(&file.data);
    let html = with_meta(&html, "buwiz-account-origin", &config.account_origin);
    let html = with_meta(&html, "buwiz-forms-origin", &config.forms_origin);
    let mut response = html.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// A file of the bundle, at `/bot/{path}`. Trunk hashes the names of the
/// files at the top of `dist/`, so those are cached for good.
pub fn file(path: &str) -> Response {
    let Some(file) = Dist::get(path).filter(|_| path != "index.html") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let cache = if path.contains('/') { "public, max-age=86400" } else { "public, max-age=31536000, immutable" };
    let mut response = file.data.into_owned().into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(path)));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    response
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map_or("", |(_, extension)| extension) {
        "wasm" => "application/wasm",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "html" => "text/html; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

/// `html` with the `content` of `<meta name="{name}" content="...">` set
/// to `value`.
fn with_meta(html: &str, name: &str, value: &str) -> String {
    let start = format!(r#"<meta name="{name}" content=""#);
    let Some(at) = html.find(&start).map(|at| at + start.len()) else {
        return html.to_owned();
    };
    let end = at + html[at..].find('"').unwrap_or(0);
    let value = value.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;");
    format!("{}{value}{}", &html[..at], &html[end..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_meta_gets_the_value() {
        let html = r#"<head><meta name="a" content="https://x.test" /><meta name="b" content="y" /></head>"#;
        assert_eq!(
            with_meta(html, "a", "http://localhost:3032"),
            r#"<head><meta name="a" content="http://localhost:3032" /><meta name="b" content="y" /></head>"#
        );
        assert_eq!(with_meta(html, "c", "z"), html);
        assert!(with_meta(html, "b", r#""><script>"#).contains(r#"content="&quot;&gt;&lt;script&gt;""#));
    }

    #[test]
    fn the_built_page_has_both_metas_and_its_bundle_under_bot() {
        let index = String::from_utf8(Dist::get("index.html").unwrap().data.into_owned()).unwrap();
        assert!(index.contains("<title>Meet Buwiz Bot | Buwiz</title>"));
        for name in ["buwiz-account-origin", "buwiz-forms-origin"] {
            assert!(index.contains(&format!(r#"<meta name="{name}" content=""#)), "{name}");
        }
        let wasm = Dist::iter().find(|path| path.ends_with("_bg.wasm")).expect("a wasm file");
        assert!(index.contains(&format!("/bot/{wasm}")), "{index}");
        assert!(!index.contains(r#"href="/assets/images"#) && !index.contains("sw.js"));
    }
}
