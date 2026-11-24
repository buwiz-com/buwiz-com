//! Fixed sign-in codes for test accounts (the app stores' review logins):
//! an Admin sets a six-digit code on a test account's staff page, and while
//! it stands a code asked for on the sign-in form is that one and nothing
//! is mailed (`admin::set_fixed_code`, `accounts::issue_code`). Each test
//! gets its own database from `#[sqlx::test]`.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use buwiz_cloud::base::crypto;
use sqlx::PgPool;
use uuid::Uuid;

use common::*;

const REVIEWER: &str = "reviewer@example.test";
const CODE: &str = "481516";

/// A site whose mail is kept, and an Admin (by role, not in
/// `ADMIN_EMAILS`) signed in: (site, Admin's id, Admin's cookie).
async fn site_with_admin(db: PgPool) -> (Site, Uuid, String) {
    let site = Site::with_mailbox(db);
    let (admin, cookie) = site.staff_with_role("boss@example.test", "admin").await;
    (site, admin, cookie)
}

/// An account with the test label, as an Admin makes it for the reviewers.
async fn test_account(site: &Site, email: &str) -> Uuid {
    let (id, _) = site.person(email).await;
    sqlx::query("UPDATE accounts SET test_account = true WHERE id = $1").bind(id).execute(&site.db).await.unwrap();
    id
}

/// The staff form that sets (or changes) the code: where it went.
async fn set_code(site: &Site, cookie: &str, account: Uuid, code: &str) -> String {
    let form = format!("code={code}&reason=Store+review&tab=account");
    location(&site.post(&format!("/staff/users/{account}/fixed-code"), cookie, &form).await)
}

/// The staff form that clears it: where it went.
async fn clear_code(site: &Site, cookie: &str, account: Uuid) -> String {
    let form = "reason=Review+over&tab=account";
    location(&site.post(&format!("/staff/users/{account}/fixed-code/clear"), cookie, form).await)
}

fn page(account: Uuid, notice: &str) -> String {
    format!("/staff/users/{account}?notice={notice}")
}

async fn has_code(site: &Site, account: Uuid) -> bool {
    sqlx::query_scalar("SELECT fixed_code_hash IS NOT NULL FROM accounts WHERE id = $1")
        .bind(account)
        .fetch_one(&site.db)
        .await
        .unwrap()
}

async fn ask(site: &Site, email: &str) -> Response {
    let request = Request::post("/auth/email/start")
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!("email={}", email.replace('@', "%40"))))
        .unwrap();
    site.send(request).await
}

async fn verify(site: &Site, email: &str, code: &str) -> Response {
    site.post("/auth/email/verify", "", &format!("email={}&code={code}", email.replace('@', "%40"))).await
}

fn signed_in(response: &Response) -> bool {
    response.headers().get(header::SET_COOKIE).is_some()
}

/// The codes mailed to `email`, oldest first.
fn mailed(site: &Site, email: &str) -> Vec<String> {
    site.state.mailer.sign_in_codes().into_iter().filter(|(to, _)| to == email).map(|(_, code)| code).collect()
}

