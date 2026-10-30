//! What the staff pages read: the claims queue, every account, and the
//! sign-ins each account has. Changes go through [`crate::claims`] and
//! [`crate::accounts`].

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::claims::{CLAIM_COLUMNS, ClaimRow};
use crate::oauth;

/// Rows a list shows at most; a search narrows the rest.
pub const LIST_LIMIT: i64 = 200;

/// `%text%` for `ILIKE`, with `text`'s own wildcards escaped, or `None` for
/// an empty search.
fn contains(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let escaped = text.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    Some(format!("%{escaped}%"))
}

/// The digits of a search, as a pattern for TINs stored without dashes.
fn tin_digits(text: &str) -> Option<String> {
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() { None } else { Some(format!("%{digits}%")) }
}

/// The tabs of the claims queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueFilter {
    Waiting,
    SentBack,
    Approved,
    Rejected,
}

impl QueueFilter {
    pub const ALL: [Self; 4] = [Self::Waiting, Self::SentBack, Self::Approved, Self::Rejected];

    pub fn parse(raw: Option<&str>) -> Self {
        Self::ALL.into_iter().find(|filter| Some(filter.code()) == raw).unwrap_or(Self::Waiting)
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::SentBack => "sent_back",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Waiting => "Waiting",
            Self::SentBack => "Sent back",
            Self::Approved => "Approved",
            Self::Rejected => "Rejected",
        }
    }

    fn status(self) -> &'static str {
        match self {
            Self::Waiting => "pending",
            Self::SentBack => "needs_fix",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }

    /// Open claims oldest first, so nobody waits longest; decided ones
    /// newest first.
    fn order(self) -> &'static str {
        match self {
            Self::Waiting => "coalesce(c.resent_at, c.created_at), c.id",
            Self::SentBack => "c.sent_back_at, c.id",
            Self::Approved | Self::Rejected => "c.decided_at DESC, c.id",
        }
    }
}

/// A claim in the queue, with what the reviewer should notice first.
#[derive(Debug, sqlx::FromRow)]
pub struct QueueRow {
    #[sqlx(flatten)]
    pub claim: ClaimRow,
    /// Other accounts with an open or approved claim on the same TIN.
    pub rivals: i64,
    /// Kinds of the files still stored.
    pub document_kinds: Vec<String>,
    /// The kind of the newest TIN ID or 2303 read on our server, and how
    /// its TIN and name compare with what was typed.
    pub reading_kind: Option<String>,
    pub tin_match: Option<String>,
    pub name_match: Option<String>,
}

pub async fn queue(db: &PgPool, filter: QueueFilter, search: &str) -> sqlx::Result<Vec<QueueRow>> {
    sqlx::query_as(&format!(
        "SELECT {CLAIM_COLUMNS},
             (SELECT count(*) FROM tin_claims o
              WHERE o.tin_root = c.tin_root AND o.branch_code = c.branch_code AND o.account_id <> c.account_id
                AND o.status IN ('pending', 'needs_fix', 'approved')) AS rivals,
             ARRAY(SELECT DISTINCT d.kind FROM claim_documents d
                   WHERE d.claim_id = c.id AND d.purged_at IS NULL) AS document_kinds,
             reading.reading_kind, reading.tin_match, reading.name_match
         FROM tin_claims c JOIN accounts a ON a.id = c.account_id
         LEFT JOIN LATERAL (
             SELECT d.kind AS reading_kind, r.tin_match, r.name_match
             FROM claim_documents d JOIN document_readings r ON r.document_id = d.id
             WHERE d.claim_id = c.id AND d.purged_at IS NULL AND r.status = 'read'
             ORDER BY d.created_at DESC LIMIT 1
         ) reading ON true
         WHERE c.status = $1
           AND ($2::text IS NULL OR a.email ILIKE $2 OR c.registered_name ILIKE $2 OR c.trade_name ILIKE $2
                OR ($3::text IS NOT NULL AND c.tin_root || c.branch_code LIKE $3))
         ORDER BY {}
         LIMIT $4",
        filter.order()
    ))
    .bind(filter.status())
    .bind(contains(search))
    .bind(tin_digits(search))
    .bind(LIST_LIMIT)
    .fetch_all(db)
    .await
}

/// Claims waiting for review and claims sent back, for the tab counts.
pub async fn open_counts(db: &PgPool) -> sqlx::Result<(i64, i64)> {
    sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status = 'pending'), count(*) FILTER (WHERE status = 'needs_fix')
         FROM tin_claims WHERE status IN ('pending', 'needs_fix')",
    )
    .fetch_one(db)
    .await
}

