//! The Buwiz Bot site, on the bare domain (see
//! [`crate::canonical::Site`]). It is the Leptos app in `bot-site/`, built
//! with Trunk into `bot-site/dist/` (`bot-site/build.sh`) and embedded in
//! the binary: `/` and the app's other [`PAGES`] are its `index.html`, and
//! its bundle and images are under `/bot/`, apart from the server's own
//! `/assets/`.

use axum::response::{IntoResponse, Response};
use http::{HeaderValue, StatusCode, header};

use crate::config::Config;

#[derive(rust_embed::Embed)]
#[folder = "bot-site/dist/"]
struct Dist;

/// One of the app's pages: its path, the title and description its HTML
/// head gets, and its prerendered body in `dist/prerender/` (see
/// `bot-site/prerender.py`, which lists the same pages).
pub struct Page {
    pub path: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    prerender: &'static str,
}

/// The app's pages, each served as its `index.html` with its own head and
/// prerendered body. The app routes them itself (bot-site/src/app.rs);
/// only these paths are its. The first is the home page, whose title and
/// description `index.html` carries.
pub const PAGES: &[Page] = &[
    Page {
        path: "/",
        title: "Buwiz Bot: AI coworkers that work on a computer you host",
        description: "Give your AI a job and a computer you host. Mini apps in the chat file BIR returns, keep the books and edit documents.",
        prerender: "prerender/home.html",
    },
    Page {
        path: "/mini-apps",
        title: "Mini apps: eBIRForms, Books, Word and Spreadsheet | Buwiz Bot",
        description: "Buwiz mini apps run inside Buwiz Bot, in the chat and full screen: eBIRForms, Books, Word and Spreadsheet.",
        prerender: "prerender/mini-apps.html",
    },
];

/// Whether `path` is one of the app's [`PAGES`].
pub fn is_page(path: &str) -> bool {
    PAGES.iter().any(|page| page.path == path)
}

/// The page at `path`, one of [`PAGES`]: index.html with its forms-origin
/// meta filled in from `config`, so its eBIRForms links go to this server's
/// host; its title, description and canonical address set for the page;
/// and what the app draws there already in its body, for crawlers that do
/// not run the app (the app removes it before it mounts).
pub fn index(config: &Config, path: &str) -> Response {
    let mut response = page_html(&config.forms_origin, path).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// The HTML of [`index`].
fn page_html(forms_origin: &str, path: &str) -> String {
    let file = Dist::get("index.html").expect("bot-site/dist/index.html is built");
    let html = String::from_utf8_lossy(&file.data);
    let mut html = with_meta(&html, "buwiz-forms-origin", forms_origin);
    let home = &PAGES[0];
    let page = PAGES.iter().find(|page| page.path == path).unwrap_or(home);
    if page.path != home.path {
        let url = format!("https://buwiz.com{}\"", page.path);
        html = html
            .replace(home.title, page.title)
            .replace(home.description, page.description)
            .replace("https://buwiz.com/\"", &url);
    }
    if let Some(body) = Dist::get(page.prerender) {
        let body = String::from_utf8_lossy(&body.data);
        html = html.replacen("<body>", &format!("<body>\n<div id=\"prerender\">{body}</div>"), 1);
    }
    html
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
    fn the_built_page_has_its_meta_and_its_bundle_under_bot() {
        let index = String::from_utf8(Dist::get("index.html").unwrap().data.into_owned()).unwrap();
        assert!(index.contains(&format!("<title>{}</title>", PAGES[0].title)));
        assert!(index.contains(&format!(r#"<meta name="description" content="{}" />"#, PAGES[0].description)));
        assert!(index.contains(r#"<meta name="buwiz-forms-origin" content=""#));
        assert!(!index.contains("buwiz-account-origin"));
        let wasm = Dist::iter().find(|path| path.ends_with("_bg.wasm")).expect("a wasm file");
        assert!(index.contains(&format!("/bot/{wasm}")), "{index}");
        assert!(!index.contains(r#"href="/assets/images"#) && !index.contains("sw.js"));
    }

    #[test]
    fn each_page_has_its_own_head_and_its_prerendered_body() {
        for page in PAGES {
            let html = page_html("https://ebirforms.buwiz.com", page.path);
            assert!(html.contains(&format!("<title>{}</title>", page.title)), "{}", page.path);
            assert_eq!(html.matches(page.description).count(), 3, "description, og and twitter: {}", page.path);
            let canonical = format!(r#"<link rel="canonical" href="https://buwiz.com{}" />"#, page.path);
            assert!(html.contains(&canonical), "{}", page.path);
            let body = &html[html.find("<body>").unwrap()..];
            assert!(body.starts_with("<body>\n<div id=\"prerender\">"), "{}", page.path);
            assert_eq!(body.matches("<h1").count(), 1, "one h1: {}", page.path);
            assert!(body.contains("<footer") && !body.contains("<script"), "{}", page.path);
        }
        let mini = page_html("https://ebirforms.buwiz.com", "/mini-apps");
        assert!(!mini.contains(PAGES[0].title) && !mini.contains(PAGES[0].description));
    }
}
