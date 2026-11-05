//! The Bot site on the bare domain, against Postgres.
//! Each test gets its own database from `#[sqlx::test]`, so DATABASE_URL
//! must point at a Postgres the tests may create databases in
//! (`docker compose up -d`, then the URL from `.env.example`).

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use sqlx::PgPool;

use common::*;

#[sqlx::test(migrations = "./migrations")]
async fn the_bot_site_is_the_bare_domain(db: PgPool) {
    let site = Site::new(db);
    let page = site.on_host("buwiz.com", "/").await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()[header::CONTENT_TYPE], "text/html; charset=utf-8");
    let page = text(page).await;
    // The Bot site's own index.html (bot-site/dist), its origins filled in.
    for part in [
        "<title>Buwiz Bot: AI coworkers that work on a computer you host</title>",
        r#"<meta name="description" content="Buwiz Bot is free and open source. Give your AI a job and a computer you host."#,
        // Nothing released, and GitHub is not reachable from the tests.
        r#"<meta name="buwiz-bot-download" content="" />"#,
        r#"<meta name="buwiz-bot-stars" content="" />"#,
        r#""license": "https://www.apache.org/licenses/LICENSE-2.0""#,
        // Drawn already, for crawlers that do not run the app.
        r#"<div id="prerender">"#,
        "<h1",
        "<footer",
        r#"href="/mini-apps#build""#,
        "Build your own",
        "Build a mini app",
        r#"<link rel="canonical" href="https://buwiz.com/" />"#,
        r#"<meta name="buwiz-forms-origin" content="https://ebirforms.buwiz.com" />"#,
    ] {
        assert!(page.contains(part), "{part}: {page}");
    }
    // Sign-in belongs to the filing site; the Bot page doesn't link it.
    assert!(!page.contains("buwiz-account-origin"), "{page}");
    assert!(!page.contains("OpenGrok") && !page.contains("Goldcoders"), "{page}");
    assert!(!page.contains("og:image") && !page.contains("noindex"));
    let head = Request::head("/").header(header::HOST, "buwiz.com").body(Body::empty()).unwrap();
    assert_eq!(site.send(head).await.status(), StatusCode::OK);

    // Its bundle is under /bot/, clear of the server's /assets/.
    let wasm = page.split('"').find(|part| part.starts_with("/bot/") && part.ends_with("_bg.wasm")).unwrap();
    let response = site.on_host("buwiz.com", wasm).await;
    assert_eq!(response.status(), StatusCode::OK, "{wasm}");
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/wasm");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "public, max-age=31536000, immutable");
    assert!(bytes(response).await.starts_with(b"\0asm"));
    let script = page.split('"').find(|part| part.starts_with("/bot/") && part.ends_with(".js")).unwrap();
    let response = site.on_host("buwiz.com", script).await;
    assert_eq!(response.headers()[header::CONTENT_TYPE], "text/javascript; charset=utf-8", "{script}");
    let logo = site.on_host("buwiz.com", "/bot/assets/brand/buwiz_wordmark.svg").await;
    assert_eq!(logo.status(), StatusCode::OK);
    assert_eq!(logo.headers()[header::CONTENT_TYPE], "image/svg+xml");
    for missing in ["/bot/index.html", "/bot/nothing.js", "/bot/assets/bir-certificate.pdf"] {
        assert_eq!(site.on_host("buwiz.com", missing).await.status(), StatusCode::NOT_FOUND, "{missing}");
    }
    // Only the Bot host has it.
    assert_eq!(site.on_host("ebirforms.buwiz.com", wasm).await.status(), StatusCode::NOT_FOUND);

    let robots = text(site.on_host("buwiz.com", "/robots.txt").await).await;
    assert!(robots.starts_with("User-agent: *\nAllow: /\n"), "{robots}");
    assert!(robots.ends_with("Sitemap: https://buwiz.com/sitemap.xml\n"), "{robots}");
    let sitemap = text(site.on_host("buwiz.com", "/sitemap.xml").await).await;
    assert_eq!(sitemap.matches("<loc>").count(), 2, "{sitemap}");
    assert!(sitemap.contains("<loc>https://buwiz.com/</loc>"), "{sitemap}");
    assert!(sitemap.contains("<loc>https://buwiz.com/mini-apps</loc>"), "{sitemap}");
}

