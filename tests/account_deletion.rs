//! People delete their own account on `/delete`: a fresh code mailed for
//! it, then everything that names the account goes in one transaction,
//! its stored files after, and its apps are signed out. Staff, accounts
//! named in staff work and a send under way are refused. Also the hourly
//! cleanup of old sign-in records. Each test gets its own database from
//! `#[sqlx::test]`.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::{Body, Bytes};
use axum::http::{Method, Request, StatusCode, Uri, header};
use axum::response::IntoResponse;
use buwiz_cloud::base::crypto;
use buwiz_cloud::base::storage::{DocumentStore, ObjectStore};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use common::*;

const ANA: &str = "ana@example.test";
const BOB: &str = "bob@example.test";

/// Asks for a deletion code: where the form went, and the code mailed, if
/// one was.
async fn ask(site: &Site, cookie: &str) -> (String, Option<String>) {
    let before = site.state.mailer.deletion_codes().len();
    let location = location(&site.post("/delete/code", cookie, "").await);
    let codes = site.state.mailer.deletion_codes();
    (location, codes.get(before).map(|(_, code)| code.clone()))
}

/// Types a code on the page: where it went.
async fn confirm(site: &Site, cookie: &str, code: &str) -> String {
    location(&site.post("/delete/confirm", cookie, &format!("code={code}")).await)
}

async fn exists(db: &PgPool, account: Uuid) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1)")
        .bind(account)
        .fetch_one(db)
        .await
        .unwrap()
}

async fn count(db: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(db).await.unwrap()
}

async fn exec(db: &PgPool, sql: &str) {
    sqlx::raw_sql(sql).execute(db).await.unwrap();
}

/// A code stored as `/delete/code` (`purpose`: `delete`) or the sign-in
/// form (`sign_in`) stores it, without mailing it.
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

/// Signs in through the forms with a mailed code, going on to `next`: the
/// response to the code.
async fn sign_in(site: &Site, email: &str, next: &str) -> axum::response::Response {
    let form = format!("email={email}&next={next}");
    site.post("/auth/email/start", "", &form).await;
    let (_, code) = site.state.mailer.sign_in_codes().last().unwrap().clone();
    site.post("/auth/email/verify", "", &format!("email={email}&code={code}&next={next}")).await
}

fn set_cookie(response: &axum::response::Response) -> String {
    let value = response.headers().get(header::SET_COOKIE).map(|value| value.to_str().unwrap()).unwrap_or_default();
    value.split(';').next().unwrap_or_default().to_owned()
}

#[sqlx::test(migrations = "./migrations")]
async fn the_page_needs_a_sign_in_and_says_what_goes_and_what_stays(db: PgPool) {
    let site = Site::with_mailbox(db);
    let sign_in_first = "/login?next=%2Fdelete&notice=sign_in_first";
    let response = site.get("/delete", "").await;
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location(&response), sign_in_first);
    assert_eq!(location(&site.post("/delete/code", "", "").await), sign_in_first);
    assert_eq!(location(&site.post("/delete/confirm", "", "code=123456").await), sign_in_first);
    assert!(site.state.mailer.deletion_codes().is_empty());

    let (_, cookie) = site.person(ANA).await;
    let response = site.get("/delete", &cookie).await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = text(response).await;
    assert!(page.contains("<title>Delete your account · Buwiz</title>"), "{page}");
    for words in [
        "Your email address and your account.",
        "Every device and sign-in. The Buwiz apps and any AI agent are signed out.",
        "Every TIN on your account, with the files you sent for it",
        "The records of every return you sent through Buwiz, with their receipts and the send permissions (tickets)",
        "Your place in the BIR queue, and the records of BIR outages",
        "The records of the sign-in codes mailed to you.",
        "Staff audit lines",
        "only with its middle digits hidden, and your account only by an id number",
        "BIR uptime records for 90 days. They hold only scrambled codes",
        "Our server logs, which carry ids only.",
        "database provider",
        "Returns the BIR already received stay with the BIR. Deleting your account does not withdraw them.",
        "Nothing stored in the app on your own computer is touched.",
        "action=\"/delete/code\"",
        "Email me a code",
    ] {
        assert!(page.contains(words), "{words}");
    }
    // The code field and its last check come only once a code was sent.
    assert!(!page.contains("action=\"/delete/confirm\""));
    assert_eq!(ask(&site, &cookie).await.0, "/delete?notice=delete_code_sent");
    let page = text(site.get("/delete?notice=delete_code_sent", &cookie).await).await;
    assert!(page.contains("action=\"/delete/confirm\""), "{page}");
    assert!(page.contains("data-confirm=\"Delete your account for good?\""), "{page}");
    assert!(page.contains("data-confirm-tone=\"danger\""), "{page}");
}

