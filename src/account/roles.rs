//! Staff roles and what each may do: the one place that decides whether a
//! person may use a staff power. Every staff route and page asks here.
//!
//! A role is kept on the account (`accounts.role`): Admin or Support. The
//! server setting `ADMIN_EMAILS` is a break-glass list: anyone in it is
//! always an Admin, whatever their account says, and the staff pages cannot
//! demote or ban them. The setting never writes a role into the database.
//! A banned account has no role at all.

use sqlx::{PgExecutor, PgPool, Postgres, Transaction};

use crate::account::accounts::Account;
use crate::base::config::Config;

/// A staff role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Reviews claims, confirms automatic activations, sees users, signs
    /// people out.
    Support,
    /// Everything.
    Admin,
}

impl Role {
    /// The value stored in `accounts.role`.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "admin" => Some(Self::Admin),
            "support" => Some(Self::Support),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Support => "support",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Admin => "Admin",
            Self::Support => "Support",
        }
    }

    pub fn can(self, power: Power) -> bool {
        match self {
            Self::Admin => true,
            Self::Support => matches!(power, Power::Review | Power::SeeUsers | Power::SignOut | Power::ConfirmAuto),
        }
    }
}

/// "Support", "Admin" or "No staff role", for a stored role.
pub fn role_label(role: Option<&str>) -> &'static str {
    role.and_then(Role::parse).map_or("No staff role", Role::label)
}

/// What staff can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    /// See the queue, claims and their files; approve, reject, send back.
    Review,
    /// See every account.
    SeeUsers,
    /// Sign an account out everywhere, or out of one app (never an
    /// Admin's, unless the one signing out is an Admin).
    SignOut,
    /// Confirm a TIN activated automatically, once its papers look right
    /// (one's own or a paused one only as an Admin).
    ConfirmAuto,
    DeleteClaim,
    EndOwnership,
    ActivateWithoutPapers,
    Pause,
    Ban,
    MarkTest,
    ChangeRole,
    /// Make a login on the staff pages.
    CreateLogin,
    /// Set or clear a hand-set BIR note (an outage, planned maintenance, or
    /// "declare Online"), and resume sending after the relay paused
    /// ([`crate::uptime::notes`]).
    SetBirStatus,
}

/// A signed-in staff member.
#[derive(Clone, Debug)]
pub struct Staff {
    pub account: Account,
    pub role: Role,
    /// An Admin through `ADMIN_EMAILS`.
    pub from_server: bool,
}

impl Staff {
    pub fn can(&self, power: Power) -> bool {
        self.role.can(power)
    }
}

/// The role an account has, from its email, its stored role and whether it
/// is banned: (role, through `ADMIN_EMAILS`).
pub fn of(config: &Config, email: &str, role: Option<&str>, banned: bool) -> Option<(Role, bool)> {
    effective(config.is_admin(email), role, banned)
}

/// Whether `email` is an Admin through `ADMIN_EMAILS`: such a person's role
/// cannot be changed, nor can they be banned, from the staff pages.
pub fn in_server_setting(config: &Config, email: &str) -> bool {
    config.is_admin(email)
}

/// Whether `email` is in the list `admin_emails` (`ADMIN_EMAILS`, as the
/// server or the seed tool was given it).
pub fn in_list(admin_emails: &[String], email: &str) -> bool {
    admin_emails.iter().any(|admin| admin == email)
}

/// The SQL for "this account (alias `account`) is staff": not banned, and
/// with a stored role or in the list bound as `list`.
pub fn is_staff_sql(account: &str, list: &str) -> String {
    format!("({account}.banned_at IS NULL AND ({account}.role IS NOT NULL OR {account}.email = ANY({list})))")
}

/// [`of`], given whether the email is in `ADMIN_EMAILS`.
pub fn effective(in_admin_emails: bool, role: Option<&str>, banned: bool) -> Option<(Role, bool)> {
    if banned {
        return None;
    }
    if in_admin_emails {
        return Some((Role::Admin, true));
    }
    role.and_then(Role::parse).map(|role| (role, false))
}

/// The staff member behind a signed-in account, or `None` for everyone else.
pub async fn staff(db: &PgPool, config: &Config, account: &Account) -> sqlx::Result<Option<Staff>> {
    let row: Option<(String, Option<String>, bool)> =
        sqlx::query_as("SELECT email, role, banned_at IS NOT NULL FROM accounts WHERE id = $1")
            .bind(account.id)
            .fetch_optional(db)
            .await?;
    Ok(row
        .and_then(|(email, role, banned)| of(config, &email, role.as_deref(), banned))
        .map(|(role, from_server)| Staff { account: account.clone(), role, from_server }))
}

/// The condition "this account (alias `account`) has signed in": it has a
/// website session, ended ones too (none is ever deleted). Every sign-in
/// starts one, and only a code mailed to the address does, so it proves
/// the address is theirs. A login staff or the seed tool made has none
/// until its person signs in.
pub fn signed_in_sql(account: &str) -> String {
    format!("EXISTS (SELECT 1 FROM web_sessions s WHERE s.account_id = {account}.id)")
}

