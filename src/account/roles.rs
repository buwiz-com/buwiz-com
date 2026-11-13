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
    /// (never one's own).
    ConfirmAuto,
    DeleteClaim,
    EndOwnership,
    ActivateWithoutPapers,
    Pause,
    Ban,
    MarkTest,
    ChangeRole,
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

/// Accounts that are effective Admins, other than `except`: not banned,
/// and with the role `admin` or in `admin_emails`. Only accounts that exist
/// count, so an address in `ADMIN_EMAILS` nobody signed in with yet is no
/// admin.
pub async fn other_admins(
    db: impl PgExecutor<'_>,
    admin_emails: &[String],
    except: Option<uuid::Uuid>,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM accounts
         WHERE banned_at IS NULL AND (role = 'admin' OR email = ANY($1)) AND ($2::uuid IS NULL OR id <> $2)",
    )
    .bind(admin_emails)
    .bind(except)
    .fetch_one(db)
    .await
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