/// The claims waiting just before and just after this one in the queue.
pub async fn neighbours(db: &PgPool, claim: &ClaimRow) -> sqlx::Result<(Option<Uuid>, Option<Uuid>)> {
    let at = claim.waiting_since();
    let previous = sqlx::query_scalar(
        "SELECT id FROM tin_claims WHERE status = 'pending' AND (coalesce(resent_at, created_at), id) < ($1, $2)
         ORDER BY coalesce(resent_at, created_at) DESC, id DESC LIMIT 1",
    )
    .bind(at)
    .bind(claim.id)
    .fetch_optional(db)
    .await?;
    let next = sqlx::query_scalar(
        "SELECT id FROM tin_claims WHERE status = 'pending' AND (coalesce(resent_at, created_at), id) > ($1, $2)
         ORDER BY coalesce(resent_at, created_at), id LIMIT 1",
    )
    .bind(at)
    .bind(claim.id)
    .fetch_optional(db)
    .await?;
    Ok((previous, next))
}

/// The filter chips of the users list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserFilter {
    Everyone,
    Waiting,
    Verified,
    NoTin,
    Staff,
}

impl UserFilter {
    pub const ALL: [Self; 5] = [Self::Everyone, Self::Waiting, Self::Verified, Self::NoTin, Self::Staff];

    pub fn parse(raw: Option<&str>) -> Self {
        Self::ALL.into_iter().find(|filter| Some(filter.code()) == raw).unwrap_or(Self::Everyone)
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Everyone => "everyone",
            Self::Waiting => "waiting",
            Self::Verified => "verified",
            Self::NoTin => "no_tin",
            Self::Staff => "staff",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Everyone => "Everyone",
            Self::Waiting => "ID waiting for review",
            Self::Verified => "Verified",
            Self::NoTin => "No TIN yet",
            Self::Staff => "Staff",
        }
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub email: String,
    pub created_at: OffsetDateTime,
    /// The newest website sign-in or app token, whichever is later.
    pub last_active: Option<OffsetDateTime>,
}

const USER_COLUMNS: &str = "a.id, a.email, a.created_at,
     GREATEST(
         (SELECT max(s.created_at) FROM web_sessions s WHERE s.account_id = a.id),
         (SELECT max(t.created_at) FROM oauth_access_tokens t JOIN oauth_grants g ON g.id = t.grant_id
          WHERE g.account_id = a.id)
     ) AS last_active";

pub async fn users(db: &PgPool, filter: UserFilter, search: &str, staff: &[String]) -> sqlx::Result<Vec<UserRow>> {
    sqlx::query_as(&format!(
        "SELECT {USER_COLUMNS} FROM accounts a
         WHERE ($1::text IS NULL OR a.email ILIKE $1 OR EXISTS (
                 SELECT 1 FROM tin_claims c WHERE c.account_id = a.id
                   AND (c.registered_name ILIKE $1 OR c.trade_name ILIKE $1
                        OR ($2::text IS NOT NULL AND c.tin_root || c.branch_code LIKE $2))))
           AND CASE $3
               WHEN 'waiting' THEN EXISTS (SELECT 1 FROM tin_claims c
                                           WHERE c.account_id = a.id AND c.status IN ('pending', 'needs_fix'))
               WHEN 'verified' THEN EXISTS (SELECT 1 FROM tin_claims c
                                            WHERE c.account_id = a.id AND c.status = 'approved')
               WHEN 'no_tin' THEN NOT EXISTS (SELECT 1 FROM tin_claims c WHERE c.account_id = a.id
                                              AND c.status IN ('pending', 'needs_fix', 'approved'))
               WHEN 'staff' THEN a.email = ANY($4)
               ELSE true
           END
         ORDER BY last_active DESC NULLS LAST, a.created_at DESC
         LIMIT $5"
    ))
    .bind(contains(search))
    .bind(tin_digits(search))
    .bind(filter.code())
    .bind(staff)
    .bind(LIST_LIMIT)
    .fetch_all(db)
    .await
}

pub async fn user(db: &PgPool, account_id: Uuid) -> sqlx::Result<Option<UserRow>> {
    sqlx::query_as(&format!("SELECT {USER_COLUMNS} FROM accounts a WHERE a.id = $1"))
        .bind(account_id)
        .fetch_optional(db)
        .await
}

/// A claim of a listed account, without withdrawn ones.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct UserClaim {
    pub account_id: Uuid,
    pub tin_root: String,
    pub branch_code: String,
    pub registered_name: String,
    pub status: String,
}

pub async fn claims_of(db: &PgPool, account_ids: &[Uuid]) -> sqlx::Result<Vec<UserClaim>> {
    sqlx::query_as(
        "SELECT account_id, tin_root, branch_code, registered_name, status FROM tin_claims
         WHERE account_id = ANY($1) AND status <> 'withdrawn' ORDER BY created_at",
    )
    .bind(account_ids)
    .fetch_all(db)
    .await
}

