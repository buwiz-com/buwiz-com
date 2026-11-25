//! In-app email sign-in (`/auth/email/start`, `/resend`, `/verify` in
//! JSON; [`buwiz_cloud::account::app_sign_in`]): the code is the web's,
//! under the web's limits, and only the device holding the request's
//! secret can use it. Also the testing cap on requests without an app
//! check. Each test gets its own database from `#[sqlx::test]`.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use buwiz_cloud::base::crypto;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use common::*;

const ANA: &str = "ana@example.test";
/// The eBIRForms app's client id on every platform.
const CLIENT: &str = "buwiz-desktop";
/// A token the fake Turnstile accepts from the app-check page's widget.
const GOOD_TOKEN: &str = "an-app";
/// A token it accepts from the sign-in form's widget.
const WEB_TOKEN: &str = "a-person";
/// A token it accepts from the app-check widget on another host.
const OTHER_HOST_TOKEN: &str = "elsewhere";

/// A fake Turnstile, with the site's secret: [`GOOD_TOKEN`] passes as the
/// app check on the test site's host, [`WEB_TOKEN`] as the sign-in form,
/// [`OTHER_HOST_TOKEN`] as the app check on another host. Its URL.
async fn turnstile() -> String {
    turnstile_counted().await.0
}

/// [`turnstile`], with how many times it was asked. Its answers for tokens
/// `web-no-hostname`, `web-no-action`, `app-no-hostname` and
/// `app-no-action` pass, without that field.
async fn turnstile_counted() -> (String, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let verify = move |Json(asked): Json<Value>| {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            let (action, hostname) = match asked["response"].as_str() {
                Some(GOOD_TOKEN) => (Some("app-check"), Some("localhost")),
                Some(WEB_TOKEN) => (Some("sign-in"), Some("localhost")),
                Some(OTHER_HOST_TOKEN) => (Some("app-check"), Some("evil.example.test")),
                Some("web-no-hostname") => (Some("sign-in"), None),
                Some("web-no-action") => (None, Some("localhost")),
                Some("app-no-hostname") => (Some("app-check"), None),
                Some("app-no-action") => (None, Some("localhost")),
                _ => return Json(json!({ "success": false })),
            };
            let mut answer = json!({ "success": asked["secret"] == TURNSTILE_SECRET_KEY });
            if let Some(action) = action {
                answer["action"] = json!(action);
            }
            if let Some(hostname) = hostname {
                answer["hostname"] = json!(hostname);
            }
            Json(answer)
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/siteverify", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, Router::new().route("/siteverify", post(verify))).await.unwrap() });
    (url, calls)
}

/// A secret as the app makes one, and the hash it sends at the start.
fn secret() -> (String, String) {
    let secret = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let hash = URL_SAFE_NO_PAD.encode(Sha256::digest(secret.as_bytes()));
    (secret, hash)
}

/// The API host, where the apps post (`api.buwiz.com` in production).
const API_HOST: &str = "api.localhost:3008";

async fn post_json(site: &Site, path: &str, body: &Value) -> Response {
    post_json_to(site, API_HOST, path, body).await
}

async fn post_json_to(site: &Site, host: &str, path: &str, body: &Value) -> Response {
    let request = Request::post(path)
        .header(header::HOST, host)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    site.send(request).await
}

/// The status and JSON of an answer.
async fn read(response: Response) -> (u16, Value) {
    let status = response.status().as_u16();
    (status, serde_json::from_str(&text(response).await).unwrap())
}

/// A started request: its id and secret.
struct Started {
    id: String,
    secret: String,
}

async fn start_with(site: &Site, email: &str, extra: Value) -> (u16, Value, String) {
    let (secret, hash) = secret();
    let mut body = json!({ "client_id": CLIENT, "email": email, "intent": "sign_in", "secret_hash": hash });
    for (key, value) in extra.as_object().unwrap() {
        body[key] = value.clone();
    }
    let (status, answer) = read(post_json(site, "/auth/email/start", &body).await).await;
    (status, answer, secret)
}

async fn start(site: &Site, email: &str) -> Started {
    let (status, answer, secret) = start_with(site, email, json!({})).await;
    assert_eq!(status, 200, "{answer}");
    Started { id: answer["request_id"].as_str().unwrap().to_owned(), secret }
}

async fn verify(site: &Site, started: &Started, code: &str) -> (u16, Value) {
    let body = json!({ "request_id": started.id, "secret": started.secret, "code": code });
    read(post_json(site, "/auth/email/verify", &body).await).await
}

async fn resend(site: &Site, started: &Started) -> (u16, Value) {
    let body = json!({ "request_id": started.id, "secret": started.secret });
    read(post_json(site, "/auth/email/resend", &body).await).await
}

/// The newest code mailed to `email` by an app's request.
fn last_code(site: &Site, email: &str) -> String {
    site.state.mailer.app_sign_in_mails().iter().rev().find(|mail| mail.to == email).unwrap().code.clone()
}

/// Ends the wait: the request's code expires, so it may be resent.
async fn expire(site: &Site, started: &Started) {
    sqlx::query(
        "WITH r AS (UPDATE app_sign_ins SET expires_at = now() - interval '1 second' WHERE id = $1::uuid RETURNING code_id)
         UPDATE email_codes SET expires_at = now() - interval '1 second' WHERE id = (SELECT code_id FROM r)",
    )
    .bind(&started.id)
    .execute(&site.db)
    .await
    .unwrap();
}

async fn count(db: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(db).await.unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_code_typed_in_the_app_gives_the_device_flows_tokens(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (status, answer, secret) = start_with(&site, ANA, json!({})).await;
    assert_eq!(status, 200);
    let keys: Vec<&String> = answer.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["code_length", "expires_in", "request_id", "resend_after"]);
    assert_eq!((answer["expires_in"].as_i64(), answer["resend_after"].as_i64()), (Some(300), Some(300)));
    assert_eq!(answer["code_length"], 6);
    let started = Started { id: answer["request_id"].as_str().unwrap().to_owned(), secret };
    // No mail for the web; one for the app.
    assert!(site.state.mailer.sign_in_codes().is_empty());
    let code = last_code(&site, ANA);

    let (status, tokens) = verify(&site, &started, &code).await;
    assert_eq!(status, 200, "{tokens}");
    let keys: Vec<&String> = tokens.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["access_token", "expires_in", "refresh_token", "scope", "token_type"]);
    assert_eq!((tokens["token_type"].as_str(), tokens["scope"].as_str()), (Some("Bearer"), Some("app")));
    assert_eq!(tokens["expires_in"], 900);
    let client: String = sqlx::query_scalar("SELECT client_id FROM oauth_grants").fetch_one(&site.db).await.unwrap();
    assert_eq!(client, CLIENT);

    let me = Request::get("/me")
        .header(header::AUTHORIZATION, format!("Bearer {}", tokens["access_token"].as_str().unwrap()))
        .body(Body::empty())
        .unwrap();
    let (status, me) = read(site.send(me).await).await;
    assert_eq!((status, me["email"].as_str()), (200, Some(ANA)));

    let refresh = Request::post("/oauth/token")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!(
            "grant_type=refresh_token&client_id={CLIENT}&refresh_token={}",
            tokens["refresh_token"].as_str().unwrap()
        )))
        .unwrap();
    let (status, refreshed) = read(site.send(refresh).await).await;
    assert_eq!(status, 200, "{refreshed}");
    assert_eq!(refreshed["scope"], "app");

    // The request is spent.
    assert_eq!(verify(&site, &started, &code).await, (400, refused("invalid_request")));
}

/// `{"error": code}` with the message of that code, for comparing whole
/// answers.
fn refused(code: &str) -> Value {
    let message = match code {
        "invalid_request" => "This sign-in request is not valid any more. Start again.",
        "code_expired" => "This code expired. Ask for a new one.",
        "too_many_attempts" => "That code was tried too many times. Ask for a new one.",
        other => panic!("{other}"),
    };
    json!({ "error": code, "message": message })
}

