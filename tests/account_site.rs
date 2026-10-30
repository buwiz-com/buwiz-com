//! The account site against Postgres: the papers each kind of TIN needs,
//! sending a claim back and resending it, the staff pages, old links, the
//! licence gate's opt-in for open claims, signing an account out
//! everywhere, what search engines and browsers read on each host, and
//! ending claims and changing their files. Each test gets its own database from `#[sqlx::test]`, so
//! DATABASE_URL must point at a Postgres the tests may create databases in
//! (`docker compose up -d`, then the URL from `.env.example`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::{Body, Bytes};
use axum::http::{Method, Request, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use buwiz_cloud::config::{Config, MailConfig};
use buwiz_cloud::link::SenderLink;
use buwiz_cloud::state::AppState;
use buwiz_cloud::storage::{DocumentStore, ObjectStore};
use buwiz_cloud::{accounts, crypto};
use serde_json::Value;
use sqlx::PgPool;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;
use zeroize::Zeroizing;

const ORIGIN: &str = "http://localhost:3008";
const STAFF: &str = "staff@example.test";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nphoto";
const PDF: &[u8] = b"%PDF-1.7\nscan";

struct Site {
    db: PgPool,
    router: axum::Router,
}

impl Site {
    fn new(db: PgPool) -> Self {
        Self::with_store(db, DocumentStore::Database)
    }

    fn with_store(db: PgPool, store: DocumentStore) -> Self {
        let config = Config {
            host: "127.0.0.1".into(),
            port: 0,
            database_url: String::new(),
            account_origin: ORIGIN.into(),
            extra_origins: Vec::new(),
            secure_cookies: false,
            document_key: [7; 32],
            admin_emails: vec![STAFF.into()],
            mail: MailConfig::Log,
            trust_list: None,
        };
        let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        // The account site never reaches the Sender in these tests.
        let sender = SenderLink::new("http://127.0.0.1:9", Zeroizing::new("unused".into()));
        let state = AppState::new(config, db.clone(), store, key, sender);
        Self { db, router: buwiz_cloud::app(state) }
    }

    async fn send(&self, request: Request<Body>) -> Response {
        self.router.clone().oneshot(request).await.unwrap()
    }

    /// A signed-in account: (id, cookie header value).
    async fn person(&self, email: &str) -> (Uuid, String) {
        let id: Uuid = sqlx::query_scalar("INSERT INTO accounts (email) VALUES ($1) RETURNING id")
            .bind(email)
            .fetch_one(&self.db)
            .await
            .unwrap();
        let token = accounts::start_session(&self.db, id).await.unwrap();
        (id, format!("buwiz_session={token}"))
    }

    async fn get(&self, path: &str, cookie: &str) -> Response {
        self.send(Request::get(path).header(header::COOKIE, cookie).body(Body::empty()).unwrap()).await
    }

    async fn on_host(&self, host: &str, path: &str) -> Response {
        self.send(Request::get(path).header(header::HOST, host).body(Body::empty()).unwrap()).await
    }

    async fn post(&self, path: &str, cookie: &str, form: &str) -> Response {
        let request = Request::post(path)
            .header(header::COOKIE, cookie)
            .header(header::ORIGIN, ORIGIN)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(form.to_owned()))
            .unwrap();
        self.send(request).await
    }

    async fn upload(&self, path: &str, cookie: &str, parts: &[(&str, Part)]) -> Response {
        let boundary = "buwiz-test-boundary";
        let mut body = Vec::new();
        for (name, part) in parts {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            match part {
                Part::Text(value) => {
                    body.extend_from_slice(
                        format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                    );
                    body.extend_from_slice(value.as_bytes());
                }
                Part::File(filename, bytes) => {
                    body.extend_from_slice(
                        format!(
                            "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n\
                             Content-Type: application/octet-stream\r\n\r\n"
                        )
                        .as_bytes(),
                    );
                    body.extend_from_slice(bytes);
                }
            }
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        let request = Request::post(path)
            .header(header::COOKIE, cookie)
            .header(header::ORIGIN, ORIGIN)
            .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
            .body(Body::from(body))
            .unwrap();
        self.send(request).await
    }

    async fn claim_status(&self, claim_id: Uuid) -> String {
        sqlx::query_scalar("SELECT status FROM tin_claims WHERE id = $1")
            .bind(claim_id)
            .fetch_one(&self.db)
            .await
            .unwrap()
    }

    async fn newest_claim(&self, account_id: Uuid) -> Uuid {
        sqlx::query_scalar("SELECT id FROM tin_claims WHERE account_id = $1 ORDER BY created_at DESC LIMIT 1")
            .bind(account_id)
            .fetch_one(&self.db)
            .await
            .unwrap()
    }
}

enum Part {
    Text(&'static str),
    File(&'static str, &'static [u8]),
}

use Part::{File, Text};

fn location(response: &Response) -> String {
    response.headers().get(header::LOCATION).map(|value| value.to_str().unwrap().to_owned()).unwrap_or_default()
}

async fn bytes(response: Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()
}

async fn text(response: Response) -> String {
    String::from_utf8(bytes(response).await).unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn each_kind_of_tin_needs_its_own_papers(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person("ana@example.test").await;
    let personal = |extra: Vec<(&'static str, Part)>| {
        let mut parts = vec![
            ("kind", Text("personal")),
            ("tin", Text("123-456-789")),
            ("branch", Text("00000")),
            ("registered_name", Text("SANTOS, ANA MARIE")),
        ];
        parts.extend(extra);
        parts
    };

    let response = site.upload("/tins/claims", &cookie, &personal(vec![("selfie", File("me.png", PNG))])).await;
    assert_eq!(location(&response), "/tins/new?notice=need_tin_id");
    let response = site.upload("/tins/claims", &cookie, &personal(vec![("form_2303", File("2303.pdf", PDF))])).await;
    assert_eq!(location(&response), "/tins/new?notice=need_tin_id");
    let wrong = personal(vec![("tin_id", File("id.png", PNG)), ("authority", File("spa.pdf", PDF))]);
    assert_eq!(location(&site.upload("/tins/claims", &cookie, &wrong).await), "/tins/new?notice=wrong_documents");
    let no_kind = [("tin", Text("123456789")), ("registered_name", Text("Ana")), ("tin_id", File("id.png", PNG))];
    assert_eq!(location(&site.upload("/tins/claims", &cookie, &no_kind).await), "/tins/new?notice=kind_missing");

    let good = personal(vec![
        ("tin_id", File("id.png", PNG)),
        ("selfie", File("me.png", PNG)),
        ("national_id", File("philsys.png", PNG)),
    ]);
    let response = site.upload("/tins/claims", &cookie, &good).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/tins?notice=claim_submitted");
    let claim = site.newest_claim(ana).await;
    let kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM claim_documents WHERE claim_id = $1 ORDER BY kind")
        .bind(claim)
        .fetch_all(&site.db)
        .await
        .unwrap();
    assert_eq!(kinds, ["national_id", "selfie", "tin_id"]);

    let business = |extra: Vec<(&'static str, Part)>| {
        let mut parts = vec![
            ("kind", Text("business")),
            ("tin", Text("987654321")),
            ("branch", Text("00002")),
            ("registered_name", Text("CORA TRADING INC.")),
            ("trade_name", Text("Cora Trading")),
            ("form_2303", File("2303.pdf", PDF)),
        ];
        parts.extend(extra);
        parts
    };
    assert_eq!(location(&site.upload("/tins/claims", &cookie, &business(vec![])).await), "/tins/new?notice=need_id");
    let selfie = business(vec![("government_id", File("id.png", PNG)), ("selfie", File("me.png", PNG))]);
    assert_eq!(location(&site.upload("/tins/claims", &cookie, &selfie).await), "/tins/new?notice=wrong_documents");
    let good = business(vec![("government_id", File("id.png", PNG)), ("authority", File("board.pdf", PDF))]);
    assert_eq!(location(&site.upload("/tins/claims", &cookie, &good).await), "/tins?notice=claim_submitted");
    let row: (String, String, Option<String>) =
        sqlx::query_as("SELECT kind, branch_code, trade_name FROM tin_claims WHERE id = $1")
            .bind(site.newest_claim(ana).await)
            .fetch_one(&site.db)
            .await
            .unwrap();
    assert_eq!(row, ("business".into(), "00002".into(), Some("Cora Trading".into())));

    let page = text(site.get("/tins", &cookie).await).await;
    assert!(page.contains("123-456-789-00000") && page.contains("987-654-321-00002"), "{page}");
    assert!(page.contains("Being checked"), "{page}");
}

#[sqlx::test(migrations = "./migrations")]
async fn a_claim_sent_back_is_fixed_and_resent(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person("ana@example.test").await;
    let (_, staff) = site.person(STAFF).await;
    let claim_parts = [
        ("kind", Text("personal")),
        ("tin", Text("123456789")),
        ("registered_name", Text("Ana Santos")),
        ("tin_id", File("blurry.png", PNG)),
    ];
    site.upload("/tins/claims", &cookie, &claim_parts).await;
    let claim = site.newest_claim(ana).await;
    let decide = format!("/staff/claims/{claim}/decision");

    let response = site.post(&decide, &staff, "decision=fix").await;
    assert_eq!(location(&response), format!("/staff/claims/{claim}?notice=fix_needs_reason"));
    let response = site.post(&decide, &staff, "decision=fix&reasons=blurry&reasons=name_mismatch&note=Retake+it").await;
    assert_eq!(location(&response), "/staff?notice=claim_sent_back");
    assert_eq!(site.claim_status(claim).await, "needs_fix");
    let reasons: Vec<String> = sqlx::query_scalar("SELECT fix_reasons FROM tin_claims WHERE id = $1")
        .bind(claim)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert_eq!(reasons, ["blurry", "name_mismatch"]);

    let page = text(site.get("/tins", &cookie).await).await;
    assert!(
        page.contains("Needs a fix") && page.contains("A photo is too blurry to read.") && page.contains("Retake it")
    );
    // Still open: the same TIN cannot be claimed again meanwhile.
    let again = site.upload("/tins/claims", &cookie, &claim_parts).await;
    assert_eq!(location(&again), "/tins/new?notice=claim_already_pending");
    let fix_page = format!("/tins/claims/{claim}/fix");
    let page = site.get(&fix_page, &cookie).await;
    assert_eq!(page.status(), StatusCode::OK);
    assert!(text(page).await.contains("Retake it"));

    let resend = format!("/tins/claims/{claim}/resend");
    let unchanged = [("registered_name", Text("Ana Santos"))];
    assert_eq!(
        location(&site.upload(&resend, &cookie, &unchanged).await),
        format!("{fix_page}?notice=fix_needs_change")
    );
    let (_, someone) = site.person("someone@example.test").await;
    let fixed = [("registered_name", Text("SANTOS, ANA")), ("tin_id", File("sharp.png", PNG))];
    assert_eq!(location(&site.upload(&resend, &someone, &fixed).await), "/tins?notice=claim_missing");
    assert_eq!(location(&site.upload(&resend, &cookie, &fixed).await), "/tins?notice=claim_resent");

    assert_eq!(site.claim_status(claim).await, "pending");
    let (name, resent): (String, Option<OffsetDateTime>) =
        sqlx::query_as("SELECT registered_name, resent_at FROM tin_claims WHERE id = $1")
            .bind(claim)
            .fetch_one(&site.db)
            .await
            .unwrap();
    assert_eq!(name, "SANTOS, ANA");
    assert!(resent.is_some());
    // The replaced file is deleted; its row and hash stay.
    let files: Vec<(String, bool)> = sqlx::query_as(
        "SELECT filename, purged_at IS NULL AND ciphertext IS NOT NULL FROM claim_documents
         WHERE claim_id = $1 ORDER BY created_at",
    )
    .bind(claim)
    .fetch_all(&site.db)
    .await
    .unwrap();
    assert_eq!(files, [("blurry.png".to_owned(), false), ("sharp.png".to_owned(), true)]);
    assert_eq!(location(&site.upload(&resend, &cookie, &fixed).await), "/tins?notice=not_sent_back");

    let review = text(site.get(&format!("/staff/claims/{claim}"), &staff).await).await;
    assert!(review.contains("Typed vs. on the document") && review.contains("Resent"), "{review}");
    assert_eq!(location(&site.post(&decide, &staff, "decision=approve").await), "/staff?notice=claim_decided");
    assert_eq!(site.claim_status(claim).await, "approved");
}

#[sqlx::test(migrations = "./migrations")]
async fn staff_pages_are_not_found_for_everyone_else(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person("ana@example.test").await;
    let (_, staff) = site.person(STAFF).await;
    let parts = [
        ("kind", Text("business")),
        ("tin", Text("987654321")),
        ("registered_name", Text("Cora Trading Inc.")),
        ("form_2303", File("2303.pdf", PDF)),
        ("government_id", File("id.png", PNG)),
    ];
    site.upload("/tins/claims", &cookie, &parts).await;
    let claim = site.newest_claim(ana).await;
    let document: Uuid =
        sqlx::query_scalar("SELECT id FROM claim_documents WHERE claim_id = $1 AND kind = 'government_id'")
            .bind(claim)
            .fetch_one(&site.db)
            .await
            .unwrap();
    let pages = [
        "/staff".to_owned(),
        "/staff?status=sent_back&q=cora".to_owned(),
        "/staff/users".to_owned(),
        "/staff/users?filter=waiting&q=987".to_owned(),
        format!("/staff/users/{ana}"),
        format!("/staff/claims/{claim}"),
        format!("/staff/documents/{document}"),
    ];
    for page in &pages {
        assert_eq!(site.get(page, &cookie).await.status(), StatusCode::NOT_FOUND, "{page}");
        let response = site.get(page, &staff).await;
        assert_eq!(response.status(), StatusCode::OK, "{page}");
    }
    let posts = [
        (format!("/staff/claims/{claim}/decision"), "decision=approve"),
        (format!("/staff/users/{ana}/sign-out"), ""),
        (format!("/staff/users/{ana}/apps/{}/sign-out", Uuid::new_v4()), ""),
    ];
    for (path, form) in &posts {
        assert_eq!(site.post(path, &cookie, form).await.status(), StatusCode::NOT_FOUND, "{path}");
    }
    assert_eq!(site.claim_status(claim).await, "pending");

    let file = site.get(&format!("/staff/documents/{document}"), &staff).await;
    assert_eq!(file.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(file.headers()[header::CONTENT_DISPOSITION], "inline; filename=\"id.png\"");
    assert_eq!(file.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(bytes(file).await, PNG);

    // Signed out, staff pages ask for a sign-in like every other page.
    let response = site.get("/staff", "").await;
    assert!(response.status().is_redirection() && location(&response).starts_with("/login"));

    let queue = text(site.get("/staff", &staff).await).await;
    assert!(queue.contains("987-654-321-00000") && queue.contains("STAFF"), "{queue}");
    let users = text(site.get("/staff/users?q=cora", &staff).await).await;
    assert!(users.contains("ana@example.test") && users.contains("Cora Trading Inc."), "{users}");
    // Users see the light header, never the staff one.
    assert!(!text(site.get("/tins", &cookie).await).await.contains("STAFF"));
}

#[sqlx::test(migrations = "./migrations")]
async fn old_links_move_to_their_new_places(db: PgPool) {
    let site = Site::new(db);
    let id = Uuid::new_v4();
    let moved = [
        ("/admin".to_owned(), "/staff".to_owned()),
        ("/admin/claims".to_owned(), "/staff".to_owned()),
        ("/admin/claims?notice=claim_decided".to_owned(), "/staff?notice=claim_decided".to_owned()),
        (format!("/admin/claims/{id}"), format!("/staff/claims/{id}")),
        (format!("/admin/documents/{id}"), format!("/staff/documents/{id}")),
        ("/account".to_owned(), "/tins".to_owned()),
        ("/account?notice=claim_submitted".to_owned(), "/tins?notice=claim_submitted".to_owned()),
        ("/staff/claims".to_owned(), "/staff".to_owned()),
    ];
    for (old, new) in moved {
        let response = site.get(&old, "").await;
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT, "{old}");
        assert_eq!(location(&response), new, "{old}");
    }
    // A 308 keeps the method, so an old decision form still posts.
    let response = site.post(&format!("/admin/claims/{id}/decision"), "", "decision=approve").await;
    assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(location(&response), format!("/staff/claims/{id}/decision"));
}

/// An app sign-in for `account_id` with the BIR app's scope.
async fn app_token(db: &PgPool, account_id: Uuid) -> String {
    let grant: Uuid = sqlx::query_scalar(
        "INSERT INTO oauth_grants (account_id, client_id, scope) VALUES ($1, 'buwiz-desktop', 'app') RETURNING id",
    )
    .bind(account_id)
    .fetch_one(db)
    .await
    .unwrap();
    let token = crypto::new_token("bwa_");
    sqlx::query("INSERT INTO oauth_access_tokens (grant_id, token_hash, expires_at) VALUES ($1, $2, now() + interval '15 minutes')")
        .bind(grant)
        .bind(crypto::hash(&token))
        .execute(db)
        .await
        .unwrap();
    token
}

async fn add_claim(db: &PgPool, account_id: Uuid, root: &str, status: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status, decided_at)
         VALUES ($1, 'business', $2, '00000', 'Juan', $3, CASE WHEN $3 IN ('approved', 'rejected') THEN now() END)
         RETURNING id",
    )
    .bind(account_id)
    .bind(root)
    .bind(status)
    .fetch_one(db)
    .await
    .unwrap()
}

async fn profiles(site: &Site, token: &str, path: &str) -> Value {
    let request =
        Request::get(path).header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap();
    serde_json::from_str::<Value>(&text(site.send(request).await).await).unwrap()["profiles"].clone()
}

#[sqlx::test(migrations = "./migrations")]
async fn tax_profiles_list_open_claims_only_when_asked(db: PgPool) {
    let site = Site::new(db);
    let (juan, _) = site.person("juan@example.test").await;
    for (root, status) in
        [("111111111", "approved"), ("222222222", "pending"), ("333333333", "needs_fix"), ("444444444", "rejected")]
    {
        add_claim(&site.db, juan, root, status).await;
    }
    let token = app_token(&site.db, juan).await;

    let default = profiles(&site, &token, "/api/tax-profiles").await;
    let default = default.as_array().unwrap();
    assert_eq!(default.len(), 1);
    let mut keys: Vec<&str> = default[0].as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["activated_at", "id", "permissions", "registered_name", "tin", "tin_masked"]);
    assert_eq!(default[0]["tin"], "111-111-111-00000");
    assert_eq!(default[0]["permissions"], serde_json::json!(["profile.view", "filing.prepare", "filing.submit"]));

    let all = profiles(&site, &token, "/api/tax-profiles?include=pending").await;
    let all = all.as_array().unwrap();
    let statuses: Vec<&str> = all.iter().map(|profile| profile["status"].as_str().unwrap()).collect();
    assert_eq!(statuses, ["approved", "pending", "needs_fix"]);
    assert_eq!(all[0]["id"], default[0]["id"]);
    for open in &all[1..] {
        assert!(open["activated_at"].is_null());
        assert_eq!(open["permissions"], serde_json::json!(["profile.view", "filing.prepare"]));
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn staff_can_sign_an_account_out_everywhere(db: PgPool) {
    let site = Site::new(db);
    let (juan, cookie) = site.person("juan@example.test").await;
    let (_, staff) = site.person(STAFF).await;
    let token = app_token(&site.db, juan).await;
    let me =
        || Request::get("/me").header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap();
    assert_eq!(site.send(me()).await.status(), StatusCode::OK);
    assert_eq!(site.get("/tins", &cookie).await.status(), StatusCode::OK);

    let page = text(site.get(&format!("/staff/users/{juan}"), &staff).await).await;
    assert!(page.contains("Buwiz app") && page.contains("1 browser signed in"), "{page}");
    let response = site.post(&format!("/staff/users/{juan}/sign-out"), &staff, "").await;
    assert_eq!(location(&response), format!("/staff/users/{juan}?notice=signed_out_everywhere"));

    assert_eq!(site.send(me()).await.status(), StatusCode::UNAUTHORIZED);
    assert!(site.get("/tins", &cookie).await.status().is_redirection());
    // Staff stay signed in: only that account was signed out.
    assert_eq!(site.get("/staff", &staff).await.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "./migrations")]
async fn sign_in_and_add_tin_pages_render(db: PgPool) {
    let site = Site::new(db);
    let (_, cookie) = site.person("ana@example.test").await;
    let verify = text(site.get("/login/verify?email=ana%40example.test", "").await).await;
    assert!(verify.contains("autocomplete=\"one-time-code\"") && verify.contains("name=\"code\""), "{verify}");
    let add = text(site.get("/tins/new", &cookie).await).await;
    for field in [
        "name=\"kind\"",
        "name=\"tin\"",
        "name=\"branch\"",
        "name=\"tin_id\"",
        "name=\"selfie\"",
        "name=\"national_id\"",
        "name=\"form_2303\"",
        "name=\"government_id\"",
        "name=\"authority\"",
    ] {
        assert!(add.contains(field), "{field}");
    }
    assert_eq!(site.get("/assets/account.css", "").await.status(), StatusCode::OK);
    assert_eq!(site.get("/assets/account.js", "").await.status(), StatusCode::OK);
}

fn content_type(response: &Response) -> &str {
    response.headers().get(header::CONTENT_TYPE).unwrap().to_str().unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn robots_and_the_sitemap_follow_the_host(db: PgPool) {
    let site = Site::new(db);
    let public = text(site.on_host("buwiz.com", "/robots.txt").await).await;
    assert!(public.contains("User-agent: *\nAllow: /\nDisallow: /tins\n"), "{public}");
    assert!(public.contains("User-agent: Claude-SearchBot\n"), "{public}");
    assert!(public.contains("Sitemap: https://buwiz.com/sitemap.xml"), "{public}");
    for host in ["account.buwiz.com", "api.buwiz.com"] {
        assert_eq!(text(site.on_host(host, "/robots.txt").await).await, "User-agent: *\nDisallow: /\n", "{host}");
        assert_eq!(site.on_host(host, "/sitemap.xml").await.status(), StatusCode::NOT_FOUND, "{host}");
    }
    let sitemap = site.on_host("buwiz.com", "/sitemap.xml").await;
    assert_eq!(content_type(&sitemap), "application/xml; charset=utf-8");
    let sitemap = text(sitemap).await;
    assert!(sitemap.contains("<loc>https://buwiz.com/</loc>") && sitemap.contains("<lastmod>"), "{sitemap}");
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
    for path in ["/", "/login", "/robots.txt"] {
        assert!(site.on_host("buwiz.com", path).await.headers().get("x-robots-tag").is_none(), "{path}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn the_home_page_alone_is_described_for_search_and_sharing(db: PgPool) {
    let site = Site::new(db);
    let home = text(site.on_host("buwiz.com", "/").await).await;
    for tag in [
        "<title>Buwiz: Philippine tax filing, made modern</title>",
        r#"<link rel="canonical" href="https://buwiz.com/">"#,
        r#"<meta property="og:url" content="https://buwiz.com/">"#,
        r#"<meta property="og:image" content="https://buwiz.com/assets/brand/og-image.png">"#,
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
    assert_eq!(json_ld[0]["logo"], "https://buwiz.com/assets/brand/icon-512.png");
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

#[sqlx::test(migrations = "./migrations")]
async fn the_documents_step_keeps_every_file_field_and_orus_moves_to_sign_up(db: PgPool) {
    let site = Site::new(db);
    let (_, cookie) = site.person("ana@example.test").await;
    let add = text(site.get("/tins/new", &cookie).await).await;
    let step = &add[add.find("data-step=\"3\"").unwrap()..];
    let personal = &step[step.find("docs for-personal").unwrap()..step.find("docs for-business").unwrap()];
    let business = &step[step.find("docs for-business").unwrap()..];
    for field in ["tin_id", "selfie", "national_id"] {
        assert!(personal.contains(&format!("name=\"{field}\"")), "personal {field}");
    }
    for field in ["form_2303", "government_id", "authority", "supporting"] {
        assert!(business.contains(&format!("name=\"{field}\"")), "business {field}");
    }
    // Each optional paper's chip opens its zone.
    for field in ["selfie", "national_id", "authority", "supporting"] {
        assert!(step.contains(&format!("aria-controls=\"extra-{field}\"")), "chip {field}");
        assert!(step.contains(&format!("id=\"extra-{field}\"")), "zone {field}");
    }
    assert!(!add.contains("orus.bir.gov.ph"), "the ORUS link left /tins/new");
    let signup = text(site.get("/signup", "").await).await;
    assert!(signup.contains("href=\"https://orus.bir.gov.ph/\" target=\"_blank\" rel=\"noopener\""), "{signup}");
}

/// A stand-in for R2: objects kept in memory by path.
#[derive(Clone, Default)]
struct FakeObjects(Arc<Mutex<HashMap<String, Vec<u8>>>>);

impl FakeObjects {
    /// Serves the store on a free port; returns it and a [`DocumentStore`]
    /// that talks to it.
    async fn start() -> (Self, DocumentStore) {
        let objects = Self::default();
        let shared = objects.0.clone();
        let app = axum::Router::new().route(
            "/{*key}",
            axum::routing::any(move |method: Method, uri: Uri, body: Bytes| {
                let shared = shared.clone();
                async move {
                    let mut objects = shared.lock().unwrap();
                    let path = uri.path().to_owned();
                    match method {
                        Method::PUT => {
                            objects.insert(path, body.to_vec());
                            StatusCode::OK.into_response()
                        }
                        Method::GET => match objects.get(&path) {
                            Some(bytes) => bytes.clone().into_response(),
                            None => StatusCode::NOT_FOUND.into_response(),
                        },
                        Method::DELETE => {
                            objects.remove(&path);
                            StatusCode::NO_CONTENT.into_response()
                        }
                        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = ObjectStore::new(&endpoint, "auto", "buwiz-test", "key", "secret").unwrap();
        (objects, DocumentStore::Objects(Box::new(store)))
    }

    /// Stored objects of one claim.
    fn of_claim(&self, claim_id: Uuid) -> Vec<String> {
        let prefix = format!("/buwiz-test/claims/{claim_id}/");
        self.0.lock().unwrap().keys().filter(|key| key.starts_with(&prefix)).cloned().collect()
    }
}

/// (action, document kind, masked TIN, actor email, reason) of an account's
/// audit lines, oldest first.
async fn audit_lines(db: &PgPool, account_id: Uuid) -> Vec<(String, Option<String>, String, String, Option<String>)> {
    sqlx::query_as(
        "SELECT action, document_kind, tin_masked, actor_email, reason FROM claim_audit
         WHERE account_id = $1 ORDER BY created_at, action",
    )
    .bind(account_id)
    .fetch_all(db)
    .await
    .unwrap()
}

async fn claim_exists(db: &PgPool, claim_id: Uuid) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM tin_claims WHERE id = $1)")
        .bind(claim_id)
        .fetch_one(db)
        .await
        .unwrap()
}

const BUSINESS: [(&str, Part); 5] = [
    ("kind", Text("business")),
    ("tin", Text("987654321")),
    ("registered_name", Text("Cora Trading Inc.")),
    ("form_2303", File("2303.pdf", PDF)),
    ("government_id", File("id.png", PNG)),
];

#[sqlx::test(migrations = "./migrations")]
async fn claimants_delete_claims_that_never_became_active(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person("ana@example.test").await;
    let (_, bob) = site.person("bob@example.test").await;

    site.upload("/tins/claims", &cookie, &BUSINESS).await;
    let sent = site.newest_claim(ana).await;
    let page = text(site.get("/tins", &cookie).await).await;
    assert!(page.contains(&format!("action=\"/tins/claims/{sent}/delete\"")), "{page}");
    assert!(page.contains("data-confirm=\"Delete this claim?\"") && page.contains("erased for good"), "{page}");
    // Someone else's claim is not there for them.
    let response = site.post(&format!("/tins/claims/{sent}/delete"), &bob, "").await;
    assert_eq!(location(&response), "/tins?notice=claim_missing");
    assert!(claim_exists(&site.db, sent).await);
    let response = site.post(&format!("/tins/claims/{sent}/delete"), &cookie, "").await;
    assert_eq!(location(&response), "/tins?notice=claim_deleted");
    assert!(!claim_exists(&site.db, sent).await);
    let files: i64 = sqlx::query_scalar("SELECT count(*) FROM claim_documents WHERE claim_id = $1")
        .bind(sent)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert_eq!(files, 0);

    for (root, status) in [("222222222", "needs_fix"), ("333333333", "rejected"), ("444444444", "withdrawn")] {
        let claim = add_claim(&site.db, ana, root, status).await;
        let response = site.post(&format!("/tins/claims/{claim}/delete"), &cookie, "").await;
        assert_eq!(location(&response), "/tins?notice=claim_deleted", "{status}");
        assert!(!claim_exists(&site.db, claim).await, "{status}");
    }
    for status in ["approved", "removed", "revoked"] {
        let claim = add_claim(&site.db, ana, "555555555", status).await;
        let response = site.post(&format!("/tins/claims/{claim}/delete"), &cookie, "").await;
        assert_eq!(location(&response), "/tins?notice=claim_not_deletable", "{status}");
        assert!(claim_exists(&site.db, claim).await, "{status}");
    }

    let lines = audit_lines(&site.db, ana).await;
    assert_eq!(lines.len(), 4);
    let (action, kind, tin, actor, reason) = &lines[0];
    assert_eq!((action.as_str(), kind, tin.as_str()), ("deleted", &None, "***-***-321-00000"));
    assert_eq!((actor.as_str(), reason), ("ana@example.test", &None));
    assert!(lines.iter().all(|line| line.0 == "deleted" && !line.2.contains("987")));
}

#[sqlx::test(migrations = "./migrations")]
async fn deleting_a_claim_purges_its_files_from_the_store(db: PgPool) {
    let (objects, store) = FakeObjects::start().await;
    let site = Site::with_store(db, store);
    let (ana, cookie) = site.person("ana@example.test").await;
    let (_, staff) = site.person(STAFF).await;

    assert_eq!(location(&site.upload("/tins/claims", &cookie, &BUSINESS).await), "/tins?notice=claim_submitted");
    let claim = site.newest_claim(ana).await;
    assert_eq!(objects.of_claim(claim).len(), 2);
    site.post(&format!("/tins/claims/{claim}/delete"), &cookie, "").await;
    assert!(objects.of_claim(claim).is_empty());

    // Staff deleting does the same.
    site.upload("/tins/claims", &cookie, &BUSINESS).await;
    let claim = site.newest_claim(ana).await;
    assert_eq!(objects.of_claim(claim).len(), 2);
    let response = site.post(&format!("/staff/claims/{claim}/delete"), &staff, "reason=Duplicate").await;
    assert_eq!(location(&response), "/staff?notice=claim_deleted");
    assert!(objects.of_claim(claim).is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn removing_an_active_tin_keeps_its_returns_and_frees_it(db: PgPool) {
    let site = Site::new(db);
    let (juan, cookie) = site.person("juan@example.test").await;
    let (_, bob) = site.person("bob@example.test").await;
    let (_, staff) = site.person(STAFF).await;
    let claim = add_claim(&site.db, juan, "111111111", "approved").await;
    let open = add_claim(&site.db, juan, "222222222", "pending").await;
    let token = app_token(&site.db, juan).await;
    let send = || {
        let body = serde_json::json!({
            "filename": "111111111000-1601Cv2018-062025#juan@example.test#.xml",
            "xml": "<return/>",
        });
        Request::post("/api/submissions")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let sent = site.send(send()).await;
    assert_eq!(sent.status(), StatusCode::CREATED);
    let submission = serde_json::from_str::<Value>(&text(sent).await).unwrap()["id"].as_str().unwrap().to_owned();

    let page = text(site.get("/tins", &cookie).await).await;
    assert!(page.contains(&format!("action=\"/tins/claims/{claim}/remove\"")) && page.contains("Remove this TIN"));
    assert_eq!(
        location(&site.post(&format!("/tins/claims/{claim}/remove"), &bob, "").await),
        "/tins?notice=claim_missing"
    );
    assert_eq!(
        location(&site.post(&format!("/tins/claims/{open}/remove"), &cookie, "").await),
        "/tins?notice=tin_not_active"
    );
    let response = site.post(&format!("/tins/claims/{claim}/remove"), &cookie, "").await;
    assert_eq!(location(&response), "/tins?notice=tin_removed");
    assert_eq!(site.claim_status(claim).await, "removed");

    // No longer active anywhere: not a tax profile, not sendable.
    assert_eq!(profiles(&site, &token, "/api/tax-profiles").await, serde_json::json!([]));
    let all = profiles(&site, &token, "/api/tax-profiles?include=pending").await;
    assert_eq!(all.as_array().unwrap().len(), 1);
    assert_eq!(all[0]["id"], open.to_string());
    let refused = site.send(send()).await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert!(text(refused).await.contains("tin_not_active"));
    // The return already sent stays, still tied to the claim.
    let kept: Uuid = sqlx::query_scalar("SELECT claim_id FROM submissions WHERE id = $1::uuid")
        .bind(&submission)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert_eq!(kept, claim);
    let list = Request::get("/api/submissions").header(header::AUTHORIZATION, format!("Bearer {token}"));
    assert!(text(site.send(list.body(Body::empty()).unwrap()).await).await.contains(&submission));
    assert!(!text(site.get("/tins", &cookie).await).await.contains("111-111-111-00000"));

    // The TIN is free: another account claims it and staff approve it.
    let (maria, maria_cookie) = site.person("maria@example.test").await;
    let parts = [
        ("kind", Text("business")),
        ("tin", Text("111111111")),
        ("registered_name", Text("Maria")),
        ("form_2303", File("2303.pdf", PDF)),
        ("government_id", File("id.png", PNG)),
    ];
    assert_eq!(location(&site.upload("/tins/claims", &maria_cookie, &parts).await), "/tins?notice=claim_submitted");
    let again = site.newest_claim(maria).await;
    let decided = site.post(&format!("/staff/claims/{again}/decision"), &staff, "decision=approve").await;
    assert_eq!(location(&decided), "/staff?notice=claim_decided");
    assert_eq!(site.claim_status(again).await, "approved");

    let lines = audit_lines(&site.db, juan).await;
    assert_eq!(lines.len(), 1);
    assert_eq!((lines[0].0.as_str(), lines[0].2.as_str()), ("removed", "***-***-111-00000"));
    let history = text(site.get(&format!("/staff/users/{juan}"), &staff).await).await;
    assert!(history.contains("The person removed ***-***-111-00000"), "{history}");
}

#[sqlx::test(migrations = "./migrations")]
async fn staff_delete_or_revoke_claims_with_a_reason(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person("ana@example.test").await;
    let (_, staff) = site.person(STAFF).await;
    site.upload("/tins/claims", &cookie, &BUSINESS).await;
    let open = site.newest_claim(ana).await;
    let active = add_claim(&site.db, ana, "111111111", "approved").await;
    let token = app_token(&site.db, ana).await;

    let review = text(site.get(&format!("/staff/claims/{open}"), &staff).await).await;
    assert!(review.contains(&format!("action=\"/staff/claims/{open}/delete\"")) && review.contains("name=\"reason\""));
    let review = text(site.get(&format!("/staff/claims/{active}"), &staff).await).await;
    assert!(review.contains(&format!("action=\"/staff/claims/{active}/revoke\"")), "{review}");

    // Not staff: the same 404 as every staff route, even the owner.
    for path in [format!("/staff/claims/{open}/delete"), format!("/staff/claims/{active}/revoke")] {
        assert_eq!(site.post(&path, &cookie, "reason=mine").await.status(), StatusCode::NOT_FOUND, "{path}");
    }
    let delete = format!("/staff/claims/{open}/delete");
    assert_eq!(
        location(&site.post(&delete, &staff, "reason=+").await),
        format!("/staff/claims/{open}?notice=reason_missing")
    );
    let revoke_open = site.post(&format!("/staff/claims/{open}/revoke"), &staff, "reason=x").await;
    assert_eq!(location(&revoke_open), format!("/staff/claims/{open}?notice=tin_not_active"));
    assert_eq!(location(&site.post(&delete, &staff, "reason=Duplicate+claim").await), "/staff?notice=claim_deleted");
    assert!(!claim_exists(&site.db, open).await);

    let delete_active = site.post(&format!("/staff/claims/{active}/delete"), &staff, "reason=x").await;
    assert_eq!(location(&delete_active), format!("/staff/claims/{active}?notice=claim_not_deletable"));
    let revoke = format!("/staff/claims/{active}/revoke");
    assert_eq!(
        location(&site.post(&revoke, &staff, "").await),
        format!("/staff/claims/{active}?notice=reason_missing")
    );
    let response = site.post(&revoke, &staff, "reason=Owner+of+record+changed").await;
    assert_eq!(location(&response), format!("/staff/claims/{active}?notice=ownership_revoked"));
    assert_eq!(site.claim_status(active).await, "revoked");
    assert_eq!(profiles(&site, &token, "/api/tax-profiles").await, serde_json::json!([]));

    let tins = text(site.get("/tins", &cookie).await).await;
    assert!(tins.contains("No longer active") && tins.contains("Owner of record changed"), "{tins}");
    assert!(text(site.get("/staff?status=ended", &staff).await).await.contains("111-111-111-00000"));
    assert!(text(site.get(&format!("/staff/claims/{active}"), &staff).await).await.contains("Revoked by staff"));

    let lines = audit_lines(&site.db, ana).await;
    let summary: Vec<(&str, &str, Option<&str>)> =
        lines.iter().map(|line| (line.0.as_str(), line.3.as_str(), line.4.as_deref())).collect();
    assert_eq!(
        summary,
        [("deleted", STAFF, Some("Duplicate claim")), ("revoked", STAFF, Some("Owner of record changed"))]
    );
    let history = text(site.get(&format!("/staff/users/{ana}"), &staff).await).await;
    assert!(history.contains("deleted a waiting claim on ***-***-321-00000: Duplicate claim"), "{history}");
    assert!(history.contains("revoked the ownership of ***-***-111-00000"), "{history}");
}

#[sqlx::test(migrations = "./migrations")]
async fn claimants_replace_and_remove_files_while_open(db: PgPool) {
    let (objects, store) = FakeObjects::start().await;
    let site = Site::with_store(db, store);
    let (ana, cookie) = site.person("ana@example.test").await;
    let (_, bob) = site.person("bob@example.test").await;
    let (_, staff) = site.person(STAFF).await;
    let parts = [
        ("kind", Text("personal")),
        ("tin", Text("123456789")),
        ("registered_name", Text("Ana Santos")),
        ("tin_id", File("old-id.png", PNG)),
        ("selfie", File("me.png", PNG)),
    ];
    site.upload("/tins/claims", &cookie, &parts).await;
    let claim = site.newest_claim(ana).await;
    let document = |kind: &'static str| {
        let db = site.db.clone();
        async move {
            sqlx::query_scalar::<_, Uuid>("SELECT id FROM claim_documents WHERE claim_id = $1 AND kind = $2")
                .bind(claim)
                .bind(kind)
                .fetch_one(&db)
                .await
                .unwrap()
        }
    };
    let (tin_id, selfie) = (document("tin_id").await, document("selfie").await);
    let files_page = format!("/tins/claims/{claim}/files");
    let back = |notice: &str| format!("{files_page}?notice={notice}");
    let page = text(site.get(&files_page, &cookie).await).await;
    assert!(page.contains("old-id.png") && page.contains(&format!("/documents/{selfie}/remove")), "{page}");
    // A required file has no remove button.
    assert!(!page.contains(&format!("/documents/{tin_id}/remove")), "{page}");
    assert_eq!(site.get(&files_page, &bob).await.status(), StatusCode::NOT_FOUND);

    let remove_tin_id = format!("/tins/claims/{claim}/documents/{tin_id}/remove");
    assert_eq!(location(&site.post(&remove_tin_id, &cookie, "").await), back("file_required"));
    let remove_selfie = format!("/tins/claims/{claim}/documents/{selfie}/remove");
    assert_eq!(location(&site.post(&remove_selfie, &bob, "").await), "/tins?notice=claim_missing");
    let replace = format!("/tins/claims/{claim}/documents/{tin_id}/replace");
    assert_eq!(
        location(&site.upload(&replace, &bob, &[("tin_id", File("x.png", PNG))]).await),
        "/tins?notice=claim_missing"
    );
    assert_eq!(location(&site.upload(&replace, &cookie, &[]).await), back("choose_file"));
    let wrong = site.upload(&replace, &cookie, &[("selfie", File("me.png", PNG))]).await;
    assert_eq!(location(&wrong), back("wrong_documents"));
    let html = site.upload(&replace, &cookie, &[("tin_id", File("id.html", b"<html>"))]).await;
    assert_eq!(location(&html), back("file_type"));
    assert_eq!(objects.of_claim(claim).len(), 2);
    let files_changed: Option<OffsetDateTime> =
        sqlx::query_scalar("SELECT files_changed_at FROM tin_claims WHERE id = $1")
            .bind(claim)
            .fetch_one(&site.db)
            .await
            .unwrap();
    assert!(files_changed.is_none());

    let old_object = format!("/buwiz-test/claims/{claim}/{tin_id}");
    assert!(objects.of_claim(claim).contains(&old_object));
    let response = site.upload(&replace, &cookie, &[("tin_id", File("new-id.png", PNG))]).await;
    assert_eq!(location(&response), back("file_replaced"));
    let new_tin_id = document("tin_id").await;
    assert_ne!(new_tin_id, tin_id);
    let stored = objects.of_claim(claim);
    assert!(!stored.contains(&old_object) && stored.contains(&format!("/buwiz-test/claims/{claim}/{new_tin_id}")));
    assert_eq!(location(&site.post(&remove_selfie, &cookie, "").await), back("file_removed"));
    assert_eq!(objects.of_claim(claim).len(), 1);
    let rows: Vec<String> = sqlx::query_scalar("SELECT filename FROM claim_documents WHERE claim_id = $1")
        .bind(claim)
        .fetch_all(&site.db)
        .await
        .unwrap();
    assert_eq!(rows, ["new-id.png"]);
    assert_eq!(site.claim_status(claim).await, "pending");

    // Staff see it on the queue and the review.
    assert!(text(site.get("/staff", &staff).await).await.contains("Files changed after sending"));
    let review = text(site.get(&format!("/staff/claims/{claim}"), &staff).await).await;
    assert!(review.contains("Files changed after sending"), "{review}");

    let lines = audit_lines(&site.db, ana).await;
    let actions: Vec<(&str, Option<&str>)> = lines.iter().map(|line| (line.0.as_str(), line.1.as_deref())).collect();
    assert_eq!(actions, [("file_replaced", Some("tin_id")), ("file_removed", Some("selfie"))]);

    // Sent back: files still change, and that alone is a fix to resend.
    let decide = format!("/staff/claims/{claim}/decision");
    site.post(&decide, &staff, "decision=fix&reasons=blurry").await;
    let replace = format!("/tins/claims/{claim}/documents/{new_tin_id}/replace");
    assert_eq!(
        location(&site.upload(&replace, &cookie, &[("tin_id", File("sharp.png", PNG))]).await),
        back("file_replaced")
    );
    let resend = format!("/tins/claims/{claim}/resend");
    assert_eq!(
        location(&site.upload(&resend, &cookie, &[("registered_name", Text("Ana Santos"))]).await),
        "/tins?notice=claim_resent"
    );
    assert!(
        !text(site.get(&format!("/staff/claims/{claim}"), &staff).await).await.contains("Files changed after sending")
    );

    // Decided or ended claims keep their files as they are.
    site.post(&decide, &staff, "decision=approve").await;
    let current = document("tin_id").await;
    let locked = site.post(&format!("/tins/claims/{claim}/documents/{current}/remove"), &cookie, "").await;
    assert_eq!(location(&locked), "/tins?notice=files_locked");
    assert_eq!(location(&site.get(&files_page, &cookie).await), "/tins?notice=files_locked");
    for status in ["rejected", "withdrawn", "removed", "revoked"] {
        let other = add_claim(&site.db, ana, "222222222", status).await;
        let file: Uuid = sqlx::query_scalar(
            "INSERT INTO claim_documents (claim_id, kind, filename, content_type, size_bytes, sha256)
             VALUES ($1, 'supporting', 'x.pdf', 'application/pdf', 1, decode('00', 'hex')) RETURNING id",
        )
        .bind(other)
        .fetch_one(&site.db)
        .await
        .unwrap();
        let response = site.post(&format!("/tins/claims/{other}/documents/{file}/remove"), &cookie, "").await;
        assert_eq!(location(&response), "/tins?notice=files_locked", "{status}");
    }
}
