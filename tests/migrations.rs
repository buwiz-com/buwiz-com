//! Migrations 11 and 12 on a database at migration 10 with a row in every
//! claim status, files, readings, audit lines, tickets and returns: the
//! rows survive and read as before, and going back down to 10 leaves a
//! database the release before this one starts on, every return kept.
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
    assert_eq!(versions(&db).await, (1..=12).collect::<Vec<_>>());
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
    fails_with(&db, &format!("UPDATE accounts SET role = 'owner' WHERE id = '{bob}'"), "accounts_role_check").await;
    exec(
        &db,
        &format!(
            "UPDATE accounts SET role = 'support', test_account = true WHERE id = '{bob}';
             INSERT INTO account_audit (account_id, action, old_value, new_value, actor_id, actor_email, reason)
             VALUES ('{bob}', 'role_changed', NULL, 'support', '{ana}', 'ana@example.test', 'Helps with the queue.');
             INSERT INTO seed_runs (id, actor_id, actor_email, file_sha256, summary)
             VALUES (gen_random_uuid(), '{ana}', 'ana@example.test', decode('00', 'hex'), '{{}}')"
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
             INSERT INTO account_audit (account_id, action, actor_id, actor_email, reason)
             VALUES ('{ana}', 'banned', '{bob}', 'bob@example.test', 'Abuse.');
             INSERT INTO seed_runs (id, actor_id, actor_email, file_sha256, summary)
             VALUES (gen_random_uuid(), '{bob}', 'bob@example.test', decode('00', 'hex'), '{{}}')",
            approved = seeded.approved
        ),
    )
    .await;
    let activated = id(
        &db,
        &format!(
            "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status, decided_at, verified_by)
             VALUES ('{bob}', 'personal', '000123462', '00000', 'SYNTHETIC TAXPAYER', 'approved', now(), 'admin') RETURNING id"
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

    // A rollback never lifts a ban or a pause by itself: each down file
    // refuses while one exists, and so does 0011's while a TIN saved
    // without papers has files.
    let refused = MIGRATOR.undo(&db, 10).await.unwrap_err().to_string();
    assert!(refused.contains("account(s) are banned"), "{refused}");
    assert_eq!(versions(&db).await, (1..=12).collect::<Vec<_>>());
    exec(&db, "UPDATE accounts SET banned_at = NULL, banned_by = NULL, ban_reason = NULL").await;
    let refused = MIGRATOR.undo(&db, 10).await.unwrap_err().to_string();
    assert!(refused.contains("TIN(s) are paused"), "{refused}");
    // 0012 went back; 0011 refused and changed nothing.
    assert_eq!(versions(&db).await, (1..=11).collect::<Vec<_>>());
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
    let refused = MIGRATOR.undo(&db, 10).await.unwrap_err().to_string();
    assert!(refused.contains("TINs saved without papers have files"), "{refused}");
    assert_eq!(versions(&db).await, (1..=11).collect::<Vec<_>>());
    exec(&db, &format!("DELETE FROM claim_documents WHERE claim_id = '{unverified}'")).await;

    // Back to 10 the way sqlx reverts, then the release before this one
    // starts on it.
    MIGRATOR.undo(&db, 10).await.unwrap();
    assert_rolled_back(&db, &old, &schema_10, &rows_10, activated, sent_meanwhile, &seeded).await;

    // The new release can come back on it, and the down files also work run
    // by hand (as psql runs them), newest first.
    MIGRATOR.run(&db).await.unwrap();
    let verified_by: Option<String> = sqlx::query_scalar("SELECT verified_by FROM tin_claims WHERE id = $1")
        .bind(activated)
        .fetch_one(&db)
        .await
        .unwrap();
    // How it became active was lost with the rollback; it counts as staff's.
    assert_eq!(verified_by.as_deref(), Some("staff"));
    for down in [
        include_str!("../migrations/0012_staff_roles.down.sql"),
        include_str!("../migrations/0011_tin_states.down.sql"),
    ] {
        let mut tx = db.begin().await.unwrap();
        sqlx::raw_sql(down).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }
    assert_rolled_back(&db, &old, &schema_10, &rows_10, activated, sent_meanwhile, &seeded).await;
}

/// The database reads as at migration 10 plus what the new release added
/// that the old one can hold, and the old release's migrator accepts it.
async fn assert_rolled_back(
    db: &PgPool,
    old: &Migrator,
    schema_10: &BTreeSet<String>,
    rows_10: &BTreeMap<String, BTreeMap<String, Value>>,
    activated: Uuid,
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
    let activated_row = rows.get_mut("tin_claims").unwrap().remove(&activated.to_string()).unwrap();
    assert_eq!(activated_row["status"], "approved");
    let sent = rows.get_mut("submissions").unwrap().remove(&sent_meanwhile.to_string()).unwrap();
    assert_eq!((sent["status"].as_str(), sent["claim_id"].as_str()), (Some("sent"), Some(&*activated.to_string())));
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
