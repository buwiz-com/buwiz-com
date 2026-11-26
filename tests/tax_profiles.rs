//! `GET /api/tax-profiles?include=all`, the TIN list the apps read to know
//! where each TIN stands and what is next, against the golden answer both
//! repositories keep (`tests/fixtures/tax_profiles_all.json`); its limit
//! per sign-in; `POST /api/tax-profiles`, the app's add of a TIN as not
//! verified; and what AI agents read about claims on `/mcp`. Each test
//! gets its own database from `#[sqlx::test]`.

mod common;

use axum::body::Body;
use axum::http::{Request, header};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use common::*;

const GOLDEN: &str = include_str!("fixtures/tax_profiles_all.json");
/// The note staff wrote on the claim sent back: only the BIR apps see it.
const NOTE: &str = "Retake it in daylight.";

/// The golden answer, with this test site's origin for the links.
fn golden() -> Value {
    serde_json::from_str(&GOLDEN.replace("https://account.buwiz.com", ORIGIN)).unwrap()
}

async fn answer(site: &Site, token: &str, path: &str) -> Value {
    let request =
        Request::get(path).header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap();
    let response = site.send(request).await;
    assert_eq!(response.status(), 200, "{path}");
    serde_json::from_str(&text(response).await).unwrap()
}

/// One claim as the golden example has it: (id, root, name, status).
async fn insert(db: &PgPool, account: Uuid, id: Option<Uuid>, root: &str, name: &str, status: &str, age_days: i32) {
    sqlx::query(
        "INSERT INTO tin_claims (id, account_id, kind, tin_root, branch_code, registered_name, status, created_at,
             submitted_at, decided_at, verified_by, removed_at)
         VALUES (coalesce($1, gen_random_uuid()), $2, 'personal', $3, '00000', $4, $5, now() - make_interval(days => $6),
             CASE WHEN $5 <> 'unverified' THEN now() - make_interval(days => $6) END,
             CASE WHEN $5 IN ('approved', 'rejected', 'withdrawn', 'removed', 'revoked') THEN to_timestamp(1767225600) END,
             CASE WHEN $5 IN ('approved', 'removed', 'revoked') THEN 'staff' END,
             CASE WHEN $5 IN ('removed', 'revoked') THEN now() END)",
    )
    .bind(id)
    .bind(account)
    .bind(root)
    .bind(name)
    .bind(status)
    .bind(age_days)
    .execute(db)
    .await
    .unwrap();
}

/// The five rows of the golden answer on juan's account, with the ids it
/// names, plus rows that must never be listed: juan's ended claims and
/// another account's TINs.
async fn golden_account(site: &Site) -> Uuid {
    let (juan, _) = site.person("juan@example.test").await;
    let (other, _) = site.person("other@example.test").await;
    let rows = golden();
    for (age, (profile, status)) in rows["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .zip(["approved", "approved", "unverified", "pending", "needs_fix"])
        .enumerate()
    {
        let id: Uuid = profile["id"].as_str().unwrap().parse().unwrap();
        let root: String = profile["tin"].as_str().unwrap().chars().filter(char::is_ascii_digit).take(9).collect();
        let name = profile["registered_name"].as_str().unwrap();
        // Oldest first, as the list orders the rows that cannot send.
        insert(&site.db, juan, Some(id), &root, name, status, 10 - age as i32).await;
    }
    sqlx::query(
        "UPDATE tin_claims SET paused_at = now(), paused_by = account_id, pause_reason = 'Checking a complaint.'
         WHERE id = '22222222-2222-4222-8222-222222222222'",
    )
    .execute(&site.db)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE tin_claims SET fix_reasons = ARRAY['blurry'], fix_note = $1, sent_back_at = now()
         WHERE id = '55555555-5555-4555-8555-555555555555'",
    )
    .bind(NOTE)
    .execute(&site.db)
    .await
    .unwrap();
    for (root, status) in
        [("000123460", "rejected"), ("000123461", "withdrawn"), ("000123462", "removed"), ("000123463", "revoked")]
    {
        insert(&site.db, juan, None, root, "ENDED", status, 20).await;
    }
    insert(&site.db, other, None, "000123464", "SOMEONE ELSE", "approved", 3).await;
    insert(&site.db, other, None, "000123455", "SOMEONE ELSE", "unverified", 3).await;
    juan
}

