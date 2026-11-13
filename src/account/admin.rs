//! What staff do to accounts: make a login, change a role, ban and lift a
//! ban, label a test account, and sign an account out everywhere or out of
//! one app. Each runs in one transaction (lock, check,
//! change, a line in `account_audit` with who, when and why). The routes
//! check the power first ([`crate::account::roles`]); the checks here are
//! the ones that depend on the account acted on.

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::account::accounts::{self, Account};
use crate::account::claims::DecideError;
use crate::account::notice::Notice;
use crate::account::roles::{self, Power, Role, Staff};
use crate::base::config::Config;

/// An account locked for a change.
struct Target {
    id: Uuid,
    email: String,
    role: Option<String>,
    banned: bool,
}

impl Target {
    fn role(&self, config: &Config) -> Option<Role> {
        roles::of(config, &self.email, self.role.as_deref(), self.banned).map(|(role, _)| role)
    }
}

async fn lock_account(tx: &mut Transaction<'_, Postgres>, account_id: Uuid) -> Result<Target, DecideError> {
    let row: Option<(Uuid, String, Option<String>, bool)> =
        sqlx::query_as("SELECT id, email, role, banned_at IS NOT NULL FROM accounts WHERE id = $1 FOR UPDATE")
            .bind(account_id)
            .fetch_optional(&mut **tx)
            .await?;
    let (id, email, role, banned) = row.ok_or(DecideError::Refused(Notice::AccountMissing))?;
    Ok(Target { id, email, role, banned })
}

/// The longest reason or note staff may write, as the pages' `maxlength`.
pub const MAX_REASON_CHARS: usize = 500;

/// A staff reason, trimmed: required, and [`MAX_REASON_CHARS`] at most.
pub(crate) fn reason_text(reason: &str) -> Result<String, Notice> {
    let reason = reason.trim();
    if reason.is_empty() {
        Err(Notice::ReasonMissing)
    } else if reason.chars().count() > MAX_REASON_CHARS {
        Err(Notice::ReasonTooLong)
    } else {
        Ok(reason.to_owned())
    }
}

/// [`reason_text`], as a refusal.
pub(crate) fn required_reason(reason: &str) -> Result<String, DecideError> {
    reason_text(reason).map_err(DecideError::Refused)
}

/// Where an account change came from: the staff pages or the seed tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Web,
    Seed,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Seed => "seed",
        }
    }
}

/// One line of an account's history in `account_audit`.
pub struct Line<'a> {
    pub account_id: Uuid,
    pub action: &'static str,
    pub old_value: Option<&'a str>,
    pub new_value: Option<&'a str>,
    pub source: Source,
    pub reason: Option<&'a str>,
}

pub(crate) async fn audit(tx: &mut Transaction<'_, Postgres>, actor: &Account, line: Line<'_>) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO account_audit (account_id, action, old_value, new_value, actor_id, actor_email, source, reason)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(line.account_id)
    .bind(line.action)
    .bind(line.old_value)
    .bind(line.new_value)
    .bind(actor.id)
    .bind(&actor.email)
    .bind(line.source.as_str())
    .bind(line.reason)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// The account for `email`, made if there is none: (id, made now). The
/// staff pages and the seed tool make logins this way; each writes its own
/// `created` line.
pub(crate) async fn create_account(tx: &mut Transaction<'_, Postgres>, email: &str) -> sqlx::Result<(Uuid, bool)> {
    let made: Option<Uuid> =
        sqlx::query_scalar("INSERT INTO accounts (email) VALUES ($1) ON CONFLICT (email) DO NOTHING RETURNING id")
            .bind(email)
            .fetch_optional(&mut **tx)
            .await?;
    match made {
        Some(id) => Ok((id, true)),
        None => {
            let id =
                sqlx::query_scalar("SELECT id FROM accounts WHERE email = $1").bind(email).fetch_one(&mut **tx).await?;
            Ok((id, false))
        }
    }
}

/// A login an Admin makes on the staff pages.
pub struct NewLogin<'a> {
    pub email: &'a str,
    pub role: Option<Role>,
    pub test: bool,
    pub reason: &'a str,
}

