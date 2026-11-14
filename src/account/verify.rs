//! Automatic activation (`AUTO_VERIFY`): a personal TIN whose TIN ID photo,
//! read by OCR on this server, clearly shows the TIN and name typed becomes
//! active without waiting for staff. **Off unless `AUTO_VERIFY=on`.**
//!
//! What it proves is weak: the claimant types the TIN and name and uploads
//! the picture, so a drawn card passes (tests/ocr.rs shows one). It catches
//! typing mistakes, not fraud. So it is hedged in on every side:
//!
//! - only a personal head-office TIN, only one TIN ID, only a photo read by
//!   OCR (never a PDF's text layer), both TIN and name a full match, read
//!   with at least 90% confidence, and a typed name of two words or more;
//! - only a claim waiting for its first review (never sent back, on this
//!   claim or an earlier one of the account), of an account that is not
//!   banned, on a TIN no other account ever had a row for (at any branch),
//!   that this account was never refused, revoked or (after an automatic
//!   activation) removed, that was never paused, and that never had a
//!   business claim. "Personal" is the claimant's own word: a business's
//!   TIN claimed as personal by someone who never claimed it as a business
//!   still passes this. Deleted claims count too: their audit lines are
//!   matched by account and masked TIN (the last three digits; a collision
//!   only leaves more claims for staff);
//! - at most one automatic activation per account waiting for a person to
//!   confirm it, and 20 a day across Buwiz, both counted under one lock so
//!   readings finishing at once cannot pass them;
//! - the papers are kept until a staff member confirms ([`confirm`]), even
//!   when the owner removes the TIN or staff revoke it, and every such TIN
//!   is listed for staff ("Activated automatically") until confirmed;
//! - one still unconfirmed after 30 days is paused by Buwiz, papers kept
//!   ([`upkeep`]).
//!
//! The rule is the pure [`assess`]; [`consider_document`] gathers its facts
//! after a reading is saved and acts on the answer. Every doubt that kept a
//! claim for staff is stored on it (`auto_check`) in plain words for the
//! reviewer.

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::types::Json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::account::accounts::Account;
use crate::account::claims::DecideError;
use crate::account::notice::Notice;
use crate::account::roles::{self, Power, Role};
use crate::account::tin::Tin;
use crate::base::config::Config;
use crate::base::crypto;
use crate::base::state::AppState;
use crate::base::storage::DocumentStore;
use crate::ocr::extract::{self, Confidence, Fields, Kind, Verdict};

/// How sure OCR must be of the TIN and of the name.
pub const MIN_CONFIDENCE: f32 = 0.90;
/// Automatic activations across Buwiz in any 24 hours.
pub const DAILY_LIMIT: i64 = 20;
/// Days an automatic activation may wait for a person before Buwiz pauses
/// it.
pub const CONFIRM_WITHIN_DAYS: i64 = 30;
/// What the pause after [`CONFIRM_WITHIN_DAYS`] says, to the owner too.
pub const UNCONFIRMED_PAUSE: &str = "Buwiz activated this TIN from its TIN ID automatically, and no one at Buwiz \
     confirmed the papers within 30 days. Sending waits until staff check them.";
/// The audit line's reason for an automatic activation.
pub const ACTIVATED_REASON: &str = "TIN and name match the TIN ID read on our server";

/// The `AUTO_VERIFY` setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Verification {
    /// Nothing is activated automatically, and nothing is checked.
    #[default]
    Off,
    On,
}

impl Verification {
    /// Only the exact value `on` turns it on: unset, empty, `ON`, `true`,
    /// `1` or a typo all leave it off.
    pub fn parse(raw: Option<&str>) -> Self {
        if raw == Some("on") { Self::On } else { Self::Off }
    }

    pub fn from_env() -> Self {
        Self::parse(std::env::var("AUTO_VERIFY").ok().as_deref())
    }
}

/// The reading of the claim's one TIN ID, as stored.
#[derive(Clone, Debug, Default)]
pub struct ReadingFacts {
    /// `reading`, `read` or `failed`.
    pub status: String,
    pub engine: Option<String>,
    pub engine_version: Option<String>,
    pub error: Option<String>,
    pub fields: Option<Fields>,
    pub confidence: Option<Confidence>,
}

/// A TIN ID file still stored on the claim.
#[derive(Clone, Debug)]
pub struct TinIdFile {
    pub document_id: Uuid,
    pub content_type: String,
    pub sha256: Vec<u8>,
    pub reading: Option<ReadingFacts>,
}

/// Everything the rule looks at, gathered under the lock.
#[derive(Clone, Debug)]
pub struct Facts {
    pub verification: Verification,
    pub banned: bool,
    pub kind: String,
    pub status: String,
    pub sent_back: bool,
    pub tin: Tin,
    pub registered_name: String,
    pub tin_ids: Vec<TinIdFile>,
    /// Another account has or had a row on this TIN's root, at any branch
    /// and in any status, deleted ones included.
    pub other_account: bool,
    /// This account had this TIN rejected, revoked or deleted, or removed
    /// it after it was activated automatically.
    pub refused_before: bool,
    /// Sending for this TIN was ever paused, on any account.
    pub paused_before: bool,
    /// The TIN ever had another claim of kind business, on any account,
    /// deleted ones included.
    pub business_before: bool,
    /// Another automatic activation on this account waits for a person.
    pub another_unconfirmed: bool,
    /// Automatic activations across Buwiz in the last 24 hours.
    pub today: i64,
}

