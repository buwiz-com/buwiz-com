//! Migrations 11 to 23 on a database at migration 10 with a row in every
//! claim status, files, readings, audit lines, tickets and returns: the
//! rows survive and read as before, and going back down to 10 leaves a
//! database the release before this one starts on, every return kept.
//! Migration 13 (test TINs) goes back to 12 on its own, 14 (BIR uptime) to
//! 13, keeping its history in the archive for the way up, 15 (fixed
//! sign-in codes) to 14, 16 (account deletion) to 15, 17 (in-app
//! sign-in) to 16, and 18 (one device row per install) to 17. Migration
//! 20 (an index on devices' sign-ins) and the server's migration
//! connection ([`buwiz_cloud::migrate`]) fail, rather than wait, behind a
//! long transaction, and recover as docs/deploy.md says. Migration 21
//! (Sign in with Buwiz) goes back to 20, and fails behind a long
//! transaction and recovers too; 22 (validating 21's checks) goes back to
//! 21, runs beside open writes, and fails behind a SHARE lock and
//! recovers. Migration 23 (reserved test roots) goes back to 22, and 24
//! (audit lines from the app) to 23, refusing while one exists.
//! Each test gets its own empty database (`migrations = false`).

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use buwiz_cloud::MIGRATOR;
use serde_json::Value;
use sqlx::PgPool;
use sqlx::migrate::Migrator;
use uuid::Uuid;

/// The migrations the release before this one embeds: 1 to 10.
fn release_10() -> Migrator {
    Migrator {
        migrations: Cow::Owned(MIGRATOR.iter().filter(|migration| migration.version <= 10).cloned().collect()),
        ..Migrator::DEFAULT
    }
}

/// The rows written at migration 10.
#[derive(Debug)]
struct Seeded {
    ana: Uuid,
    bob: Uuid,
    pending: Uuid,
    needs_fix: Uuid,
    approved: Uuid,
    rejected: Uuid,
    withdrawn: Uuid,
    removed: Uuid,
    revoked: Uuid,
    device: Uuid,
}

async fn exec(db: &PgPool, sql: &str) {
    sqlx::raw_sql(sql).execute(db).await.unwrap_or_else(|error| panic!("{error}: {sql}"));
}

async fn id(db: &PgPool, sql: &str) -> Uuid {
    sqlx::query_scalar(sql).fetch_one(db).await.unwrap_or_else(|error| panic!("{error}: {sql}"))
}

/// Accounts, a claim in each of the seven statuses migration 10 knows, the
/// files and readings of the open ones, audit lines, an app sign-in with a
/// device, a ticket and a sent return on the active TIN, and a v1 return
/// waiting to be sent.
async fn seed_at_10(db: &PgPool) -> Seeded {
    let ana = id(db, "INSERT INTO accounts (email) VALUES ('ana@example.test') RETURNING id").await;
    let bob = id(db, "INSERT INTO accounts (email) VALUES ('bob@example.test') RETURNING id").await;
    let staff = id(db, "INSERT INTO accounts (email) VALUES ('staff@example.test') RETURNING id").await;
    let claim = |root: &str, status: &str, extra_columns: &str, extra_values: &str, account: Uuid| {
        format!(
            "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status{extra_columns})
             VALUES ('{account}', 'personal', '{root}', '00000', 'SYNTHETIC TAXPAYER', '{status}'{extra_values})
             RETURNING id"
        )
    };
    let pending = id(db, &claim("000123451", "pending", "", "", ana)).await;
    let needs_fix = id(
        db,
        &claim(
            "000123452",
            "needs_fix",
            ", fix_reasons, fix_note, sent_back_at, sent_back_by",
            &format!(", ARRAY['blurry'], 'Retake it in daylight.', now(), '{staff}'"),
            ana,
        ),
    )
    .await;
    let approved = id(
        db,
        &claim(
            "000123453",
            "approved",
            ", decided_at, decided_by",
            &format!(", now() - interval '3 days', '{staff}'"),
            ana,
        ),
    )
    .await;
    let rejected = id(
        db,
        &claim(
            "000123454",
            "rejected",
            ", decided_at, decided_by, decision_note",
            &format!(", now(), '{staff}', 'Another account owns it.'"),
            bob,
        ),
    )
    .await;
    let withdrawn = id(db, &claim("000123455", "withdrawn", ", decided_at", ", now()", bob)).await;
    let removed = id(
        db,
        &claim(
            "000123456",
            "removed",
            ", decided_at, decided_by, removed_at, removed_by",
            &format!(", now() - interval '9 days', '{staff}', now(), '{bob}'"),
            bob,
        ),
    )
    .await;
    let revoked = id(
        db,
        &claim(
            "000123457",
            "revoked",
            ", decided_at, decided_by, removed_at, removed_by, removal_note",
            &format!(", now() - interval '9 days', '{staff}', now(), '{staff}', 'Ended by staff.'"),
            bob,
        ),
    )
    .await;

    // Files of the open claims, one of them read; a purged file of the
    // decided one.
    for (claim, kind) in [(pending, "tin_id"), (pending, "selfie"), (needs_fix, "tin_id")] {
        exec(
            db,
            &format!(
                "INSERT INTO claim_documents (claim_id, kind, filename, content_type, size_bytes, sha256, nonce, ciphertext)
                 VALUES ('{claim}', '{kind}', '{kind}.png', 'image/png', 5, decode('00', 'hex'), decode('01', 'hex'), decode('02', 'hex'))"
            ),
        )
        .await;
    }
    exec(
        db,
        &format!(
            "INSERT INTO claim_documents (claim_id, kind, filename, content_type, size_bytes, sha256, purged_at)
             VALUES ('{approved}', 'tin_id', 'card.png', 'image/png', 5, decode('00', 'hex'), now())"
        ),
    )
    .await;
    exec(
        db,
        &format!(
            "INSERT INTO document_readings (id, document_id, status, engine, fields, confidence, tin_match, name_match, read_at)
             SELECT gen_random_uuid(), id, 'read', 'paddle', '{{\"tin\":\"000123451\"}}', '{{\"tin\":0.97}}', 'match', 'partial', now()
             FROM claim_documents WHERE claim_id = '{pending}' AND kind = 'tin_id'"
        ),
    )
    .await;
    exec(
        db,
        &format!(
            "INSERT INTO claim_audit (claim_id, account_id, tin_masked, action, claim_status, document_kind, actor_id, actor_email, reason)
             VALUES ('{pending}', '{ana}', '***-***-451-00000', 'file_replaced', 'pending', 'tin_id', '{ana}', 'ana@example.test', NULL),
                    ('{revoked}', '{bob}', '***-***-457-00000', 'revoked', 'approved', NULL, '{staff}', 'staff@example.test', 'Ended by staff.'),
                    ('{}', '{bob}', '***-***-458-00000', 'deleted', 'pending', NULL, '{bob}', 'bob@example.test', NULL)",
            Uuid::new_v4()
        ),
    )
    .await;

    // The active TIN's sign-in, device, ticket, sent return and a v1 return
    // still waiting.
    let grant = id(
        db,
        &format!("INSERT INTO oauth_grants (account_id, client_id) VALUES ('{ana}', 'buwiz-desktop') RETURNING id"),
    )
    .await;
    let device = id(
        db,
        &format!(
            "INSERT INTO devices (account_id, grant_id, public_key, name)
             VALUES ('{ana}', '{grant}', decode(repeat('ab', 32), 'hex'), 'Ana''s Mac') RETURNING id"
        ),
    )
    .await;
    let jti = Uuid::new_v4();
    let sent = id(
        db,
        &format!(
            "INSERT INTO submissions (account_id, claim_id, form_type, period, filename, xml_sha256, status, mode,
                 receipt_jws, sent_at, purged_at, ticket_jti, device_id)
             VALUES ('{ana}', '{approved}', '1701Qv2018', '2026Q1', '00012345300000-1701Qv2018-2026Q1#ana@example.test#.xml',
                 decode('aa', 'hex'), 'sent', 'live', 'head.body.sig', now(), now(), '{jti}', '{device}')
             RETURNING id"
        ),
    )
    .await;
    exec(
        db,
        &format!(
            "INSERT INTO tickets (jti, account_id, device_id, claim_id, form_type, period, payload_sha256, email,
                 idempotency_key, jws, expires_at, submission_id, used_at)
             VALUES ('{jti}', '{ana}', '{device}', '{approved}', '1701Qv2018', '2026Q1', decode('aa', 'hex'),
                 'ana@example.test', 'k1', 'h.c.s', now() + interval '5 minutes', '{sent}', now())"
        ),
    )
    .await;
    exec(
        db,
        &format!(
            "INSERT INTO submissions (account_id, claim_id, idempotency_key, form_type, period, filename, xml_sha256,
                 xml_nonce, xml_ciphertext, status)
             VALUES ('{ana}', '{approved}', 'v1-k', '1601Cv2018', '062026', '00012345300000-1601Cv2018-062026#ana@example.test#.xml',
                 decode('bb', 'hex'), decode('01', 'hex'), decode('02', 'hex'), 'ready')"
        ),
    )
    .await;
    // A return sent before the TIN was removed stays with that claim.
    exec(
        db,
        &format!(
            "INSERT INTO submissions (account_id, claim_id, form_type, period, filename, xml_sha256, status, sent_at)
             VALUES ('{bob}', '{removed}', '1701Qv2018', '2025Q4', '00012345600000-1701Qv2018-2025Q4#bob@example.test#.xml',
                 decode('cc', 'hex'), 'sent', now())"
        ),
    )
    .await;
    Seeded { ana, bob, pending, needs_fix, approved, rejected, withdrawn, removed, revoked, device }
}

/// Every table, column, constraint, index, trigger and function of the
/// public schema, one line each.
async fn schema(db: &PgPool) -> BTreeSet<String> {
    let mut lines: Vec<String> = sqlx::query_scalar(
        "SELECT format('column %s.%s %s null=%s default=%s', table_name, column_name, data_type, is_nullable,
                       coalesce(column_default, '-'))
         FROM information_schema.columns WHERE table_schema = 'public'",
    )
    .fetch_all(db)
    .await
    .unwrap();
    for sql in [
        "SELECT format('constraint %s %s %s', c.conrelid::regclass, c.conname, pg_get_constraintdef(c.oid))
         FROM pg_constraint c JOIN pg_namespace n ON n.oid = c.connamespace WHERE n.nspname = 'public'",
        "SELECT format('index %s', indexdef) FROM pg_indexes WHERE schemaname = 'public'",
        "SELECT format('trigger %s', pg_get_triggerdef(t.oid)) FROM pg_trigger t
         JOIN pg_class c ON c.oid = t.tgrelid JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'public' AND NOT t.tgisinternal",
        "SELECT format('function %s %s', p.proname, md5(p.prosrc)) FROM pg_proc p
         JOIN pg_namespace n ON n.oid = p.pronamespace WHERE n.nspname = 'public'",
    ] {
        lines.extend(sqlx::query_scalar::<_, String>(sql).fetch_all(db).await.unwrap());
    }
    lines.into_iter().collect()
}

/// The tables whose rows must outlive both directions.
const TABLES: &[&str] = &[
    "accounts",
    "tin_claims",
    "claim_documents",
    "document_readings",
    "claim_audit",
    "oauth_grants",
    "devices",
    "tickets",
    "submissions",
];

/// Each table's rows by id (the primary key's text), as JSON objects.
async fn rows(db: &PgPool) -> BTreeMap<String, BTreeMap<String, Value>> {
    let mut all = BTreeMap::new();
    for table in TABLES {
        let key = if *table == "tickets" { "jti" } else { "id" };
        let rows: Vec<(String, Value)> =
            sqlx::query_as(&format!("SELECT {key}::text, to_jsonb(t) FROM {table} t")).fetch_all(db).await.unwrap();
        all.insert((*table).to_owned(), rows.into_iter().collect());
    }
    all
}

/// `rows` with only the columns migration 10 had.
fn only_columns_at_10(
    rows: &BTreeMap<String, BTreeMap<String, Value>>,
    at_10: &BTreeMap<String, BTreeMap<String, Value>>,
) -> BTreeMap<String, BTreeMap<String, Value>> {
    rows.iter()
        .map(|(table, rows)| {
            let columns: BTreeSet<&String> =
                at_10[table].values().next().map(|row| row.as_object().unwrap().keys().collect()).unwrap_or_default();
            let rows = rows
                .iter()
                .map(|(id, row)| {
                    let mut row = row.as_object().unwrap().clone();
                    row.retain(|column, _| columns.contains(column));
                    (id.clone(), Value::Object(row))
                })
                .collect();
            (table.clone(), rows)
        })
        .collect()
}

async fn versions(db: &PgPool) -> Vec<i64> {
    sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version").fetch_all(db).await.unwrap()
}

/// `sql` breaks a NOT NULL column: a writer must name the value itself.
async fn fails_not_null(db: &PgPool, sql: &str) {
    match sqlx::raw_sql(sql).execute(db).await {
        Err(sqlx::Error::Database(error)) => assert_eq!(error.code().as_deref(), Some("23502"), "{sql}: {error}"),
        other => panic!("{sql} should break a NOT NULL column: {other:?}"),
    }
}

async fn fails_with(db: &PgPool, sql: &str, constraint: &str) {
    match sqlx::raw_sql(sql).execute(db).await {
        Err(sqlx::Error::Database(error)) => assert_eq!(error.constraint(), Some(constraint), "{sql}"),
        other => panic!("{sql} should break {constraint}: {other:?}"),
    }
}