/// `GET /me` for an app signed in to `account`: its `practice`, or `None`
/// when it does not answer 200.
async fn practice(site: &Site, account: Uuid) -> Option<bool> {
    let token = app_token(&site.db, account).await;
    let request =
        Request::get("/me").header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap();
    let response = site.send(request).await;
    if response.status() != StatusCode::OK {
        return None;
    }
    let body: serde_json::Value = serde_json::from_slice(&bytes(response).await).unwrap();
    assert_eq!(body["user_id"], account.to_string(), "{body}");
    Some(body["practice"].as_bool().expect("practice is a boolean"))
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_sets_a_code_and_the_sign_in_forms_take_it_with_nothing_mailed(db: PgPool) {
    let (site, _, admin) = site_with_admin(db).await;
    let reviewer = test_account(&site, REVIEWER).await;
    assert_eq!(set_code(&site, &admin, reviewer, CODE).await, page(reviewer, "fixed_code_set"));

    let asked = ask(&site, REVIEWER).await;
    assert!(location(&asked).starts_with("/login/verify"), "{}", location(&asked));
    assert!(mailed(&site, REVIEWER).is_empty(), "nothing is mailed for a fixed code");
    assert!(!signed_in(&verify(&site, REVIEWER, "000000").await), "a wrong code must not sign in");
    assert!(signed_in(&verify(&site, REVIEWER, CODE).await));

    // Another test account's code stays random and mailed.
    test_account(&site, "tester@example.test").await;
    ask(&site, "tester@example.test").await;
    assert_eq!(mailed(&site, "tester@example.test").len(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn support_and_plain_users_cannot_set_or_clear_it(db: PgPool) {
    let (site, _, admin) = site_with_admin(db).await;
    let reviewer = test_account(&site, REVIEWER).await;
    let (_, support) = site.staff_with_role("helper@example.test", "support").await;
    let (_, plain) = site.person("plain@example.test").await;

    let back = format!("/staff/users/{reviewer}?notice=admin_only");
    assert_eq!(set_code(&site, &support, reviewer, CODE).await, back);
    let response = site.post(&format!("/staff/users/{reviewer}/fixed-code"), &plain, "code=481516&reason=x").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(!has_code(&site, reviewer).await);

    set_code(&site, &admin, reviewer, CODE).await;
    assert_eq!(clear_code(&site, &support, reviewer).await, back);
    let response = site.post(&format!("/staff/users/{reviewer}/fixed-code/clear"), &plain, "reason=x").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(has_code(&site, reviewer).await);
}

#[sqlx::test(migrations = "./migrations")]
async fn it_is_set_only_on_a_plain_test_account_that_is_not_ones_own(db: PgPool) {
    let (site, admin_id, admin) = site_with_admin(db).await;
    // No test label.
    let (plain, _) = site.person("plain@example.test").await;
    // A staff role.
    let staff = test_account(&site, "helper@example.test").await;
    sqlx::query("UPDATE accounts SET role = 'support' WHERE id = $1").bind(staff).execute(&site.db).await.unwrap();
    // In ADMIN_EMAILS.
    let listed = test_account(&site, STAFF).await;
    // Banned.
    let banned = test_account(&site, "banned@example.test").await;
    sqlx::query("UPDATE accounts SET banned_at = now(), banned_by = $2, ban_reason = 'Abuse' WHERE id = $1")
        .bind(banned)
        .bind(admin_id)
        .execute(&site.db)
        .await
        .unwrap();
    // The Admin's own, labelled too.
    sqlx::query("UPDATE accounts SET test_account = true WHERE id = $1")
        .bind(admin_id)
        .execute(&site.db)
        .await
        .unwrap();
    for (account, notice) in [
        (plain, "fixed_code_not_test"),
        (staff, "fixed_code_staff"),
        (listed, "fixed_code_from_server"),
        (banned, "fixed_code_banned"),
        (admin_id, "own_account"),
    ] {
        assert_eq!(set_code(&site, &admin, account, CODE).await, page(account, notice));
        assert!(!has_code(&site, account).await, "{notice}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn a_code_that_is_not_six_digits_is_refused(db: PgPool) {
    let (site, _, admin) = site_with_admin(db).await;
    let reviewer = test_account(&site, REVIEWER).await;
    for code in [
        "48151",
        "4815162",
        "48151a",
        "abcdef",
        "481%20516",
        "%EF%BC%94%EF%BC%98%EF%BC%91%EF%BC%95%EF%BC%91%EF%BC%96",
        "",
    ] {
        assert_eq!(set_code(&site, &admin, reviewer, code).await, page(reviewer, "fixed_code_digits"), "{code}");
    }
    assert!(!has_code(&site, reviewer).await);
    // A reason is required too.
    let response = site.post(&format!("/staff/users/{reviewer}/fixed-code"), &admin, "code=481516&reason=+").await;
    assert_eq!(location(&response), page(reviewer, "reason_missing"));
    assert!(!has_code(&site, reviewer).await);
}

#[sqlx::test(migrations = "./migrations")]
async fn clearing_it_brings_back_the_random_mailed_code(db: PgPool) {
    let (site, _, admin) = site_with_admin(db).await;
    let reviewer = test_account(&site, REVIEWER).await;
    set_code(&site, &admin, reviewer, CODE).await;
    // A code asked for before the clear no longer signs in.
    ask(&site, REVIEWER).await;
    assert_eq!(clear_code(&site, &admin, reviewer).await, page(reviewer, "fixed_code_cleared"));
    assert!(!signed_in(&verify(&site, REVIEWER, CODE).await));
    assert!(!has_code(&site, reviewer).await);
    // Clearing again changes nothing, and says so.
    assert_eq!(clear_code(&site, &admin, reviewer).await, page(reviewer, "fixed_code_none"));

    ask(&site, REVIEWER).await;
    let codes = mailed(&site, REVIEWER);
    assert_eq!(codes.len(), 1, "a random code is mailed again");
    assert!(signed_in(&verify(&site, REVIEWER, &codes[0]).await));
}

#[sqlx::test(migrations = "./migrations")]
async fn changing_it_closes_the_old_one(db: PgPool) {
    let (site, _, admin) = site_with_admin(db).await;
    let reviewer = test_account(&site, REVIEWER).await;
    set_code(&site, &admin, reviewer, CODE).await;
    ask(&site, REVIEWER).await;
    assert_eq!(set_code(&site, &admin, reviewer, "907312").await, page(reviewer, "fixed_code_changed"));
    assert!(!signed_in(&verify(&site, REVIEWER, CODE).await), "a code asked for before the change is closed");
    ask(&site, REVIEWER).await;
    assert!(!signed_in(&verify(&site, REVIEWER, CODE).await), "the old code is gone");
    assert!(signed_in(&verify(&site, REVIEWER, "907312").await));
    assert!(mailed(&site, REVIEWER).is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn a_ban_or_a_staff_role_makes_the_next_code_random_and_mailed(db: PgPool) {
    let (site, _, admin) = site_with_admin(db).await;
    let reviewer = test_account(&site, REVIEWER).await;
    set_code(&site, &admin, reviewer, CODE).await;
    assert_eq!(practice(&site, reviewer).await, Some(true));

    // A staff role: the code stays stored but stands in for nothing.
    let role = site.post(&format!("/staff/users/{reviewer}/role"), &admin, "role=support&reason=Joins").await;
    assert_eq!(location(&role), page(reviewer, "role_changed"));
    ask(&site, REVIEWER).await;
    assert_eq!(mailed(&site, REVIEWER).len(), 1);
    assert!(!signed_in(&verify(&site, REVIEWER, CODE).await) || mailed(&site, REVIEWER)[0] == CODE);
    assert_eq!(practice(&site, reviewer).await, Some(false));
    // The role taken away: it is a plain test account again.
    site.post(&format!("/staff/users/{reviewer}/role"), &admin, "role=none&reason=Left").await;
    assert_eq!(practice(&site, reviewer).await, Some(true));
    ask(&site, REVIEWER).await;
    assert_eq!(mailed(&site, REVIEWER).len(), 1, "nothing more was mailed");

    // A ban: random and mailed too, and the app gets no practice answer.
    let ban = site.post(&format!("/staff/users/{reviewer}/ban"), &admin, "reason=Abuse").await;
    assert_eq!(location(&ban), page(reviewer, "account_banned_staff"));
    ask(&site, REVIEWER).await;
    assert_eq!(mailed(&site, REVIEWER).len(), 2);
    assert_ne!(practice(&site, reviewer).await, Some(true));
    assert!(has_code(&site, reviewer).await);
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_emails_address_never_signs_in_with_one(db: PgPool) {
    let (site, _, admin) = site_with_admin(db).await;
    let reviewer = test_account(&site, REVIEWER).await;
    set_code(&site, &admin, reviewer, CODE).await;
    // The address is put in ADMIN_EMAILS later.
    let listed = Site::with_admins(site.db.clone(), &[REVIEWER]);
    let issued = buwiz_cloud::accounts::issue_code(&listed.db, REVIEWER, &[REVIEWER.to_owned()]).await.unwrap();
    assert!(matches!(issued, buwiz_cloud::accounts::IssuedCode::Mail(_)), "{issued:?}");
    assert_eq!(practice(&listed, reviewer).await, Some(false));
    assert_eq!(practice(&site, reviewer).await, Some(true));
}

#[sqlx::test(migrations = "./migrations")]
async fn the_label_cannot_come_off_while_a_code_is_set(db: PgPool) {
    let (site, _, admin) = site_with_admin(db).await;
    let reviewer = test_account(&site, REVIEWER).await;
    set_code(&site, &admin, reviewer, CODE).await;
    let off = format!("/staff/users/{reviewer}/test");
    let removed = site.post(&off, &admin, "test=off&reason=Done").await;
    assert_eq!(location(&removed), page(reviewer, "fixed_code_held"));
    let labelled: bool = sqlx::query_scalar("SELECT test_account FROM accounts WHERE id = $1")
        .bind(reviewer)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert!(labelled);
    // The page offers no Remove while it is set, and says why.
    let shown = text(site.get(&format!("/staff/users/{reviewer}"), &admin).await).await;
    assert!(shown.contains("clear it before you remove the test label"), "{shown}");
    assert!(!shown.contains("Remove the test label from"));

    clear_code(&site, &admin, reviewer).await;
    let removed = site.post(&off, &admin, "test=off&reason=Done").await;
    assert_eq!(location(&removed), page(reviewer, "test_unmarked"));
}

#[sqlx::test(migrations = "./migrations")]
async fn the_limits_on_codes_and_guesses_hold_for_the_fixed_code(db: PgPool) {
    let (site, _, admin) = site_with_admin(db).await;
    let reviewer = test_account(&site, REVIEWER).await;
    set_code(&site, &admin, reviewer, CODE).await;

    // Without a code asked for, the fixed code signs nobody in.
    assert!(!signed_in(&verify(&site, REVIEWER, CODE).await));

    // Five wrong guesses close the code that was asked for, fixed or not.
    ask(&site, REVIEWER).await;
    for _ in 0..5 {
        assert!(!signed_in(&verify(&site, REVIEWER, "111111").await));
    }
    assert!(!signed_in(&verify(&site, REVIEWER, CODE).await), "the code was closed by the wrong guesses");

    // Asking again opens it, up to the limit per address in an hour.
    ask(&site, REVIEWER).await;
    assert!(signed_in(&verify(&site, REVIEWER, CODE).await));
    for _ in 0..3 {
        ask(&site, REVIEWER).await;
    }
    let sixth = ask(&site, REVIEWER).await;
    assert!(location(&sixth).contains("notice=code_too_many"), "{}", location(&sixth));
    assert!(mailed(&site, REVIEWER).is_empty());

    // A code lasts ten minutes, fixed or not.
    sqlx::query("DELETE FROM email_codes").execute(&site.db).await.unwrap();
    ask(&site, REVIEWER).await;
    sqlx::query("UPDATE email_codes SET expires_at = now() - interval '1 second'").execute(&site.db).await.unwrap();
    assert!(!signed_in(&verify(&site, REVIEWER, CODE).await));
}

#[sqlx::test(migrations = "./migrations")]
async fn me_tells_the_app_to_practise_only_while_a_fixed_code_stands(db: PgPool) {
    let (site, _, admin) = site_with_admin(db).await;
    let reviewer = test_account(&site, REVIEWER).await;
    // Another test account sends as usual: the label alone is not practice.
    let tester = test_account(&site, "tester@example.test").await;
    let (ana, _) = site.person("ana@example.test").await;
    for account in [reviewer, tester, ana] {
        assert_eq!(practice(&site, account).await, Some(false));
    }
    set_code(&site, &admin, reviewer, CODE).await;
    for (account, expected) in [(reviewer, true), (tester, false), (ana, false)] {
        assert_eq!(practice(&site, account).await, Some(expected), "{account}");
    }
    clear_code(&site, &admin, reviewer).await;
    assert_eq!(practice(&site, reviewer).await, Some(false));
}

#[sqlx::test(migrations = "./migrations")]
async fn every_change_is_in_the_history_without_the_code(db: PgPool) {
    let (site, _, admin) = site_with_admin(db).await;
    let reviewer = test_account(&site, REVIEWER).await;
    set_code(&site, &admin, reviewer, CODE).await;
    set_code(&site, &admin, reviewer, "907312").await;
    clear_code(&site, &admin, reviewer).await;
    let lines = account_audit_lines(&site.db, reviewer).await;
    let line = |action: &str, reason: &str| {
        (action.to_owned(), None, None, "boss@example.test".to_owned(), "web".to_owned(), Some(reason.to_owned()))
    };
    assert_eq!(
        lines,
        [
            line("fixed_code_set", "Store review"),
            line("fixed_code_changed", "Store review"),
            line("fixed_code_cleared", "Review over")
        ]
    );
    // No column of any history line holds either code or its hash.
    let all: Vec<String> =
        sqlx::query_scalar("SELECT to_jsonb(l)::text FROM account_audit l").fetch_all(&site.db).await.unwrap();
    for code in [CODE, "907312"] {
        let hash = crypto::hex(&crypto::hash(&format!("{REVIEWER}:{code}")));
        for row in &all {
            assert!(!row.contains(code) && !row.contains(&hash), "{row}");
        }
    }
    // The history tab names each in words.
    let history = text(site.get(&format!("/staff/users/{reviewer}?tab=history"), &admin).await).await;
    for words in ["set a fixed sign-in code", "changed the fixed sign-in code", "cleared the fixed sign-in code"] {
        assert!(history.contains(words), "{words}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn the_pages_show_that_a_code_is_set_and_never_the_code(db: PgPool) {
    let (site, _, admin) = site_with_admin(db).await;
    let reviewer = test_account(&site, REVIEWER).await;
    let user = format!("/staff/users/{reviewer}");

    // Before: the control on the account's page, and the inspector's link
    // to it.
    let shown = text(site.get(&user, &admin).await).await;
    assert!(shown.contains(r#"id="fixed-code""#));
    assert!(shown.contains(r#"pattern="[0-9]{6}""#) && shown.contains(r#"inputmode="numeric""#));
    assert!(shown.contains(&format!("{user}/fixed-code")));
    let inspector = text(site.get(&format!("/staff/users?open={reviewer}"), &admin).await).await;
    assert!(inspector.contains(&format!("{user}#fixed-code")), "the users list leads to the control");

    set_code(&site, &admin, reviewer, CODE).await;
    for path in [
        user.clone(),
        format!("{user}?tab=history"),
        format!("{user}?notice=fixed_code_set"),
        "/staff/users".to_owned(),
        format!("/staff/users?open={reviewer}"),
        "/staff/users?filter=test".to_owned(),
        "/staff".to_owned(),
    ] {
        let shown = text(site.get(&path, &admin).await).await;
        assert!(!shown.contains(CODE), "{path} shows the code");
        if path.starts_with("/staff/users") {
            assert!(shown.contains("Fixed code"), "{path} shows no Fixed code pill");
        }
    }
    let shown = text(site.get(&user, &admin).await).await;
    assert!(shown.contains("Set by boss@example.test on"), "who set it");
    let list = text(site.get("/staff/users", &admin).await).await;
    assert!(list_part(&list).contains("Fixed code"), "the row has the pill");

    // Support sees that one is set, and no control.
    let (_, support) = site.staff_with_role("helper@example.test", "support").await;
    let shown = text(site.get(&user, &support).await).await;
    assert!(shown.contains("Fixed code") && !shown.contains(&format!("{user}/fixed-code")));
    let inspector = text(site.get(&format!("/staff/users?open={reviewer}"), &support).await).await;
    assert!(!inspector.contains(&format!("{user}#fixed-code")));

    // An account that cannot have one says why, and offers nothing.
    let (plain, _) = site.person("plain@example.test").await;
    let shown = text(site.get(&format!("/staff/users/{plain}"), &admin).await).await;
    assert!(shown.contains("Only a test account gets one"));
    assert!(!shown.contains(&format!("/staff/users/{plain}/fixed-code")));
}
