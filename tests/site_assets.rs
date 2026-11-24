//! What the owner, filing and Bot pages load: everything from this server, and
//! the icons they name all in the icon font's subset.
//! Each test gets its own database from `#[sqlx::test]`, so DATABASE_URL
//! must point at a Postgres the tests may create databases in.

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use sqlx::PgPool;

use common::*;

const BOT_HOST: &str = "buwiz.com";

async fn fetch(site: &Site, host: &str, path: &str, cookie: Option<&str>) -> Response {
    let mut request = Request::get(path).header(header::HOST, host);
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    site.send(request.body(Body::empty()).unwrap()).await
}

/// The string literal that comes right after each `needle` in `text` (after
/// any whitespace), for every occurrence that has one.
fn quoted_after(text: &str, needle: &str) -> Vec<String> {
    text.match_indices(needle)
        .filter_map(|(at, _)| {
            let rest = text[at + needle.len()..].trim_start().strip_prefix('"')?;
            Some(rest[..rest.find('"')?].to_owned())
        })
        .collect()
}

/// Every double-quoted string on `line`.
fn literals(line: &str) -> Vec<&str> {
    line.split('"').skip(1).step_by(2).collect()
}

/// The text of `source` from the line that has `marker` to the line that is
/// a closing `}` or `];` in column 0 or 4.
fn block<'a>(source: &'a str, marker: &str) -> &'a str {
    let start = source.find(marker).unwrap_or_else(|| panic!("{marker} is gone from the source"));
    let rest = &source[start..];
    let end = ["\n}\n", "\n    }\n", "\n];\n"].iter().filter_map(|stop| rest.find(stop)).min().unwrap_or(rest.len());
    &rest[..end]
}

/// The value of attribute `name` in a start tag, if it has one.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let at = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    let rest = &tag[at..];
    Some(&rest[..rest.find('"')?])
}

/// The start tags of `html` named `name`, without the angle brackets.
fn tags<'a>(html: &'a str, name: &str) -> Vec<&'a str> {
    let open = format!("<{name} ");
    let mut found = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find(&open) {
        rest = &rest[at..];
        let end = rest.find('>').expect("a tag ends");
        found.push(&rest[1..end]);
        rest = &rest[end..];
    }
    found
}

/// This server's own address: a path from the root, a fragment, or inline data.
fn is_local(url: &str) -> bool {
    (url.starts_with('/') && !url.starts_with("//")) || url.starts_with('#') || url.starts_with("data:")
}

/// Every address the page itself makes the browser load: stylesheets, preloads
/// and preconnects, icons, scripts, images, frames, media, and `url()` in its
/// inline styles. (Links the reader may follow are not loaded.)
fn loaded(html: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for tag in tags(html, "link") {
        let rel = attribute(tag, "rel").unwrap_or_default();
        let loads = !matches!(rel, "canonical" | "alternate" | "author" | "license" | "next" | "prev");
        if loads && let Some(href) = attribute(tag, "href") {
            urls.push(href.to_owned());
        }
    }
    for name in ["script", "img", "source", "iframe", "video", "audio", "embed", "track", "input", "object"] {
        for tag in tags(html, name) {
            for attr in ["src", "data", "poster"] {
                if let Some(url) = attribute(tag, attr) {
                    urls.push(url.to_owned());
                }
            }
            if let Some(set) = attribute(tag, "srcset") {
                urls.extend(set.split(',').filter_map(|part| part.split_whitespace().next()).map(str::to_owned));
            }
        }
    }
    for style in quoted_after(html, " style=") {
        urls.extend(css_urls(&style));
    }
    urls
}

/// The addresses a stylesheet loads: `url(…)` and `@import`.
fn css_urls(css: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = css;
    while let Some(at) = rest.find("url(") {
        rest = rest[at + 4..].trim_start();
        // A quoted address may hold parentheses (an inline SVG).
        let end = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => rest[1..].find(quote).map_or(rest.len(), |close| close + 2),
            _ => rest.find(')').unwrap_or(rest.len()),
        };
        urls.push(rest[..end].trim().trim_matches(['"', '\'']).to_owned());
        rest = &rest[end..];
    }
    let mut rest = css;
    while let Some(at) = rest.find("@import") {
        rest = &rest[at + 7..];
        let end = rest.find(';').unwrap_or(rest.len());
        let target = rest[..end].trim();
        urls.push(target.trim_start_matches("url(").trim_end_matches(')').trim().trim_matches(['"', '\'']).to_owned());
    }
    urls
}