/// An Admin makes a login: the account, with its role and test label, and
/// a line for each in `account_audit`, all with the one reason. Refused for
/// an address that already has an account. No mail goes out: the person
/// signs in with a code sent to the address when they ask for one. Locks
/// as a role change does: the roles lock, then the actor.
pub async fn create_login(
    db: &PgPool,
    config: &Config,
    actor: &Staff,
    login: &NewLogin<'_>,
) -> Result<(Uuid, Notice), DecideError> {
    let reason = required_reason(login.reason)?;
    let email = accounts::normalize_email(login.email).map_err(DecideError::Refused)?;
    let mut tx = db.begin().await?;
    roles::lock(&mut tx).await?;
    if !roles::may(&mut tx, config, actor.account.id, Power::CreateLogin).await? {
        return Err(DecideError::Refused(Notice::AdminOnly));
    }
    if login.role.is_some() && roles::in_server_setting(config, &email) {
        return Err(DecideError::Refused(Notice::RoleFromServer));
    }
    let (id, made) = create_account(&mut tx, &email).await?;
    if !made {
        return Err(DecideError::Refused(Notice::AccountExists));
    }
    let line = |action, new_value| Line {
        account_id: id,
        action,
        old_value: None,
        new_value,
        source: Source::Web,
        reason: Some(&reason),
    };
    audit(&mut tx, &actor.account, line("created", None)).await?;
    if let Some(role) = login.role {
        sqlx::query("UPDATE accounts SET role = $2 WHERE id = $1")
            .bind(id)
            .bind(role.as_str())
            .execute(&mut *tx)
            .await?;
        audit(&mut tx, &actor.account, line("role_changed", Some(role.as_str()))).await?;
    }
    if login.test {
        sqlx::query("UPDATE accounts SET test_account = true WHERE id = $1").bind(id).execute(&mut *tx).await?;
        audit(&mut tx, &actor.account, line("test_marked", None)).await?;
    }
    tx.commit().await?;
    tracing::info!(account_id = %id, actor = %actor.account.id, role = login.role.map(Role::as_str), "login created");
    Ok((id, Notice::LoginCreated))
}

/// An Admin gives an account a role, or takes it away (`None`). Refused for
/// one's own account, for a person who is an Admin through `ADMIN_EMAILS`,
/// and when it would leave Buwiz without an Admin.
pub async fn change_role(
    db: &PgPool,
    config: &Config,
    actor: &Staff,
    account_id: Uuid,
    role: Option<Role>,
    reason: &str,
) -> Result<Notice, DecideError> {
    let reason = required_reason(reason)?;
    if account_id == actor.account.id {
        return Err(DecideError::Refused(Notice::OwnAccount));
    }
    let mut tx = db.begin().await?;
    roles::lock(&mut tx).await?;
    let target = lock_account(&mut tx, account_id).await?;
    if roles::in_server_setting(config, &target.email) {
        return Err(DecideError::Refused(Notice::RoleFromServer));
    }
    let old = target.role.as_deref().and_then(Role::parse);
    if old == role {
        return Ok(Notice::RoleUnchanged);
    }
    // Under the lock, so a demotion running at the same time has committed
    // and is counted.
    let demotes_an_admin = target.role(config) == Some(Role::Admin) && role != Some(Role::Admin);
    if demotes_an_admin && roles::other_admins(&mut *tx, &config.admin_emails, Some(target.id)).await? == 0 {
        return Err(DecideError::Refused(Notice::LastAdmin));
    }
    // The page checked the actor's own role before this; it may have been
    // taken away since.
    if !roles::may(&mut tx, config, actor.account.id, Power::ChangeRole).await? {
        return Err(DecideError::Refused(Notice::AdminOnly));
    }
    let new = role.map(Role::as_str);
    sqlx::query("UPDATE accounts SET role = $2 WHERE id = $1").bind(target.id).bind(new).execute(&mut *tx).await?;
    let line = Line {
        account_id: target.id,
        action: "role_changed",
        old_value: old.map(Role::as_str),
        new_value: new,
        source: Source::Web,
        reason: Some(&reason),
    };
    audit(&mut tx, &actor.account, line).await?;
    tx.commit().await?;
    tracing::info!(account_id = %target.id, actor = %actor.account.id, role = new, "role changed");
    Ok(Notice::RoleChanged)
}

/// Support may sign anyone out but an Admin; an Admin anyone.
fn may_sign_out(config: &Config, actor: Role, target: &Target) -> bool {
    actor == Role::Admin || target.role(config) != Some(Role::Admin)
}

