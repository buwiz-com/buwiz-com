//! `GET /api/tax-profiles?include=all`, the TIN list the apps read to know
//! where each TIN stands, against the golden answer both repositories keep
//! (`tests/fixtures/tax_profiles_all.json`); and what AI agents read about
//! claims on `/mcp`. Each test gets its own database from `#[sqlx::test]`.

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
            for key in ["id", "tin_masked", "registered_name", "activated_at", "status", "can_send", "message"] {
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
