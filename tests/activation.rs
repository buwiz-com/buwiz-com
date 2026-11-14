//! An Admin activates a TIN without its papers, with a reason that only
//! staff ever read.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use common::Part::{File, Text};
use common::*;

const REASON: &str = "Papers seen in person at the office";
const REASON_FORM: &str = "reason=Papers+seen+in+person+at+the+office";

/// (status, verified_by, decided_at set, decision_note)
async fn decided(db: &PgPool, claim: Uuid) -> (String, Option<String>, bool, Option<String>) {
    sqlx::query_as("SELECT status, verified_by, decided_at IS NOT NULL, decision_note FROM tin_claims WHERE id = $1")
        .bind(claim)
        .fetch_one(db)
        .await
        .unwrap()
}

async fn skip(site: &Site, cookie: &str, tin: &'static str) {
    let parts = [
        ("kind", Text("personal")),
        ("tin", Text(tin)),
        ("registered_name", Text("Juan Dela Cruz")),
        ("skip", Text("1")),
    ];
    assert_eq!(location(&site.upload("/tins/claims", cookie, &parts).await), "/tins?notice=tin_saved");
}

async fn mcp_claims(site: &Site, token: &str) -> String {
    let request = Request::post("/mcp")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_tin_claims"}}"#))
        .unwrap();
    let response = site.send(request).await;
    assert_eq!(response.status(), StatusCode::OK);
    text(response).await
}

