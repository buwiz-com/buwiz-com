//! What search engines and browsers read on each host.
//! Each test gets its own database from `#[sqlx::test]`, so DATABASE_URL
//! must point at a Postgres the tests may create databases in
//! (`docker compose up -d`, then the URL from `.env.example`).

mod common;

use axum::http::StatusCode;
use serde_json::Value;
use sqlx::PgPool;

use common::*;

#[sqlx::test(migrations = "./migrations")]
async fn robots_and_the_sitemap_follow_the_host(db: PgPool) {
    let site = Site::new(db);
    let public = text(site.on_host(FORMS_HOST, "/robots.txt").await).await;
    assert!(public.contains("User-agent: *\nAllow: /\nDisallow: /tins\n"), "{public}");
    assert!(public.contains("User-agent: Claude-SearchBot\n"), "{public}");
    assert!(public.contains("Sitemap: https://ebirforms.buwiz.com/sitemap.xml"), "{public}");
    for host in ["account.buwiz.com", "api.buwiz.com"] {
        assert_eq!(text(site.on_host(host, "/robots.txt").await).await, "User-agent: *\nDisallow: /\n", "{host}");
        assert_eq!(site.on_host(host, "/sitemap.xml").await.status(), StatusCode::NOT_FOUND, "{host}");
    }
    let sitemap = site.on_host(FORMS_HOST, "/sitemap.xml").await;
    assert_eq!(content_type(&sitemap), "application/xml; charset=utf-8");
    let sitemap = text(sitemap).await;
    assert!(sitemap.contains("<loc>https://ebirforms.buwiz.com/</loc>") && sitemap.contains("<lastmod>"), "{sitemap}");
}

#[sqlx::test(migrations = "./migrations")]
async fn the_account_and_api_hosts_ask_not_to_be_indexed(db: PgPool) {
    let site = Site::new(db);
    for (host, path) in
        [("account.buwiz.com", "/login"), ("account.buwiz.com", "/"), ("api.buwiz.com", "/"), ("api.buwiz.com", "/me")]
    {
        let response = site.on_host(host, path).await;
        assert_eq!(response.headers().get("x-robots-tag").unwrap(), "noindex", "{host}{path}");
    }
    for host in ["buwiz.com", FORMS_HOST] {
        for path in ["/", "/login", "/robots.txt"] {
            assert!(site.on_host(host, path).await.headers().get("x-robots-tag").is_none(), "{host}{path}");
        }
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn the_home_page_alone_is_described_for_search_and_sharing(db: PgPool) {
    let site = Site::new(db);
    let home = text(site.on_host(FORMS_HOST, "/").await).await;
    for tag in [
        "<title>Buwiz: Philippine tax filing, made modern</title>",
        r#"<link rel="canonical" href="https://ebirforms.buwiz.com/">"#,
        r#"<meta property="og:url" content="https://ebirforms.buwiz.com/">"#,
        r#"<meta property="og:image" content="https://ebirforms.buwiz.com/assets/brand/og-image.png">"#,
        r#"<meta property="og:type" content="website">"#,
        r#"<meta property="og:site_name" content="Buwiz">"#,
        r#"<meta property="og:locale" content="en_PH">"#,
        r#"<meta name="twitter:card" content="summary_large_image">"#,
        r#"<link rel="manifest" href="/site.webmanifest">"#,
        r#"<link rel="icon" href="/favicon.ico" sizes="48x48">"#,
    ] {
        assert!(home.contains(tag), "{tag}");
    }
    assert!(home.contains(r#"<meta name="description" content="Prepare and file your BIR returns"#));
    assert!(!home.contains("noindex"));
    let json_ld =
        home.split(r#"<script type="application/ld+json">"#).nth(1).unwrap().split("</script>").next().unwrap();
    let json_ld: Value = serde_json::from_str(json_ld).unwrap();
    assert_eq!(json_ld[0]["@type"], "Organization");
    assert_eq!(json_ld[0]["logo"], "https://ebirforms.buwiz.com/assets/brand/icon-512.png");
    assert_eq!(json_ld[1]["@type"], "WebSite");

    let login = text(site.get("/login", "").await).await;
    assert!(login.contains("<title>Sign in · Buwiz</title>"));
    assert!(login.contains(r#"<meta name="robots" content="noindex">"#));
    assert!(!login.contains("canonical") && !login.contains("og:title") && !login.contains("ld+json"));
}

#[sqlx::test(migrations = "./migrations")]
async fn icons_the_manifest_and_the_share_card_are_served(db: PgPool) {
    let site = Site::new(db);
    for (path, kind) in [
        ("/favicon.ico", "image/x-icon"),
        ("/assets/brand/icon-192.png", "image/png"),
        ("/assets/brand/icon-512.png", "image/png"),
        ("/assets/brand/icon-maskable-512.png", "image/png"),
        ("/assets/brand/apple-touch-icon.png", "image/png"),
        ("/assets/brand/og-image.png", "image/png"),
        ("/assets/brand/buwiz_wordmark.svg", "image/svg+xml"),
        ("/assets/brand/buwiz_wordmark_email.png", "image/png"),
    ] {
        let response = site.get(path, "").await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(content_type(&response), kind, "{path}");
    }
    let manifest = site.get("/site.webmanifest", "").await;
    assert_eq!(content_type(&manifest), "application/manifest+json");
    let manifest: Value = serde_json::from_slice(&bytes(manifest).await).unwrap();
    assert_eq!(manifest["name"], "Buwiz");
    assert_eq!(manifest["theme_color"], "#0f5b46");
    for icon in manifest["icons"].as_array().unwrap() {
        let src = icon["src"].as_str().unwrap();
        assert_eq!(site.get(src, "").await.status(), StatusCode::OK, "{src}");
    }
}