#[sqlx::test(migrations = "./migrations")]
async fn a_new_address_gets_an_account_and_the_same_answer_as_a_known_one(db: PgPool) {
    let site = Site::with_mailbox(db);
    site.person(ANA).await;
    let (_, mut known, _) = start_with(&site, ANA, json!({})).await;
    let (_, mut unknown, _) = start_with(&site, "new@example.test", json!({ "intent": "create" })).await;
    for answer in [&mut known, &mut unknown] {
        answer.as_object_mut().unwrap().remove("request_id");
    }
    assert_eq!(known, unknown);

    let started = start(&site, "new@example.test").await;
    let (status, _) = verify(&site, &started, &last_code(&site, "new@example.test")).await;
    assert_eq!(status, 200);
    assert_eq!(count(&site.db, "SELECT count(*) FROM accounts WHERE email = 'new@example.test'").await, 1);
    // The create wording, for the address that asked to create.
    let mails = site.state.mailer.app_sign_in_mails();
    assert!(mails[1].text.contains("to create your Buwiz account"), "{}", mails[1].text);
    assert!(mails[0].text.contains("to sign in"), "{}", mails[0].text);
}

#[sqlx::test(migrations = "./migrations")]
async fn only_the_right_secret_and_code_before_the_expiry_sign_in(db: PgPool) {
    let site = Site::with_mailbox(db);
    let started = start(&site, ANA).await;
    let code = last_code(&site, ANA);
    let wrong_code = if code == "000000" { "111111" } else { "000000" };

    // Another secret, or an unknown request.
    let other = Started { id: started.id.clone(), secret: secret().0 };
    assert_eq!(verify(&site, &other, &code).await, (400, refused("invalid_request")));
    let unknown = Started { id: Uuid::new_v4().to_string(), secret: started.secret.clone() };
    assert_eq!(verify(&site, &unknown, &code).await, (400, refused("invalid_request")));

    // Wrong codes count down, then the code is dead.
    for left in (1..=4).rev() {
        let (status, answer) = verify(&site, &started, wrong_code).await;
        assert_eq!(
            (status, answer["error"].as_str(), answer["attempts_left"].as_i64()),
            (400, Some("invalid_code"), Some(left))
        );
    }
    assert_eq!(verify(&site, &started, wrong_code).await, (400, refused("too_many_attempts")));
    assert_eq!(verify(&site, &started, &code).await, (400, refused("too_many_attempts")));

    // Expired.
    let later = start(&site, "bea@example.test").await;
    sqlx::query("UPDATE app_sign_ins SET expires_at = now() - interval '1 second' WHERE id = $1::uuid")
        .bind(&later.id)
        .execute(&site.db)
        .await
        .unwrap();
    assert_eq!(verify(&site, &later, &last_code(&site, "bea@example.test")).await, (400, refused("code_expired")));
    assert_eq!(count(&site.db, "SELECT count(*) FROM oauth_grants").await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_resend_waits_for_the_code_to_expire_and_ends_it(db: PgPool) {
    let site = Site::with_mailbox(db);
    let started = start(&site, ANA).await;
    let first = last_code(&site, ANA);
    let (status, answer) = resend(&site, &started).await;
    assert_eq!((status, answer["error"].as_str()), (429, Some("too_many_requests")));
    assert!((295..=300).contains(&answer["retry_after"].as_i64().unwrap()), "{answer}");
    assert_eq!(site.state.mailer.app_sign_in_mails().len(), 1);

    // The wrong secret cannot resend.
    expire(&site, &started).await;
    let other = Started { id: started.id.clone(), secret: secret().0 };
    assert_eq!(resend(&site, &other).await, (400, refused("invalid_request")));

    let (status, answer) = resend(&site, &started).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["request_id"], started.id.as_str());
    assert_eq!((answer["expires_in"].as_i64(), answer["resend_after"].as_i64()), (Some(300), Some(300)));
    let second = last_code(&site, ANA);
    // The five minutes started again.
    let left = count(
        &site.db,
        &format!("SELECT extract(epoch FROM expires_at - now())::bigint FROM app_sign_ins WHERE id = '{}'", started.id),
    )
    .await;
    assert!((290..=300).contains(&left), "{left}");
    if first != second {
        let (status, answer) = verify(&site, &started, &first).await;
        assert_eq!((status, answer["error"].as_str()), (400, Some("invalid_code")));
    }
    assert_eq!(verify(&site, &started, &second).await.0, 200);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_request_is_resent_for_an_hour_only(db: PgPool) {
    let site = Site::with_mailbox(db);
    let started = start(&site, ANA).await;
    expire(&site, &started).await;
    sqlx::query("UPDATE app_sign_ins SET created_at = now() - interval '61 minutes' WHERE id = $1::uuid")
        .bind(&started.id)
        .execute(&site.db)
        .await
        .unwrap();
    assert_eq!(resend(&site, &started).await, (400, refused("invalid_request")));
}

/// A start for `email` from the install `install`: its status, answer and
/// the request.
async fn start_from(site: &Site, email: &str, install: &str) -> (u16, Value, Started) {
    let (status, answer, secret) = start_with(site, email, json!({ "install_id": install })).await;
    let id = answer["request_id"].as_str().unwrap_or_default().to_owned();
    (status, answer, Started { id, secret })
}

const PHONE: &str = "install-phone-0001";
const LAPTOP: &str = "install-laptop-0002";

#[sqlx::test(migrations = "./migrations")]
async fn one_install_gets_one_working_code_for_an_address_at_a_time(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (status, _, first) = start_from(&site, ANA, PHONE).await;
    assert_eq!(status, 200);
    let (status, answer, _) = start_from(&site, ANA, PHONE).await;
    assert_eq!((status, answer["error"].as_str()), (429, Some("code_already_sent")), "{answer}");
    let wait = answer["retry_after"].as_i64().unwrap();
    assert!((295..=300).contains(&wait), "{wait}");
    assert_eq!(
        answer["message"],
        "A sign-in code was already sent to this address. Use it, or try again in 5 minutes."
    );
    assert_eq!(site.state.mailer.app_sign_in_mails().len(), 1, "no second mail");
    assert_eq!(count(&site.db, "SELECT count(*) FROM app_sign_ins").await, 1, "no second request");

    // Another address from the same install is a new mail.
    let (status, _, _) = start_from(&site, "bea@example.test", PHONE).await;
    assert_eq!(status, 200);

    // Once the code expired, a start works again.
    expire(&site, &first).await;
    let (status, _, again) = start_from(&site, ANA, PHONE).await;
    assert_eq!(status, 200);
    assert_eq!(verify(&site, &again, &last_code(&site, ANA)).await.0, 200);
}

#[sqlx::test(migrations = "./migrations")]
async fn another_install_gets_its_own_code_and_both_work(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (_, _, phone) = start_from(&site, ANA, PHONE).await;
    let phone_code = last_code(&site, ANA);
    let (status, _, laptop) = start_from(&site, ANA, LAPTOP).await;
    assert_eq!(status, 200);
    let laptop_code = last_code(&site, ANA);
    assert_eq!(site.state.mailer.app_sign_in_mails().len(), 2);
    assert_eq!(verify(&site, &laptop, &laptop_code).await.0, 200);
    assert_eq!(verify(&site, &phone, &phone_code).await.0, 200);
}

#[sqlx::test(migrations = "./migrations")]
async fn without_an_install_id_only_the_address_limit_holds(db: PgPool) {
    let site = Site::with_mailbox(db);
    for n in 0..5 {
        let (status, answer, _) = start_with(&site, ANA, json!({})).await;
        assert_eq!(status, 200, "{n}: {answer}");
    }
    let (status, answer, _) = start_with(&site, ANA, json!({})).await;
    assert_eq!((status, answer["error"].as_str()), (429, Some("too_many_requests")));
    // With installs, the address's five an hour still stop a sixth.
    let site_installs = &site;
    for n in 0..5 {
        let (status, _, _) = start_from(site_installs, "bea@example.test", &format!("install-number-{n:04}")).await;
        assert_eq!(status, 200);
    }
    let (status, answer, _) = start_from(site_installs, "bea@example.test", "install-number-0099").await;
    assert_eq!((status, answer["error"].as_str()), (429, Some("too_many_requests")));
    assert_eq!(count(&site.db, "SELECT count(*) FROM app_sign_ins WHERE used_at IS NULL").await, 10);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_code_works_only_with_its_own_request(db: PgPool) {
    let site = Site::with_mailbox(db);
    let first = start(&site, ANA).await;
    let first_code = last_code(&site, ANA);
    let second = start(&site, ANA).await;
    let second_code = last_code(&site, ANA);
    // The first request's digits on the second: wrong, counted there.
    if first_code != second_code {
        let (status, answer) = verify(&site, &second, &first_code).await;
        assert_eq!(
            (status, answer["error"].as_str(), answer["attempts_left"].as_i64()),
            (400, Some("invalid_code"), Some(4))
        );
    }
    // A web code in the app.
    let sent = site.post("/auth/email/start", "", "email=ana%40example.test").await;
    assert_eq!(sent.status(), StatusCode::SEE_OTHER);
    let (_, web_code) = site.state.mailer.sign_in_codes().pop().unwrap();
    if web_code != first_code {
        let (status, answer) = verify(&site, &first, &web_code).await;
        assert_eq!((status, answer["error"].as_str()), (400, Some("invalid_code")));
    }
    // An app code on the web: refused as any wrong code.
    if web_code != second_code {
        let refused =
            site.post("/auth/email/verify", "", &format!("email=ana%40example.test&code={second_code}")).await;
        assert!(location(&refused).contains("notice=code_invalid"), "{}", location(&refused));
    }
    assert_eq!(verify(&site, &first, &first_code).await.0, 200);
    assert_eq!(verify(&site, &second, &second_code).await.0, 200);
}

#[sqlx::test(migrations = "./migrations")]
async fn bad_install_ids_are_refused(db: PgPool) {
    let site = Site::with_mailbox(db);
    for bad in ["short", "has spaces in it ok", "x!xxxxxxxxxxxxxxxxx"] {
        let (status, answer, _) = start_with(&site, ANA, json!({ "install_id": bad })).await;
        assert_eq!((status, answer["error"].as_str()), (400, Some("bad_install_id")), "{bad}");
    }
    assert!(site.state.mailer.app_sign_in_mails().is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn app_sign_in_minutes_sets_both_waits_and_the_mails_words(db: PgPool) {
    let site = Site::with_config(db, |config| config.app_sign_in_minutes = 2);
    let (status, answer, _) = start_with(&site, ANA, json!({})).await;
    assert_eq!(status, 200);
    assert_eq!((answer["expires_in"].as_i64(), answer["resend_after"].as_i64()), (Some(120), Some(120)));
    let mail = site.state.mailer.app_sign_in_mails().pop().unwrap();
    assert!(mail.text.contains("It expires in two minutes"), "{}", mail.text);
    // The web's code still lasts ten minutes.
    site.post("/auth/email/start", "", "email=bea%40example.test").await;
    let left = count(
        &site.db,
        "SELECT extract(epoch FROM expires_at - now())::bigint FROM email_codes WHERE email = 'bea@example.test'",
    )
    .await;
    assert!(left > 590, "{left}");
}

#[sqlx::test(migrations = "./migrations")]
async fn a_test_account_is_mailed_nothing_and_its_fixed_code_works(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (boss, _) = site.staff_with_role("boss@example.test", "admin").await;
    let (reviewer, _) = site.person("reviewer@example.test").await;
    sqlx::query(
        "UPDATE accounts SET test_account = true, fixed_code_hash = $2, fixed_code_set_by = $3, fixed_code_set_at = now()
         WHERE id = $1",
    )
    .bind(reviewer)
    .bind(crypto::hash("reviewer@example.test:481516"))
    .bind(boss)
    .execute(&site.db)
    .await
    .unwrap();
    let (_, _, earlier) = start_from(&site, "reviewer@example.test", PHONE).await;
    // Never held: a new request each time, the earlier one ended.
    let (status, _, started) = start_from(&site, "reviewer@example.test", PHONE).await;
    assert_eq!(status, 200);
    assert!(site.state.mailer.app_sign_in_mails().is_empty());
    assert_eq!(verify(&site, &earlier, "481516").await, (400, refused("invalid_request")));
    assert_eq!(verify(&site, &started, "481516").await.0, 200);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_banned_account_gets_no_tokens(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, _) = site.person(ANA).await;
    sqlx::query("UPDATE accounts SET banned_at = now(), banned_by = id, ban_reason = 'Test' WHERE id = $1")
        .bind(ana)
        .execute(&site.db)
        .await
        .unwrap();
    let started = start(&site, ANA).await;
    let (status, answer) = verify(&site, &started, &last_code(&site, ANA)).await;
    assert_eq!(status, 403);
    assert_eq!(answer["error"], "account_banned");
    assert_eq!(answer["message"], buwiz_cloud::oauth::BANNED_MESSAGE);
    assert_eq!(count(&site.db, "SELECT count(*) FROM oauth_grants").await, 0);
    assert_eq!(count(&site.db, "SELECT count(*) FROM app_sign_ins WHERE used_at IS NULL").await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_web_and_the_app_share_an_addresss_five_codes_an_hour(db: PgPool) {
    let site = Site::with_mailbox(db);
    for _ in 0..3 {
        let sent = site.post("/auth/email/start", "", "email=ana%40example.test").await;
        assert!(location(&sent).starts_with("/login/verify"), "{}", location(&sent));
    }
    start(&site, ANA).await;
    start(&site, ANA).await;
    let (status, answer, _) = start_with(&site, ANA, json!({})).await;
    assert_eq!((status, answer["error"].as_str()), (429, Some("too_many_requests")));
    assert!(answer["retry_after"].as_i64().unwrap() > 0);
    let refused = site.post("/auth/email/start", "", "email=ana%40example.test").await;
    assert!(location(&refused).ends_with("notice=code_too_many"), "{}", location(&refused));
}

#[sqlx::test(migrations = "./migrations")]
async fn only_bir_apps_and_addresses_are_taken(db: PgPool) {
    let site = Site::with_mailbox(db);
    for client in ["buwiz-books", "an-agent"] {
        let (status, answer, _) = start_with(&site, ANA, json!({ "client_id": client })).await;
        assert_eq!((status, answer["error"].as_str()), (400, Some("unknown_client")), "{client}");
    }
    let (status, answer, _) = start_with(&site, "ana", json!({})).await;
    assert_eq!((status, answer["error"].as_str()), (400, Some("invalid_email")));
    let (status, answer, _) = start_with(&site, ANA, json!({ "secret_hash": "short" })).await;
    assert_eq!((status, answer["error"].as_str()), (400, Some("invalid_request")));
    // An odd intent is not an error.
    let (status, _, _) = start_with(&site, ANA, json!({ "intent": "whatever", "attestation": { "a": 1 } })).await;
    assert_eq!(status, 200);
    assert_eq!(count(&site.db, "SELECT count(*) FROM app_sign_ins").await, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_mail_names_the_device_in_fixed_words_only(db: PgPool) {
    let site = Site::with_mailbox(db);
    let platforms = [
        (json!("ios"), "eBIRForms on iPhone"),
        (json!("ipados"), "eBIRForms on iPad"),
        (json!("android"), "eBIRForms on Android"),
        (json!("macos"), "eBIRForms on Mac"),
        (json!("windows"), "eBIRForms on Windows"),
        (json!("linux"), "eBIRForms on Linux"),
        (json!("<a href=x>Evil phone</a>"), "eBIRForms"),
        (json!(42), "eBIRForms"),
        (Value::Null, "eBIRForms"),
    ];
    for (n, (platform, device)) in platforms.iter().enumerate() {
        let email = format!("person{n}@example.test");
        let extra = if platform.is_null() { json!({}) } else { json!({ "platform": platform }) };
        let (status, _, _) = start_with(&site, &email, extra).await;
        assert_eq!(status, 200);
        let mail = site.state.mailer.app_sign_in_mails().pop().unwrap();
        assert_eq!(mail.subject, format!("Your Buwiz sign-in code for {device}"));
        assert!(
            mail.text.contains(&format!(
                "Someone asked to sign in to {device} with this address. If it wasn't you, ignore this mail."
            )),
            "{}",
            mail.text
        );
        assert!(mail.text.contains("Never give this code to anyone"));
        assert!(mail.text.contains(&mail.code) && mail.html.contains(&mail.code));
        for part in [&mail.subject, &mail.text, &mail.html] {
            assert!(!part.contains("Evil") && !part.contains("href=x"), "{part}");
        }
    }
    let stored: Vec<Option<String>> =
        sqlx::query_scalar("SELECT platform FROM app_sign_ins ORDER BY created_at, email")
            .fetch_all(&site.db)
            .await
            .unwrap();
    assert_eq!(stored.iter().filter(|platform| platform.is_none()).count(), 3);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_resend_keeps_the_platform(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (_, answer, secret) = start_with(&site, ANA, json!({ "platform": "android", "intent": "create" })).await;
    let started = Started { id: answer["request_id"].as_str().unwrap().to_owned(), secret };
    expire(&site, &started).await;
    assert_eq!(resend(&site, &started).await.0, 200);
    let mails = site.state.mailer.app_sign_in_mails();
    assert_eq!(mails.len(), 2);
    assert_eq!(mails[1].subject, "Your Buwiz sign-in code for eBIRForms on Android");
    assert!(mails[1].text.contains("to create your Buwiz account"));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_code_that_could_not_be_mailed_leaves_nothing_usable(db: PgPool) {
    let site = Site::with_mailbox(db);
    site.state.mailer.fail_app_codes(true);
    let (status, answer, _) = start_with(&site, ANA, json!({})).await;
    assert_eq!((status, answer["error"].as_str()), (502, Some("mail_failed")));
    assert_eq!(count(&site.db, "SELECT count(*) FROM app_sign_ins WHERE used_at IS NULL").await, 0);

    site.state.mailer.fail_app_codes(false);
    let started = start(&site, ANA).await;
    expire(&site, &started).await;
    site.state.mailer.fail_app_codes(true);
    assert_eq!(resend(&site, &started).await.0, 502);
    site.state.mailer.fail_app_codes(false);
    assert_eq!(verify(&site, &started, &last_code(&site, ANA)).await, (400, refused("invalid_request")));
}

#[sqlx::test(migrations = "./migrations")]
async fn while_stopping_nothing_starts(db: PgPool) {
    let site = Site::with_mailbox(db);
    let started = start(&site, ANA).await;
    site.state.stopping.begin();
    for (path, body) in [
        ("/auth/email/start", json!({ "client_id": CLIENT, "email": ANA, "secret_hash": secret().1 })),
        ("/auth/email/resend", json!({ "request_id": started.id, "secret": started.secret })),
        ("/auth/email/verify", json!({ "request_id": started.id, "secret": started.secret, "code": "123456" })),
    ] {
        let response = post_json(&site, path, &body).await;
        assert_eq!(response.headers()[header::RETRY_AFTER], "30");
        let (status, answer) = read(response).await;
        assert_eq!((status, answer["error"].as_str()), (503, Some("restarting")), "{path}");
    }
}

/// Codes given without an app check this long ago, to an address with an
/// account (so they spend the testing cap and nothing else).
async fn unchecked(db: &PgPool, n: i64, ago: &str) {
    sqlx::query("INSERT INTO accounts (email) VALUES ('filler@example.test') ON CONFLICT DO NOTHING")
        .execute(db)
        .await
        .unwrap();
    sqlx::query(&format!(
        "INSERT INTO email_codes (email, code_hash, expires_at, created_at, purpose, unchecked)
         SELECT 'filler@example.test', decode('00', 'hex'), now(), now() - interval '{ago}', 'app_sign_in', true
         FROM generate_series(1, $1)"
    ))
    .bind(n)
    .execute(db)
    .await
    .unwrap();
}

async fn unchecked_codes(db: &PgPool) -> i64 {
    count(db, "SELECT count(*) FROM email_codes WHERE unchecked").await
}

#[sqlx::test(migrations = "./migrations")]
async fn requests_without_an_app_check_are_capped_at_twenty_an_hour(db: PgPool) {
    let site = Site::with_mailbox(db);
    for n in 0..21 {
        site.person(&format!("person{n}@example.test")).await;
    }
    for n in 0..20 {
        let (status, answer, _) = start_with(&site, &format!("person{n}@example.test"), json!({})).await;
        assert_eq!(status, 200, "{n}: {answer}");
    }
    let (status, answer, _) = start_with(&site, "person20@example.test", json!({})).await;
    assert_eq!((status, answer["error"].as_str()), (429, Some("too_many_requests")));
    let wait = answer["retry_after"].as_i64().unwrap();
    assert!((3500..=3600).contains(&wait), "{wait}");
    // Kept in the database, and the refused one wrote nothing.
    assert_eq!(unchecked_codes(&site.db).await, 20);
    assert_eq!(count(&site.db, "SELECT count(*) FROM app_sign_ins").await, 20);
}

#[sqlx::test(migrations = "./migrations")]
async fn refused_requests_leave_the_cap_and_write_nothing(db: PgPool) {
    let site = Site::with_mailbox(db);
    site.person(ANA).await;
    // Twenty starts for one address: five codes, fifteen refusals.
    for n in 0..20 {
        let (status, answer, _) = start_with(&site, ANA, json!({})).await;
        let expected = if n < 5 { 200 } else { 429 };
        assert_eq!(status, expected, "{n}: {answer}");
    }
    assert_eq!(unchecked_codes(&site.db).await, 5, "the refusals took no place in the cap");
    assert_eq!(count(&site.db, "SELECT count(*) FROM app_sign_ins").await, 5, "and wrote no request");
    // Another address still starts.
    assert_eq!(start_with(&site, "bea@example.test", json!({})).await.0, 200);

    // A network over its hour of tries: refused before anything is
    // written.
    let rows = count(&site.db, "SELECT count(*) FROM app_sign_ins").await;
    for n in 0..20 {
        let (status, answer) = start_from_network(&site, &format!("net{n}@example.test"), n, json!({})).await;
        assert_eq!(status, 200, "{n}: {answer}");
    }
    let (status, _) = start_from_network(&site, "net77@example.test", 77, json!({})).await;
    assert_eq!(status, 429);
    assert_eq!(count(&site.db, "SELECT count(*) FROM app_sign_ins").await, rows + 20);
}

/// A start for `email` from `203.0.113.<n>`, with `extra` in the body.
async fn start_from_network(site: &Site, email: &str, n: u32, extra: Value) -> (u16, Value) {
    let (_, hash) = secret();
    let mut body = json!({ "client_id": CLIENT, "email": email, "secret_hash": hash });
    for (key, value) in extra.as_object().unwrap() {
        body[key] = value.clone();
    }
    let request = Request::post("/auth/email/start")
        .header(header::HOST, API_HOST)
        .header(header::CONTENT_TYPE, "application/json")
        .header("cf-connecting-ip", format!("203.0.113.{n}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    read(site.send(request).await).await
}

#[sqlx::test(migrations = "./migrations")]
async fn token_less_mail_to_new_addresses_has_its_own_budget(db: PgPool) {
    let site = Site::with_turnstile_mailbox(db, &turnstile().await);
    for n in 0..10 {
        let (status, answer, _) = start_with(&site, &format!("new{n}@example.test"), json!({})).await;
        assert_eq!(status, 200, "{n}: {answer}");
    }
    assert_eq!(site.state.mailer.app_sign_in_mails().len(), 10);
    // Over it: answered the same, nothing mailed.
    let (status, answer, _) = start_with(&site, "new10@example.test", json!({})).await;
    assert_eq!(status, 200);
    assert!(answer["request_id"].is_string());
    assert_eq!(site.state.mailer.app_sign_in_mails().len(), 10);
    // The web's own budget is untouched: the sign-in form mails a new
    // address, and so does the app with a good token.
    let form = format!("email=web%40example.test&cf-turnstile-response={WEB_TOKEN}");
    let sent = site.post("/auth/email/start", "", &form).await;
    assert!(location(&sent).starts_with("/login/verify"), "{}", location(&sent));
    assert_eq!(site.state.mailer.sign_in_codes().len(), 1);
    let (status, _, _) = start_with(&site, "checked@example.test", json!({ "turnstile": GOOD_TOKEN })).await;
    assert_eq!(status, 200);
    assert_eq!(site.state.mailer.app_sign_in_mails().len(), 11);
    let checked: bool = sqlx::query_scalar("SELECT unchecked FROM email_codes WHERE email = 'checked@example.test'")
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert!(!checked, "a checked request counts as the web's");
}

#[sqlx::test(migrations = "./migrations")]
async fn many_starts_at_once_for_one_address_mail_five(db: PgPool) {
    let site = Site::with_mailbox(db);
    site.person(ANA).await;
    let mut tasks = Vec::new();
    for _ in 0..12 {
        let router = site.router.clone();
        let (_, hash) = secret();
        let body = json!({ "client_id": CLIENT, "email": ANA, "secret_hash": hash });
        let request = Request::post("/auth/email/start")
            .header(header::HOST, API_HOST)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        tasks.push(tokio::spawn(async move { tower::ServiceExt::oneshot(router, request).await.unwrap().status() }));
    }
    let mut ok = 0;
    for task in tasks {
        if task.await.unwrap() == StatusCode::OK {
            ok += 1;
        }
    }
    assert_eq!(ok, 5);
    assert_eq!(site.state.mailer.app_sign_in_mails().len(), 5);
    assert_eq!(count(&site.db, "SELECT count(*) FROM email_codes WHERE email = 'ana@example.test'").await, 5);
}

#[sqlx::test(migrations = "./migrations")]
async fn an_install_is_held_only_while_its_code_can_be_used(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (_, _, first) = start_from(&site, ANA, PHONE).await;
    let wrong = if last_code(&site, ANA) == "000000" { "111111" } else { "000000" };
    for _ in 0..5 {
        verify(&site, &first, wrong).await;
    }
    // Its five tries are used: a new start from the install goes through.
    let (status, answer, _) = start_from(&site, ANA, PHONE).await;
    assert_eq!(status, 200, "{answer}");

    // No code was mailed (the new-address budget is used up): held all the
    // same, as for an address whose code was mailed.
    for n in 0..10 {
        sqlx::query(
            "INSERT INTO email_codes (email, code_hash, expires_at, purpose, unchecked)
             VALUES ($1, decode('00', 'hex'), now() + interval '5 minutes', 'app_sign_in', true)",
        )
        .bind(format!("used{n}@example.test"))
        .execute(&site.db)
        .await
        .unwrap();
    }
    let mails = site.state.mailer.app_sign_in_mails().len();
    assert_eq!(start_from(&site, "bea@example.test", PHONE).await.0, 200);
    let (status, answer, _) = start_from(&site, "bea@example.test", PHONE).await;
    assert_eq!((status, answer["error"].as_str()), (429, Some("code_already_sent")), "{answer}");
    assert_eq!(site.state.mailer.app_sign_in_mails().len(), mails);
}

#[sqlx::test(migrations = "./migrations")]
async fn and_fifty_a_day_with_one_alert_to_staff_at_twenty_five(db: PgPool) {
    let site = Site::with_mailbox(db);
    unchecked(&site.db, 23, "2 hours").await;
    start(&site, "a@example.test").await;
    assert!(site.state.mailer.notices().is_empty());
    start(&site, "b@example.test").await;
    let notices = site.state.mailer.notices();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].to, STAFF);
    assert!(notices[0].text.contains("25 in-app sign-in requests"), "{}", notices[0].text);
    start(&site, "c@example.test").await;
    assert_eq!(site.state.mailer.notices().len(), 1, "once a day");

    unchecked(&site.db, 24, "3 hours").await;
    let (status, answer, _) = start_with(&site, "d@example.test", json!({})).await;
    assert_eq!((status, answer["error"].as_str()), (429, Some("too_many_requests")));
    let wait = answer["retry_after"].as_i64().unwrap();
    assert!(wait > 20 * 3600, "{wait}");
}

#[sqlx::test(migrations = "./migrations")]
async fn a_good_token_skips_the_cap_and_a_bad_one_is_refused(db: PgPool) {
    let site = Site::with_turnstile_mailbox(db, &turnstile().await);
    unchecked(&site.db, 20, "1 minute").await;
    let (status, answer, _) = start_with(&site, ANA, json!({})).await;
    assert_eq!((status, answer["error"].as_str()), (429, Some("too_many_requests")));
    let (status, answer, _) = start_with(&site, ANA, json!({ "turnstile": "a-script" })).await;
    assert_eq!((status, answer["error"].as_str()), (403, Some("turnstile_failed")));
    // A token from the sign-in form's widget, or from another host, is not
    // an app check.
    for token in [WEB_TOKEN, OTHER_HOST_TOKEN] {
        let (status, answer, _) = start_with(&site, ANA, json!({ "turnstile": token })).await;
        assert_eq!((status, answer["error"].as_str()), (403, Some("turnstile_failed")), "{token}");
    }
    let (status, answer, secret) = start_with(&site, ANA, json!({ "turnstile": GOOD_TOKEN })).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(unchecked_codes(&site.db).await, 20);

    // Resend too.
    let started = Started { id: answer["request_id"].as_str().unwrap().to_owned(), secret };
    expire(&site, &started).await;
    let body = json!({ "request_id": started.id, "secret": started.secret, "turnstile": "a-script" });
    let (status, answer) = read(post_json(&site, "/auth/email/resend", &body).await).await;
    assert_eq!((status, answer["error"].as_str()), (403, Some("turnstile_failed")));
    let (status, answer) = resend(&site, &started).await;
    assert_eq!((status, answer["error"].as_str()), (429, Some("too_many_requests")));
    expire(&site, &started).await;
    let body = json!({ "request_id": started.id, "secret": started.secret, "turnstile": GOOD_TOKEN });
    let (status, _) = read(post_json(&site, "/auth/email/resend", &body).await).await;
    assert_eq!(status, 200);
    assert_eq!(verify(&site, &started, &last_code(&site, ANA)).await.0, 200);
}

#[sqlx::test(migrations = "./migrations")]
async fn without_turnstile_keys_a_token_is_ignored_and_the_cap_holds(db: PgPool) {
    let site = Site::with_mailbox(db);
    unchecked(&site.db, 20, "1 minute").await;
    let (status, answer, _) = start_with(&site, ANA, json!({ "turnstile": GOOD_TOKEN })).await;
    assert_eq!((status, answer["error"].as_str()), (429, Some("too_many_requests")));
}

#[sqlx::test(migrations = "./migrations")]
async fn the_web_form_still_signs_in_on_the_same_paths(db: PgPool) {
    let site = Site::with_mailbox(db);
    let sent = site.post("/auth/email/start", "", "email=ana%40example.test").await;
    assert_eq!(sent.status(), StatusCode::SEE_OTHER);
    let (_, code) = site.state.mailer.sign_in_codes().pop().unwrap();
    assert!(site.state.mailer.app_sign_in_mails().is_empty());
    let signed_in = site.post("/auth/email/verify", "", &format!("email=ana%40example.test&code={code}")).await;
    assert_eq!(signed_in.status(), StatusCode::SEE_OTHER);
    assert!(signed_in.headers().contains_key(header::SET_COOKIE));
}

/// The request id and link token of the newest link mailed to `email`.
fn last_link(site: &Site, email: &str) -> (String, String) {
    let mails = site.state.mailer.app_sign_in_mails();
    let mail = mails.iter().rev().find(|mail| mail.to == email).unwrap();
    let link = mail.link.clone().unwrap();
    assert!(mail.text.contains(&link), "{}", mail.text);
    let fragment = link.strip_prefix(&format!("{ORIGIN}/app-sign-in#")).unwrap_or_else(|| panic!("{link}"));
    assert!(!link.contains('?'), "the values are in the fragment only: {link}");
    let (r, t) = fragment.split_once('&').unwrap();
    (r.strip_prefix("r=").unwrap().to_owned(), t.strip_prefix("t=").unwrap().to_owned())
}

async fn verify_link(site: &Site, started: &Started, token: &str) -> (u16, Value) {
    let body = json!({ "request_id": started.id, "secret": started.secret, "link_token": token });
    read(post_json(site, "/auth/email/verify", &body).await).await
}

#[sqlx::test(migrations = "./migrations")]
async fn the_mails_link_signs_in_only_the_device_that_asked_and_once(db: PgPool) {
    let site = Site::with_mailbox(db);
    let started = start(&site, ANA).await;
    let (request, token) = last_link(&site, ANA);
    assert_eq!(request, started.id);
    let mail = site.state.mailer.app_sign_in_mails().pop().unwrap();
    assert!(mail.text.contains(&mail.code) && mail.html.contains(&mail.code));
    assert!(mail.html.contains("Open in eBIRForms"));

    // Someone else's app, holding its own secret, gets nothing from it.
    let other = Started { id: started.id.clone(), secret: secret().0 };
    assert_eq!(verify_link(&site, &other, &token).await, (400, refused("invalid_request")));
    let (status, answer) = verify_link(&site, &started, "a-made-up-token").await;
    assert_eq!((status, answer["error"].as_str()), (400, Some("link_used")));
    // Neither code nor token, or both.
    let both = json!({ "request_id": started.id, "secret": started.secret, "code": mail.code, "link_token": token });
    assert_eq!(read(post_json(&site, "/auth/email/verify", &both).await).await.0, 400);

    let (status, tokens) = verify_link(&site, &started, &token).await;
    assert_eq!((status, tokens["scope"].as_str()), (200, Some("app")), "{tokens}");
    assert_eq!(verify_link(&site, &started, &token).await, (400, refused("invalid_request")));
    // The code went with it.
    let code = json!({ "request_id": started.id, "secret": started.secret, "code": mail.code });
    assert_eq!(read(post_json(&site, "/auth/email/verify", &code).await).await, (400, refused("invalid_request")));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_new_code_ends_the_old_link(db: PgPool) {
    let site = Site::with_mailbox(db);
    let started = start(&site, ANA).await;
    let (_, first) = last_link(&site, ANA);
    expire(&site, &started).await;
    assert_eq!(resend(&site, &started).await.0, 200);
    let (_, second) = last_link(&site, ANA);
    assert_ne!(first, second);
    let (status, answer) = verify_link(&site, &started, &first).await;
    assert_eq!((status, answer["error"].as_str()), (400, Some("link_used")));

    // A web code leaves it alone: the link is the request's own.
    let sent = site.post("/auth/email/start", "", "email=ana%40example.test").await;
    assert_eq!(sent.status(), StatusCode::SEE_OTHER);
    assert_eq!(verify_link(&site, &started, &second).await.0, 200);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_link_page_only_offers_a_button(db: PgPool) {
    let site = Site::new(db);
    let response = site.get("/app-sign-in", "").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-robots-tag"], "noindex");
    // Its own script and the site's files only.
    let csp = response.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().to_owned();
    assert!(csp.contains("default-src 'none'") && csp.contains("script-src 'self'"), "{csp}");
    let page = text(response).await;
    assert!(page.contains(r#"<meta name="robots" content="noindex">"#));
    assert!(page.contains("Open this on the device where you asked to sign in, or type the code there."));
    assert!(page.contains(r#"id="open-app""#) && page.contains("Open eBIRForms"), "{page}");
    // Never a dead end: the page always says the code can be typed instead.
    assert!(page.contains("Or type the 6-digit code from this email into the app."), "{page}");
    assert!(page.contains("hidden"), "the button is hidden until the script finds a fragment");
    assert!(page.contains(r#"src="/assets/app-sign-in.js""#));
    assert!(!page.contains("http-equiv=\"refresh\""));

    let script = text(site.get("/assets/app-sign-in.js", "").await).await;
    assert!(script.contains("ebirforms://signin?r=") && script.contains("&t="), "{script}");
    assert!(script.contains("window.location.hash"));
    for redirect in [
        "location.href =",
        "location.replace",
        "location.assign",
        "window.open",
        "fetch(",
        "XMLHttpRequest",
        ".click()",
    ] {
        assert!(!script.contains(redirect), "{redirect}");
    }
    // Only on the account host.
    assert_eq!(site.on_host(FORMS_HOST, "/app-sign-in").await.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_app_check_page_runs_the_widget_and_hands_the_token_to_the_app(db: PgPool) {
    let site = Site::with_turnstile(db, "http://127.0.0.1:9");
    let response = site.get("/app-check", "").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-robots-tag"], "noindex");
    let page = text(response).await;
    assert!(page.contains(r#"<meta name="robots" content="noindex">"#));
    assert!(page.contains(&format!(r#"data-sitekey="{TURNSTILE_SITE_KEY}""#)), "{page}");
    assert!(page.contains(r#"data-callback="buwizTurnstileDone""#));
    assert!(page.contains(r#"data-error-callback="buwizTurnstileError""#));
    assert!(page.contains(r#"data-expired-callback="buwizTurnstileExpired""#));
    assert!(page.contains("challenges.cloudflare.com/turnstile/v0/api.js"));
    // Its functions are there before the widget calls them.
    assert!(page.find("/assets/app-check.js").unwrap() < page.find("challenges.cloudflare.com").unwrap());
    // No header: nothing on it leads off the account host.
    assert!(!page.contains("top-nav") && !page.contains(r#"href="/""#), "{page}");

    let done = site.get("/app-check/done", "").await;
    assert_eq!(done.status(), StatusCode::OK);
    assert!(done.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().contains("script-src 'self'"));
    let done = text(done).await;
    assert!(done.contains("You can close this window"));
    assert!(!done.contains("<script src=\"/assets/app-check.js\""));

    let script = text(site.get("/assets/app-check.js", "").await).await;
    let nsh = script.find("window.nsh.postMessage(JSON.stringify(message))").expect("the app's bridge");
    let done = script.find("window.location.assign(\"/app-check/done#\" + fragment)").expect("the /done page");
    assert!(nsh < done, "the bridge first, then /done");
    for part in [
        "{ turnstile: token }",
        "\"token=\" + encodeURIComponent(token)",
        "{ turnstile_error: code }",
        "\"error=\" + encodeURIComponent(code)",
        "fail(\"expired\")",
        "fail(\"not_configured\")",
    ] {
        assert!(script.contains(part), "{part}");
    }
    // Only the bridge: no other window is told, and the title stays.
    for gone in ["\"*\"", "'*'", "window.parent", "window.opener", "document.title", ".postMessage(typed"] {
        assert!(!script.contains(gone), "{gone}");
    }
    assert_eq!(script.matches("postMessage(").count(), 1);
    // No other window keeps a handle on either page.
    for path in ["/app-check", "/app-check/done"] {
        let response = site.get(path, "").await;
        assert_eq!(response.headers()["cross-origin-opener-policy"], "same-origin", "{path}");
    }
    // Only on the account host.
    assert_eq!(site.on_host(FORMS_HOST, "/app-check").await.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "./migrations")]
async fn without_turnstile_keys_the_app_check_page_says_so(db: PgPool) {
    let site = Site::new(db);
    let page = text(site.get("/app-check", "").await).await;
    assert!(page.contains("The app check is not set up on this server"), "{page}");
    assert!(page.contains(r#"data-state="not-configured""#));
    assert!(page.contains("/assets/app-check.js") && !page.contains("challenges.cloudflare.com"));
}

#[sqlx::test(migrations = "./migrations")]
async fn the_grant_keeps_the_install_and_platform_the_request_named(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (_, answer, secret) = start_with(&site, ANA, json!({ "install_id": PHONE, "platform": "android" })).await;
    let started = Started { id: answer["request_id"].as_str().unwrap().to_owned(), secret };
    assert_eq!(verify(&site, &started, &last_code(&site, ANA)).await.0, 200);
    let (install, platform): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT install_id, platform FROM oauth_grants").fetch_one(&site.db).await.unwrap();
    assert_eq!((install.as_deref(), platform.as_deref()), (Some(PHONE), Some("android")));
}

/// A code that is surely not the request's: one whose hash differs from
/// the stored one (an unmailed code's digits are known to nobody).
async fn wrong_code(site: &Site, started: &Started) -> String {
    let (email, hash): (String, Vec<u8>) = sqlx::query_as(
        "SELECT c.email, c.code_hash FROM app_sign_ins r JOIN email_codes c ON c.id = r.code_id WHERE r.id = $1::uuid",
    )
    .bind(&started.id)
    .fetch_one(&site.db)
    .await
    .unwrap();
    (0..).map(|n| format!("{n:06}")).find(|code| crypto::hash(&format!("{email}:{code}")) != hash).unwrap()
}

/// `answer` without what differs between two requests by nature: the
/// request id, and the seconds a code's wait has left (about the five
/// minutes; the clock may tick between the two). Any other wait stays.
fn without_ids(mut answer: Value) -> Value {
    let object = answer.as_object_mut().unwrap();
    object.remove("request_id");
    if object.get("retry_after").and_then(Value::as_i64).is_some_and(|wait| (295..=300).contains(&wait)) {
        object.remove("retry_after");
    }
    answer
}

/// An address with an account and one without, whose budget of mail is
/// full so its code is not mailed, are answered the same at every step:
/// the hold on the install, the wrong codes, a start again, the sixth start
/// in the hour.
#[sqlx::test(migrations = "./migrations")]
async fn a_start_that_could_not_mail_answers_and_behaves_as_one_that_did(db: PgPool) {
    let site = Site::with_mailbox(db);
    let known = "known@example.test";
    let unknown = "unknown@example.test";
    site.person(known).await;
    // The apps' budget for new addresses without an app check is full.
    for n in 0..10 {
        sqlx::query(
            "INSERT INTO email_codes (email, code_hash, expires_at, purpose, unchecked)
             VALUES ($1, decode('00', 'hex'), now() + interval '5 minutes', 'app_sign_in', true)",
        )
        .bind(format!("used{n}@example.test"))
        .execute(&site.db)
        .await
        .unwrap();
    }
    let both = [(known, "install-known-00001"), (unknown, "install-unknown-0001")];

    // The start, and a second one from the same install.
    let mut first = Vec::new();
    let mut answers = Vec::new();
    for (email, install) in both {
        let (status, answer, started) = start_from(&site, email, install).await;
        answers.push((status, without_ids(answer)));
        first.push(started);
    }
    assert_eq!(answers[0], answers[1]);
    assert_eq!(answers[0].0, 200);
    let mut answers = Vec::new();
    for (email, install) in both {
        let (status, answer, _) = start_from(&site, email, install).await;
        answers.push((status, without_ids(answer)));
    }
    assert_eq!(answers[0], answers[1]);
    assert_eq!((answers[0].0, answers[0].1["error"].as_str()), (429, Some("code_already_sent")));

    // Five wrong codes: the same tries left, then the same refusal.
    for try_ in 0..6i64 {
        let mut answers = Vec::new();
        for started in &first {
            answers.push(verify(&site, started, &wrong_code(&site, started).await).await);
        }
        assert_eq!(answers[0], answers[1], "try {try_}");
        if try_ < 4 {
            assert_eq!(answers[0].1["attempts_left"].as_i64(), Some(4 - try_), "try {try_}");
        } else {
            assert_eq!(answers[0], (400, refused("too_many_attempts")), "try {try_}");
        }
    }

    // Its tries spent, the install may start again; then the sixth start in
    // the hour (the hold aside: from other installs) is refused alike.
    for n in 2..=6 {
        let mut answers = Vec::new();
        for (email, install) in both {
            let install = if n == 2 { install.to_owned() } else { format!("{install}-{n}") };
            let (status, answer, _) = start_from(&site, email, &install).await;
            answers.push((status, without_ids(answer)));
        }
        assert_eq!(answers[0], answers[1], "start {n}");
        let expected = if n < 6 { 200 } else { 429 };
        assert_eq!(answers[0].0, expected, "start {n}: {:?}", answers[0]);
    }

    // The unknown address was mailed nothing; its five codes are real ones,
    // never mailed, and took no place in any budget of mail.
    let mails = site.state.mailer.app_sign_in_mails();
    assert_eq!(mails.iter().filter(|mail| mail.to == unknown).count(), 0);
    assert_eq!(mails.iter().filter(|mail| mail.to == known).count(), 5);
    let unmailed = count(&site.db, "SELECT count(*) FROM email_codes WHERE NOT mailed").await;
    assert_eq!(unmailed, 5);
    let held: Vec<(i32, bool)> =
        sqlx::query_as("SELECT length(code_hash), link_token_hash IS NOT NULL FROM app_sign_ins r JOIN email_codes c ON c.id = r.code_id WHERE r.email = $1")
            .bind(unknown)
            .fetch_all(&site.db)
            .await
            .unwrap();
    assert_eq!(held.len(), 5);
    assert!(held.iter().all(|row| *row == (32, true)), "{held:?}");
    // The testing cap counts the known address's five mails only.
    assert_eq!(
        count(&site.db, "SELECT count(*) FROM email_codes WHERE unchecked AND mailed AND email NOT LIKE 'used%'").await,
        5
    );
}

/// The 21st try at start or resend from one network in an hour is refused,
/// even when every try before it was refused, and no Turnstile call is
/// made for it; the web form's count per network is untouched.
#[sqlx::test(migrations = "./migrations")]
async fn a_network_gets_twenty_tries_an_hour_whatever_comes_of_them(db: PgPool) {
    let (url, calls) = turnstile_counted().await;
    let site = Site::with_turnstile_mailbox(db, &url);
    for n in 0..10 {
        let (status, answer) =
            start_from_network(&site, &format!("p{n}@example.test"), n, json!({ "turnstile": "a-script" })).await;
        assert_eq!((status, answer["error"].as_str()), (403, Some("turnstile_failed")), "{n}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 10);
    for n in 10..20 {
        let (status, answer) = start_from_network(&site, "not an address", n, json!({})).await;
        assert_eq!((status, answer["error"].as_str()), (400, Some("invalid_email")), "{n}");
    }
    let (status, answer) = start_from_network(&site, ANA, 21, json!({ "turnstile": "a-script" })).await;
    assert_eq!((status, answer["error"].as_str()), (429, Some("too_many_requests")), "{answer}");
    let wait = answer["retry_after"].as_i64().unwrap();
    assert!((3500..=3600).contains(&wait), "{wait}");
    assert_eq!(calls.load(Ordering::SeqCst), 10, "no Turnstile call past the budget");
    // A resend from the network counts the same.
    let resend = Request::post("/auth/email/resend")
        .header(header::HOST, API_HOST)
        .header(header::CONTENT_TYPE, "application/json")
        .header("cf-connecting-ip", "203.0.113.99")
        .body(Body::from(json!({ "request_id": Uuid::new_v4(), "secret": secret().0 }).to_string()))
        .unwrap();
    assert_eq!(site.send(resend).await.status(), StatusCode::TOO_MANY_REQUESTS);
    // Another network goes on.
    let (_, hash) = secret();
    let other = Request::post("/auth/email/start")
        .header(header::HOST, API_HOST)
        .header(header::CONTENT_TYPE, "application/json")
        .header("cf-connecting-ip", "198.51.100.7")
        .body(Body::from(json!({ "client_id": CLIENT, "email": ANA, "secret_hash": hash }).to_string()))
        .unwrap();
    assert_eq!(site.send(other).await.status(), StatusCode::OK);
    assert_eq!(count(&site.db, "SELECT count(*) FROM app_sign_ins").await, 1);

    // The web form from the same network still has all of its ten.
    for n in 0..10 {
        let request = Request::post("/auth/email/start")
            .header(header::ORIGIN, ORIGIN)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header("cf-connecting-ip", format!("203.0.113.{n}"))
            .body(Body::from(format!("email=web{n}%40example.test&cf-turnstile-response={WEB_TOKEN}")))
            .unwrap();
        let sent = site.send(request).await;
        assert!(location(&sent).starts_with("/login/verify"), "{n}: {}", location(&sent));
    }
    assert_eq!(site.state.mailer.sign_in_codes().len(), 10);
}

/// With every place for the database work taken, a start or resend is
/// `503 busy` after about two seconds, and takes no pool connection while
/// it waits: here every connection is held too, and it still answers.
#[sqlx::test(migrations = "./migrations")]
async fn with_every_place_taken_a_start_is_busy_and_holds_no_connection(db: PgPool) {
    let site = Site::with_mailbox(db);
    let started = start(&site, ANA).await;
    expire(&site, &started).await;
    let places = site.state.app_sign_in_slots.acquire_many(4).await.unwrap();
    let mut connections = Vec::new();
    for _ in 0..site.db.options().get_max_connections() {
        connections.push(site.db.acquire().await.unwrap());
    }
    for (path, body) in [
        ("/auth/email/start", json!({ "client_id": CLIENT, "email": ANA, "secret_hash": secret().1 })),
        ("/auth/email/resend", json!({ "request_id": started.id, "secret": started.secret })),
    ] {
        let began = Instant::now();
        let response = post_json(&site, path, &body).await;
        let took = began.elapsed();
        assert!(took >= Duration::from_millis(1900) && took < Duration::from_secs(5), "{path}: {took:?}");
        assert_eq!(response.headers()[header::RETRY_AFTER], "5");
        let (status, answer) = read(response).await;
        assert_eq!(
            answer,
            json!({ "error": "busy", "message": "Buwiz is busy. Nothing was sent; try again in a few seconds.", "retry_after": 5 })
        );
        assert_eq!(status, 503, "{path}");
    }
    drop(connections);
    drop(places);
    assert_eq!(resend(&site, &started).await.0, 200);
}

/// The advisory lock of the testing cap ([`UNCHECKED_LOCK`] in the code).
const UNCHECKED_LOCK: i64 = 0x6275_7769_7a02;

/// A full testing cap refuses at once, without its lock: here another
/// connection holds it. A start that must take the lock waits at most
/// three seconds for it, then is `503 busy`.
#[sqlx::test(migrations = "./migrations")]
async fn a_full_testing_cap_refuses_without_waiting_for_its_lock(db: PgPool) {
    let site = Site::with_mailbox(db);
    site.person(ANA).await;
    let mut holder = site.db.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(UNCHECKED_LOCK).execute(&mut *holder).await.unwrap();

    unchecked(&site.db, 20, "1 minute").await;
    let began = Instant::now();
    let (status, answer, _) = start_with(&site, ANA, json!({})).await;
    assert_eq!((status, answer["error"].as_str()), (429, Some("too_many_requests")), "{answer}");
    assert!(began.elapsed() < Duration::from_secs(1), "{:?}", began.elapsed());

    // Room in the cap: the code must be added under the lock, which is not
    // had in time.
    sqlx::query("DELETE FROM email_codes WHERE email = 'filler@example.test'").execute(&site.db).await.unwrap();
    let began = Instant::now();
    let (status, answer, _) = start_with(&site, ANA, json!({})).await;
    assert_eq!((status, answer["error"].as_str()), (503, Some("busy")), "{answer}");
    assert!(began.elapsed() >= Duration::from_millis(2900), "{:?}", began.elapsed());
    assert_eq!(count(&site.db, "SELECT count(*) FROM app_sign_ins").await, 0);
    holder.rollback().await.unwrap();
    assert_eq!(start_with(&site, ANA, json!({})).await.0, 200);
}

/// The apps post to the API host only (`api.buwiz.com`): start, resend and
/// verify work there; on the account host they are not found. The web's
/// form is the account host's only.
#[sqlx::test(migrations = "./migrations")]
async fn the_app_endpoints_are_the_api_hosts_and_the_form_the_account_hosts(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (secret, hash) = secret();
    let body = json!({ "client_id": CLIENT, "email": ANA, "secret_hash": hash });
    let (status, answer) = read(post_json_to(&site, "api.buwiz.com", "/auth/email/start", &body).await).await;
    assert_eq!(status, 200, "{answer}");
    let started = Started { id: answer["request_id"].as_str().unwrap().to_owned(), secret };
    expire(&site, &started).await;
    let again = json!({ "request_id": started.id, "secret": started.secret });
    assert_eq!(read(post_json_to(&site, "api.buwiz.com", "/auth/email/resend", &again).await).await.0, 200);
    let code = json!({ "request_id": started.id, "secret": started.secret, "code": last_code(&site, ANA) });

    // Not on the account host, nor on any other.
    for host in ["account.buwiz.com", ACCOUNT_HOST, "buwiz.com", "ebirforms.buwiz.com"] {
        for (path, body) in
            [("/auth/email/start", &body), ("/auth/email/resend", &again), ("/auth/email/verify", &code)]
        {
            let response = post_json_to(&site, host, path, body).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{host}{path}");
        }
    }
    let (status, tokens) = read(post_json_to(&site, "api.buwiz.com", "/auth/email/verify", &code).await).await;
    assert_eq!((status, tokens["scope"].as_str()), (200, Some("app")), "{tokens}");

    // A developer's own machine (a loopback host, any port) takes both:
    // one address serves everything there.
    for (host, email) in [
        ("localhost:3018", "lo1@example.test"),
        ("127.0.0.1:3018", "lo2@example.test"),
        ("[::1]:3018", "lo3@example.test"),
    ] {
        let body = json!({ "client_id": CLIENT, "email": email, "secret_hash": hash });
        let (status, answer) = read(post_json_to(&site, host, "/auth/email/start", &body).await).await;
        assert_eq!(status, 200, "{host}: {answer}");
    }
    // A name that only looks like one is not: `localhost.example` is a host
    // of its own.
    for host in ["localhost.example.test", "notlocalhost:3018"] {
        let body = json!({ "client_id": CLIENT, "email": "lo4@example.test", "secret_hash": hash });
        assert_eq!(
            post_json_to(&site, host, "/auth/email/start", &body).await.status(),
            StatusCode::NOT_FOUND,
            "{host}"
        );
    }

    // The web's form: on the account host only.
    for host in ["api.buwiz.com", API_HOST, "buwiz.com", "ebirforms.buwiz.com"] {
        for (path, form) in [
            ("/auth/email/start", "email=bea%40example.test".to_owned()),
            ("/auth/email/verify", "email=bea%40example.test&code=123456".to_owned()),
            ("/auth/email/resend", "email=bea%40example.test".to_owned()),
        ] {
            let request = Request::post(path)
                .header(header::HOST, host)
                .header(header::ORIGIN, ORIGIN)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap();
            assert_eq!(site.send(request).await.status(), StatusCode::NOT_FOUND, "{host}{path}");
        }
    }
    assert!(site.state.mailer.sign_in_codes().is_empty());
    let sent = site.post("/auth/email/start", "", "email=bea%40example.test").await;
    assert!(location(&sent).starts_with("/login/verify"), "{}", location(&sent));
    let (_, web_code) = site.state.mailer.sign_in_codes().pop().unwrap();
    let signed_in = site.post("/auth/email/verify", "", &format!("email=bea%40example.test&code={web_code}")).await;
    assert!(signed_in.headers().contains_key(header::SET_COOKIE));
}

/// Waits a moment for mail sent off the request (a spawned task).
async fn notices_settle(site: &Site, at_least: usize) -> Vec<buwiz_cloud::account::mail::Notice> {
    for _ in 0..40 {
        if site.state.mailer.notices().len() >= at_least {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    site.state.mailer.notices()
}

/// A Turnstile answer without the hostname or the action: the web's form
/// goes on as before these were checked, and staff are mailed once an
/// hour; an app's check is refused.
#[sqlx::test(migrations = "./migrations")]
async fn an_answer_without_hostname_or_action_passes_the_web_with_an_alert_and_fails_the_app(db: PgPool) {
    let site = Site::with_turnstile_mailbox(db, &turnstile().await);
    for (n, token) in ["web-no-hostname", "web-no-action", "web-no-hostname"].iter().enumerate() {
        let form = format!("email=web{n}%40example.test&cf-turnstile-response={token}");
        let sent = site.post("/auth/email/start", "", &form).await;
        assert!(location(&sent).starts_with("/login/verify"), "{token}: {}", location(&sent));
    }
    assert_eq!(site.state.mailer.sign_in_codes().len(), 3);
    let notices = notices_settle(&site, 1).await;
    assert_eq!(notices.len(), 1, "once an hour: {notices:?}");
    assert_eq!(notices[0].to, STAFF);
    assert_eq!(notices[0].subject, "Sign-in: Turnstile's answers lack the hostname or action");

    for token in ["app-no-hostname", "app-no-action"] {
        let (status, answer, _) = start_with(&site, ANA, json!({ "turnstile": token })).await;
        assert_eq!((status, answer["error"].as_str()), (403, Some("turnstile_failed")), "{token}");
    }
    assert!(site.state.mailer.app_sign_in_mails().is_empty());
    assert_eq!(notices_settle(&site, 2).await.len(), 1, "the app's refusals raise nothing");
}
