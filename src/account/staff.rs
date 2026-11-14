//! What the staff pages read: the claims queue, every account, and the
//! sign-ins each account has. Changes go through [`crate::account::claims`] and
//! [`crate::account::accounts`].

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::account::claims::{CLAIM_COLUMNS, ClaimRow};
use crate::account::oauth;
use crate::filing::gate::LIVE_SQL;

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
    /// Saved without papers: nothing to review yet.
    Unverified,
    /// Activated automatically and not yet confirmed by a person, in any
    /// status: removed by the owner, revoked or paused too.
    Auto,
    /// Active and not paused.
    Approved,
    Paused,
    Rejected,
    /// Approved once, then removed by the owner or revoked by staff.
    Ended,
}

impl QueueFilter {
    pub const ALL: [Self; 8] = [
        Self::Waiting,
        Self::SentBack,
        Self::Unverified,
        Self::Auto,
        Self::Approved,
        Self::Paused,
        Self::Rejected,
        Self::Ended,
    ];

    pub fn parse(raw: Option<&str>) -> Self {
        Self::ALL.into_iter().find(|filter| Some(filter.code()) == raw).unwrap_or(Self::Waiting)
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::SentBack => "sent_back",
            Self::Unverified => "unverified",
            Self::Auto => "auto",
            Self::Approved => "approved",
            Self::Paused => "paused",
            Self::Rejected => "rejected",
            Self::Ended => "ended",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Waiting => "Waiting",
            Self::SentBack => "Sent back",
            Self::Unverified => "Not verified",
            Self::Auto => "Activated automatically",
            Self::Approved => "Approved",
            Self::Paused => "Paused",
            Self::Rejected => "Rejected",
            Self::Ended => "Removed",
        }
    }

    fn statuses(self) -> &'static [&'static str] {
        match self {
            Self::Waiting => &["pending"],
            Self::SentBack => &["needs_fix"],
            Self::Unverified => &["unverified"],
            Self::Auto => {
                &["unverified", "pending", "needs_fix", "approved", "rejected", "withdrawn", "removed", "revoked"]
            }
            Self::Approved | Self::Paused => &["approved"],
            Self::Rejected => &["rejected"],
            Self::Ended => &["removed", "revoked"],
        }
    }

    /// Open claims oldest first, so nobody waits longest; decided ones
    /// newest first.
    /// Whether the tab holds paused TINs, not paused ones, or either.
    fn paused(self) -> Option<bool> {
        match self {
            Self::Approved => Some(false),
            Self::Paused => Some(true),
            _ => None,
        }
    }

    /// Whether the tab holds only automatic activations nobody confirmed.
    fn unconfirmed_auto(self) -> Option<bool> {
        (self == Self::Auto).then_some(true)
    }

    fn order(self) -> &'static str {
        match self {
            Self::Auto => "c.decided_at, c.id",
            Self::Waiting => "coalesce(c.resent_at, c.submitted_at, c.created_at), c.id",
            Self::SentBack => "c.sent_back_at, c.id",
            Self::Unverified => "c.created_at DESC, c.id",
            Self::Approved | Self::Rejected => "c.decided_at DESC, c.id",
            Self::Paused => "c.paused_at DESC, c.id",
            Self::Ended => "c.removed_at DESC, c.id",
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
    /// The claimant's account is labelled a test account.
    pub test_account: bool,
}

