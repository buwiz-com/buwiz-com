//! "Sign in with Buwiz" for the eBIRForms apps (docs/accounts.md): the
//! built-in client `buwiz-desktop` on `/oauth/authorize` with PKCE, its
//! fixed redirect addresses, the first-party page, the exchange that
//! stamps the install on the grant, and the new-device mail. Also that
//! agents keep their own rules. Each test gets its own database from
//! `#[sqlx::test]`.

mod common;

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use buwiz_cloud::base::crypto;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use common::*;

const ANA: &str = "ana@example.test";
const CLIENT: &str = "buwiz-desktop";
const SCHEME: &str = "ebirforms://signin-callback";
const LOOPBACK: &str = "http://127.0.0.1:53682/callback";
const STATE: &str = "state-0123456789abcdef";
const PHONE: &str = "install-phone-0001";
const LAPTOP: &str = "install-laptop-0002";

fn verifier() -> String {
    "v".repeat(43)
}

fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// The app's authorize parameters, with `changes` (`None` leaves one out).
fn params(changes: &[(&str, Option<&str>)]) -> Vec<(String, String)> {
    let challenge = challenge(&verifier());
    let mut params: Vec<(String, String)> = [
        ("client_id", CLIENT),
        ("redirect_uri", SCHEME),
        ("response_type", "code"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", STATE),
        ("install_id", PHONE),
        ("platform", "ios"),
    ]
    .iter()
    .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
    .collect();
    for (key, value) in changes {
        params.retain(|(name, _)| name != key);
        if let Some(value) = value {
            params.push(((*key).to_owned(), (*value).to_owned()));
        }
    }
    params
}

fn encode(params: &[(String, String)]) -> String {
    form_urlencoded::Serializer::new(String::new()).extend_pairs(params).finish()
}

fn authorize_path(changes: &[(&str, Option<&str>)]) -> String {
    format!("/oauth/authorize?{}", encode(&params(changes)))
}

async fn decide(site: &Site, cookie: &str, changes: &[(&str, Option<&str>)], decision: &str) -> Response {
    let mut form = params(changes);
    form.push(("decision".into(), decision.into()));
    site.post("/oauth/authorize/decision", cookie, &encode(&form)).await
}

/// The query of a redirect to the app.
fn query_of(location: &str) -> HashMap<String, String> {
    url::Url::parse(location).unwrap().query_pairs().into_owned().collect()
}

/// Approves with `changes` and answers the code.
async fn approve(site: &Site, cookie: &str, changes: &[(&str, Option<&str>)]) -> String {
    let response = decide(site, cookie, changes, "approve").await;
    let location = location(&response);
    query_of(&location).remove("code").unwrap_or_else(|| panic!("no code: {location}"))
}

async fn token_request(site: &Site, form: &[(&str, &str)]) -> (u16, Value) {
    let form: Vec<(String, String)> = form.iter().map(|(key, value)| ((*key).into(), (*value).into())).collect();
    let request = Request::post("/oauth/token")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(encode(&form)))
        .unwrap();
    let response = site.send(request).await;
    let status = response.status().as_u16();
    (status, serde_json::from_str(&text(response).await).unwrap())
}

async fn exchange(site: &Site, client: &str, code: &str, redirect_uri: &str, verifier: &str) -> (u16, Value) {
    token_request(
        site,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", client),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
        ],
    )
    .await
}

/// The whole sign-in from `install` on `platform`: its tokens.
async fn sign_in(site: &Site, cookie: &str, install: &str, platform: &str) -> Value {
    let code = approve(site, cookie, &[("install_id", Some(install)), ("platform", Some(platform))]).await;
    let checks = site.state.mailer.new_device_checks();
    let (status, tokens) = exchange(site, CLIENT, &code, SCHEME, &verifier()).await;
    assert_eq!(status, 200, "{tokens}");
    settled(site, checks + 1).await;
    tokens
}

/// Waits until `n` new-device checks have finished: the mail goes on its
/// own task after the token answer (at most five seconds).
async fn settled(site: &Site, n: usize) {
    for _ in 0..100 {
        if site.state.mailer.new_device_checks() >= n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the new-device check never finished");
}

async fn count(db: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(db).await.unwrap()
}

/// The page refused the request without redirecting to the app.
async fn untrusted(response: Response) -> String {
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(location(&response), "");
    let page = text(response).await;
    assert!(page.contains("This sign-in request cannot be used"), "{page}");
    page
}

/// Signature headers over `method path body` with `key`.
fn signed(
    request: axum::http::request::Builder,
    key: &SigningKey,
    method: &str,
    path: &str,
    body: &[u8],
) -> axum::http::request::Builder {
    let timestamp = time::OffsetDateTime::now_utc().unix_timestamp();
    let nonce = Uuid::new_v4().simple().to_string();
    let input =
        buwiz_cloud::devices::signing_input(method, path, &crypto::hex(&crypto::sha256(body)), timestamp, &nonce);
    let signature = URL_SAFE_NO_PAD.encode(key.sign(input.as_bytes()).to_bytes());
    request
        .header("buwiz-timestamp", timestamp.to_string())
        .header("buwiz-nonce", nonce)
        .header("buwiz-signature", signature)
}

/// `POST /api/devices` for `install` with a new key.
async fn register(site: &Site, token: &str, install: &str) -> (u16, Value) {
    let key = SigningKey::from_bytes(&rand::random());
    let body = json!({
        "public_key": URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes()),
        "name": "Ana's phone",
        "install_id": install,
        "platform": "ios",
    })
    .to_string();
    let request = Request::post("/api/devices")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json");
    let request = signed(request, &key, "POST", "/api/devices", body.as_bytes());
    let response = site.send(request.body(Body::from(body)).unwrap()).await;
    let status = response.status().as_u16();
    (status, serde_json::from_str(&text(response).await).unwrap_or_default())
}

