//! An Admin bans an account and lifts the ban: no website session, no
//! sign-in, no API call but signing out (and removing a device), and a
//! clear message at sign-in. App sign-ins stay and work again once lifted.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use common::Part::Text;
use common::*;

const BANNED: &str = "This account can't be used on Buwiz. If you think this is a mistake, write to support@buwiz.com.";

async fn bearer_get(site: &Site, token: &str, path: &str) -> StatusCode {
    let request =
        Request::get(path).header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap();
    site.send(request).await.status()
}

/// Signs in with an emailed code: (where it goes, a session cookie set).
async fn sign_in_with_code(site: &Site, email: &str) -> (String, bool) {
    let code = buwiz_cloud::accounts::issue_code(&site.db, email).await.unwrap();
    let response = site.post("/auth/email/verify", "", &format!("email={email}&code={code}")).await;
    let cookie = response.headers().get(header::SET_COOKIE).is_some();
    (location(&response), cookie)
}

async fn pending_device_sign_in(db: &PgPool, account: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO device_authorizations
             (client_id, device_code_hash, user_code, interval_seconds, expires_at, account_id, decision)
         VALUES ('buwiz-desktop', decode('aa', 'hex'), 'BCDF-GHJK', 5, now() + interval '10 minutes', $1, 'approved')
         RETURNING id",
    )
    .bind(account)
    .fetch_one(db)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_banned_account_cannot_sign_in_or_send_and_is_told_so(db: PgPool) {
    let site = Site::new(db);
    let (_, staff) = site.person(STAFF).await;
    let (ana, ana_cookie) = site.person("ana@example.test").await;
    add_claim(&site.db, ana, "000123453", "approved").await;
    let device = TestDevice::new(&site.db, ana).await;
    let waiting = pending_device_sign_in(&site.db, ana).await;
    let user = format!("/staff/users/{ana}");

    let response = site.post(&format!("{user}/ban"), &staff, "reason=+").await;
    assert_eq!(location(&response), format!("{user}?notice=reason_missing"));
    assert_eq!(site.get("/tins", &ana_cookie).await.status(), StatusCode::OK);
    let response = site.post(&format!("{user}/ban"), &staff, "reason=Filed+for+strangers").await;
    assert_eq!(location(&response), format!("{user}?notice=account_banned_staff"));

    // The website: the session is gone; a new sign-in is refused with the
    // message, though the code email was still sent.
    let tins = site.get("/tins", &ana_cookie).await;
    assert!(location(&tins).starts_with("/login?"), "{}", location(&tins));
    assert_eq!(live_sessions(&site.db, ana).await, 0);
    let (to, cookie) = sign_in_with_code(&site, "ana@example.test").await;
    assert_eq!((to.as_str(), cookie), ("/login?notice=account_banned", false));
    let login = text(site.get("/login?notice=account_banned", "").await).await;
    assert!(login.contains(BANNED), "{login}");
    // A sign-in approved but not yet picked up by the app is ended.
    let consumed: bool = sqlx::query_scalar("SELECT consumed_at IS NOT NULL FROM device_authorizations WHERE id = $1")
        .bind(waiting)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert!(consumed);

    // The apps: the grant stays, every call but signing out and removing
    // the device is refused.
    assert_eq!(bearer_get(&site, &device.token, "/api/tax-profiles").await, StatusCode::FORBIDDEN);
    let ticket = device.post(&site, "/api/tickets", &json!({ "tin": "000123453", "form_type": "1701Qv2018" })).await;
    assert_eq!(refusal(ticket).await.1, "account_banned");
    let revoked: bool = sqlx::query_scalar("SELECT revoked_at IS NULL FROM oauth_grants WHERE account_id = $1")
        .bind(ana)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert!(revoked, "the ban leaves app sign-ins as they are");
    let page = text(site.get(&user, &staff).await).await;
    assert!(page.contains(&format!("Banned by {STAFF} on")) && page.contains("Filed for strangers"), "{page}");
    assert!(page.contains("Lift the ban on ana@example.test?"), "{page}");
    let banned_list = text(site.get("/staff/users?filter=banned", &staff).await).await;
    assert!(banned_list.contains("ana@example.test") && banned_list.contains("Banned"), "{banned_list}");

    // Lifted: the old app token works again, and so does signing in.
    let response = site.post(&format!("{user}/unban"), &staff, "reason=Cleared+up").await;
    assert_eq!(location(&response), format!("{user}?notice=ban_lifted"));
    assert_eq!(bearer_get(&site, &device.token, "/api/tax-profiles").await, StatusCode::OK);
    let (to, cookie) = sign_in_with_code(&site, "ana@example.test").await;
    assert_eq!((to.as_str(), cookie), ("/tins", true));
    assert!(!text(site.get("/staff/users?filter=banned", &staff).await).await.contains("ana@example.test"));

    let lines = account_audit_lines(&site.db, ana).await;
    let actions: Vec<(&str, Option<&str>, &str)> =
        lines.iter().map(|line| (line.0.as_str(), line.5.as_deref(), line.4.as_str())).collect();
    assert_eq!(actions, [("banned", Some("Filed for strangers"), "web"), ("unbanned", Some("Cleared up"), "web")]);
    let history = text(site.get(&user, &staff).await).await;
    assert!(history.contains(&format!("Staff {STAFF} banned the account: Filed for strangers")), "{history}");
    assert!(history.contains(&format!("Staff {STAFF} lifted the ban: Cleared up")), "{history}");
}