async fn tax_profiles(site: &Site, token: &str, query: &str) -> (StatusCode, String) {
    let request = Request::get(format!("/api/tax-profiles{query}"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = site.send(request).await;
    (response.status(), text(response).await)
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_activates_their_own_tin_without_papers(db: PgPool) {
    let site = Site::new(db);
    let (boss, boss_cookie) = site.staff_with_role("boss@example.test", "admin").await;
    skip(&site, &boss_cookie, "000123453").await;
    let claim = site.newest_claim(boss).await;

    let page = text(site.get(&format!("/staff/claims/{claim}"), &boss_cookie).await).await;
    assert!(page.contains("No papers yet. Added without the upload on"), "{page}");
    assert!(page.contains("The person can prepare returns but not send them."), "{page}");
    assert!(page.contains(&format!("action=\"/staff/claims/{claim}/activate\"")), "{page}");
    assert!(page.contains("Activate 000-123-453-00000 for boss@example.test without papers?"), "{page}");

    let path = format!("/staff/claims/{claim}/activate");
    let back = format!("/staff/claims/{claim}");
    let response = site.post(&path, &boss_cookie, "reason=+").await;
    assert_eq!(location(&response), format!("{back}?notice=reason_missing"));
    assert_eq!(site.claim_status(claim).await, "unverified");

    let response = site.post(&path, &boss_cookie, REASON_FORM).await;
    assert_eq!(location(&response), format!("{back}?notice=tin_activated"));
    assert_eq!(decided(&site.db, claim).await, ("approved".into(), Some("admin".into()), true, None));
    let lines = claim_audit_lines(&site.db, boss).await;
    assert_eq!(
        lines,
        [("activated".into(), "unverified".into(), "boss@example.test".into(), "web".into(), Some(REASON.into()))]
    );

    // The apps' licence list answers with it, active and able to send.
    let token = app_token(&site.db, boss).await;
    let (status, body) = tax_profiles(&site, &token, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let profiles: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(profiles["profiles"][0]["tin"], "000-123-453-00000", "{body}");
    let (status, body) = tax_profiles(&site, &token, "?include=all,pending").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let all: Value = serde_json::from_str(&body).unwrap();
    assert_eq!((&all["profiles"][0]["status"], &all["profiles"][0]["can_send"]), (&"approved".into(), &true.into()));

    // The reason is for staff: not on My TINs, not in the TIN list, not
    // for an AI agent.
    assert!(!body.contains(REASON));
    assert!(!text(site.get("/tins", &boss_cookie).await).await.contains(REASON));
    let agent = token_for(&site, boss, "mcp").await;
    let claims = mcp_claims(&site, &agent).await;
    assert!(claims.contains("\"approved\"") && !claims.contains(REASON), "{claims}");
    // Another staff member reads it as a staff member activating their own
    // TIN, flagged in the queue and on the claim; so does boss, on their
    // own page.
    let (_, staff) = site.person(STAFF).await;
    let history = text(site.get(&format!("/staff/users/{boss}"), &staff).await).await;
    assert!(
        history.contains(&format!(
            "Staff boss@example.test activated their own TIN ***-***-453-00000 without papers: {REASON}"
        )),
        "{history}"
    );
    assert!(!history.contains("The person activated"), "{history}");
    let queue = text(site.get("/staff/queue?status=approved", &staff).await).await;
    assert!(list_part(&queue).contains("Activated by its owner, an admin"), "{queue}");
    let review = text(site.get(&format!("/staff/claims/{claim}"), &staff).await).await;
    assert!(review.contains("Activated by its owner, an admin"), "{review}");
    let own = text(site.get(&format!("/staff/users/{boss}"), &boss_cookie).await).await;
    assert!(own.contains(&format!("activated their own TIN ***-***-453-00000 without papers: {REASON}")), "{own}");

    // An Admin may decide their own papers too; the page says it is theirs.
    let parts = [
        ("kind", Text("personal")),
        ("tin", Text("000123454")),
        ("registered_name", Text("Juan Dela Cruz")),
        ("tin_id", File("tin.png", PNG)),
    ];
    site.upload("/tins/claims", &boss_cookie, &parts).await;
    let papers = site.newest_claim(boss).await;
    let page = text(site.get(&format!("/staff/claims/{papers}"), &boss_cookie).await).await;
    assert!(page.contains("This is your own claim. Your decision goes in your history"), "{page}");
    assert!(page.contains("Or activate without papers"), "{page}");
    // Approving it takes a written reason (tests/own_page.rs).
    let response = site.post(&format!("/staff/claims/{papers}/decision"), &boss_cookie, "decision=approve").await;
    assert_eq!(location(&response), format!("/staff/claims/{papers}?notice=own_needs_reason"));
    let response =
        site.post(&format!("/staff/claims/{papers}/decision"), &boss_cookie, "decision=approve&note=Mine").await;
    assert_eq!(location(&response), "/staff/queue?notice=claim_decided");
    let history = text(site.get(&format!("/staff/users/{boss}"), &staff).await).await;
    assert!(
        history.contains("Staff boss@example.test approved their own claim on ***-***-454-00000: Mine"),
        "{history}"
    );
}

async fn token_for(site: &Site, account: Uuid, scope: &str) -> String {
    token(&site.db, account, "an-agent", scope).await
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_activates_someone_elses_waiting_tin_and_its_papers_go(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, ana_cookie) = site.person("ana@example.test").await;
    let parts = [
        ("kind", Text("personal")),
        ("tin", Text("000123455")),
        ("registered_name", Text("Ana Reyes")),
        ("tin_id", File("tin.png", PNG)),
    ];
    site.upload("/tins/claims", &ana_cookie, &parts).await;
    let claim = site.newest_claim(ana).await;
    let response = site.post(&format!("/staff/claims/{claim}/activate"), &staff, REASON_FORM).await;
    assert_eq!(location(&response), format!("/staff/claims/{claim}?notice=tin_activated"));
    assert_eq!(decided(&site.db, claim).await, ("approved".into(), Some("admin".into()), true, None));
    let stored: i64 =
        sqlx::query_scalar("SELECT count(*) FROM claim_documents WHERE claim_id = $1 AND purged_at IS NULL")
            .bind(claim)
            .fetch_one(&site.db)
            .await
            .unwrap();
    assert_eq!(stored, 0);
    let token = app_token(&site.db, ana).await;
    let (status, body) = tax_profiles(&site, &token, "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("000-123-455-00000") && !body.contains(REASON), "{body}");
    let my_tins = text(site.get("/tins", &ana_cookie).await).await;
    assert!(my_tins.contains("Active since") && !my_tins.contains(REASON), "{my_tins}");
    // Once active, there is nothing left to activate.
    let again = site.post(&format!("/staff/claims/{claim}/activate"), &staff, REASON_FORM).await;
    assert_eq!(location(&again), format!("/staff/claims/{claim}?notice=already_decided"));
}

#[sqlx::test(migrations = "./migrations")]
async fn activation_is_refused_for_a_tin_owned_elsewhere_and_for_a_banned_account(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (carl, _) = site.person("carl@example.test").await;
    let (ana, ana_cookie) = site.person("ana@example.test").await;
    let owned = add_claim(&site.db, carl, "000123456", "approved").await;
    skip(&site, &ana_cookie, "000123456").await;
    let claim = site.newest_claim(ana).await;
    let response = site.post(&format!("/staff/claims/{claim}/activate"), &staff, REASON_FORM).await;
    assert_eq!(location(&response), format!("/staff/claims/{claim}?notice=tin_owned_elsewhere"));
    assert_eq!(site.claim_status(claim).await, "unverified");
    assert_eq!(site.claim_status(owned).await, "approved");
    assert!(claim_audit_lines(&site.db, ana).await.is_empty());

    // Ended elsewhere first, it can be activated here.
    let revoke = site.post(&format!("/staff/claims/{owned}/revoke"), &staff, "reason=Moved").await;
    assert_eq!(location(&revoke), format!("/staff/claims/{owned}?notice=ownership_revoked"));
    let response = site.post(&format!("/staff/claims/{claim}/activate"), &staff, REASON_FORM).await;
    assert_eq!(location(&response), format!("/staff/claims/{claim}?notice=tin_activated"));

    // A banned account gets no active TIN, by activation or by approval.
    let (dan, dan_cookie) = site.person("dan@example.test").await;
    skip(&site, &dan_cookie, "000123457").await;
    let unverified = site.newest_claim(dan).await;
    let parts = [
        ("kind", Text("personal")),
        ("tin", Text("000123458")),
        ("registered_name", Text("Dan Cruz")),
        ("tin_id", File("tin.png", PNG)),
    ];
    site.upload("/tins/claims", &dan_cookie, &parts).await;
    let pending = site.newest_claim(dan).await;
    sqlx::query("UPDATE accounts SET banned_at = now(), banned_by = id, ban_reason = 'Fraud' WHERE id = $1")
        .bind(dan)
        .execute(&site.db)
        .await
        .unwrap();
    let response = site.post(&format!("/staff/claims/{unverified}/activate"), &staff, REASON_FORM).await;
    assert_eq!(location(&response), format!("/staff/claims/{unverified}?notice=claimant_banned"));
    let response = site.post(&format!("/staff/claims/{pending}/decision"), &staff, "decision=approve").await;
    assert_eq!(location(&response), format!("/staff/claims/{pending}?notice=claimant_banned"));
    assert_eq!(
        (site.claim_status(unverified).await, site.claim_status(pending).await),
        ("unverified".into(), "pending".into())
    );
    // Rejecting stays possible.
    let response = site.post(&format!("/staff/claims/{pending}/decision"), &staff, "decision=reject&note=No").await;
    assert_eq!(location(&response), "/staff/queue?notice=claim_decided");
}