#[sqlx::test(migrations = "./migrations")]
async fn only_the_newest_code_asked_for_on_the_page_deletes(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, cookie) = site.person(ANA).await;

    // A sign-in code is no deletion code.
    site.post("/auth/email/start", "", &format!("email={ANA}")).await;
    let (_, sign_in_code) = site.state.mailer.sign_in_codes().last().unwrap().clone();
    assert_eq!(confirm(&site, &cookie, &sign_in_code).await, "/delete?notice=delete_code_invalid");
    assert!(exists(&site.db, ana).await);

    // A code asked for before the last one no longer works.
    let (_, older) = ask(&site, &cookie).await;
    let (went, newest) = ask(&site, &cookie).await;
    assert_eq!(went, "/delete?notice=delete_code_sent");
    let (older, newest) = (older.unwrap(), newest.unwrap());
    assert_eq!(site.state.mailer.deletion_codes().last().unwrap().0, ANA);
    if older != newest {
        assert_eq!(confirm(&site, &cookie, &older).await, "/delete?notice=delete_code_invalid");
        assert!(exists(&site.db, ana).await);
    }
    // And a deletion code signs nobody in.
    let response = site.post("/auth/email/verify", "", &format!("email={ANA}&code={newest}")).await;
    assert!(location(&response).starts_with("/login/verify?"), "{}", location(&response));
    assert!(set_cookie(&response).is_empty());
    // The sign-in code still works for signing in: no try of it was used.
    let response = site.post("/auth/email/verify", "", &format!("email={ANA}&code={sign_in_code}")).await;
    assert_eq!(location(&response), "/tins");

    let response = site.post("/delete/confirm", &cookie, &format!("code={newest}")).await;
    assert_eq!(location(&response), "/login?notice=account_deleted");
    assert!(set_cookie(&response).ends_with('='), "{}", set_cookie(&response));
    assert!(!exists(&site.db, ana).await);
    assert_eq!(count(&site.db, "SELECT count(*) FROM email_codes").await, 0);
    let page = text(site.get("/login?notice=account_deleted", "").await).await;
    assert!(page.contains("Your Buwiz account is deleted"), "{page}");
}

#[sqlx::test(migrations = "./migrations")]
async fn wrong_codes_use_up_the_five_tries(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, cookie) = site.person(ANA).await;
    let code = ask(&site, &cookie).await.1.unwrap();
    let wrong = if code == "000000" { "111111" } else { "000000" };
    for _ in 0..5 {
        assert_eq!(confirm(&site, &cookie, wrong).await, "/delete?notice=delete_code_invalid");
    }
    // The right code no longer works either.
    assert_eq!(confirm(&site, &cookie, &code).await, "/delete?notice=delete_code_invalid");
    assert!(exists(&site.db, ana).await);
    // An expired code doesn't.
    let code = ask(&site, &cookie).await.1.unwrap();
    exec(&site.db, "UPDATE email_codes SET expires_at = now() - interval '1 second' WHERE purpose = 'delete'").await;
    assert_eq!(confirm(&site, &cookie, &code).await, "/delete?notice=delete_code_invalid");
    // A new one does. Two of the five codes an hour are left.
    let code = ask(&site, &cookie).await.1.unwrap();
    assert_eq!(confirm(&site, &cookie, &code).await, "/login?notice=account_deleted");
    assert!(!exists(&site.db, ana).await);
}

