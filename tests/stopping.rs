//! A server that is stopping (SIGTERM came) refuses every request that
//! would start a send, before anything starts, and answers the rest as
//! before; the server process takes SIGUSR1 at any time and stops cleanly
//! on SIGTERM. Made-up data and the local test database only.

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

/// The server process: SIGUSR1 while it starts, while it serves and while
/// it stops changes nothing but the BIR sign reading its note (the default
/// action would end the process); SIGTERM then stops it with status 0, its
/// last log line written out. Only loopback and the test's own database.
#[cfg(unix)]
#[sqlx::test]
async fn the_server_process_takes_sigusr1_and_stops_cleanly(db: PgPool) {
    use std::time::{Duration, Instant};

    let name: String = sqlx::query_scalar("SELECT current_database()").fetch_one(&db).await.unwrap();
    let mut url = url::Url::parse(&std::env::var("DATABASE_URL").unwrap()).unwrap();
    assert!(matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")), "{:?}", url.host_str());
    url.set_path(&name);
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let dir = std::env::temp_dir().join(format!("stopping-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let log = dir.join("server.log");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_buwiz-cloud"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("DATABASE_URL", url.as_str())
        // A made-up key: 32 bytes of 7.
        .env("DOCUMENT_KEY", "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=")
        .env("PORT", port.to_string())
        .env("RUST_LOG", "info")
        .env("GITHUB_API_URL", "http://127.0.0.1:9")
        .env("DOWNLOADS_URL", "http://127.0.0.1:9")
        .current_dir(&dir)
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(std::fs::File::create(dir.join("server.err")).unwrap())
        .spawn()
        .unwrap();
    let pid = i32::try_from(child.id()).unwrap();
    let signal = |number: i32| {
        // SAFETY: sends a signal to the child this test started.
        assert_eq!(unsafe { libc::kill(pid, number) }, 0);
    };
    let alive = |child: &mut std::process::Child| child.try_wait().unwrap().is_none();
    // While it starts (from its first log line, written once it catches
    // SIGUSR1: the first instant of a process cannot), until its port
    // answers.
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::fs::read_to_string(&log).unwrap().is_empty() {
        assert!(
            alive(&mut child),
            "the server ended before it logged: {}",
            std::fs::read_to_string(dir.join("server.err")).unwrap()
        );
        assert!(Instant::now() < deadline, "the server logged nothing");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    loop {
        assert!(alive(&mut child), "the server ended while starting");
        signal(libc::SIGUSR1);
        if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            break;
        }
        assert!(Instant::now() < deadline, "the server did not start");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // While it serves.
    for _ in 0..3 {
        signal(libc::SIGUSR1);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(alive(&mut child), "SIGUSR1 ended the server");
    // While it stops.
    signal(libc::SIGTERM);
    signal(libc::SIGUSR1);
    let status = tokio::task::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "the server did not stop");
            std::thread::sleep(Duration::from_millis(50));
        }
    })
    .await
    .unwrap();
    assert_eq!(status.code(), Some(0), "{status:?}");
    let logged = std::fs::read_to_string(&log).unwrap();
    assert!(logged.contains("BIR uptime: SIGUSR1; reading the stored note"), "{logged}");
    assert!(logged.trim_end().ends_with("stopped"), "the last line is written out: {logged}");
    let _ = std::fs::remove_dir_all(&dir);
}
