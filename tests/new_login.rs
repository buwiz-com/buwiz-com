//! "New user" on /staff/users: an Admin makes a login, with the test label
//! if asked, and a reason; nothing is mailed. It gives no staff role: a role
//! waits for the person's first sign-in, which proves the address.

mod common;

use axum::http::StatusCode;
use sqlx::PgPool;
use uuid::Uuid;

use buwiz_cloud::account::admin;
use buwiz_cloud::account::claims::DecideError;
use buwiz_cloud::account::notice::Notice;
use buwiz_cloud::account::roles;

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
async fn an_admin_makes_a_login_with_the_test_label(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let page = text(site.get("/staff/users", &staff).await).await;
    assert!(page.contains("action=\"/staff/users/new\""), "{page}");
    assert!(page.contains("so it must be able to receive mail. Nothing is sent now."), "{page}");
    assert!(page.contains("A staff role can be given on their page after their first sign-in."), "{page}");
    assert!(!page.contains("name=\"role\""), "{page}");

    // A role sent anyway (an old form) is not read.
    let response =
        site.post("/staff/users/new", &staff, "email=+Tester%40Example.test+&role=admin&test=on&reason=QA+login").await;
    let (id, role, test) = account(&site.db, "tester@example.test").await.unwrap();
    assert_eq!(location(&response), format!("/staff/users/{id}?notice=login_created"));
    assert_eq!((role, test), (None, true));
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
    assert_eq!(lines, [expected("created", None), expected("test_marked", None)]);
    // No sign-in code was made, so none was mailed.
    let codes: i64 = sqlx::query_scalar("SELECT count(*) FROM email_codes").fetch_one(&site.db).await.unwrap();
    assert_eq!(codes, 0);
    let history = text(site.get(&format!("/staff/users/{id}"), &staff).await).await;
    assert!(history.contains(&format!("Staff {STAFF} created the login: QA login")), "{history}");
    assert!(history.contains("Test account"), "{history}");

    // Without a role or label.
    let response = site.post("/staff/users/new", &staff, "email=plain%40example.test&reason=Client").await;
    let (id, role, test) = account(&site.db, "plain@example.test").await.unwrap();
    assert_eq!(location(&response), format!("/staff/users/{id}?notice=login_created"));
    assert_eq!((role, test), (None, false));
    assert_eq!(account_audit_lines(&site.db, id).await.len(), 1);
}

/// An address that has an account already, a bad address, a missing or
/// long reason, and the test label for someone in ADMIN_EMAILS (whose page
/// cannot take it off): refused, and nothing is made.
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
        ("email=boss%40example.test&test=on&reason=x".to_owned(), "test_label_from_server"),
    ] {
        let response = site.post("/staff/users/new", &staff, &form).await;
        assert_eq!(location(&response), format!("/staff/users?notice={notice}"), "{form}");
    }
    assert_eq!(accounts(&site.db).await, before);
    assert!(account_audit_lines(&site.db, ana).await.is_empty());
    let page = text(site.get("/staff/users?notice=account_exists", &staff).await).await;
    assert!(page.contains("An account with this email already exists."), "{page}");
    // Without the label, an ADMIN_EMAILS address gets its login.
    let response = site.post("/staff/users/new", &staff, "email=boss%40example.test&reason=x").await;
    let (boss, _, test) = account(&site.db, "boss@example.test").await.unwrap();
    assert_eq!(location(&response), format!("/staff/users/{boss}?notice=login_created"));
    assert!(!test);
}

