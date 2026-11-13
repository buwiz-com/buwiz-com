//! The send gate: tickets, both submission calls, the relay and the licence
//! list refuse a TIN that is not active on the caller's account, each with
//! the code and words for where the caller's own TIN stands. Someone
//! else's TIN, no TIN and a TIN that ended all read the same.
//! Each test gets its own database from `#[sqlx::test]`.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use buwiz_cloud::filing::submissions::{self, RelayStart};
use buwiz_cloud::relay::Control;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use common::*;

const UNVERIFIED: &str =
    "This TIN is not verified yet. Verify it with its papers on My TINs at account.buwiz.com to send returns.";
const PENDING: &str =
    "This TIN's papers are still being checked, usually within one business day. You can send once it is active.";
const NEEDS_FIX: &str =
    "Sent back for a fix: A photo is too blurry to read. The name you typed doesn't match the document.";
const NEEDS_FIX_NO_REASON: &str = "Sent back for a fix. See what to change on My TINs at account.buwiz.com.";
const PAUSED: &str = "Sending is paused for this TIN. Write to support@buwiz.com.";
const NOT_ACTIVE: &str = "this TIN is not active on your account";
const BANNED: &str = "This Buwiz account is banned: it can't send returns. Write to support@buwiz.com.";
/// What a staff member typed when sending a claim back. It is for the
/// person on My TINs and in the app's own screens, never in a refusal an
/// assistant could read.
const STAFF_NOTE: &str = "Your middle name is spelled differently on the card.";

/// Each state a TIN can be in for ana, by its 9-digit root, and what a send
/// for it answers: (root, status, code, message).
const MATRIX: &[(&str, u16, &str, &str)] = &[
    ("000111001", 201, "", ""),
    ("000111002", 403, "tin_paused", PAUSED),
    ("000111003", 403, "tin_unverified", UNVERIFIED),
    ("000111004", 403, "tin_pending", PENDING),
    ("000111005", 403, "tin_needs_fix", NEEDS_FIX),
    ("000111006", 403, "tin_needs_fix", NEEDS_FIX_NO_REASON),
    // Rejected, withdrawn, removed, revoked, never added, someone else's:
    // all the same answer.
    ("000111007", 403, "tin_not_active", NOT_ACTIVE),
    ("000111008", 403, "tin_not_active", NOT_ACTIVE),
    ("000111009", 403, "tin_not_active", NOT_ACTIVE),
    ("000111010", 403, "tin_not_active", NOT_ACTIVE),
    ("000111011", 403, "tin_not_active", NOT_ACTIVE),
    ("000111012", 403, "tin_not_active", NOT_ACTIVE),
    // Someone else owns it and ana added it without papers: ana hears about
    // her own row only.
    ("000111013", 403, "tin_unverified", UNVERIFIED),
];

/// The roots in [`MATRIX`] that must answer exactly alike.
const NOT_ACTIVE_ROOTS: &[&str] = &["000111007", "000111008", "000111009", "000111010", "000111011", "000111012"];

async fn claim(db: &PgPool, account: Uuid, root: &str, status: &str) -> Uuid {
    if status == "unverified" {
        return sqlx::query_scalar(
            "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status)
             VALUES ($1, 'personal', $2, '00000', 'SYNTHETIC TAXPAYER', 'unverified') RETURNING id",
        )
        .bind(account)
        .bind(root)
        .fetch_one(db)
        .await
        .unwrap();
    }
    add_claim(db, account, root, status).await
}

async fn sql(db: &PgPool, query: &str, id: Uuid) {
    sqlx::query(query).bind(id).execute(db).await.unwrap();
}

async fn pause(db: &PgPool, claim: Uuid) {
    sql(db, "UPDATE tin_claims SET paused_at = now(), pause_reason = 'Checking a complaint.' WHERE id = $1", claim)
        .await;
}

async fn resume(db: &PgPool, claim: Uuid) {
    sql(db, "UPDATE tin_claims SET paused_at = NULL, pause_reason = NULL WHERE id = $1", claim).await;
}