#[sqlx::test(migrations = "./migrations")]
async fn the_whole_flow_signs_the_app_in_with_its_install(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, cookie) = site.person(ANA).await;
    let response = site.get(&authorize_path(&[]), &cookie).await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = text(response).await;
    assert!(page.contains("Sign in to eBIRForms on iPhone"), "{page}");
    assert!(page.contains("Continue as ana@example.test"), "{page}");
    assert!(page.contains("Use another account") && page.contains("Cancel"), "{page}");
    assert!(page.contains("File and send returns for your TINs through Buwiz"), "{page}");
    assert!(page.contains("<title>Sign in to eBIRForms · Buwiz</title>"), "{page}");
    assert!(!page.contains("Unverified app") && !page.contains("wants to read"), "{page}");
    // Never approved on its own: nothing is stored until the tap.
    assert_eq!(count(&site.db, "SELECT count(*) FROM oauth_codes").await, 0);

    let response = decide(&site, &cookie, &[], "approve").await;
    let location = location(&response);
    assert!(location.starts_with("ebirforms://signin-callback?code=bwc_"), "{location}");
    let query = query_of(&location);
    assert_eq!(query["state"], STATE);
    assert_eq!(query["iss"], ORIGIN);
    let stored: (String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT scope, install_id, platform FROM oauth_codes").fetch_one(&site.db).await.unwrap();
    assert_eq!(stored, ("app".into(), Some(PHONE.into()), Some("ios".into())));

    let (status, tokens) = exchange(&site, CLIENT, &query["code"], SCHEME, &verifier()).await;
    assert_eq!(status, 200, "{tokens}");
    assert_eq!(tokens["scope"], "app");
    assert_eq!(tokens["token_type"], "Bearer");
    let grant: (Uuid, String, String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT account_id, client_id, scope, install_id, platform FROM oauth_grants")
            .fetch_one(&site.db)
            .await
            .unwrap();
    assert_eq!(grant, (ana, CLIENT.into(), "app".into(), Some(PHONE.into()), Some("ios".into())));

    // The token is the BIR app's, and refreshes.
    let me = Request::get("/me")
        .header(header::AUTHORIZATION, format!("Bearer {}", tokens["access_token"].as_str().unwrap()))
        .body(Body::empty())
        .unwrap();
    let me: Value = serde_json::from_str(&text(site.send(me).await).await).unwrap();
    assert_eq!(me["email"], ANA);
    let (status, refreshed) = token_request(
        &site,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", CLIENT),
            ("refresh_token", tokens["refresh_token"].as_str().unwrap()),
        ],
    )
    .await;
    assert_eq!((status, refreshed["scope"].as_str()), (200, Some("app")), "{refreshed}");

    // A device registers on the sign-in; a second sign-in from the same
    // install updates its one row.
    let (status, first) = register(&site, refreshed["access_token"].as_str().unwrap(), PHONE).await;
    assert_eq!(status, 201, "{first}");
    let again = sign_in(&site, &cookie, PHONE, "ios").await;
    let (status, second) = register(&site, again["access_token"].as_str().unwrap(), PHONE).await;
    assert_eq!(status, 201, "{second}");
    assert_eq!(second["id"], first["id"]);
    assert_eq!(count(&site.db, "SELECT count(*) FROM devices").await, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_loopback_address_takes_any_port_and_nothing_else(db: PgPool) {
    let site = Site::new(db);
    let (_, cookie) = site.person(ANA).await;
    for port in ["1", "53682", "65535"] {
        let redirect = format!("http://127.0.0.1:{port}/callback");
        let code = approve(&site, &cookie, &[("redirect_uri", Some(&redirect))]).await;
        let (status, tokens) = exchange(&site, CLIENT, &code, &redirect, &verifier()).await;
        assert_eq!(status, 200, "{redirect}: {tokens}");
    }
    for bad in [
        "http://localhost:53682/callback",
        "http://[::1]:53682/callback",
        "http://127.0.0.1:53682/other",
        "http://127.0.0.1:53682/callback/",
        "http://127.0.0.1/callback",
        "http://127.0.0.1:/callback",
        "http://127.0.0.1:53682/callback?x=1",
        "http://127.0.0.1:53682/callback#x",
        "http://user@127.0.0.1:53682/callback",
        "http://127.0.0.1:65536/callback",
        "https://127.0.0.1:53682/callback",
    ] {
        let page = untrusted(site.get(&authorize_path(&[("redirect_uri", Some(bad))]), &cookie).await).await;
        assert!(!page.contains("Continue as"), "{bad}");
        assert_eq!(decide(&site, &cookie, &[("redirect_uri", Some(bad))], "approve").await.status(), 400, "{bad}");
    }
    assert_eq!(count(&site.db, "SELECT count(*) FROM oauth_codes").await, 3);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_app_scheme_must_match_exactly(db: PgPool) {
    let site = Site::new(db);
    let (_, cookie) = site.person(ANA).await;
    for bad in [
        "ebirforms://signin-callback/",
        "ebirforms://signin-callback/path",
        "ebirforms://signin-callback?x=1",
        "ebirforms://signin-callback#x",
        "ebirforms://other",
        "EBIRFORMS://signin-callback",
    ] {
        untrusted(site.get(&authorize_path(&[("redirect_uri", Some(bad))]), &cookie).await).await;
    }
    untrusted(site.get(&authorize_path(&[("redirect_uri", None)]), &cookie).await).await;
}

/// A refusal sent back to the app: its error, and the state when there
/// was one.
async fn redirected_error(site: &Site, cookie: &str, changes: &[(&str, Option<&str>)]) -> HashMap<String, String> {
    let response = site.get(&authorize_path(changes), cookie).await;
    let location = location(&response);
    assert!(location.starts_with("ebirforms://signin-callback?error="), "{changes:?}: {location}");
    query_of(&location)
}

#[sqlx::test(migrations = "./migrations")]
async fn state_install_and_platform_are_required(db: PgPool) {
    let site = Site::new(db);
    let (_, cookie) = site.person(ANA).await;
    let missing = redirected_error(&site, &cookie, &[("state", None)]).await;
    assert_eq!(missing["error"], "invalid_request");
    assert!(!missing.contains_key("state"));
    let short = redirected_error(&site, &cookie, &[("state", Some("fifteen-chars-x"))]).await;
    assert_eq!((short["error"].as_str(), short["state"].as_str()), ("invalid_request", "fifteen-chars-x"));
    for changes in [
        [("install_id", None)],
        [("install_id", Some("short"))],
        [("install_id", Some("install-phone-0001!"))],
        [("platform", None)],
        [("platform", Some("symbian"))],
        [("platform", Some("iOS"))],
    ] {
        let refused = redirected_error(&site, &cookie, &changes).await;
        assert_eq!((refused["error"].as_str(), refused["state"].as_str()), ("invalid_request", STATE), "{changes:?}");
    }
    let pkce = redirected_error(&site, &cookie, &[("code_challenge_method", Some("plain"))]).await;
    assert_eq!(pkce["error"], "invalid_request");
    // 43 characters, but not all of base64url.
    let odd = format!("{}+", "A".repeat(42));
    let pkce = redirected_error(&site, &cookie, &[("code_challenge", Some(odd.as_str()))]).await;
    assert_eq!(pkce["error"], "invalid_request");
    // The decision checks the same.
    let response = decide(&site, &cookie, &[("platform", None)], "approve").await;
    assert_eq!(query_of(&location(&response))["error"], "invalid_request");
    assert_eq!(count(&site.db, "SELECT count(*) FROM oauth_codes").await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_scope_is_app_or_nothing(db: PgPool) {
    let site = Site::new(db);
    let (_, cookie) = site.person(ANA).await;
    for scope in ["mcp", "app mcp", "books", "app "] {
        let refused = redirected_error(&site, &cookie, &[("scope", Some(scope))]).await;
        assert_eq!(refused["error"], "invalid_scope", "{scope}");
    }
    let code = approve(&site, &cookie, &[("scope", Some("app"))]).await;
    let (status, tokens) = exchange(&site, CLIENT, &code, SCHEME, &verifier()).await;
    assert_eq!((status, tokens["scope"].as_str()), (200, Some("app")));
}

/// The value of the session cookie a response sets.
fn session_cookie(response: &Response) -> String {
    let set = response.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap();
    set.split(';').next().unwrap().to_owned()
}

/// Signs in on the web with a code, coming back to `next`: the cookie.
async fn web_sign_in(site: &Site, next: &str) -> String {
    let issued = buwiz_cloud::accounts::issue_code(&site.db, ANA, &site.state.config.admin_emails).await.unwrap();
    let buwiz_cloud::accounts::IssuedCode::Mail(code) = issued else { panic!("a code to mail: {issued:?}") };
    let form = form_urlencoded::Serializer::new(String::new())
        .extend_pairs([("email", ANA), ("code", code.as_str()), ("next", next)])
        .finish();
    let response = site.post("/auth/email/verify", "", &form).await;
    assert_eq!(location(&response), next);
    session_cookie(&response)
}

/// The `next` of a redirect to `/login`.
fn next_of(location: &str) -> String {
    assert!(location.starts_with("/login?"), "{location}");
    let query = location.split_once('?').unwrap().1;
    form_urlencoded::parse(query.as_bytes()).find(|(key, _)| key == "next").unwrap().1.into_owned()
}

#[sqlx::test(migrations = "./migrations")]
async fn without_a_session_it_goes_through_sign_in_and_back(db: PgPool) {
    let site = Site::new(db);
    site.person(ANA).await;
    let path = authorize_path(&[]);
    let response = site.get(&path, "").await;
    let next = next_of(&location(&response));
    assert_eq!(next, path);
    let cookie = web_sign_in(&site, &next).await;
    let page = text(site.get(&next, &cookie).await).await;
    assert!(page.contains("Continue as ana@example.test"), "{page}");
    let code = approve(&site, &cookie, &[]).await;
    assert_eq!(exchange(&site, CLIENT, &code, SCHEME, &verifier()).await.0, 200);
}

#[sqlx::test(migrations = "./migrations")]
async fn cancel_sends_access_denied_back(db: PgPool) {
    let site = Site::new(db);
    let (_, cookie) = site.person(ANA).await;
    let response = decide(&site, &cookie, &[("redirect_uri", Some(LOOPBACK))], "deny").await;
    let location = location(&response);
    assert!(location.starts_with("http://127.0.0.1:53682/callback?error=access_denied"), "{location}");
    let query = query_of(&location);
    assert_eq!((query["state"].as_str(), query["iss"].as_str()), (STATE, ORIGIN));
    assert!(!query.contains_key("code"));
    assert_eq!(count(&site.db, "SELECT count(*) FROM oauth_codes").await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn use_another_account_signs_the_web_out_and_comes_back(db: PgPool) {
    let site = Site::new(db);
    let (_, cookie) = site.person("bob@example.test").await;
    let path = authorize_path(&[]);
    let page = text(site.get(&path, &cookie).await).await;
    assert!(page.contains(r#"<form class="auth-switch" method="post" action="/logout">"#), "{page}");
    let form = form_urlencoded::Serializer::new(String::new()).append_pair("next", &path).finish();
    let response = site.post("/logout", &cookie, &form).await;
    let login = location(&response);
    assert!(login.contains("notice=signed_out"), "{login}");
    assert_eq!(next_of(&login), path);
    // The old session is over.
    assert!(location(&site.get(&path, &cookie).await).starts_with("/login?"));
    // Signing out as before goes to /login alone, and from another site not at all.
    let (_, other) = site.person("cy@example.test").await;
    assert_eq!(location(&site.post("/logout", &other, "").await), "/login?notice=signed_out");
    let cross = Request::post("/logout")
        .header(header::COOKIE, &cookie)
        .header(header::ORIGIN, "https://evil.example")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form))
        .unwrap();
    assert_eq!(site.send(cross).await.status(), StatusCode::FORBIDDEN);

    site.person(ANA).await;
    let cookie = web_sign_in(&site, &path).await;
    assert!(text(site.get(&path, &cookie).await).await.contains("Continue as ana@example.test"));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_banned_account_is_refused(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person(ANA).await;
    // A code approved before the ban is refused at the exchange.
    let code = approve(&site, &cookie, &[]).await;
    sqlx::query("UPDATE accounts SET banned_at = now(), banned_by = id, ban_reason = 'Test' WHERE id = $1")
        .bind(ana)
        .execute(&site.db)
        .await
        .unwrap();
    let (status, answer) = exchange(&site, CLIENT, &code, SCHEME, &verifier()).await;
    assert_eq!((status, answer["error"].as_str()), (400, Some("access_denied")), "{answer}");
    assert!(location(&site.get(&authorize_path(&[]), &cookie).await).starts_with("/login?"));
    let response = decide(&site, &cookie, &[], "approve").await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(count(&site.db, "SELECT count(*) FROM oauth_codes").await, 1);
    assert_eq!(count(&site.db, "SELECT count(*) FROM oauth_grants").await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_code_works_once_with_its_verifier_redirect_and_client(db: PgPool) {
    let site = Site::new(db);
    let (_, cookie) = site.person(ANA).await;
    let code = approve(&site, &cookie, &[]).await;
    assert_eq!(exchange(&site, CLIENT, &code, SCHEME, &verifier()).await.0, 200);
    let (status, answer) = exchange(&site, CLIENT, &code, SCHEME, &verifier()).await;
    assert_eq!((status, answer["error"].as_str()), (400, Some("invalid_grant")));

    let code = approve(&site, &cookie, &[]).await;
    assert_eq!(exchange(&site, CLIENT, &code, SCHEME, &"w".repeat(43)).await.1["error"], "invalid_grant");
    // A failed try uses the code up.
    assert_eq!(exchange(&site, CLIENT, &code, SCHEME, &verifier()).await.1["error"], "invalid_grant");

    let code = approve(&site, &cookie, &[("redirect_uri", Some(LOOPBACK))]).await;
    let other_port = "http://127.0.0.1:53683/callback";
    assert_eq!(exchange(&site, CLIENT, &code, other_port, &verifier()).await.1["error"], "invalid_grant");

    let code = approve(&site, &cookie, &[]).await;
    assert_eq!(exchange(&site, "buwiz-ebirforms", &code, SCHEME, &verifier()).await.1["error"], "invalid_grant");

    let code = approve(&site, &cookie, &[]).await;
    sqlx::query("UPDATE oauth_codes SET expires_at = now() - interval '1 second'").execute(&site.db).await.unwrap();
    assert_eq!(exchange(&site, CLIENT, &code, SCHEME, &verifier()).await.1["error"], "invalid_grant");
    assert_eq!(count(&site.db, "SELECT count(*) FROM oauth_grants").await, 1);
}

/// The exchange waits at most three seconds for a lock (here the account's,
/// held by another transaction), then answers `503
/// temporarily_unavailable` and leaves the code unused.
#[sqlx::test(migrations = "./migrations")]
async fn an_exchange_that_cannot_have_its_lock_in_time_is_busy(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person(ANA).await;
    let code = approve(&site, &cookie, &[]).await;
    let mut holder = site.db.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM accounts WHERE id = $1 FOR UPDATE").bind(ana).execute(&mut *holder).await.unwrap();
    let started = std::time::Instant::now();
    let (status, answer) = exchange(&site, CLIENT, &code, SCHEME, &verifier()).await;
    assert_eq!((status, answer["error"].as_str()), (503, Some("temporarily_unavailable")), "{answer}");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    holder.rollback().await.unwrap();
    let (status, answer) = exchange(&site, CLIENT, &code, SCHEME, &verifier()).await;
    assert_eq!(status, 200, "{answer}");
}

/// Two exchanges of one code at the same moment: exactly one gets tokens.
#[sqlx::test(migrations = "./migrations")]
async fn two_exchanges_of_one_code_at_once_give_one_sign_in(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person(ANA).await;
    let code = approve(&site, &cookie, &[]).await;
    let verifier = verifier();
    // Both wait in the database on the account's row, held here, so they
    // are in it at the same time; then it is let go.
    let mut holder = site.db.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM accounts WHERE id = $1 FOR UPDATE").bind(ana).execute(&mut *holder).await.unwrap();
    let release = async {
        until_waiting(&site.db, 2).await;
        holder.rollback().await.unwrap();
    };
    let (first, second, ()) = tokio::join!(
        exchange(&site, CLIENT, &code, SCHEME, &verifier),
        exchange(&site, CLIENT, &code, SCHEME, &verifier),
        release
    );
    let mut statuses = [first.0, second.0];
    statuses.sort_unstable();
    assert_eq!(statuses, [200, 400], "{} {}", first.1, second.1);
    let refused = if first.0 == 400 { &first.1 } else { &second.1 };
    assert_eq!(refused["error"], "invalid_grant");
    assert_eq!(count(&site.db, "SELECT count(*) FROM oauth_grants").await, 1);
}

/// Waits until `n` connections wait for a lock (at most five seconds).
async fn until_waiting(db: &PgPool, n: i64) {
    for _ in 0..100 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(db)
        .await
        .unwrap();
        if waiting >= n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("{n} connections never waited for a lock");
}

/// `/oauth/token` from the network of `address` (`CF-Connecting-IP`).
async fn token_from(site: &Site, address: &str, form: &[(&str, &str)]) -> (u16, Value) {
    let form: Vec<(String, String)> = form.iter().map(|(key, value)| ((*key).into(), (*value).into())).collect();
    let request = Request::post("/oauth/token")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("cf-connecting-ip", address)
        .body(Body::from(encode(&form)))
        .unwrap();
    let response = site.send(request).await;
    let status = response.status().as_u16();
    (status, serde_json::from_str(&text(response).await).unwrap())
}

const NETWORK_A: &str = "198.51.100.7";

/// A site that counts `n` failed tries at a token per network an hour
/// before it reports further ones (it never refuses them).
fn site_with_token_failures(db: PgPool, n: u32) -> Site {
    Site::with_config(db, |config| config.sign_in_limits.token_failures_per_network = n)
}

fn made_up(prefix: &str) -> String {
    crypto::new_token(prefix)
}

async fn refresh_from(site: &Site, address: &str, refresh_token: &str) -> (u16, Value) {
    token_from(
        site,
        address,
        &[("grant_type", "refresh_token"), ("client_id", CLIENT), ("refresh_token", refresh_token)],
    )
    .await
}

async fn poll_from(site: &Site, address: &str, device_code: &str) -> (u16, Value) {
    let body = json!({ "client_id": CLIENT, "device_code": device_code });
    let request = Request::post("/auth/device/poll")
        .header(header::CONTENT_TYPE, "application/json")
        .header("cf-connecting-ip", address)
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = site.send(request).await;
    let status = response.status().as_u16();
    (status, serde_json::from_str(&text(response).await).unwrap())
}

/// Counted and alerted, never refused: a network far over its count of
/// failed tries still has its valid refresh, exchange and poll served, as
/// on main. The tries past the count are in the hour's counts, and staff
/// are alerted.
#[sqlx::test(migrations = "./migrations")]
async fn a_network_far_over_its_failures_is_still_served(db: PgPool) {
    let site = Site::with_config(db, |config| {
        config.sign_in_limits.token_failures_per_network = 2;
        config.sign_in_limits.refusals_alert_at = 5;
    });
    let (_, cookie) = site.person(ANA).await;
    let tokens = sign_in(&site, &cookie, PHONE, "ios").await;
    let code = approve(&site, &cookie, &[]).await;
    let request = Request::post("/auth/device/start")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "client_id": CLIENT }).to_string()))
        .unwrap();
    let started: Value = serde_json::from_str(&text(site.send(request).await).await).unwrap();
    let pending = started["device_code"].as_str().unwrap();

    for _ in 0..10 {
        assert_eq!(refresh_from(&site, NETWORK_A, &made_up("bwr_")).await.0, 400);
        assert_eq!(exchange_from(&site, NETWORK_A, &made_up("bwc_")).await.0, 400);
        assert_eq!(poll_from(&site, NETWORK_A, &made_up("bwd_")).await.0, 400);
    }
    let counts = site.state.sign_in_guard.refusals_this_hour();
    assert_eq!(counts.tokens_by_network, 28, "every failure past the first two is counted");

    let (status, answer) = refresh_from(&site, NETWORK_A, tokens["refresh_token"].as_str().unwrap()).await;
    assert_eq!(status, 200, "{answer}");
    let (status, answer) = exchange_from(&site, NETWORK_A, &code).await;
    assert_eq!(status, 200, "{answer}");
    let (status, answer) = poll_from(&site, NETWORK_A, pending).await;
    assert_eq!((status, answer["error"].as_str()), (400, Some("authorization_pending")), "{answer}");

    // Staff were told, once.
    for _ in 0..100 {
        if !site.state.mailer.notices().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let notices = site.state.mailer.notices();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].subject, "Sign-in: many failed tries at a token counted this hour (none refused)");
    assert!(notices[0].text.contains("None was refused"), "{}", notices[0].text);
    assert!(!notices[0].text.contains("were refused"), "{}", notices[0].text);
}

/// Polls of a real device code that is still waiting are never counted,
/// however many: only a poll that fails with `invalid_grant` is.
#[sqlx::test(migrations = "./migrations")]
async fn polls_of_a_pending_device_code_are_never_counted(db: PgPool) {
    let site = site_with_token_failures(db, 2);
    let request = Request::post("/auth/device/start")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "client_id": CLIENT }).to_string()))
        .unwrap();
    let started: Value = serde_json::from_str(&text(site.send(request).await).await).unwrap();
    let pending = started["device_code"].as_str().unwrap();
    for _ in 0..20 {
        let (status, answer) = poll_from(&site, NETWORK_A, pending).await;
        assert_eq!(status, 400);
        assert!(matches!(answer["error"].as_str(), Some("authorization_pending" | "slow_down")), "{answer}");
    }
    for _ in 0..3 {
        assert_eq!(poll_from(&site, NETWORK_A, &made_up("bwd_")).await.0, 400);
    }
    // Two made-up codes fill the count; only the third is past it.
    assert_eq!(site.state.sign_in_guard.refusals_this_hour().tokens_by_network, 1);
}

/// A try that does not fail is given back: many valid refreshes from one
/// network leave its count of failures at nothing.
#[sqlx::test(migrations = "./migrations")]
async fn a_valid_refresh_is_never_counted(db: PgPool) {
    let site = site_with_token_failures(db, 3);
    let (_, cookie) = site.person(ANA).await;
    let mut refresh = sign_in(&site, &cookie, PHONE, "ios").await["refresh_token"].as_str().unwrap().to_owned();
    for _ in 0..10 {
        let (status, answer) = refresh_from(&site, NETWORK_A, &refresh).await;
        assert_eq!(status, 200, "{answer}");
        refresh = answer["refresh_token"].as_str().unwrap().to_owned();
    }
    for _ in 0..4 {
        assert_eq!(refresh_from(&site, NETWORK_A, &made_up("bwr_")).await.0, 400);
    }
    // Three failures fill the count; only the fourth is past it.
    assert_eq!(site.state.sign_in_guard.refusals_this_hour().tokens_by_network, 1);
}

/// A refresh token, device code or code of another shape than minted is
/// `invalid_grant` before the database: here with the pool closed.
#[sqlx::test(migrations = "./migrations")]
async fn tokens_of_another_shape_are_refused_without_the_database(db: PgPool) {
    let site = Site::new(db);
    site.db.close().await;
    for (status, answer) in [
        refresh_from(&site, NETWORK_A, "bwr_short").await,
        refresh_from(&site, NETWORK_A, &format!("bwr_{}=", "A".repeat(42))).await,
        poll_from(&site, NETWORK_A, "bwd_short").await,
        exchange_from(&site, NETWORK_A, "bwc_short").await,
    ] {
        assert_eq!((status, answer["error"].as_str()), (400, Some("invalid_grant")), "{answer}");
    }
}

async fn exchange_from(site: &Site, address: &str, code: &str) -> (u16, Value) {
    let verifier = verifier();
    token_from(
        site,
        address,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT),
            ("code", code),
            ("redirect_uri", SCHEME),
            ("code_verifier", &verifier),
        ],
    )
    .await
}