/// A role goes only to an account that has signed in: until then nobody
/// has shown the address is theirs. Taking a role away is not held up.
#[sqlx::test(migrations = "./migrations")]
async fn a_role_waits_for_the_first_sign_in(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    site.post("/staff/users/new", &staff, "email=typo%40example.test&reason=New+hire").await;
    let (typo, _, _) = account(&site.db, "typo@example.test").await.unwrap();
    let user = format!("/staff/users/{typo}");
    let page = text(site.get(&user, &staff).await).await;
    assert!(
        page.contains("They have not signed in yet: a staff role can be given after their first sign-in."),
        "{page}"
    );
    for role in ["admin", "support"] {
        let response = site.post(&format!("{user}/role"), &staff, &format!("role={role}&reason=Joins")).await;
        assert_eq!(location(&response), format!("{user}?notice=not_signed_in_yet"), "{role}");
    }
    assert_eq!(account(&site.db, "typo@example.test").await.unwrap().1, None);
    let page = text(site.get(&format!("{user}?notice=not_signed_in_yet"), &staff).await).await;
    assert!(page.contains("They must sign in once first, so the address is proven."), "{page}");

    // A role the seed tool gave before any sign-in can still be taken away.
    sqlx::query("UPDATE accounts SET role = 'support' WHERE id = $1").bind(typo).execute(&site.db).await.unwrap();
    let response = site.post(&format!("{user}/role"), &staff, "role=admin&reason=Promote").await;
    assert_eq!(location(&response), format!("{user}?notice=not_signed_in_yet"));
    let response = site.post(&format!("{user}/role"), &staff, "role=none&reason=Wrong+address").await;
    assert_eq!(location(&response), format!("{user}?notice=role_changed"));
    assert_eq!(account(&site.db, "typo@example.test").await.unwrap().1, None);

    // Once they signed in (an ended session counts), the role goes in.
    let cookie = site.person_session(typo).await;
    site.post("/logout", &cookie, "").await;
    let page = text(site.get(&user, &staff).await).await;
    assert!(!page.contains("They have not signed in yet"), "{page}");
    let response = site.post(&format!("{user}/role"), &staff, "role=support&reason=Joins").await;
    assert_eq!(location(&response), format!("{user}?notice=role_changed"));
    assert_eq!(account(&site.db, "typo@example.test").await.unwrap().1.as_deref(), Some("support"));
}

/// An Admin who never signed in is no other Admin: it does not count
/// towards "someone stays Admin" when an Admin is demoted.
#[sqlx::test(migrations = "./migrations")]
async fn an_admin_who_never_signed_in_does_not_keep_buwiz_in_charge(db: PgPool) {
    let site = Site::with_admins(db, &[]);
    let (ana, _) = site.staff_with_role("ana@example.test", "admin").await;
    let (bob, _) = site.staff_with_role("bob@example.test", "admin").await;
    // Made by the seed tool, say: an Admin on record, never signed in.
    sqlx::query("INSERT INTO accounts (email, role) VALUES ('typo@example.test', 'admin')")
        .execute(&site.db)
        .await
        .unwrap();
    assert_eq!(roles::other_admins(&site.db, &[], None).await.unwrap(), 2);
    assert_eq!(roles::admins_on_record(&site.db, &[], None).await.unwrap(), 3);
    // Two Admins demoting each other, both past the page's check: the
    // second is refused as the last Admin, not let through on the
    // strength of one nobody ever signed in as.
    let config = &site.state.config;
    let ana_staff = as_staff(&site, ana).await;
    let bob_staff = as_staff(&site, bob).await;
    assert_eq!(
        admin::change_role(&site.db, config, &ana_staff, bob, None, "Leaves").await.unwrap(),
        Notice::RoleChanged
    );
    let last = admin::change_role(&site.db, config, &bob_staff, ana, None, "Leaves too").await;
    assert!(matches!(last, Err(DecideError::Refused(Notice::LastAdmin))), "{last:?}");
    assert_eq!(account(&site.db, "ana@example.test").await.unwrap().1.as_deref(), Some("admin"));
    // An address in ADMIN_EMAILS that never signed in counts no more.
    let listed = vec!["typo@example.test".to_owned()];
    sqlx::query("UPDATE accounts SET role = NULL WHERE email = 'typo@example.test'").execute(&site.db).await.unwrap();
    assert_eq!(roles::other_admins(&site.db, &listed, Some(ana)).await.unwrap(), 0);
    assert_eq!(roles::admins_on_record(&site.db, &listed, Some(ana)).await.unwrap(), 1);
}

async fn as_staff(site: &Site, id: Uuid) -> roles::Staff {
    let email: String =
        sqlx::query_scalar("SELECT email FROM accounts WHERE id = $1").bind(id).fetch_one(&site.db).await.unwrap();
    let account = buwiz_cloud::accounts::Account { id, email };
    roles::staff(&site.db, &site.state.config, &account).await.unwrap().unwrap()
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