#[sqlx::test(migrations = "./migrations")]
async fn the_bot_site_serves_its_mini_apps_page(db: PgPool) {
    let site = Site::new(db);
    let home = text(site.on_host("buwiz.com", "/").await).await;
    let page = site.on_host("buwiz.com", "/mini-apps").await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()[header::CONTENT_TYPE], "text/html; charset=utf-8");
    let page = text(page).await;
    // The same app, named and addressed for the page.
    let wasm = |html: &str| html.split('"').find(|part| part.ends_with("_bg.wasm")).map(str::to_owned);
    assert_eq!(wasm(&page), wasm(&home));
    assert!(wasm(&page).is_some(), "{page}");
    for part in [
        "<title>Mini apps: eBIRForms, Books, Word and Spreadsheet | Buwiz Bot</title>",
        r#"<meta name="description" content="Buwiz mini apps run inside Buwiz Bot"#,
        r#"<div id="prerender">"#,
        r#"id="build""#,
        "Build your own mini app",
        "Coming soon",
        "submit your mini app to Buwiz",
        r#"<link rel="canonical" href="https://buwiz.com/mini-apps" />"#,
        r#"<meta name="buwiz-forms-origin" content="https://ebirforms.buwiz.com" />"#,
    ] {
        assert!(page.contains(part), "{part}: {page}");
    }
    assert!(!page.contains("AI coworkers that work on a computer you control."), "{page}");
    let head = Request::head("/mini-apps").header(header::HOST, "buwiz.com").body(Body::empty()).unwrap();
    assert_eq!(site.send(head).await.status(), StatusCode::OK);

    // Only those paths: nothing else is the app's.
    for path in ["/mini-apps/", "/mini-apps/books", "/minis"] {
        let response = site.on_host("buwiz.com", path).await;
        assert!(!text(response).await.contains("_bg.wasm"), "{path}");
    }
    // Not on the filing site.
    let response = site.on_host(FORMS_HOST, "/mini-apps").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(!text(response).await.contains("Buwiz Bot"));
}

#[sqlx::test(migrations = "./migrations")]
async fn the_bot_site_sends_filing_and_sign_in_pages_on(db: PgPool) {
    let site = Site::new(db);
    for (path, target) in [
        ("/download/mac?arch=arm64", format!("{FORMS}/download/mac?arch=arm64")),
        ("/download", format!("{FORMS}/download")),
        ("/install-linux.sh", format!("{FORMS}/install-linux.sh")),
        ("/assets/demo.js", format!("{FORMS}/assets/demo.js")),
        ("/assets/screens/palette_dark.webp", format!("{FORMS}/assets/screens/palette_dark.webp")),
        ("/login?next=%2Ftins", format!("{ORIGIN}/login?next=%2Ftins")),
        ("/signup", format!("{ORIGIN}/signup")),
        ("/login/verify?email=a%40b.test", format!("{ORIGIN}/login/verify?email=a%40b.test")),
        ("/tins", format!("{ORIGIN}/tins")),
        ("/tins/new", format!("{ORIGIN}/tins/new")),
    ] {
        for method in [Method::GET, Method::HEAD] {
            let request = Request::builder()
                .method(method.clone())
                .uri(path)
                .header(header::HOST, "buwiz.com")
                .body(Body::empty());
            let response = site.send(request.unwrap()).await;
            assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY, "{method} {path}");
            assert_eq!(location(&response), target, "{method} {path}");
        }
    }
    // Everything else is served here as before.
    for path in ["/assets/site.css", "/assets/brand/icon-512.png", "/site.webmanifest"] {
        assert_eq!(site.on_host("buwiz.com", path).await.status(), StatusCode::OK, "{path}");
    }
    for path in ["/.well-known/jwks.json", "/.well-known/oauth-authorization-server", "/me", "/device"] {
        assert_ne!(site.on_host("buwiz.com", path).await.status(), StatusCode::MOVED_PERMANENTLY, "{path}");
    }
    assert_eq!(location(&site.on_host("buwiz.com", "/account").await), "/tins");
    let sent = site
        .send(
            Request::post("/auth/email/start")
                .header(header::HOST, "buwiz.com")
                .header(header::ORIGIN, ORIGIN)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("email=ana%40example.test"))
                .unwrap(),
        )
        .await;
    assert_eq!(sent.status(), StatusCode::SEE_OTHER);
    assert!(location(&sent).starts_with("/login/verify"), "{}", location(&sent));
}
