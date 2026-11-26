//! Test TINs: a TIN with a reserved root (`TEST_ROOTS`) an Admin adds to a
//! test account, active at once. It is marked as a test TIN wherever it is
//! listed, and the send gate treats it as any TIN: an active one sends on
//! every path, and the Sender makes each send a dry run, whatever the relay
//! mode (see `src/sender`). A person can never register one.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use buwiz_cloud::filing::gate;
use buwiz_cloud::filing::submissions::{self, RelayStart};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use common::*;

const TEST_TIN: &str =
    "A test TIN: a send for it ends as a dry run, with a signed receipt. Buwiz never sends it to the BIR.";
const PAUSED: &str = "Sending is paused for this TIN. Write to support@buwiz.com.";
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
    // In the TINs section itself, on the TIN's own row (not the History
    // line, nor the add form's default value).
    let tins = page.split("id=\"tab-tins\"").nth(1).and_then(|rest| rest.split("Identity documents").next()).unwrap();
    let row = tins.split("class=\"sx-tinrow\"").skip(1).find(|row| row.contains("000-000-000-00000")).unwrap();
    let row = row.split("Open claim").next().unwrap();
    assert!(row.contains("sx-pill testtin") && row.contains("Test TIN"), "{row}");
    assert!(page.contains("Test TIN 000-000-000-00000 added, active"), "{page}");
    let list = text(site.get("/staff/users", &staff).await).await;
    assert!(list.contains("000-000-000-00000 · test"), "{list}");
    let queue = text(site.get("/staff/queue?status=approved", &staff).await).await;
    assert!(queue.contains("Test TIN: never sends"), "{queue}");
    let review = text(site.get(&format!("/staff/claims/{claim}"), &staff).await).await;
    assert!(review.contains("Test TIN: Buwiz never sends returns for it"), "{review}");
    // The owner's page.
    let tins = text(site.get("/tins", &ana_cookie).await).await;
    assert!(tins.contains("Test TIN") && tins.contains("Buwiz never sends them to the BIR"), "{tins}");

    // The apps' TIN list marks it, active and able to send (as a dry
    // run); so does the licence list (TINs that can send).
    let token = app_token(&site.db, ana).await;
    let all = profiles(&site, &token, "/api/tax-profiles?include=all").await;
    assert_eq!(all.as_array().unwrap().len(), 1, "{all}");
    let row = &all[0];
    assert_eq!(
        (&row["test_only"], &row["status"], &row["can_send"], &row["message"], &row["tin"]),
        (&json!(true), &json!("approved"), &json!(true), &json!(TEST_TIN), &json!("000-000-000-00000")),
        "{row}"
    );
    assert!(row["permissions"].as_array().unwrap().contains(&json!("filing.submit")), "{row}");
    for query in ["", "?include=pending"] {
        let listed = profiles(&site, &token, &format!("/api/tax-profiles{query}")).await;
        assert_eq!(listed.as_array().unwrap().len(), 1, "{query}: {listed}");
        assert_eq!((&listed[0]["test_only"], &listed[0]["tin"]), (&json!(true), &json!("000-000-000-00000")));
    }
    // An AI agent's list of TIN claims marks it too.
    let agent = common::token(&site.db, ana, "an-agent", "mcp").await;
    let request = Request::post("/mcp")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {agent}"))
        .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_tin_claims"}}"#))
        .unwrap();
    let listed: Value = serde_json::from_str(&text(site.send(request).await).await).unwrap();
    let content: Value = serde_json::from_str(listed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let row = &content["claims"][0];
    // The same status the apps' list gives it.
    assert_eq!((&row["status"], &row["test_only"]), (&json!("approved"), &json!(true)), "{content}");
    // Other rows read as before: no `test_only` on them.
    add_claim(&site.db, ana, "000123453", "approved").await;
    let all = profiles(&site, &token, "/api/tax-profiles?include=all").await;
    let real = all.as_array().unwrap().iter().find(|row| row["tin"] == "000-123-453-00000").unwrap();
    assert!(real.get("test_only").is_none() && real["can_send"] == json!(true), "{all}");
    let listed = profiles(&site, &token, "/api/tax-profiles").await;
    let real = listed.as_array().unwrap().iter().find(|row| row["tin"] == "000-123-453-00000").unwrap();
    assert!(real.get("test_only").is_none(), "{listed}");
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
    assert_eq!(location(&site.post(&path(bob), &staff, FORM).await), back(bob, "test_tin_owned_elsewhere"));
    let page = text(site.get(&back(bob, "test_tin_owned_elsewhere"), &staff).await).await;
    assert!(page.contains("Another account holds this test TIN. Pick another branch"), "{page}");
    assert!(!page.contains("Reject this claim"), "{page}");
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
    let (ana, ana_cookie) = test_account(&site, "ana@example.test").await;
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
    // My TINs does not tell them to add it again with papers.
    let tins = text(site.get("/tins", &ana_cookie).await).await;
    assert!(tins.contains("No longer active.") && tins.contains("It was a test TIN"), "{tins}");
    assert!(!tins.contains("You can add the TIN again with new papers."), "{tins}");
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

/// The send gate lets an active test TIN through on every path, as any
/// active TIN: a ticket, a v1 return and the relay's lock. The server runs
/// as in production here (no BIR_RELAY_MODE of its own): the Sender, not
/// the gate, makes the send a dry run.
#[sqlx::test(migrations = "./migrations")]
async fn the_gate_lets_an_active_test_tin_send(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, _) = test_account(&site, "ana@example.test").await;
    site.post(&format!("/staff/users/{ana}/test-tin"), &staff, FORM).await;
    let claim = test_tins(&site.db, ana).await[0].0;

    let device = TestDevice::new(&site.db, ana).await;
    let response = device.post(&site, "/api/tickets", &ticket_body("t1")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let tickets: i64 = sqlx::query_scalar("SELECT count(*) FROM tickets").fetch_one(&site.db).await.unwrap();
    assert_eq!(tickets, 1);

    let v1 = json!({
        "filename": "00000000000000-1701Qv2018-2026Q1#ana@example.test#.xml",
        "xml": iaf_return("frm1701q", "00000000000000", "2026"),
    });
    let request = Request::post("/api/submissions")
        .header(header::AUTHORIZATION, format!("Bearer {}", device.token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(v1.to_string()))
        .unwrap();
    let response = site.send(request).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let stored: Value = serde_json::from_str(&text(response).await).unwrap();

    let tin = buwiz_cloud::tin::Tin::parse_any("000000000", "").unwrap();
    assert_eq!(gate::own(&site.db, ana, &tin).await.unwrap(), gate::Standing::Sendable { claim_id: claim });
    assert_eq!(gate::of_claim(&site.db, ana, claim).await.unwrap(), gate::Standing::Sendable { claim_id: claim });

    let id: Uuid = stored["id"].as_str().unwrap().parse().unwrap();
    match submissions::claim_for_relay(&site.db, ana, id).await.unwrap() {
        RelayStart::Send(_) => {}
        RelayStart::Answer(other) => panic!("{other:?}"),
    }
}

/// A paused test TIN answers as any paused TIN, in the gate and in the
/// apps' list.
#[sqlx::test(migrations = "./migrations")]
async fn a_paused_test_tin_answers_as_paused(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, _) = test_account(&site, "ana@example.test").await;
    site.post(&format!("/staff/users/{ana}/test-tin"), &staff, FORM).await;
    let claim = test_tins(&site.db, ana).await[0].0;
    let response = site.post(&format!("/staff/claims/{claim}/pause"), &staff, "reason=Check").await;
    assert_eq!(location(&response), format!("/staff/claims/{claim}?notice=tin_paused"));

    let device = TestDevice::new(&site.db, ana).await;
    let refused = (403, "tin_paused".to_owned(), PAUSED.to_owned());
    assert_eq!(refusal(device.post(&site, "/api/tickets", &ticket_body("p1")).await).await, refused);
    let tin = buwiz_cloud::tin::Tin::parse_any("000000000", "").unwrap();
    assert_eq!(gate::own(&site.db, ana, &tin).await.unwrap(), gate::Standing::Paused);
    let token = app_token(&site.db, ana).await;
    let all = profiles(&site, &token, "/api/tax-profiles?include=all").await;
    assert_eq!(
        (&all[0]["status"], &all[0]["message"], &all[0]["test_only"]),
        (&json!("paused"), &json!(PAUSED), &json!(true)),
        "{all}"
    );
    assert_eq!(profiles(&site, &token, "/api/tax-profiles").await, json!([]));
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

/// An Admin another Admin labelled a test account adds a test TIN to their
/// own account, as to anyone's: the line names them as the actor, the
/// History says it was their own, and the claim is flagged as activated
/// by its owner.
#[sqlx::test(migrations = "./migrations")]
async fn an_admin_adds_a_test_tin_to_their_own_test_account(db: PgPool) {
    let site = Site::with_admins(db, &[]);
    let (_, boss) = site.staff_with_role("boss@example.test", "admin").await;
    let (ana, ana_cookie) = site.staff_with_role("ana@example.test", "admin").await;
    let user = format!("/staff/users/{ana}");
    let response = site.post(&format!("{user}/test"), &boss, "test=on&reason=QA+admin").await;
    assert_eq!(location(&response), format!("{user}?notice=test_marked"));
    let page = text(site.get(&user, &ana_cookie).await).await;
    assert!(page.contains(&format!("action=\"{user}/test-tin\"")), "{page}");

    let response = site.post(&format!("{user}/test-tin"), &ana_cookie, FORM).await;
    assert_eq!(location(&response), format!("{user}?notice=test_tin_added"));
    let rows = test_tins(&site.db, ana).await;
    assert_eq!((rows.len(), rows[0].2.as_str(), rows[0].4), (1, "approved", Some(ana)));
    assert_eq!(
        claim_audit_lines(&site.db, ana).await,
        [("activated".into(), "new".into(), "ana@example.test".into(), "web".into(), Some("Trying the app".into()))]
    );
    for viewer in [&ana_cookie, &boss] {
        let history = text(site.get(&user, viewer).await).await;
        assert!(
            history.contains(
                "Staff ana@example.test activated their own TIN ***-***-000-00000 without papers: Trying the app"
            ),
            "{history}"
        );
        assert!(history.contains("Test TIN 000-000-000-00000 added, active"), "{history}");
    }
    let review = text(site.get(&format!("/staff/claims/{}", rows[0].0), &boss).await).await;
    assert!(review.contains("Activated by its owner, an admin"), "{review}");
    // The label stays theirs to keep: another Admin takes it off, once the
    // test TIN is revoked.
    let response = site.post(&format!("{user}/test"), &ana_cookie, "test=off&reason=Done").await;
    assert_eq!(location(&response), format!("{user}?notice=own_account"));
}

/// Someone in ADMIN_EMAILS who carries the test label (given before they
/// were listed, say) gets no test TIN: their page has no test-label
/// controls, so the label it would hold in place could not come off.
#[sqlx::test(migrations = "./migrations")]
async fn an_admin_emails_address_gets_no_test_tin(db: PgPool) {
    let site = Site::with_admins(db, &[STAFF, "boss@example.test"]);
    let (_, staff) = site.person(STAFF).await;
    let (boss, _) = test_account(&site, "boss@example.test").await;
    let user = format!("/staff/users/{boss}");
    let page = text(site.get(&user, &staff).await).await;
    assert!(page.contains("Test account") && !page.contains("test-tin"), "{page}");
    let response = site.post(&format!("{user}/test-tin"), &staff, FORM).await;
    assert_eq!(location(&response), format!("{user}?notice=test_label_from_server"));
    assert!(test_tins(&site.db, boss).await.is_empty());
}

/// A reserved root (`TEST_ROOTS`) as written on a form: `111-111-114`.
fn dashed(root: &str) -> String {
    format!("{}-{}-{}", &root[..3], &root[3..6], &root[6..])
}

/// What the people's forms answer a reserved root: all zeros keeps its old
/// words, any other reserved root says it is reserved.
fn reserved_notice(root: &str) -> &'static str {
    if root.bytes().all(|byte| byte == b'0') { "tin_zero" } else { "tin_reserved" }
}

/// Every reserved root is a test TIN staff add: an Admin adds it to a test
/// account at head office (never to one without the label), the apps' TIN
/// lists and an AI agent's list mark it `test_only`, and the gate lets it
/// send, so its send reaches the Sender, which makes it a dry run.
#[sqlx::test(migrations = "./migrations")]
async fn every_reserved_root_is_a_test_tin_an_admin_adds(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, _) = test_account(&site, "ana@example.test").await;
    let (plain, _) = site.person("plain@example.test").await;
    let token = app_token(&site.db, ana).await;
    let device = TestDevice::new(&site.db, ana).await;
    let path = |id: Uuid| format!("/staff/users/{id}/test-tin");
    let back = |id: Uuid, notice: &str| format!("/staff/users/{id}?notice={notice}");
    for root in buwiz_cloud::tin::TEST_ROOTS {
        let shown = format!("{}-00000", dashed(root));
        let form = FORM.replace("000-000-000-00000", &shown);
        assert_eq!(location(&site.post(&path(plain), &staff, &form).await), back(plain, "not_test_account"), "{root}");
        assert_eq!(location(&site.post(&path(ana), &staff, &form).await), back(ana, "test_tin_added"), "{root}");
        let (status, verified_by): (String, Option<String>) = sqlx::query_as(
            "SELECT status, verified_by FROM tin_claims WHERE account_id = $1 AND tin_root = $2 AND branch_code = '00000'",
        )
        .bind(ana)
        .bind(root)
        .fetch_one(&site.db)
        .await
        .unwrap();
        assert_eq!((status.as_str(), verified_by.as_deref()), ("approved", Some("admin")), "{root}");

        let all = profiles(&site, &token, "/api/tax-profiles?include=all").await;
        let row = all.as_array().unwrap().iter().find(|row| row["tin"] == json!(shown)).expect("listed");
        assert_eq!(
            (&row["test_only"], &row["status"], &row["can_send"], &row["message"]),
            (&json!(true), &json!("approved"), &json!(true), &json!(TEST_TIN)),
            "{row}"
        );
        let listed = profiles(&site, &token, "/api/tax-profiles").await;
        let row = listed.as_array().unwrap().iter().find(|row| row["tin"] == json!(shown)).expect("listed");
        assert_eq!(row["test_only"], json!(true), "{row}");

        let mut body = ticket_body(&format!("ticket-{root}"));
        body["tin"] = json!(format!("{root}00000"));
        assert_eq!(device.post(&site, "/api/tickets", &body).await.status(), StatusCode::CREATED, "{root}");
        let tin = buwiz_cloud::tin::Tin::parse_any(root, "").unwrap();
        assert!(matches!(gate::own(&site.db, ana, &tin).await.unwrap(), gate::Standing::Sendable { .. }), "{root}");
    }
    let held: i64 = sqlx::query_scalar("SELECT count(*) FROM tin_claims WHERE account_id = $1")
        .bind(plain)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert_eq!(held, 0);

    let agent = common::token(&site.db, ana, "an-agent", "mcp").await;
    let request = Request::post("/mcp")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {agent}"))
        .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_tin_claims"}}"#))
        .unwrap();
    let listed: Value = serde_json::from_str(&text(site.send(request).await).await).unwrap();
    let content: Value = serde_json::from_str(listed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let claims = content["claims"].as_array().unwrap();
    assert_eq!(claims.len(), buwiz_cloud::tin::TEST_ROOTS.len(), "{content}");
    assert!(claims.iter().all(|row| row["test_only"] == json!(true)), "{content}");
}

/// A person can never register a reserved root: the add-a-TIN form, with
/// papers or without, in every way of writing it, refuses it (all zeros
/// with its old words, another reserved root with "reserved for testing"),
/// and the apps' "Add it" link never fills one in. No row is made.
#[sqlx::test(migrations = "./migrations")]
async fn a_person_cannot_register_a_reserved_root(db: PgPool) {
    use common::Part::{File, Text};
    let site = Site::new(db);
    // A test account too: the label gives a person no way in.
    let (ana, cookie) = test_account(&site, "ana@example.test").await;
    let leak = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
    for root in buwiz_cloud::tin::TEST_ROOTS {
        let notice = reserved_notice(root);
        let back = format!("/tins/new?notice={notice}");
        for (tin, branch) in [
            (dashed(root), "00000"),
            (root.to_string(), "00001"),
            (format!("{}-00000", dashed(root)), ""),
            (format!("{root}00002"), "00000"),
            // A bad branch: the root is still what is named.
            (dashed(root), "12"),
        ] {
            for papers in [false, true] {
                let mut parts = vec![
                    ("kind", Text("personal")),
                    ("tin", Text(leak(tin.clone()))),
                    ("branch", Text(branch)),
                    ("registered_name", Text("SYNTHETIC TAXPAYER")),
                    ("tin_id", File("id.png", PNG)),
                ];
                if !papers {
                    parts.push(("skip", Text("1")));
                }
                let response = site.upload("/tins/claims", &cookie, &parts).await;
                assert_eq!(location(&response), back, "{tin} {branch} papers={papers}");
            }
        }
        let page = text(site.get(&format!("/tins/new?tin={root}&branch=00000"), &cookie).await).await;
        assert!(!page.contains(&format!("value=\"{}\"", dashed(root))), "{root}");
    }
    let page = text(site.get("/tins/new?notice=tin_reserved", &cookie).await).await;
    assert!(page.contains("This TIN is reserved for testing. If it is yours, write to support@buwiz.com."), "{page}");
    let page = text(site.get("/tins/new?notice=tin_zero", &cookie).await).await;
    assert!(page.contains("That TIN is not valid."), "{page}");
    let held: i64 = sqlx::query_scalar("SELECT count(*) FROM tin_claims WHERE account_id = $1")
        .bind(ana)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert_eq!(held, 0);
    // Nor can any row a person's path makes hold one: the database refuses
    // a claim waiting with papers, one saved without them, one the
    // automatic check activated, for every reserved root.
    for root in buwiz_cloud::tin::TEST_ROOTS {
        for (status, extra, values) in
            [("pending", "", ""), ("unverified", "", ""), ("approved", ", decided_at, verified_by", ", now(), 'auto'")]
        {
            let error = sqlx::query(&format!(
                "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status{extra})
                 VALUES ($1, 'personal', $2, '00000', 'SYNTHETIC TAXPAYER', '{status}'{values})"
            ))
            .bind(ana)
            .bind(root)
            .execute(&site.db)
            .await
            .unwrap_err();
            let constraint = error.as_database_error().and_then(|error| error.constraint().map(str::to_owned));
            assert_eq!(constraint.as_deref(), Some("tin_claims_test_tin_check"), "{root} {status}");
        }
    }
}

/// A v1 return's `POST /api/submissions` request for the 14-digit `tin`.
fn v1_return(token: &str, tin: &str, key: &str) -> Request<Body> {
    let body = json!({
        "filename": format!("{tin}-1701Qv2018-2026Q1#ana@example.test#.xml"),
        "xml": iaf_return("frm1701q", tin, "2026"),
        "idempotency_key": key,
    });
    Request::post("/api/submissions")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// `path` read with `token`, as JSON.
async fn read(site: &Site, token: &str, path: String) -> Value {
    let request =
        Request::get(path).header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap();
    serde_json::from_str(&text(site.send(request).await).await).unwrap()
}

/// A test TIN's submission says `"test_only": true` wherever the app reads
/// it: the `POST /api/submissions` answer and its idempotent replay,
/// `GET /api/submissions/{id}` and its item in `GET /api/submissions`, for
/// every reserved root, to other clients too. Another TIN's has no such
/// field, so it reads as before.
#[sqlx::test(migrations = "./migrations")]
async fn a_test_tins_submission_says_test_only(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, _) = test_account(&site, "ana@example.test").await;
    for root in buwiz_cloud::tin::TEST_ROOTS {
        let form = FORM.replace("000-000-000-00000", &format!("{}-00000", dashed(root)));
        site.post(&format!("/staff/users/{ana}/test-tin"), &staff, &form).await;
    }
    add_claim(&site.db, ana, "000123453", "approved").await;
    let app = app_token(&site.db, ana).await;
    let books = common::token(&site.db, ana, "buwiz-books", "books").await;
    let mut ids = Vec::new();
    for (root, test_only) in [("000000000", true), ("111111114", true), ("000123453", false)] {
        let tin = format!("{root}00000");
        let response = site.send(v1_return(&app, &tin, &format!("k-{root}"))).await;
        assert_eq!(response.status(), StatusCode::CREATED, "{root}");
        let created: Value = serde_json::from_str(&text(response).await).unwrap();
        let replay: Value =
            serde_json::from_str(&text(site.send(v1_return(&app, &tin, &format!("k-{root}"))).await).await).unwrap();
        let id = created["id"].as_str().unwrap().to_owned();
        let shown = read(&site, &app, format!("/api/submissions/{id}")).await;
        let shown_to_books = read(&site, &books, format!("/api/submissions/{id}")).await;
        for (name, view) in [("create", &created), ("replay", &replay), ("show", &shown), ("books", &shown_to_books)] {
            assert_eq!(view["id"], json!(id), "{root} {name}");
            if test_only {
                assert_eq!(view["test_only"], json!(true), "{root} {name}: {view}");
            } else {
                assert!(view.get("test_only").is_none(), "{root} {name}: {view}");
            }
        }
        ids.push((id, test_only));
    }
    let listed = read(&site, &app, "/api/submissions".to_owned()).await;
    for (id, test_only) in ids {
        let item = listed["submissions"].as_array().unwrap().iter().find(|item| item["id"] == json!(id)).unwrap();
        assert_eq!(item.get("test_only").cloned(), test_only.then_some(json!(true)), "{item}");
    }
}