/// A code that is not `bwc_` and 43 characters of base64url is
/// `invalid_grant` before the database: here with every exchange place
/// taken and the pool closed.
#[sqlx::test(migrations = "./migrations")]
async fn a_code_of_the_wrong_shape_is_refused_without_the_database(db: PgPool) {
    let site = Site::new(db);
    let places = site.state.token_slots.acquire_many(buwiz_cloud::account::agents::TOKEN_SLOTS as u32).await.unwrap();
    site.db.close().await;
    let right_length = format!("bwc_{}", "A".repeat(43));
    for bad in [
        "bwc_short".to_owned(),
        format!("bwc_{}", "A".repeat(44)),
        format!("bwx_{}", "A".repeat(43)),
        format!("bwc_{}+", "A".repeat(42)),
        format!("bwc_{}=", "A".repeat(42)),
        String::new(),
    ] {
        let started = std::time::Instant::now();
        let (status, answer) = exchange(&site, CLIENT, &bad, SCHEME, &verifier()).await;
        assert_eq!((status, answer["error"].as_str()), (400, Some("invalid_grant")), "{bad}: {answer}");
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "{bad}");
    }
    // A code of the right shape waits for a place, then is busy.
    let started = std::time::Instant::now();
    let (status, answer) = exchange(&site, CLIENT, &right_length, SCHEME, &verifier()).await;
    assert_eq!((status, answer["error"].as_str()), (503, Some("temporarily_unavailable")), "{answer}");
    assert!(started.elapsed() >= std::time::Duration::from_millis(1900));
    drop(places);
}

