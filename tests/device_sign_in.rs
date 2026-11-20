//! App sign-in (RFC 8628 device flow): the person types the code the app
//! shows. A link with the code filled in, which anyone could send, is not
//! enough to approve it.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use sqlx::PgPool;

use common::*;

async fn start(site: &Site) -> Value {
    let request = Request::post("/auth/device/start")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "client_id": "buwiz-desktop" }).to_string()))
        .unwrap();
    let response = site.send(request).await;
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_str(&text(response).await).unwrap()
}

async fn decision(db: &PgPool, user_code: &str) -> Option<String> {
    sqlx::query_scalar("SELECT decision FROM device_authorizations WHERE user_code = $1")
        .bind(user_code)
        .fetch_one(db)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_sign_in_link_never_carries_or_fills_in_the_code(db: PgPool) {
    let site = Site::new(db);
    let (_, cookie) = site.person("ana@example.test").await;
    let started = start(&site).await;
    let user_code = started["user_code"].as_str().unwrap();
    for field in ["verification_uri", "verification_uri_complete"] {
        let uri = started[field].as_str().unwrap();
        assert!(uri.ends_with("/device") && !uri.contains(user_code), "{field}: {uri}");
    }

    // A link made by hand, with the code in it: the page shows only the
    // empty code field, and nothing is approved.
    let page = text(site.get(&format!("/device?user_code={user_code}"), &cookie).await).await;
    assert!(!page.contains(user_code), "{page}");
    assert!(page.contains(r#"name="user_code""#) && page.contains("Type the code the Buwiz app shows you."), "{page}");
    assert!(!page.contains(r#"type="hidden" name="user_code""#), "{page}");
    assert_eq!(decision(&site.db, user_code).await, None);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_typed_code_is_approved_and_the_app_signs_in(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person("ana@example.test").await;
    let started = start(&site).await;
    let user_code = started["user_code"].as_str().unwrap();

    let typed = user_code.to_lowercase().replace('-', "");
    let response = site.post("/device/decision", &cookie, &format!("user_code={typed}&decision=approve")).await;
    assert_eq!(location(&response), "/device?notice=device_approved");
    assert_eq!(decision(&site.db, user_code).await.as_deref(), Some("approved"));
    let approved_by: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT account_id FROM device_authorizations WHERE user_code = $1")
            .bind(user_code)
            .fetch_one(&site.db)
            .await
            .unwrap();
    assert_eq!(approved_by, Some(ana));

    // A wrong code approves nothing.
    let response = site.post("/device/decision", &cookie, "user_code=ZZZZ-ZZZZ&decision=approve").await;
    assert_eq!(location(&response), "/device?notice=device_invalid");
}

#[sqlx::test(migrations = "./migrations")]
async fn signing_in_first_comes_back_to_the_empty_code_page(db: PgPool) {
    let site = Site::new(db);
    let response = site.post("/device/decision", "", "user_code=BCDF-GHJK&decision=approve").await;
    let to = location(&response);
    assert!(to.starts_with("/login?") && !to.contains("BCDF"), "{to}");
}
