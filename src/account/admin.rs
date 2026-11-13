//! What staff do to accounts: change a role, and sign an account out
//! everywhere or out of one app. Each runs in one transaction (lock, check,
//! change, a line in `account_audit` with who, when and why). The routes
//! check the power first ([`crate::account::roles`]); the checks here are
//! the ones that depend on the account acted on.

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::account::accounts::{self, Account};
use crate::account::claims::DecideError;
use crate::account::notice::Notice;
use crate::account::roles::{self, Role, Staff};
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

/// The trimmed reason, or the notice asking for one.
pub(crate) fn required_reason(reason: &str) -> Result<String, DecideError> {
    let reason = reason.trim();
    if reason.is_empty() { Err(DecideError::Refused(Notice::ReasonMissing)) } else { Ok(reason.to_owned()) }
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
        return Ok(Notice::RoleChanged);
    }
    // Under the lock, so a demotion running at the same time has committed
    // and is counted.
    let demotes_an_admin = target.role(config) == Some(Role::Admin) && role != Some(Role::Admin);
    if demotes_an_admin && roles::other_admins(&mut *tx, &config.admin_emails, Some(target.id)).await? == 0 {
        return Err(DecideError::Refused(Notice::LastAdmin));
    }
    // The page checked the actor's own role before this; it may have been
    // taken away since.
    if !still_admin(&mut tx, config, &actor.account).await? {
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

/// Whether `account` is an Admin as the database stands now.
async fn still_admin(tx: &mut Transaction<'_, Postgres>, config: &Config, account: &Account) -> sqlx::Result<bool> {
    let row: Option<(String, Option<String>, bool)> =
        sqlx::query_as("SELECT email, role, banned_at IS NOT NULL FROM accounts WHERE id = $1")
            .bind(account.id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(row
        .and_then(|(email, role, banned)| roles::of(config, &email, role.as_deref(), banned))
        .is_some_and(|(role, _)| role == Role::Admin))
}

/// Support may sign anyone out but an Admin; an Admin anyone.
fn may_sign_out(config: &Config, actor: &Staff, target: &Target) -> bool {
    actor.role == Role::Admin || target.role(config) != Some(Role::Admin)
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
    let target = lock_account(&mut tx, account_id).await?;
    if !may_sign_out(config, actor, &target) {
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

/// Staff end one app's or agent's sign-in on the account. The reason is
/// optional.
pub async fn sign_out_app(
    db: &PgPool,
    config: &Config,
    actor: &Staff,
    account_id: Uuid,
    grant_id: Uuid,
    reason: &str,
) -> Result<Notice, DecideError> {
    let reason = Some(reason.trim()).filter(|reason| !reason.is_empty());
    let mut tx = db.begin().await?;
    let target = lock_account(&mut tx, account_id).await?;
    if !may_sign_out(config, actor, &target) {
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
    if let Some(client) = &client {
        let line = Line {
            account_id: target.id,
            action: "app_signed_out",
            old_value: None,
            new_value: Some(client),
            source: Source::Web,
            reason,
        };
        audit(&mut tx, &actor.account, line).await?;
    }
    tx.commit().await?;
    Ok(Notice::AppSignedOut)
}