/// An in-app request and its code, through the API host: (request id,
/// secret, code).
async fn in_app_code(site: &Site, email: &str) -> (String, String, String) {
    let secret = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let hash = URL_SAFE_NO_PAD.encode(Sha256::digest(secret.as_bytes()));
    let body = json!({ "client_id": CLIENT, "email": email, "intent": "sign_in", "secret_hash": hash });
    let (status, answer) = api_json(site, "/auth/email/start", &body).await;
    assert_eq!(status, 200, "{answer}");
    let code = site.state.mailer.app_sign_in_mails().iter().rev().find(|mail| mail.to == email).unwrap().code.clone();
    (answer["request_id"].as_str().unwrap().to_owned(), secret, code)
}

async fn api_json(site: &Site, path: &str, body: &Value) -> (u16, Value) {
    let request = Request::post(path)
        .header(header::HOST, "api.localhost:3008")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = site.send(request).await;
    let status = response.status().as_u16();
    (status, serde_json::from_str(&text(response).await).unwrap())
}

/// A flood of exchanges, many more at once than their places, each
/// holding its connection as long as it can (its account's row is locked,
/// so it waits the whole three seconds): only the exchange's own places
/// use connections, the rest wait without one and are busy, and in-app
/// verify meanwhile answers its tokens at once.
#[sqlx::test(migrations = "./migrations")]
async fn a_flood_of_code_exchanges_leaves_in_app_verify_working(db: PgPool) {
    let site = Site::with_mailbox(db);
    // Room for the holder, the exchange's places and the verify.
    assert!(site.db.options().get_max_connections() >= 5);
    let (request_id, secret, in_app) = in_app_code(&site, ANA).await;
    let (bea, _) = site.person("bea@example.test").await;
    let challenge = challenge(&verifier());
    let mut codes = Vec::new();
    for _ in 0..30 {
        let code = crypto::new_token("bwc_");
        sqlx::query(
            "INSERT INTO oauth_codes (code_hash, client_id, account_id, redirect_uri, code_challenge, scope, expires_at,
                                      install_id, platform)
             VALUES ($1, 'buwiz-desktop', $2, $3, $4, 'app', now() + interval '10 minutes', $5, 'ios')",
        )
        .bind(crypto::hash(&code))
        .bind(bea)
        .bind(SCHEME)
        .bind(&challenge)
        .bind(PHONE)
        .execute(&site.db)
        .await
        .unwrap();
        codes.push(code);
    }
    let mut holder = site.db.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM accounts WHERE id = $1 FOR UPDATE").bind(bea).execute(&mut *holder).await.unwrap();

    let verifier = verifier();
    let flood =
        futures_util::future::join_all(codes.iter().map(|code| exchange(&site, CLIENT, code, SCHEME, &verifier)));
    let in_app_verify = async {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let started = std::time::Instant::now();
        let body = json!({ "request_id": request_id, "secret": secret, "code": in_app });
        let answer = api_json(&site, "/auth/email/verify", &body).await;
        (answer, started.elapsed())
    };
    let (flood, ((status, tokens), took)) = tokio::join!(flood, in_app_verify);
    assert_eq!(status, 200, "{tokens}");
    assert!(tokens["access_token"].as_str().unwrap().starts_with("bwa_"));
    assert!(took < std::time::Duration::from_secs(1), "{took:?}");
    for (status, answer) in &flood {
        assert_eq!((*status, answer["error"].as_str()), (503, Some("temporarily_unavailable")), "{answer}");
    }
    holder.rollback().await.unwrap();
    // The codes were not used: each still works.
    assert_eq!(exchange(&site, CLIENT, &codes[0], SCHEME, &verifier).await.0, 200);
}