#[sqlx::test(migrations = "./migrations")]
async fn include_all_answers_the_golden_example(db: PgPool) {
    let site = Site::new(db);
    let juan = golden_account(&site).await;
    let app = app_token(&site.db, juan).await;
    let expected = golden();
    let got = answer(&site, &app, "/api/tax-profiles?include=all").await;

    assert_eq!(got["include"], "all");
    let (got_rows, want_rows) = (got["profiles"].as_array().unwrap(), expected["profiles"].as_array().unwrap());
    assert_eq!(got_rows.len(), want_rows.len(), "{got}");
    for (got, want) in got_rows.iter().zip(want_rows) {
        let mut got_keys: Vec<&String> = got.as_object().unwrap().keys().collect();
        let mut want_keys: Vec<&String> = want.as_object().unwrap().keys().collect();
        got_keys.sort();
        want_keys.sort();
        assert_eq!(got_keys, want_keys, "{}", want["status"]);
        for key in want_keys {
            assert_eq!(got[key], want[key], "{} {key}", want["status"]);
        }
    }
    assert_eq!(got, expected);

    // A comma list with `all` in it is `all`.
    for path in ["/api/tax-profiles?include=all,pending", "/api/tax-profiles?include=pending,%20all"] {
        assert_eq!(answer(&site, &app, path).await, expected, "{path}");
    }
    // Active if and only if it can send.
    for profile in got_rows {
        assert_eq!(profile["status"] == "approved", profile["can_send"] == true, "{profile}");
    }
}

/// Without `all` the list means what it meant before: active TINs (a
/// paused one is not active), and with `pending` the ones being checked,
/// in the old shape.
#[sqlx::test(migrations = "./migrations")]
async fn the_default_and_pending_lists_keep_their_meaning(db: PgPool) {
    let site = Site::new(db);
    let juan = golden_account(&site).await;
    let app = app_token(&site.db, juan).await;
    let default = answer(&site, &app, "/api/tax-profiles").await;
    assert_eq!(default.as_object().unwrap().keys().collect::<Vec<_>>(), ["profiles"]);
    assert_eq!(
        default["profiles"],
        json!([{
            "id": "11111111-1111-4111-8111-111111111111", "tin": "000-123-453-00000", "tin_masked": "***-***-453-00000",
            "registered_name": "SYNTHETIC TAXPAYER ONE", "activated_at": 1767225600,
            "permissions": ["profile.view", "filing.prepare", "filing.submit"],
        }])
    );
    let pending = answer(&site, &app, "/api/tax-profiles?include=pending").await;
    assert!(pending.get("include").is_none());
    let rows = pending["profiles"].as_array().unwrap();
    let statuses: Vec<&str> = rows.iter().map(|row| row["status"].as_str().unwrap()).collect();
    assert_eq!(statuses, ["approved", "pending", "needs_fix"]);
    for row in rows {
        let mut keys: Vec<&String> = row.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["activated_at", "id", "permissions", "registered_name", "status", "tin", "tin_masked"]);
    }
}

/// Buwiz Books and AI agents get the same rows without the TIN, the links,
/// filing permissions or the staff's note.
#[sqlx::test(migrations = "./migrations")]
async fn other_clients_get_no_tin_no_link_and_no_note(db: PgPool) {
    let site = Site::new(db);
    let juan = golden_account(&site).await;
    let app = answer(&site, &app_token(&site.db, juan).await, "/api/tax-profiles?include=all").await;
    for (client, scope) in [("buwiz-books", "books"), ("an-agent", "mcp")] {
        let token = token(&site.db, juan, client, scope).await;
        let got = answer(&site, &token, "/api/tax-profiles?include=all").await;
        assert_eq!(got["include"], "all");
        let text = got.to_string();
        assert!(!text.contains(NOTE) && !text.contains("000-123-") && !text.contains("/tins/"), "{scope}: {text}");
        for (row, full) in got["profiles"].as_array().unwrap().iter().zip(app["profiles"].as_array().unwrap()) {
            assert!(row.get("tin").is_none(), "{scope}");
            assert_eq!(row["permissions"], json!(["profile.view"]), "{scope}");
            assert!(row["action"].is_null(), "{scope}");
            for key in ["id", "tin_masked", "registered_name", "activated_at", "status", "can_send", "next", "message"]
            {
                assert_eq!(row[key], full[key], "{scope} {key}");
            }
            if row["status"] == "needs_fix" {
                assert_eq!(row["reasons"], full["reasons"]);
                assert!(row["note"].is_null(), "{scope}");
            }
        }
    }
}