/// Why a claim was left for staff.
#[derive(Clone, Debug, PartialEq)]
pub enum Doubt {
    Off,
    Banned,
    NotPersonal,
    NotWaiting,
    SentBackBefore,
    Branch,
    NoTinId,
    SeveralTinIds,
    NotRead,
    ReadFailed(String),
    PdfText,
    /// A file of a type that is neither a PDF nor a photo checked
    /// automatically (HEIC).
    NotAPhoto,
    NotATinId,
    TinDiffers,
    NameDiffers,
    ShortName,
    LowConfidence,
    OtherAccount,
    RefusedBefore,
    PausedBefore,
    BusinessBefore,
    AnotherUnconfirmed,
    DailyLimit,
}

impl Doubt {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Banned => "banned",
            Self::NotPersonal => "not_personal",
            Self::NotWaiting => "not_waiting",
            Self::SentBackBefore => "sent_back_before",
            Self::Branch => "branch",
            Self::NoTinId => "no_tin_id",
            Self::SeveralTinIds => "several_tin_ids",
            Self::NotRead => "not_read",
            Self::ReadFailed(_) => "read_failed",
            Self::PdfText => "pdf_text",
            Self::NotAPhoto => "not_a_photo",
            Self::NotATinId => "not_a_tin_id",
            Self::TinDiffers => "tin_differs",
            Self::NameDiffers => "name_differs",
            Self::ShortName => "short_name",
            Self::LowConfidence => "low_confidence",
            Self::OtherAccount => "other_account",
            Self::RefusedBefore => "refused_before",
            Self::PausedBefore => "paused_before",
            Self::BusinessBefore => "business_before",
            Self::AnotherUnconfirmed => "another_unconfirmed",
            Self::DailyLimit => "daily_limit",
        }
    }

    /// What the reviewer reads, after "Not activated automatically: ".
    pub fn words(&self) -> String {
        match self {
            Self::Off => "automatic activation is off".to_owned(),
            Self::Banned => "the account is banned".to_owned(),
            Self::NotPersonal => "business TINs are always checked by staff".to_owned(),
            Self::NotWaiting => "the claim is not waiting for review".to_owned(),
            Self::SentBackBefore => "it was sent back before, so a person finishes it".to_owned(),
            Self::Branch => "it is a branch TIN; only head offices are checked automatically".to_owned(),
            Self::NoTinId => "there is no TIN ID on the claim".to_owned(),
            Self::SeveralTinIds => "there is more than one TIN ID on the claim".to_owned(),
            Self::NotRead => "the TIN ID has not been read yet".to_owned(),
            Self::ReadFailed(reason) => format!("the TIN ID couldn't be read: {reason}"),
            Self::PdfText => "the TIN ID is a PDF; only photos are checked automatically".to_owned(),
            Self::NotAPhoto => {
                "the TIN ID is not a JPEG, PNG or WebP photo; only those are checked automatically".to_owned()
            }
            Self::NotATinId => "the file doesn't look like a TIN ID".to_owned(),
            Self::TinDiffers => "the TIN on the ID isn't the one typed".to_owned(),
            Self::NameDiffers => "the name on the ID doesn't fully match the one typed".to_owned(),
            Self::ShortName => "the typed name has fewer than two words".to_owned(),
            Self::LowConfidence => {
                format!("OCR wasn't sure enough of the TIN or the name (it needs {:.0}%)", MIN_CONFIDENCE * 100.0)
            }
            Self::OtherAccount => "another account has or had this TIN".to_owned(),
            Self::RefusedBefore => "this account had this TIN refused, revoked or deleted, or removed it after an \
                                    automatic activation"
                .to_owned(),
            Self::PausedBefore => {
                "sending for this TIN was paused before, on some account, so an admin decides it".to_owned()
            }
            Self::BusinessBefore => {
                "this TIN was claimed as a business before, so whether it is personal is for staff to judge".to_owned()
            }
            Self::AnotherUnconfirmed => {
                "another automatic activation on this account is still waiting for a person".to_owned()
            }
            Self::DailyLimit => format!("today's limit of {DAILY_LIMIT} automatic activations was reached"),
        }
    }
}

/// What an activation rests on, kept on the claim.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Evidence {
    pub document_id: Uuid,
    pub document_sha256: String,
    pub engine: String,
    pub engine_version: Option<String>,
    pub tin_confidence: f32,
    pub name_confidence: f32,
}

