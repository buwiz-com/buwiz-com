//! Test TINs: a made-up TIN (root 000-000-000) an Admin adds to a test
//! account, active at once. It is marked as a test TIN wherever it is
//! listed, and it never sends: the send gate refuses it (`tin_test_only`)
//! on every path, whatever the relay mode.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use buwiz_cloud::filing::gate;
use buwiz_cloud::filing::submissions::{self, RelayStart};
use buwiz_cloud::relay::Control;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use common::*;

const TEST_ONLY: &str = "This is a test TIN: you can prepare returns for it, but Buwiz never sends them to the BIR.";
const FORM: &str = "tin=000-000-000-00000&kind=personal&registered_name=Test+Taxpayer&reason=Trying+the+app";

/// A signed-in account with the test label: (id, cookie).
async fn test_account(site: &Site, email: &str) -> (Uuid, String) {
    let (id, cookie) = site.person(email).await;
    sqlx::query("UPDATE accounts SET test_account = true WHERE id = $1").bind(id).execute(&site.db).await.unwrap();
    (id, cookie)
}

async fn test_tins(db: &PgPool, account: Uuid) -> Vec<(Uuid, String, String, Option<String>, Option<Uuid>)> {
    sqlx::query_as(
        "SELECT id, branch_code, status, verified_by, decided_by FROM tin_claims
         WHERE account_id = $1 AND tin_root = '000000000' ORDER BY created_at",
    )
    .bind(account)
    .fetch_all(db)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_adds_a_test_tin_to_a_test_account_and_every_list_marks_it(db: PgPool) {
    let site = Site::new(db);
    let (admin, staff) = site.person(STAFF).await;
    let (ana, ana_cookie) = test_account(&site, "ana@example.test").await;
    let user = format!("/staff/users/{ana}");
    let page = text(site.get(&user, &staff).await).await;
    assert!(page.contains(&format!("action=\"{user}/test-tin\"")), "{page}");

    let response = site.post(&format!("{user}/test-tin"), &staff, FORM).await;
    assert_eq!(location(&response), format!("{user}?notice=test_tin_added"));
    let rows = test_tins(&site.db, ana).await;
    assert_eq!(rows.len(), 1);
    let (claim, branch, status, verified_by, decided_by) = rows[0].clone();
    assert_eq!(
        (branch.as_str(), status.as_str(), verified_by.as_deref(), decided_by),
        ("00000", "approved", Some("admin"), Some(admin))
    );
    assert_eq!(
        claim_audit_lines(&site.db, ana).await,
        [("activated".into(), "new".into(), STAFF.into(), "web".into(), Some("Trying the app".into()))]
    );

    // Staff pages: the account, the users list, the queue, the claim.
    let page = text(site.get(&user, &staff).await).await;
    assert!(page.contains("Test TIN") && page.contains("000-000-000-00000"), "{page}");
    assert!(page.contains("Test TIN 000-000-000-00000 added, active"), "{page}");
    let list = text(site.get("/staff/users", &staff).await).await;
    assert!(list.contains("000-000-000-00000 · test"), "{list}");
    let queue = text(site.get("/staff?status=approved", &staff).await).await;
    assert!(queue.contains("Test TIN: never sends"), "{queue}");
    let review = text(site.get(&format!("/staff/claims/{claim}"), &staff).await).await;
    assert!(review.contains("Test TIN: Buwiz never sends returns for it"), "{review}");
    // The owner's page.
    let tins = text(site.get("/tins", &ana_cookie).await).await;
    assert!(tins.contains("Test TIN") && tins.contains("Buwiz never sends them to the BIR"), "{tins}");

    // The apps' TIN list marks it and says it cannot send; the licence
    // list (TINs that can send) leaves it out.
    let token = app_token(&site.db, ana).await;
    let all = profiles(&site, &token, "/api/tax-profiles?include=all").await;
    assert_eq!(all.as_array().unwrap().len(), 1, "{all}");
    let row = &all[0];
    assert_eq!(
        (&row["test_only"], &row["status"], &row["can_send"], &row["message"], &row["tin"]),
        (&json!(true), &json!("test_only"), &json!(false), &json!(TEST_ONLY), &json!("000-000-000-00000")),
        "{row}"
    );
    assert!(!row["permissions"].as_array().unwrap().contains(&json!("filing.submit")), "{row}");
    for query in ["", "?include=pending"] {
        let listed = profiles(&site, &token, &format!("/api/tax-profiles{query}")).await;
        assert_eq!(listed, json!([]), "{query}");
    }
    // An AI agent's list of TIN claims marks it too.
    let agent = common::token(&site.db, ana, "an-agent", "mcp").await;
    let request = Request::post("/mcp")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {agent}"))
        .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_tin_claims"}}"#))
        .unwrap();
    let listed = text(site.send(request).await).await;
    assert!(listed.contains(r#"\"test_only\":true"#) || listed.contains(r#""test_only":true"#), "{listed}");
    // Other rows read as before: no `test_only` on them.
    add_claim(&site.db, ana, "000123453", "approved").await;
    let all = profiles(&site, &token, "/api/tax-profiles?include=all").await;
    assert!(all[0].get("test_only").is_none() && all[0]["can_send"] == json!(true), "{all}");
}

/// What the form refuses: an account without the test label, a TIN that
/// is valid or has the wrong shape, no name or reason, a test TIN already
/// there or on another account, a banned account; and anyone but an Admin.
#[sqlx::test(migrations = "./migrations")]
async fn a_test_tin_is_refused_off_a_test_account_and_for_a_real_tin(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, _) = test_account(&site, "ana@example.test").await;
    let (plain, _) = site.person("plain@example.test").await;
    let (_, helper) = site.staff_with_role("helper@example.test", "support").await;
    let (_, nobody) = site.person("nobody@example.test").await;

    let path = |id: Uuid| format!("/staff/users/{id}/test-tin");
    let back = |id: Uuid, notice: &str| format!("/staff/users/{id}?notice={notice}");
    let page = text(site.get(&format!("/staff/users/{plain}"), &staff).await).await;
    assert!(!page.contains("test-tin"), "{page}");
    for (id, form, notice) in [
        (plain, FORM.to_owned(), "not_test_account"),
        (ana, FORM.replace("000-000-000-00000", "000-123-453-00000"), "not_test_tin"),
        (ana, FORM.replace("000-000-000-00000", "000-000-00"), "tin_digits"),
        (ana, FORM.replace("000-000-000-00000", "000-000-000-12"), "tin_digits"),
        (ana, FORM.replace("Test+Taxpayer", "+"), "name_missing"),
        (ana, FORM.replace("Trying+the+app", "+"), "reason_missing"),
        (ana, FORM.replace("personal", "family"), "kind_missing"),
    ] {
        let response = site.post(&path(id), &staff, &form).await;
        assert_eq!(location(&response), back(id, notice), "{form}");
    }
    assert!(test_tins(&site.db, plain).await.is_empty() && test_tins(&site.db, ana).await.is_empty());
    assert_eq!(location(&site.post(&path(ana), &helper, FORM).await), back(ana, "admin_only"));
    assert_eq!(site.post(&path(ana), &nobody, FORM).await.status(), StatusCode::NOT_FOUND);
    assert!(test_tins(&site.db, ana).await.is_empty());

    assert_eq!(location(&site.post(&path(ana), &staff, FORM).await), back(ana, "test_tin_added"));
    assert_eq!(location(&site.post(&path(ana), &staff, FORM).await), back(ana, "test_tin_there"));
    let (bob, _) = test_account(&site, "bob@example.test").await;
    assert_eq!(location(&site.post(&path(bob), &staff, FORM).await), back(bob, "tin_owned_elsewhere"));
    // Another branch is another test TIN.
    let other_branch = FORM.replace("00000", "00007");
    assert_eq!(location(&site.post(&path(bob), &staff, &other_branch).await), back(bob, "test_tin_added"));
    sqlx::query("UPDATE accounts SET banned_at = now(), banned_by = id, ban_reason = 'Done' WHERE id = $1")
        .bind(bob)
        .execute(&site.db)
        .await
        .unwrap();
    let third = FORM.replace("00000", "00008");
    assert_eq!(location(&site.post(&path(bob), &staff, &third).await), back(bob, "claimant_banned"));
    assert_eq!(test_tins(&site.db, ana).await.len(), 1);
    assert_eq!(test_tins(&site.db, bob).await.len(), 1);
}

/// The test label stays on while the account holds a test TIN: revoke it
/// first.
#[sqlx::test(migrations = "./migrations")]
async fn the_test_label_stays_while_the_account_holds_a_test_tin(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, _) = test_account(&site, "ana@example.test").await;
    let user = format!("/staff/users/{ana}");
    site.post(&format!("{user}/test-tin"), &staff, FORM).await;
    let page = text(site.get(&user, &staff).await).await;
    assert!(page.contains("It holds test TINs: revoke them before you remove the test label."), "{page}");
    let response = site.post(&format!("{user}/test"), &staff, "test=off&reason=Done").await;
    assert_eq!(location(&response), format!("{user}?notice=test_tins_held"));
    let labelled: bool = sqlx::query_scalar("SELECT test_account FROM accounts WHERE id = $1")
        .bind(ana)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert!(labelled);

    let claim = test_tins(&site.db, ana).await[0].0;
    let response = site.post(&format!("/staff/claims/{claim}/revoke"), &staff, "reason=Done").await;
    assert_eq!(location(&response), format!("/staff/claims/{claim}?notice=ownership_revoked"));
    let response = site.post(&format!("{user}/test"), &staff, "test=off&reason=Done").await;
    assert_eq!(location(&response), format!("{user}?notice=test_unmarked"));
}

fn ticket_body(key: &str) -> Value {
    json!({
        "tin": "00000000000000",
        "form": "1701Qv2018",
        "period": "2026Q1",
        "payload_sha256": "ab".repeat(32),
        "email": "ana@example.test",
        "idempotency_key": key,
    })
}

/// The send gate refuses an active test TIN with its own code on every
/// path: a ticket (so no v2 send or test send), a v1 return, a sealed
/// return whose claim turned out to be one, and the relay. The server runs
/// as in production here (no BIR_RELAY_MODE of its own): it never trusts a
/// mode to let one through.
#[sqlx::test(migrations = "./migrations")]
async fn the_gate_refuses_a_test_tin_with_its_own_code(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, _) = test_account(&site, "ana@example.test").await;
    site.post(&format!("/staff/users/{ana}/test-tin"), &staff, FORM).await;
    let claim = test_tins(&site.db, ana).await[0].0;
    let refused = (403, "tin_test_only".to_owned(), TEST_ONLY.to_owned());

    let device = TestDevice::new(&site.db, ana).await;
    assert_eq!(refusal(device.post(&site, "/api/tickets", &ticket_body("t1")).await).await, refused);
    let tickets: i64 = sqlx::query_scalar("SELECT count(*) FROM tickets").fetch_one(&site.db).await.unwrap();
    assert_eq!(tickets, 0);

    let v1 = json!({
        "filename": "00000000000000-1701Qv2018-2026Q1#ana@example.test#.xml",
        "xml": iaf_return("frm1701q", "00000000000000", "2026"),
    });
    let request = Request::post("/api/submissions")
        .header(header::AUTHORIZATION, format!("Bearer {}", device.token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(v1.to_string()))
        .unwrap();
    assert_eq!(refusal(site.send(request).await).await, refused);

    let tin = buwiz_cloud::tin::Tin::parse_any("000000000", "").unwrap();
    assert_eq!(gate::own(&site.db, ana, &tin).await.unwrap(), gate::Standing::TestOnly);
    assert_eq!(gate::of_claim(&site.db, ana, claim).await.unwrap(), gate::Standing::TestOnly);

    // A return stored for it anyway (none can be, through the API) never
    // goes to the Sender: the relay fails it with the reason.
    let stored: Uuid = sqlx::query_scalar(
        "INSERT INTO submissions (account_id, claim_id, form_type, period, filename, xml_sha256, status,
             xml_nonce, xml_ciphertext)
         VALUES ($1, $2, '1701Qv2018', '2026Q1', 'f.xml', decode('00', 'hex'), 'ready', decode('01', 'hex'), decode('02', 'hex'))
         RETURNING id",
    )
    .bind(ana)
    .bind(claim)
    .fetch_one(&site.db)
    .await
    .unwrap();
    match submissions::claim_for_relay(&site.db, ana, stored).await.unwrap() {
        RelayStart::Answer(Control::Result { status, error, receipt: None }) => {
            assert_eq!((status.as_str(), error), ("failed", Some(format!("{TEST_ONLY} (tin_test_only)"))));
        }
        RelayStart::Answer(other) => panic!("{other:?}"),
        RelayStart::Send(_) => panic!("the relay would have sent a test TIN's return"),
    }
}

