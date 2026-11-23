//! What stands between the sign-in form and a mailed code: Turnstile, the
//! limit per network, and the limit on codes for addresses without an
//! account ([`buwiz_cloud::account::guard`], `accounts::issue_code`).
//! Each test gets its own database from `#[sqlx::test]`.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use sqlx::PgPool;

use common::*;

/// The one token the fake Turnstile accepts.
const GOOD_TOKEN: &str = "a-person";

/// A fake Turnstile: `success` only for [`GOOD_TOKEN`] with the site's
/// secret. Its URL.
async fn turnstile() -> String {
    let verify = |Json(asked): Json<Value>| async move {
        let success = asked["secret"] == TURNSTILE_SECRET_KEY && asked["response"] == GOOD_TOKEN;
        Json(json!({ "success": success }))
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/siteverify", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, Router::new().route("/siteverify", post(verify))).await.unwrap() });
    url
}

/// Asks for a code with `form`, from `address` when there is one.
async fn ask(site: &Site, form: &str, address: Option<&str>) -> Response {
    let mut request = Request::post("/auth/email/start")
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(address) = address {
        request = request.header("cf-connecting-ip", address);
    }
    site.send(request.body(Body::from(form.to_owned())).unwrap()).await
}

async fn codes(site: &Site) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM email_codes").fetch_one(&site.db).await.unwrap()
}

/// Whether `response` sends the browser back to the sign-in page with
/// `notice`.
fn refused_with(response: &Response, notice: &str) -> bool {
    let to = location(response);
    response.status() == StatusCode::SEE_OTHER && to.starts_with("/login?") && to.ends_with(&format!("notice={notice}"))
}