pub async fn queue(db: &PgPool, filter: QueueFilter, search: &str) -> sqlx::Result<Vec<QueueRow>> {
    sqlx::query_as(&format!(
        "SELECT {CLAIM_COLUMNS},
             (SELECT count(*) FROM tin_claims o
              WHERE o.tin_root = c.tin_root AND o.branch_code = c.branch_code AND o.account_id <> c.account_id
                AND o.status IN ('pending', 'needs_fix', 'approved')) AS rivals,
             ARRAY(SELECT DISTINCT d.kind FROM claim_documents d
                   WHERE d.claim_id = c.id AND d.purged_at IS NULL) AS document_kinds,
             reading.reading_kind, reading.tin_match, reading.name_match, a.test_account
         FROM tin_claims c JOIN accounts a ON a.id = c.account_id
         LEFT JOIN LATERAL (
             SELECT d.kind AS reading_kind, r.tin_match, r.name_match
             FROM claim_documents d JOIN document_readings r ON r.document_id = d.id
             WHERE d.claim_id = c.id AND d.purged_at IS NULL AND r.status = 'read'
             ORDER BY d.created_at DESC LIMIT 1
         ) reading ON true
         WHERE c.status = ANY($1) AND ($5::bool IS NULL OR (c.paused_at IS NOT NULL) = $5)
           AND ($6::bool IS NULL OR (c.verified_by = 'auto' AND c.auto_confirmed_at IS NULL))
           AND ($2::text IS NULL OR a.email ILIKE $2 OR c.registered_name ILIKE $2 OR c.trade_name ILIKE $2
                OR ($3::text IS NOT NULL AND c.tin_root || c.branch_code LIKE $3))
         ORDER BY {}
         LIMIT $4",
        filter.order()
    ))
    .bind(filter.statuses())
    .bind(contains(search))
    .bind(tin_digits(search))
    .bind(LIST_LIMIT)
    .bind(filter.paused())
    .bind(filter.unconfirmed_auto())
    .fetch_all(db)
    .await
}

/// TINs activated automatically that no person has confirmed yet, for the
/// tab's count.
pub async fn unconfirmed_auto(db: &PgPool) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM tin_claims WHERE verified_by = 'auto' AND auto_confirmed_at IS NULL")
        .fetch_one(db)
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

/// The staff sidebar's live counts. Each is the length of a list every
/// staff member can already open: the Waiting, Sent back and Activated
/// automatically tabs, and the Everyone, Staff, Test accounts and Banned
/// filters of the users list (without its row limit).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, sqlx::FromRow)]
pub struct Counts {
    pub waiting: i64,
    pub sent_back: i64,
    /// [`unconfirmed_auto`].
    pub auto: i64,
    pub users: i64,
    pub staff: i64,
    pub test: i64,
    pub banned: i64,
}

/// [`Counts`], in one round trip. `admin_emails` is `ADMIN_EMAILS`, whose
/// people count as staff as in [`users`]. Small tables, counted whole: no
/// index serves a status count, and none is needed at this size.
pub async fn counts(db: &PgPool, admin_emails: &[String]) -> sqlx::Result<Counts> {
    sqlx::query_as(&format!(
        "SELECT
             (SELECT count(*) FROM tin_claims WHERE status = 'pending') AS waiting,
             (SELECT count(*) FROM tin_claims WHERE status = 'needs_fix') AS sent_back,
             (SELECT count(*) FROM tin_claims WHERE verified_by = 'auto' AND auto_confirmed_at IS NULL) AS auto,
             (SELECT count(*) FROM accounts) AS users,
             (SELECT count(*) FROM accounts a WHERE {staff}) AS staff,
             (SELECT count(*) FROM accounts WHERE test_account) AS test,
             (SELECT count(*) FROM accounts WHERE banned_at IS NOT NULL) AS banned",
        staff = crate::account::roles::is_staff_sql("a", "$1")
    ))
    .bind(admin_emails)
    .fetch_one(db)
    .await
}

/// The Overview's numbers. Test TINs (`000-000-000`) are left out of the
/// active TINs: they are made up and never send.
#[derive(Clone, Copy, Debug, sqlx::FromRow)]
pub struct Overview {
    pub waiting: i64,
    /// When the claim waiting longest started waiting.
    pub oldest_waiting: Option<OffsetDateTime>,
    /// Active and not paused.
    pub active: i64,
    /// Of those, activated in the last seven days.
    pub active_week: i64,
    pub accounts: i64,
    pub accounts_week: i64,
    /// [`unconfirmed_auto`].
    pub auto: i64,
}

