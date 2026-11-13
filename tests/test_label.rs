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

/// Nobody labels their own account, even with a direct post.
#[sqlx::test(migrations = "./migrations")]
async fn an_admin_cannot_label_their_own_account(db: PgPool) {
    let site = Site::new(db);
    let (boss, cookie) = site.staff_with_role("boss@example.test", "admin").await;
    let user = format!("/staff/users/{boss}");
    let response = site.post(&format!("{user}/test"), &cookie, "test=on&reason=Mine").await;
    assert!(location(&response).ends_with("?notice=own_account"), "{}", location(&response));
    let labelled: bool = sqlx::query_scalar("SELECT test_account FROM accounts WHERE id = $1")
        .bind(boss)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert!(!labelled);
    assert!(account_audit_lines(&site.db, boss).await.is_empty());
}

/// Two Admins labelling each other at once take their locks in the same
/// order as every other staff action (the roles lock, the actor, then the
/// account acted on): both go through, neither deadlocks.
#[sqlx::test(migrations = "./migrations")]
async fn two_admins_labelling_each_other_at_once_do_not_deadlock(db: PgPool) {
    use buwiz_cloud::account::{admin, roles};
    let site = Site::with_admins(db, &[]);
    let (a, _) = site.staff_with_role("a@example.test", "admin").await;
    let (b, _) = site.staff_with_role("b@example.test", "admin").await;
    let staff = |id: uuid::Uuid, email: &str| {
        let account = buwiz_cloud::accounts::Account { id, email: email.into() };
        let (db, config) = (site.db.clone(), site.state.config.clone());
        async move { roles::staff(&db, &config, &account).await.unwrap().unwrap() }
    };
    let (staff_a, staff_b) = (staff(a, "a@example.test").await, staff(b, "b@example.test").await);
    // Something reads both accounts a moment, so both actions get as far
    // as the account they act on before either goes on.
    let mut holder = site.db.begin().await.unwrap();
    sqlx::query("SELECT id FROM accounts WHERE id = ANY($1) FOR KEY SHARE")
        .bind(vec![a, b])
        .execute(&mut *holder)
        .await
        .unwrap();
    let mark = |actor: roles::Staff, target: uuid::Uuid| {
        let (db, config) = (site.db.clone(), site.state.config.clone());
        tokio::spawn(async move { admin::mark_test(&db, &config, &actor, target, true, "Trial").await })
    };
    let first = mark(staff_a, b);
    let second = mark(staff_b, a);
    for _ in 0..100 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(&site.db)
        .await
        .unwrap();
        if waiting >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    holder.rollback().await.unwrap();
    for outcome in [first.await.unwrap(), second.await.unwrap()] {
        assert!(matches!(outcome, Ok(buwiz_cloud::notice::Notice::TestMarked)), "{outcome:?}");
    }
}
