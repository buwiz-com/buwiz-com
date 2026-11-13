//! An Admin pauses an active TIN and resumes it: still owned, nothing
//! sends meanwhile, and the owner is told why.

mod common;

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use common::Part::{File, Text};
use common::*;

const REASON: &str = "Checking a complaint about this TIN";
const REASON_FORM: &str = "reason=Checking+a+complaint+about+this+TIN";

fn ticket(key: &str) -> Value {
    json!({
        "tin": "00012345300000",
        "form": "1701Qv2018",
        "period": "2026Q1",
        "payload_sha256": "ab".repeat(32),
        "email": "ana@example.test",
        "idempotency_key": key,
    })
}

/// (status, paused_at set, paused_by, pause_reason, decided_at, verified_by)
type Row = (String, bool, Option<Uuid>, Option<String>, Option<OffsetDateTime>, Option<String>);

async fn row(db: &PgPool, claim: Uuid) -> Row {
    sqlx::query_as(
        "SELECT status, paused_at IS NOT NULL, paused_by, pause_reason, decided_at, verified_by FROM tin_claims WHERE id = $1",
    )
    .bind(claim)
    .fetch_one(db)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn pausing_stops_sending_and_resuming_restores_it(db: PgPool) {
    let site = Site::new(db);
    let (staff_id, staff) = site.person(STAFF).await;
    let (ana, ana_cookie) = site.person("ana@example.test").await;
    let claim = add_claim(&site.db, ana, "000123453", "approved").await;
    let device = TestDevice::new(&site.db, ana).await;
    let before = row(&site.db, claim).await;
    let page = format!("/staff/claims/{claim}");

    let review = text(site.get(&page, &staff).await).await;
    assert!(review.contains("Pause sending for 000-123-453-00000?"), "{review}");
    assert_eq!(
        location(&site.post(&format!("{page}/pause"), &staff, "reason=+").await),
        format!("{page}?notice=reason_missing")
    );
    assert_eq!(
        location(&site.post(&format!("{page}/resume"), &staff, "reason=x").await),
        format!("{page}?notice=not_paused")
    );
    let response = site.post(&format!("{page}/pause"), &staff, REASON_FORM).await;
    assert_eq!(location(&response), format!("{page}?notice=tin_paused"));
    let paused = row(&site.db, claim).await;
    assert_eq!(
        (paused.0.as_str(), paused.1, paused.2, paused.3.as_deref()),
        ("approved", true, Some(staff_id), Some(REASON))
    );
    assert_eq!(
        location(&site.post(&format!("{page}/pause"), &staff, REASON_FORM).await),
        format!("{page}?notice=not_pausable")
    );

    // The send gate refuses it, and the licence list leaves it out.
    assert_eq!(refusal(device.post(&site, "/api/tickets", &ticket("k1")).await).await.1, "tin_paused");
    assert_eq!(profiles(&site, &device.token, "/api/tax-profiles").await, json!([]));
    assert_eq!(profiles(&site, &device.token, "/api/tax-profiles?include=all").await[0]["status"], "paused");

    // The owner sees why, and cannot remove it to get round the pause.
    let my_tins = text(site.get("/tins", &ana_cookie).await).await;
    for expected in [
        "Paused",
        "Sending is paused.",
        "Our staff paused this TIN on",
        REASON,
        "Questions? Write to support@buwiz.com.",
    ] {
        assert!(my_tins.contains(expected), "{expected}: {my_tins}");
    }
    assert!(
        !my_tins.contains("Active since") && !my_tins.contains(&format!("/tins/claims/{claim}/remove")),
        "{my_tins}"
    );
    let remove = site.post(&format!("/tins/claims/{claim}/remove"), &ana_cookie, "").await;
    assert_eq!(location(&remove), "/tins?notice=paused_locked");
    assert!(row(&site.db, claim).await.1);
    let add_link = site.get("/tins/new?tin=000123453", &ana_cookie).await;
    assert_eq!(location(&add_link), "/tins?notice=tin_is_paused");

    // Staff see it as paused everywhere.
    assert!(text(site.get("/staff?status=paused", &staff).await).await.contains("000-123-453-00000"));
    assert!(!text(site.get("/staff?status=approved", &staff).await).await.contains("000-123-453-00000"));
    let review = text(site.get(&page, &staff).await).await;
    assert!(review.contains(&format!("Paused by {STAFF} on")) && review.contains(REASON), "{review}");
    assert!(review.contains("Resume sending for 000-123-453-00000?"), "{review}");
    let user = text(site.get(&format!("/staff/users/{ana}"), &staff).await).await;
    assert!(
        user.contains("user-tin-status paused")
            && user.contains(&format!("Staff {STAFF} paused ***-***-453-00000: {REASON}")),
        "{user}"
    );

    // Resumed: exactly as before, and it sends again.
    assert_eq!(
        location(&site.post(&format!("{page}/resume"), &staff, "reason=+").await),
        format!("{page}?notice=reason_missing")
    );
    let response = site.post(&format!("{page}/resume"), &staff, "reason=Complaint+answered").await;
    assert_eq!(location(&response), format!("{page}?notice=tin_resumed"));
    assert_eq!(row(&site.db, claim).await, before);
    assert_eq!(device.post(&site, "/api/tickets", &ticket("k2")).await.status(), StatusCode::CREATED);

    // Paused again and then revoked: ended, with no pause left on the row,
    // but every step on record.
    site.post(&format!("{page}/pause"), &staff, REASON_FORM).await;
    site.post(&format!("{page}/revoke"), &staff, "reason=Not+the+owner").await;
    let ended = row(&site.db, claim).await;
    assert_eq!((ended.0.as_str(), ended.1), ("revoked", false));
    let lines = claim_audit_lines(&site.db, ana).await;
    let actions: Vec<(&str, &str, Option<&str>)> =
        lines.iter().map(|line| (line.0.as_str(), line.3.as_str(), line.4.as_deref())).collect();
    assert_eq!(
        actions,
        [
            ("paused", "web", Some(REASON)),
            ("resumed", "web", Some("Complaint answered")),
            ("paused", "web", Some(REASON)),
            ("revoked", "web", Some("Not the owner")),
        ]
    );

    // Added again with papers, the reviewer sees the earlier pauses.
    let parts = [
        ("kind", Text("business")),
        ("tin", Text("000123453")),
        ("registered_name", Text("Juan")),
        ("form_2303", File("2303.pdf", PDF)),
        ("government_id", File("id.png", PNG)),
    ];
    site.upload("/tins/claims", &ana_cookie, &parts).await;
    let again = site.newest_claim(ana).await;
    let review = text(site.get(&format!("/staff/claims/{again}"), &staff).await).await;
    assert!(review.contains("Paused before") && review.contains(REASON), "{review}");

    // Opening sending again on a TIN that was paused is an Admin's call:
    // Support is told so, and the claim stays waiting.
    let (_, support) = site.staff_with_role("helper@example.test", "support").await;
    let decide = format!("/staff/claims/{again}/decision");
    let page = text(site.get(&format!("/staff/claims/{again}"), &support).await).await;
    assert!(page.contains("Sending for this TIN was paused before: an admin must approve it."), "{page}");
    let response = site.post(&decide, &support, "decision=approve").await;
    assert_eq!(location(&response), format!("/staff/claims/{again}?notice=admin_must_decide"));
    assert_eq!(site.claim_status(again).await, "pending");
    let response = site.post(&decide, &staff, "decision=approve").await;
    assert_eq!(location(&response), "/staff?notice=claim_decided");
    assert_eq!(site.claim_status(again).await, "approved");
}

/// Only an active TIN can be paused, and an owner cannot remove a paused
/// one; an unpaused one is removed as before.
#[sqlx::test(migrations = "./migrations")]
async fn only_an_active_tin_is_paused(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, ana_cookie) = site.person("ana@example.test").await;
    let pending = add_claim(&site.db, ana, "000123454", "pending").await;
    let response = site.post(&format!("/staff/claims/{pending}/pause"), &staff, REASON_FORM).await;
    assert_eq!(location(&response), format!("/staff/claims/{pending}?notice=not_pausable"));
    let active = add_claim(&site.db, ana, "000123455", "approved").await;
    let remove = site.post(&format!("/tins/claims/{active}/remove"), &ana_cookie, "").await;
    assert_eq!(location(&remove), "/tins?notice=tin_removed");
}