pub async fn overview(db: &PgPool) -> sqlx::Result<Overview> {
    sqlx::query_as(&format!(
        "SELECT
             (SELECT count(*) FROM tin_claims WHERE status = 'pending') AS waiting,
             (SELECT min(coalesce(resent_at, submitted_at, created_at)) FROM tin_claims WHERE status = 'pending')
                 AS oldest_waiting,
             (SELECT count(*) FROM tin_claims WHERE status = 'approved' AND paused_at IS NULL
                 AND tin_root <> '{test}') AS active,
             (SELECT count(*) FROM tin_claims WHERE status = 'approved' AND paused_at IS NULL
                 AND tin_root <> '{test}' AND decided_at > now() - interval '7 days') AS active_week,
             (SELECT count(*) FROM accounts) AS accounts,
             (SELECT count(*) FROM accounts WHERE created_at > now() - interval '7 days') AS accounts_week,
             (SELECT count(*) FROM tin_claims WHERE verified_by = 'auto' AND auto_confirmed_at IS NULL) AS auto",
        test = crate::account::tin::TEST_ROOT
    ))
    .fetch_one(db)
    .await
}

/// Returns sent to the BIR since `since` (the start of today, Philippine
/// time), per hour from then: (hours after `since`, count). An Admin's
/// number: no staff page lists returns.
pub async fn sent_since(db: &PgPool, since: OffsetDateTime) -> sqlx::Result<Vec<(i32, i64)>> {
    sqlx::query_as(
        "SELECT floor(extract(epoch FROM sent_at - $1) / 3600)::int AS hour, count(*)
         FROM submissions WHERE status = 'sent' AND sent_at >= $1
         GROUP BY 1 ORDER BY 1",
    )
    .bind(since)
    .fetch_all(db)
    .await
}