/// No Google host, and in a stylesheet or script no mention of Google at all.
/// (A page may name a product, as the Bot site's "Gmail, Google Drive" does.)
fn assert_no_google(what: &str, text: &str, page: bool) {
    let lower = text.to_ascii_lowercase();
    let words: &[&str] = if page { &["googleapis", "gstatic", "google.com"] } else { &["google", "gstatic"] };
    for word in words {
        assert!(!lower.contains(word), "{what} mentions {word}");
    }
}

fn icon_names(html: &str) -> BTreeSet<String> {
    // `<span class="icon" aria-hidden="true">badge</span>`, and the demo's
    // `<span class="material-symbols-outlined …">`.
    let mut names = BTreeSet::new();
    for tag in ["span"] {
        for (at, _) in html.match_indices(&format!("<{tag} class=\"")) {
            let rest = &html[at..];
            let end = rest.find('>').unwrap();
            let class = attribute(&rest[1..end], "class").unwrap_or_default();
            if class == "icon" || class.starts_with("material-symbols-outlined") {
                let text = &rest[end + 1..];
                names.insert(text[..text.find('<').unwrap()].trim().to_owned());
            }
        }
    }
    names
}

fn manifest(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn read(path: &str) -> String {
    std::fs::read_to_string(manifest(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// The icons in the subset: `src/assets/fonts/material-symbols-icons.txt`.
fn subset() -> BTreeSet<String> {
    read("src/assets/fonts/material-symbols-icons.txt")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// Claims in every state My TINs shows, for the cards' icons.
async fn tins(site: &Site, account: uuid::Uuid) {
    let states = [
        ("100000001", "unverified"),
        ("100000002", "pending"),
        ("100000003", "needs_fix"),
        ("100000004", "approved"),
        ("100000005", "rejected"),
        ("100000006", "withdrawn"),
        ("100000007", "removed"),
        ("100000008", "revoked"),
    ];
    for (root, status) in states {
        add_claim(&site.db, account, root, status).await;
    }
    sqlx::query("UPDATE tin_claims SET fix_reasons = ARRAY['blurry'] WHERE status = 'needs_fix'")
        .execute(&site.db)
        .await
        .unwrap();
    let paused = add_claim(&site.db, account, "100000009", "approved").await;
    sqlx::query("UPDATE tin_claims SET paused_at = now(), paused_by = $2, pause_reason = 'A name check' WHERE id = $1")
        .bind(paused)
        .bind(account)
        .execute(&site.db)
        .await
        .unwrap();
    // The test TIN, which only an admin or a seed may have approved.
    sqlx::query(
        "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status, decided_at, verified_by)
         VALUES ($1, 'personal', '000000000', '00000', 'Test', 'approved', now(), 'admin')",
    )
    .bind(account)
    .execute(&site.db)
    .await
    .unwrap();
}

/// The owner, filing and Bot pages, signed out and in: (host, path, cookie).
async fn main_pages(site: &Site) -> Vec<(&'static str, &'static str, Option<String>)> {
    let (account, cookie) = site.person("ana@example.test").await;
    tins(site, account).await;
    let mut pages: Vec<(&'static str, &'static str, Option<String>)> = Vec::new();
    for path in ["/login", "/signup", "/login/verify?email=ana%40example.test"] {
        pages.push((ACCOUNT_HOST, path, None));
    }
    for path in ["/", "/download", "/privacy"] {
        pages.push((FORMS_HOST, path, None));
    }
    for path in ["/", "/mini-apps"] {
        pages.push((BOT_HOST, path, None));
    }
    for path in ["/tins", "/tins/new"] {
        pages.push((ACCOUNT_HOST, path, Some(cookie.clone())));
    }
    for path in ["/", "/download"] {
        pages.push((FORMS_HOST, path, Some(cookie.clone())));
    }
    pages
}

#[sqlx::test(migrations = "./migrations")]
async fn the_pages_load_nothing_from_another_host(db: PgPool) {
    let site = Site::new(db);
    let mut sheets = BTreeSet::new();
    let mut scripts = BTreeSet::new();
    for (host, path, cookie) in main_pages(&site).await {
        let response = fetch(&site, host, path, cookie.as_deref()).await;
        assert_eq!(response.status(), StatusCode::OK, "{host}{path}");
        let page = text(response).await;
        let at = format!("{host}{path} (signed {})", if cookie.is_some() { "in" } else { "out" });
        // The privacy policy names Google's mail permission, as text; the
        // check below still finds nothing loaded from Google.
        let named =
            if path == "/privacy" { page.replace("<code>https://mail.google.com/</code>", "") } else { page.clone() };
        assert_no_google(&at, &named, true);
        for url in loaded(&page) {
            assert!(is_local(&url), "{at} loads {url}");
            if url.ends_with(".css") {
                sheets.insert((host, url.clone()));
            } else if url.ends_with(".js") {
                scripts.insert((host, url.clone()));
            }
        }
        let fonts = if host == BOT_HOST { "/assets/bot-fonts.css" } else { "/assets/fonts.css" };
        assert!(page.contains(&format!(r#"href="{fonts}""#)), "{at} has no {fonts}");
    }
    // Every stylesheet and script the pages name, and the rest the server
    // serves as a stylesheet.
    for path in ["/assets/site.css", "/assets/home.css", "/assets/account.css", "/assets/demo.css", "/assets/fonts.css"]
    {
        sheets.insert((ACCOUNT_HOST, path.to_owned()));
    }
    sheets.insert((ACCOUNT_HOST, "/assets/staff.css".to_owned()));
    sheets.insert((BOT_HOST, "/assets/bot-fonts.css".to_owned()));
    assert!(sheets.len() > 8 && !scripts.is_empty(), "{sheets:?} {scripts:?}");
    for (host, path) in &sheets {
        let response = fetch(&site, host, path, None).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(content_type(&response), "text/css; charset=utf-8", "{path}");
        let css = text(response).await;
        assert_no_google(path, &css, false);
        for url in css_urls(&css) {
            assert!(is_local(&url), "{path} loads {url}");
        }
    }
    for (host, path) in &scripts {
        let response = fetch(&site, host, path, None).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_no_google(path, &text(response).await, false);
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn the_font_files_are_served_with_a_long_cache(db: PgPool) {
    let site = Site::new(db);
    let mut files = BTreeSet::new();
    for (host, css) in [(ACCOUNT_HOST, "/assets/fonts.css"), (BOT_HOST, "/assets/bot-fonts.css")] {
        let css = text(fetch(&site, host, css, None).await).await;
        for family in ["Material Symbols Outlined", if host == BOT_HOST { "Manrope" } else { "Figtree" }] {
            assert!(css.contains(&format!("font-family: \"{family}\"")), "{host}: {family}");
        }
        files.extend(css_urls(&css).into_iter().map(|url| (host, url)));
    }
    // The faces the pages' stylesheets name.
    for family in ["Bricolage Grotesque", "Figtree", "IBM Plex Mono", "JetBrains Mono"] {
        let css = text(fetch(&site, ACCOUNT_HOST, "/assets/fonts.css", None).await).await;
        assert!(css.contains(&format!("font-family: \"{family}\"")), "{family}");
    }
    let css = text(fetch(&site, BOT_HOST, "/assets/bot-fonts.css", None).await).await;
    assert!(css.contains("font-family: \"Space Grotesk\""));
    assert!(files.len() > 15);
    for (host, url) in files {
        let response = fetch(&site, host, &url, None).await;
        assert_eq!(response.status(), StatusCode::OK, "{url}");
        assert_eq!(content_type(&response), "font/woff2", "{url}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "public, max-age=86400", "{url}");
        let body = bytes(response).await;
        assert!(body.starts_with(b"wOF2"), "{url} is not woff2");
    }
}

/// The icon names the templates and scripts of the non-staff pages put on a
/// page, found in their source (the staff pages draw their own, inline SVG).
fn icons_in_source() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut sources = Vec::new();
    for dir in ["src/account", "src/sites"] {
        let mut stack = vec![manifest(dir)];
        while let Some(path) = stack.pop() {
            if path.to_string_lossy().contains("staff") {
                continue;
            }
            if path.is_dir() {
                stack.extend(std::fs::read_dir(&path).unwrap().map(|entry| entry.unwrap().path()));
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                sources.push(std::fs::read_to_string(&path).unwrap());
            }
        }
    }
    sources.push(read("src/uptime/public.rs"));
    for source in &sources {
        // icon(name: "x"), icon_name: "x", icon: "x".
        for needle in ["icon(name: ", "icon_name: ", " icon: "] {
            found.extend(quoted_after(source, needle));
        }
        // icon(name: if … { "a" } else { "b" }), and the same for icon_name.
        for line in source.lines().filter(|line| line.contains("icon(name: if ") || line.contains("icon_name: if ")) {
            found.extend(quoted_after(line, "{"));
        }
    }
    let pages = read("src/account/pages.rs");
    // My TINs' status pills: (key, label, icon, tone).
    for line in block(&pages, "fn status_look").lines().filter(|line| line.contains("=>")) {
        let parts = literals(line);
        found.insert(parts[parts.len() - 2].to_owned());
    }
    // The BIR status page's lights: (class, icon, word).
    for line in block(&read("src/account/pages/bir_status.rs"), "fn look(").lines().filter(|line| line.contains("=>")) {
        found.insert(literals(line)[1].to_owned());
    }
    // One icon per kind of paper, one per kind of BIR trouble.
    for (file, marker) in [("src/account/claims.rs", "pub fn icon("), ("src/uptime/public.rs", "pub fn icon(self)")] {
        for line in block(&read(file), marker).lines().filter(|line| line.contains("=>")) {
            found.insert(literals(line).last().unwrap().to_string());
        }
    }
    // The home page's tables: the first of each row is the icon.
    let home = read("src/account/pages/home.rs");
    for marker in ["const HERO_STATS", "const AGENT_PROMISES"] {
        let mut first = false;
        for line in block(&home, marker).lines().skip(1) {
            let trimmed = line.trim();
            if trimmed == "(" || trimmed.starts_with("(\"") {
                first = true;
            }
            if first && let Some(name) = literals(trimmed).first() {
                found.insert((*name).to_owned());
                first = false;
            }
        }
    }
    // account.js and demo.js name theirs in calls.
    found.extend(quoted_after(&read("src/account/account.js"), "iconElement("));
    let demo = read("src/sites/demo.js");
    found.extend(quoted_after(&demo, "card("));
    found.extend(quoted_after(&demo, "approveCard(\n        r,"));
    for line in demo.lines() {
        // h("span", { class: "material-symbols-outlined …" }, "name")
        if line.contains("material-symbols-outlined")
            && let Some((_, tail)) = line.rsplit_once("}, \"")
        {
            found.insert(tail[..tail.find('"').unwrap()].to_owned());
        }
        // The two the checklists pick between with `?`.
        found.extend(literals(line).into_iter().filter(|name| name.starts_with("check_box")).map(str::to_owned));
    }
    found
}

#[test]
fn every_icon_the_templates_name_is_in_the_subset() {
    let subset = subset();
    let used = icons_in_source();
    // The scan finds the tables and calls it is written for.
    for known in
        ["badge", "check_circle", "hourglass_top", "picture_as_pdf", "extension", "check_box", "lock_person", "gavel"]
    {
        assert!(used.contains(known), "the scan no longer finds {known}: {used:?}");
    }
    assert!(used.len() > 60, "{used:?}");
    let missing: Vec<_> = used.iter().filter(|name| !subset.contains(*name)).collect();
    assert!(
        missing.is_empty(),
        "icons used but not in material-symbols-icons.txt (add them, then run scripts/subset-icons.py): {missing:?}"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn every_icon_on_a_rendered_page_is_in_the_subset(db: PgPool) {
    let site = Site::new(db);
    let subset = subset();
    let mut seen = BTreeSet::new();
    for (host, path, cookie) in main_pages(&site).await {
        if host == BOT_HOST {
            continue; // the Bot site loads the full icon font
        }
        let page = text(fetch(&site, host, path, cookie.as_deref()).await).await;
        for name in icon_names(&page) {
            assert!(subset.contains(&name), "{host}{path} shows the icon {name}, which is not in the subset");
            seen.insert(name);
        }
    }
    for known in ["badge", "science", "pause_circle", "lock", "download", "verified", "smart_toy"] {
        assert!(seen.contains(known), "no page showed {known}: {seen:?}");
    }
}

#[test]
fn the_subset_font_is_what_the_list_describes() {
    let subset = subset();
    assert!(subset.len() > 60);
    let font = std::fs::read(manifest("src/assets/fonts/material-symbols-outlined-subset.woff2")).unwrap();
    assert!(font.starts_with(b"wOF2"));
    // Far smaller than the full font, which the Bot site loads.
    let full = std::fs::metadata(manifest("src/assets/fonts/material-symbols-outlined.woff2")).unwrap().len();
    assert!((font.len() as u64) * 20 < full, "{} against {full}", font.len());
}
