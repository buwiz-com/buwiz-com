//! `buwiz-cloud seed`: logins, roles, test labels and TINs from one file,
//! with a record of each run.

mod common;

use buwiz_cloud::account::seed::{self, Operator, Options, Report, SeedError};
use buwiz_cloud::base::storage::DocumentStore;
use sqlx::PgPool;
use uuid::Uuid;

use common::Part::{File, Text};
use common::*;

const EXAMPLE: &str = include_str!("../docs/seed.example.toml");

/// A run on a server with no `ADMIN_EMAILS` (`--no-admin-emails`), by a
/// synthetic operator.
fn options(actor: &str, dry_run: bool) -> Options {
    Options {
        actor: actor.into(),
        dry_run,
        admin_emails: None,
        target: None,
        expect: None,
        operator: Operator { os_user: "ops (sudo from ana)".into(), host: "box.example.test".into() },
    }
}

/// `options` for a server whose `ADMIN_EMAILS` is `admins`.
fn listing(actor: &str, dry_run: bool, admins: &[&str]) -> Options {
    Options { admin_emails: Some(admins.iter().map(|admin| (*admin).to_owned()).collect()), ..options(actor, dry_run) }
}

/// The test database as `--target` names it.
async fn target_of(db: &PgPool) -> String {
    let name: String = sqlx::query_scalar("SELECT current_database()").fetch_one(db).await.unwrap();
    let options = db.connect_options();
    format!("{}:{}/{name}", options.get_host(), options.get_port())
}

/// Runs the file as `options` say; a real run without a `--target` gets
/// the test database's, as an operator would type it.
async fn seed(db: &PgPool, file: &str, options: &Options) -> Result<Report, SeedError> {
    let mut options = options.clone();
    if !options.dry_run && options.target.is_none() {
        options.target = Some(target_of(db).await);
    }
    seed::run(db, &DocumentStore::Database, file.as_bytes(), &options).await
}

async fn count(db: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}")).fetch_one(db).await.unwrap()
}

/// Rows in every table the seed writes to.
async fn counts(db: &PgPool) -> [i64; 5] {
    [
        count(db, "accounts").await,
        count(db, "tin_claims").await,
        count(db, "account_audit").await,
        count(db, "claim_audit").await,
        count(db, "seed_runs").await,
    ]
}

async fn account(db: &PgPool, email: &str) -> (Uuid, Option<String>, bool) {
    sqlx::query_as("SELECT id, role, test_account FROM accounts WHERE email = $1")
        .bind(email)
        .fetch_one(db)
        .await
        .unwrap()
}