/// Whether the account has signed in ([`signed_in_sql`]).
pub async fn has_signed_in(db: impl PgExecutor<'_>, account_id: uuid::Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar(&format!("SELECT {} FROM accounts a WHERE a.id = $1", signed_in_sql("a")))
        .bind(account_id)
        .fetch_one(db)
        .await
}

/// Accounts that are effective Admins, other than `except`, and that have
/// signed in: not banned, with the role `admin` or in `admin_emails`, and
/// with a sign-in on record ([`signed_in_sql`]). An Admin nobody signed in
/// as yet (a typed address, maybe mistyped) is no other Admin to fall
/// back on, and neither is an address in `ADMIN_EMAILS` with no account.
pub async fn other_admins(
    db: impl PgExecutor<'_>,
    admin_emails: &[String],
    except: Option<uuid::Uuid>,
) -> sqlx::Result<i64> {
    count_admins(db, admin_emails, except, true).await
}

/// [`other_admins`], counting Admins who never signed in too: the seed
/// tool's question "does this database have an Admin at all".
pub async fn admins_on_record(
    db: impl PgExecutor<'_>,
    admin_emails: &[String],
    except: Option<uuid::Uuid>,
) -> sqlx::Result<i64> {
    count_admins(db, admin_emails, except, false).await
}

async fn count_admins(
    db: impl PgExecutor<'_>,
    admin_emails: &[String],
    except: Option<uuid::Uuid>,
    signed_in: bool,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(&format!(
        "SELECT count(*) FROM accounts a
         WHERE a.banned_at IS NULL AND (a.role = 'admin' OR a.email = ANY($1)) AND ($2::uuid IS NULL OR a.id <> $2)
           AND (NOT $3 OR {})",
        signed_in_sql("a")
    ))
    .bind(admin_emails)
    .bind(except)
    .bind(signed_in)
    .fetch_one(db)
    .await
}

/// The role `account_id` has as the database stands now, read with a share
/// lock: a role change or ban of that account waits for the caller's
/// transaction to end, and one that committed first is seen. Every staff
/// action rechecks its actor this way inside its own transaction, so a
/// request that passed the page's check just before its actor was demoted
/// or banned does nothing.
pub async fn recheck(
    tx: &mut Transaction<'_, Postgres>,
    config: &Config,
    account_id: uuid::Uuid,
) -> sqlx::Result<Option<Role>> {
    let row: Option<(String, Option<String>, bool)> =
        sqlx::query_as("SELECT email, role, banned_at IS NOT NULL FROM accounts WHERE id = $1 FOR SHARE")
            .bind(account_id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(row.and_then(|(email, role, banned)| of(config, &email, role.as_deref(), banned)).map(|(role, _)| role))
}

/// Whether `account_id` may use `power` as the database stands now
/// ([`recheck`]).
pub async fn may(
    tx: &mut Transaction<'_, Postgres>,
    config: &Config,
    account_id: uuid::Uuid,
    power: Power,
) -> sqlx::Result<bool> {
    Ok(recheck(tx, config, account_id).await?.is_some_and(|role| role.can(power)))
}

/// Serialises every change to roles and bans (the staff pages and the seed
/// tool) for the rest of the transaction, so two Admins demoting each other
/// at once cannot both pass the "someone stays Admin" check.
pub async fn lock(tx: &mut Transaction<'_, Postgres>) -> sqlx::Result<()> {
    // "buwizrol" in ASCII.
    sqlx::query("SELECT pg_advisory_xact_lock(7095465005590572908)").execute(&mut **tx).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_reviews_and_signs_out_only() {
        use Power::*;
        let all = [
            Review,
            SeeUsers,
            SignOut,
            ConfirmAuto,
            DeleteClaim,
            EndOwnership,
            ActivateWithoutPapers,
            Pause,
            Ban,
            MarkTest,
            ChangeRole,
            CreateLogin,
            SetBirStatus,
        ];
        for power in all {
            assert!(Role::Admin.can(power), "{power:?}");
            assert_eq!(
                Role::Support.can(power),
                matches!(power, Review | SeeUsers | SignOut | ConfirmAuto),
                "{power:?}"
            );
        }
    }

    #[test]
    fn the_server_setting_wins_and_a_ban_takes_everything() {
        assert_eq!(effective(true, None, false), Some((Role::Admin, true)));
        assert_eq!(effective(true, Some("support"), false), Some((Role::Admin, true)));
        assert_eq!(effective(false, Some("support"), false), Some((Role::Support, false)));
        assert_eq!(effective(false, Some("admin"), false), Some((Role::Admin, false)));
        assert_eq!(effective(false, None, false), None);
        assert_eq!(effective(false, Some("admin"), true), None);
        assert_eq!(effective(true, None, true), None);
        assert_eq!(effective(false, Some("bogus"), false), None);
    }
}