/// The claims waiting longest, oldest first, as the Waiting tab orders them.
pub async fn waiting_longest(db: &PgPool, limit: i64) -> sqlx::Result<Vec<ClaimRow>> {
    sqlx::query_as(&format!(
        "SELECT {CLAIM_COLUMNS} FROM tin_claims c JOIN accounts a ON a.id = c.account_id
         WHERE c.status = 'pending'
         ORDER BY coalesce(c.resent_at, c.submitted_at, c.created_at), c.id
         LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(db)
    .await
}

/// A line of the Overview's recent staff activity: what a staff member (or
/// the seed tool) did, to whom, and when. It carries no reason; the account's
/// history has those.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Activity {
    /// `claim` (a `claim_audit` line) or `account` (an `account_audit` one).
    pub kind: String,
    pub action: String,
    /// The masked TIN of a claim line.
    pub tin_masked: Option<String>,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub account_id: Uuid,
    /// The account acted on; `None` once it is gone.
    pub account_email: Option<String>,
    pub actor_email: String,
    /// `web` or `seed`.
    pub source: String,
    pub created_at: OffsetDateTime,
}

/// The newest staff actions on claims and accounts, newest first: lines
/// written from the staff pages or the seed tool, not Buwiz's own, and
/// not what people did to their own claims (a staff member acting on
/// their own TIN as staff still counts).
pub async fn recent_activity(db: &PgPool, limit: i64) -> sqlx::Result<Vec<Activity>> {
    sqlx::query_as(
        "(SELECT 'claim' AS kind, l.action, l.tin_masked, NULL::text AS old_value, NULL::text AS new_value,
                 l.account_id, a.email AS account_email, l.actor_email, l.source, l.created_at
          FROM claim_audit l LEFT JOIN accounts a ON a.id = l.account_id
          WHERE l.source IN ('web', 'seed')
            AND (l.actor_id <> l.account_id
                 OR l.action IN ('approved', 'rejected', 'sent_back', 'activated', 'paused', 'resumed', 'revoked',
                                 'auto_confirmed'))
          ORDER BY l.created_at DESC LIMIT $1)
         UNION ALL
         (SELECT 'account', l.action, NULL, l.old_value, l.new_value,
                 l.account_id, a.email, l.actor_email, l.source, l.created_at
          FROM account_audit l LEFT JOIN accounts a ON a.id = l.account_id
          ORDER BY l.created_at DESC LIMIT $1)
         ORDER BY created_at DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Claims in any state that match a search, newest first, by the queue's
/// rules: the claimant's email, the registered or trade name, or the TIN's
/// digits. Nothing for an empty search.
pub async fn search_claims(db: &PgPool, search: &str) -> sqlx::Result<Vec<ClaimRow>> {
    let Some(pattern) = contains(search) else {
        return Ok(Vec::new());
    };
    sqlx::query_as(&format!(
        "SELECT {CLAIM_COLUMNS} FROM tin_claims c JOIN accounts a ON a.id = c.account_id
         WHERE a.email ILIKE $1 OR c.registered_name ILIKE $1 OR c.trade_name ILIKE $1
               OR ($2::text IS NOT NULL AND c.tin_root || c.branch_code LIKE $2)
         ORDER BY c.created_at DESC, c.id
         LIMIT $3"
    ))
    .bind(pattern)
    .bind(tin_digits(search))
    .bind(SEARCH_LIMIT)
    .fetch_all(db)
    .await
}

/// Rows each part of a search-everything shows at most.
pub const SEARCH_LIMIT: i64 = 50;

/// The claims waiting just before and just after this one in the queue.
pub async fn neighbours(db: &PgPool, claim: &ClaimRow) -> sqlx::Result<(Option<Uuid>, Option<Uuid>)> {
    let at = claim.waiting_since();
    let previous = sqlx::query_scalar(
        "SELECT id FROM tin_claims
         WHERE status = 'pending' AND (coalesce(resent_at, submitted_at, created_at), id) < ($1, $2)
         ORDER BY coalesce(resent_at, submitted_at, created_at) DESC, id DESC LIMIT 1",
    )
    .bind(at)
    .bind(claim.id)
    .fetch_optional(db)
    .await?;
    let next = sqlx::query_scalar(
        "SELECT id FROM tin_claims
         WHERE status = 'pending' AND (coalesce(resent_at, submitted_at, created_at), id) > ($1, $2)
         ORDER BY coalesce(resent_at, submitted_at, created_at), id LIMIT 1",
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
    Test,
    Banned,
}

impl UserFilter {
    pub const ALL: [Self; 7] =
        [Self::Everyone, Self::Waiting, Self::Verified, Self::NoTin, Self::Staff, Self::Test, Self::Banned];

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
            Self::Test => "test",
            Self::Banned => "banned",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Everyone => "Everyone",
            Self::Waiting => "ID waiting for review",
            Self::Verified => "Verified",
            Self::NoTin => "No TIN yet",
            Self::Staff => "Staff",
            Self::Test => "Test accounts",
            Self::Banned => "Banned",
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
    /// The stored staff role (`admin`, `support`); see [`crate::account::roles`]
    /// for the role that counts.
    pub role: Option<String>,
    pub test_account: bool,
    pub banned_at: Option<OffsetDateTime>,
    pub ban_reason: Option<String>,
    pub banned_by_email: Option<String>,
}

const USER_COLUMNS: &str = "a.id, a.email, a.created_at,
     GREATEST(
         (SELECT max(s.created_at) FROM web_sessions s WHERE s.account_id = a.id),
         (SELECT max(t.created_at) FROM oauth_access_tokens t JOIN oauth_grants g ON g.id = t.grant_id
          WHERE g.account_id = a.id)
     ) AS last_active,
     a.role, a.test_account, a.banned_at, a.ban_reason,
     (SELECT b.email FROM accounts b WHERE b.id = a.banned_by) AS banned_by_email";

/// Accounts for the users list. `admin_emails` is `ADMIN_EMAILS`, whose
/// people are staff whatever their account says.
pub async fn users(
    db: &PgPool,
    filter: UserFilter,
    search: &str,
    admin_emails: &[String],
) -> sqlx::Result<Vec<UserRow>> {
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
                                              AND c.{LIVE_SQL})
               WHEN 'staff' THEN {staff}
               WHEN 'test' THEN a.test_account
               WHEN 'banned' THEN a.banned_at IS NOT NULL
               ELSE true
           END
         ORDER BY last_active DESC NULLS LAST, a.created_at DESC
         LIMIT $5",
        staff = crate::account::roles::is_staff_sql("a", "$4")
    ))
    .bind(contains(search))
    .bind(tin_digits(search))
    .bind(filter.code())
    .bind(admin_emails)
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
    /// As people read it: an active TIN staff paused is `paused`.
    pub status: String,
}