#[sqlx::test(migrations = false)]
async fn rows_from_migration_10_survive(db: PgPool) {
    release_10().run(&db).await.unwrap();
    let seeded = seed_at_10(&db).await;
    let before = rows(&db).await;

    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());
    let after = rows(&db).await;
    assert_eq!(only_columns_at_10(&after, &before), before, "no row or old column changed");

    // Every claim came with papers, and every TIN that was ever active was
    // made active by staff.
    let claims: Vec<(Uuid, String, Option<String>, bool, bool)> = sqlx::query_as(
        "SELECT id, status, verified_by, submitted_at = created_at, paused_at IS NULL AND auto_check IS NULL
         FROM tin_claims",
    )
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(claims.len(), 7);
    for (claim, status, verified_by, submitted_then, untouched) in &claims {
        let expected = matches!(status.as_str(), "approved" | "removed" | "revoked").then(|| "staff".to_owned());
        assert_eq!(verified_by, &expected, "{status} {claim}");
        assert!(*submitted_then && *untouched, "{status}");
    }
    let statuses: BTreeSet<&str> = claims.iter().map(|claim| claim.1.as_str()).collect();
    assert_eq!(
        statuses,
        BTreeSet::from(["pending", "needs_fix", "approved", "rejected", "withdrawn", "removed", "revoked"])
    );
    let accounts: Vec<(Option<String>, bool, Option<String>)> =
        sqlx::query_as("SELECT role, test_account, ban_reason FROM accounts").fetch_all(&db).await.unwrap();
    assert!(accounts.iter().all(|account| *account == (None, false, None)), "{accounts:?}");
    let sources: Vec<String> = sqlx::query_scalar("SELECT source FROM claim_audit").fetch_all(&db).await.unwrap();
    assert_eq!(sources, ["web", "web", "web"]);
    // Lines from before name no claim kind; new ones name a known one.
    let kinds: i64 = sqlx::query_scalar("SELECT count(*) FROM claim_audit WHERE claim_kind IS NOT NULL")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(kinds, 0);
    fails_with(
        &db,
        &format!("UPDATE claim_audit SET claim_kind = 'shop' WHERE account_id = '{}'", seeded.ana),
        "claim_audit_claim_kind_check",
    )
    .await;

    let indexes: BTreeSet<String> =
        sqlx::query_scalar("SELECT indexname::text FROM pg_indexes WHERE tablename = 'tin_claims'")
            .fetch_all(&db)
            .await
            .unwrap()
            .into_iter()
            .collect();
    for index in ["tin_claims_one_owner", "tin_claims_one_live", "tin_claims_tin"] {
        assert!(indexes.contains(index), "{index}: {indexes:?}");
    }
    assert!(!indexes.contains("tin_claims_one_open"));

    // What the new checks hold from now on.
    let (ana, bob) = (seeded.ana, seeded.bob);
    let insert = |account: Uuid, root: &str, status: &str, extra_columns: &str, extra_values: &str| {
        format!(
            "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status{extra_columns})
             VALUES ('{account}', 'personal', '{root}', '00000', 'SYNTHETIC TAXPAYER', '{status}'{extra_values})"
        )
    };
    fails_with(&db, &insert(bob, "000123460", "approved", "", ""), "tin_claims_decided_check").await;
    fails_with(&db, &insert(bob, "000123460", "approved", ", decided_at", ", now()"), "tin_claims_verified_check")
        .await;
    fails_with(&db, &insert(bob, "000123460", "unverified", ", submitted_at", ", now()"), "tin_claims_submitted_check")
        .await;
    fails_with(
        &db,
        &insert(bob, "000123460", "pending", ", paused_at, pause_reason", ", now(), 'x'"),
        "tin_claims_paused_check",
    )
    .await;
    fails_with(
        &db,
        &format!("UPDATE tin_claims SET paused_at = now() WHERE id = '{}'", seeded.approved),
        "tin_claims_paused_check",
    )
    .await;
    fails_with(
        &db,
        &format!("UPDATE tin_claims SET auto_confirmed_at = now() WHERE id = '{}'", seeded.approved),
        "tin_claims_auto_confirmed_check",
    )
    .await;
    // A pause names who paused it and why: a staff member, or Buwiz itself
    // for an automatic activation nobody confirmed. A blank reason is none.
    fails_with(
        &db,
        &format!(
            "UPDATE tin_claims SET paused_at = now(), pause_reason = 'Checking.' WHERE id = '{}'",
            seeded.approved
        ),
        "tin_claims_paused_check",
    )
    .await;
    fails_with(
        &db,
        &format!(
            "UPDATE tin_claims SET paused_at = now(), paused_by = '{bob}', pause_reason = '  ' WHERE id = '{}'",
            seeded.approved
        ),
        "tin_claims_paused_check",
    )
    .await;
    fails_with(
        &db,
        &format!("UPDATE tin_claims SET pause_reason = 'Checking.' WHERE id = '{}'", seeded.approved),
        "tin_claims_paused_check",
    )
    .await;
    // Tabs, newlines and Unicode spaces are blank too, as the code's `trim`
    // sees them.
    for blank in [r"E'\t\n'", r"E'\r\u00a0\u3000 '"] {
        fails_with(
            &db,
            &format!(
                "UPDATE tin_claims SET paused_at = now(), paused_by = '{bob}', pause_reason = {blank} WHERE id = '{}'",
                seeded.approved
            ),
            "tin_claims_paused_check",
        )
        .await;
        fails_with(
            &db,
            &format!(
                "UPDATE accounts SET banned_at = now(), banned_by = '{ana}', ban_reason = {blank} WHERE id = '{bob}'"
            ),
            "accounts_ban_check",
        )
        .await;
    }
    let auto = id(
        &db,
        &format!(
            "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status, decided_at, verified_by,
                 paused_at, pause_reason)
             VALUES ('{bob}', 'personal', '000123463', '00000', 'SYNTHETIC TAXPAYER', 'approved', now(), 'auto',
                 now(), 'Not confirmed by a person within 30 days.') RETURNING id"
        ),
    )
    .await;
    fails_with(
        &db,
        &format!("UPDATE tin_claims SET auto_confirmed_at = now(), auto_confirmed_by = '{ana}' WHERE id = '{auto}'"),
        "tin_claims_paused_check",
    )
    .await;
    exec(&db, &format!("DELETE FROM tin_claims WHERE id = '{auto}'")).await;
    fails_with(&db, &insert(bob, "000123460", "paused", "", ""), "tin_claims_status_check").await;
    // Ana's open claim on a TIN is her one live row for it, as is her
    // active one.
    fails_with(&db, &insert(ana, "000123451", "unverified", "", ""), "tin_claims_one_live").await;
    fails_with(&db, &insert(ana, "000123453", "unverified", "", ""), "tin_claims_one_live").await;
    // Anyone else may add ana's active TIN without papers; only one owns it.
    exec(&db, &insert(bob, "000123453", "unverified", "", "")).await;
    let carl = id(&db, "INSERT INTO accounts (email) VALUES ('carl@example.test') RETURNING id").await;
    exec(&db, &insert(carl, "000123453", "unverified", "", "")).await;
    let dan = id(&db, "INSERT INTO accounts (email) VALUES ('dan@example.test') RETURNING id").await;
    fails_with(
        &db,
        &insert(dan, "000123453", "approved", ", decided_at, verified_by", ", now(), 'admin'"),
        "tin_claims_one_owner",
    )
    .await;
    fails_with(
        &db,
        &insert(bob, "000123459", "approved", ", decided_at, verified_by", ", now(), 'anyone'"),
        "tin_claims_verified_by_check",
    )
    .await;
    exec(
        &db,
        &format!(
            "UPDATE tin_claims SET paused_at = now(), paused_by = '{bob}', pause_reason = 'Checking a complaint.'
             WHERE id = '{}'",
            seeded.approved
        ),
    )
    .await;
    exec(
        &db,
        &format!(
            "INSERT INTO claim_audit (claim_id, account_id, tin_masked, action, claim_status, actor_id, actor_email, reason, source)
             VALUES ('{}', '{ana}', '***-***-453-00000', 'paused', 'approved', '{bob}', 'bob@example.test', 'Checking.', 'web')",
            seeded.approved
        ),
    )
    .await;
    fails_with(&db, &format!("UPDATE accounts SET banned_at = now() WHERE id = '{bob}'"), "accounts_ban_check").await;
    fails_with(
        &db,
        &format!("UPDATE accounts SET banned_at = now(), ban_reason = 'Abuse.' WHERE id = '{bob}'"),
        "accounts_ban_check",
    )
    .await;
    fails_with(
        &db,
        &format!("UPDATE accounts SET banned_at = now(), banned_by = '{ana}', ban_reason = ' ' WHERE id = '{bob}'"),
        "accounts_ban_check",
    )
    .await;
    // Every audit writer says where its line came from.
    fails_not_null(
        &db,
        &format!(
            "INSERT INTO claim_audit (claim_id, account_id, tin_masked, action, claim_status, actor_id, actor_email)
             VALUES ('{}', '{ana}', '***-***-453-00000', 'paused', 'approved', '{bob}', 'bob@example.test')",
            seeded.approved
        ),
    )
    .await;
    fails_not_null(
        &db,
        &format!(
            "INSERT INTO account_audit (account_id, action, actor_id, actor_email)
             VALUES ('{bob}', 'banned', '{ana}', 'ana@example.test')"
        ),
    )
    .await;
    fails_with(&db, &format!("UPDATE accounts SET role = 'owner' WHERE id = '{bob}'"), "accounts_role_check").await;
    exec(
        &db,
        &format!(
            "UPDATE accounts SET role = 'support', test_account = true WHERE id = '{bob}';
             INSERT INTO account_audit (account_id, action, old_value, new_value, actor_id, actor_email, source, reason)
             VALUES ('{bob}', 'role_changed', NULL, 'support', '{ana}', 'ana@example.test', 'web', 'Helps with the queue.');
             INSERT INTO seed_runs (id, actor_id, actor_email, os_user, host_name, target, file_sha256, plan_digest, summary)
             VALUES (gen_random_uuid(), '{ana}', 'ana@example.test', 'ops', 'box', 'db:5432/buwiz', decode('00', 'hex'), 'd', '{{}}')"
        ),
    )
    .await;
}

