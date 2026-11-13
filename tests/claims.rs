//! TIN claims against Postgres: the papers each kind of TIN needs, sending
//! a claim back and resending it, and ending claims and changing their
//! files.
//! Each test gets its own database from `#[sqlx::test]`, so DATABASE_URL
//! must point at a Postgres the tests may create databases in
//! (`docker compose up -d`, then the URL from `.env.example`).

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::{Body, Bytes};
use axum::http::{Method, Request, StatusCode, Uri, header};
use axum::response::IntoResponse;
use buwiz_cloud::base::storage::{DocumentStore, ObjectStore};
use serde_json::Value;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use common::Part::{File, Text};
use common::*;

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
    // Staff made it active with its papers, and it says since when.
    let (verified_by, decided): (Option<String>, bool) =
        sqlx::query_as("SELECT verified_by, decided_at IS NOT NULL FROM tin_claims WHERE id = $1")
            .bind(claim)
            .fetch_one(&site.db)
            .await
            .unwrap();
    assert_eq!((verified_by.as_deref(), decided), (Some("staff"), true));
    assert_eq!(site.readings(claim).await, []);
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
            "xml": common::iaf_return("frm1601c", "11111111100000", "2025"),
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

/// Each review decision leaves its own audit line (who, when, what, from
/// where), so a second send-back or the claimant deleting the claim does
/// not erase who decided.
#[sqlx::test(migrations = "./migrations")]
async fn every_review_decision_has_an_audit_line_that_outlives_the_claim(db: PgPool) {
    let site = Site::new(db);
    let (_, support) = site.staff_with_role("helper@example.test", "support").await;
    let (_, staff) = site.person(STAFF).await;
    let (cora, cora_cookie) = site.person("cora@example.test").await;
    site.upload("/tins/claims", &cora_cookie, &BUSINESS).await;
    let claim = site.newest_claim(cora).await;
    let decide = format!("/staff/claims/{claim}/decision");
    let resend = format!("/tins/claims/{claim}/resend");
    let fix = [("registered_name", Text("Cora Trading Inc")), ("supporting", File("more.pdf", PDF))];

    let response = site.post(&decide, &support, "decision=fix&reasons=blurry&note=Retake+it").await;
    assert_eq!(location(&response), "/staff?notice=claim_sent_back");
    site.upload(&resend, &cora_cookie, &fix).await;
    let response = site.post(&decide, &staff, "decision=fix&reasons=name_mismatch").await;
    assert_eq!(location(&response), "/staff?notice=claim_sent_back");
    let response = site.post(&decide, &support, "decision=reject&note=Not+the+company%27s+papers").await;
    assert_eq!(location(&response), "/staff?notice=claim_decided");
    let response = site.post(&format!("/tins/claims/{claim}/delete"), &cora_cookie, "").await;
    assert_eq!(location(&response), "/tins?notice=claim_deleted");
    assert!(!claim_exists(&site.db, claim).await);

    let lines = claim_audit_lines(&site.db, cora).await;
    let lines: Vec<(&str, &str, &str, &str, Option<&str>)> = lines
        .iter()
        .map(|line| (line.0.as_str(), line.1.as_str(), line.2.as_str(), line.3.as_str(), line.4.as_deref()))
        .collect();
    assert_eq!(
        lines,
        [
            ("sent_back", "pending", "helper@example.test", "web", Some("Blurry photo: Retake it")),
            ("sent_back", "pending", STAFF, "web", Some("Name doesn't match")),
            // Rejected while waiting for the second fix.
            ("rejected", "needs_fix", "helper@example.test", "web", Some("Not the company's papers")),
            ("deleted", "rejected", "cora@example.test", "web", None),
        ]
    );
    // The staff history names who decided, the claim long gone.
    let history = text(site.get(&format!("/staff/users/{cora}"), &staff).await).await;
    assert!(history.contains("Staff helper@example.test rejected a claim on ***-***-321-00000"), "{history}");
    assert!(history.contains("Staff helper@example.test sent ***-***-321-00000 back for a fix"), "{history}");

    // An approval has its line too.
    site.upload("/tins/claims", &cora_cookie, &BUSINESS).await;
    let again = site.newest_claim(cora).await;
    let response = site.post(&format!("/staff/claims/{again}/decision"), &support, "decision=approve").await;
    assert_eq!(location(&response), "/staff?notice=claim_decided");
    let last = claim_audit_lines(&site.db, cora).await.pop().unwrap();
    assert_eq!(
        (last.0.as_str(), last.1.as_str(), last.2.as_str(), last.3.as_str(), last.4),
        ("approved", "pending", "helper@example.test", "web", None)
    );
}
