//! Migrations 11 to 14 on a database at migration 10 with a row in every
//! claim status, files, readings, audit lines, tickets and returns: the
//! rows survive and read as before, and going back down to 10 leaves a
//! database the release before this one starts on, every return kept.
//! Migration 13 (test TINs) goes back to 12 on its own, and 14 (BIR
//! uptime) to 13, keeping its history in the archive for the way up.
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
    assert_eq!(versions(&db).await, (1..=14).collect::<Vec<_>>());
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
    assert_eq!(versions(&db).await, (1..=14).collect::<Vec<_>>());
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
    // 14 went back on its own first: it never refuses.
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
    assert_eq!(versions(&db).await, (1..=14).collect::<Vec<_>>());
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
/// `rollback_archive`; the way up brings them back, a proof whose claim is
/// gone excepted, which stays in the archive.
#[sqlx::test(migrations = false)]
async fn bir_uptime_goes_back_to_13_and_its_history_comes_back(db: PgPool) {
    release_13().run(&db).await.unwrap();
    let schema_13 = schema(&db).await;
    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(versions(&db).await, (1..=14).collect::<Vec<_>>());

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
                 online_minutes, busy_minutes, offline_minutes, quiet_minutes, note_minutes, worst, hours, final)
             VALUES ('2026-11-11', 4, 1, 3, 0, 0, 0, 0, 3, 3, 900, 0, 530, 10, 0, 'offline', '{hours}', true);
             INSERT INTO bir_events (scope, from_state, to_state, cause, reason)
             VALUES ('upload', 'online', 'offline', 'rule', '3 accounts on 3 networks didn''t get in within 15 min'),
                    ('overall', 'online', 'offline', 'rule', '3 accounts on 3 networks didn''t get in within 15 min');
             INSERT INTO bir_notes (kind, text, until, set_by, set_by_email)
             VALUES ('outage', 'The upload server is down.', now() + interval '2 hours', '{ana}', 'ana@example.test');
             INSERT INTO bir_sign (id, seq, state, updated_at) VALUES (1, 41, '{{}}', now());
             INSERT INTO bir_proofs (id, account_id, claim_id, day, form, period, kid, jws)
             VALUES ('p_kept', '{ana}', '{kept_claim}', '2026-11-11', '1601C', '102026', 'k1', 'a.b.c'),
                    ('p_gone', '{ana}', '{gone_claim}', '2026-11-11', '1601C', '102026', 'k1', 'a.b.c');
             INSERT INTO bir_host_keys (fingerprint, first_seen, last_seen, seen)
             VALUES ('SHA256:{fingerprint}', now(), now(), '[]');
             INSERT INTO bir_held (account_id, claim_id, form, period, code)
             VALUES ('{ana}', '{kept_claim}', '1601C', '102026', 'bir_offline')",
            hours = "o".repeat(15) + &"x".repeat(9),
            fingerprint = "A".repeat(43),
        ),
    )
    .await;
    let kept = uptime_kept(&db).await;
    assert_eq!(kept, (1, 2, 1, 2, Some(41)));
    let events: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, scope FROM bir_events ORDER BY id").fetch_all(&db).await.unwrap();

    // Back to 13 the way sqlx reverts: 13's schema exactly, the archive
    // filled.
    MIGRATOR.undo(&db, 13).await.unwrap();
    assert_eq!(versions(&db).await, (1..=13).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_13, "the schema is migration 13's");
    assert_eq!(archived(&db, "bir_days", "day").await, ["2026-11-11"]);
    assert_eq!(archived(&db, "bir_events", "scope").await, ["overall", "upload"]);
    assert_eq!(archived(&db, "bir_proofs", "id").await, ["p_gone", "p_kept"]);
    assert_eq!(archived(&db, "bir_sign", "seq").await, ["41"]);
    release_13().run(&db).await.unwrap();

    // Meanwhile one claim goes away; the way up brings back everything but
    // its proof, and the events' ids go on from where they were.
    exec(&db, &format!("DELETE FROM tin_claims WHERE id = '{gone_claim}'")).await;
    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(uptime_kept(&db).await, (1, 2, 1, 1, Some(41)));
    let back: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, scope FROM bir_events ORDER BY id").fetch_all(&db).await.unwrap();
    assert_eq!(back, events);
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
    assert_eq!(archived(&db, "bir_proofs", "id").await, ["p_gone"]);
    // What is not kept: tries, held requests, host keys.
    for table in ["bir_tries", "bir_held", "bir_host_keys"] {
        let rows: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}")).fetch_one(&db).await.unwrap();
        assert_eq!(rows, 0, "{table}");
    }

    // By hand, as psql runs it, the down file does the same.
    let mut tx = db.begin().await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/0014_bir_uptime.down.sql")).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(versions(&db).await, (1..=13).collect::<Vec<_>>());
    assert_eq!(schema(&db).await, schema_13);
    MIGRATOR.run(&db).await.unwrap();
    assert_eq!(uptime_kept(&db).await, (1, 3, 1, 1, Some(41)));
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
    assert_eq!(versions(&db).await, (1..=14).collect::<Vec<_>>());
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
