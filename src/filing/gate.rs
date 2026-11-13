//! The send gate: the one place that knows whether a TIN can send returns
//! for an account. Tickets, both submission calls, the relay and the TIN
//! list all ask here, so they never disagree.
//!
//! Only the caller's own live row counts (`unverified`, `pending`,
//! `needs_fix` or `approved`; there is at most one, `tin_claims_one_live`).
//! No row, a row that ended (rejected, withdrawn, removed, revoked) and a
//! TIN another account owns all read [`Standing::NotActive`], with today's
//! `tin_not_active`: a refusal never tells whether someone else has the
//! TIN.
//!
//! A test TIN (root `000-000-000`, [`Tin::is_test`]) never sends, in any
//! relay mode: an active one reads [`Standing::TestOnly`]
//! (`tin_test_only`). This server cannot know the Sender's
//! `BIR_RELAY_MODE` (the Sender reads it, and here it would default to
//! `dry_run` when unset), so it never trusts it to let one through; the
//! Sender refuses one too.
//!
//! A refusal's words are fixed sentences. The note staff write when they
//! send a claim back is never in one: refusals end up where an assistant can
//! read them. It travels only in the app's TIN list (`include=all`).

use axum::http::StatusCode;
use sqlx::PgExecutor;
use uuid::Uuid;

use crate::account::claims;
use crate::account::tin::Tin;
use crate::base::error::ApiError;

/// The condition on a claim `c` that may send, for queries that lock or
/// list sendable claims themselves (the relay, the licence list). The same
/// rule as [`classify`]'s [`Standing::Sendable`]; a test checks the two
/// agree on every status, paused or not, test TIN or not.
pub const SENDABLE_SQL: &str = "c.status = 'approved' AND c.paused_at IS NULL AND c.tin_root <> '000000000'";

/// The statuses of a live row: the one row an account may hold per TIN
/// (`tin_claims_one_live`, whose condition a test checks against this
/// list). Every other status has ended.
pub const LIVE_STATUSES: [&str; 4] = ["unverified", "pending", "needs_fix", "approved"];

/// [`LIVE_STATUSES`] as SQL on a `status` column. Write `c.{LIVE_SQL}` for
/// an aliased table.
pub const LIVE_SQL: &str = "status IN ('unverified', 'pending', 'needs_fix', 'approved')";

/// Whether a claim with this `status` is live ([`LIVE_STATUSES`]).
pub fn is_live(status: &str) -> bool {
    LIVE_STATUSES.contains(&status)
}

pub const UNVERIFIED_MESSAGE: &str =
    "This TIN is not verified yet. Verify it with its papers on My TINs at account.buwiz.com to send returns.";
pub const PENDING_MESSAGE: &str =
    "This TIN's papers are still being checked, usually within one business day. You can send once it is active.";
pub const NEEDS_FIX_PREFIX: &str = "Sent back for a fix: ";
pub const NEEDS_FIX_NO_REASON: &str = "Sent back for a fix. See what to change on My TINs at account.buwiz.com.";
pub const PAUSED_MESSAGE: &str = "Sending is paused for this TIN. Write to support@buwiz.com.";
pub const NOT_ACTIVE_MESSAGE: &str = "this TIN is not active on your account";
pub const TEST_ONLY_MESSAGE: &str =
    "This is a test TIN: you can prepare returns for it, but Buwiz never sends them to the BIR.";

/// Where a TIN stands for sending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Standing {
    /// Active and not paused: this claim sends.
    Sendable { claim_id: Uuid },
    /// Added without papers.
    Unverified,
    /// Papers waiting for review.
    Pending,
    /// Sent back; the reason sentences from [`claims::FIX_REASONS`].
    NeedsFix { reasons: Vec<&'static str> },
    /// Active, but staff paused sending.
    Paused,
    /// Active, but a test TIN: it never sends.
    TestOnly,
    /// Anything else: no row, ended, someone else's.
    NotActive,
}

