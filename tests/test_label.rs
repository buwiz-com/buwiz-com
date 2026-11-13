//! The test-account label: Admin only, seen and filtered by staff, with no
//! effect on sending.

mod common;

use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;

use common::*;

#[sqlx::test(migrations = "./migrations")]
async fn test_accounts_are_labelled_and_filtered_and_send_as_usual(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, _) = site.person("ana@example.test").await;
    site.person("bob@example.test").await;
    add_claim(&site.db, ana, "000123453", "approved").await;
    let user = format!("/staff/users/{ana}");
    let page = text(site.get(&user, &staff).await).await;
    assert!(page.contains("Mark ana@example.test as a test account?"), "{page}");

    let response = site.post(&format!("{user}/test"), &staff, "test=on&reason=+").await;
    assert_eq!(location(&response), format!("{user}?notice=reason_missing"));
    let response = site.post(&format!("{user}/test"), &staff, "test=on&reason=Owner%27s+trial+login").await;
    assert_eq!(location(&response), format!("{user}?notice=test_marked"));

    let page = text(site.get(&user, &staff).await).await;
    assert!(page.contains("Test account") && page.contains("Remove the test label from ana@example.test?"), "{page}");
    let list = text(site.get("/staff/users?filter=test", &staff).await).await;
    assert!(list.contains("ana@example.test") && !list.contains("bob@example.test"), "{list}");
    assert!(list.contains(">Test<"), "{list}");
    let queue = text(site.get("/staff?status=approved", &staff).await).await;
    assert!(queue.contains(">Test<"), "{queue}");

    // Sends as usual.
    let device = TestDevice::new(&site.db, ana).await;
    let body = json!({
        "tin": "00012345300000",
        "form": "1701Qv2018",
        "period": "2026Q1",
        "payload_sha256": "ab".repeat(32),
        "email": "ana@example.test",
        "idempotency_key": "t1",
    });
    assert_eq!(device.post(&site, "/api/tickets", &body).await.status(), StatusCode::CREATED);

    let response = site.post(&format!("{user}/test"), &staff, "test=off&reason=Done").await;
    assert_eq!(location(&response), format!("{user}?notice=test_unmarked"));
    assert!(!text(site.get("/staff/users?filter=test", &staff).await).await.contains("ana@example.test"));
    let lines = account_audit_lines(&site.db, ana).await;
    let actions: Vec<(&str, Option<&str>)> = lines.iter().map(|line| (line.0.as_str(), line.5.as_deref())).collect();
    assert_eq!(actions, [("test_marked", Some("Owner's trial login")), ("test_unmarked", Some("Done"))]);
    let history = text(site.get(&user, &staff).await).await;
    assert!(history.contains(&format!("Staff {STAFF} marked it as a test account: Owner's trial login")), "{history}");
}