#[sqlx::test(migrations = false)]
async fn going_back_to_migration_10_keeps_every_return(db: PgPool) {
    let old = release_10();
    old.run(&db).await.unwrap();
    let seeded = seed_at_10(&db).await;
    let schema_10 = schema(&db).await;
    let rows_10 = rows(&db).await;
    MIGRATOR.run(&db).await.unwrap();
    assert_ne!(schema(&db).await, schema_10);

    // What happens on the new release before someone rolls it back: a TIN
    // added without papers by ana, and by bob on ana's active TIN; ana's
    // TIN paused and its line in the audit; an admin activating one of
    // bob's TINs without papers; a return sent meanwhile; a role, a ban, an
    // account audit line and a seed run.
    let (ana, bob) = (seeded.ana, seeded.bob);
    exec(
        &db,
        &format!(
            "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status)
             VALUES ('{ana}', 'personal', '000123461', '00000', 'SYNTHETIC TAXPAYER', 'unverified'),
                    ('{bob}', 'personal', '000123453', '00000', 'SYNTHETIC TAXPAYER', 'unverified');
             UPDATE tin_claims SET paused_at = now(), paused_by = '{bob}', pause_reason = 'Checking.' WHERE id = '{approved}';
             INSERT INTO claim_audit (claim_id, account_id, tin_masked, action, claim_status, actor_id, actor_email, reason, source)
             VALUES ('{approved}', '{ana}', '***-***-453-00000', 'paused', 'approved', '{bob}', 'bob@example.test', 'Checking.', 'web');
             UPDATE accounts SET role = 'admin', test_account = true WHERE id = '{bob}';
             UPDATE accounts SET banned_at = now(), banned_by = '{bob}', ban_reason = 'Abuse.' WHERE id = '{ana}';
             INSERT INTO account_audit (account_id, action, actor_id, actor_email, source, reason)
             VALUES ('{ana}', 'banned', '{bob}', 'bob@example.test', 'web', 'Abuse.');
             INSERT INTO seed_runs (id, actor_id, actor_email, os_user, host_name, target, file_sha256, plan_digest, summary)
             VALUES (gen_random_uuid(), '{bob}', 'bob@example.test', 'ops', 'box', 'db:5432/buwiz', decode('00', 'hex'), 'd', '{{}}')",
            approved = seeded.approved
        ),
    )
    .await;
    let activated = id(
        &db,
        &format!(
            "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status, decided_at, decided_by,
                 verified_by)
             VALUES ('{bob}', 'personal', '000123462', '00000', 'SYNTHETIC TAXPAYER', 'approved', now(), '{bob}', 'admin')
             RETURNING id"
        ),
    )
    .await;
    // An automatic activation a person confirmed: no reviewer named.
    let confirmed_auto = id(
        &db,
        &format!(
            "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status, decided_at, verified_by,
                 auto_confirmed_at, auto_confirmed_by)
             VALUES ('{bob}', 'personal', '000123466', '00000', 'SYNTHETIC TAXPAYER', 'approved', now(), 'auto', now(), '{ana}')
             RETURNING id"
        ),
    )
    .await;
    // The seed tool names who ran it as the reviewer.
    let seeded_tin = id(
        &db,
        &format!(
            "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status, decided_at, decided_by,
                 verified_by)
             VALUES ('{bob}', 'personal', '000123464', '00000', 'SYNTHETIC TAXPAYER', 'approved', now(), '{bob}', 'seed')
             RETURNING id"
        ),
    )
    .await;
    let sent_meanwhile = id(
        &db,
        &format!(
            "INSERT INTO submissions (account_id, claim_id, form_type, period, filename, xml_sha256, status, sent_at)
             VALUES ('{bob}', '{activated}', '1701Qv2018', '2026Q2', '00012346200000-1701Qv2018-2026Q2#bob@example.test#.xml',
                 decode('dd', 'hex'), 'sent', now()) RETURNING id"
        ),
    )
    .await;

    // Review decisions and an activation without papers have audit lines
    // of their own.
    exec(
        &db,
        &format!(
            "INSERT INTO claim_audit (claim_id, account_id, tin_masked, action, claim_status, actor_id, actor_email, reason, source)
             VALUES ('{activated}', '{bob}', '***-***-462-00000', 'activated', 'unverified', '{bob}', 'bob@example.test', 'Known to us.', 'web'),
                    ('{pending}', '{ana}', '***-***-451-00000', 'approved', 'pending', '{bob}', 'bob@example.test', NULL, 'web'),
                    ('{pending}', '{ana}', '***-***-451-00000', 'rejected', 'pending', '{bob}', 'bob@example.test', 'No.', 'web'),
                    ('{pending}', '{ana}', '***-***-451-00000', 'sent_back', 'pending', '{bob}', 'bob@example.test', NULL, 'web'),
                    ('{seeded_tin}', '{bob}', '***-***-464-00000', 'created', 'new', '{bob}', 'bob@example.test', 'seed run', 'seed')",
            pending = seeded.pending
        ),
    )
    .await;

    // 13 goes back on its own, and stays back: from 13 the rollback is step
    // by step. Each refusal below leaves the database at 12, which the
    // release before test TINs runs on.
    MIGRATOR.undo(&db, 12).await.unwrap();
    assert_eq!(versions(&db).await, (1..=12).collect::<Vec<_>>());

    // From 12, a rollback goes all the way to 10 or changes nothing. 0012's
    // down file refuses first for every reason 0011's would, so each
    // refusal leaves the database at 12 with nothing dropped.
    let untouched = at_12(&db).await;
    let refuses = |words: &'static str| {
        let db = db.clone();
        let untouched = untouched.clone();
        async move {
            let refused = MIGRATOR.undo(&db, 10).await.unwrap_err().to_string();
            assert!(refused.contains(words), "{refused}");
            assert_eq!(at_12(&db).await, untouched, "{words}");
        }
    };
    refuses("account(s) are banned").await;
    exec(&db, "UPDATE accounts SET banned_at = NULL, banned_by = NULL, ban_reason = NULL").await;
    refuses("TIN(s) are paused").await;
    let paused: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tin_claims WHERE paused_at IS NOT NULL").fetch_one(&db).await.unwrap();
    assert_eq!(paused, 1);
    exec(&db, "UPDATE tin_claims SET paused_at = NULL, paused_by = NULL, pause_reason = NULL").await;
    let unverified = id(&db, "SELECT id FROM tin_claims WHERE status = 'unverified' LIMIT 1").await;
    exec(
        &db,
        &format!(
            "INSERT INTO claim_documents (claim_id, kind, filename, content_type, size_bytes, sha256, nonce, ciphertext)
             VALUES ('{unverified}', 'tin_id', 'tin.png', 'image/png', 5, decode('00', 'hex'), decode('01', 'hex'), decode('02', 'hex'))"
        ),
    )
    .await;
    refuses("TINs saved without papers have files").await;
    exec(&db, &format!("DELETE FROM claim_documents WHERE claim_id = '{unverified}'")).await;
    let auto = id(
        &db,
        &format!(
            "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status, decided_at, verified_by)
             VALUES ('{bob}', 'personal', '000123465', '00000', 'SYNTHETIC TAXPAYER', 'removed', now(), 'auto') RETURNING id"
        ),
    )
    .await;
    refuses("Confirm each on the \"Activated automatically\" tab first, revoked and removed ones too").await;
    exec(&db, &format!("DELETE FROM tin_claims WHERE id = '{auto}'")).await;
    // Bob is an Admin by stored role: the release before this one would
    // not know him.
    refuses("have a stored staff role").await;
    // Run by hand, each down file refuses a pause on its own too.
    for (down, words) in [
        (include_str!("../migrations/0012_staff_roles.down.sql"), "TIN(s) are paused"),
        (include_str!("../migrations/0011_tin_states.down.sql"), "TIN(s) are paused"),
    ] {
        exec(&db, "UPDATE tin_claims SET paused_at = now(), paused_by = account_id, pause_reason = 'Again.' WHERE id = (SELECT id FROM tin_claims WHERE status = 'approved' AND paused_at IS NULL LIMIT 1)").await;
        let mut tx = db.begin().await.unwrap();
        let refused = sqlx::raw_sql(down).execute(&mut *tx).await.unwrap_err().to_string();
        assert!(refused.contains(words), "{refused}");
        drop(tx);
        exec(&db, "UPDATE tin_claims SET paused_at = NULL, paused_by = NULL, pause_reason = NULL").await;
    }
    assert_eq!(at_12(&db).await, untouched);
    exec(&db, "UPDATE accounts SET role = NULL").await;

    // Back to 10 the way sqlx reverts, then the release before this one
    // starts on it. The audit lines 10 cannot hold, the account history
    // and the seed runs are kept in `rollback_archive`.
    MIGRATOR.undo(&db, 10).await.unwrap();
    assert_rolled_back(
        &db,
        &old,
        &schema_10,
        &rows_10,
        &[activated, seeded_tin, confirmed_auto],
        sent_meanwhile,
        &seeded,
    )
    .await;
    assert_eq!(
        archived(&db, "claim_audit", "action").await,
        ["activated", "approved", "created", "paused", "rejected", "sent_back"]
    );
    assert_eq!(archived(&db, "account_audit", "action").await, ["banned"]);
    assert_eq!(archived(&db, "seed_runs", "actor_email").await, ["bob@example.test"]);
    let reason: Option<String> =
        sqlx::query_scalar("SELECT reason FROM rollback_archive.claim_audit WHERE action = 'activated'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(reason.as_deref(), Some("Known to us."));

    // The new release can come back on it, and the down files also work run
    // by hand (as psql runs them), newest first.
    MIGRATOR.run(&db).await.unwrap();
    let verified_by = |claim: Uuid| {
        let db = db.clone();
        async move {
            sqlx::query_scalar::<_, Option<String>>("SELECT verified_by FROM tin_claims WHERE id = $1")
                .bind(claim)
                .fetch_one(&db)
                .await
                .unwrap()
        }
    };
    // How each became active was lost with the rollback. A TIN that names
    // a reviewer (the admin who activated it, the seed's `--as`) counts as
    // staff's; one that does not (a confirmed automatic one) is not on
    // record.
    assert_eq!(verified_by(activated).await.as_deref(), Some("staff"));
    assert_eq!(verified_by(confirmed_auto).await.as_deref(), Some("unknown"));
    assert_eq!(verified_by(seeded_tin).await.as_deref(), Some("staff"));
    assert_eq!(verified_by(seeded.approved).await.as_deref(), Some("staff"));
    assert_eq!(verified_by(seeded.revoked).await.as_deref(), Some("staff"));
    // A stored role again: by hand, the operator drops it on purpose.
    exec(
        &db,
        &format!(
            "UPDATE accounts SET role = 'admin' WHERE id = '{bob}';
             INSERT INTO seed_runs (id, actor_id, actor_email, os_user, host_name, target, file_sha256, plan_digest, summary)
             VALUES (gen_random_uuid(), '{bob}', 'bob@example.test', 'ops', 'box', 'db:5432/buwiz', decode('01', 'hex'), 'd', '{{}}')"
        ),
    )
    .await;
    for (down, set) in [
        (include_str!("../migrations/0024_app_audit_source.down.sql"), ""),
        (include_str!("../migrations/0023_test_roots.down.sql"), ""),
        (include_str!("../migrations/0022_app_authorize_validate.down.sql"), ""),
        (include_str!("../migrations/0021_app_authorize.down.sql"), ""),
        (include_str!("../migrations/0020_devices_grant_index.down.sql"), ""),
        (include_str!("../migrations/0019_device_install_index.down.sql"), ""),
        (include_str!("../migrations/0018_device_installs.down.sql"), ""),
        (include_str!("../migrations/0017_app_sign_ins.down.sql"), ""),
        (include_str!("../migrations/0016_account_deletion.down.sql"), ""),
        (include_str!("../migrations/0015_fixed_codes.down.sql"), ""),
        (include_str!("../migrations/0014_bir_uptime.down.sql"), ""),
        (include_str!("../migrations/0013_test_tins.down.sql"), ""),
        (include_str!("../migrations/0012_staff_roles.down.sql"), "SET LOCAL buwiz.rollback_drop_roles = 'yes'"),
        (include_str!("../migrations/0011_tin_states.down.sql"), ""),
    ] {
        let mut tx = db.begin().await.unwrap();
        sqlx::raw_sql(set).execute(&mut *tx).await.unwrap();
        sqlx::raw_sql(down).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }
    assert_rolled_back(
        &db,
        &old,
        &schema_10,
        &rows_10,
        &[activated, seeded_tin, confirmed_auto],
        sent_meanwhile,
        &seeded,
    )
    .await;
    // A second rollback adds to the archive.
    assert_eq!(archived(&db, "seed_runs", "actor_email").await, ["bob@example.test", "bob@example.test"]);
}

/// What a refused rollback must leave as it was: the versions, the roles,
/// the test labels, the account history, the seed runs and the audit
/// lines.
async fn at_12(db: &PgPool) -> (Vec<i64>, i64, i64, i64, i64, i64) {
    let count = |sql: &'static str| {
        let db = db.clone();
        async move { sqlx::query_scalar::<_, i64>(sql).fetch_one(&db).await.unwrap() }
    };
    (
        versions(db).await,
        count("SELECT count(*) FROM accounts WHERE role IS NOT NULL").await,
        count("SELECT count(*) FROM accounts WHERE test_account").await,
        count("SELECT count(*) FROM account_audit").await,
        count("SELECT count(*) FROM seed_runs").await,
        count("SELECT count(*) FROM claim_audit").await,
    )
}

/// One column of a `rollback_archive` table, sorted.
async fn archived(db: &PgPool, table: &str, column: &str) -> Vec<String> {
    sqlx::query_scalar(&format!("SELECT {column}::text FROM rollback_archive.{table} ORDER BY 1"))
        .fetch_all(db)
        .await
        .unwrap()
}

/// 0012's down file makes 0011's checks word for word, so a rollback never
/// stops between the two.
#[test]
fn the_newer_down_file_makes_every_check_of_the_older_one() {
    let older = include_str!("../migrations/0011_tin_states.down.sql");
    let start = older.find("-- BEGIN checks of 0011").unwrap();
    let end = older.find("-- END checks of 0011").unwrap();
    let checks = &older[start..end];
    assert!(checks.contains("RAISE EXCEPTION"));
    assert!(include_str!("../migrations/0012_staff_roles.down.sql").contains(checks));
}

/// The database reads as at migration 10 plus what the new release added
/// that the old one can hold, and the old release's migrator accepts it.
async fn assert_rolled_back(
    db: &PgPool,
    old: &Migrator,
    schema_10: &BTreeSet<String>,
    rows_10: &BTreeMap<String, BTreeMap<String, Value>>,
    activated: &[Uuid],
    sent_meanwhile: Uuid,
    seeded: &Seeded,
) {
    assert_eq!(versions(db).await, (1..=10).collect::<Vec<_>>());
    let schema = schema(db).await;
    assert_eq!(
        schema.symmetric_difference(schema_10).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "the schema is migration 10's"
    );
    // What the release before this one does when it starts.
    old.run(db).await.unwrap();

    let mut rows = rows(db).await;
    // The TIN an admin activated and the return sent for it stay; nothing
    // the new release added can be told apart from what the old one writes.
    for claim in activated {
        let claim = *claim;
        let activated_row = rows.get_mut("tin_claims").unwrap().remove(&claim.to_string()).unwrap();
        assert_eq!(activated_row["status"], "approved");
    }
    let sent = rows.get_mut("submissions").unwrap().remove(&sent_meanwhile.to_string()).unwrap();
    assert_eq!((sent["status"].as_str(), sent["claim_id"].as_str()), (Some("sent"), Some(&*activated[0].to_string())));
    // Everything from migration 10 is there as it was: the TIN that was
    // paused (and resumed before the rollback) is active, the unverified
    // rows and the pause's audit line are gone, the role with its column.
    assert_eq!(&rows, rows_10);
    let approved = &rows["tin_claims"][&seeded.approved.to_string()];
    assert_eq!(approved["status"], "approved");
    for claim in [
        seeded.pending,
        seeded.needs_fix,
        seeded.approved,
        seeded.rejected,
        seeded.withdrawn,
        seeded.removed,
        seeded.revoked,
    ] {
        assert!(rows["tin_claims"].contains_key(&claim.to_string()));
    }
    assert!(rows["devices"].contains_key(&seeded.device.to_string()));
}

/// A transaction left open on the claims (a pre-flight `BEGIN` in a psql
/// session) makes start-up fail within seconds, saying why, instead of
/// hanging with the site down; once it ends, the migrations go in.
#[sqlx::test(migrations = false)]
async fn a_stray_open_transaction_fails_the_migration_instead_of_hanging(db: PgPool) {
    release_10().run(&db).await.unwrap();
    seed_at_10(&db).await;
    let mut stray = db.begin().await.unwrap();
    sqlx::query("SELECT count(*) FROM tin_claims").execute(&mut *stray).await.unwrap();
    let started = std::time::Instant::now();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(40), MIGRATOR.run(&db))
        .await
        .expect("the migration waited for the lock without end");
    let error = outcome.expect_err("the migration ran past an open transaction").to_string();
    assert!(error.contains("lock timeout"), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
    assert_eq!(versions(&db).await, (1..=10).collect::<Vec<_>>());
    stray.rollback().await.unwrap();
    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());
}

/// Migration 13: a test TIN (root 000-000-000) is only ever one an Admin or
/// the seed tool made active, active or ended (removed, revoked), and going
/// back to 12 refuses while any row
/// holds one, then drops the check.
#[sqlx::test(migrations = false)]
async fn test_tins_are_admin_rows_and_block_a_rollback(db: PgPool) {
    MIGRATOR.run(&db).await.unwrap();
    let ana = id(&db, "INSERT INTO accounts (email) VALUES ('ana@example.test') RETURNING id").await;
    let insert = |status: &str, extra: &str, values: &str| {
        format!(
            "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status{extra})
             VALUES ('{ana}', 'personal', '000000000', '00000', 'TEST TAXPAYER', '{status}'{values}) RETURNING id"
        )
    };
    for (status, extra, values) in [
        ("unverified", "", ""),
        ("pending", "", ""),
        ("approved", ", decided_at, verified_by", ", now(), 'staff'"),
        ("approved", ", decided_at, verified_by", ", now(), 'auto'"),
        // An Admin's or the seed's, but not active or ended from active.
        ("unverified", ", verified_by", ", 'admin'"),
        ("pending", ", verified_by", ", 'admin'"),
        ("needs_fix", ", verified_by", ", 'seed'"),
        ("rejected", ", verified_by", ", 'admin'"),
        ("withdrawn", ", verified_by", ", 'seed'"),
    ] {
        fails_with(&db, &insert(status, extra, values), "tin_claims_test_tin_check").await;
    }
    // Active or ended from active, an Admin's or the seed's.
    let mut branch = 0;
    for (status, by) in [("approved", "seed"), ("removed", "admin"), ("revoked", "seed")] {
        branch += 1;
        let row = insert(status, ", decided_at, verified_by", &format!(", now(), '{by}'"))
            .replace("'00000'", &format!("'{branch:05}'"));
        id(&db, &row).await;
    }
    exec(&db, "DELETE FROM tin_claims WHERE tin_root = '000000000'").await;
    let test_tin = id(&db, &insert("approved", ", decided_at, verified_by", ", now(), 'admin'")).await;
    // Nor can a test TIN's row be moved to another status.
    for status in ["pending", "unverified", "needs_fix", "rejected", "withdrawn"] {
        fails_with(
            &db,
            &format!("UPDATE tin_claims SET status = '{status}' WHERE id = '{test_tin}'"),
            "tin_claims_test_tin_check",
        )
        .await;
    }
    // A real TIN is as before.
    exec(
        &db,
        &format!(
            "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status)
             VALUES ('{ana}', 'personal', '000123453', '00000', 'SYNTHETIC TAXPAYER', 'unverified')"
        ),
    )
    .await;

    let refused = MIGRATOR.undo(&db, 12).await.unwrap_err().to_string();
    assert!(refused.contains("1 row(s) hold a test TIN (000-000-000)"), "{refused}");
    // 15 and 14 went back on their own first: they never refuse.
    assert_eq!(versions(&db).await, (1..=13).collect::<Vec<_>>());
    // Ended, it still blocks: the old release reads ended rows too.
    exec(&db, &format!("UPDATE tin_claims SET status = 'revoked', removed_at = now() WHERE id = '{test_tin}'")).await;
    assert!(MIGRATOR.undo(&db, 12).await.is_err());
    exec(&db, "DELETE FROM tin_claims WHERE tin_root = '000000000'").await;
    MIGRATOR.undo(&db, 12).await.unwrap();
    assert_eq!(versions(&db).await, (1..=12).collect::<Vec<_>>());
    let checks: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pg_constraint WHERE conname = 'tin_claims_test_tin_check'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(checks, 0);
    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());
}