async fn register_agent(site: &Site, name: &str, redirect: &str) -> (u16, Value) {
    let body = json!({ "client_name": name, "redirect_uris": [redirect] });
    let request = Request::post("/oauth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = site.send(request).await;
    let status = response.status().as_u16();
    (status, serde_json::from_str(&text(response).await).unwrap())
}

/// An agent's parameters: its id and registered address, no install or
/// platform, then `extra`.
fn agent_changes<'a>(agent_id: &'a str, extra: &[(&'a str, Option<&'a str>)]) -> Vec<(&'a str, Option<&'a str>)> {
    let mut changes = vec![
        ("client_id", Some(agent_id)),
        ("redirect_uri", Some("https://agent.example.test/cb")),
        ("install_id", None),
        ("platform", None),
    ];
    changes.extend_from_slice(extra);
    changes
}

#[sqlx::test(migrations = "./migrations")]
async fn agents_keep_mcp_and_their_own_addresses(db: PgPool) {
    let site = Site::new(db);
    let (_, cookie) = site.person(ANA).await;
    let (status, agent) = register_agent(&site, "buwiz-desktop", "https://agent.example.test/cb").await;
    assert_eq!(status, 201);
    let agent_id = agent["client_id"].as_str().unwrap().to_owned();
    assert!(agent_id.starts_with("agent_") && buwiz_cloud::oauth::client_name(&agent_id).is_none(), "{agent_id}");
    // No agent may register the app's own scheme.
    for redirect in [SCHEME, "ebirforms://anything"] {
        let (status, refused) = register_agent(&site, "Sneaky", redirect).await;
        assert_eq!((status, refused["error"].as_str()), (400, Some("invalid_redirect_uri")), "{redirect}");
    }

    let agent_path = |extra: &[(&'static str, Option<&'static str>)]| agent_changes(&agent_id, extra);
    for redirect in [SCHEME, LOOPBACK] {
        untrusted(site.get(&authorize_path(&agent_path(&[("redirect_uri", Some(redirect))])), &cookie).await).await;
    }
    let response = site.get(&authorize_path(&agent_path(&[("scope", Some("app"))])), &cookie).await;
    assert!(location(&response).starts_with("https://agent.example.test/cb?error=invalid_scope"));
    // The challenge is 43 characters of base64url, for agents too.
    let response = site
        .get(
            &authorize_path(&agent_path(&[("code_challenge", Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA+"))])),
            &cookie,
        )
        .await;
    assert!(location(&response).starts_with("https://agent.example.test/cb?error=invalid_request"));
    let page = text(site.get(&authorize_path(&agent_path(&[])), &cookie).await).await;
    assert!(page.contains("Unverified app") && !page.contains("Continue as"), "{page}");

    // An agent's code gives mcp, even with app-like fields sent along.
    let mut form = params(&agent_path(&[]));
    form.extend([("install_id".into(), PHONE.into()), ("platform".into(), "ios".into())]);
    form.push(("decision".into(), "approve".into()));
    let response = site.post("/oauth/authorize/decision", &cookie, &encode(&form)).await;
    let code = query_of(&location(&response)).remove("code").unwrap();
    let (status, tokens) = exchange(&site, &agent_id, &code, "https://agent.example.test/cb", &verifier()).await;
    assert_eq!((status, tokens["scope"].as_str()), (200, Some("mcp")), "{tokens}");
    let grant: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT install_id, platform FROM oauth_grants").fetch_one(&site.db).await.unwrap();
    assert_eq!(grant, (None, None));

    // An agent registered before the rule, with the app's scheme, never
    // has it used.
    sqlx::query(
        "INSERT INTO oauth_clients (client_id, client_name, redirect_uris)
         VALUES ('agent_old', 'Old agent', ARRAY['ebirforms://signin-callback'])",
    )
    .execute(&site.db)
    .await
    .unwrap();
    let page = untrusted(
        site.get(&authorize_path(&agent_changes("agent_old", &[("redirect_uri", Some(SCHEME))])), &cookie).await,
    )
    .await;
    assert!(page.contains("may not use"), "{page}");
    untrusted(site.get(&authorize_path(&agent_changes("agent_old", &[("redirect_uri", None)])), &cookie).await).await;
}

#[sqlx::test(migrations = "./migrations")]
async fn only_buwiz_desktop_of_the_built_in_clients_may_authorize(db: PgPool) {
    let site = Site::new(db);
    let (_, cookie) = site.person(ANA).await;
    for client in ["buwiz-ebirforms", "buwiz-books"] {
        let page = untrusted(site.get(&authorize_path(&[("client_id", Some(client))]), &cookie).await).await;
        assert!(page.contains("This app cannot sign in here."), "{page}");
        assert!(page.contains("Go back to the app and sign in again."), "{page}");
    }
    // A row in oauth_clients under a built-in id changes nothing: built-in
    // clients use their fixed list only.
    sqlx::query(
        "INSERT INTO oauth_clients (client_id, client_name, redirect_uris)
         VALUES ('buwiz-books', 'Books', ARRAY['https://evil.example/cb']),
                ('buwiz-desktop', 'Desktop', ARRAY['https://evil.example/cb'])",
    )
    .execute(&site.db)
    .await
    .unwrap();
    for client in ["buwiz-books", "buwiz-desktop"] {
        let changes = [("client_id", Some(client)), ("redirect_uri", Some("https://evil.example/cb"))];
        untrusted(site.get(&authorize_path(&changes), &cookie).await).await;
    }
    assert_eq!(count(&site.db, "SELECT count(*) FROM oauth_codes").await, 0);
}

fn assert_no_frames(response: &Response, what: &str) {
    assert_eq!(response.headers()["x-frame-options"], "DENY", "{what}");
    let policy = response.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(policy.contains("frame-ancestors 'none'"), "{what}: {policy}");
}

#[sqlx::test(migrations = "./migrations")]
async fn the_pages_and_the_decision_never_show_in_a_frame(db: PgPool) {
    let site = Site::new(db);
    let (_, cookie) = site.person(ANA).await;
    assert_no_frames(&site.get(&authorize_path(&[]), &cookie).await, "the app's page");
    assert_no_frames(&site.get(&authorize_path(&[]), "").await, "the redirect to sign in");
    assert_no_frames(&decide(&site, &cookie, &[], "approve").await, "the approval");
    assert_no_frames(&decide(&site, &cookie, &[], "deny").await, "the cancel");
    assert_no_frames(&decide(&site, "", &[], "approve").await, "the decision without a session");
    let (_, agent) = register_agent(&site, "Agent", "https://agent.example.test/cb").await;
    let changes = [
        ("client_id", agent["client_id"].as_str()),
        ("redirect_uri", Some("https://agent.example.test/cb")),
        ("install_id", None),
        ("platform", None),
    ];
    assert_no_frames(&site.get(&authorize_path(&changes), &cookie).await, "the agents' page");
    // Other pages are as before: no policy of their own.
    assert!(!site.get("/tins", &cookie).await.headers().contains_key(header::CONTENT_SECURITY_POLICY));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_new_install_is_mailed_and_a_known_device_is_not(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (_, cookie) = site.person(ANA).await;
    let tokens = sign_in(&site, &cookie, PHONE, "ios").await;
    let mails = site.state.mailer.new_device_mails();
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0].to, ANA);
    assert_eq!(mails[0].subject, "New device signed in: eBIRForms on iPhone");
    assert!(mails[0].text.contains("signed in to eBIRForms on iPhone on "), "{}", mails[0].text);
    assert!(
        mails[0].text.contains(&format!("If this wasn't you, log it out at {ORIGIN}/tins#devices-h")),
        "{}",
        mails[0].text
    );
    for request_word in [PHONE, STATE, "ebirforms://"] {
        assert!(!mails[0].text.contains(request_word), "{}", mails[0].text);
    }

    // Once the install is a device of the account, signing in again from
    // it mails nothing.
    assert_eq!(register(&site, tokens["access_token"].as_str().unwrap(), PHONE).await.0, 201);
    sign_in(&site, &cookie, PHONE, "ios").await;
    assert_eq!(site.state.mailer.new_device_mails().len(), 1);

    // Another install is new.
    sign_in(&site, &cookie, LAPTOP, "macos").await;
    let mails = site.state.mailer.new_device_mails();
    assert_eq!(mails.len(), 2);
    assert_eq!(mails[1].subject, "New device signed in: eBIRForms on Mac");
}

/// Whether the device is new is decided in the exchange, not on the mail's
/// task: an app that registers its device before the task runs still has
/// its new device mailed.
#[sqlx::test(migrations = "./migrations")]
async fn a_device_registered_before_the_mail_task_runs_is_still_mailed(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (_, cookie) = site.person(ANA).await;
    site.state.mailer.hold_new_device_mails(true);
    let code = approve(&site, &cookie, &[]).await;
    let (status, tokens) = exchange(&site, CLIENT, &code, SCHEME, &verifier()).await;
    assert_eq!(status, 200, "{tokens}");
    assert_eq!(register(&site, tokens["access_token"].as_str().unwrap(), PHONE).await.0, 201);
    assert_eq!(count(&site.db, "SELECT count(*) FROM devices").await, 1);
    assert_eq!(site.state.mailer.new_device_checks(), 0, "the task is still held");
    site.state.mailer.hold_new_device_mails(false);
    settled(&site, 1).await;
    let mails = site.state.mailer.new_device_mails();
    assert_eq!(mails.len(), 1);
    assert_eq!((mails[0].to.as_str(), mails[0].subject.as_str()), (ANA, "New device signed in: eBIRForms on iPhone"));
}

/// A device the owner removed (My devices) that signs in again is a new
/// device: it is mailed again.
#[sqlx::test(migrations = "./migrations")]
async fn a_removed_device_that_signs_in_again_is_mailed(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, cookie) = site.person(ANA).await;
    let tokens = sign_in(&site, &cookie, PHONE, "ios").await;
    let (status, device) = register(&site, tokens["access_token"].as_str().unwrap(), PHONE).await;
    assert_eq!(status, 201, "{device}");
    let id: Uuid = device["id"].as_str().unwrap().parse().unwrap();
    assert!(buwiz_cloud::devices::revoke(&site.db, ana, id).await.unwrap());
    assert_eq!(removed_by(&site.db, id).await.as_deref(), Some("owner"));
    assert_eq!(site.state.mailer.new_device_mails().len(), 1);

    sign_in(&site, &cookie, PHONE, "ios").await;
    let mails = site.state.mailer.new_device_mails();
    assert_eq!(mails.len(), 2);
    assert_eq!(mails[1].subject, "New device signed in: eBIRForms on iPhone");

    // Registering again brings the row back: no longer removed.
    let tokens = sign_in(&site, &cookie, PHONE, "ios").await;
    assert_eq!(register(&site, tokens["access_token"].as_str().unwrap(), PHONE).await.0, 201);
    assert_eq!(removed_by(&site.db, id).await, None);
}

async fn removed_by(db: &PgPool, device: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT removed_by FROM devices WHERE id = $1").bind(device).fetch_one(db).await.unwrap()
}

/// A device staff removed that signs in again is mailed as a new device.
#[sqlx::test(migrations = "./migrations")]
async fn a_device_staff_removed_that_signs_in_again_is_mailed(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, cookie) = site.person(ANA).await;
    let (_, staff) = site.staff_with_role("boss@example.test", "admin").await;
    let tokens = sign_in(&site, &cookie, PHONE, "ios").await;
    let (status, device) = register(&site, tokens["access_token"].as_str().unwrap(), PHONE).await;
    assert_eq!(status, 201, "{device}");
    let id: Uuid = device["id"].as_str().unwrap().parse().unwrap();
    let response = site.post(&format!("/staff/users/{ana}/devices/{id}/sign-out"), &staff, "reason=Lost+phone").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(removed_by(&site.db, id).await.as_deref(), Some("staff"));

    sign_in(&site, &cookie, PHONE, "ios").await;
    assert_eq!(site.state.mailer.new_device_mails().len(), 2);
}

/// A device whose sign-in ended (here the person signed it out) and that
/// signs in again is the same device: no mail.
#[sqlx::test(migrations = "./migrations")]
async fn a_device_that_signed_out_and_signs_in_again_is_not_mailed(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, cookie) = site.person(ANA).await;
    let tokens = sign_in(&site, &cookie, PHONE, "ios").await;
    let (status, device) = register(&site, tokens["access_token"].as_str().unwrap(), PHONE).await;
    assert_eq!(status, 201, "{device}");
    let id: Uuid = device["id"].as_str().unwrap().parse().unwrap();
    let grant: Uuid =
        sqlx::query_scalar("SELECT grant_id FROM devices WHERE id = $1").bind(id).fetch_one(&site.db).await.unwrap();
    assert!(buwiz_cloud::devices::sign_out_grant(&site.db, ana, grant).await.unwrap());
    let (revoked, by): (bool, Option<String>) =
        sqlx::query_as("SELECT revoked_at IS NOT NULL, removed_by FROM devices WHERE id = $1")
            .bind(id)
            .fetch_one(&site.db)
            .await
            .unwrap();
    assert_eq!((revoked, by), (true, None));

    sign_in(&site, &cookie, PHONE, "ios").await;
    assert_eq!(site.state.mailer.new_device_mails().len(), 1);
}