/// A paused test TIN still answers as a test TIN: never sendable, paused
/// or not, so the gate and the apps' list say test_only first.
#[sqlx::test(migrations = "./migrations")]
async fn a_paused_test_tin_still_answers_test_only(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, _) = test_account(&site, "ana@example.test").await;
    site.post(&format!("/staff/users/{ana}/test-tin"), &staff, FORM).await;
    let claim = test_tins(&site.db, ana).await[0].0;
    let response = site.post(&format!("/staff/claims/{claim}/pause"), &staff, "reason=Check").await;
    assert_eq!(location(&response), format!("/staff/claims/{claim}?notice=tin_paused"));

    let device = TestDevice::new(&site.db, ana).await;
    let refused = (403, "tin_test_only".to_owned(), TEST_ONLY.to_owned());
    assert_eq!(refusal(device.post(&site, "/api/tickets", &ticket_body("p1")).await).await, refused);
    let tin = buwiz_cloud::tin::Tin::parse_any("000000000", "").unwrap();
    assert_eq!(gate::own(&site.db, ana, &tin).await.unwrap(), gate::Standing::TestOnly);
    let token = app_token(&site.db, ana).await;
    let all = profiles(&site, &token, "/api/tax-profiles?include=all").await;
    assert_eq!((&all[0]["status"], &all[0]["message"]), (&json!("test_only"), &json!(TEST_ONLY)), "{all}");
}

