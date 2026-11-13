//! `buwiz-cloud seed <FILE> --as <admin email> [--dry-run]`: creates
//! logins, their staff roles and test labels, and their TINs from one TOML
//! file, in one transaction, with a record of what it did (`seed_runs`, and
//! a line in `account_audit` or `claim_audit` for each change, from source
//! `seed`). See `docs/seed.example.toml` for the file.
//!
//! It changes only what differs from the file, so a second run with the
//! same file changes nothing. It never runs migrations and starts no
//! server: it refuses a database whose applied migrations are not exactly
//! this release's (by number and checksum). It waits at most
//! [`LOCK_TIMEOUT`] for a lock another change holds.
//!
//! What it refuses, before anything is written (the whole file):
//! - a file that does not parse, or has a key it does not know;
//! - a bad email, the same login or the same TIN twice;
//! - an active TIN without a reason, or on a banned account;
//! - an active TIN another account owns, unless `move_from` names that
//!   account; and `move_from` naming an account with nothing to move;
//! - a TIN whose papers are waiting for review on the login (decide it on
//!   /staff);
//! - `--as` that is not an Admin (on a database with no Admin at all, the
//!   file may make `--as` the first one);
//! - a change of roles that leaves no Admin.
//!
//! A TIN that is not valid (000-000-000, or not 9 digits) is refused on its
//! own: it is reported and left out, and its login is still made.
//!
//! Reasons stay with staff: they go into the audit lines only, never into
//! `decision_note` or `removal_note`, and the old owner of a TIN that moves
//! never sees who has it now.

use std::fmt::Write as _;
use std::process::ExitCode;

use serde::Deserialize;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::account::accounts::{self, Account};
use crate::account::admin::{self, Line, Source};
use crate::account::claims::ClaimKind;
use crate::account::roles::{self, Role};
use crate::account::tin::Tin;
use crate::base::crypto;
use crate::base::storage::DocumentStore;