/// Where a claim with this `status` stands. `fix_reasons` are its stored
/// reason codes; unknown ones are left out. `test_tin`: its TIN is a test
/// TIN ([`Tin::is_test`]).
pub fn classify(claim_id: Uuid, status: &str, paused: bool, fix_reasons: &[String], test_tin: bool) -> Standing {
    match status {
        "approved" if paused => Standing::Paused,
        "approved" if test_tin => Standing::TestOnly,
        "approved" => Standing::Sendable { claim_id },
        "unverified" => Standing::Unverified,
        "pending" => Standing::Pending,
        "needs_fix" => Standing::NeedsFix {
            reasons: fix_reasons.iter().filter_map(|code| claims::fix_reason(code)).map(|(_, text)| text).collect(),
        },
        _ => Standing::NotActive,
    }
}

type Row = (Uuid, String, bool, Vec<String>);

/// Where `tin` stands on this account: its own live row only.
pub async fn own(db: impl PgExecutor<'_>, account_id: Uuid, tin: &Tin) -> sqlx::Result<Standing> {
    let row: Option<Row> = sqlx::query_as(&format!(
        "SELECT id, status, paused_at IS NOT NULL, fix_reasons FROM tin_claims
         WHERE account_id = $1 AND tin_root = $2 AND branch_code = $3 AND {LIVE_SQL}"
    ))
    .bind(account_id)
    .bind(tin.root())
    .bind(tin.branch())
    .fetch_optional(db)
    .await?;
    Ok(row.map_or(Standing::NotActive, |(id, status, paused, reasons)| {
        classify(id, &status, paused, &reasons, tin.is_test())
    }))
}

/// Where the claim a ticket or a return was made for stands now, for the
/// account sending it. A claim that ended is [`Standing::NotActive`] even
/// when the account added the TIN again since: the ticket belongs to the
/// old one. So is a claim of another account (a claim never changes
/// account, but nothing here relies on that).
pub async fn of_claim(db: impl PgExecutor<'_>, account_id: Uuid, claim_id: Uuid) -> sqlx::Result<Standing> {
    let row: Option<(Uuid, String, bool, Vec<String>, String)> = sqlx::query_as(
        "SELECT id, status, paused_at IS NOT NULL, fix_reasons, tin_root FROM tin_claims WHERE id = $1 AND account_id = $2",
    )
    .bind(claim_id)
    .bind(account_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map_or(Standing::NotActive, |(id, status, paused, reasons, root)| {
        classify(id, &status, paused, &reasons, root == crate::account::tin::TEST_ROOT)
    }))
}

impl Standing {
    /// The claim to send with, or the `403` that says why not.
    pub fn sendable(self) -> Result<Uuid, ApiError> {
        match self {
            Self::Sendable { claim_id } => Ok(claim_id),
            refused => Err(ApiError::Refused {
                status: StatusCode::FORBIDDEN,
                code: refused.code(),
                message: refused.message(),
            }),
        }
    }

    pub fn can_send(&self) -> bool {
        matches!(self, Self::Sendable { .. })
    }