/// At most three new-device mails an account in a day: past that, the
/// sign-in stands and nothing is mailed. A day later it is mailed again.
#[sqlx::test(migrations = "./migrations")]
async fn the_new_device_mail_is_limited_to_three_a_day(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (_, cookie) = site.person(ANA).await;
    for n in 1..=5 {
        let tokens = sign_in(&site, &cookie, &format!("install-number-{n:04}"), "ios").await;
        assert!(tokens["access_token"].as_str().unwrap().starts_with("bwa_"));
    }
    let mails = site.state.mailer.new_device_mails();
    assert_eq!(mails.len(), 3);
    // The third says it is the third of at most three in 24 hours, and asks
    // them to check.
    for mail in &mails[..2] {
        assert!(!mail.text.contains("this is the third"), "{}", mail.text);
    }
    assert!(
        mails[2].text.contains("We send at most three new-device notices in 24 hours, and this is the third."),
        "{}",
        mails[2].text
    );
    assert!(!mails[2].text.contains("today"), "{}", mails[2].text);
    assert!(mails[2].text.contains(&format!("Check My devices at {ORIGIN}/tins#devices-h")), "{}", mails[2].text);
    assert_eq!(count(&site.db, "SELECT count(*) FROM new_device_mails").await, 3);
    assert_eq!(count(&site.db, "SELECT count(*) FROM oauth_grants WHERE revoked_at IS NULL").await, 5);

    sqlx::query("UPDATE new_device_mails SET sent_at = sent_at - interval '1 day'").execute(&site.db).await.unwrap();
    sign_in(&site, &cookie, "install-number-0006", "ios").await;
    assert_eq!(site.state.mailer.new_device_mails().len(), 4);
}

