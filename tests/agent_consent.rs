//! Any AI agent can register itself with any name, so the consent page
//! never vouches for that name: it marks the app unverified and shows the
//! full origin the code goes to.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use sqlx::PgPool;

use common::*;

#[sqlx::test(migrations = "./migrations")]
async fn a_self_registered_app_is_shown_as_unverified_with_its_redirect_origin(db: PgPool) {
    let site = Site::new(db);
    let (_, cookie) = site.person("alice@example.test").await;
    let body = json!({ "client_name": "Claude", "redirect_uris": ["https://mallory.example:8443/cb"] });
    let request = Request::post("/oauth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = site.send(request).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let registered: Value = serde_json::from_str(&text(response).await).unwrap();
    let client_id = registered["client_id"].as_str().unwrap();

    let path = format!(
        "/oauth/authorize?client_id={client_id}&response_type=code&code_challenge={}&code_challenge_method=S256",
        "a".repeat(43)
    );
    let response = site.get(&path, &cookie).await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = text(response).await;
    assert!(page.contains("Unverified app"), "{page}");
    assert!(page.contains("An app calling itself <q>Claude</q>"), "{page}");
    assert!(!page.contains("<h1>Claude wants"), "{page}");
    assert!(page.contains("https://mallory.example:8443"), "{page}");
}