#[sqlx::test(migrations = "./migrations")]
async fn codes_for_deleting_share_the_hourly_limit_of_the_address(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (_, cookie) = site.person(ANA).await;
    for _ in 0..5 {
        assert_eq!(ask(&site, &cookie).await.0, "/delete?notice=delete_code_sent");
    }
    assert_eq!(ask(&site, &cookie).await, ("/delete?notice=code_too_many".to_owned(), None));
}

/// A practice account signs in with its fixed code, but deleting it takes
/// a random code mailed to its address.
#[sqlx::test(migrations = "./migrations")]
async fn a_practice_account_gets_a_mailed_code_not_its_fixed_one(db: PgPool) {
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
    let (went, code) = ask(&site, &cookie).await;
    assert_eq!(went, "/delete?notice=delete_code_sent");
    let code = code.unwrap();
    assert_eq!(site.state.mailer.deletion_codes(), [("reviewer@example.test".to_owned(), code.clone())]);
    if code != "481516" {
        assert_eq!(confirm(&site, &cookie, "481516").await, "/delete?notice=delete_code_invalid");
    }
    assert_eq!(confirm(&site, &cookie, &code).await, "/login?notice=account_deleted");
    assert!(!exists(&site.db, reviewer).await);
}

/// The tables that hold rows of an account, and their counts.
const TABLES: &[&str] = &[
    "accounts",
    "email_codes",
    "web_sessions",
    "device_authorizations",
    "oauth_grants",
    "oauth_access_tokens",
    "oauth_refresh_tokens",
    "oauth_codes",
    "devices",
    "request_nonces",
    "tin_claims",
    "claim_documents",
    "document_readings",
    "submissions",
    "tickets",
    "bir_held",
    "bir_proofs",
    "bir_queue_reports",
    "bir_turns",
];

async fn counts(db: &PgPool) -> Vec<(&'static str, i64)> {
    let mut counts = Vec::new();
    for table in TABLES {
        counts.push((*table, count(db, &format!("SELECT count(*) FROM {table}")).await));
    }
    counts
}