/// Ana's TINs in every state of [`MATRIX`], and bob, who owns 000111012
/// and 000111013.
async fn matrix_accounts(site: &Site) -> (Uuid, Uuid) {
    let (ana, _) = site.person("ana@example.test").await;
    let (bob, _) = site.person("bob@example.test").await;
    claim(&site.db, ana, "000111001", "approved").await;
    let paused = claim(&site.db, ana, "000111002", "approved").await;
    pause(&site.db, paused).await;
    claim(&site.db, ana, "000111003", "unverified").await;
    claim(&site.db, ana, "000111004", "pending").await;
    let fix = claim(&site.db, ana, "000111005", "needs_fix").await;
    sqlx::query("UPDATE tin_claims SET fix_reasons = ARRAY['blurry', 'name_mismatch'], fix_note = $2 WHERE id = $1")
        .bind(fix)
        .bind(STAFF_NOTE)
        .execute(&site.db)
        .await
        .unwrap();
    let fix = claim(&site.db, ana, "000111006", "needs_fix").await;
    sqlx::query("UPDATE tin_claims SET fix_note = $2 WHERE id = $1")
        .bind(fix)
        .bind(STAFF_NOTE)
        .execute(&site.db)
        .await
        .unwrap();
    claim(&site.db, ana, "000111007", "rejected").await;
    claim(&site.db, ana, "000111008", "withdrawn").await;
    claim(&site.db, ana, "000111009", "removed").await;
    claim(&site.db, ana, "000111010", "revoked").await;
    claim(&site.db, bob, "000111012", "approved").await;
    claim(&site.db, bob, "000111013", "approved").await;
    claim(&site.db, ana, "000111013", "unverified").await;
    (ana, bob)
}

fn ticket_body(root: &str, key: &str) -> Value {
    json!({
        "tin": format!("{root}00000"),
        "form": "1701Qv2018",
        "period": "2026Q1",
        "payload_sha256": "ab".repeat(32),
        "email": "ana@example.test",
        "idempotency_key": key,
    })
}

fn v1_body(root: &str) -> Value {
    json!({ "filename": format!("{root}00000-1701Qv2018-2026Q1#ana@example.test#.xml"), "xml": "<return/>" })
}

async fn post_v1(site: &Site, token: &str, body: &Value) -> axum::response::Response {
    let request = Request::post("/api/submissions")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    site.send(request).await
}

async fn count(db: &PgPool, query: &str) -> i64 {
    sqlx::query_scalar(query).fetch_one(db).await.unwrap()
}

/// Checks one path's answers against [`MATRIX`]: each code and message, no
/// staff note, and the not-active answers byte for byte alike.
fn check_matrix(path: &str, answers: &[(&str, u16, String, String, Vec<u8>)]) {
    for ((root, status, code, message), (answered_root, got_status, got_code, got_message, _)) in
        MATRIX.iter().zip(answers)
    {
        assert_eq!(root, answered_root);
        if *status == 201 {
            assert_eq!(*got_status, 201, "{path} {root}: {got_code} {got_message}");
            continue;
        }
        assert_eq!((*got_status, got_code.as_str(), got_message.as_str()), (*status, *code, *message), "{path} {root}");
        assert!(!got_message.contains(STAFF_NOTE) && !got_message.contains("tin_not_active"), "{path} {root}");
    }
    let alike: Vec<&Vec<u8>> =
        answers.iter().filter(|(root, ..)| NOT_ACTIVE_ROOTS.contains(root)).map(|(.., body)| body).collect();
    assert_eq!(alike.len(), NOT_ACTIVE_ROOTS.len());
    assert!(alike.windows(2).all(|pair| pair[0] == pair[1]), "{path}: not-active answers differ");
}

/// Ticket and v1 answers, both read back, for one root.
async fn answer(
    response: axum::response::Response,
    root: &'static str,
) -> (&'static str, u16, String, String, Vec<u8>) {
    let status = response.status().as_u16();
    let body = bytes(response).await;
    let value: Value = serde_json::from_slice(&body).unwrap_or_default();
    let code = value["error"]["code"].as_str().unwrap_or_default().to_owned();
    let message = value["error"]["message"].as_str().unwrap_or_default().to_owned();
    (root, status, code, message, body)
}

