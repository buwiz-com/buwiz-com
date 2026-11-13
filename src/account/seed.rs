//! `buwiz-cloud seed <FILE> --as <admin email> [--dry-run]`: creates
//! logins, their staff roles and test labels, and their TINs from one TOML
//! file, in one transaction, with a record of what it did (`seed_runs`, and
//! a line in `account_audit` or `claim_audit` for each change, from source
//! `seed`). See `docs/seed.example.toml` for the file.
//!
//! It changes only what differs from the file, so a second run with the
//! same file changes nothing. It never runs migrations and starts no
//! server: it refuses a database that is not at this release's newest
//! migration.
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
    /// `ADMIN_EMAILS` of the server it runs for.
    pub admin_emails: Vec<String>,
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
}

#[derive(Debug, thiserror::Error)]
pub enum SeedError {
    /// The file is refused; nothing was written.
    #[error("the seed file is refused:\n{}", .0.join("\n"))]
    Refused(Vec<String>),
    #[error("this database is at migration {database}, this release at {release}: deploy this release first")]
    Behind { database: i64, release: i64 },
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for SeedError {
    fn from(error: sqlx::Error) -> Self {
        Self::Internal(error.into())
    }
}

/// Reads and checks the file on its own, without the database: (logins,
/// TINs refused on their own).
fn plan(file: &[u8]) -> Result<(Vec<PlannedLogin>, Vec<String>), SeedError> {
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
                Ok(tin) => tin,
                Err(notice) => {
                    refused_tins.push(format!("refused TIN {} for {email}: {}", entry.tin, notice.message()));
                    continue;
                }
            };
            let shown = tin.to_string();
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

/// Runs the file against `db` as `options` say. A dry run does everything,
/// then rolls back.
pub async fn run(db: &PgPool, store: &DocumentStore, file: &[u8], options: &Options) -> Result<Report, SeedError> {
    let (logins, refused_tins) = plan(file)?;
    let actor_email = accounts::normalize_email(&options.actor)
        .map_err(|_| SeedError::Refused(vec![format!("--as {:?} is not a valid email", options.actor)]))?;
    let database: i64 = sqlx::query_scalar("SELECT coalesce(max(version), 0) FROM _sqlx_migrations WHERE success")
        .fetch_one(db)
        .await?;
    let release = crate::MIGRATOR.iter().map(|migration| migration.version).max().unwrap_or_default();
    if database != release {
        return Err(SeedError::Behind { database, release });
    }
    let (accounts_before, claims_before): (i64, i64) =
        sqlx::query_as("SELECT (SELECT count(*) FROM accounts), (SELECT count(*) FROM tin_claims)")
            .fetch_one(db)
            .await?;

    let run_id = Uuid::new_v4();
    let mut tx = db.begin().await?;
    roles::lock(&mut tx).await?;
    let mut seed = Seed {
        tx,
        run_id,
        admin_emails: &options.admin_emails,
        shown_full: options.dry_run,
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
    if roles::other_admins(&mut *seed.tx, &options.admin_emails, None).await? == 0 {
        seed.problems.push("the roles in the file leave Buwiz with no Admin".to_owned());
    }
    for (login, account_id) in logins.iter().zip(&ids) {
        for tin in &login.tins {
            seed.tin(&actor, *account_id, &login.email, tin).await?;
        }
    }
    let Seed { mut tx, lines, changes, problems, objects, .. } = seed;
    if !problems.is_empty() {
        return Err(SeedError::Refused(problems));
    }
    let mut lines = lines;
    lines.extend(refused_tins.iter().cloned());
    let file_sha256 = crypto::sha256(file);
    let summary = json!({ "changes": changes, "refused": refused_tins });
    sqlx::query("INSERT INTO seed_runs (id, actor_id, actor_email, file_sha256, summary) VALUES ($1, $2, $3, $4, $5)")
        .bind(run_id)
        .bind(actor.id)
        .bind(&actor.email)
        .bind(&file_sha256)
        .bind(&summary)
        .execute(&mut *tx)
        .await?;
    if options.dry_run {
        tx.rollback().await?;
    } else {
        tx.commit().await?;
        store.delete_all(&objects).await;
        tracing::info!(%run_id, actor = %actor.email, changes = changes.len(), "seed run");
    }
    Ok(Report {
        run_id,
        file_sha256: crypto::hex(&file_sha256),
        accounts: accounts_before,
        claims: claims_before,
        lines,
        changes: changes.len(),
        dry_run: options.dry_run,
    })
}

/// A run under way, in its transaction.
struct Seed<'a> {
    tx: Transaction<'static, Postgres>,
    run_id: Uuid,
    admin_emails: &'a [String],
    /// A dry run shows full TINs, so a typo shows before anything is done.
    shown_full: bool,
    lines: Vec<String>,
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
        if self.shown_full { tin.to_string() } else { tin.masked() }
    }

