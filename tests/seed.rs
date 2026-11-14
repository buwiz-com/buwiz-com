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
        no_dry_run_check: false,
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

/// Runs the file as `options` say. A real run goes as the operator runs
/// it: without a `--target` it gets the test database's, and without an
/// `--expect` (or `--no-dry-run-check`) the dry run goes first and the real
/// run expects its digest. When that dry run refuses, the real run is still
/// made, unchecked, so the answer is the real run's own.
async fn seed(db: &PgPool, file: &str, options: &Options) -> Result<Report, SeedError> {
    let mut options = options.clone();
    if !options.dry_run && options.target.is_none() {
        options.target = Some(target_of(db).await);
    }
    if !options.dry_run && options.expect.is_none() && !options.no_dry_run_check {
        let dry = Options { dry_run: true, target: None, ..options.clone() };
        match seed::run(db, &DocumentStore::Database, file.as_bytes(), &dry).await {
            Ok(dry) => options.expect = Some(dry.digest),
            Err(_) => options.no_dry_run_check = true,
        }
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
    // And the file cannot leave nobody in charge: whoever runs it stays an
    // Admin, since it cannot change its own role.
    let demote = "[[login]]\nemail = \"eve@example.test\"\nrole = \"none\"\n[[login]]\nemail = \"mallory@example.test\"\nrole = \"none\"\n";
    let problem = refused(seed(&db, demote, &options("eve@example.test", false)).await);
    assert!(problem.contains("--as eve@example.test cannot change its own role"), "{problem}");
    assert_eq!(account(&db, "eve@example.test").await.1.as_deref(), Some("admin"));
    assert_eq!(account(&db, "mallory@example.test").await.1.as_deref(), Some("admin"));
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
    // Only ever a database on this machine (CI names it localhost).
    assert!(matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")), "{:?}", url.host_str());
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
    let path = path.as_str();
    let none = ("ADMIN_EMAILS", "");
    let command = |args: &[&str]| command(args, &[none]);

    let dry = command(&["seed", path, "--as", "admin@example.test", "--dry-run", "--no-admin-emails"]);
    let out = String::from_utf8_lossy(&dry.stdout);
    assert_eq!(dry.status.code(), Some(0), "{out}{}", String::from_utf8_lossy(&dry.stderr));
    assert!(out.contains(&format!("Database: {target}")), "{out}");
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
    // A real run without the dry run's digest is refused, unless it says
    // on purpose that it has none.
    let again = command(&["seed", path, "--as", "admin@example.test", "--no-admin-emails", "--target", &target]);
    let stderr = String::from_utf8_lossy(&again.stderr);
    assert_eq!(again.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("--expect") && stderr.contains("Refused; nothing was written"), "{stderr}");
    let again = command(&[
        "seed",
        path,
        "--as",
        "admin@example.test",
        "--no-admin-emails",
        "--target",
        &target,
        "--no-dry-run-check",
    ]);
    let out = String::from_utf8_lossy(&again.stdout);
    assert_eq!(again.status.code(), Some(0), "{out}{}", String::from_utf8_lossy(&again.stderr));
    assert!(out.contains("Nothing to change."), "{out}");
    assert!(String::from_utf8_lossy(&again.stderr).contains("--no-dry-run-check"), "it says it was not checked");
    // The missing --target is refused without printing the value to paste.
    let blind = String::from_utf8_lossy(&blind.stderr).into_owned();
    assert!(blind.contains("--target") && blind.contains("dry run") && !blind.contains(&target), "{blind}");
    // Nor can a run be both checked and unchecked, or a dry run unchecked.
    for args in [
        &["seed", path, "--as", "a@example.test", "--target", &target, "--expect", "d", "--no-dry-run-check"][..],
        &["seed", path, "--as", "a@example.test", "--dry-run", "--no-dry-run-check"][..],
    ] {
        assert_eq!(command(args).status.code(), Some(2), "{args:?}");
    }

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
    // It says where to find the value, and does not print it: a real run
    // pastes the dry run's line, after reading that dry run.
    assert!(problem.contains("--target") && problem.contains("dry run"), "{problem}");
    assert!(!problem.contains(&target), "{problem}");
    // Another database on the same server, the same database on another
    // port, and a near miss of the name.
    let (address, _) = target.rsplit_once('/').unwrap();
    let (host, port) = address.rsplit_once(':').unwrap();
    let other_port = port.parse::<u16>().unwrap().wrapping_add(1);
    let name = target.rsplit('/').next().unwrap();
    for wrong in [format!("{address}/not_{name}"), format!("{host}:{other_port}/{name}"), format!("{target}x")] {
        let problem =
            refused(run(Options { target: Some(wrong.clone()), ..options("admin@example.test", false) }).await);
        assert!(problem.contains(&format!("connected to {target}, not {wrong}")), "{problem}");
    }
    assert_eq!(counts(&db).await, [0; 5]);
    let real = Options {
        target: Some(target.clone()),
        expect: Some(dry.digest.clone()),
        ..options("admin@example.test", false)
    };
    let real = run(real).await.unwrap();
    assert!(real.changes > 0);
}

/// A real run commits only the plan a dry run showed: it needs that dry
/// run's `--expect`, unless `--no-dry-run-check` says on purpose that no
/// dry run was read.
#[sqlx::test(migrations = "./migrations")]
async fn a_real_run_needs_the_dry_run_digest_or_says_it_has_none(db: PgPool) {
    let target = target_of(&db).await;
    let run = |options: Options| {
        let db = db.clone();
        async move { seed::run(&db, &DocumentStore::Database, EXAMPLE.as_bytes(), &options).await }
    };
    let blind = Options { target: Some(target.clone()), ..options("admin@example.test", false) };
    let problem = refused(run(blind.clone()).await);
    assert!(problem.contains("--expect") && problem.contains("dry run"), "{problem}");
    assert!(problem.contains("--no-dry-run-check"), "{problem}");
    assert_eq!(counts(&db).await, [0; 5]);
    let unchecked = run(Options { no_dry_run_check: true, ..blind }).await.unwrap();
    assert!(unchecked.changes > 0);
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

/// The file may not leave only Admins nobody signed in as, where an Admin
/// who had signed in was left before: one mistyped address would leave
/// nobody able to sign in as an Admin. A first Admin made on a database
/// with none (above) is no such change.
#[sqlx::test(migrations = "./migrations")]
async fn the_file_cannot_leave_only_admins_who_never_signed_in(db: PgPool) {
    let site = Site::with_admins(db, &[]);
    site.staff_with_role("ana@example.test", "admin").await;
    let root = listing("root@example.test", false, &["root@example.test"]);
    let demote = "[[login]]\nemail = \"ana@example.test\"\nrole = \"none\"\n";
    let problem = refused(seed(&site.db, demote, &root).await);
    assert!(problem.contains("the roles in the file leave Buwiz with no Admin who has signed in"), "{problem}");
    assert_eq!(account(&site.db, "ana@example.test").await.1.as_deref(), Some("admin"));
    // Once the server's Admin has signed in, the same file runs.
    site.person("root@example.test").await;
    seed(&site.db, demote, &root).await.unwrap();
    assert_eq!(account(&site.db, "ana@example.test").await.1, None);
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
    // A list separated by something else than commas is noticed.
    let semicolons = command(
        &["seed", &path, "--as", "root@example.test", "--dry-run"],
        &[("ADMIN_EMAILS", "root@example.test; ops@example.test")],
    );
    let stderr = String::from_utf8_lossy(&semicolons.stderr);
    assert!(
        stderr.contains("not an email address") && stderr.contains("root@example.test; ops@example.test"),
        "{stderr}"
    );
    assert!(out.contains(&format!("--target {target}")), "{out}");
    assert_eq!(counts(&db).await, [0; 5]);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Moving a paused TIN would lift its pause without a word: refused, and
/// a TIN that was ever paused moves only when its entry says so.
#[sqlx::test(migrations = "./migrations")]
async fn a_move_from_a_paused_owner_is_refused(db: PgPool) {
    let site = Site::new(db);
    let db = &site.db;
    let (_, staff) = site.person(STAFF).await;
    let opts = listing(STAFF, false, &[STAFF]);
    let (old, _) = site.person("old@example.test").await;
    let claim = add_claim(db, old, "000123456", "approved").await;
    let response = site.post(&format!("/staff/claims/{claim}/pause"), &staff, "reason=Fraud+complaint").await;
    assert_eq!(location(&response), format!("/staff/claims/{claim}?notice=tin_paused"));
    let file = |extra: &str| {
        format!(
            "[[login]]\nemail = \"new@example.test\"\n[[login.tin]]\ntin = \"000-123-456\"\nkind = \"business\"\nname = \"N\"\nstate = \"active\"\nreason = \"Moves\"\nmove_from = \"old@example.test\"\n{extra}"
        )
    };
    let before = counts(db).await;
    for extra in ["", "reapply = true\n"] {
        let problem = refused(seed(db, &file(extra), &opts).await);
        assert!(
            problem.contains("TIN ***-***-456-00000 for new@example.test: it is paused on old@example.test"),
            "{problem}"
        );
        assert!(problem.contains("resume it or revoke it on the staff pages first"), "{problem}");
    }
    assert_eq!(counts(db).await, before);
    assert_eq!(site.claim_status(claim).await, "approved");

    // Resumed, it was still paused once: the entry must say to go ahead.
    let response = site.post(&format!("/staff/claims/{claim}/resume"), &staff, "reason=Cleared").await;
    assert_eq!(location(&response), format!("/staff/claims/{claim}?notice=tin_resumed"));
    let problem = refused(seed(db, &file(""), &opts).await);
    assert!(problem.contains("it was paused before") && problem.contains("reapply = true"), "{problem}");
    let report = seed(db, &file("reapply = true\n"), &opts).await.unwrap();
    assert_eq!(report.changes, 2, "{:?}", report.lines);
    assert_eq!(site.claim_status(claim).await, "revoked");
}

/// Running an old file again keeps what staff decided on the website
/// since: a role and a test label set there stay, and each is reported.
#[sqlx::test(migrations = "./migrations")]
async fn a_rerun_keeps_a_role_and_a_label_staff_changed_on_the_website(db: PgPool) {
    let site = Site::new(db);
    let db = &site.db;
    let (_, staff) = site.person(STAFF).await;
    let opts = listing(STAFF, false, &[STAFF]);
    let file = "[[login]]\nemail = \"bob@example.test\"\nrole = \"admin\"\ntest = true\n";
    seed(db, file, &opts).await.unwrap();
    let (bob, role, test) = account(db, "bob@example.test").await;
    assert_eq!((role.as_deref(), test), (Some("admin"), true));
    // An Admin demotes bob for cause and takes the label off.
    let user = format!("/staff/users/{bob}");
    let response = site.post(&format!("{user}/role"), &staff, "role=none&reason=For+cause").await;
    assert_eq!(location(&response), format!("{user}?notice=role_changed"));
    let response = site.post(&format!("{user}/test"), &staff, "test=off&reason=Real+customer").await;
    assert_eq!(location(&response), format!("{user}?notice=test_unmarked"));

    let before = counts(db).await;
    let again = seed(db, file, &opts).await.unwrap();
    assert_eq!(again.changes, 0, "{:?}", again.lines);
    let (_, role, test) = account(db, "bob@example.test").await;
    assert_eq!((role, test), (None, false));
    let lines = again.lines.join("\n");
    assert!(
        lines.contains(
            "kept role bob@example.test No staff role (the file says Admin): its newest change was made by staff on the website"
        ),
        "{lines}"
    );
    assert!(
        lines.contains(
            "kept test label bob@example.test off (the file says on): its newest change was made by staff on the website"
        ),
        "{lines}"
    );
    assert_eq!(counts(db).await[..4], before[..4]);
}

/// A TIN that staff revoked, or a TIN saved without papers that staff
/// deleted, is not put back by running the file again, unless its entry
/// says `reapply = true`.
#[sqlx::test(migrations = "./migrations")]
async fn a_rerun_does_not_bring_back_a_tin_ended_on_the_website(db: PgPool) {
    let site = Site::new(db);
    let db = &site.db;
    let (_, staff) = site.person(STAFF).await;
    let opts = listing(STAFF, false, &[STAFF]);
    seed(db, EXAMPLE, &opts).await.unwrap();
    let (jane, ..) = account(db, "jane@example.test").await;
    let (shop, ..) = account(db, "shop@example.test").await;
    let active = site.newest_claim(jane).await;
    let unverified = site.newest_claim(shop).await;
    let response = site.post(&format!("/staff/claims/{active}/revoke"), &staff, "reason=Not+hers").await;
    assert_eq!(location(&response), format!("/staff/claims/{active}?notice=ownership_revoked"));
    let response = site.post(&format!("/staff/claims/{unverified}/delete"), &staff, "reason=Typo").await;
    assert_eq!(location(&response), "/staff/queue?notice=claim_deleted");

    let before = counts(db).await;
    let problem = refused(seed(db, EXAMPLE, &opts).await);
    assert!(
        problem.contains("TIN ***-***-453-00000 on jane@example.test: it was revoked on this login on the website"),
        "{problem}"
    );
    assert!(
        problem.contains("TIN ***-***-454-00000 on shop@example.test: it was deleted from this login on the website"),
        "{problem}"
    );
    assert!(problem.contains("set reapply = true on its entry"), "{problem}");
    assert_eq!(counts(db).await, before);
    assert_eq!(site.claim_status(active).await, "revoked");

    // An owner who removed it is heard the same way.
    let (ana, ana_cookie) = site.person("ana@example.test").await;
    let removed = add_claim(db, ana, "000123460", "approved").await;
    let response = site.post(&format!("/tins/claims/{removed}/remove"), &ana_cookie, "").await;
    assert_eq!(location(&response), "/tins?notice=tin_removed");
    let ana_file = "[[login]]\nemail = \"ana@example.test\"\n[[login.tin]]\ntin = \"000123460\"\nkind = \"business\"\nname = \"A\"\nstate = \"active\"\nreason = \"x\"\n";
    let problem = refused(seed(db, ana_file, &opts).await);
    assert!(problem.contains("it was removed on this login on the website"), "{problem}");

    // Said explicitly, the file wins.
    let reapplied = EXAMPLE
        .replace("reason = \"Owner's test login\"", "reason = \"Owner's test login\"\n  reapply = true")
        .replace(
            "state = \"not_verified\"    # saved without papers; it can prepare, not send",
            "state = \"not_verified\"\n  reapply = true",
        );
    let report = seed(db, &reapplied, &opts).await.unwrap();
    assert_eq!(report.changes, 2, "{:?}", report.lines);
    assert_eq!(claim_of(db, jane, "000123453").await.0, "approved");
    assert_eq!(claim_of(db, shop, "000123454").await.0, "unverified");
}

/// Like the website, the seed never changes the role of the one running
/// it, except to make the first Admin on a server with none.
#[sqlx::test(migrations = "./migrations")]
async fn the_seed_never_changes_the_role_of_whoever_runs_it(db: PgPool) {
    seed(&db, "[[login]]\nemail = \"eve@example.test\"\nrole = \"admin\"\n", &options("eve@example.test", false))
        .await
        .unwrap();
    for role in ["support", "none"] {
        let file = format!("[[login]]\nemail = \"eve@example.test\"\nrole = \"{role}\"\n");
        let problem = refused(seed(&db, &file, &options("eve@example.test", false)).await);
        assert!(problem.contains("--as eve@example.test cannot change its own role"), "{problem}");
    }
    assert_eq!(account(&db, "eve@example.test").await.1.as_deref(), Some("admin"));
}

/// Every account and claim the seed makes has an audit line: the `--as`
/// account too when the run makes it, and a TIN saved as not verified. A
/// row the run makes has no status before it.
#[sqlx::test(migrations = "./migrations")]
async fn every_account_and_claim_the_seed_makes_has_an_audit_line(db: PgPool) {
    let report = seed(&db, EXAMPLE, &listing("root@example.test", false, &["root@example.test"])).await.unwrap();
    assert!(
        report.lines.iter().any(|line| line == "created login root@example.test (runs this seed)"),
        "{:?}",
        report.lines
    );
    let (root, ..) = account(&db, "root@example.test").await;
    let lines = account_audit_lines(&db, root).await;
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(
        (lines[0].0.as_str(), lines[0].3.as_str(), lines[0].4.as_str()),
        ("created", "root@example.test", "seed")
    );
    let (shop, ..) = account(&db, "shop@example.test").await;
    let shop_lines = claim_audit_lines(&db, shop).await;
    assert_eq!(shop_lines.len(), 1, "{shop_lines:?}");
    assert_eq!(
        (shop_lines[0].0.as_str(), shop_lines[0].1.as_str(), shop_lines[0].3.as_str()),
        ("created", "new", "seed")
    );
    let (jane, ..) = account(&db, "jane@example.test").await;
    let jane_lines = claim_audit_lines(&db, jane).await;
    assert_eq!((jane_lines[0].0.as_str(), jane_lines[0].1.as_str()), ("activated", "new"));
    // Every account the run made, and every claim, has its line.
    let unaudited: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM accounts a WHERE NOT EXISTS (SELECT 1 FROM account_audit l WHERE l.account_id = a.id))
              + (SELECT count(*) FROM tin_claims c WHERE NOT EXISTS (SELECT 1 FROM claim_audit l WHERE l.claim_id = c.id))",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(unaudited, 0);
    // Each claim line names the claim's kind, so it outlives the claim.
    let kindless: i64 =
        sqlx::query_scalar("SELECT count(*) FROM claim_audit WHERE claim_kind IS NULL").fetch_one(&db).await.unwrap();
    assert_eq!(kindless, 0);
    // Run again, nothing is made and nothing is said to be.
    let again = seed(&db, EXAMPLE, &listing("root@example.test", false, &["root@example.test"])).await.unwrap();
    assert_eq!(again.changes, 0, "{:?}", again.lines);
}

/// A waiting claim a move deletes may have its files in object storage:
/// without the object store the run is refused, and a file it could not
/// delete there is reported, never said to be deleted.
#[sqlx::test(migrations = "./migrations")]
async fn a_move_that_deletes_stored_files_needs_the_object_store(db: PgPool) {
    seed(&db, "[[login]]\nemail = \"admin@example.test\"\nrole = \"admin\"\n", &options("admin@example.test", false))
        .await
        .unwrap();
    let waiting: Uuid = sqlx::query_scalar("INSERT INTO accounts (email) VALUES ('waiting@example.test') RETURNING id")
        .fetch_one(&db)
        .await
        .unwrap();
    let claim = add_claim(&db, waiting, "000123457", "pending").await;
    sqlx::query(
        "INSERT INTO claim_documents (claim_id, kind, filename, content_type, size_bytes, sha256, nonce, object_key)
         VALUES ($1, 'form_2303', 'f.png', 'image/png', 5, decode('00', 'hex'), decode('01', 'hex'), 'claims/' || $1 || '/f')",
    )
    .bind(claim)
    .execute(&db)
    .await
    .unwrap();
    let file = "[[login]]\nemail = \"new@example.test\"\n[[login.tin]]\ntin = \"000-123-457\"\nkind = \"business\"\nname = \"N\"\nstate = \"active\"\nreason = \"Moves\"\nmove_from = \"waiting@example.test\"\n";
    let before = counts(&db).await;
    for dry_run in [true, false] {
        let problem = refused(seed(&db, file, &options("admin@example.test", dry_run)).await);
        assert!(problem.contains("stored files in object storage") && problem.contains("S3_"), "{problem}");
    }
    assert_eq!(counts(&db).await, before);

    // The command, with an object store that cannot be reached: the move
    // is made, and the file left behind is named on stderr, exit 1.
    let (dir, path, command) = command_in(&db).await;
    std::fs::write(&path, file).unwrap();
    let target = target_of(&db).await;
    let unreachable = [
        ("S3_ENDPOINT", "http://127.0.0.1:9"),
        ("S3_BUCKET", "buwiz-test"),
        ("S3_ACCESS_KEY_ID", "test"),
        ("S3_SECRET_ACCESS_KEY", "test"),
        ("ADMIN_EMAILS", ""),
    ];
    let args =
        ["seed", &path, "--as", "admin@example.test", "--no-admin-emails", "--target", &target, "--no-dry-run-check"];
    let run = command(&args, &unreachable);
    let (out, err) = (String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
    assert_eq!(run.status.code(), Some(1), "{out}{err}");
    assert!(err.contains(&format!("claims/{claim}/f")) && err.contains("could not be deleted"), "{err}");
    assert!(!out.contains("files deleted") && !out.contains("file(s) deleted"), "{out}");
    assert_eq!(count(&db, "tin_claims").await, 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Activating a TIN on which other accounts have a claim waiting, or saved
/// without papers, says so in the plan: those claims can no longer be
/// approved while it is active. The dry run names them; a real run says
/// "another account". The notes are part of the plan's digest.
#[sqlx::test(migrations = "./migrations")]
async fn activating_a_tin_others_wait_on_says_so_in_the_plan(db: PgPool) {
    let opts = options("admin@example.test", false);
    seed(&db, "[[login]]\nemail = \"admin@example.test\"\nrole = \"admin\"\n", &opts).await.unwrap();
    let carl: Uuid = sqlx::query_scalar("INSERT INTO accounts (email) VALUES ('carl@example.test') RETURNING id")
        .fetch_one(&db)
        .await
        .unwrap();
    let dora: Uuid = sqlx::query_scalar("INSERT INTO accounts (email) VALUES ('dora@example.test') RETURNING id")
        .fetch_one(&db)
        .await
        .unwrap();
    let waiting = add_claim(&db, carl, "000123456", "pending").await;
    add_claim(&db, dora, "000123456", "unverified").await;
    let file = "[[login]]\nemail = \"new@example.test\"\n[[login.tin]]\ntin = \"000-123-456\"\nkind = \"personal\"\nname = \"N E W\"\nstate = \"active\"\nreason = \"Known\"\n";

    let dry = seed(&db, file, &options("admin@example.test", true)).await.unwrap();
    let lines = dry.lines.join("\n");
    assert!(
        lines.contains(
            "note: TIN 000-123-456-00000 on new@example.test: carl@example.test has a claim on it waiting for review"
        ),
        "{lines}"
    );
    assert!(lines.contains("dora@example.test has a claim on it saved as not verified"), "{lines}");
    // Without the waiting claim the plan is another one.
    sqlx::query("UPDATE tin_claims SET status = 'withdrawn' WHERE id = $1").bind(waiting).execute(&db).await.unwrap();
    let fewer = seed(&db, file, &options("admin@example.test", true)).await.unwrap();
    assert_ne!(fewer.digest, dry.digest);
    assert!(!fewer.lines.join("\n").contains("carl@example.test"), "{:?}", fewer.lines);
    sqlx::query("UPDATE tin_claims SET status = 'pending' WHERE id = $1").bind(waiting).execute(&db).await.unwrap();

    let real = seed(&db, file, &opts).await.unwrap();
    let lines = real.lines.join("\n");
    assert!(
        lines.contains(
            "note: TIN ***-***-456-00000 on new@example.test: another account has a claim on it waiting for review"
        ),
        "{lines}"
    );
    assert!(lines.contains("another account has a claim on it saved as not verified"), "{lines}");
    assert!(!lines.contains("carl@example.test") && !lines.contains("dora@example.test"), "{lines}");
    assert_eq!(real.digest, dry.digest);
}

/// A website change that began before a seed run but committed after it
/// is newer than the seed's: the next run keeps it. The history is ordered
/// by when each line was written, not by when its transaction began.
#[sqlx::test(migrations = "./migrations")]
async fn a_website_change_committed_after_a_seed_run_is_kept_by_the_next(db: PgPool) {
    let opts = options("admin@example.test", false);
    let file = "[[login]]\nemail = \"admin@example.test\"\nrole = \"admin\"\n[[login]]\nemail = \"bob@example.test\"\nrole = \"support\"\n";
    seed(&db, file, &opts).await.unwrap();
    let (admin, ..) = account(&db, "admin@example.test").await;
    let (bob, ..) = account(&db, "bob@example.test").await;
    // An Admin's change of bob's role starts on the website...
    let mut website = db.begin().await.unwrap();
    sqlx::query("SELECT 1").execute(&mut *website).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    // ...a seed run goes through meanwhile, setting bob to Admin...
    let promote = file.replace("role = \"support\"", "role = \"admin\"");
    seed(&db, &promote, &opts).await.unwrap();
    assert_eq!(account(&db, "bob@example.test").await.1.as_deref(), Some("admin"));
    // ...and the website's change writes and commits after it.
    sqlx::query("UPDATE accounts SET role = NULL WHERE id = $1;").bind(bob).execute(&mut *website).await.unwrap();
    sqlx::query(
        "INSERT INTO account_audit (account_id, action, old_value, new_value, actor_id, actor_email, source, reason)
         VALUES ($1, 'role_changed', 'admin', NULL, $2, 'admin@example.test', 'web', 'For cause')",
    )
    .bind(bob)
    .bind(admin)
    .execute(&mut *website)
    .await
    .unwrap();
    website.commit().await.unwrap();

    let again = seed(&db, &promote, &opts).await.unwrap();
    assert_eq!(again.changes, 0, "{:?}", again.lines);
    assert!(
        again.lines.iter().any(|line| line.starts_with("kept role bob@example.test No staff role")),
        "{:?}",
        again.lines
    );
    assert_eq!(account(&db, "bob@example.test").await.1, None);
}

/// What the seed itself ended on a login (a TIN it moved to another) is
/// said to be the seed's doing, not the website's; it still needs
/// `reapply = true` to come back.
#[sqlx::test(migrations = "./migrations")]
async fn an_ending_made_by_an_earlier_seed_run_is_named_as_such(db: PgPool) {
    let site = Site::new(db);
    let db = &site.db;
    let opts = options("admin@example.test", false);
    seed(db, "[[login]]\nemail = \"admin@example.test\"\nrole = \"admin\"\n", &opts).await.unwrap();
    let (old, _) = site.person("old@example.test").await;
    add_claim(db, old, "000123456", "approved").await;
    let (waiting, waiting_cookie) = site.person("waiting@example.test").await;
    let parts = [
        ("kind", Text("personal")),
        ("tin", Text("000123457")),
        ("registered_name", Text("W")),
        ("tin_id", File("tin.png", PNG)),
    ];
    site.upload("/tins/claims", &waiting_cookie, &parts).await;
    let _ = waiting;
    let entry = |email: &str, tin: &str, from: &str, extra: &str| {
        format!(
            "[[login]]\nemail = \"{email}\"\n[[login.tin]]\ntin = \"{tin}\"\nkind = \"business\"\nname = \"N\"\nstate = \"active\"\nreason = \"Moves\"\nmove_from = \"{from}\"\n{extra}"
        )
    };
    let away = format!(
        "{}{}",
        entry("new@example.test", "000-123-456", "old@example.test", ""),
        entry("other@example.test", "000-123-457", "waiting@example.test", "")
    );
    seed(db, &away, &opts).await.unwrap();
    // Moving them back: refused, and the seed is named as what ended them.
    let back = format!(
        "{}{}",
        entry("old@example.test", "000-123-456", "new@example.test", ""),
        entry("waiting@example.test", "000-123-457", "other@example.test", "")
    );
    let problem = refused(seed(db, &back, &opts).await);
    assert!(
        problem.contains("TIN ***-***-456-00000 on old@example.test: an earlier seed run moved it from this login"),
        "{problem}"
    );
    assert!(
        problem.contains(
            "TIN ***-***-457-00000 on waiting@example.test: an earlier seed run deleted its claim on this login"
        ),
        "{problem}"
    );
    assert!(!problem.contains("on the website"), "{problem}");
    let back = format!(
        "{}{}",
        entry("old@example.test", "000-123-456", "new@example.test", "reapply = true\n"),
        entry("waiting@example.test", "000-123-457", "other@example.test", "reapply = true\n")
    );
    assert_eq!(seed(db, &back, &opts).await.unwrap().changes, 2);
}

/// The seed activating the `--as` Admin's own TIN is flagged on the staff
/// pages like an Admin activating their own on the website.
#[sqlx::test(migrations = "./migrations")]
async fn the_seed_activating_its_own_admins_tin_is_flagged(db: PgPool) {
    let site = Site::new(db);
    let db = &site.db;
    let (_, other_admin) = site.staff_with_role("boss@example.test", "admin").await;
    let file = format!(
        "[[login]]\nemail = \"{STAFF}\"\n[[login.tin]]\ntin = \"000-123-453\"\nkind = \"personal\"\nname = \"S T A F F\"\nstate = \"active\"\nreason = \"Mine\"\n[[login]]\nemail = \"jo@example.test\"\n[[login.tin]]\ntin = \"000-123-454\"\nkind = \"personal\"\nname = \"J O\"\nstate = \"active\"\nreason = \"Hers\"\n"
    );
    seed(db, &file, &listing(STAFF, false, &[STAFF])).await.unwrap();
    let (own, ..) = account(db, STAFF).await;
    let (jo, ..) = account(db, "jo@example.test").await;
    let own_claim = site.newest_claim(own).await;
    let jos = site.newest_claim(jo).await;
    let page = text(site.get(&format!("/staff/claims/{own_claim}"), &other_admin).await).await;
    assert!(page.contains("Activated by its owner, an admin"), "{page}");
    let page = text(site.get(&format!("/staff/claims/{jos}"), &other_admin).await).await;
    assert!(!page.contains("Activated by its owner, an admin"), "{page}");
    let queue = text(site.get("/staff/queue?status=approved", &other_admin).await).await;
    assert!(queue.contains("Activated by its owner, an admin"), "{queue}");
}

/// A test TIN goes in only when its entry says `test_tin = true`, active,
/// on a login with the test label; the label then stays on.
#[sqlx::test(migrations = "./migrations")]
async fn a_test_tin_goes_in_only_on_purpose_on_a_test_login(db: PgPool) {
    let site = Site::with_admins(db, &[]);
    let db = &site.db;
    let admin = "[[login]]\nemail = \"admin@example.test\"\nrole = \"admin\"\n";
    seed(db, admin, &options("admin@example.test", false)).await.unwrap();
    let entry = |test: &str, tin: &str, extra: &str| {
        format!(
            "[[login]]\nemail = \"jane@example.test\"\n{test}[[login.tin]]\ntin = \"{tin}\"\nkind = \"personal\"\n\
             name = \"JANE TEST\"\nstate = \"active\"\nreason = \"Trying the app\"\n{extra}"
        )
    };
    let run = |file: String| async move { seed(db, &file, &options("admin@example.test", false)).await };
    for (file, words) in [
        (entry("test = true\n", "000-123-453", "test_tin = true\n"), "test_tin = true is only for a made-up TIN"),
        (entry("", "000-000-000-00001", "test_tin = true\n"), "a test TIN goes on a test account only"),
        (
            entry("test = true\n", "000-000-000-00001", "test_tin = true\n").replace("\"active\"", "\"not_verified\""),
            "a test TIN is only ever active",
        ),
    ] {
        let problem = refused(run(file).await);
        assert!(problem.contains(words), "{words}: {problem}");
    }
    let none: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tin_claims WHERE tin_root = '000000000'").fetch_one(db).await.unwrap();
    assert_eq!(none, 0);
    // Without test_tin, it is refused on its own as any TIN that is not
    // valid; the login is still made.
    let report = run(entry("test = true\n", "000-000-000-00001", "")).await.unwrap();
    assert!(report.lines.iter().any(|line| line.contains("refused TIN")), "{:?}", report.lines);

    let report = run(entry("test = true\n", "000-000-000-00001", "test_tin = true\n")).await.unwrap();
    assert!(
        report.lines.iter().any(|line| line.contains("TIN ***-***-000-00001 active on jane@example.test")),
        "{:?}",
        report.lines
    );
    let (jane, _, labelled) = account(db, "jane@example.test").await;
    assert!(labelled);
    let (status, verified_by, ..) = claim_of(db, jane, "000000000").await;
    assert_eq!((status.as_str(), verified_by.as_deref()), ("approved", Some("seed")));
    let lines = claim_audit_lines(db, jane).await;
    assert_eq!((lines[0].0.as_str(), lines[0].3.as_str()), ("activated", "seed"));

    // The label stays while jane holds it.
    let problem = refused(run(entry("test = false\n", "000-000-000-00001", "test_tin = true\n")).await);
    assert!(problem.contains("login jane@example.test: it holds a test TIN, so the test label stays"), "{problem}");
    assert!(account(db, "jane@example.test").await.2);
}