/// The migrations of the release before BIR uptime: 1 to 13.
fn release_13() -> Migrator {
    Migrator {
        migrations: Cow::Owned(MIGRATOR.iter().filter(|migration| migration.version <= 13).cloned().collect()),
        ..Migrator::DEFAULT
    }
}

/// Counts of what migration 14's down file keeps: days, events, notes,
/// proofs, and the sign's `seq`.
async fn uptime_kept(db: &PgPool) -> (i64, i64, i64, i64, Option<i64>) {
    let count = |sql: &'static str| {
        let db = db.clone();
        async move { sqlx::query_scalar::<_, i64>(sql).fetch_one(&db).await.unwrap() }
    };
    (
        count("SELECT count(*) FROM bir_days").await,
        count("SELECT count(*) FROM bir_events").await,
        count("SELECT count(*) FROM bir_notes").await,
        count("SELECT count(*) FROM bir_proofs").await,
        sqlx::query_scalar("SELECT seq FROM bir_sign").fetch_optional(db).await.unwrap(),
    )
}

/// One `bir_tries` row: `values` are mode, outcome, stage, dispatcher,
/// upload, class, kind, touched_bir and counted.
fn try_row(values: &str) -> String {
    format!(
        "INSERT INTO bir_tries (started_at, ended_at, source, mode, outcome, stage, dispatcher, upload, failure_class,
             failure_kind, touched_bir, counted, admission, key_epoch, account_key)
         VALUES ('2026-11-11T07:12:00Z', '2026-11-11T07:13:00Z', 'relay', {values}, 'open', 1,
             decode('0102030405060708', 'hex'))"
    )
}