/// Where an account stands with its identity, as the users list shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdStatus {
    Staff,
    Waiting,
    Verified,
    None,
}

impl IdStatus {
    /// Staff first; then a claim waiting for review or a fix; then any
    /// approved claim.
    pub fn of(is_staff: bool, statuses: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        if is_staff {
            return Self::Staff;
        }
        let mut status = Self::None;
        for claim in statuses {
            match claim.as_ref() {
                "pending" | "needs_fix" => return Self::Waiting,
                "approved" => status = Self::Verified,
                _ => {}
            }
        }
        status
    }

    /// (label, icon, CSS tone)
    pub fn look(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Staff => ("Staff", "shield_person", "staff"),
            Self::Waiting => ("ID waiting", "hourglass_top", "waiting"),
            Self::Verified => ("ID verified", "verified_user", "verified"),
            Self::None => ("No ID yet", "remove_moderator", "none"),
        }
    }
}

/// An app or agent sign-in of an account.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct AppRow {
    pub id: Uuid,
    pub client_id: String,
    pub scope: String,
    /// The name an agent registered with; `None` for the Buwiz apps.
    pub registered_name: Option<String>,
    pub created_at: OffsetDateTime,
    pub revoked_at: Option<OffsetDateTime>,
    pub revoke_reason: Option<String>,
    /// The newest access token.
    pub last_used: Option<OffsetDateTime>,
    /// Not revoked, and holding a token that still works.
    pub signed_in: bool,
}

impl AppRow {
    pub fn name(&self) -> String {
        match oauth::client_name(&self.client_id) {
            Some(name) if self.scope != oauth::SCOPE_MCP => format!("{name} app"),
            _ => self.registered_name.clone().unwrap_or_else(|| self.client_id.clone()),
        }
    }

    pub fn is_agent(&self) -> bool {
        self.scope == oauth::SCOPE_MCP
    }
}

/// The account's app and agent sign-ins, newest first.
pub async fn apps(db: &PgPool, account_id: Uuid) -> sqlx::Result<Vec<AppRow>> {
    sqlx::query_as(
        "SELECT g.id, g.client_id, g.scope, oc.client_name AS registered_name, g.created_at, g.revoked_at,
             g.revoke_reason,
             (SELECT max(t.created_at) FROM oauth_access_tokens t WHERE t.grant_id = g.id) AS last_used,
             (g.revoked_at IS NULL AND (
                 EXISTS (SELECT 1 FROM oauth_refresh_tokens r
                         WHERE r.grant_id = g.id AND r.used_at IS NULL AND r.expires_at > now())
                 OR EXISTS (SELECT 1 FROM oauth_access_tokens t WHERE t.grant_id = g.id AND t.expires_at > now())
             )) AS signed_in
         FROM oauth_grants g LEFT JOIN oauth_clients oc ON oc.client_id = g.client_id
         WHERE g.account_id = $1
         ORDER BY g.created_at DESC
         LIMIT 50",
    )
    .bind(account_id)
    .fetch_all(db)
    .await
}

/// Website sign-ins: (started, still signed in), newest first.
pub async fn sessions(db: &PgPool, account_id: Uuid) -> sqlx::Result<Vec<(OffsetDateTime, bool)>> {
    sqlx::query_as(
        "SELECT created_at, revoked_at IS NULL AND expires_at > now() FROM web_sessions
         WHERE account_id = $1 ORDER BY created_at DESC LIMIT 20",
    )
    .bind(account_id)
    .fetch_all(db)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_escape_wildcards_and_find_tin_digits() {
        assert_eq!(contains("  "), None);
        assert_eq!(contains("100%_a\\b").as_deref(), Some("%100\\%\\_a\\\\b%"));
        assert_eq!(tin_digits("123-456").as_deref(), Some("%123456%"));
        assert_eq!(tin_digits("ana"), None);
    }

    #[test]
    fn id_status_puts_staff_then_waiting_first() {
        assert_eq!(IdStatus::of(true, ["pending"]), IdStatus::Staff);
        assert_eq!(IdStatus::of(false, ["approved", "needs_fix"]), IdStatus::Waiting);
        assert_eq!(IdStatus::of(false, ["rejected", "approved"]), IdStatus::Verified);
        assert_eq!(IdStatus::of(false, ["rejected"]), IdStatus::None);
    }

    #[test]
    fn unknown_filters_fall_back_to_the_default() {
        assert_eq!(QueueFilter::parse(Some("sent_back")), QueueFilter::SentBack);
        assert_eq!(QueueFilter::parse(Some("<x>")), QueueFilter::Waiting);
        assert_eq!(UserFilter::parse(None), UserFilter::Everyone);
    }
}
