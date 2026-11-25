//! One device row per install (docs/accounts.md, "Devices"): an app's
//! `install_id` rides on its sign-in, and a second sign-in from the same
//! install updates its one row instead of adding one. Also the sign-ins
//! not yet added as a device, and staff logging a device out. Each test
//! gets its own database from `#[sqlx::test]`.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use buwiz_cloud::base::crypto;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use common::*;

const ANA: &str = "ana@example.test";
const PHONE: &str = "install-phone-0001";
const LAPTOP: &str = "install-laptop-0002";

/// A BIR app's sign-in for `account`, as verify or the device flow makes
/// it: (grant, access token).
async fn sign_in(db: &PgPool, account: Uuid, install: Option<&str>, platform: Option<&str>) -> (Uuid, String) {
    let grant: Uuid = sqlx::query_scalar(
        "INSERT INTO oauth_grants (account_id, client_id, scope, install_id, platform)
         VALUES ($1, 'buwiz-ebirforms', 'app', $2, $3) RETURNING id",
    )
    .bind(account)
    .bind(install)
    .bind(platform)
    .fetch_one(db)
    .await
    .unwrap();
    let token = crypto::new_token("bwa_");
    sqlx::query(
        "INSERT INTO oauth_access_tokens (grant_id, token_hash, expires_at) VALUES ($1, $2, now() + interval '15 minutes')",
    )
    .bind(grant)
    .bind(crypto::hash(&token))
    .execute(db)
    .await
    .unwrap();
    (grant, token)
}

/// Signature headers over `method path body` with `key`, and the device
/// header when there is one.
fn signed(
    mut request: axum::http::request::Builder,
    key: &SigningKey,
    method: &str,
    path: &str,
    body: &[u8],
    device: Option<Uuid>,
) -> axum::http::request::Builder {
    let timestamp = time::OffsetDateTime::now_utc().unix_timestamp();
    let nonce = Uuid::new_v4().simple().to_string();
    let input =
        buwiz_cloud::devices::signing_input(method, path, &crypto::hex(&crypto::sha256(body)), timestamp, &nonce);
    let signature = URL_SAFE_NO_PAD.encode(key.sign(input.as_bytes()).to_bytes());
    if let Some(device) = device {
        request = request.header("buwiz-device", device.to_string());
    }
    request
        .header("buwiz-timestamp", timestamp.to_string())
        .header("buwiz-nonce", nonce)
        .header("buwiz-signature", signature)
}

/// `POST /api/devices` with a new key and `fields`: (status, answer, key).
async fn register(site: &Site, token: &str, fields: Value) -> (u16, Value, SigningKey) {
    let key = SigningKey::from_bytes(&rand::random());
    let mut body =
        json!({ "public_key": URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes()), "name": "Ana's phone" });
    for (name, value) in fields.as_object().unwrap() {
        body[name] = value.clone();
    }
    let body = body.to_string();
    let request = Request::post("/api/devices")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json");
    let request = signed(request, &key, "POST", "/api/devices", body.as_bytes(), None);
    let response = site.send(request.body(Body::from(body)).unwrap()).await;
    let status = response.status().as_u16();
    (status, serde_json::from_str(&text(response).await).unwrap_or_default(), key)
}

async fn list(site: &Site, token: &str) -> (u16, Value) {
    let request = Request::get("/api/devices")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = site.send(request).await;
    let status = response.status().as_u16();
    (status, serde_json::from_str(&text(response).await).unwrap_or_default())
}

async fn revoked(db: &PgPool, grant: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT revoke_reason FROM oauth_grants WHERE id = $1").bind(grant).fetch_one(db).await.unwrap()
}