/// Migration 14: new tables only, whose checks keep a try's row to what the
/// record may hold. Going back to 13 never refuses, leaves 13's schema, and
/// keeps the day history, events, notes, proofs and the sign in
/// `rollback_archive`; the way up brings them back (a proof whose claim is
/// gone without it), a proof whose account is gone excepted, which stays in
/// the archive.
#[sqlx::test(migrations = false)]
async fn bir_uptime_goes_back_to_13_and_its_history_comes_back(db: PgPool) {
    release_13().run(&db).await.unwrap();
    let schema_13 = schema(&db).await;
    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());

    // What a try may hold: a live failure has a class and a kind, a lab
    // try neither and is never counted, times are to the minute, keys are
    // 8 bytes.
    exec(&db, &try_row("'live', 'failed', 'upload', 'ok', 'fail', 'bir', 'timeout', true, true")).await;
    exec(&db, &try_row("'live', 'sent', 'uploaded', 'ok', 'ok', NULL, NULL, true, true")).await;
    exec(&db, &try_row("'lab', 'failed', 'upload', 'none', 'fail', NULL, NULL, false, false")).await;
    exec(&db, &try_row("'unknown', 'failed', 'none', 'none', 'none', 'ours', 'sender_down', false, false")).await;
    for bad in [
        try_row("'live', 'failed', 'upload', 'ok', 'fail', NULL, NULL, true, true"),
        try_row("'live', 'sent', 'uploaded', 'ok', 'ok', 'bir', 'cut', true, true"),
        try_row("'live', 'failed', 'upload', 'ok', 'fail', 'bir', NULL, true, true"),
        try_row("'lab', 'failed', 'upload', 'none', 'fail', 'bir', 'timeout', false, false"),
        try_row("'lab', 'sent', 'uploaded', 'none', 'ok', NULL, NULL, false, true"),
        try_row("'live', 'failed', 'upload', 'ok', 'fail', 'bir', 'Timeout', true, true"),
        try_row("'live', 'failed', 'upload', 'ok', 'fail', 'bir', 'timeout', true, true")
            .replace("07:12:00Z", "07:12:31Z"),
        try_row("'live', 'failed', 'upload', 'ok', 'fail', 'bir', 'timeout', true, true")
            .replace("0102030405060708", "010203040506070809"),
        try_row("'live', 'failed', 'upload', 'ok', 'fail', 'bir', 'timeout', true, true")
            .replace("'relay'", "'direct'"),
    ] {
        match sqlx::raw_sql(&bad).execute(&db).await {
            Err(sqlx::Error::Database(error)) => assert_eq!(error.code().as_deref(), Some("23514"), "{bad}"),
            other => panic!("{bad} should break a check: {other:?}"),
        }
    }

    let ana = id(&db, "INSERT INTO accounts (email) VALUES ('ana@example.test') RETURNING id").await;
    let bea = id(&db, "INSERT INTO accounts (email) VALUES ('bea@example.test') RETURNING id").await;
    let claim = |root: &str| {
        format!(
            "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status, decided_at, verified_by)
             VALUES ('{ana}', 'personal', '{root}', '00000', 'SYNTHETIC TAXPAYER', 'approved', now(), 'staff') RETURNING id"
        )
    };
    let kept_claim = id(&db, &claim("000123453")).await;
    let gone_claim = id(&db, &claim("000123461")).await;
    exec(
        &db,
        &format!(
            "INSERT INTO bir_days (day, tries, got_in, failed, maybe, ours, user_network, held, accounts, networks,
                 online_minutes, busy_minutes, offline_minutes, quiet_minutes, note_minutes, unwatched_minutes, worst,
                 hours, outages, windows, final)
             VALUES ('2026-11-11', 4, 1, 3, 1, 2, 5, 6, 3, 3, 880, 5, 530, 10, 15, 20, 'offline', '{hours}',
                 '[{{\"from\": \"2026-11-11T07:10:00Z\", \"to\": \"2026-11-11T16:00:00Z\", \"window\": \"upload\", \"source\": \"tries\"}}]',
                 '{{\"upload\": {{\"worst\": \"offline\", \"online_minutes\": 880}}}}', true);
             INSERT INTO bir_events (scope, from_state, to_state, cause, reason, sign_seq, ord, mailed_at)
             VALUES ('upload', 'online', 'offline', 'rule', '3 accounts on 3 networks didn''t get in within 15 min',
                     1794381000, 0, NULL),
                    ('overall', 'online', 'offline', 'rule', '3 accounts on 3 networks didn''t get in within 15 min',
                     1794381000, 1, '2026-11-11T07:15:00Z');
             INSERT INTO bir_events (scope, from_state, to_state, cause, reason, actor_id, actor_email, sign_seq, ord)
             VALUES ('overall', 'offline', 'offline', 'note_cleared', 'An Admin cleared the note: made-up', '{ana}',
                     'ana@example.test', 1794381300, 0);
             INSERT INTO bir_notes (kind, text, until, set_by, set_by_email)
             VALUES ('outage', 'The upload server is down.', now() + interval '2 hours', '{ana}', 'ana@example.test');
             INSERT INTO bir_notes (kind, text, starts_at, until, set_by, set_by_email, cleared_at, cleared_by,
                                    cleared_by_email, cleared_how, cleared_reason)
             VALUES ('maintenance', 'Planned work.', '2026-11-10T01:00:00Z', '2026-11-10T03:00:00Z', '{ana}',
                     'ana@example.test', '2026-11-10T02:00:00Z', '{ana}', 'ana@example.test', 'staff', 'Made-up: done');
             INSERT INTO bir_sign (id, seq, state, updated_at) VALUES (1, 41, '{{\"made_up\": true}}', now());
             INSERT INTO bir_proofs (id, account_id, claim_id, day, form, period, kid, jws)
             VALUES ('p_kept', '{ana}', '{kept_claim}', '2026-11-11', '1601C', '102026', 'k1', 'a.b.c'),
                    ('p_gone', '{ana}', '{gone_claim}', '2026-11-11', '1601C', '102026', 'k1', 'a.b.c'),
                    ('p_orphan', '{bea}', '{kept_claim}', '2026-11-11', '1601C', '102026', 'k1', 'a.b.c');
             INSERT INTO bir_host_keys (fingerprint, first_seen, last_seen, seen)
             VALUES ('SHA256:{fingerprint}', now(), now(), '[]');
             INSERT INTO bir_held (account_id, claim_id, form, period, code)
             VALUES ('{ana}', '{kept_claim}', '1601C', '102026', 'bir_offline')",
            hours = "-".repeat(2) + "obxmn" + &"w".repeat(8) + &"x".repeat(9),
            fingerprint = "A".repeat(43),
        ),
    )
    .await;
    let kept = uptime_kept(&db).await;
    assert_eq!(kept, (1, 3, 2, 3, Some(41)));
    let events: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, scope FROM bir_events ORDER BY id").fetch_all(&db).await.unwrap();
    let whole = whole_rows(&db).await;

    // An archive table an earlier rollback left, its columns in another
    // order: the down file names every column, so it fills it right.
    exec(
        &db,
        "CREATE SCHEMA rollback_archive;
         CREATE TABLE rollback_archive.bir_sign (updated_at timestamptz, state jsonb, archived_at timestamptz,
                                                 seq bigint, id smallint);
         CREATE TABLE rollback_archive.bir_notes AS
             SELECT cleared_reason, cleared_how, id, now() AS archived_at, kind, text, starts_at, until, set_by,
                    set_by_email, cleared_at, cleared_by, cleared_by_email
               FROM bir_notes WITH NO DATA",
    )
    .await;

    // Back to 13 the way sqlx reverts: 13's schema exactly, the archive
    // filled.
    MIGRATOR.undo(&db, 13).await.unwrap();
    assert_eq!(versions(&db).await, (1..=13).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_13, "the schema is migration 13's");
    assert_eq!(archived(&db, "bir_days", "day").await, ["2026-11-11"]);
    assert_eq!(archived(&db, "bir_events", "scope").await, ["overall", "overall", "upload"]);
    assert_eq!(archived(&db, "bir_proofs", "id").await, ["p_gone", "p_kept", "p_orphan"]);
    assert_eq!(archived(&db, "bir_sign", "seq").await, ["41"]);
    release_13().run(&db).await.unwrap();

    // Meanwhile one claim and one account go away; the way up brings back
    // everything but the gone account's proof (the gone claim's without its
    // claim), and the events' ids go on from where they were.
    exec(&db, &format!("DELETE FROM tin_claims WHERE id = '{gone_claim}'")).await;
    exec(&db, &format!("DELETE FROM accounts WHERE id = '{bea}'")).await;
    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(uptime_kept(&db).await, (1, 3, 2, 2, Some(41)));
    let back: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, scope FROM bir_events ORDER BY id").fetch_all(&db).await.unwrap();
    assert_eq!(back, events);
    // Every column of every row, as it was (the proofs aside whose account
    // or claim went).
    let mut expected = whole.clone();
    let proofs = expected.get_mut("bir_proofs").unwrap();
    proofs.retain(|row| !row.contains("p_orphan"));
    for row in proofs.iter_mut().filter(|row| row.contains("p_gone")) {
        *row = row.replace(&format!(r#""claim_id":"{gone_claim}""#), r#""claim_id":null"#);
    }
    proofs.sort();
    let mut back_rows = whole_rows(&db).await;
    back_rows.get_mut("bir_proofs").unwrap().sort();
    assert_eq!(back_rows, expected);
    let next: i64 = sqlx::query_scalar(
        "INSERT INTO bir_events (scope, from_state, to_state, cause, reason)
         VALUES ('overall', 'offline', 'online', 'scout', 'A scout got in') RETURNING id",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert!(next > events.last().unwrap().0);
    let archive_tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.tables
         WHERE table_schema = 'rollback_archive' AND table_name LIKE 'bir_%' ORDER BY 1",
    )
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(archive_tables, ["bir_proofs"]);
    assert_eq!(archived(&db, "bir_proofs", "id").await, ["p_orphan"]);
    // What is not kept: tries, held requests, host keys.
    for table in ["bir_tries", "bir_held", "bir_host_keys"] {
        let rows: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}")).fetch_one(&db).await.unwrap();
        assert_eq!(rows, 0, "{table}");
    }

    // By hand, as psql runs it, the down file does the same (after 24's
    // to 15's, newest first).
    for down in [
        include_str!("../migrations/0024_app_audit_source.down.sql"),
        include_str!("../migrations/0023_test_roots.down.sql"),
        include_str!("../migrations/0022_app_authorize_validate.down.sql"),
        include_str!("../migrations/0021_app_authorize.down.sql"),
        include_str!("../migrations/0020_devices_grant_index.down.sql"),
        include_str!("../migrations/0019_device_install_index.down.sql"),
        include_str!("../migrations/0018_device_installs.down.sql"),
        include_str!("../migrations/0017_app_sign_ins.down.sql"),
        include_str!("../migrations/0016_account_deletion.down.sql"),
        include_str!("../migrations/0015_fixed_codes.down.sql"),
        include_str!("../migrations/0014_bir_uptime.down.sql"),
    ] {
        let mut tx = db.begin().await.unwrap();
        sqlx::raw_sql(down).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(versions(&db).await, (1..=13).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_13);
    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(uptime_kept(&db).await, (1, 4, 2, 2, Some(41)));
    let again = whole_rows(&db).await;
    assert_eq!(again["bir_days"], expected["bir_days"]);
    assert_eq!(again["bir_notes"], expected["bir_notes"]);
    assert_eq!(again["bir_sign"], expected["bir_sign"]);
}

/// At 14 a claim can be deleted whatever was held or proved for it: its
/// held requests and proofs stay, without it, each found by an index.
#[sqlx::test(migrations = false)]
async fn a_claim_goes_and_its_holds_and_proofs_stay(db: PgPool) {
    MIGRATOR.run(&db).await.unwrap();
    let ana = id(&db, "INSERT INTO accounts (email) VALUES ('ana@example.test') RETURNING id").await;
    let claim = id(
        &db,
        &format!(
            "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status, decided_at, verified_by)
             VALUES ('{ana}', 'personal', '000123453', '00000', 'SYNTHETIC TAXPAYER', 'approved', now(), 'staff') RETURNING id"
        ),
    )
    .await;
    exec(
        &db,
        &format!(
            "INSERT INTO bir_held (account_id, claim_id, form, period, code)
             VALUES ('{ana}', '{claim}', '1601C', '102026', 'bir_offline');
             INSERT INTO bir_proofs (id, account_id, claim_id, day, form, period, kid, jws)
             VALUES ('p_made_up', '{ana}', '{claim}', '2026-11-11', '1601C', '102026', 'k1', 'a.b.c')"
        ),
    )
    .await;
    exec(&db, &format!("DELETE FROM tin_claims WHERE id = '{claim}'")).await;
    let held: Vec<Option<Uuid>> = sqlx::query_scalar("SELECT claim_id FROM bir_held").fetch_all(&db).await.unwrap();
    assert_eq!(held, [None]);
    let proofs: Vec<(String, Option<Uuid>, String)> =
        sqlx::query_as("SELECT id, claim_id, jws FROM bir_proofs").fetch_all(&db).await.unwrap();
    assert_eq!(proofs, [("p_made_up".to_owned(), None, "a.b.c".to_owned())]);
    let indexed: Vec<String> = sqlx::query_scalar(
        "SELECT indexname::text FROM pg_indexes WHERE indexname IN ('bir_held_claim', 'bir_proofs_claim') ORDER BY 1",
    )
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(indexed, ["bir_held_claim", "bir_proofs_claim"]);
}

/// Every row of the tables migration 14's down file keeps, each whole as
/// JSON, by table.
async fn whole_rows(db: &PgPool) -> BTreeMap<String, Vec<String>> {
    let mut tables = BTreeMap::new();
    for table in ["bir_days", "bir_events", "bir_notes", "bir_proofs", "bir_sign"] {
        let rows: Vec<String> =
            sqlx::query_scalar(&format!("SELECT row_to_json(t)::text FROM {table} t ORDER BY row_to_json(t)::text"))
                .fetch_all(db)
                .await
                .unwrap();
        tables.insert(table.to_owned(), rows);
    }
    tables
}

/// Migration 14 changes two existing tables, `tickets` and `devices`: a
/// transaction left open on either makes start-up fail within seconds
/// instead of hanging, and once it ends the migration goes in. The new
/// check on `tickets` is added without scanning the table (every existing
/// row is NULL), so it holds no lock longer than adding the column does.
async fn a_stray_open_transaction_fails_migration_14(db: PgPool, table: &str) {
    release_13().run(&db).await.unwrap();
    let mut stray = db.begin().await.unwrap();
    sqlx::query(&format!("SELECT count(*) FROM {table}")).execute(&mut *stray).await.unwrap();
    let started = std::time::Instant::now();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(40), MIGRATOR.run(&db))
        .await
        .expect("the migration waited for the lock without end");
    let error = outcome.expect_err("the migration ran past an open transaction").to_string();
    assert!(error.contains("lock timeout"), "{table}: {error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(30), "{table}");
    assert_eq!(versions(&db).await, (1..=13).collect::<Vec<_>>(), "{table}");
    stray.rollback().await.unwrap();
    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());
    let validated: bool =
        sqlx::query_scalar("SELECT convalidated FROM pg_constraint WHERE conname = 'tickets_bir_admission_check'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert!(!validated, "the check was added without a scan");
}

#[sqlx::test(migrations = false)]
async fn a_stray_open_transaction_on_tickets_fails_migration_14(db: PgPool) {
    a_stray_open_transaction_fails_migration_14(db, "tickets").await;
}

#[sqlx::test(migrations = false)]
async fn a_stray_open_transaction_on_devices_fails_migration_14(db: PgPool) {
    a_stray_open_transaction_fails_migration_14(db, "devices").await;
}

/// The migrations of the release before fixed sign-in codes: 1 to 14.
fn release_14() -> Migrator {
    Migrator {
        migrations: Cow::Owned(MIGRATOR.iter().filter(|migration| migration.version <= 14).cloned().collect()),
        ..Migrator::DEFAULT
    }
}

/// Migration 15: a fixed sign-in code's hash lives on a test account only,
/// always with who set it and when, and the history knows its three
/// actions. Going back to 14 never refuses, leaves 14's schema exactly and
/// moves those history lines to the archive; the way up works again, by
/// sqlx and by hand.
#[sqlx::test(migrations = false)]
async fn fixed_codes_live_on_test_accounts_and_go_back_to_14(db: PgPool) {
    release_14().run(&db).await.unwrap();
    let schema_14 = schema(&db).await;
    let ana = id(&db, "INSERT INTO accounts (email) VALUES ('ana@example.test') RETURNING id").await;
    let boss = id(&db, "INSERT INTO accounts (email, role) VALUES ('boss@example.test', 'admin') RETURNING id").await;
    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());

    let hash = "decode(repeat('ab', 32), 'hex')";
    let set = |columns: &str| format!("UPDATE accounts SET {columns} WHERE id = '{ana}'");
    let whole = format!("fixed_code_hash = {hash}, fixed_code_set_by = '{boss}', fixed_code_set_at = now()");
    // Not without the test label.
    fails_with(&db, &set(&whole), "accounts_fixed_code_check").await;
    exec(&db, &set("test_account = true")).await;
    // Never without who and when, and only a SHA-256 hash.
    for columns in [
        format!("fixed_code_hash = {hash}"),
        format!("fixed_code_hash = {hash}, fixed_code_set_by = '{boss}'"),
        format!("fixed_code_hash = decode('ab', 'hex'), fixed_code_set_by = '{boss}', fixed_code_set_at = now()"),
        format!("fixed_code_set_by = '{boss}', fixed_code_set_at = now()"),
    ] {
        fails_with(&db, &set(&columns), "accounts_fixed_code_check").await;
    }
    exec(&db, &set(&whole)).await;
    // The label cannot come off while it is set.
    fails_with(&db, &set("test_account = false"), "accounts_fixed_code_check").await;
    let line = |action: &str| {
        format!(
            "INSERT INTO account_audit (account_id, action, actor_id, actor_email, source, reason)
             VALUES ('{ana}', '{action}', '{boss}', 'boss@example.test', 'web', 'Store review')"
        )
    };
    for action in ["fixed_code_set", "fixed_code_changed", "fixed_code_cleared", "banned"] {
        exec(&db, &line(action)).await;
    }
    fails_with(&db, &line("fixed_code_shown"), "account_audit_action_check").await;

    // Back to 14 the way sqlx reverts: 14's schema exactly, the code gone,
    // its history in the archive, every other line kept.
    MIGRATOR.undo(&db, 14).await.unwrap();
    assert_eq!(versions(&db).await, (1..=14).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_14, "the schema is migration 14's");
    assert_eq!(
        archived(&db, "account_audit", "action").await,
        ["fixed_code_changed", "fixed_code_cleared", "fixed_code_set"]
    );
    let kept: Vec<String> = sqlx::query_scalar("SELECT action FROM account_audit").fetch_all(&db).await.unwrap();
    assert_eq!(kept, ["banned"]);
    release_14().run(&db).await.unwrap();

    // Up again: no code came back. Set one, then go back by hand.
    MIGRATOR.run(&db).await.unwrap();
    let none: bool = sqlx::query_scalar("SELECT fixed_code_hash IS NULL FROM accounts WHERE id = $1")
        .bind(ana)
        .fetch_one(&db)
        .await
        .unwrap();
    assert!(none);
    exec(&db, &set(&whole)).await;
    exec(&db, &line("fixed_code_set")).await;
    for down in [
        include_str!("../migrations/0024_app_audit_source.down.sql"),
        include_str!("../migrations/0023_test_roots.down.sql"),
        include_str!("../migrations/0022_app_authorize_validate.down.sql"),
        include_str!("../migrations/0021_app_authorize.down.sql"),
        include_str!("../migrations/0020_devices_grant_index.down.sql"),
        include_str!("../migrations/0019_device_install_index.down.sql"),
        include_str!("../migrations/0018_device_installs.down.sql"),
        include_str!("../migrations/0017_app_sign_ins.down.sql"),
        include_str!("../migrations/0016_account_deletion.down.sql"),
        include_str!("../migrations/0015_fixed_codes.down.sql"),
    ] {
        let mut tx = db.begin().await.unwrap();
        sqlx::raw_sql(down).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(versions(&db).await, (1..=14).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_14);
    // A second rollback adds to the archive.
    assert_eq!(archived(&db, "account_audit", "action").await.len(), 4);
    release_14().run(&db).await.unwrap();
}

/// The migrations of the release before account deletion: 1 to 15.
fn release_15() -> Migrator {
    Migrator {
        migrations: Cow::Owned(MIGRATOR.iter().filter(|migration| migration.version <= 15).cloned().collect()),
        ..Migrator::DEFAULT
    }
}

/// Migration 16: every code so far is a sign-in code, a code has one of two
/// purposes, and the history knows `deleted`. Going back to 15 never
/// refuses, leaves 15's schema exactly, deletes the deletion codes (15
/// would take them for sign-in codes) and moves the `deleted` lines to the
/// archive; the way up works again, by sqlx and by hand.
#[sqlx::test(migrations = false)]
async fn account_deletion_goes_back_to_15(db: PgPool) {
    release_15().run(&db).await.unwrap();
    let schema_15 = schema(&db).await;
    let ana = id(&db, "INSERT INTO accounts (email) VALUES ('ana@example.test') RETURNING id").await;
    let code = "INSERT INTO email_codes (email, code_hash, expires_at) VALUES ('ana@example.test', decode('01', 'hex'), now())";
    exec(&db, code).await;
    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());
    let purposes: Vec<String> = sqlx::query_scalar("SELECT purpose FROM email_codes").fetch_all(&db).await.unwrap();
    assert_eq!(purposes, ["sign_in"]);

    let with_purpose = |purpose: &str| {
        format!(
            "INSERT INTO email_codes (email, code_hash, expires_at, purpose)
             VALUES ('ana@example.test', decode('02', 'hex'), now(), '{purpose}')"
        )
    };
    exec(&db, &with_purpose("delete")).await;
    fails_with(&db, &with_purpose("reset"), "email_codes_purpose_check").await;
    let line = |action: &str| {
        format!(
            "INSERT INTO account_audit (account_id, action, actor_id, actor_email, source)
             VALUES ('{ana}', '{action}', '{ana}', '', 'web')"
        )
    };
    exec(&db, &line("deleted")).await;
    exec(&db, &line("signed_out")).await;
    fails_with(&db, &line("erased"), "account_audit_action_check").await;

    // Back to 15 the way sqlx reverts.
    MIGRATOR.undo(&db, 15).await.unwrap();
    assert_eq!(versions(&db).await, (1..=15).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_15, "the schema is migration 15's");
    assert_eq!(archived(&db, "account_audit", "action").await, ["deleted"]);
    let kept: Vec<String> = sqlx::query_scalar("SELECT action FROM account_audit").fetch_all(&db).await.unwrap();
    assert_eq!(kept, ["signed_out"]);
    let codes: Vec<Vec<u8>> = sqlx::query_scalar("SELECT code_hash FROM email_codes").fetch_all(&db).await.unwrap();
    assert_eq!(codes, [vec![1u8]], "the sign-in code stays, the deletion code goes");
    release_15().run(&db).await.unwrap();

    // Up again, then back by hand.
    MIGRATOR.run(&db).await.unwrap();
    exec(&db, &with_purpose("delete")).await;
    exec(&db, &line("deleted")).await;
    for down in [
        include_str!("../migrations/0024_app_audit_source.down.sql"),
        include_str!("../migrations/0023_test_roots.down.sql"),
        include_str!("../migrations/0022_app_authorize_validate.down.sql"),
        include_str!("../migrations/0021_app_authorize.down.sql"),
        include_str!("../migrations/0020_devices_grant_index.down.sql"),
        include_str!("../migrations/0019_device_install_index.down.sql"),
        include_str!("../migrations/0018_device_installs.down.sql"),
        include_str!("../migrations/0017_app_sign_ins.down.sql"),
        include_str!("../migrations/0016_account_deletion.down.sql"),
    ] {
        let mut tx = db.begin().await.unwrap();
        sqlx::raw_sql(down).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(versions(&db).await, (1..=15).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_15);
    assert_eq!(archived(&db, "account_audit", "action").await.len(), 2);
    release_15().run(&db).await.unwrap();
}

/// The migrations of the release before in-app sign-in: 1 to 16.
fn release_16() -> Migrator {
    Migrator {
        migrations: Cow::Owned(MIGRATOR.iter().filter(|migration| migration.version <= 16).cloned().collect()),
        ..Migrator::DEFAULT
    }
}

/// Migration 17: three new tables, whose checks hold a request to a known
/// intent and platform, a well-formed install id and 32-byte hashes, and
/// the `app_sign_in` purpose. Going back to 16 never refuses, leaves 16's
/// schema exactly and keeps every web code, deleting the in-app ones; the
/// way up works again, by sqlx and by hand.
#[sqlx::test(migrations = false)]
async fn in_app_sign_in_goes_back_to_16(db: PgPool) {
    release_16().run(&db).await.unwrap();
    let schema_16 = schema(&db).await;
    exec(&db, "INSERT INTO accounts (email) VALUES ('ana@example.test')").await;
    let code = "INSERT INTO email_codes (email, code_hash, expires_at) VALUES ('ana@example.test', decode('01', 'hex'), now())";
    exec(&db, code).await;
    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());

    let request = |intent: &str, platform: &str, hash_bytes: usize| {
        format!(
            "INSERT INTO app_sign_ins (email, client_id, secret_hash, intent, platform, expires_at)
             VALUES ('ana@example.test', 'buwiz-ebirforms', decode(repeat('ab', {hash_bytes}), 'hex'), '{intent}',
                     {platform}, now() + interval '10 minutes')"
        )
    };
    exec(&db, &request("sign_in", "'ios'", 32)).await;
    exec(&db, &request("create", "NULL", 32)).await;
    fails_with(&db, &request("sign_up", "NULL", 32), "app_sign_ins_intent_check").await;
    fails_with(&db, &request("sign_in", "'symbian'", 32), "app_sign_ins_platform_check").await;
    fails_with(&db, &request("sign_in", "NULL", 31), "app_sign_ins_secret_hash_check").await;
    fails_not_null(
        &db,
        "INSERT INTO app_sign_ins (email, client_id, secret_hash, intent)
         VALUES ('ana@example.test', 'buwiz-ebirforms', decode(repeat('ab', 32), 'hex'), 'sign_in')",
    )
    .await;
    exec(
        &db,
        "INSERT INTO email_codes (email, code_hash, expires_at, purpose)
         VALUES ('ana@example.test', decode('02', 'hex'), now(), 'app_sign_in')",
    )
    .await;
    fails_with(
        &db,
        "INSERT INTO app_sign_ins (email, client_id, secret_hash, intent, install_id, expires_at)
         VALUES ('ana@example.test', 'buwiz-ebirforms', decode(repeat('ab', 32), 'hex'), 'sign_in', 'short', now())",
        "app_sign_ins_install_id_check",
    )
    .await;
    exec(
        &db,
        "INSERT INTO email_codes (email, code_hash, expires_at, purpose, unchecked)
         VALUES ('ana@example.test', decode('03', 'hex'), now(), 'app_sign_in', true)",
    )
    .await;
    exec(&db, "INSERT INTO app_sign_in_alerts (day) VALUES ('2026-11-25')").await;

    // Back to 16 the way sqlx reverts.
    MIGRATOR.undo(&db, 16).await.unwrap();
    assert_eq!(versions(&db).await, (1..=16).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_16, "the schema is migration 16's");
    let codes: i64 = sqlx::query_scalar("SELECT count(*) FROM email_codes").fetch_one(&db).await.unwrap();
    assert_eq!(codes, 1, "sign-in codes stay, in-app codes go");
    release_16().run(&db).await.unwrap();

    // Up again, then back by hand.
    MIGRATOR.run(&db).await.unwrap();
    exec(&db, &request("sign_in", "'android'", 32)).await;
    for down in [
        include_str!("../migrations/0024_app_audit_source.down.sql"),
        include_str!("../migrations/0023_test_roots.down.sql"),
        include_str!("../migrations/0022_app_authorize_validate.down.sql"),
        include_str!("../migrations/0021_app_authorize.down.sql"),
        include_str!("../migrations/0020_devices_grant_index.down.sql"),
        include_str!("../migrations/0019_device_install_index.down.sql"),
        include_str!("../migrations/0018_device_installs.down.sql"),
        include_str!("../migrations/0017_app_sign_ins.down.sql"),
    ] {
        let mut tx = db.begin().await.unwrap();
        sqlx::raw_sql(down).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(versions(&db).await, (1..=16).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_16);
    release_16().run(&db).await.unwrap();
}

/// The migrations of the release before one device row per install: 1 to
/// 17.
fn release_17() -> Migrator {
    Migrator {
        migrations: Cow::Owned(MIGRATOR.iter().filter(|migration| migration.version <= 17).cloned().collect()),
        ..Migrator::DEFAULT
    }
}

/// Migrations 18 and 19: devices get the columns an app sends, checked,
/// and `signed_in_at` (NULL for earlier rows: nothing is rewritten); one
/// row per account and install (19, built concurrently); grants and device
/// sign-ins keep the install; ending a grant removes its devices. Going
/// back to 17 never refuses,
/// keeps every device and grant, and leaves 17's schema exactly; the way
/// up works again, by sqlx and by hand.
#[sqlx::test(migrations = false)]
async fn one_device_row_per_install_goes_back_to_17(db: PgPool) {
    release_17().run(&db).await.unwrap();
    let schema_17 = schema(&db).await;
    let ana = id(&db, "INSERT INTO accounts (email) VALUES ('ana@example.test') RETURNING id").await;
    let grant = id(
        &db,
        &format!("INSERT INTO oauth_grants (account_id, client_id) VALUES ('{ana}', 'buwiz-ebirforms') RETURNING id"),
    )
    .await;
    let device = id(
        &db,
        &format!(
            "INSERT INTO devices (account_id, grant_id, public_key, name, created_at)
             VALUES ('{ana}', '{grant}', decode(repeat('01', 32), 'hex'), 'Old Mac', '2026-11-01T00:00:00Z') RETURNING id"
        ),
    )
    .await;
    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());
    // No row was rewritten: an earlier device reads its sign-in as its
    // creation.
    let (signed_in, install): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT signed_in_at::text, install_id FROM devices WHERE id = $1")
            .bind(device)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!((signed_in, install), (None, None));

    let row = |key: &str, install: &str, extra: &str| {
        format!(
            "INSERT INTO devices (account_id, grant_id, public_key, name, install_id{})
             VALUES ('{ana}', '{grant}', decode(repeat('{key}', 32), 'hex'), 'Phone', '{install}'{})",
            if extra.is_empty() { "" } else { ", model" },
            if extra.is_empty() { String::new() } else { format!(", '{extra}'") }
        )
    };
    exec(&db, &row("02", "install-phone-0001", "")).await;
    fails_with(&db, &row("03", "install-phone-0001", ""), "devices_account_install").await;
    fails_with(&db, &row("04", "short", ""), "devices_install_id_check").await;
    fails_with(&db, &row("05", "install-other-0002", &"x".repeat(81)), "devices_model_check").await;
    fails_with(
        &db,
        &format!("UPDATE oauth_grants SET platform = 'symbian' WHERE id = '{grant}'"),
        "oauth_grants_platform_check",
    )
    .await;
    exec(
        &db,
        &format!("UPDATE oauth_grants SET install_id = 'install-phone-0001', platform = 'ios' WHERE id = '{grant}'"),
    )
    .await;
    // Ending the grant removes its devices, at the grant's time.
    exec(&db, &format!("UPDATE oauth_grants SET revoked_at = '2026-11-20T00:00:00Z' WHERE id = '{grant}'")).await;
    let removed: Vec<String> =
        sqlx::query_scalar("SELECT revoked_at::text FROM devices ORDER BY id").fetch_all(&db).await.unwrap();
    assert!(removed.len() == 2 && removed.iter().all(|at| at.starts_with("2026-11-20")), "{removed:?}");

    // Back to 17 the way sqlx reverts.
    MIGRATOR.undo(&db, 17).await.unwrap();
    assert_eq!(versions(&db).await, (1..=17).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_17, "the schema is migration 17's");
    let devices: i64 = sqlx::query_scalar("SELECT count(*) FROM devices").fetch_one(&db).await.unwrap();
    assert_eq!(devices, 2, "every device stays");
    release_17().run(&db).await.unwrap();

    // Up again, then back by hand.
    MIGRATOR.run(&db).await.unwrap();
    for down in [
        include_str!("../migrations/0024_app_audit_source.down.sql"),
        include_str!("../migrations/0023_test_roots.down.sql"),
        include_str!("../migrations/0022_app_authorize_validate.down.sql"),
        include_str!("../migrations/0021_app_authorize.down.sql"),
        include_str!("../migrations/0020_devices_grant_index.down.sql"),
        include_str!("../migrations/0019_device_install_index.down.sql"),
        include_str!("../migrations/0018_device_installs.down.sql"),
    ] {
        let mut tx = db.begin().await.unwrap();
        sqlx::raw_sql(down).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(versions(&db).await, (1..=17).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_17);
    release_17().run(&db).await.unwrap();
}

/// The migrations up to 19.
fn release_19() -> Migrator {
    Migrator {
        migrations: Cow::Owned(MIGRATOR.iter().filter(|migration| migration.version <= 19).cloned().collect()),
        ..Migrator::DEFAULT
    }
}

/// Migration 20 builds its index CONCURRENTLY, outside a transaction, so
/// it cannot set its own lock_timeout: the server's migrations run on a
/// connection that has one ([`buwiz_cloud::migrate`]). Behind a long
/// transaction that wrote to `devices`, it fails within seconds instead of
/// waiting, leaving an invalid index and no record; as docs/deploy.md says,
/// dropping it and starting again works. The pool's connections never get
/// the setting.
#[sqlx::test(migrations = false)]
async fn migration_20_fails_behind_a_long_transaction_and_recovers(db: PgPool) {
    release_19().run(&db).await.unwrap();
    let mut stray = db.begin().await.unwrap();
    sqlx::query("UPDATE devices SET name = name").execute(&mut *stray).await.unwrap();
    let started = std::time::Instant::now();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(40), buwiz_cloud::migrate(&db))
        .await
        .expect("the migration waited for the lock without end");
    let error = outcome.expect_err("the migration ran past an open transaction").to_string();
    assert!(error.contains("lock timeout"), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
    assert_eq!(versions(&db).await, (1..=19).collect::<Vec<_>>());
    let valid: Option<bool> = sqlx::query_scalar(
        "SELECT i.indisvalid FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid WHERE c.relname = 'devices_grant_id'",
    )
    .fetch_optional(&db)
    .await
    .unwrap();
    assert_eq!(valid, Some(false), "an invalid index is left");
    stray.rollback().await.unwrap();

    // Starting again as it is fails on the leftover index ...
    let error = buwiz_cloud::migrate(&db).await.expect_err("the invalid index is in the way").to_string();
    assert!(error.contains("already exists"), "{error}");
    // ... and works once it is dropped.
    exec(&db, "DROP INDEX CONCURRENTLY IF EXISTS devices_grant_id").await;
    buwiz_cloud::migrate(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());
    let valid: bool = sqlx::query_scalar(
        "SELECT i.indisvalid FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid WHERE c.relname = 'devices_grant_id'",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert!(valid);
    for _ in 0..db.size() {
        let setting: String = sqlx::query_scalar("SHOW lock_timeout").fetch_one(&db).await.unwrap();
        assert_eq!(setting, "0", "the pool's connections keep the default");
    }
}

/// The migrations of the release before Sign in with Buwiz: 1 to 20.
fn release_20() -> Migrator {
    Migrator {
        migrations: Cow::Owned(MIGRATOR.iter().filter(|migration| migration.version <= 20).cloned().collect()),
        ..Migrator::DEFAULT
    }
}

/// Migrations 21 and 22, on a database at 20 with an agent's code, an
/// app's sign-in and its device: a code may belong to a built-in client,
/// which is no row of `oauth_clients`, and keeps the app's install and
/// platform, checked; a device says who removed it (`removed_by`,
/// checked); new-device mails are recorded. The rows there are stay as
/// they were. Going back to 20 drops the apps' codes only, keeps the
/// agents' and every grant and device, and leaves 20's schema exactly; the
/// way up works again, by sqlx and by hand.
#[sqlx::test(migrations = false)]
async fn app_codes_go_back_to_20(db: PgPool) {
    release_20().run(&db).await.unwrap();
    let schema_20 = schema(&db).await;
    let ana = id(&db, "INSERT INTO accounts (email) VALUES ('ana@example.test') RETURNING id").await;
    exec(
        &db,
        "INSERT INTO oauth_clients (client_id, client_name, redirect_uris)
         VALUES ('agent_test', 'Test agent', ARRAY['https://agent.example.test/cb'])",
    )
    .await;
    let grant = id(
        &db,
        &format!(
            "INSERT INTO oauth_grants (account_id, client_id, install_id, platform)
             VALUES ('{ana}', 'buwiz-ebirforms', 'install-phone-0001', 'ios') RETURNING id"
        ),
    )
    .await;
    exec(
        &db,
        &format!(
            "INSERT INTO devices (account_id, grant_id, public_key, name, install_id, platform, signed_in_at)
             VALUES ('{ana}', '{grant}', decode(repeat('01', 32), 'hex'), 'Phone', 'install-phone-0001', 'ios', now())"
        ),
    )
    .await;
    let code = |hash: &str, client: &str, extra: &str| {
        format!(
            "INSERT INTO oauth_codes (code_hash, client_id, account_id, redirect_uri, code_challenge, scope, expires_at{})
             VALUES (decode('{hash}', 'hex'), '{client}', '{ana}', 'ebirforms://signin-callback', 'c', 'app',
                     now() + interval '10 minutes'{})",
            if extra.is_empty() { "" } else { ", install_id, platform" },
            if extra.is_empty() { String::new() } else { format!(", {extra}") }
        )
    };
    exec(&db, &code("01", "agent_test", "")).await;
    fails_with(&db, &code("02", "buwiz-desktop", ""), "oauth_codes_client_id_fkey").await;
    let rows = |db: PgPool| async move {
        sqlx::query_scalar::<_, String>(
            "SELECT concat_ws(' ', 'grant', client_id, install_id, platform, revoked_at) FROM oauth_grants
             UNION ALL SELECT concat_ws(' ', 'device', name, install_id, platform, revoked_at) FROM devices
             ORDER BY 1",
        )
        .fetch_all(&db)
        .await
        .unwrap()
    };
    let before = rows(db.clone()).await;

    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());
    assert_eq!(rows(db.clone()).await, before, "no grant or device changed");
    exec(&db, &code("02", "buwiz-desktop", "'install-phone-0001', 'ios'")).await;
    fails_with(&db, &code("03", "buwiz-desktop", "'short', 'ios'"), "oauth_codes_install_id_check").await;
    fails_with(&db, &code("04", "buwiz-desktop", "'install-phone-0001', 'symbian'"), "oauth_codes_platform_check")
        .await;
    // A device says who removed it: the owner or staff, or nobody.
    let removed_by = |db: PgPool| async move {
        sqlx::query_scalar::<_, Option<String>>("SELECT removed_by FROM devices").fetch_one(&db).await.unwrap()
    };
    assert_eq!(removed_by(db.clone()).await, None, "an existing device was removed by nobody");
    fails_with(&db, "UPDATE devices SET removed_by = 'someone'", "devices_removed_by_check").await;
    exec(&db, "UPDATE devices SET removed_by = 'staff'").await;
    assert_eq!(removed_by(db.clone()).await.as_deref(), Some("staff"));
    let codes = |db: PgPool| async move {
        sqlx::query_scalar::<_, String>("SELECT client_id FROM oauth_codes ORDER BY 1").fetch_all(&db).await.unwrap()
    };
    assert_eq!(codes(db.clone()).await, ["agent_test", "buwiz-desktop"]);

    // Back to 20 the way sqlx reverts: the app's code goes, the agent's stays.
    MIGRATOR.undo(&db, 20).await.unwrap();
    assert_eq!(versions(&db).await, (1..=20).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_20, "the schema is migration 20's");
    assert_eq!(codes(db.clone()).await, ["agent_test"]);
    assert_eq!(rows(db.clone()).await, before, "every grant and device stays");
    release_20().run(&db).await.unwrap();

    // Up again, then back by hand.
    MIGRATOR.run(&db).await.unwrap();
    exec(&db, &code("05", "buwiz-desktop", "'install-phone-0001', 'android'")).await;
    exec(&db, &format!("INSERT INTO new_device_mails (account_id) VALUES ('{ana}')")).await;
    for down in [
        include_str!("../migrations/0024_app_audit_source.down.sql"),
        include_str!("../migrations/0023_test_roots.down.sql"),
        include_str!("../migrations/0022_app_authorize_validate.down.sql"),
        include_str!("../migrations/0021_app_authorize.down.sql"),
    ] {
        let mut tx = db.begin().await.unwrap();
        sqlx::raw_sql(down).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(versions(&db).await, (1..=20).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_20);
    assert_eq!(codes(db.clone()).await, ["agent_test"]);
    assert_eq!(rows(db.clone()).await, before);
    release_20().run(&db).await.unwrap();
}

/// The migrations of the release before migration 22: 1 to 21.
fn release_21() -> Migrator {
    Migrator {
        migrations: Cow::Owned(MIGRATOR.iter().filter(|migration| migration.version <= 21).cloned().collect()),
        ..Migrator::DEFAULT
    }
}

/// Whether each of migration 21's checks is validated, by name.
async fn checks_validated(db: &PgPool) -> Vec<(String, bool)> {
    sqlx::query_as(
        "SELECT conname::text, convalidated FROM pg_constraint
         WHERE conname IN ('oauth_codes_install_id_check', 'oauth_codes_platform_check', 'devices_removed_by_check')
         ORDER BY 1",
    )
    .fetch_all(db)
    .await
    .unwrap()
}

/// Migration 21 adds its checks NOT VALID (no row is read under its
/// ACCESS EXCLUSIVE locks); 22 validates them in a transaction of its own.
/// Going back to 21 makes them NOT VALID again, leaving 21's schema
/// exactly, by sqlx and by hand; the way up works again.
#[sqlx::test(migrations = false)]
async fn migration_22_validates_21s_checks_and_goes_back_to_21(db: PgPool) {
    release_21().run(&db).await.unwrap();
    let schema_21 = schema(&db).await;
    let ana = id(&db, "INSERT INTO accounts (email) VALUES ('ana@example.test') RETURNING id").await;
    let grant = id(
        &db,
        &format!("INSERT INTO oauth_grants (account_id, client_id) VALUES ('{ana}', 'buwiz-desktop') RETURNING id"),
    )
    .await;
    exec(
        &db,
        &format!(
            "INSERT INTO devices (account_id, grant_id, public_key, name, removed_by, revoked_at)
             VALUES ('{ana}', '{grant}', decode(repeat('01', 32), 'hex'), 'Old Mac', 'owner', now())"
        ),
    )
    .await;
    let not_valid = vec![
        ("devices_removed_by_check".to_owned(), false),
        ("oauth_codes_install_id_check".to_owned(), false),
        ("oauth_codes_platform_check".to_owned(), false),
    ];
    let valid: Vec<(String, bool)> = not_valid.iter().map(|(name, _)| (name.clone(), true)).collect();
    assert_eq!(checks_validated(&db).await, not_valid);
    // Not valid yet, but every new row is checked already.
    fails_with(&db, "UPDATE devices SET removed_by = 'someone'", "devices_removed_by_check").await;

    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());
    assert_eq!(checks_validated(&db).await, valid);

    MIGRATOR.undo(&db, 21).await.unwrap();
    assert_eq!(versions(&db).await, (1..=21).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_21, "the schema is migration 21's");
    release_21().run(&db).await.unwrap();

    MIGRATOR.run(&db).await.unwrap();
    for down in [
        include_str!("../migrations/0024_app_audit_source.down.sql"),
        include_str!("../migrations/0023_test_roots.down.sql"),
        include_str!("../migrations/0022_app_authorize_validate.down.sql"),
    ] {
        let mut tx = db.begin().await.unwrap();
        sqlx::raw_sql(down).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(versions(&db).await, (1..=21).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_21);
    let devices: i64 = sqlx::query_scalar("SELECT count(*) FROM devices").fetch_one(&db).await.unwrap();
    assert_eq!(devices, 1);
    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(checks_validated(&db).await, valid);
}

/// Migration 21 runs in a transaction with its own lock_timeout. Behind a
/// long transaction that read either table it changes (`oauth_codes`, or
/// `devices` for `removed_by`), it fails within seconds instead of
/// waiting, records nothing and changes nothing; once that transaction
/// ends, starting again works.
///
/// Migration 22 waits for no one's reads or writes: with a transaction
/// open that updated `devices` and added a code, as signed requests and
/// sign-ins do, it runs. Behind a lock that blocks it (a SHARE lock, as a
/// schema change takes) it fails within seconds, records nothing, and
/// starting again works. The pool's connections never get the setting.
#[sqlx::test(migrations = false)]
async fn migration_21_fails_behind_a_long_transaction_and_recovers(db: PgPool) {
    release_20().run(&db).await.unwrap();
    let schema_20 = schema(&db).await;
    let fails_and_records_nothing = |db: PgPool| async move {
        let started = std::time::Instant::now();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(40), buwiz_cloud::migrate(&db))
            .await
            .expect("the migration waited for the lock without end");
        let error = outcome.expect_err("the migration ran past an open transaction").to_string();
        assert!(error.contains("lock timeout"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(30));
    };
    for table in ["oauth_codes", "devices"] {
        let mut stray = db.begin().await.unwrap();
        sqlx::query(&format!("SELECT count(*) FROM {table}")).execute(&mut *stray).await.unwrap();
        fails_and_records_nothing(db.clone()).await;
        assert_eq!(versions(&db).await, (1..=20).collect::<Vec<_>>(), "{table}");
        stray.rollback().await.unwrap();
        assert_eq!(schema(&db).await, schema_20, "{table}: nothing of migration 21 is left");
    }

    // At 21, with writes open on both tables, 22 runs.
    release_21().run(&db).await.unwrap();
    let ana = id(&db, "INSERT INTO accounts (email) VALUES ('ana@example.test') RETURNING id").await;
    let mut writing = db.begin().await.unwrap();
    sqlx::query("UPDATE devices SET last_used_at = now()").execute(&mut *writing).await.unwrap();
    sqlx::query(&format!(
        "INSERT INTO oauth_codes (code_hash, client_id, account_id, redirect_uri, code_challenge, scope, expires_at)
         VALUES (decode('01', 'hex'), 'buwiz-desktop', '{ana}', 'ebirforms://signin-callback', 'c', 'app', now())"
    ))
    .execute(&mut *writing)
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), buwiz_cloud::migrate(&db))
        .await
        .expect("migration 22 waited for the writes")
        .unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());
    writing.rollback().await.unwrap();

    // Behind a SHARE lock, 22 fails and records nothing.
    MIGRATOR.undo(&db, 21).await.unwrap();
    for table in ["oauth_codes", "devices"] {
        let mut stray = db.begin().await.unwrap();
        sqlx::query(&format!("LOCK TABLE {table} IN SHARE MODE")).execute(&mut *stray).await.unwrap();
        fails_and_records_nothing(db.clone()).await;
        assert_eq!(versions(&db).await, (1..=21).collect::<Vec<_>>(), "{table}");
        stray.rollback().await.unwrap();
    }

    buwiz_cloud::migrate(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());
    for _ in 0..db.size() {
        let setting: String = sqlx::query_scalar("SHOW lock_timeout").fetch_one(&db).await.unwrap();
        assert_eq!(setting, "0", "the pool's connections keep the default");
    }
}