#[sqlx::test(migrations = "./migrations")]
async fn with_turnstile_on_only_a_token_it_accepts_gets_a_code(db: PgPool) {
    let site = Site::with_turnstile(db, &turnstile().await);
    for form in [
        "email=ana%40example.test",
        "email=ana%40example.test&cf-turnstile-response=",
        "email=ana%40example.test&cf-turnstile-response=a-script",
    ] {
        let response = ask(&site, form, None).await;
        assert!(refused_with(&response, "not_human"), "{form}: {}", location(&response));
    }
    assert_eq!(codes(&site).await, 0);

    let sent = ask(&site, &format!("email=ana%40example.test&cf-turnstile-response={GOOD_TOKEN}"), None).await;
    assert!(location(&sent).starts_with("/login/verify"), "{}", location(&sent));
    assert_eq!(codes(&site).await, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_widget_is_in_every_form_that_asks_for_a_code_only_when_turnstile_is_on(db: PgPool) {
    let pages = ["/login", "/signup", "/login/verify?email=ana%40example.test"];
    let widget = format!(r#"<div class="cf-turnstile" data-sitekey="{TURNSTILE_SITE_KEY}""#);
    let on = Site::with_turnstile(db.clone(), "http://127.0.0.1:9");
    for page in pages {
        let html = text(on.get(page, "").await).await;
        // One form on each page asks for a code, and the widget is in it.
        let form = html.split(r#"action="/auth/email/start">"#).nth(1).unwrap_or_else(|| panic!("{page}: {html}"));
        let form = &form[..form.find("</form>").unwrap()];
        assert!(form.contains(&widget), "{page}: {form}");
        assert_eq!(html.matches("challenges.cloudflare.com/turnstile").count(), 1, "{page}");
    }
    let not_human = text(on.get("/login?notice=not_human", "").await).await;
    assert!(not_human.contains("We could not confirm that you are not a robot."), "{not_human}");

    let off = Site::new(db);
    for page in pages {
        let html = text(off.get(page, "").await).await;
        assert!(!html.contains("turnstile"), "{page}: {html}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn a_network_is_refused_after_ten_codes_in_the_hour(db: PgPool) {
    let site = Site::new(db);
    // Ten addresses of one /24, a new email each time: the limit per
    // address never comes into it.
    for n in 0..10 {
        let sent = ask(&site, &format!("email=person{n}%40example.test"), Some(&format!("203.0.113.{n}"))).await;
        assert!(location(&sent).starts_with("/login/verify"), "{n}: {}", location(&sent));
    }
    let refused = ask(&site, "email=person10%40example.test", Some("203.0.113.77")).await;
    assert!(refused_with(&refused, "code_network_too_many"), "{}", location(&refused));
    assert_eq!(codes(&site).await, 10);
    // Someone on another network still gets a code.
    let sent = ask(&site, "email=person10%40example.test", Some("198.51.100.4")).await;
    assert!(location(&sent).starts_with("/login/verify"), "{}", location(&sent));
}

/// Where a request for a code was sent on to, without the expiry (it is
/// the time of the request).
fn code_page(response: &Response) -> String {
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let to = location(response);
    assert!(to.starts_with("/login/verify?"), "{to}");
    to.split('&').filter(|part| !part.starts_with("expires=")).collect::<Vec<_>>().join("&")
}

/// `count` codes for addresses without an account, made `age` ago.
async fn strangers_codes(site: &Site, count: i32, age: &str) {
    sqlx::query(
        "INSERT INTO email_codes (email, code_hash, expires_at, created_at)
         SELECT 'stranger' || n || '-' || $2 || '@example.test', '\\x00', now(), now() - $2::interval
         FROM generate_series(1, $1) n",
    )
    .bind(count)
    .bind(age)
    .execute(&site.db)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn new_addresses_share_an_hourly_limit_that_never_holds_back_an_account(db: PgPool) {
    let site = Site::new(db);
    site.person("ana@example.test").await;
    strangers_codes(&site, 19, "5 minutes").await;
    let sent = ask(&site, "email=new%40example.test", None).await;
    assert!(location(&sent).starts_with("/login/verify"), "{}", location(&sent));
    assert_eq!(codes(&site).await, 20);
    // That was the twentieth: the next new address gets no code, and the
    // same answer, so the page tells nothing about who has an account ...
    let held = ask(&site, "email=newer%40example.test", None).await;
    assert_eq!(code_page(&held), code_page(&sent).replace("new%40", "newer%40"));
    assert_eq!(codes(&site).await, 20);
    // ... and someone with an account signs in as ever.
    let sent = ask(&site, "email=ana%40example.test", None).await;
    assert!(location(&sent).starts_with("/login/verify"), "{}", location(&sent));
    assert_eq!(codes(&site).await, 21);
}

#[sqlx::test(migrations = "./migrations")]
async fn new_addresses_share_a_daily_limit_too(db: PgPool) {
    let site = Site::new(db);
    // Spread over the day, so no hour is full; codes older than a day
    // do not count.
    strangers_codes(&site, 59, "2 hours").await;
    strangers_codes(&site, 30, "25 hours").await;
    let sent = ask(&site, "email=new%40example.test", None).await;
    assert!(location(&sent).starts_with("/login/verify"), "{}", location(&sent));
    assert_eq!(codes(&site).await, 90);
    let held = ask(&site, "email=newer%40example.test", None).await;
    assert!(location(&held).starts_with("/login/verify"), "{}", location(&held));
    assert_eq!(codes(&site).await, 90);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_post_that_mails_nothing_does_not_use_up_its_networks_hour(db: PgPool) {
    let site = Site::new(db);
    let from = Some("203.0.113.5");
    // Five codes for one address, then that address is at its own limit.
    for n in 0..5 {
        let sent = ask(&site, "email=ana%40example.test", from).await;
        assert!(location(&sent).starts_with("/login/verify"), "{n}: {}", location(&sent));
    }
    for _ in 0..12 {
        let refused = ask(&site, "email=ana%40example.test", from).await;
        assert!(refused_with(&refused, "code_too_many"), "{}", location(&refused));
    }
    // The network still has the other five of its ten.
    for n in 0..5 {
        let sent = ask(&site, &format!("email=person{n}%40example.test"), from).await;
        assert!(location(&sent).starts_with("/login/verify"), "{n}: {}", location(&sent));
    }
    let refused = ask(&site, "email=person5%40example.test", from).await;
    assert!(refused_with(&refused, "code_network_too_many"), "{}", location(&refused));
}