/// One of everything for an account: TINs, a file and its reading, a
/// device, its sign-ins and tokens, a sent return with its ticket, sign-in
/// records, BIR queue and outage rows, and audit lines. `n` keeps unique
/// values apart.
async fn fill(site: &Site, account: Uuid, email: &str, n: u8) {
    let db = &site.db;
    let active = add_claim(db, account, &format!("00012345{n}"), "approved").await;
    let pending = add_claim(db, account, &format!("00098765{n}"), "pending").await;
    let device = TestDevice::new(db, account).await;
    let refresh = crypto::new_token("bwr_");
    sqlx::query(
        "INSERT INTO oauth_refresh_tokens (grant_id, token_hash, expires_at)
         SELECT grant_id, $2, now() + interval '30 days' FROM devices WHERE id = $1",
    )
    .bind(device.device_id)
    .bind(crypto::hash(&refresh))
    .execute(db)
    .await
    .unwrap();
    let jti = Uuid::new_v4();
    let device_id = device.device_id;
    exec(
        db,
        &format!(
            "INSERT INTO claim_documents (claim_id, kind, filename, content_type, size_bytes, sha256, nonce, ciphertext)
             VALUES ('{pending}', 'form_2303', '2303.pdf', 'application/pdf', 5, decode('00', 'hex'), decode('01', 'hex'), decode('02', 'hex'));
             INSERT INTO document_readings (id, document_id, status)
             SELECT gen_random_uuid(), id, 'read' FROM claim_documents WHERE claim_id = '{pending}';
             INSERT INTO submissions (id, account_id, claim_id, form_type, period, filename, xml_sha256, status, receipt_jws,
                 sent_at, ticket_jti, device_id)
             VALUES ('{jti}', '{account}', '{active}', '1701Qv2018', '2026Q1', 'f#{email}#.xml', decode('aa', 'hex'), 'sent',
                 'head.body.sig', now(), '{jti}', '{device_id}');
             INSERT INTO tickets (jti, account_id, device_id, claim_id, form_type, period, payload_sha256, email,
                 idempotency_key, jws, expires_at, submission_id, used_at)
             VALUES ('{jti}', '{account}', '{device_id}', '{active}', '1701Qv2018', '2026Q1', decode('aa', 'hex'), '{email}',
                 'k1', 'h.c.s', now() + interval '5 minutes', '{jti}', now());
             INSERT INTO request_nonces (public_key, nonce) SELECT public_key, 'n{n}' FROM devices WHERE id = '{device_id}';
             INSERT INTO device_authorizations
                 (client_id, device_code_hash, user_code, interval_seconds, expires_at, account_id, decision)
             VALUES ('buwiz-desktop', decode('a{n}', 'hex'), 'BCDF-GHJ{n}', 5, now() + interval '10 minutes', '{account}', 'approved');
             INSERT INTO oauth_clients (client_id, client_name, redirect_uris)
             VALUES ('agent_test', 'Test agent', ARRAY['https://agent.example.test/cb']) ON CONFLICT DO NOTHING;
             INSERT INTO oauth_codes (code_hash, client_id, account_id, redirect_uri, code_challenge, scope, expires_at)
             VALUES (decode('b{n}', 'hex'), 'agent_test', '{account}', 'https://agent.example.test/cb', 'c', 'mcp',
                 now() + interval '10 minutes');
             INSERT INTO bir_held (account_id, claim_id, form, period, code)
             VALUES ('{account}', '{active}', '1701Q', '2026Q1', 'bir_offline');
             INSERT INTO bir_proofs (id, account_id, claim_id, day, form, period, kid, jws)
             VALUES ('p_{n}', '{account}', '{active}', '2026-11-11', '1701Q', '2026Q1', 'k1', 'a.b.c');
             INSERT INTO bir_queue_reports (device_key, account_id, client, paused, waiting, reported_at, expires_at)
             VALUES ('d:{device_id}', '{account}', 'desktop', false, '[]', now(), now() + interval '20 minutes');
             INSERT INTO bir_turns (id, kind, account_id, device_key, slot, not_before, expires_at)
             VALUES ('t_{n}', 'admit', '{account}', 'd:{device_id}', 's1', now(), now() + interval '5 minutes');
             INSERT INTO claim_audit (claim_id, account_id, tin_masked, action, claim_status, actor_id, actor_email, source)
             VALUES (gen_random_uuid(), '{account}', '***-***-999-00000', 'deleted', 'pending', '{account}', '{email}', 'web');"
        ),
    )
    .await;
    plant_code(db, email, "222222", "sign_in").await;
}

#[sqlx::test(migrations = "./migrations")]
async fn every_row_of_the_account_goes_and_other_accounts_keep_theirs(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, cookie) = site.person(ANA).await;
    let (bob, _) = site.person(BOB).await;
    fill(&site, ana, ANA, 1).await;
    fill(&site, bob, BOB, 2).await;
    // Staff acted on Ana: the line names them, not her.
    exec(
        &site.db,
        &format!(
            "INSERT INTO account_audit (account_id, action, actor_id, actor_email, source, reason)
             VALUES ('{ana}', 'signed_out', gen_random_uuid(), 'staff@example.test', 'web', 'Lost phone')"
        ),
    )
    .await;
    let before = counts(&site.db).await;
    for (table, rows) in &before {
        assert!(*rows > 0 && rows % 2 == 0, "{table}: {rows}");
    }

    let code = ask(&site, &cookie).await.1.unwrap();
    assert_eq!(confirm(&site, &cookie, &code).await, "/login?notice=account_deleted");

    // Every table holds Bob's half only.
    let after = counts(&site.db).await;
    let halves: Vec<(&str, i64)> = before.iter().map(|(table, rows)| (*table, rows / 2)).collect();
    assert_eq!(after, halves);
    let left = |table: &str| format!("SELECT count(*) FROM {table} WHERE account_id = '{bob}'");
    for table in ["web_sessions", "devices", "tin_claims", "submissions", "tickets", "bir_held", "bir_turns"] {
        assert!(count(&site.db, &left(table)).await > 0, "{table}");
    }
    let codes = |email: &str| format!("SELECT count(*) FROM email_codes WHERE email = '{email}'");
    assert_eq!(count(&site.db, &codes(ANA)).await, 0);
    assert_eq!(count(&site.db, &codes(BOB)).await, 1);
    assert_eq!(count(&site.db, "SELECT count(*) FROM oauth_clients").await, 1);

    // The audit lines stay, by id: her own action lost her address, staff's
    // kept theirs, and a `deleted` line names her by id only.
    assert_eq!(
        claim_audit_lines(&site.db, ana).await,
        [("deleted".to_owned(), "pending".to_owned(), String::new(), "web".to_owned(), None)]
    );
    assert_eq!(claim_audit_lines(&site.db, bob).await[0].2, BOB);
    let lines = account_audit_lines(&site.db, ana).await;
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].3, "staff@example.test");
    assert_eq!(lines[1], ("deleted".to_owned(), None, None, String::new(), "web".to_owned(), None));
    let actor: Uuid = sqlx::query_scalar("SELECT actor_id FROM account_audit WHERE action = 'deleted'")
        .fetch_one(&site.db)
        .await
        .unwrap();
    assert_eq!(actor, ana);
    let mentions = count(&site.db, &format!("SELECT count(*) FROM account_audit a WHERE a::text LIKE '%{ANA}%'")).await
        + count(&site.db, &format!("SELECT count(*) FROM claim_audit a WHERE a::text LIKE '%{ANA}%'")).await;
    assert_eq!(mentions, 0);
    // The browser session went with the account.
    assert_eq!(site.get("/delete", &cookie).await.status(), StatusCode::TEMPORARY_REDIRECT);
}

