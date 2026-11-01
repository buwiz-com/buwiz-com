//! The Buwiz Bot site, on the bare domain (see
//! [`crate::canonical::Site`]). It is the Leptos app in `bot-site/`, built
//! with Trunk into `bot-site/dist/` (`bot-site/build.sh`) and embedded in
//! the binary: `/` and the app's other [`PAGES`] are its `index.html`, and
//! its bundle and images are under `/bot/`, apart from the server's own
//! `/assets/`.

use axum::response::{IntoResponse, Response};
use http::{HeaderValue, StatusCode, header};

use crate::config::Config;
use crate::downloads::{Platform, Release};

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
        description: "Buwiz Bot is free and open source. Give your AI a job and a computer you host. Mini apps in the chat file BIR returns, keep the books and edit documents.",
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
/// host; its download meta from Buwiz Bot's `release` (see
/// [`download_meta`]) and its stars meta from `stars`, each empty while
/// unknown; its title, description and canonical address set for the page;
/// and what the app draws there already in its body, for crawlers that do
/// not run the app (the app removes it before it mounts).
pub fn index(config: &Config, path: &str, release: Option<&Release>, stars: Option<u64>) -> Response {
    let mut response = page_html(&config.forms_origin, path, release, stars).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// The HTML of [`index`].
fn page_html(forms_origin: &str, path: &str, release: Option<&Release>, stars: Option<u64>) -> String {
    let file = Dist::get("index.html").expect("bot-site/dist/index.html is built");
    let html = String::from_utf8_lossy(&file.data);
    let html = with_meta(&html, "buwiz-forms-origin", forms_origin);
    let html = with_meta(&html, "buwiz-bot-download", &download_meta(release));
    let stars = stars.map(|count| count.to_string()).unwrap_or_default();
    let mut html = with_meta(&html, "buwiz-bot-stars", &stars);
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

/// What the download button needs, as compact JSON: the version and, for
/// each desktop platform with an installer, the one `/download/{platform}`
/// would give ([`Release::pick`]), as its address in the bucket and its
/// size in bytes. For example
/// `{"mac":{"size":84213000,"url":"https://…/bot/releases/v0.4.0/Buwiz-Bot.dmg"},"version":"0.4.0"}`.
/// Empty while nothing is released.
fn download_meta(release: Option<&Release>) -> String {
    let Some(release) = release else {
        return String::new();
    };
    let mut meta = serde_json::Map::new();
    meta.insert("version".into(), release.version.clone().into());
    for platform in [Platform::Mac, Platform::Windows, Platform::Linux] {
        if let Some(file) = release.pick(platform, None, None) {
            meta.insert(platform.slug().into(), serde_json::json!({"url": file.url, "size": file.size}));
        }
    }
    serde_json::Value::Object(meta).to_string()
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
            let html = page_html("https://ebirforms.buwiz.com", page.path, None, None);
            assert!(html.contains(&format!("<title>{}</title>", page.title)), "{}", page.path);
            assert_eq!(html.matches(page.description).count(), 3, "description, og and twitter: {}", page.path);
            let canonical = format!(r#"<link rel="canonical" href="https://buwiz.com{}" />"#, page.path);
            assert!(html.contains(&canonical), "{}", page.path);
            let body = &html[html.find("<body>").unwrap()..];
            assert!(body.starts_with("<body>\n<div id=\"prerender\">"), "{}", page.path);
            assert_eq!(body.matches("<h1").count(), 1, "one h1: {}", page.path);
            assert!(body.contains("<footer") && !body.contains("<script"), "{}", page.path);
        }
        let mini = page_html("https://ebirforms.buwiz.com", "/mini-apps", None, None);
        assert!(!mini.contains(PAGES[0].title) && !mini.contains(PAGES[0].description));
    }

    const SHA: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    fn bot_release(files: &[(&str, &str, &str, &str, u64)]) -> Release {
        let files: Vec<_> = files
            .iter()
            .map(|(platform, arch, kind, file, size)| {
                serde_json::json!({"platform": platform, "arch": arch, "kind": kind, "file": file, "size": size, "sha256": SHA})
            })
            .collect();
        let json = serde_json::json!({"version": "0.4.0", "files": files}).to_string();
        Release::parse(json.as_bytes(), "https://downloads.buwiz.com/bot").unwrap()
    }

    #[test]
    fn nothing_released_and_no_stars_leave_both_metas_empty() {
        for page in PAGES {
            let html = page_html("https://ebirforms.buwiz.com", page.path, None, None);
            assert!(html.contains(r#"<meta name="buwiz-bot-download" content="" />"#), "{}", page.path);
            assert!(html.contains(r#"<meta name="buwiz-bot-stars" content="" />"#), "{}", page.path);
        }
    }

    #[test]
    fn a_release_and_the_stars_fill_the_metas() {
        let release = bot_release(&[
            ("macos", "arm64", "dmg", "Buwiz-Bot-0.4.0-arm64.dmg", 80_000_000),
            ("macos", "universal", "dmg", "Buwiz-Bot-0.4.0-universal.dmg", 84_213_000),
            ("windows", "x64", "msi", "Buwiz-Bot-0.4.0-x64.msi", 61_000_000),
            ("windows", "x64", "exe", "Buwiz-Bot-0.4.0-x64-setup.exe", 60_000_000),
            ("linux", "universal", "sh", "install.sh", 2_000),
            ("android", "universal", "apk", "Buwiz-Bot.apk", 30_000_000),
        ]);
        let meta = download_meta(Some(&release));
        let parsed: serde_json::Value = serde_json::from_str(&meta).unwrap();
        // The same defaults as the filing site's /download/{platform}; no
        // Linux installer (the script is never a default), no Android.
        assert_eq!(
            parsed,
            serde_json::json!({
                "version": "0.4.0",
                "mac": {"url": "https://downloads.buwiz.com/bot/releases/v0.4.0/Buwiz-Bot-0.4.0-universal.dmg", "size": 84_213_000},
                "windows": {"url": "https://downloads.buwiz.com/bot/releases/v0.4.0/Buwiz-Bot-0.4.0-x64-setup.exe", "size": 60_000_000},
            })
        );
        assert!(!meta.contains(' ') && !meta.contains('\n'), "compact: {meta}");

        let html = page_html("https://ebirforms.buwiz.com", "/", Some(&release), Some(128));
        let escaped = meta.replace('"', "&quot;");
        assert!(html.contains(&format!(r#"<meta name="buwiz-bot-download" content="{escaped}" />"#)), "{html}");
        assert!(html.contains(r#"<meta name="buwiz-bot-stars" content="128" />"#));
        assert!(!html.contains(r#"{"mac""#), "the JSON is escaped");
    }
}