    fn changed(&mut self, line: String, masked: String) {
        self.lines.push(line);
        self.changes.push(masked);
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
        let bootstrap = !admin_now
            && !banned
            && roles::other_admins(&mut *self.tx, self.admin_emails, None).await? == 0
            && logins.iter().any(|login| login.email == email && login.role == Some(Some(Role::Admin)));
        if !admin_now && !bootstrap {
            let why = if banned {
                format!("--as {email} is banned")
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
            self.lines.push(format!("unchanged login {email}"));
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
                    self.lines.push(format!("unchanged TIN {shown} on {email} ({state}; never downgraded)"));
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
                    self.lines.push(format!("unchanged TIN {shown} active on {email} (paused: left paused)"));
                    return Ok(());
                }
                "approved" => {
                    self.lines.push(format!("unchanged TIN {shown} active on {email}"));
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

const USAGE: &str = "usage: buwiz-cloud seed <FILE> --as <admin email> [--dry-run]";

/// The command line: `seed <FILE> --as <email> [--dry-run]`. Reads
/// `DATABASE_URL` and `ADMIN_EMAILS`, and the `S3_*` settings only to
/// delete the files of a claim waiting for review that a move deletes.
/// Exit codes: 0 done (or a dry run), 2 refused, 1 an error.
pub async fn cli(args: &[String]) -> ExitCode {
    let (mut file, mut actor, mut dry_run) = (None, None, false);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--as" => actor = args.next().cloned(),
            other if file.is_none() && !other.starts_with('-') => file = Some(other.to_owned()),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let (Some(file), Some(actor)) = (file, actor) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match cli_run(&file, actor, dry_run).await {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
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

async fn cli_run(path: &str, actor: String, dry_run: bool) -> Result<String, SeedError> {
    let bytes =
        std::fs::read(path).map_err(|error| SeedError::Refused(vec![format!("cannot read {path}: {error}")]))?;
    let url = std::env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL is not set"))?;
    let admin_emails = crate::base::config::admin_emails();
    let store = crate::base::config::document_store(false)?;
    let db = PgPoolOptions::new().max_connections(2).connect(&url).await.map_err(anyhow::Error::from)?;
    let report = run(&db, &store, &bytes, &Options { actor, dry_run, admin_emails }).await?;
    let mut out = String::new();
    let _ = writeln!(out, "Database: {}", database_target(&url));
    let _ = writeln!(out, "Before: {} accounts, {} TIN claims", report.accounts, report.claims);
    let _ = writeln!(out, "File sha256: {}", report.file_sha256);
    if report.dry_run {
        let _ = writeln!(out, "Dry run: nothing is written. TINs are shown in full; check every digit.");
    } else {
        let _ = writeln!(out, "Seed run {}", report.run_id);
    }
    for line in &report.lines {
        let _ = writeln!(out, "  {line}");
    }
    match (report.changes, report.dry_run) {
        (0, _) => {
            let _ = writeln!(out, "Nothing to change.");
        }
        (count, true) => {
            let _ = writeln!(out, "{count} change(s) would be made. Run again without --dry-run to make them.");
        }
        (count, false) => {
            let _ = writeln!(out, "{count} change(s) made, recorded as seed run {}.", report.run_id);
        }
    }
    Ok(out)
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
        let (logins, refused) = plan(file).unwrap();
        assert_eq!(logins.len(), 1);
        assert_eq!(logins[0].email, "juan@example.test");
        assert!(logins[0].tins.is_empty());
        assert_eq!(refused, ["refused TIN 000-000-000 for juan@example.test: That TIN is not valid."]);
    }

    #[test]
    fn the_whole_file_is_refused_for_one_bad_entry() {
        let refused = |file: &str| match plan(file.as_bytes()) {
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
}