/// Tickets are the only way to send a v2 return, and test sends (dry runs)
/// are v2 returns: a TIN refused here can neither send nor test-send.
#[sqlx::test(migrations = "./migrations")]
async fn tickets_name_the_callers_own_state(db: PgPool) {
    let site = Site::new(db);
    let (ana, _) = matrix_accounts(&site).await;
    let device = TestDevice::new(&site.db, ana).await;
    let mut answers = Vec::new();
    for (index, (root, ..)) in MATRIX.iter().enumerate() {
        let response = device.post(&site, "/api/tickets", &ticket_body(root, &format!("k{index}"))).await;
        answers.push(answer(response, root).await);
    }
    check_matrix("tickets", &answers);
    // Only the active TIN got a ticket.
    assert_eq!(count(&site.db, "SELECT count(*) FROM tickets").await, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn v1_refusals_name_the_callers_own_state(db: PgPool) {
    let site = Site::new(db);
    let (ana, _) = matrix_accounts(&site).await;
    let token = app_token(&site.db, ana).await;
    let mut answers = Vec::new();
    for (root, ..) in MATRIX {
        answers.push(answer(post_v1(&site, &token, &v1_body(root)).await, root).await);
    }
    check_matrix("v1", &answers);
    assert_eq!(count(&site.db, "SELECT count(*) FROM submissions").await, 1);
}

/// A ticket issued while the TIN was active is not spent when the TIN
/// stopped being active before the sealed return came: the app can try
/// again once it is.
#[sqlx::test(migrations = "./migrations")]
async fn a_ticket_issued_before_a_pause_or_an_ending_cannot_be_spent(db: PgPool) {
    let site = Site::new(db);
    let (ana, _) = site.person("ana@example.test").await;
    let device = TestDevice::new(&site.db, ana).await;
    let envelope = json!({ "kid": "k", "enc": "e", "ct": "c" });
    let ticket = |key: &'static str, root: &'static str| {
        let device = &device;
        let site = &site;
        async move {
            let response = device.post(site, "/api/tickets", &ticket_body(root, key)).await;
            assert_eq!(response.status(), StatusCode::CREATED);
            let body: Value = serde_json::from_str(&text(response).await).unwrap();
            (body["ticket"].as_str().unwrap().to_owned(), body["jti"].as_str().unwrap().parse::<Uuid>().unwrap())
        }
    };
    let used = |jti: Uuid| {
        let db = site.db.clone();
        async move {
            sqlx::query_scalar::<_, bool>("SELECT used_at IS NOT NULL FROM tickets WHERE jti = $1")
                .bind(jti)
                .fetch_one(&db)
                .await
                .unwrap()
        }
    };

    let paused = claim(&site.db, ana, "000222001", "approved").await;
    let (sealed, jti) = ticket("p1", "000222001").await;
    pause(&site.db, paused).await;
    let refused =
        refusal(device.post(&site, "/api/v2/submissions", &json!({ "ticket": sealed, "envelope": envelope })).await)
            .await;
    assert_eq!(refused, (403, "tin_paused".to_owned(), PAUSED.to_owned()));
    assert!(!used(jti).await);
    resume(&site.db, paused).await;
    let response = device.post(&site, "/api/v2/submissions", &json!({ "ticket": sealed, "envelope": envelope })).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert!(used(jti).await);

    for (status, root) in [("removed", "000222002"), ("revoked", "000222003")] {
        let ended = claim(&site.db, ana, root, "approved").await;
        let (sealed, jti) = ticket(if status == "removed" { "r1" } else { "r2" }, root).await;
        sql(&site.db, &format!("UPDATE tin_claims SET status = '{status}', removed_at = now() WHERE id = $1"), ended)
            .await;
        let refused = refusal(
            device.post(&site, "/api/v2/submissions", &json!({ "ticket": sealed, "envelope": envelope })).await,
        )
        .await;
        assert_eq!(refused, (403, "tin_not_active".to_owned(), NOT_ACTIVE.to_owned()), "{status}");
        assert!(!used(jti).await, "{status}");
    }
    assert_eq!(count(&site.db, "SELECT count(*) FROM submissions").await, 1);
}

async fn submission(db: &PgPool, account: Uuid, claim: Uuid, status: &str, v2: bool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO submissions (account_id, claim_id, form_type, period, filename, xml_sha256, status,
             xml_nonce, xml_ciphertext, envelope)
         VALUES ($1, $2, '1701Qv2018', '2026Q1', 'f.xml', decode('00', 'hex'), $3,
             CASE WHEN $4 THEN NULL ELSE decode('01', 'hex') END, CASE WHEN $4 THEN NULL ELSE decode('02', 'hex') END,
             CASE WHEN $4 THEN '{\"kid\":\"k\",\"enc\":\"e\",\"ct\":\"c\"}' END)
         RETURNING id",
    )
    .bind(account)
    .bind(claim)
    .bind(status)
    .bind(v2)
    .fetch_one(db)
    .await
    .unwrap()
}

/// (status, error, whether the XML or envelope is still kept)
async fn stored(db: &PgPool, id: Uuid) -> (String, Option<String>, bool) {
    sqlx::query_as(
        "SELECT status, error, (xml_ciphertext IS NOT NULL OR envelope IS NOT NULL) FROM submissions WHERE id = $1",
    )
    .bind(id)
    .fetch_one(db)
    .await
    .unwrap()
}

fn answered(start: RelayStart) -> (String, Option<String>) {
    match start {
        RelayStart::Answer(Control::Result { status, error, receipt: None }) => (status, error),
        RelayStart::Answer(other) => panic!("not a result: {other:?}"),
        RelayStart::Send(_) => panic!("the relay would have sent it"),
    }
}

/// A return waiting to go out when its TIN stops being active is failed
/// with the reason, so the app takes it back to a draft; nothing reached
/// BIR. A send already under way on another connection is left alone.
#[sqlx::test(migrations = "./migrations")]
async fn a_waiting_return_is_failed_when_its_tin_stops_being_active(db: PgPool) {
    let site = Site::new(db);
    let db = &site.db;
    let (ana, _) = site.person("ana@example.test").await;
    let active = claim(db, ana, "000333001", "approved").await;
    let paused = claim(db, ana, "000333002", "approved").await;
    let revoked = claim(db, ana, "000333003", "approved").await;
    let removed = claim(db, ana, "000333004", "approved").await;

    let v1 = submission(db, ana, active, "ready", false).await;
    assert!(matches!(submissions::claim_for_relay(db, ana, v1).await.unwrap(), RelayStart::Send(_)));
    assert_eq!(stored(db, v1).await.0, "sending");

    let v2 = submission(db, ana, paused, "ready", true).await;
    let v1_retry = submission(db, ana, paused, "failed", false).await;
    let elsewhere = submission(db, ana, paused, "sending", false).await;
    let v2_revoked = submission(db, ana, revoked, "ready", true).await;
    let v1_removed = submission(db, ana, removed, "ready", false).await;
    pause(db, paused).await;
    sql(db, "UPDATE tin_claims SET status = 'revoked', removed_at = now() WHERE id = $1", revoked).await;
    sql(db, "UPDATE tin_claims SET status = 'removed', removed_at = now() WHERE id = $1", removed).await;

    let failed_paused = format!("{PAUSED} (tin_paused)");
    let failed_not_active = format!("{NOT_ACTIVE} (tin_not_active)");
    for (id, error, kept) in [
        (v2, &failed_paused, false),
        (v1_retry, &failed_paused, true),
        (v2_revoked, &failed_not_active, false),
        (v1_removed, &failed_not_active, true),
    ] {
        let start = submissions::claim_for_relay(db, ana, id).await.unwrap();
        assert_eq!(answered(start), ("failed".to_owned(), Some(error.clone())));
        // A v2 envelope goes with its ticket; a v1 return may be sent again
        // once the TIN is active.
        assert_eq!(stored(db, id).await, ("failed".to_owned(), Some(error.clone()), kept));
    }

    let (status, error) = answered(submissions::claim_for_relay(db, ana, elsewhere).await.unwrap());
    assert_eq!(status, "sending");
    assert_eq!(error.as_deref(), Some("this submission cannot be sent now (status sending)"));
    assert_eq!(stored(db, elsewhere).await.0, "sending");

    // Resumed, the v1 return that failed goes out.
    resume(db, paused).await;
    assert!(matches!(submissions::claim_for_relay(db, ana, v1_retry).await.unwrap(), RelayStart::Send(_)));
}

/// The licence list leaves out a paused TIN, as the apps and agents read
/// it, and keeps every other answer.
#[sqlx::test(migrations = "./migrations")]
async fn the_licence_list_leaves_out_a_paused_tin(db: PgPool) {
    let site = Site::new(db);
    let (ana, _) = matrix_accounts(&site).await;
    let app = app_token(&site.db, ana).await;
    let tins = |profiles: Value| -> Vec<String> {
        profiles.as_array().unwrap().iter().map(|profile| profile["tin"].as_str().unwrap().to_owned()).collect()
    };
    assert_eq!(tins(profiles(&site, &app, "/api/tax-profiles").await), ["000-111-001-00000"]);
    let open = profiles(&site, &app, "/api/tax-profiles?include=pending").await;
    assert_eq!(
        tins(open.clone()),
        ["000-111-001-00000", "000-111-004-00000", "000-111-005-00000", "000-111-006-00000"]
    );
    let statuses: Vec<&str> = open.as_array().unwrap().iter().map(|p| p["status"].as_str().unwrap()).collect();
    assert_eq!(statuses, ["approved", "pending", "needs_fix", "needs_fix"]);

    let agent = token(&site.db, ana, "an-agent", "mcp").await;
    let call = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_tax_profiles"}}"#;
    let request = Request::post("/mcp")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {agent}"))
        .body(Body::from(call))
        .unwrap();
    let answer: Value = serde_json::from_str(&text(site.send(request).await).await).unwrap();
    let listed: Vec<&str> = answer["result"]["structuredContent"]["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|profile| profile["tin_masked"].as_str().unwrap())
        .collect();
    assert_eq!(listed, ["***-***-001-00000"]);
}

/// A banned account's app hears why on every call, and can still refresh
/// its tokens and sign out.
#[sqlx::test(migrations = "./migrations")]
async fn a_banned_account_is_refused_on_every_bearer_call_but_sign_out(db: PgPool) {
    let site = Site::new(db);
    let (ana, _) = site.person("ana@example.test").await;
    claim(&site.db, ana, "000444001", "approved").await;
    let device = TestDevice::new(&site.db, ana).await;
    let agent = token(&site.db, ana, "an-agent", "mcp").await;
    let refresh = buwiz_cloud::base::crypto::new_token("bwr_");
    sqlx::query(
        "INSERT INTO oauth_refresh_tokens (grant_id, token_hash, expires_at)
         SELECT grant_id, $2, now() + interval '30 days' FROM devices WHERE id = $1",
    )
    .bind(device.device_id)
    .bind(buwiz_cloud::base::crypto::hash(&refresh))
    .execute(&site.db)
    .await
    .unwrap();
    let get = |path: &str, token: &str| {
        Request::get(path).header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap()
    };
    assert_eq!(site.send(get("/me", &device.token)).await.status(), StatusCode::OK);

    sqlx::query("UPDATE accounts SET banned_at = now(), ban_reason = 'Abuse.' WHERE id = $1")
        .bind(ana)
        .execute(&site.db)
        .await
        .unwrap();
    let banned = (403, "account_banned".to_owned(), BANNED.to_owned());
    for path in ["/me", "/api/tax-profiles", "/api/tax-profiles?include=pending", "/api/submissions", "/api/devices"] {
        assert_eq!(refusal(site.send(get(path, &device.token)).await).await, banned, "{path}");
    }
    assert_eq!(refusal(post_v1(&site, &device.token, &v1_body("000444001")).await).await, banned, "v1");
    assert_eq!(refusal(device.post(&site, "/api/tickets", &ticket_body("000444001", "b1")).await).await, banned);
    let call = Request::post("/mcp")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {agent}"))
        .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#))
        .unwrap();
    assert_eq!(refusal(site.send(call).await).await, banned, "mcp");
    assert_eq!(count(&site.db, "SELECT count(*) FROM tickets").await, 0);

    // The refresh still works, so the app keeps its sign-in and keeps
    // hearing why.
    let refreshed = Request::post("/oauth/token")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!("grant_type=refresh_token&client_id=buwiz-desktop&refresh_token={refresh}")))
        .unwrap();
    let refreshed = site.send(refreshed).await;
    assert_eq!(refreshed.status(), StatusCode::OK);
    let fresh: Value = serde_json::from_str(&text(refreshed).await).unwrap();
    let fresh = fresh["access_token"].as_str().unwrap().to_owned();
    assert_eq!(refusal(site.send(get("/api/tax-profiles", &fresh)).await).await, banned);

    let logout = Request::post("/api/auth/logout")
        .header(header::AUTHORIZATION, format!("Bearer {fresh}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(site.send(logout).await.status(), StatusCode::OK);
    assert_eq!(site.send(get("/api/tax-profiles", &fresh)).await.status(), StatusCode::UNAUTHORIZED);
}
