//! "New user" on /staff/users: an Admin makes a login, with a role and the
//! test label if asked, and a reason; nothing is mailed.

mod common;

use axum::http::StatusCode;
use sqlx::PgPool;
use uuid::Uuid;

use common::*;

async fn account(db: &PgPool, email: &str) -> Option<(Uuid, Option<String>, bool)> {
    sqlx::query_as("SELECT id, role, test_account FROM accounts WHERE email = $1")
        .bind(email)
        .fetch_optional(db)
        .await
        .unwrap()
}

async fn accounts(db: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM accounts").fetch_one(db).await.unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_makes_a_login_with_a_role_and_the_test_label(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let page = text(site.get("/staff/users", &staff).await).await;
    assert!(page.contains("action=\"/staff/users/new\""), "{page}");
    assert!(page.contains("so it must be able to receive mail. Nothing is sent now."), "{page}");

    let response = site
        .post("/staff/users/new", &staff, "email=+Tester%40Example.test+&role=support&test=on&reason=QA+login")
        .await;
    let (id, role, test) = account(&site.db, "tester@example.test").await.unwrap();
    assert_eq!(location(&response), format!("/staff/users/{id}?notice=login_created"));
    assert_eq!((role.as_deref(), test), (Some("support"), true));
    let lines = account_audit_lines(&site.db, id).await;
    let expected = |action: &str, role: Option<&str>| {
        (
            action.to_owned(),
            None,
            role.map(str::to_owned),
            STAFF.to_owned(),
            "web".to_owned(),
            Some("QA login".to_owned()),
        )
    };
    assert_eq!(
        lines,
        [expected("created", None), expected("role_changed", Some("support")), expected("test_marked", None)]
    );
    // No sign-in code was made, so none was mailed.
    let codes: i64 = sqlx::query_scalar("SELECT count(*) FROM email_codes").fetch_one(&site.db).await.unwrap();
    assert_eq!(codes, 0);
    let history = text(site.get(&format!("/staff/users/{id}"), &staff).await).await;
    assert!(history.contains(&format!("Staff {STAFF} created the login: QA login")), "{history}");
    assert!(history.contains("Test account"), "{history}");

    // Without a role or label.
    let response = site.post("/staff/users/new", &staff, "email=plain%40example.test&role=none&reason=Client").await;
    let (id, role, test) = account(&site.db, "plain@example.test").await.unwrap();
    assert_eq!(location(&response), format!("/staff/users/{id}?notice=login_created"));
    assert_eq!((role, test), (None, false));
    assert_eq!(account_audit_lines(&site.db, id).await.len(), 1);
}

/// An address that has an account already, a bad address, a missing or
/// long reason, an unknown role, and a role for someone in ADMIN_EMAILS:
/// refused, and nothing is made.
#[sqlx::test(migrations = "./migrations")]
async fn a_login_that_exists_or_is_not_right_is_refused(db: PgPool) {
    let site = Site::with_admins(db, &[STAFF, "boss@example.test"]);
    let (_, staff) = site.person(STAFF).await;
    let (ana, _) = site.person("ana@example.test").await;
    let before = accounts(&site.db).await;
    let long = "a".repeat(501);
    for (form, notice) in [
        ("email=ANA%40example.test&reason=Again".to_owned(), "account_exists"),
        ("email=not-an-email&reason=x".to_owned(), "email_invalid"),
        ("email=new%40example.test&reason=+".to_owned(), "reason_missing"),
        ("email=new%40example.test".to_owned(), "reason_missing"),
        (format!("email=new%40example.test&reason={long}"), "reason_too_long"),
        ("email=new%40example.test&role=boss&reason=x".to_owned(), "role_unknown"),
        ("email=boss%40example.test&role=support&reason=x".to_owned(), "role_from_server"),
    ] {
        let response = site.post("/staff/users/new", &staff, &form).await;
        assert_eq!(location(&response), format!("/staff/users?notice={notice}"), "{form}");
    }
    assert_eq!(accounts(&site.db).await, before);
    assert!(account_audit_lines(&site.db, ana).await.is_empty());
    let page = text(site.get("/staff/users?notice=account_exists", &staff).await).await;
    assert!(page.contains("An account with this email already exists."), "{page}");
}

/// Only an Admin makes logins: Support is sent back, everyone else does
/// not find the route.
#[sqlx::test(migrations = "./migrations")]
async fn only_an_admin_makes_a_login(db: PgPool) {
    let site = Site::new(db);
    let (_, helper) = site.staff_with_role("helper@example.test", "support").await;
    let (_, plain) = site.person("plain@example.test").await;
    let before = accounts(&site.db).await;
    let form = "email=new%40example.test&reason=Joins";
    assert_eq!(location(&site.post("/staff/users/new", &helper, form).await), "/staff/users?notice=admin_only");
    assert_eq!(site.post("/staff/users/new", &plain, form).await.status(), StatusCode::NOT_FOUND);
    assert!(location(&site.post("/staff/users/new", "", form).await).starts_with("/login?"));
    assert_eq!(accounts(&site.db).await, before);
    let page = text(site.get("/staff/users", &helper).await).await;
    assert!(!page.contains("action=\"/staff/users/new\""), "{page}");
}