/// The actor's role as it stands now, if it may still sign people out: the
/// page's check came before, and a demotion or ban may have come since.
/// Read before the account acted on is locked, as the other actions do.
async fn sign_out_role(
    tx: &mut Transaction<'_, Postgres>,
    config: &Config,
    actor: &Staff,
) -> Result<Role, DecideError> {
    roles::recheck(tx, config, actor.account.id)
        .await?
        .filter(|role| role.can(Power::SignOut))
        .ok_or(DecideError::Refused(Notice::AdminOnly))
}

/// Staff end every sign-in of an account ([`accounts::end_sign_ins`]), saying
/// why.
pub async fn sign_out(
    db: &PgPool,
    config: &Config,
    actor: &Staff,
    account_id: Uuid,
    reason: &str,
) -> Result<Notice, DecideError> {
    let reason = required_reason(reason)?;
    let mut tx = db.begin().await?;
    let role = sign_out_role(&mut tx, config, actor).await?;
    let target = lock_account(&mut tx, account_id).await?;
    if !may_sign_out(config, role, &target) {
        return Err(DecideError::Refused(Notice::AdminOnly));
    }
    accounts::end_sign_ins(&mut tx, target.id, true).await?;
    let line = Line {
        account_id: target.id,
        action: "signed_out",
        old_value: None,
        new_value: None,
        source: Source::Web,
        reason: Some(&reason),
    };
    audit(&mut tx, &actor.account, line).await?;
    tx.commit().await?;
    tracing::info!(account_id = %target.id, actor = %actor.account.id, "signed out everywhere");
    Ok(Notice::SignedOutEverywhere)
}

/// Staff end one app's or agent's sign-in on the account, saying why. A
/// line is written only when a sign-in was ended.
pub async fn sign_out_app(
    db: &PgPool,
    config: &Config,
    actor: &Staff,
    account_id: Uuid,
    grant_id: Uuid,
    reason: &str,
) -> Result<Notice, DecideError> {
    let reason = required_reason(reason)?;
    let mut tx = db.begin().await?;
    let role = sign_out_role(&mut tx, config, actor).await?;
    let target = lock_account(&mut tx, account_id).await?;
    if !may_sign_out(config, role, &target) {
        return Err(DecideError::Refused(Notice::AdminOnly));
    }
    let client: Option<String> = sqlx::query_scalar(
        "UPDATE oauth_grants SET revoked_at = now(), revoke_reason = 'signed_out_by_staff'
         WHERE id = $1 AND account_id = $2 AND revoked_at IS NULL
         RETURNING client_id",
    )
    .bind(grant_id)
    .bind(target.id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(client) = client else {
        return Ok(Notice::AppAlreadySignedOut);
    };
    let line = Line {
        account_id: target.id,
        action: "app_signed_out",
        old_value: None,
        new_value: Some(&client),
        source: Source::Web,
        reason: Some(&reason),
    };
    audit(&mut tx, &actor.account, line).await?;
    tx.commit().await?;
    Ok(Notice::AppSignedOut)
}

/// An Admin bans an account: it loses its website sessions and the
/// sign-ins not yet picked up by an app or agent, and cannot sign in. Its
/// app sign-ins stay, but every API call but signing out is refused
/// (`account_banned`). Refused for one's own account and for any Admin.
pub async fn ban(
    db: &PgPool,
    config: &Config,
    actor: &Staff,
    account_id: Uuid,
    reason: &str,
) -> Result<Notice, DecideError> {
    let reason = required_reason(reason)?;
    if account_id == actor.account.id {
        return Err(DecideError::Refused(Notice::OwnAccount));
    }
    let mut tx = db.begin().await?;
    roles::lock(&mut tx).await?;
    if !roles::may(&mut tx, config, actor.account.id, Power::Ban).await? {
        return Err(DecideError::Refused(Notice::AdminOnly));
    }
    let target = lock_account(&mut tx, account_id).await?;
    if roles::in_server_setting(config, &target.email) {
        return Err(DecideError::Refused(Notice::RoleFromServer));
    }
    if target.role(config) == Some(Role::Admin) {
        return Err(DecideError::Refused(Notice::BanAdmin));
    }
    if target.banned {
        return Ok(Notice::AccountBannedStaff);
    }
    sqlx::query("UPDATE accounts SET banned_at = now(), banned_by = $2, ban_reason = $3 WHERE id = $1")
        .bind(target.id)
        .bind(actor.account.id)
        .bind(&reason)
        .execute(&mut *tx)
        .await?;
    accounts::end_sign_ins(&mut tx, target.id, false).await?;
    let line = Line {
        account_id: target.id,
        action: "banned",
        old_value: None,
        new_value: None,
        source: Source::Web,
        reason: Some(&reason),
    };
    audit(&mut tx, &actor.account, line).await?;
    tx.commit().await?;
    tracing::info!(account_id = %target.id, actor = %actor.account.id, "account banned");
    Ok(Notice::AccountBannedStaff)
}

/// An Admin lifts a ban. The account's app sign-ins work again at once.
pub async fn unban(
    db: &PgPool,
    config: &Config,
    actor: &Staff,
    account_id: Uuid,
    reason: &str,
) -> Result<Notice, DecideError> {
    let reason = required_reason(reason)?;
    let mut tx = db.begin().await?;
    roles::lock(&mut tx).await?;
    if !roles::may(&mut tx, config, actor.account.id, Power::Ban).await? {
        return Err(DecideError::Refused(Notice::AdminOnly));
    }
    let target = lock_account(&mut tx, account_id).await?;
    if !target.banned {
        return Ok(Notice::BanLifted);
    }
    sqlx::query("UPDATE accounts SET banned_at = NULL, banned_by = NULL, ban_reason = NULL WHERE id = $1")
        .bind(target.id)
        .execute(&mut *tx)
        .await?;
    let line = Line {
        account_id: target.id,
        action: "unbanned",
        old_value: None,
        new_value: None,
        source: Source::Web,
        reason: Some(&reason),
    };
    audit(&mut tx, &actor.account, line).await?;
    tx.commit().await?;
    tracing::info!(account_id = %target.id, actor = %actor.account.id, "ban lifted");
    Ok(Notice::BanLifted)
}

/// Whether the account holds a test TIN that has not ended.
pub(crate) async fn holds_test_tins(tx: &mut Transaction<'_, Postgres>, account_id: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM tin_claims WHERE account_id = $1 AND tin_root = $2 AND {})",
        crate::filing::gate::LIVE_SQL
    ))
    .bind(account_id)
    .bind(crate::account::tin::TEST_ROOT)
    .fetch_one(&mut **tx)
    .await
}