/// What an agent reads about claims: a reviewer's note only when the claim
/// was rejected; for a claim sent back, the fixed reasons.
#[sqlx::test(migrations = "./migrations")]
async fn agents_read_only_a_rejections_note(db: PgPool) {
    let site = Site::new(db);
    let juan = golden_account(&site).await;
    sqlx::query(
        "UPDATE tin_claims SET decision_note = 'Kept for staff.' WHERE id = '11111111-1111-4111-8111-111111111111'",
    )
    .execute(&site.db)
    .await
    .unwrap();
    sqlx::query("UPDATE tin_claims SET decision_note = 'Another account owns this TIN.' WHERE status = 'rejected'")
        .execute(&site.db)
        .await
        .unwrap();
    let agent = token(&site.db, juan, "an-agent", "mcp").await;
    let call = Request::post("/mcp")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {agent}"))
        .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_tin_claims"}}"#))
        .unwrap();
    let answer: Value = serde_json::from_str(&text(site.send(call).await).await).unwrap();
    let text = answer.to_string();
    assert!(!text.contains(NOTE) && !text.contains("Kept for staff."), "{text}");
    let claims = answer["result"]["structuredContent"]["claims"].as_array().unwrap().clone();
    let by_tin = |masked: &str| claims.iter().find(|claim| claim["tin_masked"] == masked).unwrap().clone();
    assert_eq!(by_tin("***-***-460-00000")["note"], "Another account owns this TIN.");
    assert!(by_tin("***-***-453-00000")["note"].is_null());
    assert_eq!(by_tin("***-***-454-00000")["status"], "paused");
    assert_eq!(by_tin("***-***-455-00000")["status"], "unverified");
    let fix = by_tin("***-***-457-00000");
    assert!(fix["note"].is_null());
    assert_eq!(fix["reasons"], json!(["A photo is too blurry to read."]));
}

/// A test TIN on `account`, as an Admin adds one to a test account.
async fn test_tin(db: &PgPool, account: Uuid) {
    sqlx::query("UPDATE accounts SET test_account = true WHERE id = $1").bind(account).execute(db).await.unwrap();
    sqlx::query(
        "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status, decided_at, verified_by)
         VALUES ($1, 'personal', '111111114', '00000', 'TEST TAXPAYER', 'approved', now(), 'admin')",
    )
    .bind(account)
    .execute(db)
    .await
    .unwrap();
}

/// Each row says what the app does next, from the same standing as its
/// `status`: `send` exactly when it can send, `fix` exactly when it has an
/// action (verify, or fix and send again), `wait` otherwise (in review,
/// paused). A test TIN reads as any TIN, with `test_only`. Ended rows and
/// another account's are not listed. `include=all,pending`, which shipped
/// apps ask for, answers exactly as `include=all`, `next` included.
#[sqlx::test(migrations = "./migrations")]
async fn each_row_says_what_is_next(db: PgPool) {
    let site = Site::new(db);
    let juan = golden_account(&site).await;
    test_tin(&site.db, juan).await;
    let app = app_token(&site.db, juan).await;
    let all = answer(&site, &app, "/api/tax-profiles?include=all").await;
    let rows = all["profiles"].as_array().unwrap();
    let read: Vec<(&str, &str, bool)> = rows
        .iter()
        .map(|row| (row["status"].as_str().unwrap(), row["next"].as_str().unwrap(), row["test_only"] == true))
        .collect();
    assert_eq!(
        read,
        [
            ("approved", "send", false),
            ("approved", "send", true),
            ("paused", "wait", false),
            ("unverified", "fix", false),
            ("pending", "wait", false),
            ("needs_fix", "fix", false),
        ],
        "{all}"
    );
    for row in rows {
        assert_eq!(row["next"] == "fix", !row["action"].is_null(), "{row}");
        assert_eq!(row["next"] == "send", row["can_send"] == true, "{row}");
    }
    let text = all.to_string();
    assert!(!text.contains("ENDED") && !text.contains("SOMEONE ELSE"), "{text}");

    let shipped = answer(&site, &app, "/api/tax-profiles?include=all,pending").await;
    assert_eq!(shipped, all);
    assert!(shipped["profiles"].as_array().unwrap().iter().all(|row| row["next"].is_string()), "{shipped}");
    // The other modes are unchanged: no `next`.
    for path in ["/api/tax-profiles", "/api/tax-profiles?include=pending"] {
        let listed = answer(&site, &app, path).await;
        assert!(listed["profiles"].as_array().unwrap().iter().all(|row| row.get("next").is_none()), "{path}");
    }
}