/// A test TIN proves no identity: an account whose only TIN is one reads
/// "No ID yet", and the "Verified" filter leaves it out.
#[sqlx::test(migrations = "./migrations")]
async fn a_test_tin_alone_is_no_verified_id(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, _) = test_account(&site, "ana@example.test").await;
    site.post(&format!("/staff/users/{ana}/test-tin"), &staff, FORM).await;
    let page = text(site.get(&format!("/staff/users/{ana}"), &staff).await).await;
    assert!(page.contains("No ID yet") && !page.contains("ID verified"), "{page}");
    let list = text(site.get("/staff/users?q=ana%40", &staff).await).await;
    assert!(list.contains("ana@example.test") && list.contains("No ID yet"), "{list}");
    assert!(!list.contains("ID verified"), "{list}");
    let verified = text(site.get("/staff/users?filter=verified", &staff).await).await;
    assert!(!verified.contains("ana@example.test"), "{verified}");

    // A real TIN, active, makes it verified as before.
    add_claim(&site.db, ana, "000123453", "approved").await;
    let page = text(site.get(&format!("/staff/users/{ana}"), &staff).await).await;
    assert!(page.contains("ID verified"), "{page}");
    let verified = text(site.get("/staff/users?filter=verified", &staff).await).await;
    assert!(verified.contains("ana@example.test"), "{verified}");
}
