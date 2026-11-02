//! The account site against Postgres: the staff pages, old links, the
//! licence gate's opt-in for open claims, signing an account out
//! everywhere, the sign-in pages, and who may use `/mcp`.
//! Each test gets its own database from `#[sqlx::test]`, so DATABASE_URL
//! must point at a Postgres the tests may create databases in
//! (`docker compose up -d`, then the URL from `.env.example`).

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use common::Part::{File, Text};
use common::*;

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

#[sqlx::test(migrations = "./migrations")]
async fn only_an_agent_token_may_use_mcp(db: PgPool) {
    let site = Site::new(db);
    let (ana, _) = site.person("ana@example.test").await;
    const LIST: &str = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    const CALL: &str = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_tin_claims"}}"#;
    let call = |token: Option<&str>, body: &'static str| {
        let mut request = Request::post("/mcp").header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        request.body(Body::from(body)).unwrap()
    };
    let challenge = |response: &axum::response::Response| {
        response.headers().get(header::WWW_AUTHENTICATE).unwrap().to_str().unwrap().to_owned()
    };
    add_claim(&site.db, ana, "123456789", "approved").await;

    for body in [LIST, CALL] {
        let response = site.send(call(None, body)).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{body}");
        assert!(challenge(&response).contains(r#"error="invalid_token""#), "{body}");
    }

    for (client, scope) in [("buwiz-desktop", "app"), ("buwiz-books", "books")] {
        let token = token(&site.db, ana, client, scope).await;
        for body in [LIST, CALL] {
            let response = site.send(call(Some(&token), body)).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{scope} {body}");
            let challenge = challenge(&response);
            assert!(challenge.contains(r#"error="insufficient_scope""#), "{scope} {body}");
            assert!(challenge.contains(r#"scope="mcp""#), "{scope} {body}");
            assert!(!text(response).await.contains("123"), "{scope} {body}");
        }
    }

    let token = token(&site.db, ana, "an-agent", "mcp").await;
    let response = site.send(call(Some(&token), LIST)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(text(response).await.contains("list_tin_claims"));
    let response = site.send(call(Some(&token), CALL)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let answer: serde_json::Value = serde_json::from_str(&text(response).await).unwrap();
    let claims = &answer["result"]["structuredContent"]["claims"];
    assert_eq!(claims.as_array().map(Vec::len), Some(1), "{answer}");
    assert_eq!(claims[0]["status"], "approved", "{answer}");
}

#[sqlx::test(migrations = "./migrations")]
async fn only_the_bir_apps_see_a_returns_full_tin(db: PgPool) {
    let site = Site::new(db);
    let (ana, _) = site.person("ana@example.test").await;
    let claim = add_claim(&site.db, ana, "123456789", "approved").await;
    let filename = "12345678900000-1601Cv2018-062025#ana@example.test#.xml";
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO submissions (account_id, claim_id, form_type, period, filename, xml_sha256, status, remote_path, receipt_jws)
         VALUES ($1, $2, '1601Cv2018', '062025', $3, '\\x00', 'sent', '/in/' || $3, 'head.body.sig')
         RETURNING id",
    )
    .bind(ana)
    .bind(claim)
    .bind(filename)
    .fetch_one(&site.db)
    .await
    .unwrap();
    let get = |token: &str, path: String| {
        Request::get(path).header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap()
    };
    let mcp = |token: &str, tool: &str| {
        let body = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{tool}","arguments":{{"id":"{id}"}}}}}}"#
        );
        Request::post("/mcp")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(body))
            .unwrap()
    };

    let app = app_token(&site.db, ana).await;
    let full = text(site.send(get(&app, format!("/api/submissions/{id}"))).await).await;
    assert!(full.contains(filename) && full.contains("head.body.sig"), "{full}");

    let books = token(&site.db, ana, "buwiz-books", "books").await;
    let agent = token(&site.db, ana, "an-agent", "mcp").await;
    let answers = [
        text(site.send(get(&books, "/api/submissions".to_owned())).await).await,
        text(site.send(get(&books, format!("/api/submissions/{id}"))).await).await,
        text(site.send(mcp(&agent, "list_submissions")).await).await,
        text(site.send(mcp(&agent, "get_submission")).await).await,
    ];
    for answer in answers {
        assert!(answer.contains("***-***-789-00000-1601Cv2018-062025.xml"), "{answer}");
        for secret in ["123456789", "ana@example.test", "head.body.sig", "/in/"] {
            assert!(!answer.contains(secret), "{secret} in {answer}");
        }
    }
}
