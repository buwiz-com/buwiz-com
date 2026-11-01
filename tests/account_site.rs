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
use buwiz_cloud::downloads::{self, Arch, Kind, Platform, Total};
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
/// Nothing listens on the discard port: the downloads bucket is unreachable,
/// so nothing is released.
const NO_DOWNLOADS: &str = "http://127.0.0.1:9";
/// The filing site's own host.
const FORMS: &str = "https://ebirforms.buwiz.com";
/// The filing site's host, as in [`FORMS`].
const FORMS_HOST: &str = "ebirforms.buwiz.com";
/// The account host that hostless test requests go to. Not [`ORIGIN`]'s own
/// host: `localhost:3008` is the Bot site.
const ACCOUNT_HOST: &str = "account.localhost:3008";

struct Site {
    db: PgPool,
    router: axum::Router,
}

impl Site {
    fn new(db: PgPool) -> Self {
        Self::with_store(db, DocumentStore::Database)
    }

    fn with_store(db: PgPool, store: DocumentStore) -> Self {
        Self::build(db, store, NO_DOWNLOADS)
    }

    /// A site whose installers are at `downloads_url`.
    fn with_downloads(db: PgPool, downloads_url: &str) -> Self {
        Self::build(db, DocumentStore::Database, downloads_url)
    }

