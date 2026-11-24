//! The store review login (`REVIEW_LOGIN_EMAIL`, `REVIEW_LOGIN_CODE`): one
//! address whose sign-in code is fixed and never mailed, for the app stores'
//! reviewers ([`buwiz_cloud::base::config::ReviewLogin`],
//! `accounts::issue_code`). Each test gets its own database from
//! `#[sqlx::test]`.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use buwiz_cloud::accounts;
use buwiz_cloud::base::config::ReviewLogin;
use sqlx::PgPool;
use uuid::Uuid;

use common::*;

const REVIEWER: &str = "reviewer@example.test";
const CODE: &str = "481516";

fn login() -> ReviewLogin {
    ReviewLogin { email: REVIEWER.into(), code: CODE.into() }
}

/// An account with the test label, as an Admin makes it for the reviewers.
async fn test_account(site: &Site, email: &str) -> Uuid {
    let (id, _) = site.person(email).await;
    sqlx::query("UPDATE accounts SET test_account = true WHERE id = $1").bind(id).execute(&site.db).await.unwrap();
    id
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

#[sqlx::test(migrations = "./migrations")]
async fn the_review_login_signs_in_with_its_fixed_code_through_the_forms(db: PgPool) {
    let site = Site::with_review_login(db, REVIEWER, CODE);
    test_account(&site, REVIEWER).await;

    let asked = ask(&site, REVIEWER).await;
    assert!(location(&asked).starts_with("/login/verify"), "{}", location(&asked));
    assert!(!signed_in(&verify(&site, REVIEWER, "000000").await), "a wrong code must not sign in");
    assert!(signed_in(&verify(&site, REVIEWER, CODE).await));
}

#[sqlx::test(migrations = "./migrations")]
async fn the_fixed_code_is_issued_only_for_the_review_address_while_it_is_a_plain_test_account(db: PgPool) {
    let site = Site::new(db);
    let review = login();

    // No account yet: the setting alone opens nothing.
    let issued = accounts::issue_code(&site.db, REVIEWER, Some(&review)).await.unwrap();
    assert!(!issued.fixed);

    // An account without the test label.
    let (id, _) = site.person(REVIEWER).await;
    assert!(!accounts::issue_code(&site.db, REVIEWER, Some(&review)).await.unwrap().fixed);

    // With the label: the fixed code, marked so it is not mailed.
    sqlx::query("UPDATE accounts SET test_account = true WHERE id = $1").bind(id).execute(&site.db).await.unwrap();
    let issued = accounts::issue_code(&site.db, REVIEWER, Some(&review)).await.unwrap();
    assert!(issued.fixed);
    assert_eq!(issued.code, CODE);

    // Staff never sign in with it.
    sqlx::query("UPDATE accounts SET role = 'support' WHERE id = $1").bind(id).execute(&site.db).await.unwrap();
    assert!(!accounts::issue_code(&site.db, REVIEWER, Some(&review)).await.unwrap().fixed);

    // Another test account's code stays random, and so does every code on
    // a server without the setting.
    test_account(&site, "ana@example.test").await;
    assert!(!accounts::issue_code(&site.db, "ana@example.test", Some(&review)).await.unwrap().fixed);
    assert!(!accounts::issue_code(&site.db, "ana@example.test", None).await.unwrap().fixed);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_banned_review_account_gets_no_fixed_code(db: PgPool) {
    let site = Site::new(db);
    let id = test_account(&site, REVIEWER).await;
    sqlx::query("UPDATE accounts SET banned_at = now(), banned_by = id, ban_reason = 'Test' WHERE id = $1")
        .bind(id)
        .execute(&site.db)
        .await
        .unwrap();
    assert!(!accounts::issue_code(&site.db, REVIEWER, Some(&login())).await.unwrap().fixed);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_limits_on_codes_and_guesses_hold_for_the_fixed_code(db: PgPool) {
    let site = Site::with_review_login(db, REVIEWER, CODE);
    test_account(&site, REVIEWER).await;

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

    // Without a code that was asked for, the fixed code signs nobody in.
    sqlx::query("DELETE FROM email_codes").execute(&site.db).await.unwrap();
    assert!(!signed_in(&verify(&site, REVIEWER, CODE).await));
}

#[sqlx::test(migrations = "./migrations")]
async fn me_tells_the_app_to_practise_only_for_the_review_login(db: PgPool) {
    let site = Site::with_review_login(db, REVIEWER, CODE);
    let reviewer = test_account(&site, REVIEWER).await;
    // Another test account sends as usual: the label alone is not practice.
    let tester = test_account(&site, "tester@example.test").await;
    let (ana, _) = site.person("ana@example.test").await;
    for (account, practice) in [(reviewer, true), (tester, false), (ana, false)] {
        let token = app_token(&site.db, account).await;
        let request =
            Request::get("/me").header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap();
        let response = site.send(request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(&bytes(response).await).unwrap();
        assert_eq!(body["practice"], practice, "{body}");
        assert_eq!(body["user_id"], account.to_string(), "{body}");
    }
    // Without the label the review address is an ordinary account again.
    sqlx::query("UPDATE accounts SET test_account = false WHERE id = $1")
        .bind(reviewer)
        .execute(&site.db)
        .await
        .unwrap();
    let token = app_token(&site.db, reviewer).await;
    let request =
        Request::get("/me").header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes(site.send(request).await).await).unwrap();
    assert_eq!(body["practice"], false, "{body}");
}