/// The rule: `Ok` only when nothing is in doubt; otherwise every doubt,
/// so the reviewer sees them all.
pub fn assess(facts: &Facts) -> Result<Evidence, Vec<Doubt>> {
    let mut doubts = Vec::new();
    if facts.verification != Verification::On {
        doubts.push(Doubt::Off);
    }
    if facts.banned {
        doubts.push(Doubt::Banned);
    }
    if facts.kind != "personal" {
        doubts.push(Doubt::NotPersonal);
    }
    if facts.status != "pending" {
        doubts.push(Doubt::NotWaiting);
    }
    if facts.sent_back {
        doubts.push(Doubt::SentBackBefore);
    }
    if facts.tin.branch() != "00000" {
        doubts.push(Doubt::Branch);
    }
    if facts.other_account {
        doubts.push(Doubt::OtherAccount);
    }
    if facts.refused_before {
        doubts.push(Doubt::RefusedBefore);
    }
    if facts.paused_before {
        doubts.push(Doubt::PausedBefore);
    }
    if facts.business_before {
        doubts.push(Doubt::BusinessBefore);
    }
    if facts.another_unconfirmed {
        doubts.push(Doubt::AnotherUnconfirmed);
    }
    if facts.today >= DAILY_LIMIT {
        doubts.push(Doubt::DailyLimit);
    }
    let evidence = match facts.tin_ids.as_slice() {
        [] => {
            doubts.push(Doubt::NoTinId);
            None
        }
        [file] => read_evidence(facts, file, &mut doubts),
        _ => {
            doubts.push(Doubt::SeveralTinIds);
            None
        }
    };
    match evidence {
        Some(evidence) if doubts.is_empty() => Ok(evidence),
        _ => Err(doubts),
    }
}

/// What the one TIN ID's reading shows, or the doubts it raises.
fn read_evidence(facts: &Facts, file: &TinIdFile, doubts: &mut Vec<Doubt>) -> Option<Evidence> {
    let photo = matches!(file.content_type.as_str(), "image/jpeg" | "image/png" | "image/webp");
    if !photo && file.content_type != "application/pdf" {
        doubts.push(Doubt::NotAPhoto);
        return None;
    }
    let Some(reading) = file.reading.as_ref().filter(|reading| reading.status != "reading") else {
        doubts.push(if photo { Doubt::NotRead } else { Doubt::PdfText });
        return None;
    };
    if !photo || reading.engine.as_deref() == Some(crate::ocr::PDF_TEXT) {
        doubts.push(Doubt::PdfText);
        return None;
    }
    if reading.status != "read" {
        doubts.push(Doubt::ReadFailed(reading.error.clone().unwrap_or_else(|| "no reason given".to_owned())));
        return None;
    }
    let (Some(fields), Some(engine)) = (reading.fields.as_ref(), reading.engine.as_ref()) else {
        doubts.push(Doubt::NotRead);
        return None;
    };
    let before = doubts.len();
    if fields.kind != Kind::TinId {
        doubts.push(Doubt::NotATinId);
    }
    // Against what is typed now, not the verdict stored with the reading.
    if fields.tin.as_deref().map(|read| extract::compare_tin(&facts.tin, read)) != Some(Verdict::Match) {
        doubts.push(Doubt::TinDiffers);
    }
    if fields.name.as_deref().map(|read| extract::compare_names(&facts.registered_name, read)) != Some(Verdict::Match) {
        doubts.push(Doubt::NameDiffers);
    }
    if facts.registered_name.split_whitespace().count() < 2 {
        doubts.push(Doubt::ShortName);
    }
    let confidence = reading.confidence.clone().unwrap_or_default();
    let (tin_confidence, name_confidence) = (confidence.tin.unwrap_or(0.0), confidence.name.unwrap_or(0.0));
    if tin_confidence < MIN_CONFIDENCE || name_confidence < MIN_CONFIDENCE {
        doubts.push(Doubt::LowConfidence);
    }
    (doubts.len() == before).then(|| Evidence {
        document_id: file.document_id,
        document_sha256: crypto::hex(&file.sha256),
        engine: engine.clone(),
        engine_version: reading.engine_version.clone(),
        tin_confidence,
        name_confidence,
    })
}

/// `auto_check` for a claim left for staff: each doubt's code and words.
pub fn left_for_staff(doubts: &[Doubt]) -> Value {
    json!({
        "rule": 1,
        "outcome": "left_for_staff",
        "checked_at": OffsetDateTime::now_utc().unix_timestamp(),
        "doubts": doubts.iter().map(|doubt| json!({ "code": doubt.code(), "words": doubt.words() })).collect::<Vec<_>>(),
    })
}

/// `auto_check` for an activation: what it rests on.
pub fn activated(evidence: &Evidence) -> Value {
    json!({
        "rule": 1,
        "outcome": "activated",
        "checked_at": OffsetDateTime::now_utc().unix_timestamp(),
        "engine": evidence.engine,
        "engine_version": evidence.engine_version,
        "document_id": evidence.document_id,
        "document_sha256": evidence.document_sha256,
        "tin_match": "match",
        "name_match": "match",
        "confidence": { "tin": evidence.tin_confidence, "name": evidence.name_confidence },
    })
}