async fn rows(db: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM devices").fetch_one(db).await.unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_second_sign_in_from_one_install_updates_its_one_row(db: PgPool) {
    let site = Site::new(db);
    let (ana, _) = site.person(ANA).await;
    let (first_grant, first_token) = sign_in(&site.db, ana, Some(PHONE), Some("ios")).await;
    let fields = json!({ "install_id": PHONE, "platform": "ios", "model": "iPhone16,1", "os_version": "18.1" });
    let (status, first, old_key) = register(&site, &first_token, fields.clone()).await;
    assert_eq!(status, 201, "{first}");
    let id: Uuid = first["id"].as_str().unwrap().parse().unwrap();

    let (second_grant, second_token) = sign_in(&site.db, ana, Some(PHONE), Some("ios")).await;
    let (status, second, _) = register(&site, &second_token, fields).await;
    assert_eq!(status, 201, "{second}");
    assert_eq!(second["id"], first["id"], "the same row");
    assert_eq!(second["created_at"], first["created_at"]);
    assert_eq!(rows(&site.db).await, 1);
    assert_eq!(revoked(&site.db, first_grant).await.as_deref(), Some("device_replaced"));
    assert_eq!(revoked(&site.db, second_grant).await, None);
    // The old sign-in is over, and the old key no longer signs for the row.
    assert_eq!(list(&site, &first_token).await.0, 401);
    let request = Request::post("/api/tickets")
        .header(header::AUTHORIZATION, format!("Bearer {second_token}"))
        .header(header::CONTENT_TYPE, "application/json");
    let request = signed(request, &old_key, "POST", "/api/tickets", b"{}", Some(id));
    let (status, code, _) = refusal(site.send(request.body(Body::from("{}")).unwrap()).await).await;
    assert_eq!((status, code.as_str()), (401, "bad_signature"));

    // The list: one row, this sign-in's, named as sold.
    let (_, listed) = list(&site, &second_token).await;
    let devices = listed["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0]["current"], true);
    assert_eq!(devices[0]["model"], "iPhone16,1");
    assert_eq!(devices[0]["model_name"], "iPhone 15 Pro");
    assert_eq!((devices[0]["platform"].as_str(), devices[0]["os_version"].as_str()), (Some("ios"), Some("18.1")));
    assert!(devices[0]["signed_in_at"].is_string());
    assert_eq!(listed["pending"], json!([]));
}

#[sqlx::test(migrations = "./migrations")]
async fn another_install_is_another_row_and_without_one_it_is_as_before(db: PgPool) {
    let site = Site::new(db);
    let (ana, _) = site.person(ANA).await;
    let (_, phone) = sign_in(&site.db, ana, Some(PHONE), None).await;
    let (_, laptop) = sign_in(&site.db, ana, None, None).await;
    let (_, old_app) = sign_in(&site.db, ana, None, None).await;
    assert_eq!(register(&site, &phone, json!({ "install_id": PHONE })).await.0, 201);
    // The grant had no install: the registration stamps it.
    assert_eq!(register(&site, &laptop, json!({ "install_id": LAPTOP, "model": "Pixel 8" })).await.0, 201);
    let stamped: i64 = sqlx::query_scalar("SELECT count(*) FROM oauth_grants WHERE install_id = $1")
        .bind(LAPTOP)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert_eq!(stamped, 1);
    // No install id at all: a row as before.
    let (status, answer, _) = register(&site, &old_app, json!({})).await;
    assert_eq!(status, 201, "{answer}");
    assert_eq!(rows(&site.db).await, 3);
    let (_, listed) = list(&site, &old_app).await;
    let devices = listed["devices"].as_array().unwrap();
    let pixel = devices.iter().find(|device| device["model"] == "Pixel 8").unwrap();
    assert_eq!(pixel["model_name"], "Pixel 8");
    assert_eq!(devices.iter().filter(|device| device["current"] == true).count(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_registration_must_match_its_sign_in(db: PgPool) {
    let site = Site::new(db);
    let (ana, _) = site.person(ANA).await;
    let (_, token) = sign_in(&site.db, ana, Some(PHONE), None).await;
    for fields in [json!({ "install_id": LAPTOP }), json!({})] {
        let (status, answer, _) = register(&site, &token, fields).await;
        assert_eq!((status, answer["error"]["code"].as_str()), (400, Some("install_mismatch")), "{answer}");
    }
    let (status, answer, _) = register(&site, &token, json!({ "install_id": "short" })).await;
    assert_eq!((status, answer["error"]["code"].as_str()), (400, Some("bad_install_id")));
    let (status, answer, _) = register(&site, &token, json!({ "install_id": PHONE, "model": "x".repeat(81) })).await;
    assert_eq!((status, answer["error"]["code"].as_str()), (400, Some("bad_model")));
    let (status, answer, _) = register(&site, &token, json!({ "install_id": PHONE, "computer_name": "a\u{7}b" })).await;
    assert_eq!((status, answer["error"]["code"].as_str()), (400, Some("bad_computer_name")));
    // One registration per sign-in.
    assert_eq!(register(&site, &token, json!({ "install_id": PHONE, "platform": "symbian" })).await.0, 201);
    let (status, answer, _) = register(&site, &token, json!({ "install_id": PHONE })).await;
    assert_eq!((status, answer["error"]["code"].as_str()), (409, Some("device_registered")));
    let platform: Option<String> =
        sqlx::query_scalar("SELECT platform FROM devices").fetch_one(&site.db).await.unwrap();
    assert_eq!(platform, None, "an unknown platform is kept as nothing");
}

#[sqlx::test(migrations = "./migrations")]
async fn removed_devices_show_for_thirty_days(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person(ANA).await;
    let (_, token) = sign_in(&site.db, ana, None, None).await;
    let old = TestDevice::new(&site.db, ana).await;
    let recent = TestDevice::new(&site.db, ana).await;
    for (device, ago) in [(old.device_id, "31 days"), (recent.device_id, "29 days")] {
        sqlx::query(&format!("UPDATE devices SET revoked_at = now() - interval '{ago}' WHERE id = $1"))
            .bind(device)
            .execute(&site.db)
            .await
            .unwrap();
    }
    let (_, listed) = list(&site, &token).await;
    let ids: Vec<&str> =
        listed["devices"].as_array().unwrap().iter().map(|device| device["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [recent.device_id.to_string()]);
    let page = text(site.get("/tins", &cookie).await).await;
    assert_eq!(page.matches(r#"class="device-row""#).count(), 1, "{page}");
}

#[sqlx::test(migrations = "./migrations")]
async fn a_sign_in_without_a_device_is_listed_and_can_be_ended(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person(ANA).await;
    let (bob, _) = site.person("bob@example.test").await;
    let (grant, token) = sign_in(&site.db, ana, Some(PHONE), Some("ios")).await;
    let (other_grant, other_token) = sign_in(&site.db, ana, None, Some("android")).await;
    let (bob_grant, _) = sign_in(&site.db, bob, None, None).await;

    let (_, listed) = list(&site, &token).await;
    let pending = listed["pending"].as_array().unwrap();
    assert_eq!(pending.len(), 2);
    let mine = pending.iter().find(|sign_in| sign_in["grant_id"] == grant.to_string()).unwrap();
    assert_eq!((mine["current"].as_bool(), mine["platform"].as_str()), (Some(true), Some("ios")));
    assert_eq!(mine["client_id"], "buwiz-ebirforms");
    assert!(mine["signed_in_at"].is_string());
    let keys: Vec<&String> = mine.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["client_id", "current", "grant_id", "platform", "signed_in_at"]);

    // The account site lists them too, with a Log out.
    let page = text(site.get("/tins", &cookie).await).await;
    assert_eq!(page.matches("Signed in · not added as a device yet").count(), 2, "{page}");
    assert!(page.contains(&format!(r#"action="/apps/{other_grant}/sign-out""#)));

    // Another account's grant is not ours to end.
    let request = |grant: Uuid, token: &str| {
        Request::post(format!("/api/sign-ins/{grant}/sign-out"))
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(site.send(request(bob_grant, &token)).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(site.send(request(Uuid::new_v4(), &token)).await.status(), StatusCode::NOT_FOUND);
    // Ending the other one from this app: its token is refused after.
    let response = site.send(request(other_grant, &token)).await;
    assert_eq!(serde_json::from_str::<Value>(&text(response).await).unwrap(), json!({ "signed_out": true }));
    assert_eq!(list(&site, &other_token).await.0, 401);

    // Once this app registers, it moves from `pending` to `devices`.
    assert_eq!(register(&site, &token, json!({ "install_id": PHONE })).await.0, 201);
    let (_, listed) = list(&site, &token).await;
    assert_eq!(listed["pending"], json!([]));
    assert_eq!(listed["devices"][0]["current"], true);
    let page = text(site.get("/tins", &cookie).await).await;
    assert!(!page.contains("not added as a device yet"), "{page}");

    // On the site: the person logs out their own sign-in, not another's.
    let (third, third_token) = sign_in(&site.db, ana, None, None).await;
    let response = site.post(&format!("/apps/{bob_grant}/sign-out"), &cookie, "").await;
    assert_eq!(location(&response), "/tins");
    assert_eq!(revoked(&site.db, bob_grant).await, None);
    let response = site.post(&format!("/apps/{third}/sign-out"), &cookie, "").await;
    assert_eq!(location(&response), "/tins?notice=app_signed_out");
    assert_eq!(list(&site, &third_token).await.0, 401);
}

#[sqlx::test(migrations = "./migrations")]
async fn staff_log_a_device_out_with_a_reason(db: PgPool) {
    let site = Site::new(db);
    let (ana, _) = site.person(ANA).await;
    let (_, support) = site.staff_with_role("support@example.test", "support").await;
    let (_, nobody) = site.person("bob@example.test").await;
    let (grant, token) = sign_in(&site.db, ana, Some(PHONE), Some("ios")).await;
    let (_, answer, _) = register(&site, &token, json!({ "install_id": PHONE, "model": "iPhone16,1" })).await;
    let device = answer["id"].as_str().unwrap().to_owned();

    let page = text(site.get(&format!("/staff/users/{ana}"), &support).await).await;
    assert!(page.contains("iPhone 15 Pro") && page.contains(&format!("/devices/{device}/sign-out")), "{page}");

    let path = format!("/staff/users/{ana}/devices/{device}/sign-out");
    // Not staff: as if there were no such page.
    assert_eq!(site.post(&path, &nobody, "reason=Lost+phone&tab=sign").await.status(), StatusCode::NOT_FOUND);
    // A reason is needed.
    let response = site.post(&path, &support, "reason=&tab=sign").await;
    assert!(!location(&response).contains("app_signed_out"), "{}", location(&response));
    assert_eq!(revoked(&site.db, grant).await, None);

    let response = site.post(&path, &support, "reason=Lost+phone&tab=sign").await;
    assert!(location(&response).contains("notice=app_signed_out"), "{}", location(&response));
    assert_eq!(revoked(&site.db, grant).await.as_deref(), Some("signed_out_by_staff"));
    assert_eq!(list(&site, &token).await.0, 401);
    let lines = account_audit_lines(&site.db, ana).await;
    let line = lines.last().unwrap();
    assert_eq!(
        (line.0.as_str(), line.2.as_deref(), line.5.as_deref()),
        ("app_signed_out", Some("buwiz-ebirforms"), Some("Lost phone"))
    );
    assert_eq!(line.3, "support@example.test");
}

#[sqlx::test(migrations = "./migrations")]
async fn the_device_flow_carries_the_install_to_its_grant(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person(ANA).await;
    let start = |body: Value| {
        Request::post("/oauth/device/code")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let response = site.send(start(json!({ "client_id": "buwiz-desktop", "install_id": "bad" }))).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let refused: Value = serde_json::from_str(&text(response).await).unwrap();
    assert_eq!(refused["error"], "bad_install_id");

    let response = site.send(start(json!({ "client_id": "buwiz-desktop", "install_id": LAPTOP }))).await;
    let started: Value = serde_json::from_str(&text(response).await).unwrap();
    let user_code = started["user_code"].as_str().unwrap();
    site.post("/device/decision", &cookie, &format!("user_code={user_code}&decision=approve")).await;
    let poll = Request::post("/auth/device/poll")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "client_id": "buwiz-desktop", "device_code": started["device_code"] }).to_string()))
        .unwrap();
    let response = site.send(poll).await;
    assert_eq!(response.status(), StatusCode::OK);
    let tokens: Value = serde_json::from_str(&text(response).await).unwrap();
    let install: Option<String> = sqlx::query_scalar("SELECT install_id FROM oauth_grants WHERE account_id = $1")
        .bind(ana)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert_eq!(install.as_deref(), Some(LAPTOP));
    let token = tokens["access_token"].as_str().unwrap();
    let (status, answer, _) = register(&site, token, json!({ "install_id": PHONE })).await;
    assert_eq!((status, answer["error"]["code"].as_str()), (400, Some("install_mismatch")));
    assert_eq!(register(&site, token, json!({ "install_id": LAPTOP })).await.0, 201);
}

/// Apps from before install ids register again on the same sign-in as they
/// always could: another row, and the newest key signs.
#[sqlx::test(migrations = "./migrations")]
async fn an_old_app_may_register_again_on_its_sign_in(db: PgPool) {
    let site = Site::new(db);
    let (ana, _) = site.person(ANA).await;
    let (_, token) = sign_in(&site.db, ana, None, None).await;
    assert_eq!(register(&site, &token, json!({})).await.0, 201);
    let (status, answer, key) = register(&site, &token, json!({})).await;
    assert_eq!(status, 201, "{answer}");
    assert_eq!(rows(&site.db).await, 2);
    let id: Uuid = answer["id"].as_str().unwrap().parse().unwrap();
    // A signed route takes the newest key: past the signature, the ticket
    // is refused for its body, not its device.
    let request = Request::post("/api/tickets")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json");
    let request = signed(request, &key, "POST", "/api/tickets", b"{}", Some(id));
    let (status, code, _) = refusal(site.send(request.body(Body::from("{}")).unwrap()).await).await;
    assert_eq!((status, code.as_str()), (400, "bad_request"), "{status} {code}");
    let used: bool = sqlx::query_scalar("SELECT last_used_at IS NOT NULL FROM devices WHERE id = $1")
        .bind(id)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert!(used);
}

/// A sign-in without an install may not take an install whose device
/// belongs to another live sign-in of the account; once that sign-in is
/// over (ended, or its tokens expired) it may.
#[sqlx::test(migrations = "./migrations")]
async fn a_sign_in_without_an_install_cannot_take_a_live_devices_install(db: PgPool) {
    let site = Site::new(db);
    let (ana, _) = site.person(ANA).await;
    let (phone_grant, phone) = sign_in(&site.db, ana, Some(PHONE), None).await;
    let (_, first, _) = register(&site, &phone, json!({ "install_id": PHONE })).await;
    let (other_grant, other) = sign_in(&site.db, ana, None, None).await;
    let (status, answer, _) = register(&site, &other, json!({ "install_id": PHONE })).await;
    assert_eq!((status, answer["error"]["code"].as_str()), (409, Some("install_in_use")), "{answer}");
    // Nothing changed: the phone's sign-in and device, and the other grant.
    assert_eq!(revoked(&site.db, phone_grant).await, None);
    assert_eq!(rows(&site.db).await, 1);
    let stamped: Option<String> = sqlx::query_scalar("SELECT install_id FROM oauth_grants WHERE id = $1")
        .bind(other_grant)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert_eq!(stamped, None);
    assert_eq!(list(&site, &phone).await.0, 200);

    // The phone's tokens expire: the install may be taken.
    sqlx::query("UPDATE oauth_access_tokens SET expires_at = now() - interval '1 minute' WHERE grant_id = $1")
        .bind(phone_grant)
        .execute(&site.db)
        .await
        .unwrap();
    let (status, answer, _) = register(&site, &other, json!({ "install_id": PHONE })).await;
    assert_eq!(status, 201, "{answer}");
    assert_eq!(answer["id"], first["id"]);

    // And when the holding sign-in was ended.
    let (_, tablet) = sign_in(&site.db, ana, Some(LAPTOP), None).await;
    register(&site, &tablet, json!({ "install_id": LAPTOP })).await;
    let logout = Request::post("/api/auth/logout")
        .header(header::AUTHORIZATION, format!("Bearer {tablet}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(site.send(logout).await.status(), StatusCode::OK);
    let (_, late) = sign_in(&site.db, ana, None, None).await;
    assert_eq!(register(&site, &late, json!({ "install_id": LAPTOP })).await.0, 201);
}

/// However a sign-in ends, its device goes with it, and staff cannot
/// rewrite when a sign-in ended.
#[sqlx::test(migrations = "./migrations")]
async fn a_device_ends_with_its_sign_in(db: PgPool) {
    let site = Site::new(db);
    let (ana, _) = site.person(ANA).await;
    let (_, support) = site.staff_with_role("support@example.test", "support").await;
    let live = |db: PgPool, id: String| async move {
        sqlx::query_scalar::<_, bool>("SELECT revoked_at IS NULL FROM devices WHERE id = $1::uuid")
            .bind(id)
            .fetch_one(&db)
            .await
            .unwrap()
    };

    // Signing out.
    let (_, token) = sign_in(&site.db, ana, None, None).await;
    let (_, device, _) = register(&site, &token, json!({})).await;
    let logout = Request::post("/api/auth/logout")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    site.send(logout).await;
    assert!(!live(site.db.clone(), device["id"].as_str().unwrap().to_owned()).await);

    // A refresh token used twice ends the sign-in.
    let (grant, token) = sign_in(&site.db, ana, None, None).await;
    let (_, device, _) = register(&site, &token, json!({})).await;
    let refresh = crypto::new_token("bwr_");
    sqlx::query(
        "INSERT INTO oauth_refresh_tokens (grant_id, token_hash, expires_at, used_at)
         VALUES ($1, $2, now() + interval '30 days', now())",
    )
    .bind(grant)
    .bind(crypto::hash(&refresh))
    .execute(&site.db)
    .await
    .unwrap();
    let reuse = Request::post("/oauth/token")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!("grant_type=refresh_token&client_id=buwiz-ebirforms&refresh_token={refresh}")))
        .unwrap();
    assert_eq!(site.send(reuse).await.status(), StatusCode::BAD_REQUEST);
    assert_eq!(revoked(&site.db, grant).await.as_deref(), Some("refresh_token_reuse"));
    let device_id = device["id"].as_str().unwrap().to_owned();
    assert!(!live(site.db.clone(), device_id.clone()).await);

    // Staff signing that device out afterwards change nothing.
    let ended: String = sqlx::query_scalar("SELECT revoked_at::text FROM oauth_grants WHERE id = $1")
        .bind(grant)
        .fetch_one(&site.db)
        .await
        .unwrap();
    let path = format!("/staff/users/{ana}/devices/{device_id}/sign-out");
    let response = site.post(&path, &support, "reason=Lost+phone&tab=sign").await;
    assert!(location(&response).contains("notice=app_already_signed_out"), "{}", location(&response));
    let still: String = sqlx::query_scalar("SELECT revoked_at::text FROM oauth_grants WHERE id = $1")
        .bind(grant)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert_eq!(still, ended);
}