    /// The `status` of the TIN list's row (`include=all`): `approved` only
    /// for a TIN that can send.
    pub fn api_status(&self) -> &'static str {
        match self {
            Self::Sendable { .. } => "approved",
            Self::Unverified => "unverified",
            Self::Pending => "pending",
            Self::NeedsFix { .. } => "needs_fix",
            Self::Paused => "paused",
            Self::TestOnly => "test_only",
            Self::NotActive => "not_active",
        }
    }

    /// The TIN list's line for the row: where it stands and what is next.
    pub fn list_message(&self) -> String {
        match self {
            Self::Sendable { .. } => "Active. You can send returns for this TIN.".to_owned(),
            Self::Unverified => "Not verified. Verify it with its papers to send returns.".to_owned(),
            Self::Pending => "Waiting for review, usually within one business day.".to_owned(),
            Self::NeedsFix { .. } | Self::Paused | Self::TestOnly | Self::NotActive => self.message(),
        }
    }

    /// The refusal code, as the apps read it.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Sendable { .. } => "",
            Self::Unverified => "tin_unverified",
            Self::Pending => "tin_pending",
            Self::NeedsFix { .. } => "tin_needs_fix",
            Self::Paused => "tin_paused",
            Self::TestOnly => "tin_test_only",
            Self::NotActive => "tin_not_active",
        }
    }

    /// The refusal's words: one sentence or two, with the next step.
    pub fn message(&self) -> String {
        match self {
            Self::Sendable { .. } => String::new(),
            Self::Unverified => UNVERIFIED_MESSAGE.to_owned(),
            Self::Pending => PENDING_MESSAGE.to_owned(),
            Self::NeedsFix { reasons } if reasons.is_empty() => NEEDS_FIX_NO_REASON.to_owned(),
            Self::NeedsFix { reasons } => format!("{NEEDS_FIX_PREFIX}{}", reasons.join(" ")),
            Self::Paused => PAUSED_MESSAGE.to_owned(),
            Self::TestOnly => TEST_ONLY_MESSAGE.to_owned(),
            Self::NotActive => NOT_ACTIVE_MESSAGE.to_owned(),
        }
    }

    /// `"{message} ({code})"`: how a refusal reads in a relay `result`, the
    /// shape the apps already print server errors in.
    pub fn failure(&self) -> String {
        format!("{} ({})", self.message(), self.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|code| (*code).to_owned()).collect()
    }

    #[test]
    fn only_an_active_unpaused_claim_sends() {
        let id = Uuid::new_v4();
        assert_eq!(classify(id, "approved", false, &[], false), Standing::Sendable { claim_id: id });
        assert_eq!(classify(id, "approved", true, &[], false), Standing::Paused);
        assert_eq!(classify(id, "unverified", false, &[], false), Standing::Unverified);
        assert_eq!(classify(id, "pending", false, &[], false), Standing::Pending);
        for ended in ["rejected", "withdrawn", "removed", "revoked", "something new"] {
            assert_eq!(classify(id, ended, false, &[], false), Standing::NotActive, "{ended}");
            assert!(!is_live(ended), "{ended}");
        }
        for live in LIVE_STATUSES {
            assert!(LIVE_SQL.contains(&format!("'{live}'")), "{live}");
            assert_ne!(classify(id, live, false, &[], false), Standing::NotActive, "{live}");
        }
        assert_eq!(LIVE_SQL.matches('\'').count(), 2 * LIVE_STATUSES.len(), "{LIVE_SQL}");
        assert_eq!(Standing::Sendable { claim_id: id }.sendable().unwrap(), id);
        assert!(Standing::Paused.sendable().is_err());
    }

    #[test]
    fn an_active_test_tin_never_sends() {
        let id = Uuid::new_v4();
        assert_eq!(classify(id, "approved", false, &[], true), Standing::TestOnly);
        assert_eq!(classify(id, "approved", true, &[], true), Standing::Paused);
        assert_eq!(classify(id, "removed", false, &[], true), Standing::NotActive);
        let refused = Standing::TestOnly.sendable().unwrap_err();
        assert!(
            matches!(refused, ApiError::Refused { status: StatusCode::FORBIDDEN, code: "tin_test_only", .. }),
            "{refused:?}"
        );
        assert_eq!(Standing::TestOnly.api_status(), "test_only");
        assert!(SENDABLE_SQL.contains(&format!("c.tin_root <> '{}'", crate::account::tin::TEST_ROOT)));
    }

    #[test]
    fn a_claim_sent_back_names_its_fixed_reasons_only() {
        let fix = classify(Uuid::nil(), "needs_fix", false, &codes(&["blurry", "tin_mismatch", "not_a_code"]), false);
        assert_eq!(
            fix.message(),
            "Sent back for a fix: A photo is too blurry to read. The TIN you typed doesn't match the document."
        );
        assert_eq!(fix.code(), "tin_needs_fix");
        assert_eq!(classify(Uuid::nil(), "needs_fix", false, &[], false).message(), NEEDS_FIX_NO_REASON);
    }

    #[test]
    fn refusals_are_whole_sentences_without_another_code_in_them() {
        let all = [
            Standing::Unverified,
            Standing::Pending,
            Standing::NeedsFix { reasons: vec!["A photo is too blurry to read."] },
            Standing::NeedsFix { reasons: Vec::new() },
            Standing::Paused,
            Standing::TestOnly,
            Standing::NotActive,
        ];
        for standing in all {
            let message = standing.message();
            assert!(!message.contains("tin_not_active") && !message.is_empty(), "{message}");
            assert_eq!(standing.failure(), format!("{message} ({})", standing.code()));
        }
    }
}