fn get_profiles(token: &str, path: &str) -> Request<Body> {
    Request::get(path).header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap()
}

/// The seconds a `429` says to wait.
fn retry_after(response: &axum::response::Response) -> i64 {
    response.headers().get(header::RETRY_AFTER).expect("Retry-After").to_str().unwrap().parse().unwrap()
}

/// 120 reads of the list per sign-in in 10 minutes, every mode counted
/// together; the 121st is `429 rate_limited` with `Retry-After`, in every
/// mode. The limit is the sign-in's, not the account's: another sign-in of
/// the same account reads on.
#[sqlx::test(migrations = "./migrations")]
async fn the_list_is_limited_per_sign_in(db: PgPool) {
    let site = Site::new(db);
    let juan = golden_account(&site).await;
    let first = app_token(&site.db, juan).await;
    let modes = ["/api/tax-profiles", "/api/tax-profiles?include=pending", "/api/tax-profiles?include=all"];
    for call in 0..120 {
        let response = site.send(get_profiles(&first, modes[call % modes.len()])).await;
        assert_eq!(response.status(), 200, "call {}", call + 1);
    }
    for path in modes {
        let response = site.send(get_profiles(&first, path)).await;
        assert_eq!(response.status(), 429, "{path}");
        let wait = retry_after(&response);
        assert!((1..=600).contains(&wait), "{wait}");
        let (_, code, message) = refusal(response).await;
        assert_eq!(code, "rate_limited");
        let minutes = (wait + 59) / 60;
        let unit = if minutes == 1 { "minute" } else { "minutes" };
        assert_eq!(message, format!("Too many requests. Try again in {minutes} {unit}."));
    }
    let second = app_token(&site.db, juan).await;
    assert_eq!(answer(&site, &second, "/api/tax-profiles?include=all").await, golden());
}

async fn add(site: &Site, token: &str, body: &str) -> axum::response::Response {
    let request = Request::post("/api/tax-profiles")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap();
    site.send(request).await
}

fn personal(tin: &str) -> String {
    json!({ "kind": "personal", "tin": tin, "registered_name": "Dela Cruz Juan" }).to_string()
}

/// The 201's row, after checking its status.
async fn added(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), 201);
    let body: Value = serde_json::from_str(&text(response).await).unwrap();
    assert_eq!(body.as_object().unwrap().keys().collect::<Vec<_>>(), ["profile"], "{body}");
    body["profile"].clone()
}

async fn rows_of(db: &PgPool, account: Uuid) -> (i64, i64) {
    let claims: i64 = sqlx::query_scalar("SELECT count(*) FROM tin_claims WHERE account_id = $1")
        .bind(account)
        .fetch_one(db)
        .await
        .unwrap();
    let lines: i64 = sqlx::query_scalar("SELECT count(*) FROM claim_audit WHERE account_id = $1")
        .bind(account)
        .fetch_one(db)
        .await
        .unwrap();
    (claims, lines)
}