/// The migrations of the release before reserved test roots: 1 to 22.
fn release_22() -> Migrator {
    Migrator {
        migrations: Cow::Owned(MIGRATOR.iter().filter(|migration| migration.version <= 22).cloned().collect()),
        ..Migrator::DEFAULT
    }
}

/// A test TIN's row an Admin made, at `root` and `branch`.
fn admin_test_row(account: Uuid, root: &str, branch: &str, status: &str, by: &str) -> String {
    format!(
        "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status, decided_at, verified_by)
         VALUES ('{account}', 'personal', '{root}', '{branch}', 'TEST TAXPAYER', '{status}', now(), '{by}') RETURNING id"
    )
}

/// Migration 23: migration 13's check holds for every reserved root, and
/// only for those; going back to 22 refuses while a row holds a root 13
/// did not reserve, then puts 13's check back exactly; the way up works
/// again, by sqlx and by hand.
#[sqlx::test(migrations = false)]
async fn reserved_test_roots_go_back_to_22(db: PgPool) {
    release_22().run(&db).await.unwrap();
    let schema_22 = schema(&db).await;
    let check = "SELECT pg_get_constraintdef(oid) FROM pg_constraint WHERE conname = 'tin_claims_test_tin_check'";
    let check_22: String = sqlx::query_scalar(check).fetch_one(&db).await.unwrap();
    let ana = id(&db, "INSERT INTO accounts (email) VALUES ('ana@example.test') RETURNING id").await;
    // A zero-root test TIN from 13 stays as it is.
    let zeros = id(&db, &admin_test_row(ana, "000000000", "00000", "approved", "admin")).await;
    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());
    let validated: bool =
        sqlx::query_scalar("SELECT convalidated FROM pg_constraint WHERE conname = 'tin_claims_test_tin_check'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert!(validated);

    let insert = |root: &str, status: &str, extra: &str, values: &str| {
        format!(
            "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status{extra})
             VALUES ('{ana}', 'personal', '{root}', '00009', 'TEST TAXPAYER', '{status}'{values}) RETURNING id"
        )
    };
    for root in buwiz_cloud::tin::TEST_ROOTS {
        for (status, extra, values) in [
            ("unverified", "", ""),
            ("pending", "", ""),
            ("approved", ", decided_at, verified_by", ", now(), 'staff'"),
            ("approved", ", decided_at, verified_by", ", now(), 'auto'"),
            ("pending", ", verified_by", ", 'admin'"),
            ("rejected", ", verified_by", ", 'admin'"),
        ] {
            fails_with(&db, &insert(root, status, extra, values), "tin_claims_test_tin_check").await;
        }
        let mut branch = 0;
        for (status, by) in [("approved", "seed"), ("removed", "admin"), ("revoked", "seed")] {
            branch += 1;
            id(&db, &admin_test_row(ana, root, &format!("{branch:05}"), status, by)).await;
        }
    }
    // A real TIN is as before, and so is a near miss of a reserved root.
    for root in ["000123453", "111111113"] {
        exec(&db, &insert(root, "unverified", "", "")).await;
    }

    // A dry-run return on one of them.
    let test_claim =
        id(&db, "SELECT id FROM tin_claims WHERE tin_root = '111111114' ORDER BY branch_code LIMIT 1").await;
    exec(&db, &stored_return(ana, test_claim)).await;

    // Back to 22: refused while 111-111-114 has a row, ended ones too, and
    // it says to keep the rows and their returns; nothing is deleted.
    let refused = MIGRATOR.undo(&db, 22).await.unwrap_err().to_string();
    assert!(refused.contains("3 row(s) hold the test root 111-111-114"), "{refused}");
    // (24 went back on its own first: with no audit line from the app, it
    // never refuses.)
    for words in [
        "Stop the rollback",
        "Do not delete these rows or their submissions and tickets",
        "end each claim on the staff pages with a reason (removed or revoked)",
        "write to the owner, who decides",
    ] {
        assert!(refused.contains(words), "{words}: {refused}");
    }
    assert_eq!(versions(&db).await, (1..=23).collect::<Vec<_>>());
    assert_eq!(returns_of(&db, test_claim).await, 1);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM tin_claims WHERE tin_root = '111111114'")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(left, 3);
    // (The test's own clean-up; the runbook never deletes them.)
    exec(&db, &format!("DELETE FROM submissions WHERE claim_id = '{test_claim}'")).await;
    exec(&db, "DELETE FROM tin_claims WHERE tin_root = '111111114'").await;
    MIGRATOR.undo(&db, 22).await.unwrap();
    assert_eq!(versions(&db).await, (1..=22).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_22, "the schema is migration 22's");
    assert_eq!(sqlx::query_scalar::<_, String>(check).fetch_one(&db).await.unwrap(), check_22);
    // At 22, 111-111-114 is an ordinary TIN again, and zeros still checked.
    exec(&db, &insert("111111114", "unverified", "", "")).await;
    fails_with(&db, &insert("000000000", "unverified", "", ""), "tin_claims_test_tin_check").await;
    release_22().run(&db).await.unwrap();
    exec(&db, "DELETE FROM tin_claims WHERE tin_root = '111111114'").await;

    // Up again, then back by hand.
    MIGRATOR.run(&db).await.unwrap();
    for down in [
        include_str!("../migrations/0024_app_audit_source.down.sql"),
        include_str!("../migrations/0023_test_roots.down.sql"),
    ] {
        let mut tx = db.begin().await.unwrap();
        sqlx::raw_sql(down).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(versions(&db).await, (1..=22).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_22);
    assert_eq!(sqlx::query_scalar::<_, String>(check).fetch_one(&db).await.unwrap(), check_22);
    let kept: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tin_claims WHERE id = $1").bind(zeros).fetch_one(&db).await.unwrap();
    assert_eq!(kept, 1);
}

/// Migration 23 refuses to run while a row holds a reserved root and is not
/// a test TIN, and names each such row: at 22 a person's claim on
/// 111-111-114, in any status (even one an Admin made active), is a real
/// TIN. Nothing changes; once the rows are handled it goes in.
#[sqlx::test(migrations = false)]
async fn migration_23_refuses_a_real_claim_on_a_reserved_root(db: PgPool) {
    release_22().run(&db).await.unwrap();
    let ana = id(&db, "INSERT INTO accounts (email) VALUES ('ana@example.test') RETURNING id").await;
    let pending = id(
        &db,
        &format!(
            "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status)
             VALUES ('{ana}', 'personal', '111111114', '00000', 'JUAN REAL', 'pending') RETURNING id"
        ),
    )
    .await;
    // Activated by an Admin without papers: still a real person's TIN.
    let activated = id(&db, &admin_test_row(ana, "111111114", "00001", "approved", "admin")).await;
    // A return the release before sent for it as a real TIN.
    exec(&db, &stored_return(ana, activated)).await;
    let refused = MIGRATOR.run(&db).await.unwrap_err().to_string();
    assert!(refused.contains("refusing to reserve the test roots"), "{refused}");
    for words in [
        "Stop the deploy",
        "Do not delete these rows or their submissions and tickets",
        "end each claim on the staff pages with a reason (removed or revoked)",
        "write to the owner, who decides",
    ] {
        assert!(refused.contains(words), "{words}: {refused}");
    }
    assert_eq!(returns_of(&db, activated).await, 1, "the return is kept");
    for (claim, branch, status, by) in
        [(pending, "00000", "pending", "(none)"), (activated, "00001", "approved", "admin")]
    {
        let named =
            format!("id={claim} account_id={ana} root=111111114 branch={branch} status={status} verified_by={by}");
        assert!(refused.contains(&named), "{named}: {refused}");
    }
    assert_eq!(versions(&db).await, (1..=22).collect::<Vec<_>>());
    let check: String = sqlx::query_scalar(
        "SELECT pg_get_constraintdef(oid) FROM pg_constraint WHERE conname = 'tin_claims_test_tin_check'",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert!(!check.contains("111111114"), "{check}");

    // (The test's own clean-up, to see it go in; the runbook never deletes
    // a claim, a ticket or a return.)
    exec(&db, &format!("DELETE FROM submissions WHERE claim_id = '{activated}'")).await;
    exec(&db, "DELETE FROM tin_claims WHERE tin_root = '111111114'").await;
    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());
}

/// A sent return on `claim`.
fn stored_return(account: Uuid, claim: Uuid) -> String {
    format!(
        "INSERT INTO submissions (account_id, claim_id, form_type, period, filename, xml_sha256, status, sent_at)
         VALUES ('{account}', '{claim}', '1601Cv2018', '062025', '11111111400000-1601Cv2018-062025#a@example.test#.xml',
             decode('dd', 'hex'), 'sent', now())"
    )
}

async fn returns_of(db: &PgPool, claim: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM submissions WHERE claim_id = $1").bind(claim).fetch_one(db).await.unwrap()
}

/// What in `text` deletes or rewrites a return, a ticket or a claim:
/// any case, any spacing, a schema-qualified name (`public.submissions`,
/// `public . submissions`) or a quoted one (`"submissions"`), `ONLY` and
/// `IF EXISTS`, any `TRUNCATE` at all, `DROP TABLE`, and an `ALTER TABLE`
/// that drops something from `submissions` or `tickets`.
fn rewrites_returns(text: &str) -> Option<String> {
    let flat = text.to_ascii_lowercase().replace('"', "");
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    let flat = flat.replace(" .", ".").replace(". ", ".").replace("public.", "");
    let flat = flat.replace(" only ", " ").replace(" if exists ", " ");
    if flat.contains("truncate") {
        return Some("truncate".into());
    }
    for table in ["submissions", "tickets", "tin_claims"] {
        for verb in ["delete from", "update", "drop table"] {
            if table == "tin_claims" && verb == "update" {
                continue;
            }
            let statement = format!("{verb} {table}");
            if flat.contains(&statement) {
                return Some(statement);
            }
        }
    }
    for statement in flat.split(';') {
        for table in ["submissions", "tickets"] {
            if statement.contains(&format!("alter table {table}")) && statement.contains(" drop ") {
                return Some(format!("alter table {table} ... drop"));
            }
        }
    }
    flat.contains("deleted with its tickets").then(|| "deleted with its tickets".into())
}

/// Migration 23's files and runbook never delete or rewrite a return or a
/// ticket: a claim on 111-111-114 from before it may hold real filings.
/// The runbook stops, keeps them, ends the claim with a reason and writes
/// to the owner, and states the reservation's limit.
#[test]
fn migration_23_never_deletes_returns_or_tickets() {
    for caught in [
        "DELETE FROM public.\"submissions\" WHERE true",
        "Update\n  \"Tickets\" SET jws = ''",
        "truncate table tin_claims, submissions",
        "TRUNCATE ONLY tickets",
        "delete from only public.tickets",
        "DELETE FROM Tin_Claims",
        "delete from public . submissions",
        "UPDATE public .\n tickets SET jws = ''",
        "DROP TABLE submissions",
        "drop table if exists public.\"tickets\" cascade",
        "ALTER TABLE submissions DROP COLUMN receipt_jws",
        "alter table only public . tickets\n  drop column jws",
        "ALTER TABLE IF EXISTS \"submissions\" DROP CONSTRAINT submissions_status_check",
    ] {
        assert!(rewrites_returns(caught).is_some(), "{caught}");
    }
    assert_eq!(rewrites_returns("DELETE FROM _sqlx_migrations WHERE version = 23"), None);
    assert_eq!(rewrites_returns("ALTER TABLE tin_claims DROP CONSTRAINT tin_claims_test_tin_check;"), None);
    let deploy = include_str!("../docs/deploy.md");
    let section = deploy.split("## Migration 23 (reserved test roots)").nth(1).expect("the section");
    let section = &section[..section.find("\n## ").unwrap()];
    for (name, text) in [
        ("up", include_str!("../migrations/0023_test_roots.up.sql")),
        ("down", include_str!("../migrations/0023_test_roots.down.sql")),
        ("docs/deploy.md", section),
    ] {
        assert_eq!(rewrites_returns(text), None, "{name}");
    }
    let flat = section.split_whitespace().collect::<Vec<_>>().join(" ");
    for words in [
        "The reservation assumes 111-111-114 is nobody's real TIN; if it is someone's, that person cannot register it and is sent to support, and a claim already holding it stops the deploy.",
        "Stop the deploy.",
        "Keep that account's submissions and tickets exactly as they are",
        "End the claim the normal staff way, with a reason",
        "Write to the owner",
        "No step here deletes or changes a submission or a ticket.",
    ] {
        assert!(flat.contains(words), "{words}");
    }
}

/// The release before reserved test roots never starts on a database at
/// 23: its Gatekeeper runs its migrations first thing (src/lib.rs), and
/// sqlx refuses any applied migration the binary does not embed, whatever
/// the rows hold (a 111-111-114 test TIN is stored here only to show the
/// case that matters). Nothing turns the check off. It covers the
/// Gatekeeper's start only: not the Sender, which has no database, nor the
/// `seed` and `bir` subcommands, which do not run the migrations. The
/// Gatekeeper's guards on a test TIN's job (the reserved route, and the
/// `dry_run` frame) and the runbook's lines in docs/deploy.md cover those.
#[sqlx::test(migrations = false)]
async fn the_release_before_never_starts_on_reserved_roots(db: PgPool) {
    assert!(!MIGRATOR.ignore_missing, "the Gatekeeper must refuse a database newer than itself");
    release_22().run(&db).await.unwrap();
    MIGRATOR.run(&db).await.unwrap();
    let ana = id(&db, "INSERT INTO accounts (email) VALUES ('ana@example.test') RETURNING id").await;
    id(&db, &admin_test_row(ana, "111111114", "00000", "approved", "admin")).await;
    let latest = *versions(&db).await.last().unwrap();

    let refused = release_22().run(&db).await.unwrap_err();
    assert!(
        matches!(refused, sqlx::migrate::MigrateError::VersionMissing(version) if version > 22 && version <= latest),
        "{refused}"
    );
    // The refusal changed nothing.
    assert_eq!(*versions(&db).await.last().unwrap(), latest);
}

/// The migrations of the release before migration 24: 1 to 23.
fn release_23() -> Migrator {
    Migrator {
        migrations: Cow::Owned(MIGRATOR.iter().filter(|migration| migration.version <= 23).cloned().collect()),
        ..Migrator::DEFAULT
    }
}

/// The check on an audit line's source, as Postgres writes it.
const SOURCE_CHECK: &str =
    "SELECT pg_get_constraintdef(oid) FROM pg_constraint WHERE conname = 'claim_audit_source_check'";

/// An audit line of `account` from `source`, as the app's add writes one.
fn audit_line(account: Uuid, source: &str) -> String {
    format!(
        "INSERT INTO claim_audit
             (claim_id, account_id, tin_masked, action, claim_status, actor_id, actor_email, reason, source, claim_kind)
         VALUES (gen_random_uuid(), '{account}', '***-***-453-00000', 'created', 'new', '{account}', 'ana@example.test',
             'added from the app', '{source}', 'personal')"
    )
}

async fn audit_lines(db: &PgPool) -> Vec<String> {
    sqlx::query_scalar("SELECT source FROM claim_audit ORDER BY source").fetch_all(db).await.unwrap()
}

/// Migration 24: an audit line may come from the app (`source` 'app'), and
/// from nowhere else new; the lines already there stay. The release before
/// (at 23) never starts on it. Going back to 23 refuses while a line from
/// the app exists, by sqlx and by hand, deleting and changing nothing; with
/// none, it puts migration 11's check back exactly, and the release before
/// starts. The way up works again.
#[sqlx::test(migrations = false)]
async fn audit_lines_from_the_app_block_going_back_to_23(db: PgPool) {
    release_23().run(&db).await.unwrap();
    let schema_23 = schema(&db).await;
    let check_23: String = sqlx::query_scalar(SOURCE_CHECK).fetch_one(&db).await.unwrap();
    let ana = id(&db, "INSERT INTO accounts (email) VALUES ('ana@example.test') RETURNING id").await;
    for source in ["web", "auto", "seed"] {
        exec(&db, &audit_line(ana, source)).await;
    }
    fails_with(&db, &audit_line(ana, "app"), "claim_audit_source_check").await;

    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());
    let validated: bool =
        sqlx::query_scalar("SELECT convalidated FROM pg_constraint WHERE conname = 'claim_audit_source_check'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert!(validated);
    exec(&db, &audit_line(ana, "app")).await;
    fails_with(&db, &audit_line(ana, "phone"), "claim_audit_source_check").await;
    assert_eq!(audit_lines(&db).await, ["app", "auto", "seed", "web"]);
    let check_24: String = sqlx::query_scalar(SOURCE_CHECK).fetch_one(&db).await.unwrap();
    assert!(check_24.contains("'app'"), "{check_24}");

    // The release before refuses to start on 24, whatever the rows hold.
    let refused = release_23().run(&db).await.unwrap_err();
    assert!(matches!(refused, sqlx::migrate::MigrateError::VersionMissing(24)), "{refused}");

    // Back to 23: refused while a line from the app exists; nothing changes.
    let refused = MIGRATOR.undo(&db, 23).await.unwrap_err().to_string();
    assert!(refused.contains("1 claim_audit line(s) came from the app"), "{refused}");
    for words in ["Stop the rollback", "Do not delete or change these lines", "Go forward instead"] {
        assert!(refused.contains(words), "{words}: {refused}");
    }
    let mut tx = db.begin().await.unwrap();
    let by_hand = sqlx::raw_sql(include_str!("../migrations/0024_app_audit_source.down.sql"))
        .execute(&mut *tx)
        .await
        .unwrap_err()
        .to_string();
    assert!(by_hand.contains("came from the app"), "{by_hand}");
    drop(tx);
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());
    assert_eq!(audit_lines(&db).await, ["app", "auto", "seed", "web"]);
    assert_eq!(sqlx::query_scalar::<_, String>(SOURCE_CHECK).fetch_one(&db).await.unwrap(), check_24);

    // (The test's own clean-up; the runbook never deletes an audit line.)
    exec(&db, "DELETE FROM claim_audit WHERE source = 'app'").await;
    MIGRATOR.undo(&db, 23).await.unwrap();
    assert_eq!(versions(&db).await, (1..=23).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_23, "the schema is migration 23's");
    assert_eq!(sqlx::query_scalar::<_, String>(SOURCE_CHECK).fetch_one(&db).await.unwrap(), check_23);
    assert_eq!(audit_lines(&db).await, ["auto", "seed", "web"]);
    release_23().run(&db).await.unwrap();
    fails_with(&db, &audit_line(ana, "app"), "claim_audit_source_check").await;

    // Up again, then back by hand.
    MIGRATOR.run(&db).await.unwrap();
    let mut tx = db.begin().await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/0024_app_audit_source.down.sql")).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(versions(&db).await, (1..=23).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_23);
    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());
}

/// Migration 24 runs under its own lock_timeout: behind a transaction open
/// on `claim_audit` it fails within seconds instead of waiting, records
/// nothing and changes nothing; once that transaction ends, it goes in.
#[sqlx::test(migrations = false)]
async fn migration_24_fails_behind_a_long_transaction_and_recovers(db: PgPool) {
    release_23().run(&db).await.unwrap();
    let schema_23 = schema(&db).await;
    let mut stray = db.begin().await.unwrap();
    sqlx::query("SELECT count(*) FROM claim_audit").execute(&mut *stray).await.unwrap();
    let started = std::time::Instant::now();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(40), buwiz_cloud::migrate(&db))
        .await
        .expect("the migration waited for the lock without end");
    let error = outcome.expect_err("the migration ran past an open transaction").to_string();
    assert!(error.contains("lock timeout"), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
    assert_eq!(versions(&db).await, (1..=23).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_23, "nothing of migration 24 is left");
    stray.rollback().await.unwrap();
    buwiz_cloud::migrate(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=24).collect::<Vec<_>>());
}