/// An Admin labels an account as a test account, or takes the label off
/// (refused while it holds a test TIN: revoke those first). Only staff see
/// it; sends work as usual. Not one's own account. Locks as a ban does: the
/// roles lock, the actor, then the account.
pub async fn mark_test(
    db: &PgPool,
    config: &Config,
    actor: &Staff,
    account_id: Uuid,
    test: bool,
    reason: &str,
) -> Result<Notice, DecideError> {
    let reason = required_reason(reason)?;
    let notice = if test { Notice::TestMarked } else { Notice::TestUnmarked };
    if account_id == actor.account.id {
        return Err(DecideError::Refused(Notice::OwnAccount));
    }
    let mut tx = db.begin().await?;
    roles::lock(&mut tx).await?;
    if !roles::may(&mut tx, config, actor.account.id, Power::MarkTest).await? {
        return Err(DecideError::Refused(Notice::AdminOnly));
    }
    let target = lock_account(&mut tx, account_id).await?;
    let labelled: bool = sqlx::query_scalar("SELECT test_account FROM accounts WHERE id = $1")
        .bind(target.id)
        .fetch_one(&mut *tx)
        .await?;
    if labelled == test {
        return Ok(notice);
    }
    // A test TIN lives on a test account only: the label stays while one
    // is on it. Its own add takes the roles lock too, so none comes in
    // meanwhile.
    if !test && holds_test_tins(&mut tx, target.id).await? {
        return Err(DecideError::Refused(Notice::TestTinsHeld));
    }
    sqlx::query("UPDATE accounts SET test_account = $2 WHERE id = $1")
        .bind(target.id)
        .bind(test)
        .execute(&mut *tx)
        .await?;
    let line = Line {
        account_id: target.id,
        action: if test { "test_marked" } else { "test_unmarked" },
        old_value: None,
        new_value: None,
        source: Source::Web,
        reason: Some(&reason),
    };
    audit(&mut tx, &actor.account, line).await?;
    tx.commit().await?;
    Ok(notice)
}