pub async fn claims_of(db: &PgPool, account_ids: &[Uuid]) -> sqlx::Result<Vec<UserClaim>> {
    sqlx::query_as(
        "SELECT account_id, tin_root, branch_code, registered_name,
             CASE WHEN paused_at IS NOT NULL THEN 'paused' ELSE status END AS status FROM tin_claims
         WHERE account_id = ANY($1) AND status <> 'withdrawn' ORDER BY created_at",
    )
    .bind(account_ids)
    .fetch_all(db)
    .await
}

/// A line of `claim_audit`: a claim deleted, a TIN removed or revoked, a
/// file replaced or removed, a TIN activated without papers, paused or
/// resumed.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct AuditRow {
    pub tin_masked: String,
    pub action: String,
    pub claim_status: String,
    pub document_kind: Option<String>,
    pub actor_id: Uuid,
    pub actor_email: String,
    pub reason: Option<String>,
    /// `web`, `auto` or `seed`.
    pub source: String,
    pub created_at: OffsetDateTime,
}

/// The audit lines about an account's claims, newest first.
pub async fn audit(db: &PgPool, account_id: Uuid) -> sqlx::Result<Vec<AuditRow>> {
    sqlx::query_as(
        "SELECT tin_masked, action, claim_status, document_kind, actor_id, actor_email, reason, source, created_at
         FROM claim_audit WHERE account_id = $1 ORDER BY created_at DESC LIMIT 40",
    )
    .bind(account_id)
    .fetch_all(db)
    .await
}

/// A line of `account_audit`: what staff or the seed tool did to the
/// account itself.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct AccountAuditRow {
    pub action: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub actor_id: Uuid,
    pub actor_email: String,
    /// `web` or `seed`.
    pub source: String,
    pub reason: Option<String>,
    pub created_at: OffsetDateTime,
}

/// The account's own audit lines, newest first.
pub async fn account_audit(db: &PgPool, account_id: Uuid) -> sqlx::Result<Vec<AccountAuditRow>> {
    sqlx::query_as(
        "SELECT action, old_value, new_value, actor_id, actor_email, source, reason, created_at
         FROM account_audit WHERE account_id = $1 ORDER BY created_at DESC LIMIT 40",
    )
    .bind(account_id)
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
    /// approved claim. A TIN saved without papers proves no identity, so it
    /// counts for neither.
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
        assert_eq!(IdStatus::of(false, ["unverified"]), IdStatus::None);
        assert_eq!(IdStatus::of(false, ["unverified", "approved"]), IdStatus::Verified);
    }

    #[test]
    fn unknown_filters_fall_back_to_the_default() {
        assert_eq!(QueueFilter::parse(Some("sent_back")), QueueFilter::SentBack);
        assert_eq!(QueueFilter::parse(Some("<x>")), QueueFilter::Waiting);
        assert_eq!(UserFilter::parse(None), UserFilter::Everyone);
    }
}
