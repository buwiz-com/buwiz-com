//! A server that is stopping (SIGTERM came) refuses every request that
//! would start a send, before anything starts, and answers the rest as
//! before. Made-up data and the local test database only.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{Site, text};
use serde_json::Value;
use sqlx::PgPool;

fn request(method: &str, path: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("authorization", "Bearer bwa_made-up")
        .body(Body::from(if method == "POST" { "{}" } else { "" }))
        .unwrap()
}

const SENDS: [(&str, &str); 4] = [
    ("POST", "/api/tickets"),
    ("POST", "/api/v2/submissions"),
    ("POST", "/api/submissions"),
    ("GET", "/api/submissions/00000000-0000-0000-0000-000000000abc/relay"),
];

#[sqlx::test]
async fn a_stopping_server_starts_no_send(db: PgPool) {
    let site = Site::new(db);
    // Before: each answers as it always did (here: the made-up token).
    for (method, path) in SENDS {
        let response = site.send(request(method, path)).await;
        assert_ne!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{method} {path}");
    }
    site.state.stopping.begin();
    for (method, path) in SENDS {
        let response = site.send(request(method, path)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{method} {path}");
        assert_eq!(response.headers().get("retry-after").unwrap(), "30");
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
        let body: Value = serde_json::from_str(&text(response).await).unwrap();
        assert_eq!(body["error"]["code"], "restarting", "a code no app reads as a TIN refusal");
    }
    // What starts no send is answered as before.
    let stopped = site;
    let running = Site::new(stopped.db.clone());
    for path in ["/.well-known/buwiz-trust.json", "/healthz", "/api/submissions"] {
        let before = running.send(request("GET", path)).await;
        let after = stopped.send(request("GET", path)).await;
        assert_eq!(after.status(), before.status(), "{path}");
        assert_eq!(text(after).await, text(before).await, "{path}");
    }
}