    /// The filing site is on [`FORMS`], as `FORMS_ORIGIN` puts it.
    fn build(db: PgPool, store: DocumentStore, downloads_url: &str) -> Self {
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
            downloads_url: downloads_url.into(),
            forms_origin: FORMS.into(),
        };
        let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        // The account site never reaches the Sender in these tests.
        let sender = SenderLink::new("http://127.0.0.1:9", Zeroizing::new("unused".into()));
        let state = AppState::new(config, db.clone(), store, key, sender);
        Self { db, router: buwiz_cloud::app(state) }
    }

    /// A request without a `Host` goes to the account host, [`ACCOUNT_HOST`].
    async fn send(&self, mut request: Request<Body>) -> Response {
        request.headers_mut().entry(header::HOST).or_insert(header::HeaderValue::from_static(ACCOUNT_HOST));
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

    /// The claim's document readings once none is still running: (file,
    /// status, error).
    async fn readings(&self, claim_id: Uuid) -> Vec<(String, String, Option<String>)> {
        for _ in 0..100 {
            let readings: Vec<(String, String, Option<String>)> = sqlx::query_as(
                "SELECT d.filename, r.status, r.error FROM document_readings r
                 JOIN claim_documents d ON d.id = r.document_id WHERE d.claim_id = $1 ORDER BY d.created_at",
            )
            .bind(claim_id)
            .fetch_all(&self.db)
            .await
            .unwrap();
            if readings.iter().all(|(_, status, _)| status != "reading") {
                return readings;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("document readings did not finish");
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

    // Only the new TIN ID is read; the replaced file's reading went with it.
    // These test files are not real photos, and OCR may not be built in.
    let readings = site.readings(claim).await;
    assert_eq!(readings.len(), 1, "{readings:?}");
    let (file, status, error) = &readings[0];
    assert_eq!((file.as_str(), status.as_str()), ("sharp.png", "failed"));
    let expected = if cfg!(feature = "ocr") { "the photo could not be opened" } else { "OCR not built in" };
    assert_eq!(error.as_deref(), Some(expected));

    let review = text(site.get(&format!("/staff/claims/{claim}"), &staff).await).await;
    assert!(review.contains("Typed vs. on the document") && review.contains("Resent"), "{review}");
    assert!(review.contains("Read by OCR on our server") && review.contains(&format!("read: {expected}")), "{review}");
    assert_eq!(location(&site.post(&decide, &staff, "decision=approve").await), "/staff?notice=claim_decided");
    assert_eq!(site.claim_status(claim).await, "approved");
    assert_eq!(site.readings(claim).await, []);
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
async fn sign_in_and_sign_up_name_their_button_and_go_without_a_password(db: PgPool) {
    let site = Site::new(db);
    let button = |label: &str| {
        format!(r#"type="submit">{label}<span class="icon" aria-hidden="true">arrow_forward</span></button>"#)
    };
    let hint = r#"<p class="auth-hint"><span class="icon" aria-hidden="true">lock</span>Passwordless login</p>"#;
    let lead = "Enter your email and we will send you a 6-digit code.";
    let login = text(site.get("/login", "").await).await;
    assert!(login.contains(&button("Sign in")) && login.contains(hint) && login.contains(lead), "{login}");
    let signup = text(site.get("/signup", "").await).await;
    assert!(signup.contains(&button("Create account")) && signup.contains(hint) && signup.contains(lead), "{signup}");
    for page in [login, signup] {
        assert!(!page.contains("Email me a code") && !page.contains("10 minutes"), "{page}");
    }
}

/// The text of the code page's countdown and its `data-expires-at`.
fn code_timer(page: &str) -> (String, String) {
    let timer = &page[page.find("code-timer").unwrap()..];
    let timer = &timer[..timer.find("</p>").unwrap()];
    let expires_at = timer.split("data-expires-at=\"").nth(1).unwrap().split('"').next().unwrap();
    let words = timer.split("data-timer-text=\"\">").nth(1).unwrap().split('<').next().unwrap();
    assert!(!timer.contains("role="), "the ticking text is not announced: {timer}");
    (words.to_owned(), expires_at.to_owned())
}

#[sqlx::test(migrations = "./migrations")]
async fn the_code_page_counts_down_to_the_codes_real_expiry(db: PgPool) {
    let site = Site::new(db);
    let sent = site.post("/auth/email/start", "", "email=Ana%40Example.test").await;
    assert_eq!(sent.status(), StatusCode::SEE_OTHER);
    let verify = location(&sent);
    let stored: OffsetDateTime =
        sqlx::query_scalar("SELECT expires_at FROM email_codes").fetch_one(&site.db).await.unwrap();
    let page = text(site.get(&verify, "").await).await;
    let (words, expires_at) = code_timer(&page);
    // The link's expiry is the code's, give or take the time to store it.
    let linked = expires_at.parse::<i128>().unwrap();
    let real = stored.unix_timestamp_nanos() / 1_000_000;
    assert!((0..10_000).contains(&(real - linked)), "{linked} vs {real}");
    assert!(words == "Code expires in 10:00" || words.starts_with("Code expires in 9:5"), "{words}");
    assert!(page.contains(&format!(r#"<input type="hidden" name="expires" value="{expires_at}">"#)), "{page}");
    assert!(page.contains(r#"<p class="visually-hidden" role="status" data-timer-status=""></p>"#), "{page}");
    assert!(page.contains(r#"<form class="auth-resend" "#), "{page}");
    // The icon shares the heading's line, and the notice says only to look.
    assert!(page.contains(
        r#"<div class="auth-heading"><span class="auth-badge"><span class="icon" aria-hidden="true">mark_email_unread</span></span><h1>Check your email</h1></div>"#
    ));
    assert!(page.contains(
        r#"role="status"><span class="icon" aria-hidden="true">check_circle</span><span>Please check your email</span>"#
    ));
    assert!(!page.contains("Paste the 6-digit code"));

    // A wrong code comes back to the same countdown, not a fresh one.
    let wrong = site
        .post("/auth/email/verify", "", &format!("email=ana%40example.test&code=000000&expires={expires_at}"))
        .await;
    let page = text(site.get(&location(&wrong), "").await).await;
    assert_eq!(code_timer(&page).1, expires_at);

    // Reloaded 18 seconds before the end, it shows 0:18.
    let soon = (OffsetDateTime::now_utc() + time::Duration::milliseconds(18_500)).unix_timestamp_nanos() / 1_000_000;
    let page = text(site.get(&format!("/login/verify?email=ana%40example.test&expires={soon}"), "").await).await;
    assert!(["Code expires in 0:18", "Code expires in 0:17"].contains(&code_timer(&page).0.as_str()));

    // The page tells nothing from the database: a code for someone's email
    // without the link's expiry, a run-out one, or nonsense all say expired
    // and point at a new code. An expiry past ten minutes is cut to ten.
    let past = (OffsetDateTime::now_utc() - time::Duration::minutes(1)).unix_timestamp_nanos() / 1_000_000;
    for query in
        ["email=ana%40example.test", &format!("email=ana%40example.test&expires={past}"), "email=x&expires=soon"]
    {
        let page = text(site.get(&format!("/login/verify?{query}"), "").await).await;
        assert_eq!(code_timer(&page).0, "Code expired", "{query}");
        assert!(page.contains("code-timer expired") && page.contains(r#"<form class="auth-resend due" "#), "{page}");
    }
    let page = text(site.get("/login/verify?email=ana%40example.test&expires=99999999999999", "").await).await;
    assert!(["Code expires in 10:00", "Code expires in 9:59"].contains(&code_timer(&page).0.as_str()));
}

#[sqlx::test(migrations = "./migrations")]
async fn the_header_is_as_wide_on_every_page(db: PgPool) {
    let site = Site::new(db);
    let home = text(site.get("/assets/home.css", "").await).await;
    assert!(home.contains(".home.landing, .top { --shell: 1400px; --shell-pad: var(--gutter); }"));
    for sheet in ["/assets/site.css", "/assets/home.css", "/assets/account.css"] {
        let css = text(site.get(sheet, "").await).await;
        for rule in css.lines().filter(|line| line.contains("--shell") && line.contains('{')) {
            let selector = rule.split('{').next().unwrap();
            assert!(!selector.contains("top") || selector == ".home.landing, .top ", "{sheet}: {rule}");
        }
    }
    let site_css = text(site.get("/assets/site.css", "").await).await;
    assert!(site_css.contains("html { scrollbar-gutter: stable; }"));
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
    // The new TIN ID is read; the old one's reading went with its row.
    let readings = site.readings(claim).await;
    assert_eq!(readings.iter().map(|(file, ..)| file.as_str()).collect::<Vec<_>>(), ["new-id.png"]);
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

/// A stand-in for the `buwiz-releases` bucket: the root `/latest.json` and
/// the app's own `/ebirforms/latest.json` are whatever the test sets, or a
/// 404 while unset.
#[derive(Clone, Default)]
struct FakeReleases {
    root: Arc<Mutex<Option<Vec<u8>>>>,
    own: Arc<Mutex<Option<Vec<u8>>>>,
}

impl FakeReleases {
    /// Serves the bucket on a free port; returns it and its URL.
    async fn start() -> (Self, String) {
        let releases = Self::default();
        let serve = |shared: Arc<Mutex<Option<Vec<u8>>>>| {
            axum::routing::get(move || {
                let latest = shared.lock().unwrap().clone();
                async move {
                    match latest {
                        Some(bytes) => bytes.into_response(),
                        None => StatusCode::NOT_FOUND.into_response(),
                    }
                }
            })
        };
        let app = axum::Router::new()
            .route("/latest.json", serve(releases.root.clone()))
            .route("/ebirforms/latest.json", serve(releases.own.clone()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (releases, url)
    }

    /// The root `latest.json`, as before the bucket had a folder per app.
    fn set_latest(&self, latest: &[u8]) {
        *self.root.lock().unwrap() = Some(latest.to_vec());
    }

    /// The app's own `ebirforms/latest.json`.
    fn set_own_latest(&self, latest: &[u8]) {
        *self.own.lock().unwrap() = Some(latest.to_vec());
    }
}

const SHA_MAC: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const SHA_DEB: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const SHA_RPM: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

/// One file of a manifest.
fn release_file(platform: &str, arch: &str, kind: &str, file: &str, size: u64, sha: &str) -> Value {
    serde_json::json!({"platform": platform, "arch": arch, "kind": kind, "file": file, "size": size, "sha256": sha})
}

/// Version 1.2.0 as the release CI (eBIRForms) writes it, named as it names
/// them, with what the first release had: no Android file, one Linux
/// file, and an iOS entry this server does not know, plus `extra` files.
fn latest_json(extra: &[Value]) -> Vec<u8> {
    let other = "0".repeat(64);
    let mut files = vec![
        release_file("macos", "universal", "dmg", "Buwiz-1.2.0-macos-universal.dmg", 84_213_000, SHA_MAC),
        release_file("windows", "x64", "exe", "Buwiz-1.2.0-windows-x64-setup.exe", 60_000_000, &other),
        release_file("linux", "x64", "deb", "buwiz_1.2.0_amd64.deb", 55_000_000, SHA_DEB),
        release_file("ios", "arm64", "ipa", "Buwiz.ipa", 1, &other),
    ];
    files.extend_from_slice(extra);
    serde_json::json!({"version": "1.2.0", "published_at": "2026-11-02T01:00:00Z", "files": files})
        .to_string()
        .into_bytes()
}

/// The rest of what the CI uploads: the MSI, ARM64 debs, rpms and tarballs
/// for both processors, the Linux install script and the signed APK.
fn later_files() -> Vec<Value> {
    let other = "0".repeat(64);
    vec![
        release_file("windows", "x64", "msi", "Buwiz-1.2.0-windows-x64.msi", 62_000_000, &other),
        release_file("linux", "arm64", "deb", "buwiz_1.2.0_arm64.deb", 54_000_000, &other),
        release_file("linux", "x64", "rpm", "Buwiz-1.2.0-linux-x64.rpm", 56_000_000, SHA_RPM),
        release_file("linux", "arm64", "rpm", "Buwiz-1.2.0-linux-arm64.rpm", 55_500_000, &other),
        release_file("linux", "x64", "targz", "Buwiz-1.2.0-linux-x64.tar.gz", 57_000_000, &other),
        release_file("linux", "arm64", "targz", "Buwiz-1.2.0-linux-arm64.tar.gz", 56_500_000, &other),
        release_file("linux", "universal", "sh", "Buwiz-1.2.0-linux-install.sh", 4_000, &other),
        release_file("android", "arm64", "apk", "Buwiz-1.2.0-android-arm64.apk", 40_000_000, &other),
    ]
}

async fn download_from(site: &Site, path: &str) -> Response {
    site.on_host(FORMS_HOST, path).await
}

/// Asks for `path` on the filing site as a browser does, with `User-Agent`.
async fn browse(site: &Site, path: &str, user_agent: &str) -> Response {
    browse_on(site, FORMS_HOST, path, user_agent).await
}

/// Asks for `path` on `host` as a browser does, with `User-Agent`.
async fn browse_on(site: &Site, host: &str, path: &str, user_agent: &str) -> Response {
    let request = Request::get(path)
        .header(header::HOST, host)
        .header(header::USER_AGENT, user_agent)
        .header(header::ACCEPT, "text/html,application/xhtml+xml,*/*;q=0.8");
    site.send(request.body(Body::empty()).unwrap()).await
}

fn cache_control(response: &Response) -> &str {
    response.headers().get(header::CACHE_CONTROL).map_or("", |value| value.to_str().unwrap())
}

/// Asks for each path and checks it redirects, uncached, to `target`.
async fn assert_redirects(site: &Site, cases: &[(&str, String)]) {
    for (path, target) in cases {
        let response = download_from(site, path).await;
        assert_eq!(response.status(), StatusCode::FOUND, "{path}");
        assert_eq!(&location(&response), target, "{path}");
        assert_eq!(cache_control(&response), "no-store", "{path}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn downloads_go_to_the_released_installer_without_caching(db: PgPool) {
    let (releases, url) = FakeReleases::start().await;
    releases.set_latest(&latest_json(&[]));
    let site = Site::with_downloads(db.clone(), &url);
    let file = |name: &str| format!("{url}/releases/v1.2.0/{name}");
    assert_redirects(
        &site,
        &[
            ("/download/mac", file("Buwiz-1.2.0-macos-universal.dmg")),
            ("/download/mac?arch=arm64", file("Buwiz-1.2.0-macos-universal.dmg")),
            ("/download/windows", file("Buwiz-1.2.0-windows-x64-setup.exe")),
            ("/download/windows?kind=exe&arch=x64", file("Buwiz-1.2.0-windows-x64-setup.exe")),
            // Linux is the x64 deb unless asked otherwise, and anything it
            // does not know is too.
            ("/download/linux", file("buwiz_1.2.0_amd64.deb")),
            ("/download/linux?kind=deb&arch=x64", file("buwiz_1.2.0_amd64.deb")),
            ("/download/linux?kind=nonsense&arch=nonsense", file("buwiz_1.2.0_amd64.deb")),
            ("/download/linux?kind=msi", file("buwiz_1.2.0_amd64.deb")),
            // No such file: the platform on the download page, with the
            // choice made there.
            ("/download/linux?kind=rpm&arch=arm64", "/download?kind=rpm&arch=arm64#linux".into()),
            ("/download/linux?arch=arm64", "/download?kind=deb&arch=arm64#linux".into()),
            ("/download/linux?kind=appimage", "/download?kind=appimage&arch=x64#linux".into()),
            ("/download/windows?kind=msi", "/download#windows".into()),
            ("/download/android", "/download#android".into()),
            ("/download/mac?kind=nonsense", "/download#mac".into()),
        ],
    )
    .await;
    for path in ["/download/ios", "/download/macos", "/download/mac/extra"] {
        assert_eq!(download_from(&site, path).await.status(), StatusCode::NOT_FOUND, "{path}");
    }
    for host in ["account.buwiz.com", "api.buwiz.com"] {
        for path in ["/download/mac", "/download", "/install-linux.sh"] {
            assert_eq!(site.on_host(host, path).await.status(), StatusCode::NOT_FOUND, "{host}{path}");
        }
    }
    // No install script yet: a browser goes to the Linux downloads, and
    // curl gets a 404 rather than a web page to run.
    let script = browse(&site, "/install-linux.sh", "Mozilla/5.0 (X11; Linux x86_64)").await;
    assert_eq!((script.status(), location(&script)), (StatusCode::FOUND, "/download#linux".to_owned()));
    let curl = site
        .send(
            Request::get("/install-linux.sh")
                .header(header::HOST, FORMS_HOST)
                .header(header::ACCEPT, "*/*")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(curl.status(), StatusCode::NOT_FOUND);
    assert_eq!(cache_control(&curl), "no-store");

    // With every file the CI makes, each kind and processor is a query
    // away; Windows still defaults to the installer, not the MSI.
    releases.set_latest(&latest_json(&later_files()));
    let site = Site::with_downloads(db, &url);
    assert_redirects(
        &site,
        &[
            ("/download/windows", file("Buwiz-1.2.0-windows-x64-setup.exe")),
            ("/download/windows?kind=msi", file("Buwiz-1.2.0-windows-x64.msi")),
            ("/download/linux", file("buwiz_1.2.0_amd64.deb")),
            ("/download/linux?kind=deb&arch=arm64", file("buwiz_1.2.0_arm64.deb")),
            ("/download/linux?kind=rpm", file("Buwiz-1.2.0-linux-x64.rpm")),
            ("/download/linux?kind=rpm&arch=x64", file("Buwiz-1.2.0-linux-x64.rpm")),
            ("/download/linux?kind=rpm&arch=arm64", file("Buwiz-1.2.0-linux-arm64.rpm")),
            ("/download/linux?kind=targz&arch=x64", file("Buwiz-1.2.0-linux-x64.tar.gz")),
            ("/download/linux?kind=targz&arch=arm64", file("Buwiz-1.2.0-linux-arm64.tar.gz")),
            ("/download/android", file("Buwiz-1.2.0-android-arm64.apk")),
            ("/install-linux.sh", file("Buwiz-1.2.0-linux-install.sh")),
        ],
    )
    .await;
}

#[sqlx::test(migrations = "./migrations")]
async fn downloads_are_counted_per_file_and_day_without_crawlers(db: PgPool) {
    let (releases, url) = FakeReleases::start().await;
    releases.set_latest(&latest_json(&later_files()));
    let site = Site::with_downloads(db.clone(), &url);
    let fetch = |method: Method, path: &str, agent: &str| {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, FORMS_HOST)
            .header(header::USER_AGENT, agent)
            .body(Body::empty())
            .unwrap()
    };
    let person = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Safari/605.1.15";
    for (method, path, agent) in [
        (Method::GET, "/download/mac", person),
        (Method::GET, "/download/mac?kind=dmg", person),
        (Method::GET, "/download/linux?kind=rpm&arch=arm64", person),
        (Method::GET, "/install-linux.sh", "curl/8.7.1"),
        // Not counted: crawlers and link previews, no User-Agent, a HEAD.
        (Method::GET, "/download/mac", "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)"),
        (Method::GET, "/download/mac", "Slackbot-LinkExpanding 1.0 (+https://api.slack.com/robots)"),
        (Method::GET, "/download/mac", ""),
        (Method::HEAD, "/download/windows", person),
    ] {
        assert_eq!(site.send(fetch(method, path, agent)).await.status(), StatusCode::FOUND, "{path} {agent}");
    }
    let total = |platform, arch, kind, count| Total { platform, arch, kind, count };
    assert_eq!(
        downloads::totals(&db, downloads::APP).await.unwrap(),
        vec![
            total(Platform::Mac, Arch::Universal, Kind::Dmg, 2),
            total(Platform::Linux, Arch::Arm64, Kind::Rpm, 1),
            total(Platform::Linux, Arch::Universal, Kind::Sh, 1),
        ]
    );
    // Kept per version and Manila day, and nothing about who asked.
    let rows: Vec<(String, String, String, String, i64, bool)> = sqlx::query_as(
        "SELECT version, platform, arch, kind, count, day = (now() AT TIME ZONE 'Asia/Manila')::date
         FROM download_counts ORDER BY platform, kind",
    )
    .fetch_all(&db)
    .await
    .unwrap();
    let row = |platform: &str, arch: &str, kind: &str, count| {
        ("1.2.0".to_owned(), platform.to_owned(), arch.to_owned(), kind.to_owned(), count, true)
    };
    assert_eq!(
        rows,
        vec![row("linux", "arm64", "rpm", 1), row("linux", "universal", "sh", 1), row("macos", "universal", "dmg", 2)]
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn ebirforms_reads_its_own_latest_json_or_else_the_root_one_and_counts_both_as_its_own(db: PgPool) {
    let host = FORMS_HOST;
    let (releases, url) = FakeReleases::start().await;
    // Only the root one, as before the bucket had a folder per app.
    releases.set_latest(&latest_json(&[]));
    let site = Site::with_downloads(db.clone(), &url);
    let response = browse_on(&site, host, "/download/mac", MAC).await;
    assert_eq!(location(&response), format!("{url}/releases/v1.2.0/Buwiz-1.2.0-macos-universal.dmg"));
    assert!(text(browse_on(&site, host, "/download", MAC).await).await.contains("Version 1.2.0,"));

    // Its own one: read first, with the files in its folder.
    let own = String::from_utf8(latest_json(&[])).unwrap().replace("1.2.0", "1.3.0");
    releases.set_own_latest(own.as_bytes());
    let site = Site::with_downloads(db.clone(), &url);
    let response = browse_on(&site, host, "/download/mac", MAC).await;
    assert_eq!(location(&response), format!("{url}/ebirforms/releases/v1.3.0/Buwiz-1.3.0-macos-universal.dmg"));
    let page = text(browse_on(&site, host, "/download", MAC).await).await;
    assert!(page.contains("Version 1.3.0,") && !page.contains("1.2.0"), "{page}");
    assert!(!page.contains(&url), "never straight to the bucket");

    // One Download button per device: one per card, the visitor's marked once.
    let cards = platform_cards(&page);
    assert_eq!(cards.len(), 5);
    assert_eq!(cards.iter().filter(|card| card.starts_with(" yours\"")).count(), 1);
    for name in ["macOS", "Windows", "Linux"] {
        let card = card(&cards, name);
        assert_eq!(card.matches("btn-solid").count(), 1, "{card}");
        assert_eq!(card.matches("Download for").count(), 1, "{card}");
    }
    for name in ["Android", "iOS"] {
        assert!(!card(&cards, name).contains("btn-solid"), "{name}");
    }
    // The home page: one Download button for the device, nowhere else.
    for (agent, href) in [(MAC, "/download/mac"), (WINDOWS, "/download/windows"), (LINUX, "/download/linux")] {
        let home = text(browse_on(&site, host, "/", agent).await).await;
        assert_eq!(home.matches("btn-get").count(), 1, "{agent}");
        assert_eq!(home.matches("Download for").count(), 1, "{agent}");
        assert_eq!(home.matches(r#"href="/download/"#).count(), 1, "{agent}");
        assert!(home.contains(&format!(r#"href="{href}""#)), "{agent}");
        assert_eq!(home.matches(r#"href="/download""#).count(), 1, "Other platforms, once: {agent}");
    }

    // Counted as eBIRForms' downloads, per version.
    let fallback = Site::with_downloads(db.clone(), &url);
    *releases.own.lock().unwrap() = None;
    assert_eq!(browse_on(&fallback, host, "/download/mac", MAC).await.status(), StatusCode::FOUND);
    let total = |platform, arch, kind, count| Total { platform, arch, kind, count };
    assert_eq!(
        downloads::totals(&db, downloads::APP).await.unwrap(),
        vec![total(Platform::Mac, Arch::Universal, Kind::Dmg, 3)]
    );
    let rows: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT app, version, count FROM download_counts ORDER BY version")
            .fetch_all(&db)
            .await
            .unwrap();
    assert_eq!(rows, vec![("ebirforms".into(), "1.2.0".into(), 2), ("ebirforms".into(), "1.3.0".into(), 1)]);
}

/// The download page's platform cards, each up to its end.
fn platform_cards(page: &str) -> Vec<&str> {
    page.split("<li class=\"panel dl-card").skip(1).map(|card| card.split("</li></ul>").next().unwrap()).collect()
}

fn card<'a>(cards: &[&'a str], name: &str) -> &'a str {
    cards.iter().find(|card| card.contains(&format!("<h2>{name}</h2>"))).unwrap()
}

/// The home page hero's download, up to the platform logos.
fn hero_downloads(home: &str) -> &str {
    let hero = &home[home.find(r#"<section id="hero""#).unwrap()..];
    &hero[..hero.find(r#"<div class="platforms"#).unwrap()]
}

/// The home page hero's row of platform logos.
fn platform_row(home: &str) -> &str {
    let row = &home[home.find(r#"<div class="platforms"#).unwrap()..];
    &row[..row.find("</div>").unwrap()]
}

/// The Linux install command's row: selectable text, and a Copy button the
/// script shows where it can copy.
const INSTALL_LINE: &str = r#"<div class="install-line" data-copy-group=""><code>curl -fsSL https://ebirforms.buwiz.com/install-linux.sh | sh</code><button type="button" data-copy="curl -fsSL https://ebirforms.buwiz.com/install-linux.sh | sh" hidden="">Copy</button><span class="copy-status" role="status"></span></div>"#;

const MAC: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Safari/605.1.15";
const WINDOWS: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/130.0 Safari/537.36";
const LINUX: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0";
const ANDROID: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 Mobile Safari/537.36";
const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148";
const CHROMEBOOK: &str = "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 Chrome/130.0 Safari/537.36";

#[sqlx::test(migrations = "./migrations")]
async fn the_download_page_lists_the_release_and_highlights_the_visitors_platform(db: PgPool) {
    let (releases, url) = FakeReleases::start().await;
    releases.set_latest(&latest_json(&[]));
    let site = Site::with_downloads(db.clone(), &url);
    let page = text(browse(&site, "/download", MAC).await).await;
    assert!(page.contains("<title>Download Buwiz for Windows, macOS, Linux and Android</title>"), "{page}");
    assert!(page.contains(r#"<link rel="canonical" href="https://ebirforms.buwiz.com/download">"#));
    assert!(!page.contains("noindex"));
    assert!(page.contains("Version 1.2.0, released November 2, 2026."));
    // One card per platform, the Mac one marked as the visitor's, each with
    // one button through the counted route, its version and size, and its
    // SHA-256 in small print.
    let cards = platform_cards(&page);
    assert_eq!(cards.len(), 5);
    assert_eq!(cards.iter().filter(|card| card.starts_with(" yours\"")).count(), 1);
    let mac = card(&cards, "macOS");
    assert!(mac.starts_with(r#" yours" id="mac">"#), "{mac}");
    assert!(mac.contains(r#"<span class="dl-yours">Your device</span>"#), "{mac}");
    for (name, href, label, facts, sha) in [
        ("macOS", "/download/mac", "Download for Mac", "Version 1.2.0 · .dmg · 84.2 MB", SHA_MAC),
        (
            "Windows",
            "/download/windows",
            "Download for Windows",
            "Version 1.2.0 · .exe · x64 · 60.0 MB",
            &"0".repeat(64),
        ),
        ("Linux", "/download/linux", "Download for Linux", "Version 1.2.0 · .deb · x64 · 55.0 MB", SHA_DEB),
    ] {
        let card = card(&cards, name);
        assert_eq!(card.matches("<a ").count(), 1, "one link: {card}");
        assert!(card.contains(&format!(r#"<a class="btn btn-solid" href="{href}">"#)), "{card}");
        assert!(card.contains(&format!("</span>{label}</a>")), "{card}");
        assert!(card.contains(&format!(r#"<p class="dl-facts">{facts}</p>"#)), "{card}");
        assert!(card.contains(&format!("<code>{sha}</code>")), "{card}");
    }
    // Android has no file yet; iOS will come from the App Store.
    let android = card(&cards, "Android");
    assert!(
        android.contains(r#"<span class="soon-tag">Coming soon</span>"#) && !android.contains("href="),
        "{android}"
    );
    let ios = card(&cards, "iOS");
    assert!(ios.contains("Coming to the App Store") && !ios.contains("href=") && !ios.contains("btn"), "{ios}");
    assert!(!page.contains("Buwiz.ipa") && !page.contains("/download/android"));
    assert!(!page.contains(&url), "never straight to the bucket");
    for gone in
        ["<form", "<fieldset", r#"type="radio""#, "dl-terminal", "data-copy", "<script src=\"/assets/download.js"]
    {
        assert!(!page.contains(gone), "{gone}");
    }

    let sitemap = text(site.on_host(FORMS_HOST, "/sitemap.xml").await).await;
    assert!(
        sitemap.contains("<loc>https://ebirforms.buwiz.com/download</loc>\n    <lastmod>2026-11-02</lastmod>"),
        "{sitemap}"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn the_home_page_has_one_download_button_for_the_visitors_device(db: PgPool) {
    let (releases, url) = FakeReleases::start().await;
    releases.set_latest(&latest_json(&[]));
    let site = Site::with_downloads(db.clone(), &url);
    let button = |href: &str, label: &str| {
        format!(
            r#"<a class="btn btn-solid btn-get" href="{href}"><span class="icon" aria-hidden="true">download</span>{label}</a>"#
        )
    };
    let other = r#"<a class="get-other" href="/download">Other platforms</a>"#;
    let check = async |site: &Site, agent: &str, href: &str, label: &str| {
        let home = text(browse(site, "/", agent).await).await;
        let hero = hero_downloads(&home);
        assert!(hero.contains(&button(href, label)), "{agent}: {hero}");
        assert_eq!(hero.matches("<a ").count(), if href == "/download" { 1 } else { 2 }, "{agent}: {hero}");
        assert_eq!(hero.contains(other), href != "/download", "{agent}: {hero}");
        // The header has the account; the hero has none of it.
        // No install script in this release, so no command to copy either.
        for gone in
            ["/signup", "/login", "/tins", "Other downloads", "Not Ubuntu", "Chromebook", "data-copy", "<button"]
        {
            assert!(!hero.contains(gone), "{agent}: {gone}: {hero}");
        }
        // The logos only show where Buwiz runs: no links, nothing to click.
        let row = platform_row(&home);
        assert!(!row.contains("<a ") && !row.contains("href="), "{agent}: {row}");
        assert!(row.contains(r#"<li class="platform" title="Windows">"#), "{row}");
        assert!(row.contains(r#"<li class="platform soon" title="Android, coming soon">"#), "{row}");
        assert!(
            row.contains(
                r#"<p class="visually-hidden">Available for Windows, macOS and Linux; Android and iOS coming soon</p>"#
            ),
            "{row}"
        );
        assert!(!home.contains("download.js") && !home.contains("alert(") && !home.contains("confirm("), "{agent}");
    };
    check(&site, MAC, "/download/mac", "Download for Mac").await;
    check(&site, WINDOWS, "/download/windows", "Download for Windows").await;
    check(&site, LINUX, "/download/linux", "Download for Linux").await;
    // No file of their own: the download page.
    for agent in [IPHONE, CHROMEBOOK, ANDROID, "curl/8.7.1"] {
        check(&site, agent, "/download", "Download").await;
    }

    // With an APK out, Android gets its button.
    releases.set_latest(&latest_json(&later_files()));
    let site = Site::with_downloads(db.clone(), &url);
    let home = text(browse(&site, "/", ANDROID).await).await;
    assert!(hero_downloads(&home).contains(&button("/download/android", "Download for Android")), "{home}");
    assert!(platform_row(&home).contains("Available for Windows, macOS, Linux and Android; iOS coming soon"));
    assert!(!home.contains("install-line"), "Linux only");
    // With the install script out, a Linux visitor gets its command to copy
    // under the button; nobody else does.
    let home = text(browse(&site, "/", LINUX).await).await;
    let hero = hero_downloads(&home);
    assert!(hero.contains(&button("/download/linux", "Download for Linux")), "{hero}");
    assert!(hero.contains(INSTALL_LINE), "{hero}");
    for agent in [MAC, WINDOWS, CHROMEBOOK] {
        assert!(!text(browse(&site, "/", agent).await).await.contains("install-line"), "{agent}");
    }

    // Signed in, the hero still has just the download; the header has My TINs.
    let (_, cookie) = site.person("owner@example.com").await;
    let request = Request::get("/")
        .header(header::HOST, FORMS_HOST)
        .header(header::USER_AGENT, MAC)
        .header(header::COOKIE, &cookie)
        .body(Body::empty())
        .unwrap();
    let home = text(site.send(request).await).await;
    let hero = hero_downloads(&home);
    assert!(!hero.contains("/tins") && hero.matches("<a ").count() == 2, "{hero}");
    let header = &home[..home.find("<main").unwrap()];
    assert!(header.contains(r#"<a href="/tins" class="">My TINs</a>"#), "{header}");
}

#[sqlx::test(migrations = "./migrations")]
async fn the_download_page_lists_other_files_as_plain_links(db: PgPool) {
    let (releases, url) = FakeReleases::start().await;
    releases.set_latest(&latest_json(&later_files()));
    let site = Site::with_downloads(db.clone(), &url);
    let page = text(browse(&site, "/download", LINUX).await).await;
    let cards = platform_cards(&page);

    // Linux: the x64 deb is the button; every other file a small link
    // through this site, which counts it. No choices, no terminal.
    let linux = card(&cards, "Linux");
    assert!(linux.starts_with(r#" yours" id="linux">"#), "{linux}");
    assert!(linux.contains(r#"<a class="btn btn-solid" href="/download/linux">"#), "{linux}");
    assert_eq!(linux.matches("btn-solid").count(), 1, "{linux}");
    for (label, route) in [
        (".deb · ARM64", "/download/linux?kind=deb&amp;arch=arm64"),
        (".rpm · x64", "/download/linux?kind=rpm&amp;arch=x64"),
        (".rpm · ARM64", "/download/linux?kind=rpm&amp;arch=arm64"),
        (".tar.gz · x64", "/download/linux?kind=targz&amp;arch=x64"),
        (".tar.gz · ARM64", "/download/linux?kind=targz&amp;arch=arm64"),
        ("Install script", "/install-linux.sh"),
    ] {
        assert!(linux.contains(&format!(r#"<a href="{route}">{label}</a>"#)), "{label}: {linux}");
    }
    assert!(!linux.contains("/download/linux?kind=deb&amp;arch=x64"), "the button's file is not listed again");
    for gone in ["<form", "<fieldset", r#"type="radio""#, "dl-terminal"] {
        assert!(!linux.contains(gone), "{gone}: {linux}");
    }
    // The install script's command, on one line, to copy.
    assert!(linux.contains(INSTALL_LINE), "{linux}");
    assert_eq!(page.matches("install-line").count(), 1, "Linux only: {page}");

    // Windows: the installer is the button, the MSI a small link.
    let windows = card(&cards, "Windows");
    assert!(windows.contains(r#"<a class="btn btn-solid" href="/download/windows">"#), "{windows}");
    assert!(
        windows.contains(r#"<a href="/download/windows?kind=msi&amp;arch=x64">.msi (for IT) · x64</a>"#),
        "{windows}"
    );
    let android = card(&cards, "Android");
    assert!(android.contains(r#"<a class="btn btn-solid" href="/download/android">"#), "{android}");
    assert!(!page.contains(&url), "never straight to the bucket");

    // A Linux release without an x64 deb: the button is its best file.
    let other = "0".repeat(64);
    let only_rpm = serde_json::json!({
        "version": "1.2.0",
        "files": [release_file("linux", "x64", "rpm", "Buwiz-1.2.0-linux-x64.rpm", 1, &other)],
    });
    releases.set_latest(only_rpm.to_string().as_bytes());
    let site = Site::with_downloads(db.clone(), &url);
    let page = text(browse(&site, "/download", LINUX).await).await;
    let linux = card(&platform_cards(&page), "Linux");
    assert!(linux.contains(r#"<a class="btn btn-solid" href="/download/linux?kind=rpm&amp;arch=x64">"#), "{linux}");
    assert!(!linux.contains("install-line"), "no script released: {linux}");
    // And the home page sends a Linux visitor to the download page.
    let home = text(browse(&site, "/", LINUX).await).await;
    assert!(hero_downloads(&home).contains(r#"<a class="btn btn-solid btn-get" href="/download">"#), "{home}");
}

/// What every page says while nothing is released.
async fn assert_coming_soon(site: &Site, why: &str) {
    let response = download_from(site, "/download/windows").await;
    assert_eq!(response.status(), StatusCode::FOUND, "{why}");
    assert_eq!(location(&response), "/download", "{why}");
    assert_eq!(cache_control(&response), "no-store", "{why}");
    let script = browse(site, "/install-linux.sh", LINUX).await;
    assert_eq!(location(&script), "/download#linux", "{why}");

    let page = download_from(site, "/download").await;
    assert_eq!(page.status(), StatusCode::OK, "{why}");
    let page = text(page).await;
    assert!(page.contains("Buwiz is coming soon to Windows, macOS, Linux and Android"), "{why}");
    assert_eq!(page.matches(r#"<span class="soon-tag">Coming soon</span>"#).count(), 4, "{why}");
    assert!(page.contains("Coming to the App Store"), "{why}");
    assert!(!page.contains("href=\"/download/") && !page.contains("releases/v"), "{why}");
    assert!(page.contains(r#"<meta name="robots" content="noindex">"#), "{why}");

    let home = text(site.on_host(FORMS_HOST, "/").await).await;
    assert!(
        home.contains(r#"<div class="platforms rise d3"><span class="soon-tag">Coming soon</span><p class="visually-hidden">Coming soon to Windows, macOS, Linux, Android and iOS</p><ul aria-hidden="true"><li class="platform" title="Windows, coming soon"><svg"#),
        "{why}"
    );
    assert!(!home.contains("/download") && !home.contains("btn-get"), "{why}");

    let sitemap = text(site.on_host(FORMS_HOST, "/sitemap.xml").await).await;
    assert!(!sitemap.contains("/download"), "{why}");
}

#[sqlx::test(migrations = "./migrations")]
async fn nothing_is_released_until_latest_json_exists(db: PgPool) {
    let (_releases, url) = FakeReleases::start().await;
    assert_coming_soon(&Site::with_downloads(db, &url), "no latest.json").await;
}

#[sqlx::test(migrations = "./migrations")]
async fn a_broken_or_unreachable_manifest_means_coming_soon(db: PgPool) {
    assert_coming_soon(&Site::new(db.clone()), "unreachable").await;
    let sha = "0".repeat(64);
    let path_trick = serde_json::json!({
        "version": "1.2.0",
        "files": [{"platform": "windows", "arch": "x64", "kind": "msi", "file": "../../evil.msi", "size": 1, "sha256": sha}],
    })
    .to_string();
    let bad_version = String::from_utf8(latest_json(&[])).unwrap().replace("\"1.2.0\"", "\"1.2.0/../..\"");
    for (latest, why) in
        [("{\"version\":", "broken JSON"), (path_trick.as_str(), "path trick"), (bad_version.as_str(), "bad version")]
    {
        let (releases, url) = FakeReleases::start().await;
        releases.set_latest(latest.as_bytes());
        assert_coming_soon(&Site::with_downloads(db.clone(), &url), why).await;
    }
}

/// What each of the filing site's routes answers on `host`: (path, status,
/// `Location`).
async fn answers(site: &Site, host: &str) -> Vec<(&'static str, StatusCode, String)> {
    let mut answers = Vec::new();
    for path in [
        "/",
        "/download",
        "/download/mac?arch=arm64",
        "/install-linux.sh",
        "/assets/demo.js",
        "/assets/screens/palette_dark.webp",
        "/assets/site.css",
        "/login",
        "/signup",
        "/tins",
        "/account",
        "/robots.txt",
        "/sitemap.xml",
        "/site.webmanifest",
    ] {
        let response = site.on_host(host, path).await;
        answers.push((path, response.status(), location(&response)));
    }
    answers
}

#[sqlx::test(migrations = "./migrations")]
async fn the_filing_site_answers_on_its_own_host(db: PgPool) {
    let site = Site::new(db);
    let ok = |path| (path, StatusCode::OK, String::new());
    let expected = vec![
        ok("/"),
        ok("/download"),
        ("/download/mac?arch=arm64", StatusCode::FOUND, "/download".to_owned()),
        ("/install-linux.sh", StatusCode::NOT_FOUND, String::new()),
        ok("/assets/demo.js"),
        ok("/assets/screens/palette_dark.webp"),
        ok("/assets/site.css"),
        ok("/login"),
        ok("/signup"),
        ("/tins", StatusCode::TEMPORARY_REDIRECT, "/login?next=%2Ftins&notice=sign_in_first".to_owned()),
        ("/account", StatusCode::PERMANENT_REDIRECT, "/tins".to_owned()),
        ok("/robots.txt"),
        ok("/sitemap.xml"),
        ok("/site.webmanifest"),
    ];
    for host in ["forms.buwiz.com", "ebirforms.buwiz.com"] {
        assert_eq!(answers(&site, host).await, expected, "{host}");
        let home = text(site.on_host(host, "/").await).await;
        assert!(home.contains("<title>Buwiz: Philippine tax filing, made modern</title>"), "{host}");
        assert!(!home.contains("Buwiz Bot"), "{host}");
        assert!(home.contains(r#"<link rel="canonical" href="https://ebirforms.buwiz.com/">"#), "{host}");
        assert!(
            home.contains(
                r#"<meta property="og:image" content="https://ebirforms.buwiz.com/assets/brand/og-image.png">"#
            ),
            "{host}"
        );
    }
    let robots = text(site.on_host("ebirforms.buwiz.com", "/robots.txt").await).await;
    assert!(robots.ends_with("Sitemap: https://ebirforms.buwiz.com/sitemap.xml\n"), "{robots}");
    let sitemap = text(site.on_host("ebirforms.buwiz.com", "/sitemap.xml").await).await;
    assert!(sitemap.contains("<loc>https://ebirforms.buwiz.com/</loc>"), "{sitemap}");
    // The account and API hosts are as they were.
    assert_eq!(location(&site.on_host("account.buwiz.com", "/").await), "/account");
    assert_eq!(site.on_host("account.buwiz.com", "/login").await.status(), StatusCode::OK);
    assert_eq!(site.on_host("api.buwiz.com", "/").await.status(), StatusCode::OK);
}

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
        r#"<meta name="description" content="Give your AI a job and a computer you host."#,
        // Drawn already, for crawlers that do not run the app.
        r#"<div id="prerender">"#,
        "<h1",
        "<footer",
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
        r#"<link rel="canonical" href="https://buwiz.com/mini-apps" />"#,
        r#"<meta name="buwiz-forms-origin" content="https://ebirforms.buwiz.com" />"#,
    ] {
        assert!(page.contains(part), "{part}: {page}");
    }
    assert!(!page.contains("Meet Buwiz Bot"), "{page}");
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

#[sqlx::test(migrations = "./migrations")]
async fn sign_in_links_go_to_the_account_host(db: PgPool) {
    let site = Site::new(db);
    let home = text(site.on_host(FORMS_HOST, "/").await).await;
    let header = home.split("<header").nth(1).unwrap().split("</header>").next().unwrap();
    assert!(header.contains(r#"<a class="btn btn-ghost" href="http://localhost:3008/login">Sign in</a>"#), "{header}");
    assert!(header.contains(r#"<a class="btn btn-solid" href="http://localhost:3008/signup">"#), "{header}");
    let account = home.split(r#"<section id="account""#).nth(1).unwrap().split("</section>").next().unwrap();
    assert!(account.contains(r#"href="http://localhost:3008/signup""#), "{account}");
    assert!(account.contains(r#"href="http://localhost:3008/login""#), "{account}");
    assert!(!account.contains(r#"href="/login""#) && !account.contains(r#"href="/signup""#), "{account}");
    // The sign-in pages' own switch links stay on their host.
    let login = text(site.get("/login", "").await).await;
    let card = login.split("</header>").nth(1).unwrap();
    assert!(card.contains(r#"href="/signup"#), "{card}");
}

#[sqlx::test(migrations = "./migrations")]
async fn download_counts_are_kept_per_app(db: PgPool) {
    let app: String =
        sqlx::query_scalar("SELECT column_default FROM information_schema.columns WHERE table_name = 'download_counts' AND column_name = 'app'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(app, "'ebirforms'::text");
    let key: Vec<String> = sqlx::query_scalar(
        "SELECT a.attname::text FROM pg_index i
         JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey)
         WHERE i.indrelid = 'download_counts'::regclass AND i.indisprimary
         ORDER BY array_position(i.indkey, a.attnum)",
    )
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(key, ["app", "day", "version", "platform", "arch", "kind"]);

    let (releases, url) = FakeReleases::start().await;
    releases.set_latest(&latest_json(&[]));
    let site = Site::with_downloads(db.clone(), &url);
    let person = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Safari/605.1.15";
    let request = Request::get("/download/mac").header(header::HOST, FORMS_HOST).header(header::USER_AGENT, person);
    assert_eq!(site.send(request.body(Body::empty()).unwrap()).await.status(), StatusCode::FOUND);
    // Another app's count of the same file and day is its own row, and not
    // in this app's totals.
    sqlx::query(
        "INSERT INTO download_counts (app, day, version, platform, arch, kind, count)
         SELECT 'bot', day, version, platform, arch, kind, 5 FROM download_counts",
    )
    .execute(&db)
    .await
    .unwrap();
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT app, count FROM download_counts ORDER BY app").fetch_all(&db).await.unwrap();
    assert_eq!(rows, [("bot".to_owned(), 5), ("ebirforms".to_owned(), 1)]);
    assert_eq!(
        downloads::totals(&db, downloads::APP).await.unwrap(),
        [Total { platform: Platform::Mac, arch: Arch::Universal, kind: Kind::Dmg, count: 1 }]
    );
}