/// What the reviewer reads of a stored `auto_check`: the doubts' words
/// when the check left the claim for staff.
pub fn doubt_words(auto_check: Option<&Value>) -> Vec<String> {
    let Some(check) = auto_check.filter(|check| check["outcome"] == "left_for_staff") else {
        return Vec::new();
    };
    check["doubts"]
        .as_array()
        .map(|doubts| doubts.iter().filter_map(|doubt| doubt["words"].as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

/// Serialises every automatic activation, so the per-account and daily
/// limits are counted one reading at a time ("buwizaut" in ASCII).
const LOCK: i64 = 7_095_465_005_590_559_092;

/// Whether `document_id`'s claim can be activated automatically now, and
/// does it. Called after OCR saves the document's reading. Does nothing at
/// all while [`Verification::Off`]. Never fails the caller: problems are
/// logged.
pub async fn consider_document(state: &AppState, document_id: Uuid) {
    if state.verification != Verification::On {
        return;
    }
    if let Err(error) = consider(&state.db, state.verification, document_id).await {
        tracing::error!(%document_id, error = ?error, "automatic activation check failed");
    }
}

/// What happened to a claim the check looked at.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// The document is gone (replaced or deleted): nothing to look at.
    Gone,
    Activated(Uuid),
    LeftForStaff(Uuid, Vec<Doubt>),
}

/// [`consider_document`], answering what it did. One transaction: the
/// lock, the claim locked, the facts, the rule, the result.
pub async fn consider(db: &PgPool, verification: Verification, document_id: Uuid) -> anyhow::Result<Outcome> {
    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(LOCK).execute(&mut *tx).await?;
    // The claim, locked: a file replaced, papers resent or a decision made
    // meanwhile waits for this, or this for it.
    #[derive(sqlx::FromRow)]
    struct Claim {
        id: Uuid,
        account_id: Uuid,
        email: String,
        kind: String,
        status: String,
        sent_back: bool,
        tin_root: String,
        branch_code: String,
        registered_name: String,
        banned: bool,
    }
    let claim: Option<Claim> = sqlx::query_as(
        "SELECT c.id, c.account_id, a.email, c.kind, c.status, c.sent_back_at IS NOT NULL AS sent_back, c.tin_root,
             c.branch_code, c.registered_name, a.banned_at IS NOT NULL AS banned
         FROM claim_documents d JOIN tin_claims c ON c.id = d.claim_id JOIN accounts a ON a.id = c.account_id
         WHERE d.id = $1
         FOR UPDATE OF c FOR SHARE OF a",
    )
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(Claim {
        id: claim_id,
        account_id,
        email,
        kind,
        status,
        sent_back,
        tin_root: root,
        branch_code: branch,
        registered_name,
        banned,
    }) = claim
    else {
        return Ok(Outcome::Gone);
    };
    let tin = Tin::parse(&root, &branch)
        .map_err(|_| anyhow::anyhow!("claim {claim_id} has a stored TIN that is not valid"))?;
    #[derive(sqlx::FromRow)]
    struct File {
        id: Uuid,
        content_type: String,
        sha256: Vec<u8>,
        status: Option<String>,
        engine: Option<String>,
        engine_version: Option<String>,
        error: Option<String>,
        fields: Option<Json<Fields>>,
        confidence: Option<Json<Confidence>>,
    }
    let files: Vec<File> = sqlx::query_as(
        "SELECT d.id, d.content_type, d.sha256, r.status, r.engine, r.engine_version, r.error, r.fields, r.confidence
         FROM claim_documents d LEFT JOIN document_readings r ON r.document_id = d.id
         WHERE d.claim_id = $1 AND d.kind = 'tin_id' AND d.purged_at IS NULL
         ORDER BY d.created_at",
    )
    .bind(claim_id)
    .fetch_all(&mut *tx)
    .await?;
    let tin_ids = files
        .into_iter()
        .map(|file| TinIdFile {
            document_id: file.id,
            content_type: file.content_type,
            sha256: file.sha256,
            reading: file.status.map(|status| ReadingFacts {
                status,
                engine: file.engine,
                engine_version: file.engine_version,
                error: file.error,
                fields: file.fields.map(|fields| fields.0),
                confidence: file.confidence.map(|confidence| confidence.0),
            }),
        })
        .collect();
    // Rows, and the audit lines deleted claims leave behind. Those keep the
    // TIN masked (its last three digits and branch), so they are matched
    // on that: another TIN with the same last digits only leaves more
    // claims for staff. `$5` matches any branch of the root, `$6` this one.
    // A test TIN's lines (root 000-000-000, masked ***-***-000-…) are left
    // out by their claim, whose row stays (it was active, so only the
    // rollback of migration 13 deletes it): they would cast doubt on every
    // real TIN ending in 000.
    let any_branch = format!("***-***-{}-%", &tin.root()[6..]);
    let not_test = format!(
        "NOT EXISTS (SELECT 1 FROM tin_claims t WHERE t.id = claim_audit.claim_id AND t.tin_root = '{}')",
        crate::account::tin::TEST_ROOT
    );
    let (other_account, refused_before, sent_back_elsewhere, business_before, another_unconfirmed, today): (
        bool,
        bool,
        bool,
        bool,
        bool,
        i64,
    ) = sqlx::query_as(&format!(
        "SELECT
             EXISTS (SELECT 1 FROM tin_claims WHERE tin_root = $1 AND account_id <> $3)
               OR EXISTS (SELECT 1 FROM claim_audit WHERE account_id <> $3 AND tin_masked LIKE $5 AND {not_test}),
             EXISTS (SELECT 1 FROM tin_claims WHERE tin_root = $1 AND branch_code = $2 AND account_id = $3
                       AND (status IN ('rejected', 'revoked') OR (status = 'removed' AND verified_by = 'auto')))
               OR EXISTS (SELECT 1 FROM claim_audit WHERE account_id = $3 AND claim_id <> $4 AND tin_masked = $6
                            AND action IN ('rejected', 'revoked', 'deleted')),
             EXISTS (SELECT 1 FROM claim_audit WHERE account_id = $3 AND claim_id <> $4 AND tin_masked = $6
                       AND action = 'sent_back'),
             EXISTS (SELECT 1 FROM tin_claims WHERE tin_root = $1 AND kind = 'business' AND id <> $4)
               OR EXISTS (SELECT 1 FROM claim_audit WHERE claim_id <> $4 AND claim_kind = 'business'
                            AND tin_masked LIKE $5 AND {not_test}),
             EXISTS (SELECT 1 FROM tin_claims WHERE account_id = $3 AND id <> $4
                       AND verified_by = 'auto' AND auto_confirmed_at IS NULL),
             (SELECT count(*) FROM claim_audit WHERE action = 'auto_activated' AND created_at > now() - interval '24 hours')"
    ))
    .bind(tin.root())
    .bind(tin.branch())
    .bind(account_id)
    .bind(claim_id)
    .bind(&any_branch)
    .bind(tin.masked())
    .fetch_one(&mut *tx)
    .await?;
    let paused_before = crate::account::claims::was_paused(&mut tx, &tin).await?;
    let facts = Facts {
        verification,
        banned,
        kind,
        status: status.clone(),
        sent_back: sent_back || sent_back_elsewhere,
        tin: tin.clone(),
        registered_name,
        tin_ids,
        other_account,
        refused_before,
        paused_before,
        business_before,
        another_unconfirmed,
        today,
    };
    let doubts = match assess(&facts) {
        Ok(evidence) => {
            let updated = sqlx::query(
                "UPDATE tin_claims SET status = 'approved', decided_at = now(), decided_by = NULL,
                     verified_by = 'auto', auto_check = $2
                 WHERE id = $1 AND status = 'pending'",
            )
            .bind(claim_id)
            .bind(activated(&evidence))
            .execute(&mut *tx)
            .await;
            match updated {
                // The row is locked and was pending: exactly one row, or
                // nothing is activated.
                Ok(done) if done.rows_affected() != 1 => {
                    anyhow::bail!("activating claim {claim_id} changed {} rows", done.rows_affected());
                }
                Ok(_) => {
                    sqlx::query(
                        "INSERT INTO claim_audit
                             (claim_id, account_id, tin_masked, action, claim_status, actor_id, actor_email, reason, source,
                              claim_kind)
                         VALUES ($1, $2, $3, 'auto_activated', $4, $2, $5, $6, 'auto', 'personal')",
                    )
                    .bind(claim_id)
                    .bind(account_id)
                    .bind(tin.masked())
                    .bind(&status)
                    .bind(&email)
                    .bind(ACTIVATED_REASON)
                    .execute(&mut *tx)
                    .await?;
                    tx.commit().await?;
                    tracing::warn!(%claim_id, %account_id, "tin activated automatically; waiting for staff to confirm");
                    return Ok(Outcome::Activated(claim_id));
                }
                // Someone else became the owner meanwhile.
                Err(sqlx::Error::Database(error)) if error.constraint() == Some("tin_claims_one_owner") => {
                    drop(tx);
                    tx = db.begin().await?;
                    vec![Doubt::OtherAccount]
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(doubts) => doubts,
    };
    // Only a claim still waiting keeps what the check saw; a decided one
    // keeps what it had.
    sqlx::query("UPDATE tin_claims SET auto_check = $2 WHERE id = $1 AND status IN ('pending', 'needs_fix')")
        .bind(claim_id)
        .bind(left_for_staff(&doubts))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    tracing::info!(%claim_id, doubts = ?doubts.iter().map(Doubt::code).collect::<Vec<_>>(), "left for staff");
    Ok(Outcome::LeftForStaff(claim_id, doubts))
}

/// Staff confirm an automatic activation after looking at its papers (also
/// one the owner removed or staff revoked since): the papers are deleted,
/// and a pause Buwiz put on it for waiting too long is lifted. On one's own
/// TIN only as an Admin, with `reason` (kept on the `auto_confirmed` line).
/// Only an Admin reopens a paused TIN: Support confirms only one
/// that is not paused, and an Admin lifting Buwiz's pause gives `reason`
/// (a staff pause stays, and needs none). The actor's role is checked
/// again inside the transaction; the route checks the power first
/// ([`crate::account::roles::Power::ConfirmAuto`]).
pub async fn confirm(
    db: &PgPool,
    config: &Config,
    store: &DocumentStore,
    claim_id: Uuid,
    actor: &Account,
    reason: &str,
) -> Result<Notice, DecideError> {
    let mut tx = db.begin().await?;
    let role = roles::recheck(&mut tx, config, actor.id).await?;
    if !role.is_some_and(|role| role.can(Power::ConfirmAuto)) {
        return Err(DecideError::Refused(Notice::AdminOnly));
    }
    #[derive(sqlx::FromRow)]
    struct Row {
        account_id: Uuid,
        kind: String,
        status: String,
        tin_root: String,
        branch_code: String,
        verified_by: Option<String>,
        confirmed: bool,
        paused: bool,
        paused_by_buwiz: bool,
    }
    let row: Option<Row> = sqlx::query_as(
        "SELECT account_id, kind, status, tin_root, branch_code, verified_by, auto_confirmed_at IS NOT NULL AS confirmed,
             paused_at IS NOT NULL AS paused, paused_at IS NOT NULL AND paused_by IS NULL AS paused_by_buwiz
         FROM tin_claims WHERE id = $1 FOR UPDATE",
    )
    .bind(claim_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(Row {
        account_id: owner,
        kind,
        status,
        tin_root: root,
        branch_code: branch,
        verified_by,
        confirmed,
        paused,
        paused_by_buwiz,
    }) = row
    else {
        return Err(DecideError::Refused(Notice::ClaimMissing));
    };
    if verified_by.as_deref() != Some("auto") || confirmed {
        return Err(DecideError::Refused(Notice::NotAuto));
    }
    // Like a decision: on one's own TIN, an Admin's alone.
    if owner == actor.id && role != Some(Role::Admin) {
        return Err(DecideError::Refused(Notice::OwnClaim));
    }
    if paused && role != Some(Role::Admin) {
        return Err(DecideError::Refused(Notice::AdminMustConfirm));
    }
    // As visible as activating it without papers: the reason is on record.
    let own = if owner == actor.id {
        Some(crate::account::admin::reason_text(reason).map_err(|_| DecideError::Refused(Notice::OwnNeedsReason))?)
    } else {
        None
    };
    let resumed = if paused_by_buwiz {
        let reason = crate::account::admin::required_reason(reason)?;
        Some(format!("Confirmed after Buwiz paused it for waiting too long: {reason}"))
    } else {
        None
    };
    sqlx::query(
        "UPDATE tin_claims SET auto_confirmed_at = now(), auto_confirmed_by = $2,
             paused_at = CASE WHEN $3 THEN NULL ELSE paused_at END,
             pause_reason = CASE WHEN $3 THEN NULL ELSE pause_reason END
         WHERE id = $1",
    )
    .bind(claim_id)
    .bind(actor.id)
    .bind(paused_by_buwiz)
    .execute(&mut *tx)
    .await?;
    let objects: Vec<Option<String>> = sqlx::query_scalar(
        "UPDATE claim_documents SET ciphertext = NULL, nonce = NULL, purged_at = now()
         WHERE claim_id = $1 AND purged_at IS NULL
         RETURNING object_key",
    )
    .bind(claim_id)
    .fetch_all(&mut *tx)
    .await?;
    let masked = Tin::parse_any(&root, &branch).map_err(|_| anyhow::anyhow!("stored TIN"))?.masked();
    let mut lines = vec![("auto_confirmed", own.as_deref())];
    if let Some(reason) = resumed.as_deref() {
        lines.push(("resumed", Some(reason)));
    }
    for (action, reason) in lines {
        sqlx::query(
            "INSERT INTO claim_audit
                 (claim_id, account_id, tin_masked, action, claim_status, actor_id, actor_email, reason, source, claim_kind)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'web', $9)",
        )
        .bind(claim_id)
        .bind(owner)
        .bind(&masked)
        .bind(action)
        .bind(&status)
        .bind(actor.id)
        .bind(&actor.email)
        .bind(reason)
        .bind(&kind)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    store.delete_all(&objects.into_iter().flatten().collect::<Vec<_>>()).await;
    tracing::info!(%claim_id, actor = %actor.id, "automatic activation confirmed");
    Ok(Notice::AutoConfirmed)
}

/// Hourly: an automatic activation still active and unconfirmed after
/// [`CONFIRM_WITHIN_DAYS`] is paused by Buwiz (no `paused_by`), with
/// [`UNCONFIRMED_PAUSE`] and an audit line, its papers kept. Each is paused
/// once: one staff resumed without confirming stays resumed. Returns the
/// claims paused.
pub async fn upkeep(db: &PgPool) -> sqlx::Result<Vec<Uuid>> {
    let mut tx = db.begin().await?;
    let paused: Vec<(Uuid, Uuid, String, String, String, String)> = sqlx::query_as(&format!(
        "UPDATE tin_claims c SET paused_at = now(), paused_by = NULL, pause_reason = $1
         FROM accounts a
         WHERE a.id = c.account_id AND c.verified_by = 'auto' AND c.auto_confirmed_at IS NULL
           AND c.status = 'approved' AND c.paused_at IS NULL
           AND c.decided_at < now() - interval '{CONFIRM_WITHIN_DAYS} days'
           AND NOT EXISTS (SELECT 1 FROM claim_audit l WHERE l.claim_id = c.id AND l.action = 'paused' AND l.source = 'auto')
         RETURNING c.id, c.account_id, a.email, c.tin_root, c.branch_code, c.kind"
    ))
    .bind(UNCONFIRMED_PAUSE)
    .fetch_all(&mut *tx)
    .await?;
    for (claim_id, account_id, email, root, branch, kind) in &paused {
        let masked = Tin::parse_any(root, branch).map(|tin| tin.masked()).unwrap_or_default();
        sqlx::query(
            "INSERT INTO claim_audit
                 (claim_id, account_id, tin_masked, action, claim_status, actor_id, actor_email, reason, source,
                  claim_kind)
             VALUES ($1, $2, $3, 'paused', 'approved', $2, $4, $5, 'auto', $6)",
        )
        .bind(claim_id)
        .bind(account_id)
        .bind(masked)
        .bind(email)
        .bind(UNCONFIRMED_PAUSE)
        .bind(kind)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    for (claim_id, ..) in &paused {
        tracing::warn!(%claim_id, "automatic activation unconfirmed for {CONFIRM_WITHIN_DAYS} days: paused");
    }
    Ok(paused.into_iter().map(|(claim_id, ..)| claim_id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading() -> ReadingFacts {
        ReadingFacts {
            status: "read".into(),
            engine: Some("paddleocr".into()),
            engine_version: Some("pp-ocrv5".into()),
            error: None,
            fields: Some(Fields {
                kind: Kind::TinId,
                tin: Some("000-123-453-000".into()),
                name: Some("DELA CRUZ, JUANA SANTOS".into()),
                trade_name: None,
                birth_date: Some("1990-01-15".into()),
                rdo: None,
            }),
            confidence: Some(Confidence { tin: Some(0.97), name: Some(0.95), ..Confidence::default() }),
        }
    }

    fn file() -> TinIdFile {
        TinIdFile {
            document_id: Uuid::nil(),
            content_type: "image/png".into(),
            sha256: vec![0xab; 32],
            reading: Some(reading()),
        }
    }

    /// A clear case: everything holds.
    fn clear() -> Facts {
        Facts {
            verification: Verification::On,
            banned: false,
            kind: "personal".into(),
            status: "pending".into(),
            sent_back: false,
            tin: Tin::parse("000123453", "").unwrap(),
            registered_name: "Juana Santos Dela Cruz".into(),
            tin_ids: vec![file()],
            other_account: false,
            refused_before: false,
            paused_before: false,
            business_before: false,
            another_unconfirmed: false,
            today: 0,
        }
    }

    fn only(change: impl FnOnce(&mut Facts)) -> Vec<Doubt> {
        let mut facts = clear();
        change(&mut facts);
        assess(&facts).unwrap_err()
    }

    fn with_reading(change: impl FnOnce(&mut ReadingFacts)) -> impl FnOnce(&mut Facts) {
        move |facts| change(facts.tin_ids[0].reading.as_mut().unwrap())
    }

    #[test]
    fn a_clear_personal_tin_id_photo_activates() {
        let evidence = assess(&clear()).unwrap();
        assert_eq!(evidence.engine, "paddleocr");
        assert_eq!(evidence.document_sha256, "ab".repeat(32));
        assert_eq!((evidence.tin_confidence, evidence.name_confidence), (0.97, 0.95));
        // Exactly at the limits still passes.
        let mut facts = clear();
        facts.today = DAILY_LIMIT - 1;
        facts.tin_ids[0].reading.as_mut().unwrap().confidence =
            Some(Confidence { tin: Some(MIN_CONFIDENCE), name: Some(MIN_CONFIDENCE), ..Confidence::default() });
        assert!(assess(&facts).is_ok());
    }

    #[test]
    fn each_doubt_alone_leaves_the_claim_for_staff() {
        use Doubt::*;
        let cases: Vec<(Vec<Doubt>, Doubt)> = vec![
            (only(|facts| facts.verification = Verification::Off), Off),
            (only(|facts| facts.banned = true), Banned),
            (only(|facts| facts.kind = "business".into()), NotPersonal),
            (only(|facts| facts.status = "needs_fix".into()), NotWaiting),
            (only(|facts| facts.sent_back = true), SentBackBefore),
            (
                only(|facts| {
                    facts.tin = Tin::parse("000123453", "00001").unwrap();
                    facts.tin_ids[0].reading.as_mut().unwrap().fields.as_mut().unwrap().tin =
                        Some("000-123-453-001".into());
                }),
                Branch,
            ),
            (only(|facts| facts.tin_ids.clear()), NoTinId),
            (only(|facts| facts.tin_ids.push(file())), SeveralTinIds),
            (only(|facts| facts.tin_ids[0].reading = None), NotRead),
            (only(with_reading(|reading| reading.status = "reading".into())), NotRead),
            (
                only(with_reading(|reading| {
                    reading.status = "failed".into();
                    reading.error = Some(crate::ocr::NOT_BUILT_IN.into());
                })),
                ReadFailed(crate::ocr::NOT_BUILT_IN.into()),
            ),
            (only(with_reading(|reading| reading.engine = Some(crate::ocr::PDF_TEXT.into()))), PdfText),
            (only(with_reading(|reading| reading.fields.as_mut().unwrap().kind = Kind::Other)), NotATinId),
            (
                only(with_reading(|reading| reading.fields.as_mut().unwrap().tin = Some("000-123-454".into()))),
                TinDiffers,
            ),
            (only(with_reading(|reading| reading.fields.as_mut().unwrap().tin = None)), TinDiffers),
            (
                only(with_reading(|reading| reading.fields.as_mut().unwrap().name = Some("JUANA S DELA CRUZ".into()))),
                NameDiffers,
            ),
            (only(with_reading(|reading| reading.fields.as_mut().unwrap().name = None)), NameDiffers),
            (
                only(|facts| {
                    facts.registered_name = "Juana".into();
                    facts.tin_ids[0].reading.as_mut().unwrap().fields.as_mut().unwrap().name = Some("JUANA".into());
                }),
                ShortName,
            ),
            (
                only(with_reading(|reading| reading.confidence.as_mut().unwrap().tin = Some(MIN_CONFIDENCE - 0.01))),
                LowConfidence,
            ),
            (only(with_reading(|reading| reading.confidence.as_mut().unwrap().name = None)), LowConfidence),
            (only(|facts| facts.other_account = true), OtherAccount),
            (only(|facts| facts.refused_before = true), RefusedBefore),
            (only(|facts| facts.another_unconfirmed = true), AnotherUnconfirmed),
            (only(|facts| facts.today = DAILY_LIMIT), DailyLimit),
            (only(|facts| facts.paused_before = true), PausedBefore),
            (only(|facts| facts.business_before = true), BusinessBefore),
            (only(|facts| facts.tin_ids[0].content_type = "image/heic".into()), NotAPhoto),
        ];
        for (doubts, expected) in cases {
            assert_eq!(doubts, vec![expected.clone()], "{expected:?}");
            assert!(!expected.words().is_empty() && !expected.code().is_empty());
        }
    }

    /// A HEIC photo is not a PDF: it is named for what it is.
    #[test]
    fn a_photo_of_a_type_not_checked_is_named_as_such() {
        let doubts = only(|facts| facts.tin_ids[0].content_type = "image/heic".into());
        assert_eq!(doubts, vec![Doubt::NotAPhoto]);
        assert!(!Doubt::NotAPhoto.words().contains("PDF"), "{}", Doubt::NotAPhoto.words());
        let unread = only(|facts| {
            facts.tin_ids[0].content_type = "image/heic".into();
            facts.tin_ids[0].reading = None;
        });
        assert_eq!(unread, vec![Doubt::NotAPhoto]);
    }

    #[test]
    fn pdf_text_is_never_enough() {
        // Read from a PDF's text layer: confidence 1.0 and a perfect match
        // prove nothing about the picture.
        let doubts = only(|facts| {
            facts.tin_ids[0].content_type = "application/pdf".into();
            let reading = facts.tin_ids[0].reading.as_mut().unwrap();
            reading.engine = Some(crate::ocr::PDF_TEXT.into());
            reading.confidence = Some(Confidence { tin: Some(1.0), name: Some(1.0), ..Confidence::default() });
        });
        assert_eq!(doubts, vec![Doubt::PdfText]);
        // A PDF is refused whatever read it, even before it is read.
        assert_eq!(only(|facts| facts.tin_ids[0].content_type = "application/pdf".into()), vec![Doubt::PdfText]);
        assert_eq!(
            only(|facts| {
                facts.tin_ids[0].content_type = "application/pdf".into();
                facts.tin_ids[0].reading = None;
            }),
            vec![Doubt::PdfText]
        );
    }

    #[test]
    fn every_doubt_is_reported_at_once() {
        let doubts = only(|facts| {
            facts.kind = "business".into();
            facts.other_account = true;
            facts.tin_ids.clear();
        });
        assert_eq!(doubts, vec![Doubt::NotPersonal, Doubt::OtherAccount, Doubt::NoTinId]);
        let check = left_for_staff(&doubts);
        assert_eq!(check["outcome"], "left_for_staff");
        assert_eq!(check["doubts"][1]["code"], "other_account");
        assert_eq!(doubt_words(Some(&check))[1], "another account has or had this TIN");
        assert!(doubt_words(Some(&activated(&assess(&clear()).unwrap()))).is_empty());
    }

    #[test]
    fn only_the_exact_word_on_turns_it_on() {
        assert_eq!(Verification::parse(Some("on")), Verification::On);
        for off in [None, Some(""), Some("off"), Some("ON"), Some("On"), Some("true"), Some("1"), Some(" on")] {
            assert_eq!(Verification::parse(off), Verification::Off, "{off:?}");
        }
        assert_eq!(Verification::default(), Verification::Off);
    }
}