/// The seed file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeedFile {
    #[serde(default)]
    login: Vec<LoginEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginEntry {
    email: String,
    /// Left out: the role stays as it is.
    role: Option<RoleEntry>,
    /// Left out: the label stays as it is.
    test: Option<bool>,
    #[serde(default)]
    tin: Vec<TinEntry>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RoleEntry {
    Admin,
    Support,
    None,
}

impl RoleEntry {
    fn role(self) -> Option<Role> {
        match self {
            Self::Admin => Some(Role::Admin),
            Self::Support => Some(Role::Support),
            Self::None => None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TinEntry {
    tin: String,
    branch: Option<String>,
    kind: KindEntry,
    name: String,
    state: StateEntry,
    reason: Option<String>,
    move_from: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum KindEntry {
    Personal,
    Business,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum StateEntry {
    Active,
    NotVerified,
}

/// A login as checked, ready to apply.
struct PlannedLogin {
    email: String,
    /// `Some(None)` takes the role away.
    role: Option<Option<Role>>,
    test: Option<bool>,
    tins: Vec<PlannedTin>,
}

struct PlannedTin {
    tin: Tin,
    kind: ClaimKind,
    name: String,
    active: bool,
    reason: String,
    move_from: Option<String>,
}

/// How the tool was asked to run.
#[derive(Clone, Debug)]
pub struct Options {
    /// `--as`: the Admin running it, recorded as the actor of every change.
    pub actor: String,
    pub dry_run: bool,
    /// `ADMIN_EMAILS` of the server it runs for; `None` with
    /// `--no-admin-emails`, for a server that has no such list.
    pub admin_emails: Option<Vec<String>>,
    /// `--target host:port/name`: the database the operator means.
    pub target: Option<String>,
    /// `--expect <digest>`: the plan digest the dry run printed.
    pub expect: Option<String>,
    /// Who runs it, as far as the machine knows.
    pub operator: Operator,
}

/// Who runs the seed, as the machine knows them (recorded in `seed_runs`
/// beside `--as`, which anyone can type).
#[derive(Clone, Debug)]
pub struct Operator {
    /// The OS user, and the user behind `sudo` when there is one.
    pub os_user: String,
    pub host: String,
}

/// What a run did, or would do.
#[derive(Debug)]
pub struct Report {
    pub run_id: Uuid,
    pub file_sha256: String,
    /// Before the run.
    pub accounts: i64,
    pub claims: i64,
    /// One line per login and TIN: what changed, what was left, what was
    /// refused.
    pub lines: Vec<String>,
    pub changes: usize,
    pub dry_run: bool,
    /// The database it ran against, `host:port/name`.
    pub target: String,
    /// The digest of the plan, for `--expect`.
    pub digest: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SeedError {
    /// The file is refused; nothing was written.
    #[error("the seed file is refused:\n{}", .0.join("\n"))]
    Refused(Vec<String>),
    #[error(
        "this database is at migration {database}, this release at {release}: run the seed of the release the \
         database runs"
    )]
    Behind { database: i64, release: i64 },
    #[error(
        "another change (a role change, a ban, a claim being decided) held a lock for more than {LOCK_TIMEOUT}: run it again"
    )]
    Busy,
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for SeedError {
    fn from(error: sqlx::Error) -> Self {
        match &error {
            // lock_timeout: another change held a lock too long.
            sqlx::Error::Database(database) if database.code().as_deref() == Some("55P03") => Self::Busy,
            _ => Self::Internal(error.into()),
        }
    }
}

/// A TIN as a run shows it: in full in a dry run (`full`), so a typo shows
/// before anything is done; masked otherwise.
fn shown(tin: &Tin, full: bool) -> String {
    if full { tin.to_string() } else { tin.masked() }
}

/// What was typed for a TIN that is not one, as a run shows it: as typed
/// in a dry run, otherwise its last three digits only.
fn shown_raw(raw: &str, full: bool) -> String {
    let digits: Vec<char> = raw.chars().filter(char::is_ascii_digit).collect();
    match digits.len() {
        _ if full => raw.to_owned(),
        0..3 => "with fewer than 3 digits".to_owned(),
        count => format!("ending in {}", digits[count - 3..].iter().collect::<String>()),
    }
}

/// A TIN refused on its own: (as the run shows it, masked).
type RefusedTin = (String, String);

/// Reads and checks the file on its own, without the database: (logins,
/// TINs refused on their own). `full` shows TINs in full (a dry run).
fn plan(file: &[u8], full: bool) -> Result<(Vec<PlannedLogin>, Vec<RefusedTin>), SeedError> {
    let text = std::str::from_utf8(file).map_err(|_| SeedError::Refused(vec!["the file is not UTF-8".into()]))?;
    let parsed: SeedFile = toml::from_str(text).map_err(|error| SeedError::Refused(vec![error.to_string()]))?;
    let mut problems = Vec::new();
    let mut refused_tins = Vec::new();
    let mut emails: Vec<String> = Vec::new();
    let mut tins: Vec<Tin> = Vec::new();
    let mut logins = Vec::new();
    for login in parsed.login {
        let Ok(email) = accounts::normalize_email(&login.email) else {
            problems.push(format!("login {:?}: not a valid email", login.email));
            continue;
        };
        if emails.contains(&email) {
            problems.push(format!("login {email}: in the file twice"));
            continue;
        }
        emails.push(email.clone());
        let mut planned = Vec::new();
        for entry in login.tin {
            let tin = match Tin::parse_fields(&entry.tin, entry.branch.as_deref().unwrap_or("")) {
                // A `tin` that carries its branch and a `branch` key that
                // names another one: which is meant is not for us to guess.
                Ok(tin) if branch_conflict(&tin, entry.branch.as_deref()) => {
                    problems.push(format!(
                        "TIN {} for {email}: its branch {} differs from branch = {:?}; keep one of them",
                        tin.masked(),
                        tin.branch(),
                        entry.branch.as_deref().unwrap_or_default()
                    ));
                    continue;
                }
                Ok(tin) => tin,
                Err(notice) => {
                    let line =
                        |full| format!("refused TIN {} for {email}: {}", shown_raw(&entry.tin, full), notice.message());
                    refused_tins.push((line(full), line(false)));
                    continue;
                }
            };
            let shown = shown(&tin, full);
            if tins.contains(&tin) {
                problems.push(format!("TIN {shown}: in the file twice"));
                continue;
            }
            tins.push(tin.clone());
            let name = entry.name.trim().to_owned();
            if name.is_empty() || name.len() > 200 {
                problems.push(format!("TIN {shown} for {email}: the registered name is missing or too long"));
            }
            let active = entry.state == StateEntry::Active;
            let reason = entry.reason.as_deref().unwrap_or_default().trim().to_owned();
            if active && reason.is_empty() {
                problems.push(format!("TIN {shown} for {email}: an active TIN needs a reason"));
            }
            let move_from = match entry.move_from.as_deref().map(accounts::normalize_email) {
                None => None,
                Some(Err(_)) => {
                    problems.push(format!("TIN {shown} for {email}: move_from is not a valid email"));
                    None
                }
                Some(Ok(from)) if !active => {
                    problems.push(format!("TIN {shown} for {email}: move_from {from} is only for an active TIN"));
                    None
                }
                Some(Ok(from)) if from == email => {
                    problems.push(format!("TIN {shown} for {email}: move_from is the same login"));
                    None
                }
                Some(Ok(from)) => Some(from),
            };
            let kind = match entry.kind {
                KindEntry::Personal => ClaimKind::Personal,
                KindEntry::Business => ClaimKind::Business,
            };
            planned.push(PlannedTin { tin, kind, name, active, reason, move_from });
        }
        logins.push(PlannedLogin { email, role: login.role.map(RoleEntry::role), test: login.test, tins: planned });
    }
    if problems.is_empty() { Ok((logins, refused_tins)) } else { Err(SeedError::Refused(problems)) }
}

/// Whether a `branch` key names another branch than the one `tin` was read
/// with (from the `tin` itself, as printed on Form 2303).
fn branch_conflict(tin: &Tin, branch: Option<&str>) -> bool {
    let Some(branch) = branch.filter(|branch| !branch.trim().is_empty()) else {
        return false;
    };
    Tin::parse(tin.root(), branch).map_or(true, |keyed| keyed.branch() != tin.branch())
}

/// How long the seed waits for a lock another change holds (a role change,
/// a ban, a claim being decided) before it gives up, having written nothing.
/// It holds the roles lock meanwhile, so it must not wait for long.
pub const LOCK_TIMEOUT: &str = "5s";

/// Refuses a database whose applied migrations are not exactly this
/// release's: a missing one, one this release does not know, or one with
/// the same number but other contents (another build's file).
async fn check_migrations(db: &PgPool) -> Result<(), SeedError> {
    let applied: Vec<(i64, Vec<u8>, bool)> =
        sqlx::query_as("SELECT version, checksum, success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(db)
            .await?;
    let release: Vec<&sqlx::migrate::Migration> =
        crate::MIGRATOR.iter().filter(|migration| !migration.migration_type.is_down_migration()).collect();
    let database = applied.iter().filter(|(_, _, success)| *success).map(|(version, ..)| *version).max();
    let newest = release.iter().map(|migration| migration.version).max().unwrap_or_default();
    if database != Some(newest) {
        return Err(SeedError::Behind { database: database.unwrap_or_default(), release: newest });
    }
    for (version, checksum, success) in &applied {
        match release.iter().find(|migration| migration.version == *version) {
            Some(migration) if *success && migration.checksum.as_ref() == checksum.as_slice() => {}
            _ => {
                return Err(SeedError::Refused(vec![format!(
                    "this database's migration {version} is not this release's (another build's file, or one \
                     that failed): run the seed of the release the database runs"
                )]));
            }
        }
    }
    if let Some(missing) = release.iter().find(|migration| !applied.iter().any(|(v, ..)| *v == migration.version)) {
        return Err(SeedError::Behind { database: missing.version - 1, release: newest });
    }
    Ok(())
}

/// The database a pool is connected to, as `--target` names it:
/// `host:port/name`, the name as the server reports it.
async fn connected_target(db: &PgPool) -> sqlx::Result<String> {
    let name: String = sqlx::query_scalar("SELECT current_database()").fetch_one(db).await?;
    let options = db.connect_options();
    Ok(format!("{}:{}/{name}", options.get_host(), options.get_port()))
}

/// The digest of a plan: the database, who runs it as whom with which
/// `ADMIN_EMAILS`, the file and every line of the plan (TINs masked; the
/// file's hash tells two TINs with the same last digits apart).
fn plan_digest(
    target: &str,
    actor: &str,
    admin_emails: Option<&[String]>,
    file_sha256: &[u8],
    lines: &[String],
) -> String {
    let mut text = format!(
        "buwiz seed plan 1\ntarget {target}\nas {actor}\nadmin_emails {}\nfile {}\n",
        admin_emails.map_or_else(|| "-".to_owned(), |list| list.join(",")),
        crypto::hex(file_sha256)
    );
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    crypto::hex(&crypto::sha256(text.as_bytes()))
}

/// Runs the file against `db` as `options` say: [`prepare`], then
/// [`Pending::finish`].
pub async fn run(db: &PgPool, store: &DocumentStore, file: &[u8], options: &Options) -> Result<Report, SeedError> {
    prepare(db, store, file, options).await?.finish(store).await
}

/// A run made in its transaction and not yet committed: what it would
/// write is in [`Pending::report`].
pub struct Pending {
    tx: Transaction<'static, Postgres>,
    report: Report,
    actor: Account,
    file_sha256: Vec<u8>,
    summary: serde_json::Value,
    objects: Vec<String>,
    expect: Option<String>,
    operator: Operator,
}

/// Checks the file, the options and the database, then makes every change
/// in one transaction and holds it open: nothing is visible to anyone
/// until [`Pending::finish`]. A real run must name the database it is
/// connected to (`--target`), so it never writes to another by accident.
pub async fn prepare(db: &PgPool, store: &DocumentStore, file: &[u8], options: &Options) -> Result<Pending, SeedError> {
    let _ = store;
    let full = options.dry_run;
    let (logins, refused_tins) = plan(file, full)?;
    let actor_email = accounts::normalize_email(&options.actor)
        .map_err(|_| SeedError::Refused(vec![format!("--as {:?} is not a valid email", options.actor)]))?;
    if options.admin_emails.as_ref().is_some_and(Vec::is_empty) {
        return Err(SeedError::Refused(vec![
            "ADMIN_EMAILS is empty: give the server's list, or --no-admin-emails for a server without one".to_owned(),
        ]));
    }
    let target = connected_target(db).await?;
    match &options.target {
        Some(named) if *named != target => {
            return Err(SeedError::Refused(vec![format!(
                "this run is connected to {target}, not {named}; nothing was written"
            )]));
        }
        None if !options.dry_run => {
            return Err(SeedError::Refused(vec![format!(
                "a real run names its database: --target {target} (the dry run prints it); nothing was written"
            )]));
        }
        _ => {}
    }
    check_migrations(db).await?;
    let (accounts_before, claims_before): (i64, i64) =
        sqlx::query_as("SELECT (SELECT count(*) FROM accounts), (SELECT count(*) FROM tin_claims)")
            .fetch_one(db)
            .await?;

    let run_id = Uuid::new_v4();
    let mut tx = db.begin().await?;
    sqlx::query(&format!("SET LOCAL lock_timeout = '{LOCK_TIMEOUT}'")).execute(&mut *tx).await?;
    roles::lock(&mut tx).await?;
    let admin_emails = options.admin_emails.as_deref().unwrap_or_default();
    let mut seed = Seed {
        tx,
        run_id,
        admin_emails,
        server_list: options.admin_emails.is_some(),
        shown_full: full,
        lines: Vec::new(),
        changes: Vec::new(),
        problems: Vec::new(),
        objects: Vec::new(),
        bootstrapped: None,
    };
    let actor = seed.actor(&actor_email, &logins).await?;
    let mut ids = Vec::with_capacity(logins.len());
    for login in &logins {
        ids.push(seed.login(&actor, login).await?);
    }
    if roles::other_admins(&mut *seed.tx, admin_emails, None).await? == 0 {
        seed.problems.push("the roles in the file leave Buwiz with no Admin".to_owned());
    }
    for (login, account_id) in logins.iter().zip(&ids) {
        for tin in &login.tins {
            seed.tin(&actor, *account_id, &login.email, tin).await?;
        }
    }
    let Seed { tx, mut lines, changes, problems, objects, .. } = seed;
    if !problems.is_empty() {
        return Err(SeedError::Refused(problems));
    }
    lines.extend(refused_tins.iter().cloned());
    let masked: Vec<String> = lines.iter().map(|(_, masked)| masked.clone()).collect();
    let file_sha256 = crypto::sha256(file);
    let digest = plan_digest(&target, &actor.email, options.admin_emails.as_deref(), &file_sha256, &masked);
    let refused: Vec<&String> = refused_tins.iter().map(|(_, masked)| masked).collect();
    let summary = json!({ "changes": changes, "refused": refused });
    let report = Report {
        run_id,
        file_sha256: crypto::hex(&file_sha256),
        accounts: accounts_before,
        claims: claims_before,
        lines: lines.into_iter().map(|(shown, _)| shown).collect(),
        changes: changes.len(),
        dry_run: options.dry_run,
        target,
        digest,
    };
    Ok(Pending {
        tx,
        report,
        actor,
        file_sha256,
        summary,
        objects,
        expect: options.expect.clone(),
        operator: options.operator.clone(),
    })
}

impl Pending {
    /// What the run would write.
    pub fn report(&self) -> &Report {
        &self.report
    }

    /// A dry run rolls back. A real run checks the plan against `--expect`,
    /// records itself in `seed_runs` and commits, then deletes the stored
    /// files of the claims it deleted.
    pub async fn finish(self, store: &DocumentStore) -> Result<Report, SeedError> {
        let Self { mut tx, report, actor, file_sha256, summary, objects, expect, operator } = self;
        if let Some(expected) = expect.filter(|_| !report.dry_run)
            && expected != report.digest
        {
            tx.rollback().await?;
            return Err(SeedError::Refused(vec![format!(
                "the plan is not the one the dry run showed (--expect {expected}, this plan {}); nothing was \
                 written. Run the dry run again and read it.",
                report.digest
            )]));
        }
        sqlx::query(
            "INSERT INTO seed_runs (id, actor_id, actor_email, os_user, host_name, target, file_sha256, plan_digest,
                 summary)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(report.run_id)
        .bind(actor.id)
        .bind(&actor.email)
        .bind(&operator.os_user)
        .bind(&operator.host)
        .bind(&report.target)
        .bind(&file_sha256)
        .bind(&report.digest)
        .bind(&summary)
        .execute(&mut *tx)
        .await?;
        if report.dry_run {
            tx.rollback().await?;
        } else {
            tx.commit().await?;
            store.delete_all(&objects).await;
            tracing::info!(run_id = %report.run_id, changes = report.changes, "seed run");
        }
        Ok(report)
    }
}

/// A run under way, in its transaction.
struct Seed<'a> {
    tx: Transaction<'static, Postgres>,
    run_id: Uuid,
    admin_emails: &'a [String],
    /// The server's `ADMIN_EMAILS` was given (not `--no-admin-emails`).
    server_list: bool,
    /// A dry run shows full TINs, so a typo shows before anything is done.
    shown_full: bool,
    /// Every line of the plan: (as the run shows it, masked).
    lines: Vec<(String, String)>,
    /// The changes alone, for `seed_runs` (TINs masked).
    changes: Vec<String>,
    /// What refuses the file.
    problems: Vec<String>,
    /// Stored files to delete once committed.
    objects: Vec<String>,
    /// The account of `--as`, when this run made it.
    bootstrapped: Option<Uuid>,
}

/// A line of `claim_audit` from the seed tool.
struct ClaimLine<'a> {
    claim_id: Uuid,
    account_id: Uuid,
    action: &'static str,
    status_before: &'a str,
    reason: &'a str,
}

/// A live claim on a TIN, locked.
struct LiveClaim {
    id: Uuid,
    account_id: Uuid,
    email: String,
    status: String,
    paused: bool,
}

impl Seed<'_> {
    fn shown(&self, tin: &Tin) -> String {
        shown(tin, self.shown_full)
    }

    fn changed(&mut self, line: String, masked: String) {
        self.lines.push((line, masked.clone()));
        self.changes.push(masked);
    }

    /// A line that changes nothing: (as shown, masked).
    fn unchanged(&mut self, line: String, masked: String) {
        self.lines.push((line, masked));
    }

    /// `text` with the TIN as the run shows it, and masked.
    fn both(&self, tin: &Tin, text: impl Fn(&str) -> String) -> (String, String) {
        (text(&self.shown(tin)), text(&tin.masked()))
    }

    fn reason(&self, what: &str) -> String {
        format!("seed {}: {what}", self.run_id)
    }

    /// The Admin running it: an Admin on this database, or in the server's
    /// `ADMIN_EMAILS` (its account is made if it has none). On a database
    /// with no Admin at all, the file may make `--as` the first one.
    async fn actor(&mut self, email: &str, logins: &[PlannedLogin]) -> Result<Account, SeedError> {
        let row: Option<(Uuid, Option<String>, bool)> =
            sqlx::query_as("SELECT id, role, banned_at IS NOT NULL FROM accounts WHERE email = $1")
                .bind(email)
                .fetch_optional(&mut *self.tx)
                .await?;
        let in_setting = self.admin_emails.iter().any(|admin| admin == email);
        let banned = row.as_ref().is_some_and(|(_, _, banned)| *banned);
        let role = row.as_ref().and_then(|(_, role, _)| role.as_deref());
        let admin_now = roles::effective(in_setting, role, banned).is_some_and(|(role, _)| role == Role::Admin);
        // A server with ADMIN_EMAILS has its Admins: one of them runs it.
        let bootstrap = !admin_now
            && !banned
            && !self.server_list
            && roles::other_admins(&mut *self.tx, self.admin_emails, None).await? == 0
            && logins.iter().any(|login| login.email == email && login.role == Some(Some(Role::Admin)));
        if !admin_now && !bootstrap {
            let why = if banned {
                format!("--as {email} is banned")
            } else if self.server_list {
                format!(
                    "--as {email} is not an Admin on this database nor in ADMIN_EMAILS: run it as an Admin, one of \
                     the server's ADMIN_EMAILS for a start (the file makes the first Admin only on a server without \
                     that list)"
                )
            } else if roles::other_admins(&mut *self.tx, self.admin_emails, None).await? > 0 {
                format!("--as {email} is not an Admin on this database")
            } else {
                format!(
                    "--as {email} is not an Admin, and this database has none: give {email} role = \"admin\" in this file"
                )
            };
            return Err(SeedError::Refused(vec![why]));
        }
        let (id, made) = self.create_account(email).await?;
        if made {
            self.bootstrapped = Some(id);
        }
        Ok(Account { id, email: email.to_owned() })
    }

    /// The account for `email`, made if missing: (id, made now).
    async fn create_account(&mut self, email: &str) -> sqlx::Result<(Uuid, bool)> {
        let made: Option<Uuid> =
            sqlx::query_scalar("INSERT INTO accounts (email) VALUES ($1) ON CONFLICT (email) DO NOTHING RETURNING id")
                .bind(email)
                .fetch_optional(&mut *self.tx)
                .await?;
        match made {
            Some(id) => Ok((id, true)),
            None => {
                let id = sqlx::query_scalar("SELECT id FROM accounts WHERE email = $1")
                    .bind(email)
                    .fetch_one(&mut *self.tx)
                    .await?;
                Ok((id, false))
            }
        }
    }

    async fn account_line(&mut self, actor: &Account, line: Line<'_>) -> sqlx::Result<()> {
        admin::audit(&mut self.tx, actor, line).await
    }

    /// Makes the login and sets its role and test label where they differ.
    async fn login(&mut self, actor: &Account, login: &PlannedLogin) -> Result<Uuid, SeedError> {
        let email = login.email.as_str();
        let (id, made) = self.create_account(email).await?;
        let mut changed = false;
        if made || self.bootstrapped == Some(id) {
            let reason = self.reason("a login in the seed file");
            let line = Line {
                account_id: id,
                action: "created",
                old_value: None,
                new_value: None,
                source: Source::Seed,
                reason: Some(&reason),
            };
            self.account_line(actor, line).await?;
            self.changed(format!("created login {email}"), format!("created login {email}"));
            changed = true;
        }
        let (stored_role, stored_test): (Option<String>, bool) =
            sqlx::query_as("SELECT role, test_account FROM accounts WHERE id = $1 FOR UPDATE")
                .bind(id)
                .fetch_one(&mut *self.tx)
                .await?;
        if let Some(role) = login.role {
            let old = stored_role.as_deref().and_then(Role::parse);
            if old != role {
                let new = role.map(Role::as_str);
                sqlx::query("UPDATE accounts SET role = $2 WHERE id = $1")
                    .bind(id)
                    .bind(new)
                    .execute(&mut *self.tx)
                    .await?;
                let reason = self.reason("role in the seed file");
                let line = Line {
                    account_id: id,
                    action: "role_changed",
                    old_value: old.map(Role::as_str),
                    new_value: new,
                    source: Source::Seed,
                    reason: Some(&reason),
                };
                self.account_line(actor, line).await?;
                let mut text =
                    format!("role {email} {} → {}", roles::role_label(old.map(Role::as_str)), roles::role_label(new));
                if self.admin_emails.iter().any(|admin| admin == email) && role != Some(Role::Admin) {
                    text.push_str(" (still an Admin through ADMIN_EMAILS)");
                }
                self.changed(text.clone(), text);
                changed = true;
            }
        }
        if let Some(test) = login.test
            && test != stored_test
        {
            sqlx::query("UPDATE accounts SET test_account = $2 WHERE id = $1")
                .bind(id)
                .bind(test)
                .execute(&mut *self.tx)
                .await?;
            let reason = self.reason("test label in the seed file");
            let action = if test { "test_marked" } else { "test_unmarked" };
            let line = Line {
                account_id: id,
                action,
                old_value: None,
                new_value: None,
                source: Source::Seed,
                reason: Some(&reason),
            };
            self.account_line(actor, line).await?;
            let text = format!("test label {email} {}", if test { "off → on" } else { "on → off" });
            self.changed(text.clone(), text);
            changed = true;
        }
        if !changed {
            let line = format!("unchanged login {email}");
            self.unchanged(line.clone(), line);
        }
        Ok(id)
    }

    async fn claim_line(&mut self, actor: &Account, tin: &Tin, line: ClaimLine<'_>) -> sqlx::Result<()> {
        sqlx::query(
            "INSERT INTO claim_audit
                 (claim_id, account_id, tin_masked, action, claim_status, actor_id, actor_email, reason, source)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'seed')",
        )
        .bind(line.claim_id)
        .bind(line.account_id)
        .bind(tin.masked())
        .bind(line.action)
        .bind(line.status_before)
        .bind(actor.id)
        .bind(&actor.email)
        .bind(line.reason)
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    /// Puts one TIN of the file on its login.
    async fn tin(
        &mut self,
        actor: &Account,
        account_id: Uuid,
        email: &str,
        entry: &PlannedTin,
    ) -> Result<(), SeedError> {
        let tin = &entry.tin;
        let shown = self.shown(tin);
        let live: Vec<(Uuid, Uuid, String, String, bool)> = sqlx::query_as(&format!(
            "SELECT c.id, c.account_id, a.email, c.status, c.paused_at IS NOT NULL
             FROM tin_claims c JOIN accounts a ON a.id = c.account_id
             WHERE c.tin_root = $1 AND c.branch_code = $2 AND c.{}
             ORDER BY c.created_at
             FOR UPDATE OF c",
            crate::filing::gate::LIVE_SQL
        ))
        .bind(tin.root())
        .bind(tin.branch())
        .fetch_all(&mut *self.tx)
        .await?;
        let live: Vec<LiveClaim> = live
            .into_iter()
            .map(|(id, account_id, email, status, paused)| LiveClaim { id, account_id, email, status, paused })
            .collect();
        let mine = live.iter().find(|claim| claim.account_id == account_id);

        if !entry.active {
            match mine {
                Some(claim) => {
                    let state = match claim.status.as_str() {
                        "approved" => "active",
                        "pending" => "waiting for review",
                        "needs_fix" => "sent back",
                        _ => "not verified",
                    };
                    let (line, masked) =
                        self.both(tin, |tin| format!("unchanged TIN {tin} on {email} ({state}; never downgraded)"));
                    self.unchanged(line, masked);
                }
                None => {
                    sqlx::query(
                        "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, status)
                         VALUES ($1, $2, $3, $4, $5, 'unverified')",
                    )
                    .bind(account_id)
                    .bind(entry.kind.as_str())
                    .bind(tin.root())
                    .bind(tin.branch())
                    .bind(&entry.name)
                    .execute(&mut *self.tx)
                    .await?;
                    let masked = tin.masked();
                    self.changed(
                        format!("TIN {shown} not verified on {email} (new)"),
                        format!("TIN {masked} not verified on {email} (new)"),
                    );
                }
            }
            return Ok(());
        }

        if let Some(claim) = mine {
            match claim.status.as_str() {
                "approved" if claim.paused => {
                    let (line, masked) =
                        self.both(tin, |tin| format!("unchanged TIN {tin} active on {email} (paused: left paused)"));
                    self.unchanged(line, masked);
                    return Ok(());
                }
                "approved" => {
                    let (line, masked) = self.both(tin, |tin| format!("unchanged TIN {tin} active on {email}"));
                    self.unchanged(line, masked);
                    return Ok(());
                }
                "pending" | "needs_fix" => {
                    self.problems.push(format!(
                        "TIN {shown} on {email}: its papers are waiting for review; decide it on /staff"
                    ));
                    return Ok(());
                }
                _ => {}
            }
        }
        let banned: bool = sqlx::query_scalar("SELECT banned_at IS NOT NULL FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_one(&mut *self.tx)
            .await?;
        if banned {
            self.problems.push(format!("TIN {shown} on {email}: the login is banned; lift the ban first"));
            return Ok(());
        }
        let move_reason = |seed: &Self| seed.reason(&format!("moved to {email} by the seed file: {}", entry.reason));
        let owner = live.iter().find(|claim| claim.account_id != account_id && claim.status == "approved");
        let mut how = "new".to_owned();
        match (owner, &entry.move_from) {
            (Some(owner), Some(from)) if owner.email == *from => {
                // The old row stays for its returns. Its owner sees that
                // staff ended it, never who has it now.
                sqlx::query(
                    "UPDATE tin_claims SET status = 'revoked', removed_at = now(), removed_by = $2, removal_note = NULL,
                         paused_at = NULL, paused_by = NULL, pause_reason = NULL
                     WHERE id = $1",
                )
                .bind(owner.id)
                .bind(actor.id)
                .execute(&mut *self.tx)
                .await?;
                let reason = move_reason(self);
                let (owner_id, owner_account) = (owner.id, owner.account_id);
                let line = ClaimLine {
                    claim_id: owner_id,
                    account_id: owner_account,
                    action: "revoked",
                    status_before: "approved",
                    reason: &reason,
                };
                self.claim_line(actor, tin, line).await?;
                how = format!("moved from {from}, which had it active");
            }
            (Some(owner), _) => {
                self.problems.push(format!(
                    "TIN {shown} for {email}: it is active on {}; to move it, set move_from = \"{}\"",
                    owner.email, owner.email
                ));
                return Ok(());
            }
            (None, Some(from)) => {
                let waiting = live
                    .iter()
                    .find(|claim| claim.email == *from && matches!(claim.status.as_str(), "pending" | "needs_fix"));
                let Some(waiting) = waiting else {
                    self.problems.push(format!(
                        "TIN {shown} for {email}: move_from {from} has no active or waiting claim on it"
                    ));
                    return Ok(());
                };
                // A claim that never became active has no returns: it goes,
                // with its files, and only the audit line stays.
                let (waiting_id, waiting_account, waiting_status) =
                    (waiting.id, waiting.account_id, waiting.status.clone());
                let objects: Vec<String> = sqlx::query_scalar(
                    "SELECT object_key FROM claim_documents
                     WHERE claim_id = $1 AND purged_at IS NULL AND object_key IS NOT NULL",
                )
                .bind(waiting_id)
                .fetch_all(&mut *self.tx)
                .await?;
                sqlx::query("DELETE FROM tin_claims WHERE id = $1").bind(waiting_id).execute(&mut *self.tx).await?;
                self.objects.extend(objects);
                let reason = move_reason(self);
                let line = ClaimLine {
                    claim_id: waiting_id,
                    account_id: waiting_account,
                    action: "deleted",
                    status_before: &waiting_status,
                    reason: &reason,
                };
                self.claim_line(actor, tin, line).await?;
                how = format!("moved from {from}, whose claim waiting for review was deleted");
            }
            (None, None) => {}
        }
        let unverified = mine.filter(|claim| claim.status == "unverified").map(|claim| claim.id);
        let claim_id = match unverified {
            // The file's kind and name win over what was typed.
            Some(claim_id) => {
                sqlx::query(
                    "UPDATE tin_claims SET status = 'approved', kind = $2, registered_name = $3, decided_at = now(),
                         decided_by = $4, verified_by = 'seed'
                     WHERE id = $1",
                )
                .bind(claim_id)
                .bind(entry.kind.as_str())
                .bind(&entry.name)
                .bind(actor.id)
                .execute(&mut *self.tx)
                .await?;
                how = format!("{how}; was not verified");
                claim_id
            }
            None => {
                sqlx::query_scalar(
                    "INSERT INTO tin_claims
                         (account_id, kind, tin_root, branch_code, registered_name, status, decided_at, decided_by, verified_by)
                     VALUES ($1, $2, $3, $4, $5, 'approved', now(), $6, 'seed')
                     RETURNING id",
                )
                .bind(account_id)
                .bind(entry.kind.as_str())
                .bind(tin.root())
                .bind(tin.branch())
                .bind(&entry.name)
                .bind(actor.id)
                .fetch_one(&mut *self.tx)
                .await?
            }
        };
        let reason = self.reason(&entry.reason);
        let status_before = if unverified.is_some() { "unverified" } else { "approved" };
        let line = ClaimLine { claim_id, account_id, action: "activated", status_before, reason: &reason };
        self.claim_line(actor, tin, line).await?;
        let how = how.trim_start_matches("new; ").to_owned();
        let masked = tin.masked();
        self.changed(
            format!("TIN {shown} active on {email} ({how})"),
            format!("TIN {masked} active on {email} ({how})"),
        );
        Ok(())
    }
}

/// The database a URL points at, as `host:port/name`, without the password.
fn database_target(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(url) => format!(
            "{}:{}{}",
            url.host_str().unwrap_or("?"),
            url.port().map_or_else(|| "5432".to_owned(), |port| port.to_string()),
            url.path()
        ),
        Err(_) => "(a DATABASE_URL that does not parse)".to_owned(),
    }
}

const USAGE: &str = "usage: buwiz-cloud seed <FILE> --as <admin email> --dry-run [--no-admin-emails]
       buwiz-cloud seed <FILE> --as <admin email> --target <host:port/name> [--expect <digest>] [--no-admin-emails]

Run it with the server's own settings (DATABASE_URL, ADMIN_EMAILS, S3_*): see docs/deploy.md.
  --dry-run            make every change, show it (TINs in full), then roll back
  --target T           a real run: the database it must be connected to, as the dry run prints it
  --expect D           a real run: refuse unless the plan's digest is D, as the dry run printed it
  --no-admin-emails    the server has no ADMIN_EMAILS (without this, ADMIN_EMAILS must be set)";

/// The command line, parsed.
struct Args {
    file: String,
    actor: String,
    dry_run: bool,
    target: Option<String>,
    expect: Option<String>,
    no_admin_emails: bool,
}

fn parse_args(args: &[String]) -> Option<Args> {
    let (mut file, mut actor, mut dry_run, mut target, mut expect, mut no_admin_emails) =
        (None, None, false, None, None, false);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--no-admin-emails" => no_admin_emails = true,
            "--as" => actor = Some(args.next()?.clone()),
            "--target" => target = Some(args.next()?.clone()),
            "--expect" => expect = Some(args.next()?.clone()),
            other if file.is_none() && !other.starts_with('-') => file = Some(other.to_owned()),
            _ => return None,
        }
    }
    if dry_run && expect.is_some() {
        return None;
    }
    Some(Args { file: file?, actor: actor?, dry_run, target, expect, no_admin_emails })
}