/// My devices says "Signed out" for a device whose sign-in ended and
/// "Removed" for one the owner removed; a signed-out device can be removed
/// too, and is then mailed when it comes back.
#[sqlx::test(migrations = "./migrations")]
async fn my_devices_tells_signed_out_from_removed(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, cookie) = site.person(ANA).await;
    let tokens = sign_in(&site, &cookie, PHONE, "ios").await;
    let (status, device) = register(&site, tokens["access_token"].as_str().unwrap(), PHONE).await;
    assert_eq!(status, 201, "{device}");
    let id: Uuid = device["id"].as_str().unwrap().parse().unwrap();
    let grant: Uuid =
        sqlx::query_scalar("SELECT grant_id FROM devices WHERE id = $1").bind(id).fetch_one(&site.db).await.unwrap();
    assert!(buwiz_cloud::devices::sign_out_grant(&site.db, ana, grant).await.unwrap());
    let page = text(site.get("/tins", &cookie).await).await;
    assert!(page.contains(">Signed out<") && !page.contains(">Removed<"), "{page}");
    assert!(page.contains(&format!("/devices/{id}/revoke")), "a signed-out device can be removed");

    let response = site.post(&format!("/devices/{id}/revoke"), &cookie, "").await;
    assert!(location(&response).contains("notice="), "{}", location(&response));
    assert_eq!(removed_by(&site.db, id).await.as_deref(), Some("owner"));
    let page = text(site.get("/tins", &cookie).await).await;
    assert!(page.contains(">Removed<") && !page.contains(">Signed out<"), "{page}");
    // Removing it again changes nothing.
    assert!(!buwiz_cloud::devices::revoke(&site.db, ana, id).await.unwrap());

    sign_in(&site, &cookie, PHONE, "ios").await;
    assert_eq!(site.state.mailer.new_device_mails().len(), 2);
}