/// A stand-in for R2: objects kept in memory by path (as in
/// `tests/claims.rs`).
#[derive(Clone, Default)]
struct FakeObjects(Arc<Mutex<HashMap<String, Vec<u8>>>>);

impl FakeObjects {
    async fn start() -> (Self, DocumentStore) {
        let objects = Self::default();
        let shared = objects.0.clone();
        let app = axum::Router::new().route(
            "/{*key}",
            axum::routing::any(move |method: Method, uri: Uri, body: Bytes| {
                let shared = shared.clone();
                async move {
                    let mut objects = shared.lock().unwrap();
                    let path = uri.path().to_owned();
                    match method {
                        Method::PUT => {
                            objects.insert(path, body.to_vec());
                            StatusCode::OK.into_response()
                        }
                        Method::GET => match objects.get(&path) {
                            Some(bytes) => bytes.clone().into_response(),
                            None => StatusCode::NOT_FOUND.into_response(),
                        },
                        Method::DELETE => {
                            objects.remove(&path);
                            StatusCode::NO_CONTENT.into_response()
                        }
                        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = ObjectStore::new(&endpoint, "auto", "buwiz-test", "key", "secret").unwrap();
        (objects, DocumentStore::Objects(Box::new(store)))
    }

    fn of_claim(&self, claim_id: Uuid) -> Vec<String> {
        let prefix = format!("/buwiz-test/claims/{claim_id}/");
        self.0.lock().unwrap().keys().filter(|key| key.starts_with(&prefix)).cloned().collect()
    }
}

const BUSINESS: [(&str, Part); 5] = [
    ("kind", Part::Text("business")),
    ("tin", Part::Text("987654321")),
    ("registered_name", Part::Text("Cora Trading Inc.")),
    ("form_2303", Part::File("2303.pdf", PDF)),
    ("government_id", Part::File("id.png", PNG)),
];

#[sqlx::test(migrations = "./migrations")]
async fn the_stored_files_of_open_claims_are_deleted_after_the_rows(db: PgPool) {
    let (objects, store) = FakeObjects::start().await;
    let site = Site::with_store(db, store);
    let (ana, cookie) = site.person(ANA).await;
    let (bob, bob_cookie) = site.person(BOB).await;
    site.upload("/tins/claims", &cookie, &BUSINESS).await;
    site.upload("/tins/claims", &bob_cookie, &BUSINESS).await;
    let (anas, bobs) = (site.newest_claim(ana).await, site.newest_claim(bob).await);
    assert_eq!(objects.of_claim(anas).len(), 2);
    assert_eq!(objects.of_claim(bobs).len(), 2);

    // This site logs its mail, so the code is planted as the page stores it.
    plant_code(&site.db, ANA, "654321", "delete").await;
    assert_eq!(confirm(&site, &cookie, "654321").await, "/login?notice=account_deleted");
    assert!(!exists(&site.db, ana).await);
    assert!(objects.of_claim(anas).is_empty());
    assert_eq!(objects.of_claim(bobs).len(), 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn staff_are_refused_and_told_to_drop_the_role_first(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (sam, cookie) = site.staff_with_role("sam@example.test", "support").await;
    let page = text(site.get("/delete", &cookie).await).await;
    assert!(page.contains("Your account has a staff role. Ask an Admin to remove the role first"), "{page}");
    assert!(!page.contains("action=\"/delete/code\""));
    assert_eq!(ask(&site, &cookie).await, ("/delete?notice=delete_staff".to_owned(), None));
    plant_code(&site.db, "sam@example.test", "654321", "delete").await;
    assert_eq!(confirm(&site, &cookie, "654321").await, "/delete?notice=delete_staff");
    assert!(exists(&site.db, sam).await);

    // An Admin through ADMIN_EMAILS has no stored role to drop.
    let (admin, cookie) = site.person(STAFF).await;
    assert_eq!(ask(&site, &cookie).await, ("/delete?notice=delete_admin_list".to_owned(), None));
    plant_code(&site.db, STAFF, "654321", "delete").await;
    assert_eq!(confirm(&site, &cookie, "654321").await, "/delete?notice=delete_admin_list");
    assert!(exists(&site.db, admin).await);
}

/// Someone who was staff once is named in other accounts' rows, where
/// NULL means something else: those rows are never changed, so support
/// deletes the account by hand.
#[sqlx::test(migrations = "./migrations")]
async fn an_account_named_in_staff_work_is_sent_to_support(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (rita, cookie) = site.person("rita@example.test").await;
    let (bob, _) = site.person(BOB).await;
    let claim = add_claim(&site.db, bob, "000123451", "approved").await;
    let named = [
        format!("UPDATE tin_claims SET decided_by = '{rita}' WHERE id = '{claim}'"),
        format!(
            "UPDATE tin_claims SET paused_at = now(), paused_by = '{rita}', pause_reason = 'Check' WHERE id = '{claim}'"
        ),
        format!("UPDATE accounts SET banned_at = now(), banned_by = '{rita}', ban_reason = 'Abuse' WHERE id = '{bob}'"),
        format!(
            "INSERT INTO account_audit (account_id, action, actor_id, actor_email, source)
             VALUES ('{bob}', 'signed_out', '{rita}', 'rita@example.test', 'web')"
        ),
        format!(
            "INSERT INTO bir_notes (kind, text, until, set_by, set_by_email)
             VALUES ('outage', 'Down.', now() + interval '1 hour', '{rita}', 'rita@example.test')"
        ),
    ];
    let undo = [
        format!("UPDATE tin_claims SET decided_by = NULL WHERE id = '{claim}'"),
        format!("UPDATE tin_claims SET paused_at = NULL, paused_by = NULL, pause_reason = NULL WHERE id = '{claim}'"),
        format!("UPDATE accounts SET banned_at = NULL, banned_by = NULL, ban_reason = NULL WHERE id = '{bob}'"),
        "DELETE FROM account_audit".to_owned(),
        "DELETE FROM bir_notes".to_owned(),
    ];
    for (name, undo) in named.iter().zip(&undo) {
        exec(&site.db, name).await;
        let page = text(site.get("/delete", &cookie).await).await;
        assert!(page.contains("Write to support@buwiz.com to delete it."), "{name}");
        assert_eq!(ask(&site, &cookie).await, ("/delete?notice=delete_referenced".to_owned(), None), "{name}");
        exec(&site.db, undo).await;
    }
    // Named when the code is typed: refused, and the code is not used up.
    let code = ask(&site, &cookie).await.1.unwrap();
    exec(&site.db, &named[0]).await;
    assert_eq!(confirm(&site, &cookie, &code).await, "/delete?notice=delete_referenced");
    assert!(exists(&site.db, rita).await);
    exec(&site.db, &undo[0]).await;
    assert_eq!(confirm(&site, &cookie, &code).await, "/login?notice=account_deleted");
    assert!(!exists(&site.db, rita).await);
    // Bob's claim kept its row and its NULLs.
    assert_eq!(site.claim_status(claim).await, "approved");
}

#[sqlx::test(migrations = "./migrations")]
async fn a_send_under_way_holds_the_deletion(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, cookie) = site.person(ANA).await;
    let claim = add_claim(&site.db, ana, "000123451", "approved").await;
    let code = ask(&site, &cookie).await.1.unwrap();
    exec(
        &site.db,
        &format!(
            "INSERT INTO submissions (account_id, claim_id, form_type, period, filename, xml_sha256, status, sending_since)
             VALUES ('{ana}', '{claim}', '1701Qv2018', '2026Q1', 'f.xml', decode('00', 'hex'), 'sending', now())"
        ),
    )
    .await;
    let page = text(site.get("/delete", &cookie).await).await;
    assert!(page.contains("A return is being sent right now; try again in a few minutes."), "{page}");
    assert_eq!(ask(&site, &cookie).await, ("/delete?notice=delete_sending".to_owned(), None));
    assert_eq!(confirm(&site, &cookie, &code).await, "/delete?notice=delete_sending");
    assert!(exists(&site.db, ana).await);

    exec(&site.db, "UPDATE submissions SET status = 'sent', sent_at = now()").await;
    assert_eq!(confirm(&site, &cookie, &code).await, "/login?notice=account_deleted");
    assert!(!exists(&site.db, ana).await);
}

/// A banned account can't sign in, but it can sign in to delete itself;
/// its history stays.
#[sqlx::test(migrations = "./migrations")]
async fn a_banned_account_may_delete_itself(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, _) = site.person(ANA).await;
    exec(
        &site.db,
        &format!(
            "UPDATE accounts SET banned_at = now(), banned_by = '{ana}', ban_reason = 'Abuse.' WHERE id = '{ana}';
             INSERT INTO account_audit (account_id, action, actor_id, actor_email, source, reason)
             VALUES ('{ana}', 'banned', gen_random_uuid(), 'boss@example.test', 'web', 'Abuse.')"
        ),
    )
    .await;
    let response = sign_in(&site, ANA, "/tins").await;
    assert_eq!(location(&response), "/login?notice=account_banned");
    assert!(set_cookie(&response).is_empty());

    let response = sign_in(&site, ANA, "/delete").await;
    assert_eq!(location(&response), "/delete");
    let cookie = set_cookie(&response);
    assert!(cookie.starts_with("buwiz_session=bws_"), "{cookie}");
    // That session opens no other page.
    assert_eq!(location(&site.get("/tins", &cookie).await), "/login?next=%2Ftins&notice=sign_in_first");
    assert_eq!(site.get("/delete", &cookie).await.status(), StatusCode::OK);
    let code = ask(&site, &cookie).await.1.unwrap();
    assert_eq!(confirm(&site, &cookie, &code).await, "/login?notice=account_deleted");
    assert!(!exists(&site.db, ana).await);
    let actions: Vec<String> = account_audit_lines(&site.db, ana).await.into_iter().map(|line| line.0).collect();
    assert_eq!(actions, ["banned", "deleted"]);
}

/// The app is signed out the way it already understands: its access token
/// is refused as `invalid_token`, and its refresh as `invalid_grant`.
#[sqlx::test(migrations = "./migrations")]
async fn the_apps_tokens_stop_working(db: PgPool) {
    let site = Site::with_mailbox(db);
    let (ana, cookie) = site.person(ANA).await;
    let device = TestDevice::new(&site.db, ana).await;
    let agent = token(&site.db, ana, "an-agent", "mcp").await;
    let refresh = crypto::new_token("bwr_");
    sqlx::query(
        "INSERT INTO oauth_refresh_tokens (grant_id, token_hash, expires_at)
         SELECT grant_id, $2, now() + interval '30 days' FROM devices WHERE id = $1",
    )
    .bind(device.device_id)
    .bind(crypto::hash(&refresh))
    .execute(&site.db)
    .await
    .unwrap();
    let get = |path: &str, token: &str| {
        Request::get(path).header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap()
    };
    let refresh_request = || {
        Request::post("/oauth/token")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(format!("grant_type=refresh_token&client_id=buwiz-desktop&refresh_token={refresh}")))
            .unwrap()
    };
    assert_eq!(site.send(get("/me", &device.token)).await.status(), StatusCode::OK);

    let code = ask(&site, &cookie).await.1.unwrap();
    assert_eq!(confirm(&site, &cookie, &code).await, "/login?notice=account_deleted");

    let signed_out = (401, "invalid_token".to_owned(), "sign in again".to_owned());
    for path in ["/me", "/api/tax-profiles", "/api/devices", "/api/submissions"] {
        assert_eq!(refusal(site.send(get(path, &device.token)).await).await, signed_out, "{path}");
    }
    assert_eq!(refusal(site.send(get("/me", &agent)).await).await, signed_out, "agent");
    let logout = Request::post("/api/auth/logout")
        .header(header::AUTHORIZATION, format!("Bearer {}", device.token))
        .body(Body::empty())
        .unwrap();
    assert_eq!(site.send(logout).await.status(), StatusCode::UNAUTHORIZED);
    let refreshed = site.send(refresh_request()).await;
    assert_eq!(refreshed.status(), StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_str(&text(refreshed).await).unwrap();
    assert_eq!(body["error"], "invalid_grant");
}

#[sqlx::test(migrations = "./migrations")]
async fn the_hourly_upkeep_deletes_sign_in_records_after_30_days(db: PgPool) {
    let site = Site::new(db);
    let (ana, _) = site.person(ANA).await;
    exec(
        &site.db,
        &format!(
            "INSERT INTO email_codes (email, code_hash, expires_at, created_at, purpose) VALUES
                 ('old@example.test', decode('01', 'hex'), now() - interval '31 days', now() - interval '31 days', 'sign_in'),
                 ('old@example.test', decode('02', 'hex'), now() - interval '31 days', now() - interval '31 days', 'delete'),
                 ('new@example.test', decode('03', 'hex'), now() - interval '29 days', now() - interval '29 days', 'sign_in');
             INSERT INTO device_authorizations (client_id, device_code_hash, user_code, interval_seconds, expires_at, account_id)
             VALUES ('buwiz-desktop', decode('a1', 'hex'), 'BCDF-GHJ1', 5, now() - interval '31 days', '{ana}'),
                    ('buwiz-desktop', decode('a2', 'hex'), 'BCDF-GHJ2', 5, now() - interval '29 days', '{ana}'),
                    ('buwiz-desktop', decode('a3', 'hex'), 'BCDF-GHJ3', 5, now() + interval '5 minutes', NULL);
             INSERT INTO oauth_clients (client_id, client_name, redirect_uris)
             VALUES ('agent_test', 'Test agent', ARRAY['https://agent.example.test/cb']);
             INSERT INTO oauth_codes (code_hash, client_id, account_id, redirect_uri, code_challenge, scope, expires_at)
             VALUES (decode('b1', 'hex'), 'agent_test', '{ana}', 'https://agent.example.test/cb', 'c', 'mcp', now() - interval '31 days'),
                    (decode('b2', 'hex'), 'agent_test', '{ana}', 'https://agent.example.test/cb', 'c', 'mcp', now() - interval '29 days');"
        ),
    )
    .await;
    buwiz_cloud::account::accounts::upkeep(&site.db).await.unwrap();
    let left: Vec<String> = sqlx::query_scalar("SELECT email FROM email_codes").fetch_all(&site.db).await.unwrap();
    assert_eq!(left, ["new@example.test"]);
    let left: Vec<String> = sqlx::query_scalar("SELECT user_code FROM device_authorizations ORDER BY user_code")
        .fetch_all(&site.db)
        .await
        .unwrap();
    assert_eq!(left, ["BCDF-GHJ2", "BCDF-GHJ3"]);
    assert_eq!(count(&site.db, "SELECT count(*) FROM oauth_codes").await, 1);
    // The account and its sessions are not sign-in records.
    assert!(exists(&site.db, ana).await);
    assert_eq!(live_sessions(&site.db, ana).await, 1);
}