/// What a website session could still reach is closed by the ban: the TIN
/// forms, approving an app's sign-in, consenting to an agent, removing a
/// device, and the staff pages for someone in ADMIN_EMAILS.
#[sqlx::test(migrations = "./migrations")]
async fn a_ban_closes_every_website_session_door(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person("ana@example.test").await;
    let (server_admin, staff_cookie) = site.person(STAFF).await;
    for id in [ana, server_admin] {
        sqlx::query("UPDATE accounts SET banned_at = now(), ban_reason = 'Test' WHERE id = $1")
            .bind(id)
            .execute(&site.db)
            .await
            .unwrap();
    }
    let parts =
        [("kind", Text("personal")), ("tin", Text("000123453")), ("registered_name", Text("Ana")), ("skip", Text("1"))];
    let add = site.upload("/tins/claims", &cookie, &parts).await;
    assert!(location(&add).starts_with("/login?"), "{}", location(&add));
    // Approving an app's sign-in and removing a device ask for a sign-in;
    // consenting to an agent is refused outright, as for anyone signed out.
    for path in ["/device/decision".to_owned(), format!("/devices/{}/revoke", Uuid::new_v4())] {
        let response = site.post(&path, &cookie, "user_code=BCDF-GHJK&decision=approve").await;
        assert!(location(&response).starts_with("/login?"), "{path}: {}", response.status());
    }
    let consent = site.post("/oauth/authorize/decision", &cookie, "decision=approve").await;
    assert_eq!(consent.status(), StatusCode::FORBIDDEN);
    assert_eq!(text(consent).await, "Sign in first.");
    let claims: i64 = sqlx::query_scalar("SELECT count(*) FROM tin_claims").fetch_one(&site.db).await.unwrap();
    assert_eq!(claims, 0);
    for page in ["/tins", "/tins/new", "/device", "/staff", "/staff/users"] {
        let response = site.get(page, &staff_cookie).await;
        assert!(location(&response).starts_with("/login?"), "{page}: {}", response.status());
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn a_banned_app_can_still_remove_its_own_device(db: PgPool) {
    let site = Site::new(db);
    let (ana, _) = site.person("ana@example.test").await;
    let device = TestDevice::new(&site.db, ana).await;
    sqlx::query("UPDATE accounts SET banned_at = now(), ban_reason = 'Test' WHERE id = $1")
        .bind(ana)
        .execute(&site.db)
        .await
        .unwrap();
    let request = Request::post(format!("/api/devices/{}/revoke", device.device_id))
        .header(header::AUTHORIZATION, format!("Bearer {}", device.token))
        .body(Body::empty())
        .unwrap();
    let response = site.send(request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let revoked: bool = sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM devices WHERE id = $1")
        .bind(device.device_id)
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert!(revoked);
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_cannot_be_banned_nor_oneself(db: PgPool) {
    let site = Site::new(db);
    let (boss, boss_cookie) = site.staff_with_role("boss@example.test", "admin").await;
    let (other, _) = site.staff_with_role("other@example.test", "admin").await;
    let (server_admin, _) = site.person(STAFF).await;
    let (helper, _) = site.staff_with_role("helper@example.test", "support").await;
    for (id, notice) in [(boss, "own_account"), (other, "ban_admin"), (server_admin, "role_from_server")] {
        let response = site.post(&format!("/staff/users/{id}/ban"), &boss_cookie, "reason=x").await;
        assert_eq!(location(&response), format!("/staff/users/{id}?notice={notice}"));
    }
    // Support can be banned; it takes the role's powers with it.
    let response = site.post(&format!("/staff/users/{helper}/ban"), &boss_cookie, "reason=Left").await;
    assert_eq!(location(&response), format!("/staff/users/{helper}?notice=account_banned_staff"));
    let banned: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts WHERE banned_at IS NOT NULL")
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert_eq!(banned, 1);
    let page = text(site.get(&format!("/staff/users/{other}"), &boss_cookie).await).await;
    assert!(page.contains("An admin can't be banned. Change the role first."), "{page}");
}
