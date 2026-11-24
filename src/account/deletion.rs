//! People delete their own account, on `/delete` (docs/accounts.md,
//! "Deleting an account"). They ask for a code, which is mailed to the
//! account's address right then ([`accounts::issue_deletion_code`]), and
//! typing it deletes the account in one transaction. Only a code asked
//! for there works: sign-in codes are of another purpose.
//!
//! Deleting the `accounts` row takes every row that names the account with
//! it (`ON DELETE CASCADE`: TINs and their files and readings, returns and
//! tickets, devices, sign-ins and their tokens, the BIR queue and outage
//! records). This also deletes its codes, which have no foreign key, and
//! the stored objects of files that were still kept. Audit lines stay; the
//! account's own actions in them lose its address.

use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::account::accounts::{self, CodeError, Purpose};
use crate::account::admin::{self, Line, Source};
use crate::account::notice::Notice;
use crate::account::roles;
use crate::base::storage::DocumentStore;

/// The SQL for "rows the deletion cannot take along name `$1` as who did
/// something": the staff columns of other accounts' TINs and accounts
/// (foreign keys without `ON DELETE`, where NULL has a meaning of its own,
/// e.g. `paused_by` NULL is a pause by Buwiz), audit lines about other
/// accounts, the BIR sign's notes, events and host keys, and seed runs.
/// Its own rows go with it and are not counted.
const NAMED_ELSEWHERE: &str = "
    SELECT EXISTS (SELECT 1 FROM tin_claims WHERE account_id <> $1
                     AND $1 IN (decided_by, sent_back_by, removed_by, paused_by, auto_confirmed_by))
        OR EXISTS (SELECT 1 FROM accounts WHERE id <> $1 AND $1 IN (banned_by, fixed_code_set_by))
        OR EXISTS (SELECT 1 FROM claim_audit WHERE actor_id = $1 AND account_id <> $1)
        OR EXISTS (SELECT 1 FROM account_audit WHERE actor_id = $1 AND account_id <> $1)
        OR EXISTS (SELECT 1 FROM bir_events WHERE actor_id = $1)
        OR EXISTS (SELECT 1 FROM bir_notes WHERE $1 IN (set_by, cleared_by))
        OR EXISTS (SELECT 1 FROM bir_host_keys WHERE confirmed_by = $1)
        OR EXISTS (SELECT 1 FROM seed_runs WHERE actor_id = $1)
        -- Never so (a return or ticket is for its own account's TIN), but
        -- the deletion would fail on it.
        OR EXISTS (SELECT 1 FROM submissions s JOIN tin_claims c ON c.id = s.claim_id
                    WHERE c.account_id = $1 AND s.account_id <> $1)
        OR EXISTS (SELECT 1 FROM tickets t JOIN tin_claims c ON c.id = t.claim_id
                    WHERE c.account_id = $1 AND t.account_id <> $1)";

/// Why the account of `account_id` (with `email`) cannot be deleted now,
/// or `None`. Staff first: ask an Admin. Then a ban, and records of staff
/// work elsewhere that name it: write to support. Then a send under way:
/// wait. A banned account is not deleted here because the ban lives on its
/// row: deleting it would free the address to sign up afresh, unbanned.
pub async fn refusal(
    db: &mut PgConnection,
    admin_emails: &[String],
    account_id: Uuid,
    email: &str,
) -> sqlx::Result<Option<Notice>> {
    let (role, banned): (Option<String>, bool) =
        sqlx::query_as("SELECT role, banned_at IS NOT NULL FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_one(&mut *db)
            .await?;
    if role.is_some() {
        return Ok(Some(Notice::DeleteStaff));
    }
    if roles::in_list(admin_emails, email) {
        return Ok(Some(Notice::DeleteAdminList));
    }
    if banned {
        return Ok(Some(Notice::DeleteBanned));
    }
    let named: bool = sqlx::query_scalar(NAMED_ELSEWHERE).bind(account_id).fetch_one(&mut *db).await?;
    if named {
        return Ok(Some(Notice::DeleteReferenced));
    }
    let sending: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM submissions WHERE account_id = $1 AND status = 'sending')")
            .bind(account_id)
            .fetch_one(&mut *db)
            .await?;
    Ok(sending.then_some(Notice::DeleteSending))
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteError {
    #[error("refused: {0:?}")]
    Refused(Notice),
    #[error(transparent)]
    Code(#[from] CodeError),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for DeleteError {
    fn from(error: sqlx::Error) -> Self {
        Self::Internal(error.into())
    }
}

/// Mails a fresh code for deleting the account, unless it is refused
/// ([`refusal`]). The code is random even for a test account with a fixed
/// sign-in code: whoever deletes the account must read its mailbox.
pub async fn send_code(
    db: &PgPool,
    mailer: &crate::account::mail::Mailer,
    admin_emails: &[String],
    account_id: Uuid,
    email: &str,
) -> Result<(), DeleteError> {
    if let Some(notice) = refusal(&mut *db.acquire().await?, admin_emails, account_id, email).await? {
        return Err(DeleteError::Refused(notice));
    }
    let code = accounts::issue_deletion_code(db, email).await?;
    mailer.send_deletion_code(email, &code).await?;
    Ok(())
}

/// Deletes the account of `account_id` if `code` is its newest code for
/// deleting it, in one transaction, then the stored objects of its files.
///
/// The account row is locked first, so a new return, ticket or sign-in
/// waits and then fails on the missing account, and so are its returns, so
/// none starts sending while this runs; the refusals are then checked
/// again. A wrong code is counted (and committed) as on the sign-in page.
pub async fn delete(
    db: &PgPool,
    store: &DocumentStore,
    admin_emails: &[String],
    account_id: Uuid,
    code: &str,
) -> Result<(), DeleteError> {
    let mut tx = db.begin().await?;
    let email: Option<String> = sqlx::query_scalar("SELECT email FROM accounts WHERE id = $1 FOR UPDATE")
        .bind(account_id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(email) = email else {
        return Err(DeleteError::Refused(Notice::AccountMissing));
    };
    sqlx::query("SELECT 1 FROM submissions WHERE account_id = $1 FOR UPDATE")
        .bind(account_id)
        .execute(&mut *tx)
        .await?;
    if let Some(notice) = refusal(&mut tx, admin_emails, account_id, &email).await? {
        return Err(DeleteError::Refused(notice));
    }
    if !accounts::take_code(&mut tx, &email, Purpose::Delete, code).await? {
        tx.commit().await?;
        return Err(DeleteError::Code(CodeError::Invalid));
    }
    // Files whose bytes are still in the store: claims still open (a
    // decided claim's files were purged already).
    let objects: Vec<String> = sqlx::query_scalar(
        "SELECT d.object_key FROM claim_documents d JOIN tin_claims c ON c.id = d.claim_id
         WHERE c.account_id = $1 AND d.purged_at IS NULL AND d.object_key IS NOT NULL",
    )
    .bind(account_id)
    .fetch_all(&mut *tx)
    .await?;
    // The audit lines stay; where the person was the actor on their own
    // account (a claim they deleted; Buwiz's own lines also name the
    // owner), the line keeps the id and loses the address.
    for table in ["claim_audit", "account_audit"] {
        sqlx::query(&format!("UPDATE {table} SET actor_email = '' WHERE account_id = $1 AND actor_id = $1"))
            .bind(account_id)
            .execute(&mut *tx)
            .await?;
    }
    // Recent signatures of its devices (kept minutes, by key).
    sqlx::query(
        "DELETE FROM request_nonces WHERE public_key IN (SELECT public_key FROM devices WHERE account_id = $1)",
    )
    .bind(account_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM email_codes WHERE email = $1").bind(&email).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM accounts WHERE id = $1").bind(account_id).execute(&mut *tx).await?;
    let me = accounts::Account { id: account_id, email: String::new() };
    let line =
        Line { account_id, action: "deleted", old_value: None, new_value: None, source: Source::Web, reason: None };
    admin::audit(&mut tx, &me, line).await?;
    tx.commit().await?;
    // As when a claim is deleted: a failure is logged, and the bytes are
    // ciphertext whose key stays with us.
    store.delete_all(&objects).await;
    tracing::info!(account = %account_id, files = objects.len(), "account deleted by its owner");
    Ok(())
}