/// The app adds a TIN as not verified: `201` with the row as `include=all`
/// lists it, the claim saved as the site's "add papers later" saves it, and
/// one audit line, from the app.
#[sqlx::test(migrations = "./migrations")]
async fn the_app_adds_a_tin_as_not_verified(db: PgPool) {
    let site = Site::new(db);
    let (juan, _) = site.person("juan@example.test").await;
    let app = app_token(&site.db, juan).await;
    let body = json!({ "kind": "personal", "tin": "123-456-789", "registered_name": "  Dela Cruz Juan " });
    let profile = added(add(&site, &app, &body.to_string()).await).await;
    let id: Uuid = profile["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        profile,
        json!({
            "id": id, "tin": "123-456-789-00000", "tin_masked": "***-***-789-00000",
            "registered_name": "Dela Cruz Juan", "activated_at": null,
            "permissions": ["profile.view", "filing.prepare"],
            "status": "unverified", "can_send": false, "next": "fix",
            "message": "Not verified. Verify it with its papers to send returns.",
            "action": { "label": "Verify to send", "url": format!("{ORIGIN}/tins/claims/{id}/verify") },
        })
    );
    let listed = answer(&site, &app, "/api/tax-profiles?include=all").await;
    assert_eq!(listed["profiles"], json!([profile]));
    let stored: (String, String, String, String, Option<String>, bool) = sqlx::query_as(
        "SELECT kind, tin_root, branch_code, status, trade_name, submitted_at IS NULL FROM tin_claims WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&site.db)
    .await
    .unwrap();
    assert_eq!(stored, ("personal".into(), "123456789".into(), "00000".into(), "unverified".into(), None, true));
    assert_eq!(
        claim_audit_lines(&site.db, juan).await,
        [(
            "created".to_owned(),
            "new".to_owned(),
            "juan@example.test".to_owned(),
            "app".to_owned(),
            Some("added from the app".to_owned())
        )]
    );
    let line: (Uuid, Uuid, String, Option<String>) =
        sqlx::query_as("SELECT claim_id, actor_id, tin_masked, claim_kind FROM claim_audit WHERE account_id = $1")
            .bind(juan)
            .fetch_one(&site.db)
            .await
            .unwrap();
    assert_eq!(line, (id, juan, "***-***-789-00000".to_owned(), Some("personal".to_owned())));

    // A business TIN with its branch, in 14 digits or 12, and a trade name.
    for (tin, branch) in [("987-654-321-00002", "00002"), ("987 654 321 003", "00003")] {
        let body = json!({ "kind": "business", "tin": tin, "registered_name": "Juan Corp", "trade_name": " Juan's " });
        let profile = added(add(&site, &app, &body.to_string()).await).await;
        assert_eq!(profile["tin"], format!("987-654-321-{branch}"));
        let trade: Option<String> = sqlx::query_scalar("SELECT trade_name FROM tin_claims WHERE id = $1")
            .bind(profile["id"].as_str().unwrap().parse::<Uuid>().unwrap())
            .fetch_one(&site.db)
            .await
            .unwrap();
        assert_eq!(trade.as_deref(), Some("Juan's"));
    }
    assert_eq!(rows_of(&site.db, juan).await, (3, 3));
}

/// Each refusal, with its status and code; none writes anything.
#[sqlx::test(migrations = "./migrations")]
async fn the_apps_add_refuses_with_a_code(db: PgPool) {
    let site = Site::new(db);
    let (juan, _) = site.person("juan@example.test").await;
    for (client, scope) in [("buwiz-books", "books"), ("an-agent", "mcp")] {
        let other = token(&site.db, juan, client, scope).await;
        let (status, code, _) = refusal(add(&site, &other, &personal("123456789")).await).await;
        assert_eq!((status, code.as_str()), (403, "insufficient_scope"), "{scope}");
    }
    let name = |name: &str| json!({ "kind": "personal", "tin": "123456789", "registered_name": name }).to_string();
    let long = "x".repeat(201);
    let cases = [
        ("{", 400, "bad_request"),
        ("[]", 400, "bad_request"),
        (r#"{"kind":"personal","registered_name":"Juan"}"#, 400, "bad_request"),
        (r#"{"kind":"company","tin":"123456789","registered_name":"Juan"}"#, 400, "bad_request"),
        (r#"{"tin":"123456789","registered_name":"Juan"}"#, 400, "bad_request"),
        (&personal("12345678"), 400, "bad_tin"),
        (&personal("1234567890"), 400, "bad_tin"),
        (&personal("123-456-789-0000"), 400, "bad_tin"),
        (&personal("no digits"), 400, "bad_tin"),
        (&name(""), 400, "name_missing"),
        (&name("   "), 400, "name_missing"),
        (&name(&long), 400, "name_missing"),
        (
            &json!({ "kind": "business", "tin": "123456789", "registered_name": "Juan", "trade_name": long })
                .to_string(),
            400,
            "name_missing",
        ),
    ];
    for (body, status, code) in cases {
        // A sign-in of its own each, so the limit on adds never answers.
        let app = app_token(&site.db, juan).await;
        let (got_status, got_code, message) = refusal(add(&site, &app, body).await).await;
        assert_eq!((got_status, got_code.as_str()), (status, code), "{body}");
        assert!(message.ends_with('.') && !message.is_empty(), "{body}: {message}");
    }
    assert_eq!(rows_of(&site.db, juan).await, (0, 0));
}

/// A test TIN is refused `test_tin` in every way of writing it, before the
/// database is asked anything: staff add test TINs. Nothing is written.
#[sqlx::test(migrations = "./migrations")]
async fn the_app_cannot_add_a_test_tin(db: PgPool) {
    let site = Site::new(db);
    let (juan, _) = site.person("juan@example.test").await;
    for root in buwiz_cloud::tin::TEST_ROOTS {
        let dashed = format!("{}-{}-{}", &root[..3], &root[3..6], &root[6..]);
        for tin in [dashed.clone(), format!("{dashed}-00000"), format!("{root}00001"), format!("{root}001")] {
            for kind in ["personal", "business"] {
                let app = app_token(&site.db, juan).await;
                let body = json!({ "kind": kind, "tin": tin, "registered_name": "Test Taxpayer" }).to_string();
                let (status, code, message) = refusal(add(&site, &app, &body).await).await;
                assert_eq!((status, code.as_str()), (400, "test_tin"), "{tin}");
                assert_eq!(
                    message,
                    "This TIN is reserved for testing: only Buwiz staff add test TINs. If it is yours, write to support@buwiz.com."
                );
            }
        }
    }
    assert_eq!(rows_of(&site.db, juan).await, (0, 0));
}

/// What this account already holds is refused with `409`: active, saved
/// without papers, in review or sent back. A TIN that ended can be added
/// again.
#[sqlx::test(migrations = "./migrations")]
async fn the_apps_add_refuses_a_tin_already_on_the_account(db: PgPool) {
    let site = Site::new(db);
    let (juan, _) = site.person("juan@example.test").await;
    let cases = [
        ("000123451", "approved", "tin_already_active", "This TIN is already active on your account."),
        (
            "000123452",
            "unverified",
            "tin_already_added",
            "This TIN is already on My TINs. Verify it there to send returns.",
        ),
        ("000123453", "pending", "claim_already_pending", "You already have a claim for this TIN waiting for review."),
        (
            "000123454",
            "needs_fix",
            "claim_already_pending",
            "You already have a claim for this TIN waiting for review.",
        ),
    ];
    for (root, status, ..) in cases {
        add_claim(&site.db, juan, root, status).await;
    }
    for (root, status, code, message) in cases {
        let app = app_token(&site.db, juan).await;
        let refused = refusal(add(&site, &app, &personal(root)).await).await;
        assert_eq!(refused, (409, code.to_owned(), message.to_owned()), "{status}");
    }
    for ended in ["rejected", "removed"] {
        let root = if ended == "rejected" { "000123455" } else { "000123456" };
        add_claim(&site.db, juan, root, ended).await;
        let app = app_token(&site.db, juan).await;
        assert_eq!(add(&site, &app, &personal(root)).await.status(), 201, "{ended}");
    }
    assert_eq!(rows_of(&site.db, juan).await, (8, 2));
}

/// The cap on TINs waiting to be verified holds as on the site: the 20th
/// is added, the 21st refused `too_many_unverified`.
#[sqlx::test(migrations = "./migrations")]
async fn the_apps_add_keeps_the_cap_on_unverified_tins(db: PgPool) {
    let site = Site::new(db);
    let (juan, _) = site.person("juan@example.test").await;
    for index in 0..19 {
        add_claim(&site.db, juan, &format!("000200{index:03}"), "unverified").await;
    }
    let app = app_token(&site.db, juan).await;
    added(add(&site, &app, &personal("000-300-001")).await).await;
    let refused = refusal(add(&site, &app, &personal("000-300-002")).await).await;
    assert_eq!(
        refused,
        (
            409,
            "too_many_unverified".to_owned(),
            "You have 20 TINs waiting to be verified. Verify or remove some first.".to_owned()
        )
    );
    assert_eq!(rows_of(&site.db, juan).await, (20, 1));
}

/// 10 adds per sign-in in an hour, whatever comes of them; the 11th is
/// `429 rate_limited` with `Retry-After`. Another sign-in of the same
/// account adds on.
#[sqlx::test(migrations = "./migrations")]
async fn the_apps_add_is_limited_per_sign_in(db: PgPool) {
    let site = Site::new(db);
    let (juan, _) = site.person("juan@example.test").await;
    let app = app_token(&site.db, juan).await;
    for index in 0..9 {
        added(add(&site, &app, &personal(&format!("000-400-{index:03}"))).await).await;
    }
    // A refusal counts too.
    assert_eq!(refusal(add(&site, &app, &personal("000-400-000")).await).await.1, "tin_already_added");
    let response = add(&site, &app, &personal("000-400-100")).await;
    assert_eq!(response.status(), 429);
    let wait = retry_after(&response);
    assert!((3540..=3600).contains(&wait), "{wait}");
    assert_eq!(
        refusal(response).await,
        (429, "rate_limited".to_owned(), "Too many requests. Try again in 60 minutes.".to_owned())
    );
    // The list is limited apart.
    answer(&site, &app, "/api/tax-profiles?include=all").await;
    let second = app_token(&site.db, juan).await;
    added(add(&site, &second, &personal("000-400-100")).await).await;
    assert_eq!(rows_of(&site.db, juan).await, (10, 10));
}

/// Adding a TIN another account holds, active or not verified, answers
/// exactly as adding a free one: nothing says someone else has it. Each
/// refusal after speaks of this account's own row only.
#[sqlx::test(migrations = "./migrations")]
async fn the_apps_add_never_tells_whether_another_account_has_the_tin(db: PgPool) {
    let site = Site::new(db);
    let (juan, _) = site.person("juan@example.test").await;
    let (other, _) = site.person("other@example.test").await;
    add_claim(&site.db, other, "000500001", "approved").await;
    add_claim(&site.db, other, "000500002", "unverified").await;
    let app = app_token(&site.db, juan).await;
    let mut rows = Vec::new();
    for root in ["000500001", "000500002", "000500003"] {
        let mut profile = added(add(&site, &app, &personal(root)).await).await;
        let id = profile["id"].as_str().unwrap().to_owned();
        assert_eq!(profile["tin"], format!("{}-{}-{}-00000", &root[..3], &root[3..6], &root[6..]));
        for key in ["id", "tin", "tin_masked"] {
            profile.as_object_mut().unwrap().remove(key);
        }
        assert_eq!(profile["action"]["url"], format!("{ORIGIN}/tins/claims/{id}/verify"));
        profile["action"]["url"] = Value::Null;
        rows.push(profile);
    }
    assert_eq!(rows[0], rows[2], "held active elsewhere reads as free");
    assert_eq!(rows[1], rows[2], "held unverified elsewhere reads as free");
    for root in ["000500001", "000500002", "000500003"] {
        let refused = refusal(add(&site, &app, &personal(root)).await).await;
        assert_eq!(
            refused,
            (
                409,
                "tin_already_added".to_owned(),
                "This TIN is already on My TINs. Verify it there to send returns.".to_owned()
            ),
            "{root}"
        );
    }
    // The other account's rows are as they were.
    let theirs: Vec<(String, String)> =
        sqlx::query_as("SELECT tin_root, status FROM tin_claims WHERE account_id = $1 ORDER BY tin_root")
            .bind(other)
            .fetch_all(&site.db)
            .await
            .unwrap();
    assert_eq!(theirs, [("000500001".into(), "approved".into()), ("000500002".into(), "unverified".into())]);
    assert_eq!(rows_of(&site.db, other).await, (2, 0));
}
