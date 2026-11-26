//! Resetting a forgotten app PIN by email code (`POST /api/pin-reset/start`
//! and `/verify`; [`buwiz_cloud::account::pin_reset`]): the code goes to
//! the account's own address, works only on the device (or, for a practice
//! account, the sign-in) that asked, only for this purpose, and verify
//! answers `200` only once the audit line is committed and the account's
//! notice was taken by the mail service. Each test gets its own database
//! from `#[sqlx::test]`.

mod common;

use axum::body::Body;
use axum::http::{Request, header};
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use buwiz_cloud::base::crypto;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use common::*;

const ANA: &str = "ana@example.test";
/// The API host, where the apps post (`api.buwiz.com` in production).
const API_HOST: &str = "api.localhost:3008";
/// A made-up fixed code for the practice account of these tests.
const FIXED: &str = "481516";
const REVIEWER: &str = "reviewer@example.test";

/// An answer: its status, JSON and `Retry-After`.
struct Answer {
    status: u16,
    body: Value,
    retry_after: Option<i64>,
    no_store: bool,
}

impl Answer {
    fn code(&self) -> &str {
        self.body["error"]["code"].as_str().unwrap_or_default()
    }

    /// The status and error code.
    fn refused(&self) -> (u16, &str) {
        (self.status, self.code())
    }
}

async fn read(response: Response) -> Answer {
    let status = response.status().as_u16();
    let retry_after = response.headers().get(header::RETRY_AFTER).map(|value| value.to_str().unwrap().parse().unwrap());
    let no_store = response.headers().get(header::CACHE_CONTROL).is_some_and(|value| value == "no-store");
    let text = text(response).await;
    let body = serde_json::from_str(&text).unwrap_or_else(|_| panic!("not JSON: {text}"));
    Answer { status, body, retry_after, no_store }
}

async fn post(site: &Site, token: &str, path: &str, body: &str) -> Answer {
    let request = Request::post(path)
        .header(header::HOST, API_HOST)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap();
    read(site.send(request).await).await
}

async fn start(site: &Site, token: &str) -> Answer {
    post(site, token, "/api/pin-reset/start", "").await
}

/// A start that works: its request id.
async fn started(site: &Site, token: &str) -> String {
    let answer = start(site, token).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    answer.body["request_id"].as_str().unwrap().to_owned()
}

async fn verify(site: &Site, token: &str, request_id: &str, code: &str) -> Answer {
    let body = json!({ "request_id": request_id, "code": code }).to_string();
    post(site, token, "/api/pin-reset/verify", &body).await
}

/// The newest PIN reset code mailed.
fn last_code(site: &Site) -> String {
    site.state.mailer.pin_reset_code_mails().last().expect("a code was mailed").code.clone()
}

/// A code that is not `code`.
fn other_than(code: &str) -> String {
    if code == "000000" { "000001".into() } else { "000000".into() }
}

async fn count(db: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(db).await.unwrap()
}

async fn exec(db: &PgPool, sql: &str) {
    sqlx::raw_sql(sql).execute(db).await.unwrap_or_else(|error| panic!("{error}: {sql}"));
}

/// The request's code and the request itself stop working.
async fn expire(db: &PgPool, request_id: &str) {
    exec(
        db,
        &format!(
            "UPDATE email_codes SET expires_at = now() - interval '1 second'
              WHERE id = (SELECT code_id FROM pin_resets WHERE id = '{request_id}');
             UPDATE pin_resets SET expires_at = now() - interval '1 second' WHERE id = '{request_id}'"
        ),
    )
    .await;
}

/// The sign-in (grant) of `token`.
async fn grant_of(db: &PgPool, token: &str) -> Uuid {
    sqlx::query_scalar("SELECT grant_id FROM oauth_access_tokens WHERE token_hash = $1")
        .bind(crypto::hash(token))
        .fetch_one(db)
        .await
        .unwrap()
}

/// A second device of `account`, named `name`.
async fn device_named(db: &PgPool, account: Uuid, name: &str) -> TestDevice {
    let device = TestDevice::new(db, account).await;
    sqlx::query("UPDATE devices SET name = $2 WHERE id = $1")
        .bind(device.device_id)
        .bind(name)
        .execute(db)
        .await
        .unwrap();
    device
}