/// The command line: see [`USAGE`]. Reads `DATABASE_URL` and
/// `ADMIN_EMAILS`, and the `S3_*` settings to delete the files of a claim
/// waiting for review that a move deletes. Exit codes: 0 done (or a dry
/// run), 2 refused (nothing written), 1 an error (nothing written).
pub async fn cli(args: &[String]) -> ExitCode {
    let Some(args) = parse_args(args) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match cli_run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(SeedError::Refused(problems)) => {
            eprintln!("Refused; nothing was written:");
            for problem in problems {
                eprintln!("  {problem}");
            }
            ExitCode::from(2)
        }
        Err(error) => {
            eprintln!("Error; nothing was written: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// The server's `ADMIN_EMAILS` as the command was given it: never guessed
/// from a shell that lacks it.
fn admin_emails_given(no_admin_emails: bool) -> Result<Option<Vec<String>>, SeedError> {
    let set = std::env::var("ADMIN_EMAILS").is_ok_and(|raw| !raw.trim().is_empty());
    match (set, no_admin_emails) {
        (true, false) => Ok(Some(crate::base::config::admin_emails())),
        (false, true) => Ok(None),
        (false, false) => Err(SeedError::Refused(vec![
            "ADMIN_EMAILS is not set. Run the seed with the server's own settings (docs/deploy.md), or pass \
             --no-admin-emails if the server has no ADMIN_EMAILS"
                .to_owned(),
        ])),
        (true, true) => Err(SeedError::Refused(vec![
            "ADMIN_EMAILS is set, and --no-admin-emails says the server has none: drop one of them".to_owned(),
        ])),
    }
}

/// Who runs it, as far as this machine knows: the OS user (and the one
/// behind `sudo`) and the host name.
fn operator() -> Operator {
    let output = |program: &str, args: &[&str]| {
        std::process::Command::new(program)
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .filter(|text| !text.is_empty())
    };
    let user = output("id", &["-un"]).or_else(|| std::env::var("USER").ok()).unwrap_or_else(|| "unknown".to_owned());
    let os_user = match std::env::var("SUDO_USER").ok().filter(|sudo| !sudo.is_empty() && *sudo != user) {
        Some(sudo) => format!("{user} (sudo from {sudo})"),
        None => user,
    };
    Operator { os_user, host: output("hostname", &[]).unwrap_or_else(|| "unknown".to_owned()) }
}

async fn cli_run(args: Args) -> Result<(), SeedError> {
    let path = &args.file;
    let bytes =
        std::fs::read(path).map_err(|error| SeedError::Refused(vec![format!("cannot read {path}: {error}")]))?;
    let admin_emails = admin_emails_given(args.no_admin_emails)?;
    let url = std::env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL is not set"))?;
    let store = crate::base::config::document_store(false)?;
    let db = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .map_err(|error| anyhow::anyhow!("cannot connect to {}: {error}", database_target(&url)))?;
    let options = Options {
        actor: args.actor,
        dry_run: args.dry_run,
        admin_emails,
        target: args.target,
        expect: args.expect,
        operator: operator(),
    };
    let pending = prepare(&db, &store, &bytes, &options).await?;
    let report = pending.report();
    // Everything it is about to write, before it writes.
    let mut out = String::new();
    let _ = writeln!(out, "Database: {}", report.target);
    let _ = writeln!(out, "Before: {} accounts, {} TIN claims", report.accounts, report.claims);
    let _ = match &options.admin_emails {
        Some(list) => writeln!(out, "ADMIN_EMAILS: {}", list.join(", ")),
        None => writeln!(out, "ADMIN_EMAILS: none (--no-admin-emails)"),
    };
    let _ = writeln!(out, "File sha256: {}", report.file_sha256);
    if report.dry_run {
        let _ = writeln!(out, "Dry run: nothing is written. TINs are shown in full; check every digit.");
    } else {
        let _ = writeln!(out, "Plan digest: {}", report.digest);
        let _ = writeln!(out, "Seed run {}, writing:", report.run_id);
    }
    for line in &report.lines {
        let _ = writeln!(out, "  {line}");
    }
    print!("{out}");
    let _ = std::io::Write::flush(&mut std::io::stdout());

    let report = pending.finish(&store).await?;
    let mut out = String::new();
    match (report.changes, report.dry_run) {
        (0, _) => {
            let _ = writeln!(out, "Nothing to change.");
        }
        (count, true) => {
            let _ = writeln!(out, "{count} change(s) would be made.");
        }
        (count, false) => {
            let _ = writeln!(out, "{count} change(s) made, recorded as seed run {}.", report.run_id);
        }
    }
    if report.dry_run {
        let _ = writeln!(out, "Plan digest: {}", report.digest);
        let _ = writeln!(out, "To make these changes, run it again without --dry-run, adding:");
        let _ = writeln!(out, "  --target {} --expect {}", report.target, report.digest);
    }
    print!("{out}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_target_database_is_named_without_its_password() {
        assert_eq!(database_target("postgres://u:secret@127.0.0.1:54330/buwiz_cloud"), "127.0.0.1:54330/buwiz_cloud");
        assert_eq!(database_target("postgres://u:secret@db.example.test/buwiz"), "db.example.test:5432/buwiz");
    }

    #[test]
    fn a_bad_tin_is_refused_alone_and_its_login_kept() {
        let file = br#"
            [[login]]
            email = "Juan@Example.test"
              [[login.tin]]
              tin = "000-000-000"
              kind = "personal"
              name = "JUAN EXAMPLE"
              state = "active"
              reason = "Owner's test login"
        "#;
        let (logins, refused) = plan(file, true).unwrap();
        assert_eq!(logins.len(), 1);
        assert_eq!(logins[0].email, "juan@example.test");
        assert!(logins[0].tins.is_empty());
        assert_eq!(
            refused,
            [(
                "refused TIN 000-000-000 for juan@example.test: That TIN is not valid.".to_owned(),
                "refused TIN ending in 000 for juan@example.test: That TIN is not valid.".to_owned()
            )]
        );
    }

    #[test]
    fn the_whole_file_is_refused_for_one_bad_entry() {
        let refused = |file: &str| match plan(file.as_bytes(), true) {
            Err(SeedError::Refused(problems)) => problems.join("\n"),
            other => panic!("not refused: {:?}", other.map(|(logins, _)| logins.len())),
        };
        assert!(refused("[[login]]\nemail = \"a@example.test\"\ncolour = \"red\"\n").contains("unknown field"));
        assert!(refused("[[login]]\nemail = \"a@example.test\"\nrole = \"boss\"\n").contains("unknown variant"));
        assert!(refused("[[login]]\nemail = \"not an email\"\n").contains("not a valid email"));
        assert!(
            refused("[[login]]\nemail = \"a@example.test\"\n[[login]]\nemail = \"A@example.test\"\n").contains("twice")
        );
        let tin = |extra: &str| {
            format!(
                "[[login]]\nemail = \"a@example.test\"\n[[login.tin]]\ntin = \"000-123-453\"\nkind = \"personal\"\nname = \"A\"\n{extra}"
            )
        };
        assert!(refused(&tin("state = \"active\"\n")).contains("needs a reason"));
        assert!(
            refused(&tin("state = \"not_verified\"\nmove_from = \"b@example.test\"\n"))
                .contains("only for an active TIN")
        );
        assert!(
            refused(&tin("state = \"active\"\nreason = \"x\"\nmove_from = \"a@example.test\"\n"))
                .contains("same login")
        );
        let twice = format!(
            "{}[[login]]\nemail = \"b@example.test\"\n[[login.tin]]\ntin = \"000123453\"\nbranch = \"000\"\nkind = \"business\"\nname = \"B\"\nstate = \"not_verified\"\n",
            tin("state = \"not_verified\"\n")
        );
        assert!(refused(&twice).contains("TIN 000-123-453-00000: in the file twice"));
    }

    #[test]
    fn a_tin_whose_branch_differs_from_its_branch_key_is_refused() {
        let file = |tin: &str, branch: &str| {
            format!(
                "[[login]]\nemail = \"a@example.test\"\n[[login.tin]]\ntin = \"{tin}\"\nbranch = \"{branch}\"\nkind = \"business\"\nname = \"A\"\nstate = \"not_verified\"\n"
            )
        };
        match plan(file("000-123-453-00002", "00000").as_bytes(), true) {
            Err(SeedError::Refused(problems)) => {
                assert!(problems.join("\n").contains("branch"), "{problems:?}");
            }
            other => panic!("not refused: {:?}", other.map(|(logins, _)| logins.len())),
        }
        // The same branch twice, or in one place only, is fine.
        for (tin, branch) in [("000-123-453-00002", "00002"), ("000-123-453-002", "00002"), ("000-123-453", "00002")] {
            let (logins, _) = plan(file(tin, branch).as_bytes(), true).unwrap();
            assert_eq!(logins[0].tins[0].tin.branch(), "00002", "{tin} {branch}");
        }
    }
}