/// (status, verified_by, decided_at set, decision_note, kind, name)
async fn claim_of(
    db: &PgPool,
    account: Uuid,
    root: &str,
) -> (String, Option<String>, bool, Option<String>, String, String) {
    sqlx::query_as(
        "SELECT status, verified_by, decided_at IS NOT NULL, decision_note, kind, registered_name FROM tin_claims
         WHERE account_id = $1 AND tin_root = $2 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(account)
    .bind(root)
    .fetch_one(db)
    .await
    .unwrap()
}

fn refused(outcome: Result<Report, SeedError>) -> String {
    match outcome {
        Err(SeedError::Refused(problems)) => problems.join("\n"),
        other => panic!("not refused: {other:?}"),
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn seeding_a_fresh_database_twice_changes_nothing_the_second_time(db: PgPool) {
    let site = Site::with_admins(db, &[]);
    let db = &site.db;
    let report = seed(db, EXAMPLE, &options("admin@example.test", false)).await.unwrap();
    assert_eq!((report.accounts, report.claims), (0, 0));
    let run = report.run_id;
    let lines = report.lines.join("\n");
    for expected in [
        "created login admin@example.test",
        "role admin@example.test No staff role → Admin",
        "role support@example.test No staff role → Support",
        "test label jane@example.test off → on",
        "TIN ***-***-453-00000 active on jane@example.test (new)",
        "TIN ***-***-454-00000 not verified on shop@example.test (new)",
        "created login juan@example.test",
        // A real run never shows what was typed for a TIN in full.
        "refused TIN ending in 000 for juan@example.test: That TIN is not valid.",
    ] {
        assert!(lines.contains(expected), "{expected}\n{lines}");
    }

    let (admin, role, _) = account(db, "admin@example.test").await;
    assert_eq!(role.as_deref(), Some("admin"));
    assert_eq!(account(db, "support@example.test").await.1.as_deref(), Some("support"));
    let (jane, _, test) = account(db, "jane@example.test").await;
    assert!(test);
    assert_eq!(
        claim_of(db, jane, "000123453").await,
        ("approved".into(), Some("seed".into()), true, None, "personal".into(), "JANE EXAMPLE".into())
    );
    let (shop, _, _) = account(db, "shop@example.test").await;
    assert_eq!(claim_of(db, shop, "000123454").await.0, "unverified");
    // The login whose TIN is not valid is made, with no TIN.
    let (juan, _, _) = account(db, "juan@example.test").await;
    let juans: i64 = sqlx::query_scalar("SELECT count(*) FROM tin_claims WHERE account_id = $1")
        .bind(juan)
        .fetch_one(db)
        .await
        .unwrap();
    assert_eq!(juans, 0);

    // Every change is recorded: who, from where and why.
    let lines = account_audit_lines(db, jane).await;
    // One transaction: the lines share a time, so their order is not kept.
    let actions = |lines: &[AccountLine]| {
        let mut actions: Vec<String> = lines.iter().map(|line| line.0.clone()).collect();
        actions.sort();
        actions
    };
    assert_eq!(actions(&lines), ["created", "test_marked"]);
    for line in &lines {
        assert_eq!((line.3.as_str(), line.4.as_str()), ("admin@example.test", "seed"));
        assert!(line.5.as_deref().unwrap().starts_with(&format!("seed {run}:")), "{line:?}");
    }
    let admin_lines = account_audit_lines(db, admin).await;
    assert_eq!(actions(&admin_lines), ["created", "role_changed"]);
    let claim_lines = claim_audit_lines(db, jane).await;
    assert_eq!(claim_lines.len(), 1);
    assert_eq!((claim_lines[0].0.as_str(), claim_lines[0].3.as_str()), ("activated", "seed"));
    assert_eq!(claim_lines[0].4.as_deref(), Some(format!("seed {run}: Owner's test login").as_str()));
    let (sha, changes): (Vec<u8>, serde_json::Value) =
        sqlx::query_as("SELECT file_sha256, summary->'changes' FROM seed_runs WHERE id = $1")
            .bind(run)
            .fetch_one(db)
            .await
            .unwrap();
    assert_eq!(buwiz_cloud::crypto::hex(&sha), report.file_sha256);
    assert_eq!(changes.as_array().unwrap().len(), report.changes);

    // The seeded TIN answers in the apps' list.
    let token = app_token(db, jane).await;
    let listed = profiles(&site, &token, "/api/tax-profiles").await;
    assert_eq!(listed[0]["tin"], "000-123-453-00000", "{listed}");

    let before = counts(db).await;
    let again = seed(db, EXAMPLE, &options("admin@example.test", false)).await.unwrap();
    assert_eq!(again.changes, 0, "{:?}", again.lines);
    assert!(
        again.lines.iter().all(|line| line.starts_with("unchanged") || line.starts_with("refused")),
        "{:?}",
        again.lines
    );
    let after = counts(db).await;
    // Only the empty run is recorded.
    assert_eq!(after[..4], before[..4]);
    assert_eq!(after[4], before[4] + 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_dry_run_shows_full_tins_and_changes_nothing(db: PgPool) {
    let report = seed(&db, EXAMPLE, &options("admin@example.test", true)).await.unwrap();
    assert!(report.dry_run && report.changes > 0);
    let lines = report.lines.join("\n");
    assert!(lines.contains("TIN 000-123-453-00000 active on jane@example.test (new)"), "{lines}");
    assert!(!lines.contains("***"), "{lines}");
    assert_eq!(counts(&db).await, [0; 5]);
    let real = seed(&db, EXAMPLE, &options("admin@example.test", false)).await.unwrap();
    assert_eq!(real.changes, report.changes);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_bad_file_is_refused_whole_and_writes_nothing(db: PgPool) {
    let opts = options("admin@example.test", false);
    let admin = "[[login]]\nemail = \"admin@example.test\"\nrole = \"admin\"\n";
    for (file, expected) in [
        (format!("{admin}colour = \"red\"\n"), "unknown field"),
        (format!("{admin}[[login]]\nemail = \"bad email\"\n"), "not a valid email"),
        (
            format!(
                "{admin}[[login.tin]]\ntin = \"000-123-453\"\nkind = \"personal\"\nname = \"A\"\nstate = \"active\"\n"
            ),
            "needs a reason",
        ),
    ] {
        assert!(refused(seed(&db, &file, &opts).await).contains(expected), "{file}");
        assert_eq!(counts(&db).await, [0; 5], "{file}");
    }

    // Checks against the database refuse it all too: an active TIN owned
    // elsewhere without move_from, or with the wrong one.
    seed(&db, admin, &opts).await.unwrap();
    let owner: Uuid = sqlx::query_scalar("INSERT INTO accounts (email) VALUES ('carl@example.test') RETURNING id")
        .fetch_one(&db)
        .await
        .unwrap();
    add_claim(&db, owner, "000123456", "approved").await;
    let before = counts(&db).await;
    let take = |from: &str| {
        format!(
            "[[login]]\nemail = \"new@example.test\"\n[[login.tin]]\ntin = \"000123456\"\nkind = \"business\"\nname = \"N\"\nstate = \"active\"\nreason = \"x\"\n{from}"
        )
    };
    let problem = refused(seed(&db, &take(""), &opts).await);
    assert!(problem.contains("it is active on carl@example.test; to move it, set move_from"), "{problem}");
    let problem = refused(seed(&db, &take("move_from = \"dora@example.test\"\n"), &opts).await);
    assert!(problem.contains("it is active on carl@example.test"), "{problem}");
    assert_eq!(counts(&db).await, before);
}

#[sqlx::test(migrations = "./migrations")]
async fn seed_moves_an_active_or_waiting_tin_and_a_rerun_changes_nothing(db: PgPool) {
    let site = Site::new(db);
    let db = &site.db;
    let opts = options("admin@example.test", false);
    seed(db, "[[login]]\nemail = \"admin@example.test\"\nrole = \"admin\"\n", &opts).await.unwrap();
    let (old, old_cookie) = site.person("old@example.test").await;
    let old_claim = add_claim(db, old, "000123456", "approved").await;
    // Another one waits for review with its papers.
    let (waiting, waiting_cookie) = site.person("waiting@example.test").await;
    let parts = [
        ("kind", Text("personal")),
        ("tin", Text("000123457")),
        ("registered_name", Text("W")),
        ("tin_id", File("tin.png", PNG)),
    ];
    site.upload("/tins/claims", &waiting_cookie, &parts).await;
    let waiting_claim = site.newest_claim(waiting).await;

    let file = "
        [[login]]
        email = \"new@example.test\"
          [[login.tin]]
          tin = \"000-123-456\"
          kind = \"business\"
          name = \"EXAMPLE HOLDINGS INC\"
          state = \"active\"
          reason = \"Moves to the company login\"
          move_from = \"old@example.test\"
          [[login.tin]]
          tin = \"000-123-457\"
          kind = \"personal\"
          name = \"WILMA EXAMPLE\"
          state = \"active\"
          reason = \"Moves too\"
          move_from = \"waiting@example.test\"
    ";
    let report = seed(db, file, &opts).await.unwrap();
    let lines = report.lines.join("\n");
    assert!(
        lines.contains(
            "TIN ***-***-456-00000 active on new@example.test (moved from old@example.test, which had it active)"
        ),
        "{lines}"
    );
    assert!(lines.contains("moved from waiting@example.test, whose claim waiting for review was deleted"), "{lines}");

    // The old row stays for its returns, ended, with nothing that names the
    // new owner or the reason.
    let (status, note): (String, Option<String>) =
        sqlx::query_as("SELECT status, removal_note FROM tin_claims WHERE id = $1")
            .bind(old_claim)
            .fetch_one(db)
            .await
            .unwrap();
    assert_eq!((status.as_str(), note), ("revoked", None));
    let old_page = text(site.get("/tins", &old_cookie).await).await;
    assert!(old_page.contains("No longer active.") && !old_page.contains("new@example.test"), "{old_page}");
    assert!(!old_page.contains("Moves to the company login"), "{old_page}");
    let old_lines = claim_audit_lines(db, old).await;
    assert_eq!((old_lines[0].0.as_str(), old_lines[0].3.as_str()), ("revoked", "seed"));
    let (new, _, _) = account(db, "new@example.test").await;
    assert_eq!(claim_of(db, new, "000123456").await.1.as_deref(), Some("seed"));
    assert_eq!(claim_of(db, new, "000123457").await.0, "approved");
    // The waiting claim went, files and all; its audit line stays.
    let gone: i64 = sqlx::query_scalar("SELECT count(*) FROM tin_claims WHERE id = $1")
        .bind(waiting_claim)
        .fetch_one(db)
        .await
        .unwrap();
    let files: i64 = sqlx::query_scalar("SELECT count(*) FROM claim_documents WHERE claim_id = $1")
        .bind(waiting_claim)
        .fetch_one(db)
        .await
        .unwrap();
    assert_eq!((gone, files), (0, 0));
    let waiting_lines = claim_audit_lines(db, waiting).await;
    assert_eq!((waiting_lines[0].0.as_str(), waiting_lines[0].1.as_str()), ("deleted", "pending"));

    let before = counts(db).await;
    let again = seed(db, file, &opts).await.unwrap();
    assert_eq!(again.changes, 0, "{:?}", again.lines);
    assert_eq!(counts(db).await[..4], before[..4]);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_first_admin_comes_from_the_file_only_on_a_database_with_none(db: PgPool) {
    let make_eve = "[[login]]\nemail = \"eve@example.test\"\nrole = \"admin\"\n";
    // No Admin yet: --as must be made one by the file.
    let problem =
        refused(seed(&db, "[[login]]\nemail = \"eve@example.test\"\n", &options("eve@example.test", false)).await);
    assert!(problem.contains("this database has none"), "{problem}");
    seed(&db, make_eve, &options("eve@example.test", false)).await.unwrap();
    // Now there is one: nobody else can make themselves Admin that way.
    let make_mallory = "[[login]]\nemail = \"mallory@example.test\"\nrole = \"admin\"\n";
    let problem = refused(seed(&db, make_mallory, &options("mallory@example.test", false)).await);
    assert!(problem.contains("--as mallory@example.test is not an Admin on this database"), "{problem}");
    // The server's own ADMIN_EMAILS is an Admin, even before signing in.
    let root = listing("root@example.test", false, &["root@example.test"]);
    seed(&db, make_mallory, &root).await.unwrap();
    // And the file cannot leave nobody in charge.
    let demote = "[[login]]\nemail = \"eve@example.test\"\nrole = \"none\"\n[[login]]\nemail = \"mallory@example.test\"\nrole = \"none\"\n";
    let problem = refused(seed(&db, demote, &options("eve@example.test", false)).await);
    assert!(problem.contains("no Admin"), "{problem}");
    assert_eq!(account(&db, "eve@example.test").await.1.as_deref(), Some("admin"));
}

#[sqlx::test(migrations = "./migrations")]
async fn seed_refuses_a_database_behind_this_release(db: PgPool) {
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version = (SELECT max(version) FROM _sqlx_migrations)")
        .execute(&db)
        .await
        .unwrap();
    let outcome = seed(&db, EXAMPLE, &options("admin@example.test", false)).await;
    assert!(matches!(outcome, Err(SeedError::Behind { .. })), "{outcome:?}");
    assert_eq!(count(&db, "accounts").await, 0);
}

/// The seed command in a clean environment, from a directory with no
/// `.env`, against this test's own database: (the directory, the seed
/// file's path, a function running the command with extra settings).
async fn command_in(
    db: &PgPool,
) -> (std::path::PathBuf, String, impl Fn(&[&str], &[(&str, &str)]) -> std::process::Output + use<>) {
    let name: String = sqlx::query_scalar("SELECT current_database()").fetch_one(db).await.unwrap();
    let mut url = url::Url::parse(&std::env::var("DATABASE_URL").unwrap()).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    url.set_path(&name);
    let dir = std::env::temp_dir().join(format!("seed-{}", Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let file = dir.join("seed.toml");
    std::fs::write(&file, EXAMPLE).unwrap();
    let cwd = dir.clone();
    let command = move |args: &[&str], settings: &[(&str, &str)]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_buwiz-cloud"))
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("DATABASE_URL", url.as_str())
            .env("DOCUMENT_KEY", "dGhyb3dhd2F5LXRlc3Qta2V5LW5vdC1hLXNlY3JldC0wMDA=")
            .envs(settings.iter().copied())
            .current_dir(&cwd)
            .output()
            .unwrap()
    };
    (dir, file.to_str().unwrap().to_owned(), command)
}

/// The command itself, run as the owner would: the dry run, then the real
/// run with the `--target` and `--expect` the dry run printed.
#[sqlx::test(migrations = "./migrations")]
async fn the_seed_command_dry_runs_then_runs(db: PgPool) {
    let (dir, path, command) = command_in(&db).await;
    let target = target_of(&db).await;
    let name = target.rsplit('/').next().unwrap().to_owned();
    let path = path.as_str();
    let none = ("ADMIN_EMAILS", "");
    let command = |args: &[&str]| command(args, &[none]);

    let dry = command(&["seed", path, "--as", "admin@example.test", "--dry-run", "--no-admin-emails"]);
    let out = String::from_utf8_lossy(&dry.stdout);
    assert_eq!(dry.status.code(), Some(0), "{out}{}", String::from_utf8_lossy(&dry.stderr));
    assert!(out.contains(&format!("Database: 127.0.0.1:54330/{name}")), "{out}");
    assert!(out.contains("Before: 0 accounts, 0 TIN claims"), "{out}");
    assert!(out.contains("ADMIN_EMAILS: none (--no-admin-emails)"), "{out}");
    assert!(out.contains("Dry run: nothing is written."), "{out}");
    assert!(out.contains("TIN 000-123-453-00000 active on jane@example.test (new)"), "{out}");
    assert!(!out.contains("buwiz_cloud:buwiz_cloud"), "the password stays out: {out}");
    let digest = out
        .lines()
        .find_map(|line| line.trim().strip_prefix("Plan digest: "))
        .expect("the dry run prints its plan digest")
        .to_owned();
    assert!(out.contains(&format!("--target {target} --expect {digest}")), "{out}");
    assert_eq!(counts(&db).await, [0; 5]);

    // A real run names its database, or nothing is written.
    let blind = command(&["seed", path, "--as", "admin@example.test", "--no-admin-emails"]);
    assert_eq!(blind.status.code(), Some(2), "{}", String::from_utf8_lossy(&blind.stdout));
    assert_eq!(counts(&db).await, [0; 5]);
    let real = command(&[
        "seed",
        path,
        "--as",
        "admin@example.test",
        "--no-admin-emails",
        "--target",
        &target,
        "--expect",
        &digest,
    ]);
    let out = String::from_utf8_lossy(&real.stdout);
    assert_eq!(real.status.code(), Some(0), "{out}{}", String::from_utf8_lossy(&real.stderr));
    // What it is about to write comes first, then that it was written.
    let planned = out.find("TIN ***-***-453-00000 active on jane@example.test (new)").expect(&out);
    let made = out.find("change(s) made").expect(&out);
    assert!(out.find(&format!("Database: {target}")).unwrap() < planned && planned < made, "{out}");
    assert!(!out.contains("000-123-453"), "{out}");
    let again = command(&["seed", path, "--as", "admin@example.test", "--no-admin-emails", "--target", &target]);
    assert!(String::from_utf8_lossy(&again.stdout).contains("Nothing to change."));

    std::fs::write(dir.join("seed.toml"), "[[login]]\nemail = \"x@example.test\"\nwho = 1\n").unwrap();
    let bad = command(&["seed", path, "--as", "admin@example.test", "--no-admin-emails", "--target", &target]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("Refused; nothing was written"));
    assert_eq!(command(&["seed", path]).status.code(), Some(2));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A database at the same number but with other migration files (another
/// build's 0011 or 0012) is refused too.
#[sqlx::test(migrations = "./migrations")]
async fn seed_refuses_a_database_whose_migrations_differ_from_this_release(db: PgPool) {
    sqlx::query("UPDATE _sqlx_migrations SET checksum = decode('00', 'hex') WHERE version = 12")
        .execute(&db)
        .await
        .unwrap();
    let problem = refused(seed(&db, EXAMPLE, &options("admin@example.test", false)).await);
    assert!(problem.contains("migration 12"), "{problem}");
    assert_eq!(counts(&db).await, [0; 5]);
}

/// The seed waits a few seconds at most for a lock another change holds
/// (a role change, a ban), then gives up having written nothing, so it
/// never holds those changes up for long.
#[sqlx::test(migrations = "./migrations")]
async fn seed_gives_up_on_a_lock_held_by_a_concurrent_role_change(db: PgPool) {
    // A role change on the website, under way: it holds the roles lock.
    let mut change = db.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(7095465005590572908)").execute(&mut *change).await.unwrap();
    let started = std::time::Instant::now();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        seed(&db, EXAMPLE, &options("admin@example.test", false)),
    )
    .await
    .expect("the seed waited for the lock without end");
    let error = outcome.expect_err("the seed ran while the roles were locked").to_string();
    assert!(error.contains("lock"), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    change.rollback().await.unwrap();
    assert_eq!(counts(&db).await, [0; 5]);
}

/// A real run writes only to the database the operator names with
/// `--target`, which must be the one it is connected to; a dry run names it.
#[sqlx::test(migrations = "./migrations")]
async fn a_real_run_must_name_the_database_it_is_connected_to(db: PgPool) {
    let target = target_of(&db).await;
    let run = |options: Options| {
        let db = db.clone();
        async move { seed::run(&db, &DocumentStore::Database, EXAMPLE.as_bytes(), &options).await }
    };
    let dry = run(options("admin@example.test", true)).await.unwrap();
    assert_eq!(dry.target, target);
    let problem = refused(run(options("admin@example.test", false)).await);
    assert!(problem.contains("--target"), "{problem}");
    for wrong in ["127.0.0.1:54330/buwiz_cloud".to_owned(), target.replace(":54330/", ":5432/"), format!("{target}x")] {
        let problem =
            refused(run(Options { target: Some(wrong.clone()), ..options("admin@example.test", false) }).await);
        assert!(problem.contains(&format!("connected to {target}, not {wrong}")), "{problem}");
    }
    assert_eq!(counts(&db).await, [0; 5]);
    let real = run(Options { target: Some(target.clone()), ..options("admin@example.test", false) }).await.unwrap();
    assert!(real.changes > 0);
}

/// `--expect` ties a real run to the dry run read before it: if the plan
/// changed in between, nothing is written.
#[sqlx::test(migrations = "./migrations")]
async fn a_real_run_with_expect_refuses_a_plan_that_changed_since_the_dry_run(db: PgPool) {
    let dry = seed(&db, EXAMPLE, &options("admin@example.test", true)).await.unwrap();
    assert_eq!(dry.digest.len(), 64, "{}", dry.digest);
    // Meanwhile jane signs up on the website: the plan is not the same.
    sqlx::query("INSERT INTO accounts (email) VALUES ('jane@example.test')").execute(&db).await.unwrap();
    let expecting = |digest: &str| Options { expect: Some(digest.to_owned()), ..options("admin@example.test", false) };
    let problem = refused(seed(&db, EXAMPLE, &expecting(&dry.digest)).await);
    assert!(problem.contains("plan") && problem.contains(&dry.digest), "{problem}");
    assert_eq!(counts(&db).await, [1, 0, 0, 0, 0]);
    // The dry run again gives the digest the real run then accepts.
    let again = seed(&db, EXAMPLE, &options("admin@example.test", true)).await.unwrap();
    assert_ne!(again.digest, dry.digest);
    let real = seed(&db, EXAMPLE, &expecting(&again.digest)).await.unwrap();
    assert_eq!((real.digest, real.changes), (again.digest, again.changes));
}

/// A real run never shows or stores a full TIN, not even in what it
/// refuses; it records who ran it as far as the machine knows.
#[sqlx::test(migrations = "./migrations")]
async fn a_real_run_shows_no_full_tin_and_records_who_ran_it(db: PgPool) {
    let typo = format!(
        "{EXAMPLE}\n[[login]]\nemail = \"typo@example.test\"\n  [[login.tin]]\n  tin = \"000-987-65\"\n  kind = \"personal\"\n  name = \"T\"\n  state = \"not_verified\"\n"
    );
    let real = seed(&db, &typo, &options("admin@example.test", false)).await.unwrap();
    let lines = real.lines.join("\n");
    assert!(lines.contains("***-***-453-00000") && lines.contains("refused TIN"), "{lines}");
    for full in ["000-123-453", "000123453", "000-987-65", "00098765", "98765"] {
        assert!(!lines.contains(full), "{full}: {lines}");
    }
    let (os_user, host, target, summary): (String, String, String, String) =
        sqlx::query_as("SELECT os_user, host_name, target, summary::text FROM seed_runs WHERE id = $1")
            .bind(real.run_id)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!((os_user.as_str(), host.as_str()), ("ops (sudo from ana)", "box.example.test"));
    assert_eq!(target, target_of(&db).await);
    for full in ["000-123-453", "000123453", "000-987-65", "98765"] {
        assert!(!summary.contains(full), "{full}: {summary}");
    }
    // Refusals too: full in a dry run, so a typo shows; masked in a real one.
    let twice = "[[login]]\nemail = \"a@example.test\"\n[[login.tin]]\ntin = \"000-123-459\"\nkind = \"personal\"\nname = \"A\"\nstate = \"not_verified\"\n[[login]]\nemail = \"b@example.test\"\n[[login.tin]]\ntin = \"000123459\"\nkind = \"personal\"\nname = \"B\"\nstate = \"not_verified\"\n";
    let problem = refused(seed(&db, twice, &options("admin@example.test", true)).await);
    assert!(problem.contains("TIN 000-123-459-00000: in the file twice"), "{problem}");
    let problem = refused(seed(&db, twice, &options("admin@example.test", false)).await);
    assert!(
        problem.contains("TIN ***-***-459-00000: in the file twice") && !problem.contains("000-123-459"),
        "{problem}"
    );
}

/// Who is an Admin comes from the server's own `ADMIN_EMAILS`: a list must
/// be given (or `--no-admin-emails`, for a server without one). While the
/// server has a list, the file cannot make `--as` the first Admin: an
/// Admin from that list runs it.
#[sqlx::test(migrations = "./migrations")]
async fn the_first_admin_comes_from_the_file_only_when_the_server_has_no_list(db: PgPool) {
    let make_eve = "[[login]]\nemail = \"eve@example.test\"\nrole = \"admin\"\n";
    let problem = refused(seed(&db, make_eve, &listing("eve@example.test", false, &[])).await);
    assert!(problem.contains("ADMIN_EMAILS is empty"), "{problem}");
    let problem = refused(seed(&db, make_eve, &listing("eve@example.test", false, &["root@example.test"])).await);
    assert!(problem.contains("--as eve@example.test is not an Admin"), "{problem}");
    assert!(problem.contains("ADMIN_EMAILS"), "{problem}");
    assert_eq!(counts(&db).await, [0; 5]);
    // An Admin of the server's list may run it, before ever signing in.
    let report = seed(&db, make_eve, &listing("root@example.test", false, &["root@example.test"])).await.unwrap();
    assert!(
        report.lines.iter().any(|line| line == "role eve@example.test No staff role → Admin"),
        "{:?}",
        report.lines
    );
}

/// The command refuses to guess the list from the shell it runs in.
#[sqlx::test(migrations = "./migrations")]
async fn the_seed_command_needs_admin_emails_or_says_there_is_none(db: PgPool) {
    let (dir, path, command) = command_in(&db).await;
    let target = target_of(&db).await;
    let missing = command(&["seed", &path, "--as", "admin@example.test", "--dry-run"], &[]);
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert_eq!(missing.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("ADMIN_EMAILS") && stderr.contains("--no-admin-emails"), "{stderr}");
    let both = command(
        &["seed", &path, "--as", "root@example.test", "--dry-run", "--no-admin-emails"],
        &[("ADMIN_EMAILS", "root@example.test")],
    );
    assert_eq!(both.status.code(), Some(2), "{}", String::from_utf8_lossy(&both.stderr));
    // With the server's list, it says which list it used.
    let listed = command(
        &["seed", &path, "--as", "root@example.test", "--dry-run"],
        &[("ADMIN_EMAILS", " Root@Example.test ,ops@example.test")],
    );
    let out = String::from_utf8_lossy(&listed.stdout);
    assert_eq!(listed.status.code(), Some(0), "{out}{}", String::from_utf8_lossy(&listed.stderr));
    assert!(out.contains("ADMIN_EMAILS: root@example.test, ops@example.test"), "{out}");
    assert!(out.contains(&format!("--target {target}")), "{out}");
    assert_eq!(counts(&db).await, [0; 5]);
    std::fs::remove_dir_all(&dir).unwrap();
}