/// A practice account (a store reviewer's, with the fixed code [`FIXED`]),
/// as staff set one up.
async fn practice_account(site: &Site) -> Uuid {
    let (boss, _) = site.staff_with_role("boss@example.test", "admin").await;
    let (reviewer, _) = site.person(REVIEWER).await;
    sqlx::query(
        "UPDATE accounts SET test_account = true, fixed_code_hash = $2, fixed_code_set_by = $3, fixed_code_set_at = now()
         WHERE id = $1",
    )
    .bind(reviewer)
    .bind(crypto::hash(&format!("{REVIEWER}:{FIXED}")))
    .bind(boss)
    .execute(&site.db)
    .await
    .unwrap();
    reviewer
}

/// The staff alerts kept so far, once the spawned alert had time to run.
async fn staff_alerts(site: &Site, subject: &str) -> Vec<buwiz_cloud::account::mail::Notice> {
    for _ in 0..100 {
        let alerts: Vec<_> =
            site.state.mailer.notices().into_iter().filter(|notice| notice.subject == subject).collect();
        if !alerts.is_empty() {
            return alerts;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Vec::new()
}

/// Runs of digits in `text`.
fn numbers(text: &str) -> Vec<&str> {
    text.split(|c: char| !c.is_ascii_digit()).filter(|run| !run.is_empty()).collect()
}

#[sqlx::test(migrations = "./migrations")]
async fn the_code_is_mailed_and_verify_answers_only_once_the_account_is_told(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, _) = site.person(ANA).await;
    let phone = device_named(&site.db, ana, "Ana's iPhone").await;

    let answer = start(&site, &phone.token).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert!(answer.no_store);
    assert_eq!(answer.body["expires_in"], 300);
    let request_id = answer.body["request_id"].as_str().unwrap().to_owned();
    let mails = site.state.mailer.pin_reset_code_mails();
    assert_eq!(mails.len(), 1);
    assert_eq!((mails[0].to.as_str(), mails[0].subject.as_str()), (ANA, "Your eBIRForms PIN reset code"));
    assert_eq!(mails[0].code.len(), 6);
    // Nothing else was mailed, and no notice before verify.
    assert!(site.state.mailer.pin_reset_notices().is_empty());
    assert!(site.state.mailer.app_sign_in_mails().is_empty() && site.state.mailer.sign_in_codes().is_empty());
    let (device, purpose): (Option<Uuid>, String) = sqlx::query_as(
        "SELECT r.device_id, c.purpose FROM pin_resets r JOIN email_codes c ON c.id = r.code_id WHERE r.id = $1::uuid",
    )
    .bind(&request_id)
    .fetch_one(&site.db)
    .await
    .unwrap();
    assert_eq!((device, purpose.as_str()), (Some(phone.device_id), "pin_reset"));

    let before = time::OffsetDateTime::now_utc().unix_timestamp();
    let answer = verify(&site, &phone.token, &request_id, &mails[0].code).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert!(answer.no_store);
    assert_eq!(answer.body["verified"], true);
    assert_eq!(answer.body["device"], "Ana's iPhone");
    let at = answer.body["at"].as_i64().unwrap();
    assert!((before - 1..=before + 5).contains(&at), "{at}");

    let notices = site.state.mailer.pin_reset_notices();
    assert_eq!(notices.len(), 1);
    assert_eq!((notices[0].to.as_str(), notices[0].subject.as_str()), (ANA, "A PIN reset was asked for in eBIRForms"));
    assert!(notices[0].text.starts_with("A PIN reset was asked for on Ana's iPhone on "), "{}", notices[0].text);
    let lines = account_audit_lines(&site.db, ana).await;
    let reason = format!("device Ana's iPhone ({})", phone.device_id);
    assert_eq!(lines, [("pin_reset_verified".into(), None, None, ANA.into(), "app".into(), Some(reason))]);
    assert_eq!(count(&site.db, "SELECT count(*) FROM pin_resets WHERE used_at IS NOT NULL").await, 1);
    assert_eq!(count(&site.db, "SELECT count(*) FROM email_codes WHERE consumed_at IS NOT NULL").await, 1);

    // The line reads as the person's own on the staff pages, and is not
    // staff activity.
    let activity = buwiz_cloud::account::staff::recent_activity(&site.db, 10).await.unwrap();
    assert!(activity.iter().all(|line| line.action != "pin_reset_verified"), "{activity:?}");
}

#[sqlx::test(migrations = "./migrations")]
async fn a_notice_the_mail_service_does_not_take_gives_no_200(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, _) = site.person(ANA).await;
    let phone = TestDevice::new(&site.db, ana).await;
    let request_id = started(&site, &phone.token).await;
    let code = last_code(&site);

    site.state.mailer.fail_pin_reset_notices(true);
    let answer = verify(&site, &phone.token, &request_id, &code).await;
    assert_eq!(answer.refused(), (503, "mail_unavailable"));
    let message = answer.body["error"]["message"].as_str().unwrap();
    assert!(message.contains("Your PIN was not changed"), "{message}");
    // Verified and logged before the mail: the line is there, the request
    // and its code are spent.
    assert_eq!(account_audit_lines(&site.db, ana).await.len(), 1);
    assert_eq!(count(&site.db, "SELECT count(*) FROM pin_resets WHERE used_at IS NOT NULL").await, 1);
    assert!(site.state.mailer.pin_reset_notices().is_empty());

    // The spent code stays spent: the person asks for a new one.
    site.state.mailer.fail_pin_reset_notices(false);
    assert_eq!(verify(&site, &phone.token, &request_id, &code).await.refused(), (400, "code_invalid"));
    let request_id = started(&site, &phone.token).await;
    let answer = verify(&site, &phone.token, &request_id, &last_code(&site)).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert_eq!(account_audit_lines(&site.db, ana).await.len(), 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn staff_are_told_when_a_notice_was_not_taken(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, _) = site.person(ANA).await;
    let phone = TestDevice::new(&site.db, ana).await;
    let request_id = started(&site, &phone.token).await;
    // Only the account's notice fails; staff mail goes out.
    site.state.mailer.fail_pin_reset_notices(true);
    let answer = verify(&site, &phone.token, &request_id, &last_code(&site)).await;
    assert_eq!(answer.refused(), (503, "mail_unavailable"));
    let alerts = staff_alerts(&site, "PIN reset: the notice to the account was not mailed").await;
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].to, STAFF);
    assert!(alerts[0].text.contains(&format!("Account {ana}, request {request_id}")), "{}", alerts[0].text);
    assert!(!alerts[0].text.contains('@'), "{}", alerts[0].text);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_code_that_could_not_be_mailed_is_not_kept(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, _) = site.person(ANA).await;
    let phone = TestDevice::new(&site.db, ana).await;
    site.state.mailer.fail_pin_reset_codes(true);
    assert_eq!(start(&site, &phone.token).await.refused(), (503, "mail_unavailable"));
    assert_eq!(count(&site.db, "SELECT count(*) FROM pin_resets").await, 0);
    assert_eq!(count(&site.db, "SELECT count(*) FROM email_codes").await, 0);
    // Not held, and not counted: three more starts of the device's hour.
    site.state.mailer.fail_pin_reset_codes(false);
    for _ in 0..3 {
        let request_id = started(&site, &phone.token).await;
        expire(&site.db, &request_id).await;
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn a_request_works_only_on_the_device_that_made_it_and_once(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, _) = site.person(ANA).await;
    let phone = device_named(&site.db, ana, "Ana's iPhone").await;
    let laptop = device_named(&site.db, ana, "Ana's Mac").await;
    let request_id = started(&site, &phone.token).await;
    let code = last_code(&site);
    // Made on A, used on B: refused, as a wrong code is.
    assert_eq!(verify(&site, &laptop.token, &request_id, &code).await.refused(), (400, "code_invalid"));
    assert_eq!(verify(&site, &phone.token, &request_id, &code).await.status, 200);
    // Used: refused, on either.
    assert_eq!(verify(&site, &phone.token, &request_id, &code).await.refused(), (400, "code_invalid"));
    assert_eq!(verify(&site, &laptop.token, &request_id, &code).await.refused(), (400, "code_invalid"));
    // An unknown request, and another account's.
    let unknown = Uuid::new_v4().to_string();
    assert_eq!(verify(&site, &phone.token, &unknown, &code).await.refused(), (400, "code_invalid"));
    let (bob, _) = site.person("bob@example.test").await;
    let bobs = TestDevice::new(&site.db, bob).await;
    let request_id = started(&site, &phone.token).await;
    assert_eq!(verify(&site, &bobs.token, &request_id, &last_code(&site)).await.refused(), (400, "code_invalid"));
    assert_eq!(account_audit_lines(&site.db, ana).await.len(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_practice_accounts_request_works_only_on_its_sign_in(db: PgPool) {
    let site = Site::with_mailbox(db);
    let reviewer = practice_account(&site).await;
    // No device registered, as the app does for a practice account.
    let ipad = app_token(&site.db, reviewer).await;
    let other = app_token(&site.db, reviewer).await;

    let answer = post(&site, &ipad, "/api/pin-reset/start", r#"{"device_name": "Reviewer's iPad"}"#).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let request_id = answer.body["request_id"].as_str().unwrap().to_owned();
    // Its fixed code is its holder's: nothing is mailed.
    assert!(site.state.mailer.pin_reset_code_mails().is_empty());
    let device: Option<Uuid> = sqlx::query_scalar("SELECT device_id FROM pin_resets WHERE id = $1::uuid")
        .bind(&request_id)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert_eq!(device, None);

    // Made on grant A, used on grant B: refused.
    assert_eq!(verify(&site, &other, &request_id, FIXED).await.refused(), (400, "code_invalid"));
    // The notice is still needed before the 200.
    site.state.mailer.fail_pin_reset_notices(true);
    assert_eq!(verify(&site, &ipad, &request_id, FIXED).await.refused(), (503, "mail_unavailable"));
    site.state.mailer.fail_pin_reset_notices(false);
    assert_eq!(verify(&site, &ipad, &request_id, FIXED).await.refused(), (400, "code_invalid"), "used");

    let answer = post(&site, &ipad, "/api/pin-reset/start", r#"{"device_name": "Reviewer's iPad"}"#).await;
    let request_id = answer.body["request_id"].as_str().unwrap().to_owned();
    let answer = verify(&site, &ipad, &request_id, FIXED).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert_eq!(answer.body["device"], "Reviewer's iPad");
    let notices = site.state.mailer.pin_reset_notices();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].to, REVIEWER);
    assert!(notices[0].text.starts_with("A PIN reset was asked for on Reviewer's iPad on "), "{}", notices[0].text);
    assert!(notices[0].text.contains("and remove Reviewer's iPad."), "{}", notices[0].text);
    let grant = grant_of(&site.db, &ipad).await;
    let reasons: Vec<Option<String>> =
        account_audit_lines(&site.db, reviewer).await.into_iter().map(|line| line.5).collect();
    let line = Some(format!("grant {grant}"));
    assert_eq!(reasons, [line.clone(), line]);

    // Without a reported name: "this device". Never device_not_registered.
    let request_id = started(&site, &other).await;
    let answer = verify(&site, &other, &request_id, FIXED).await;
    assert_eq!((answer.status, &answer.body["device"]), (200, &json!("this device")));
    let notice = site.state.mailer.pin_reset_notices().pop().unwrap();
    assert!(notice.text.starts_with("A PIN reset was asked for on this device on "), "{}", notice.text);
    assert!(site.state.mailer.pin_reset_code_mails().is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn a_practice_accounts_code_must_be_its_own(db: PgPool) {
    let site = Site::with_mailbox(db);
    let reviewer = practice_account(&site).await;
    let ipad = app_token(&site.db, reviewer).await;
    let request_id = started(&site, &ipad).await;
    assert_eq!(verify(&site, &ipad, &request_id, &other_than(FIXED)).await.refused(), (400, "code_invalid"));
    assert_eq!(verify(&site, &ipad, &request_id, FIXED).await.status, 200);
}

/// A code of `purpose` for `email`, stored as its own path stores one,
/// without mailing it.
async fn plant_code(db: &PgPool, email: &str, code: &str, purpose: &str) {
    sqlx::query(
        "INSERT INTO email_codes (email, code_hash, expires_at, purpose) VALUES ($1, $2, now() + interval '10 minutes', $3)",
    )
    .bind(email)
    .bind(crypto::hash(&format!("{email}:{code}")))
    .bind(purpose)
    .execute(db)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn a_sign_in_or_delete_code_never_resets_a_pin(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, _) = site.person(ANA).await;
    let phone = TestDevice::new(&site.db, ana).await;
    let request_id = started(&site, &phone.token).await;
    let reset = last_code(&site);
    let (sign_in, delete) = if reset.starts_with('1') { ("222222", "333333") } else { ("111111", "222222") };
    plant_code(&site.db, ANA, sign_in, "sign_in").await;
    plant_code(&site.db, ANA, delete, "delete").await;
    for code in [sign_in, delete] {
        assert_eq!(verify(&site, &phone.token, &request_id, code).await.refused(), (400, "code_invalid"), "{code}");
    }
    // Those codes were not touched, and still work for their own purpose.
    assert_eq!(
        count(&site.db, "SELECT count(*) FROM email_codes WHERE purpose <> 'pin_reset' AND attempts = 0").await,
        2
    );
    let response = site.post("/auth/email/verify", "", &format!("email={ANA}&code={sign_in}")).await;
    assert_eq!(location(&response), "/tins");
    assert_eq!(verify(&site, &phone.token, &request_id, &reset).await.status, 200);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_pin_reset_code_never_signs_in_or_deletes(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, cookie) = site.person(ANA).await;
    let phone = TestDevice::new(&site.db, ana).await;
    let request_id = started(&site, &phone.token).await;
    let code = last_code(&site);

    // The web's sign-in form.
    let response = site.post("/auth/email/verify", "", &format!("email={ANA}&code={code}")).await;
    assert!(location(&response).starts_with("/login/verify?"), "{}", location(&response));
    // The account's deletion.
    let response = site.post("/delete/confirm", &cookie, &format!("code={code}")).await;
    assert_eq!(location(&response), "/delete?notice=delete_code_invalid");
    assert_eq!(count(&site.db, &format!("SELECT count(*) FROM accounts WHERE id = '{ana}'")).await, 1);
    // An app's sign-in: its request takes its own code only.
    let secret = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let hash = URL_SAFE_NO_PAD.encode(Sha256::digest(secret.as_bytes()));
    let body = json!({ "client_id": "buwiz-desktop", "email": ANA, "intent": "sign_in", "secret_hash": hash });
    let request = Request::post("/auth/email/start")
        .header(header::HOST, API_HOST)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let answer = read(site.send(request).await).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let sign_in_id = answer.body["request_id"].as_str().unwrap().to_owned();
    let app_code = site.state.mailer.app_sign_in_mails().last().unwrap().code.clone();
    if app_code != code {
        let body = json!({ "request_id": sign_in_id, "secret": secret, "code": code });
        let request = Request::post("/auth/email/verify")
            .header(header::HOST, API_HOST)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let answer = read(site.send(request).await).await;
        assert_eq!((answer.status, answer.body["error"].as_str()), (400, Some("invalid_code")), "{}", answer.body);
    }
    assert_eq!(count(&site.db, "SELECT count(*) FROM web_sessions WHERE revoked_at IS NULL").await, 1);
    // None of them used it: it still resets the PIN.
    assert_eq!(count(&site.db, "SELECT attempts::bigint FROM email_codes WHERE purpose = 'pin_reset'").await, 0);
    assert_eq!(verify(&site, &phone.token, &request_id, &code).await.status, 200);
}

#[sqlx::test(migrations = "./migrations")]
async fn an_expired_code_is_code_expired(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, _) = site.person(ANA).await;
    let phone = TestDevice::new(&site.db, ana).await;
    let request_id = started(&site, &phone.token).await;
    expire(&site.db, &request_id).await;
    assert_eq!(verify(&site, &phone.token, &request_id, &last_code(&site)).await.refused(), (400, "code_expired"));
    // Its code ended a moment before the request: the same.
    let request_id = started(&site, &phone.token).await;
    exec(
        &site.db,
        &format!(
            "UPDATE email_codes SET expires_at = now() - interval '1 second'
              WHERE id = (SELECT code_id FROM pin_resets WHERE id = '{request_id}')"
        ),
    )
    .await;
    assert_eq!(verify(&site, &phone.token, &request_id, &last_code(&site)).await.refused(), (400, "code_expired"));
    assert!(account_audit_lines(&site.db, ana).await.is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn a_code_tried_too_many_times_is_done(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, _) = site.person(ANA).await;
    let phone = TestDevice::new(&site.db, ana).await;
    let request_id = started(&site, &phone.token).await;
    let code = last_code(&site);
    let wrong = other_than(&code);
    for _ in 0..4 {
        assert_eq!(verify(&site, &phone.token, &request_id, &wrong).await.refused(), (400, "code_invalid"));
    }
    // The fifth wrong try (CODE_ATTEMPTS) uses the code up, committed.
    assert_eq!(verify(&site, &phone.token, &request_id, &wrong).await.refused(), (429, "too_many_tries"));
    assert_eq!(count(&site.db, "SELECT attempts::bigint FROM email_codes").await, 5);
    assert_eq!(verify(&site, &phone.token, &request_id, &code).await.refused(), (429, "too_many_tries"));
    assert!(account_audit_lines(&site.db, ana).await.is_empty());
    // It no longer holds the device: a new code may be asked for.
    let request_id = started(&site, &phone.token).await;
    assert_eq!(verify(&site, &phone.token, &request_id, &last_code(&site)).await.status, 200);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_device_gets_one_working_code_at_a_time(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, _) = site.person(ANA).await;
    let phone = TestDevice::new(&site.db, ana).await;
    let laptop = TestDevice::new(&site.db, ana).await;
    let request_id = started(&site, &phone.token).await;
    let answer = start(&site, &phone.token).await;
    assert_eq!(answer.refused(), (429, "code_already_sent"));
    assert!((1..=300).contains(&answer.retry_after.unwrap()), "{:?}", answer.retry_after);
    assert!(answer.no_store);
    assert_eq!(site.state.mailer.pin_reset_code_mails().len(), 1);
    // Another device has its own.
    started(&site, &laptop.token).await;
    // Once the code no longer works, a new one.
    expire(&site.db, &request_id).await;
    started(&site, &phone.token).await;
    // code_already_sent took no start of the device's three: one is left.
    let request_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM pin_resets WHERE device_id = $1 AND expires_at > now() ORDER BY created_at DESC LIMIT 1",
    )
    .bind(phone.device_id)
    .fetch_one(&site.db)
    .await
    .unwrap();
    expire(&site.db, &request_id.to_string()).await;
    started(&site, &phone.token).await;
}

#[sqlx::test(migrations = "./migrations")]
async fn starts_are_limited_per_device_and_per_account_an_hour(db: PgPool) {
    let site = Site::with_mailbox(db.clone());
    let (ana, _) = site.person(ANA).await;
    let phone = TestDevice::new(&site.db, ana).await;
    let laptop = TestDevice::new(&site.db, ana).await;
    let tablet = TestDevice::new(&site.db, ana).await;
    for _ in 0..3 {
        let request_id = started(&site, &phone.token).await;
        expire(&site.db, &request_id).await;
    }
    let answer = start(&site, &phone.token).await;
    assert_eq!(answer.refused(), (429, "rate_limited"));
    assert!((3000..=3600).contains(&answer.retry_after.unwrap()), "{:?}", answer.retry_after);
    // Another device of the account: two more, the account's five.
    for _ in 0..2 {
        let request_id = started(&site, &laptop.token).await;
        expire(&site.db, &request_id).await;
    }
    // (Moved out of the address's own five codes an hour, so only the
    // account's limit in memory is left to refuse.)
    exec(&site.db, "UPDATE email_codes SET created_at = now() - interval '2 hours'").await;
    let answer = start(&site, &tablet.token).await;
    assert_eq!(answer.refused(), (429, "rate_limited"));
    assert!(answer.retry_after.unwrap() > 0);
    assert_eq!(count(&site.db, "SELECT count(*) FROM pin_resets").await, 5);
    // In memory: after a restart they start again.
    let restarted = Site::with_mailbox(db);
    started(&restarted, &tablet.token).await;
}

#[sqlx::test(migrations = "./migrations")]
async fn ten_starts_a_day_are_counted_in_the_database(db: PgPool) {
    let site = Site::with_mailbox(db.clone());
    let (ana, _) = site.person(ANA).await;
    let phone = TestDevice::new(&site.db, ana).await;
    let grant = grant_of(&site.db, &phone.token).await;
    // Nine from earlier today, before a restart.
    for hours in 2..11 {
        exec(
            &site.db,
            &format!(
                "INSERT INTO pin_resets (account_id, device_id, grant_id, device_name, created_at, expires_at)
                 VALUES ('{ana}', '{}', '{grant}', 'Test Mac', now() - interval '{hours} hours',
                         now() - interval '{hours} hours' + interval '5 minutes')",
                phone.device_id
            ),
        )
        .await;
    }
    let request_id = started(&site, &phone.token).await;
    expire(&site.db, &request_id).await;
    let answer = start(&site, &phone.token).await;
    assert_eq!(answer.refused(), (429, "rate_limited"));
    // Until the oldest of the day's ten is a day old: about 14 hours.
    let wait = answer.retry_after.unwrap();
    assert!((13 * 3600..=14 * 3600 + 5).contains(&wait), "{wait}");
    // A fresh limiter (as after a restart) does not reset it.
    let restarted = Site::with_mailbox(db);
    assert_eq!(start(&restarted, &phone.token).await.refused(), (429, "rate_limited"));
    // A day later the oldest no longer counts.
    exec(&restarted.db, "UPDATE pin_resets SET created_at = created_at - interval '14 hours'").await;
    started(&restarted, &phone.token).await;
}

#[sqlx::test(migrations = "./migrations")]
async fn verify_is_limited_to_ten_tries_a_device_an_hour(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, _) = site.person(ANA).await;
    let phone = TestDevice::new(&site.db, ana).await;
    let laptop = TestDevice::new(&site.db, ana).await;
    let request_id = started(&site, &phone.token).await;
    for _ in 0..10 {
        let unknown = Uuid::new_v4().to_string();
        assert_eq!(verify(&site, &phone.token, &unknown, "123456").await.refused(), (400, "code_invalid"));
    }
    let answer = verify(&site, &phone.token, &request_id, &last_code(&site)).await;
    assert_eq!(answer.refused(), (429, "rate_limited"));
    assert!((3000..=3600).contains(&answer.retry_after.unwrap()), "{:?}", answer.retry_after);
    // Nothing was taken, and another device is not held.
    assert_eq!(count(&site.db, "SELECT count(*) FROM email_codes WHERE consumed_at IS NULL").await, 1);
    let unknown = Uuid::new_v4().to_string();
    assert_eq!(verify(&site, &laptop.token, &unknown, "123456").await.refused(), (400, "code_invalid"));
}

#[sqlx::test(migrations = "./migrations")]
async fn only_the_bir_app_on_an_added_device_may_ask(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, _) = site.person(ANA).await;
    // Signed in, but no device added.
    let bare = app_token(&site.db, ana).await;
    assert_eq!(start(&site, &bare).await.refused(), (409, "device_not_registered"));
    let unknown = Uuid::new_v4().to_string();
    assert_eq!(verify(&site, &bare, &unknown, "123456").await.refused(), (409, "device_not_registered"));
    // A removed device is no device.
    let phone = TestDevice::new(&site.db, ana).await;
    let request_id = started(&site, &phone.token).await;
    exec(&site.db, &format!("UPDATE devices SET revoked_at = now() WHERE id = '{}'", phone.device_id)).await;
    assert_eq!(start(&site, &phone.token).await.refused(), (409, "device_not_registered"));
    assert_eq!(
        verify(&site, &phone.token, &request_id, &last_code(&site)).await.refused(),
        (409, "device_not_registered")
    );
    // Books and agents never.
    for (client, scope) in [("buwiz-books", "books"), ("agent_test", "mcp")] {
        let token = token(&site.db, ana, client, scope).await;
        assert_eq!(start(&site, &token).await.refused(), (403, "insufficient_scope"), "{scope}");
        assert_eq!(
            verify(&site, &token, &request_id, "123456").await.refused(),
            (403, "insufficient_scope"),
            "{scope}"
        );
    }
    // No token, or a made-up one.
    assert_eq!(start(&site, "bwa_made_up").await.refused(), (401, "invalid_token"));
    let request = Request::post("/api/pin-reset/start").header(header::HOST, API_HOST).body(Body::empty()).unwrap();
    assert_eq!(read(site.send(request).await).await.refused(), (401, "invalid_token"));
    // A malformed body.
    let other = TestDevice::new(&site.db, ana).await;
    let long = json!({ "device_name": "x".repeat(101) }).to_string();
    for body in ["nope", "[1]", long.as_str()] {
        assert_eq!(
            post(&site, &other.token, "/api/pin-reset/start", body).await.refused(),
            (400, "bad_request"),
            "{body}"
        );
    }
    for body in ["", "{}", r#"{"request_id": "not-an-id", "code": "123456"}"#] {
        assert_eq!(
            post(&site, &other.token, "/api/pin-reset/verify", body).await.refused(),
            (400, "bad_request"),
            "{body}"
        );
    }
    assert_eq!(count(&site.db, "SELECT count(*) FROM pin_resets").await, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_notice_says_where_and_when_with_no_code_and_no_link(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, _) = site.person(ANA).await;
    let phone = device_named(&site.db, ana, "Ana's iPhone").await;
    let request_id = started(&site, &phone.token).await;
    let code = last_code(&site);
    let answer = verify(&site, &phone.token, &request_id, &code).await;
    let at = time::OffsetDateTime::from_unix_timestamp(answer.body["at"].as_i64().unwrap()).unwrap();
    let notice = site.state.mailer.pin_reset_notices().pop().unwrap();
    let text = &notice.text;
    assert!(text.starts_with("A PIN reset was asked for on Ana's iPhone on "), "{text}");
    let utc = format!("{:02}:{:02} UTC", at.hour(), at.minute());
    assert!(text.contains(", Manila time (") && text.contains(&utc), "{text}");
    assert!(text.contains("If this was you, there is nothing to do."), "{text}");
    assert!(
        text.contains(
            "If it was not you: sign in at localhost:3008, open My devices, and remove Ana's iPhone. Removing it \
             signs it out and stops it from sending returns."
        ),
        "{text}"
    );
    for part in [notice.subject.as_str(), text.as_str()] {
        assert!(!part.contains(&code), "{part}");
        assert!(numbers(part).iter().all(|number| number.len() < 6), "{part}");
        assert!(!part.contains("://") && !part.contains("http"), "{part}");
    }
    // The code's own mail has the code as its only number, and no link but
    // the site's home.
    let mail = site.state.mailer.pin_reset_code_mails().pop().unwrap();
    assert!(mail.text.contains(&format!("Your Buwiz verification code is {code}")), "{}", mail.text);
    assert!(numbers(&mail.text).iter().all(|number| *number == code), "{}", mail.text);
    assert!(!mail.text.contains("Ana's iPhone") && !mail.html.contains("Ana's iPhone"));
    assert_eq!(mail.html.matches("<a ").count(), 1, "{}", mail.html);
    assert!(mail.html.contains(r#"href="https://buwiz.com""#));
}

#[sqlx::test(migrations = "./migrations")]
async fn deleting_the_account_takes_its_requests_and_keeps_the_line(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, cookie) = site.person(ANA).await;
    let phone = TestDevice::new(&site.db, ana).await;
    let request_id = started(&site, &phone.token).await;
    assert_eq!(verify(&site, &phone.token, &request_id, &last_code(&site)).await.status, 200);
    started(&site, &phone.token).await;
    assert_eq!(count(&site.db, "SELECT count(*) FROM pin_resets").await, 2);

    site.post("/delete/code", &cookie, "").await;
    let (_, code) = site.state.mailer.deletion_codes().pop().unwrap();
    let response = site.post("/delete/confirm", &cookie, &format!("code={code}")).await;
    assert_eq!(location(&response), "/login?notice=account_deleted");
    assert_eq!(count(&site.db, "SELECT count(*) FROM accounts WHERE email = 'ana@example.test'").await, 0);
    assert_eq!(count(&site.db, "SELECT count(*) FROM pin_resets").await, 0);
    // The line stays, by id, without the address.
    let lines = account_audit_lines(&site.db, ana).await;
    let actions: Vec<(&str, &str, &str)> =
        lines.iter().map(|line| (line.0.as_str(), line.3.as_str(), line.4.as_str())).collect();
    assert_eq!(actions, [("pin_reset_verified", "", "app"), ("deleted", "", "web")]);
}