/// The decision waits at most three seconds for a lock, then is `503`,
/// and no code is made.
#[sqlx::test(migrations = "./migrations")]
async fn a_decision_that_cannot_have_its_lock_in_time_is_busy(db: PgPool) {
    let site = Site::new(db);
    let (ana, cookie) = site.person(ANA).await;
    let mut holder = site.db.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM accounts WHERE id = $1 FOR UPDATE").bind(ana).execute(&mut *holder).await.unwrap();
    let started = std::time::Instant::now();
    let response = decide(&site, &cookie, &[], "approve").await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::RETRY_AFTER], "5");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    holder.rollback().await.unwrap();
    assert_eq!(count(&site.db, "SELECT count(*) FROM oauth_codes").await, 0);
    assert!(location(&decide(&site, &cookie, &[], "approve").await).contains("code="));
}

/// The device flow waits at most three seconds for the lock the day's
/// new-device mails are counted under (held here); then the poll is `503`,
/// nothing is written, and the next poll signs in.
#[sqlx::test(migrations = "./migrations")]
async fn the_device_flows_mail_lock_is_waited_for_three_seconds_at_most(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, cookie) = site.person(ANA).await;
    let request = Request::post("/auth/device/start")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "client_id": CLIENT, "install_id": LAPTOP }).to_string()))
        .unwrap();
    let started: Value = serde_json::from_str(&text(site.send(request).await).await).unwrap();
    let user_code = started["user_code"].as_str().unwrap();
    let response = site.post("/device/decision", &cookie, &format!("user_code={user_code}&decision=approve")).await;
    assert_eq!(location(&response), "/device?notice=device_approved");
    let device_code = started["device_code"].as_str().unwrap();

    let mut holder = site.db.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1, hashtext($2::text))")
        .bind(0x6277_6e64_i32)
        .bind(ana)
        .execute(&mut *holder)
        .await
        .unwrap();
    let started_at = std::time::Instant::now();
    let (status, answer) = poll_from(&site, NETWORK_A, device_code).await;
    assert_eq!((status, answer["error"].as_str()), (503, Some("temporarily_unavailable")), "{answer}");
    let waited = started_at.elapsed();
    assert!(
        waited >= std::time::Duration::from_millis(2900) && waited < std::time::Duration::from_secs(6),
        "{waited:?}: three seconds for the lock, and a little more for the rest"
    );
    assert_eq!(count(&site.db, "SELECT count(*) FROM oauth_grants").await, 0);
    sqlx::query("SELECT pg_advisory_unlock_all()").execute(&mut *holder).await.unwrap();
    drop(holder);

    let (status, tokens) = poll_from(&site, NETWORK_A, device_code).await;
    assert_eq!(status, 200, "{tokens}");
    settled(&site, 1).await;
    assert_eq!(site.state.mailer.new_device_mails().len(), 1);
}

async fn device_flow(site: &Site, cookie: &str, install: Option<&str>) -> Value {
    let mut body = json!({ "client_id": CLIENT });
    if let Some(install) = install {
        body["install_id"] = json!(install);
    }
    let request = Request::post("/auth/device/start")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let started: Value = serde_json::from_str(&text(site.send(request).await).await).unwrap();
    let user_code = started["user_code"].as_str().unwrap();
    let response = site.post("/device/decision", cookie, &format!("user_code={user_code}&decision=approve")).await;
    assert_eq!(location(&response), "/device?notice=device_approved");
    let poll = json!({ "client_id": CLIENT, "device_code": started["device_code"] });
    let request = Request::post("/auth/device/poll")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(poll.to_string()))
        .unwrap();
    let response = site.send(request).await;
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_str(&text(response).await).unwrap()
}

/// The device flow is approved in a browser too, so a sign-in from an
/// install new to the account is mailed as Sign in with Buwiz's is, by
/// the same rule (a removed device is new, a signed-out one is not). One
/// without an install id (apps from before them) cannot be told apart and
/// is not mailed.
#[sqlx::test(migrations = "./migrations")]
async fn the_device_flow_mails_a_new_install_too(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, cookie) = site.person(ANA).await;
    device_flow(&site, &cookie, None).await;
    let tokens = device_flow(&site, &cookie, Some(LAPTOP)).await;
    settled(&site, 1).await;
    let mails = site.state.mailer.new_device_mails();
    assert_eq!(mails.len(), 1, "{mails:?}");
    assert_eq!((mails[0].to.as_str(), mails[0].subject.as_str()), (ANA, "New device signed in: eBIRForms"));

    // A known device signing in again by the device flow mails nothing.
    let (status, device) = register(&site, tokens["access_token"].as_str().unwrap(), LAPTOP).await;
    assert_eq!(status, 201, "{device}");
    let id: Uuid = device["id"].as_str().unwrap().parse().unwrap();
    let tokens = device_flow(&site, &cookie, Some(LAPTOP)).await;
    settled(&site, 2).await;
    assert_eq!(site.state.mailer.new_device_mails().len(), 1);

    // Signed out, then in again: the same device, no mail.
    assert_eq!(register(&site, tokens["access_token"].as_str().unwrap(), LAPTOP).await.0, 201);
    let grant: Uuid =
        sqlx::query_scalar("SELECT grant_id FROM devices WHERE id = $1").bind(id).fetch_one(&site.db).await.unwrap();
    assert!(buwiz_cloud::devices::sign_out_grant(&site.db, ana, grant).await.unwrap());
    let tokens = device_flow(&site, &cookie, Some(LAPTOP)).await;
    settled(&site, 3).await;
    assert_eq!(site.state.mailer.new_device_mails().len(), 1);

    // Removed by the owner, then in again: mailed.
    assert_eq!(register(&site, tokens["access_token"].as_str().unwrap(), LAPTOP).await.0, 201);
    assert!(buwiz_cloud::devices::revoke(&site.db, ana, id).await.unwrap());
    device_flow(&site, &cookie, Some(LAPTOP)).await;
    settled(&site, 4).await;
    assert_eq!(site.state.mailer.new_device_mails().len(), 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_fixed_code_account_is_not_mailed(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (boss, _) = site.staff_with_role("boss@example.test", "admin").await;
    let (reviewer, cookie) = site.person("reviewer@example.test").await;
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
    let tokens = sign_in(&site, &cookie, PHONE, "android").await;
    assert_eq!(tokens["scope"], "app");
    assert!(site.state.mailer.new_device_mails().is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn a_mail_that_fails_does_not_fail_the_sign_in(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (_, cookie) = site.person(ANA).await;
    site.state.mailer.fail_new_device_mails(true);
    let tokens = sign_in(&site, &cookie, PHONE, "windows").await;
    assert!(tokens["access_token"].as_str().unwrap().starts_with("bwa_"));
    assert_eq!(count(&site.db, "SELECT count(*) FROM oauth_grants WHERE revoked_at IS NULL").await, 1);
    assert!(site.state.mailer.new_device_mails().is_empty());
}
