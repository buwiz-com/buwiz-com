//! TIN claims: an account proves it owns a TIN. A personal TIN is proved
//! with a TIN ID (a selfie and a National ID photo make the check
//! stronger); a business TIN with Form 2303 and a government ID, plus proof
//! of authority when the 2303 is in a company's name. Staff review each
//! claim by hand. An approved claim is an active tax profile, and active
//! tax profiles are what license the Buwiz apps (`GET /api/tax-profiles`).
//!
//! Lifecycle: `pending` → `approved` | `rejected` | `needs_fix` (staff) |
//! `withdrawn` (claimant). A claim sent back (`needs_fix`) is resent with
//! new files and is `pending` again. One TIN has at most one approved
//! owner. Document bytes are dropped when a claim ends; the hash and
//! metadata stay.
//!
//! Ending: a claim that never became active can be deleted, rows, files
//! and all (by the claimant, or by staff with a reason). An approved claim
//! keeps its row for the returns sent with it: `removed` when the owner
//! gives the TIN up, `revoked` when staff end the ownership. Either way the
//! TIN can be claimed again. Each of these, and each file the claimant
//! replaces or removes on an open claim, leaves a line in `claim_audit`.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Query, State};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::accounts::Account;
use crate::crypto;
use crate::error::ApiError;
use crate::notice::Notice;
use crate::oauth::Bearer;
use crate::state::AppState;
use crate::storage::DocumentStore;
use crate::tin::Tin;

pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_SUPPORTING_FILES: usize = 5;
/// Permissions an approved TIN grants in the Buwiz BIR app.
pub const PROFILE_PERMISSIONS: &[&str] = &["profile.view", "filing.prepare", "filing.submit"];
/// What a TIN still being checked allows in the BIR app: preparing
/// returns, not sending them.
pub const OPEN_PERMISSIONS: &[&str] = &["profile.view", "filing.prepare"];
/// What other clients (Buwiz Books, AI agents) may do with a profile.
pub const VIEW_PERMISSIONS: &[&str] = &["profile.view"];

/// Whose TIN it is, which decides the documents that prove it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimKind {
    /// An employee or individual taxpayer: a TIN ID.
    Personal,
    /// Any TIN with a Form 2303: the 2303 and a government ID.
    Business,
}

impl ClaimKind {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "personal" => Some(Self::Personal),
            "business" => Some(Self::Business),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Business => "business",
        }
    }

    pub fn label(kind: &str) -> &'static str {
        if kind == "personal" { "Personal" } else { "Business" }
    }

    /// The documents a claim of this kind cannot do without.
    pub fn required(self) -> &'static [DocumentKind] {
        match self {
            Self::Personal => &[DocumentKind::TinId],
            Self::Business => &[DocumentKind::Form2303, DocumentKind::GovernmentId],
        }
    }

    /// The documents a claim of this kind may carry.
    pub fn accepts(self, document: DocumentKind) -> bool {
        use DocumentKind::*;
        match self {
            Self::Personal => matches!(document, TinId | Selfie | NationalId | Supporting),
            Self::Business => matches!(document, Form2303 | GovernmentId | Authority | Supporting),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentKind {
    Form2303,
    GovernmentId,
    Supporting,
    /// The TIN ID card or the digital TIN ID from ORUS.
    TinId,
    Selfie,
    /// A photo of the National ID (PhilSys). Its QR signature is not
    /// checked yet.
    NationalId,
    /// Secretary's certificate, board resolution or SPA, for a 2303 in a
    /// company's name.
    Authority,
}

impl DocumentKind {
    pub const ALL: [Self; 7] = [
        Self::TinId,
        Self::Selfie,
        Self::NationalId,
        Self::Form2303,
        Self::GovernmentId,
        Self::Authority,
        Self::Supporting,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Form2303 => "form_2303",
            Self::GovernmentId => "government_id",
            Self::Supporting => "supporting",
            Self::TinId => "tin_id",
            Self::Selfie => "selfie",
            Self::NationalId => "national_id",
            Self::Authority => "authority",
        }
    }

    /// The upload field with this name, which is also the stored kind.
    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == raw)
    }

    pub fn label(kind: &str) -> &'static str {
        match kind {
            "form_2303" => "Form 2303",
            "government_id" => "Government ID",
            "tin_id" => "TIN ID",
            "selfie" => "Selfie",
            "national_id" => "National ID",
            "authority" => "Proof of authority",
            _ => "Other papers",
        }
    }

    pub fn icon(kind: &str) -> &'static str {
        match kind {
            "form_2303" => "description",
            "government_id" | "tin_id" => "badge",
            "selfie" => "face",
            "national_id" => "id_card",
            "authority" => "gavel",
            _ => "attach_file",
        }
    }
}

/// Why staff send a claim back: (code, chip label, what the claimant reads).
pub const FIX_REASONS: &[(&str, &str, &str)] = &[
    ("blurry", "Blurry photo", "A photo is too blurry to read."),
    ("name_mismatch", "Name doesn't match", "The name you typed doesn't match the document."),
    ("wrong_document", "Wrong document", "A file isn't the document we asked for."),
    ("tin_mismatch", "TIN doesn't match", "The TIN you typed doesn't match the document."),
    (
        "authority_missing",
        "Proof of authority missing",
        "The Form 2303 is in a company's name: add proof you can act for it.",
    ),
];

pub fn fix_reason(code: &str) -> Option<(&'static str, &'static str)> {
    FIX_REASONS.iter().find(|(known, ..)| *known == code).map(|(_, label, message)| (*label, *message))
}

/// A file as uploaded, before it is stored.
pub struct Upload {
    pub kind: DocumentKind,
    pub filename: String,
    pub bytes: Vec<u8>,
}

/// The media type, judged from the first bytes rather than the browser's
/// claim. Only scans and photos are accepted.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"%PDF-") {
        Some("application/pdf")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && matches!(&bytes[8..12], b"heic" | b"heix" | b"mif1") {
        Some("image/heic")
    } else {
        None
    }
}

/// A file name safe to show and to put in `Content-Disposition`.
fn clean_filename(raw: &str) -> String {
    let name: String = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ' '))
        .take(100)
        .collect();
    let name = name.trim();
    if name.is_empty() { "document".to_owned() } else { name.to_owned() }
}

/// A new claim as the person sent it.
pub struct NewClaim {
    pub kind: ClaimKind,
    pub tin: Tin,
    pub registered_name: String,
    pub trade_name: String,
    pub uploads: Vec<Upload>,
}

/// Trimmed registered and trade names, or why they cannot be used.
fn check_names(registered_name: &str, trade_name: &str) -> Result<(String, Option<String>), Notice> {
    let registered_name = registered_name.trim();
    let trade_name = trade_name.trim();
    if registered_name.is_empty() || registered_name.len() > 200 || trade_name.len() > 200 {
        return Err(Notice::NameMissing);
    }
    Ok((registered_name.to_owned(), Some(trade_name.to_owned()).filter(|name| !name.is_empty())))
}

/// Checks each file against the claim's kind, its size and its real type.
fn check_uploads(kind: ClaimKind, uploads: Vec<Upload>) -> Result<Vec<(Upload, &'static str)>, Notice> {
    if uploads.iter().any(|upload| !kind.accepts(upload.kind)) {
        return Err(Notice::WrongDocuments);
    }
    if uploads.iter().filter(|upload| upload.kind == DocumentKind::Supporting).count() > MAX_SUPPORTING_FILES {
        return Err(Notice::TooManySupporting);
    }
    let mut checked = Vec::with_capacity(uploads.len());
    for upload in uploads {
        if upload.bytes.len() > MAX_FILE_BYTES {
            return Err(Notice::FileTooBig);
        }
        let Some(content_type) = sniff(&upload.bytes) else {
            return Err(Notice::FileType);
        };
        checked.push((upload, content_type));
    }
    Ok(checked)
}

fn missing_notice(document: DocumentKind) -> Notice {
    match document {
        DocumentKind::TinId => Notice::NeedTinId,
        DocumentKind::Form2303 => Notice::Need2303,
        _ => Notice::NeedId,
    }
}

/// Opens a claim with its files. Errors are messages for the person.
pub async fn submit(
    db: &PgPool,
    store: &DocumentStore,
    key: &[u8; 32],
    account_id: Uuid,
    claim: NewClaim,
) -> Result<Uuid, SubmitError> {
    let NewClaim { kind, tin, registered_name, trade_name, uploads } = claim;
    let (registered_name, trade_name) = check_names(&registered_name, &trade_name).map_err(SubmitError::Invalid)?;
    for required in kind.required() {
        if !uploads.iter().any(|upload| upload.kind == *required) {
            return Err(SubmitError::Invalid(missing_notice(*required)));
        }
    }
    let checked = check_uploads(kind, uploads).map_err(SubmitError::Invalid)?;

    let mut tx = db.begin().await?;
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT status FROM tin_claims
         WHERE account_id = $1 AND tin_root = $2 AND branch_code = $3 AND status IN ('pending', 'needs_fix', 'approved')",
    )
    .bind(account_id)
    .bind(tin.root())
    .bind(tin.branch())
    .fetch_optional(&mut *tx)
    .await?;
    match existing.as_deref() {
        Some("approved") => return Err(SubmitError::Invalid(Notice::TinAlreadyActive)),
        Some(_) => return Err(SubmitError::Invalid(Notice::ClaimAlreadyPending)),
        None => {}
    }
    let inserted = sqlx::query_scalar(
        "INSERT INTO tin_claims (account_id, kind, tin_root, branch_code, registered_name, trade_name, status)
         VALUES ($1, $2, $3, $4, $5, $6, 'pending') RETURNING id",
    )
    .bind(account_id)
    .bind(kind.as_str())
    .bind(tin.root())
    .bind(tin.branch())
    .bind(&registered_name)
    .bind(&trade_name)
    .fetch_one(&mut *tx)
    .await;
    let claim_id: Uuid = match inserted {
        // The same claim sent twice at once: the second one loses the race.
        Err(sqlx::Error::Database(error)) if error.constraint() == Some("tin_claims_one_open") => {
            return Err(SubmitError::Invalid(Notice::ClaimAlreadyPending));
        }
        other => other?,
    };
    let mut uploaded = Vec::new();
    let stored = store_documents(&mut tx, store, key, claim_id, checked, &mut uploaded).await;
    let committed = match stored {
        Ok(_) => tx.commit().await.map_err(SubmitError::from),
        Err(error) => Err(error),
    };
    if let Err(error) = committed {
        // The rows rolled back; drop the objects that made it to storage.
        store.delete_all(&uploaded).await;
        return Err(error);
    }
    tracing::info!(%claim_id, %account_id, kind = kind.as_str(), "tin claim submitted");
    Ok(claim_id)
}

/// What the claimant changes on a claim sent back to them.
pub struct Resend {
    pub registered_name: String,
    pub trade_name: String,
    /// New files. Each replaces the files of its kind; other papers are
    /// added.
    pub uploads: Vec<Upload>,
}

/// The claimant fixes a claim staff sent back, and it waits for review
/// again. Replaced files are deleted like those of a decided claim.
pub async fn resend(
    db: &PgPool,
    store: &DocumentStore,
    key: &[u8; 32],
    account_id: Uuid,
    claim_id: Uuid,
    fix: Resend,
) -> Result<(), SubmitError> {
    let mut tx = db.begin().await?;
    // Files replaced on the claim's files page since it was sent back count
    // as a change.
    let row: Option<(String, String, String, Option<String>, bool)> = sqlx::query_as(
        "SELECT status, kind, registered_name, trade_name, coalesce(files_changed_at > sent_back_at, false)
         FROM tin_claims WHERE id = $1 AND account_id = $2 FOR UPDATE",
    )
    .bind(claim_id)
    .bind(account_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((status, kind, old_name, old_trade, files_changed)) = row else {
        return Err(SubmitError::Invalid(Notice::ClaimMissing));
    };
    if status != "needs_fix" {
        return Err(SubmitError::Invalid(Notice::NotSentBack));
    }
    let kind = ClaimKind::parse(&kind).expect("stored kinds are valid");
    let (registered_name, trade_name) =
        check_names(&fix.registered_name, &fix.trade_name).map_err(SubmitError::Invalid)?;
    if fix.uploads.is_empty() && !files_changed && registered_name == old_name && trade_name == old_trade {
        return Err(SubmitError::Invalid(Notice::FixNeedsChange));
    }
    let checked = check_uploads(kind, fix.uploads).map_err(SubmitError::Invalid)?;

    let mut replaced: Vec<&str> = checked
        .iter()
        .map(|(upload, _)| upload.kind)
        .filter(|kind| *kind != DocumentKind::Supporting)
        .map(DocumentKind::as_str)
        .collect();
    replaced.sort_unstable();
    replaced.dedup();
    let purged: Vec<Option<String>> = sqlx::query_scalar(
        "UPDATE claim_documents SET ciphertext = NULL, nonce = NULL, purged_at = now()
         WHERE claim_id = $1 AND kind = ANY($2) AND purged_at IS NULL
         RETURNING object_key",
    )
    .bind(claim_id)
    .bind(&replaced)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE tin_claims SET status = 'pending', resent_at = now(), registered_name = $2, trade_name = $3
         WHERE id = $1",
    )
    .bind(claim_id)
    .bind(&registered_name)
    .bind(&trade_name)
    .execute(&mut *tx)
    .await?;
    let mut uploaded = Vec::new();
    let stored = store_documents(&mut tx, store, key, claim_id, checked, &mut uploaded).await;
    let committed = match stored {
        Ok(_) => tx.commit().await.map_err(SubmitError::from),
        Err(error) => Err(error),
    };
    if let Err(error) = committed {
        store.delete_all(&uploaded).await;
        return Err(error);
    }
    store.delete_all(&purged.into_iter().flatten().collect::<Vec<_>>()).await;
    tracing::info!(%claim_id, %account_id, "tin claim resent");
    Ok(())
}

async fn store_documents(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    store: &DocumentStore,
    key: &[u8; 32],
    claim_id: Uuid,
    checked: Vec<(Upload, &'static str)>,
    uploaded: &mut Vec<String>,
) -> Result<Vec<Uuid>, SubmitError> {
    let mut ids = Vec::with_capacity(checked.len());
    for (upload, content_type) in checked {
        let document_id = Uuid::new_v4();
        ids.push(document_id);
        let (nonce, ciphertext) = crypto::seal(key, &document_context(claim_id, document_id), &upload.bytes)?;
        let (in_row, object_key) = match store {
            DocumentStore::Database => (Some(ciphertext), None),
            DocumentStore::Objects(objects) => {
                let object_key = format!("claims/{claim_id}/{document_id}");
                objects.put(&object_key, ciphertext).await?;
                uploaded.push(object_key.clone());
                (None, Some(object_key))
            }
        };
        sqlx::query(
            "INSERT INTO claim_documents
                 (id, claim_id, kind, filename, content_type, size_bytes, sha256, nonce, ciphertext, object_key)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(document_id)
        .bind(claim_id)
        .bind(upload.kind.as_str())
        .bind(clean_filename(&upload.filename))
        .bind(content_type)
        .bind(i32::try_from(upload.bytes.len()).unwrap_or(i32::MAX))
        .bind(crypto::sha256(&upload.bytes))
        .bind(nonce)
        .bind(in_row)
        .bind(object_key)
        .execute(&mut **tx)
        .await?;
    }
    Ok(ids)
}

#[derive(Debug, thiserror::Error)]
pub enum SubmitError {
    #[error("{0:?}")]
    Invalid(Notice),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for SubmitError {
    fn from(error: sqlx::Error) -> Self {
        Self::Internal(error.into())
    }
}

fn document_context(claim_id: Uuid, document_id: Uuid) -> Vec<u8> {
    format!("{claim_id}/{document_id}").into_bytes()
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct ClaimRow {
    pub id: Uuid,
    pub account_id: Uuid,
    pub email: String,
    pub kind: String,
    pub tin_root: String,
    pub branch_code: String,
    pub registered_name: String,
    pub trade_name: Option<String>,
    pub status: String,
    pub decision_note: Option<String>,
    pub fix_reasons: Vec<String>,
    pub fix_note: Option<String>,
    pub created_at: OffsetDateTime,
    pub sent_back_at: Option<OffsetDateTime>,
    pub resent_at: Option<OffsetDateTime>,
    pub decided_at: Option<OffsetDateTime>,
    pub removed_at: Option<OffsetDateTime>,
    pub removal_note: Option<String>,
    pub files_changed_at: Option<OffsetDateTime>,
}

impl ClaimRow {
    pub fn tin(&self) -> Tin {
        Tin::parse(&self.tin_root, &self.branch_code).expect("stored TINs are valid")
    }

    /// Waiting for staff or for the claimant's fix.
    pub fn is_open(&self) -> bool {
        matches!(self.status.as_str(), "pending" | "needs_fix")
    }

    /// When it last went into the review queue.
    pub fn waiting_since(&self) -> OffsetDateTime {
        self.resent_at.unwrap_or(self.created_at)
    }

    /// When the claimant changed files on a claim already waiting for
    /// review, after it went into the queue.
    pub fn files_changed_while_waiting(&self) -> Option<OffsetDateTime> {
        self.files_changed_at.filter(|at| self.status == "pending" && *at > self.waiting_since())
    }
}

pub const CLAIM_COLUMNS: &str = "c.id, c.account_id, a.email, c.kind, c.tin_root, c.branch_code, c.registered_name, \
     c.trade_name, c.status, c.decision_note, c.fix_reasons, c.fix_note, c.created_at, c.sent_back_at, c.resent_at, \
     c.decided_at, c.removed_at, c.removal_note, c.files_changed_at";

pub async fn for_account(db: &PgPool, account_id: Uuid) -> sqlx::Result<Vec<ClaimRow>> {
    sqlx::query_as(&format!(
        "SELECT {CLAIM_COLUMNS} FROM tin_claims c JOIN accounts a ON a.id = c.account_id
         WHERE c.account_id = $1 ORDER BY c.created_at DESC"
    ))
    .bind(account_id)
    .fetch_all(db)
    .await
}

pub async fn find(db: &PgPool, claim_id: Uuid) -> sqlx::Result<Option<ClaimRow>> {
    sqlx::query_as(&format!(
        "SELECT {CLAIM_COLUMNS} FROM tin_claims c JOIN accounts a ON a.id = c.account_id WHERE c.id = $1"
    ))
    .bind(claim_id)
    .fetch_optional(db)
    .await
}

/// Other open or approved claims on the same TIN: the reviewer must see a
/// dispute.
pub async fn rivals(db: &PgPool, claim: &ClaimRow) -> sqlx::Result<Vec<ClaimRow>> {
    sqlx::query_as(&format!(
        "SELECT {CLAIM_COLUMNS} FROM tin_claims c JOIN accounts a ON a.id = c.account_id
         WHERE c.tin_root = $1 AND c.branch_code = $2 AND c.id <> $3
           AND c.status IN ('pending', 'needs_fix', 'approved')
         ORDER BY c.created_at"
    ))
    .bind(&claim.tin_root)
    .bind(&claim.branch_code)
    .bind(claim.id)
    .fetch_all(db)
    .await
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct DocumentRow {
    pub id: Uuid,
    pub claim_id: Uuid,
    pub kind: String,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i32,
    pub purged_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
}

impl DocumentRow {
    /// Still stored, and a photo a browser can show in `<img>`.
    pub fn is_viewable_image(&self) -> bool {
        self.purged_at.is_none() && matches!(self.content_type.as_str(), "image/jpeg" | "image/png" | "image/webp")
    }
}

const DOCUMENT_COLUMNS: &str =
    "d.id, d.claim_id, d.kind, d.filename, d.content_type, d.size_bytes, d.purged_at, d.created_at";

/// A claim's files in the order a reviewer reads them, required ones first.
pub async fn documents(db: &PgPool, claim_id: Uuid) -> sqlx::Result<Vec<DocumentRow>> {
    let mut rows: Vec<DocumentRow> = sqlx::query_as(&format!(
        "SELECT {DOCUMENT_COLUMNS} FROM claim_documents d WHERE d.claim_id = $1 ORDER BY d.created_at"
    ))
    .bind(claim_id)
    .fetch_all(db)
    .await?;
    rows.sort_by_key(|row| DocumentKind::ALL.iter().position(|kind| kind.as_str() == row.kind));
    Ok(rows)
}

/// Every file sent with the account's claims, newest first.
pub async fn account_documents(db: &PgPool, account_id: Uuid) -> sqlx::Result<Vec<DocumentRow>> {
    sqlx::query_as(&format!(
        "SELECT {DOCUMENT_COLUMNS} FROM claim_documents d JOIN tin_claims c ON c.id = d.claim_id
         WHERE c.account_id = $1 ORDER BY d.created_at DESC"
    ))
    .bind(account_id)
    .fetch_all(db)
    .await
}

pub struct OpenedDocument {
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// Decrypts one stored document, if its bytes still exist.
pub async fn open_document(
    db: &PgPool,
    store: &DocumentStore,
    key: &[u8; 32],
    document_id: Uuid,
) -> anyhow::Result<Option<OpenedDocument>> {
    #[derive(sqlx::FromRow)]
    struct Stored {
        claim_id: Uuid,
        filename: String,
        content_type: String,
        nonce: Option<Vec<u8>>,
        ciphertext: Option<Vec<u8>>,
        object_key: Option<String>,
    }
    let row: Option<Stored> = sqlx::query_as(
        "SELECT claim_id, filename, content_type, nonce, ciphertext, object_key FROM claim_documents WHERE id = $1",
    )
    .bind(document_id)
    .fetch_optional(db)
    .await?;
    let Some(Stored { claim_id, filename, content_type, nonce: Some(nonce), ciphertext, object_key }) = row else {
        return Ok(None);
    };
    let ciphertext = match (ciphertext, object_key, store) {
        (Some(ciphertext), _, _) => ciphertext,
        (None, Some(object_key), DocumentStore::Objects(objects)) => match objects.get(&object_key).await? {
            Some(ciphertext) => ciphertext,
            None => return Ok(None),
        },
        _ => return Ok(None),
    };
    let bytes = crypto::open(key, &document_context(claim_id, document_id), &nonce, &ciphertext)?;
    Ok(Some(OpenedDocument { filename, content_type, bytes }))
}

#[derive(Debug, thiserror::Error)]
pub enum DecideError {
    #[error("{0:?}")]
    Refused(Notice),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for DecideError {
    fn from(error: sqlx::Error) -> Self {
        Self::Internal(error.into())
    }
}

/// What staff decide about a claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The account becomes the TIN's only owner.
    Approve,
    Reject,
    /// Back to the claimant with reason codes from [`FIX_REASONS`].
    SendBack(Vec<String>),
}

/// Staff approve, reject or send back a claim. Approving and rejecting end
/// it (also a claim waiting for a fix that never came); only a claim
/// waiting for review can be sent back.
pub async fn decide(
    db: &PgPool,
    store: &DocumentStore,
    claim_id: Uuid,
    reviewer_id: Uuid,
    decision: &Decision,
    note: &str,
) -> Result<Notice, DecideError> {
    let mut tx = db.begin().await?;
    let row: Option<(Uuid, String)> =
        sqlx::query_as("SELECT account_id, status FROM tin_claims WHERE id = $1 FOR UPDATE")
            .bind(claim_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((claimant, status)) = row else {
        return Err(DecideError::Refused(Notice::ClaimMissing));
    };
    if claimant == reviewer_id {
        return Err(DecideError::Refused(Notice::OwnClaim));
    }
    let note = note.trim();
    if let Decision::SendBack(reasons) = decision {
        if status != "pending" {
            return Err(DecideError::Refused(Notice::AlreadyDecided));
        }
        if reasons.iter().any(|reason| fix_reason(reason).is_none()) {
            return Err(DecideError::Refused(Notice::FixNeedsReason));
        }
        if reasons.is_empty() && note.is_empty() {
            return Err(DecideError::Refused(Notice::FixNeedsReason));
        }
        sqlx::query(
            "UPDATE tin_claims SET status = 'needs_fix', fix_reasons = $2, fix_note = NULLIF($3, ''),
                 sent_back_at = now(), sent_back_by = $4
             WHERE id = $1",
        )
        .bind(claim_id)
        .bind(reasons)
        .bind(note)
        .bind(reviewer_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        tracing::info!(%claim_id, %reviewer_id, "tin claim sent back");
        return Ok(Notice::ClaimSentBack);
    }
    if !matches!(status.as_str(), "pending" | "needs_fix") {
        return Err(DecideError::Refused(Notice::AlreadyDecided));
    }
    let approve = *decision == Decision::Approve;
    if !approve && note.is_empty() {
        return Err(DecideError::Refused(Notice::RejectNeedsNote));
    }
    let status = if approve { "approved" } else { "rejected" };
    let updated = sqlx::query(
        "UPDATE tin_claims SET status = $2, decided_at = now(), decided_by = $3, decision_note = NULLIF($4, '')
         WHERE id = $1",
    )
    .bind(claim_id)
    .bind(status)
    .bind(reviewer_id)
    .bind(note)
    .execute(&mut *tx)
    .await;
    match updated {
        Err(sqlx::Error::Database(error)) if error.constraint() == Some("tin_claims_one_owner") => {
            return Err(DecideError::Refused(Notice::TinOwnedElsewhere));
        }
        other => {
            other?;
        }
    }
    let objects = purge_documents(&mut tx, claim_id).await?;
    tx.commit().await?;
    store.delete_all(&objects).await;
    tracing::info!(%claim_id, %reviewer_id, status, "tin claim decided");
    Ok(Notice::ClaimDecided)
}

/// The claimant gives up a claim that is still open.
pub async fn withdraw(db: &PgPool, store: &DocumentStore, claim_id: Uuid, account_id: Uuid) -> sqlx::Result<bool> {
    let mut tx = db.begin().await?;
    let updated = sqlx::query(
        "UPDATE tin_claims SET status = 'withdrawn', decided_at = now()
         WHERE id = $1 AND account_id = $2 AND status IN ('pending', 'needs_fix')",
    )
    .bind(claim_id)
    .bind(account_id)
    .execute(&mut *tx)
    .await?;
    let objects = if updated.rows_affected() == 1 { purge_documents(&mut tx, claim_id).await? } else { Vec::new() };
    tx.commit().await?;
    store.delete_all(&objects).await;
    Ok(updated.rows_affected() == 1)
}

/// Marks a claim's documents deleted and returns the object keys to remove
/// once the transaction commits.
async fn purge_documents(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, claim_id: Uuid) -> sqlx::Result<Vec<String>> {
    let keys: Vec<Option<String>> = sqlx::query_scalar(
        "UPDATE claim_documents SET ciphertext = NULL, nonce = NULL, purged_at = now()
         WHERE claim_id = $1 AND purged_at IS NULL
         RETURNING object_key",
    )
    .bind(claim_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(keys.into_iter().flatten().collect())
}

/// Who ends a claim: its owner, or staff, who must say why.
pub enum By<'a> {
    Owner,
    Staff { reason: &'a str },
}

/// Statuses of a claim that never became an active TIN, so nothing refers
/// to it and it can be deleted outright.
pub const DELETABLE: &[&str] = &["pending", "needs_fix", "rejected", "withdrawn"];

/// A claim locked for a change, with what its audit line needs.
struct Locked {
    id: Uuid,
    account_id: Uuid,
    kind: String,
    status: String,
    tin_masked: String,
}

async fn lock_claim(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, claim_id: Uuid) -> sqlx::Result<Option<Locked>> {
    let row: Option<(Uuid, String, String, String, String)> = sqlx::query_as(
        "SELECT account_id, kind, status, tin_root, branch_code FROM tin_claims WHERE id = $1 FOR UPDATE",
    )
    .bind(claim_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(account_id, kind, status, root, branch)| Locked {
        id: claim_id,
        account_id,
        kind,
        status,
        tin_masked: Tin::parse(&root, &branch).expect("stored TINs are valid").masked(),
    }))
}

/// The owner acts on their own claim; staff on someone else's, with a
/// reason. Returns the trimmed reason.
fn check_actor(claim: &Locked, actor: &Account, by: &By<'_>) -> Result<Option<String>, Notice> {
    match by {
        By::Owner if claim.account_id != actor.id => Err(Notice::ClaimMissing),
        By::Owner => Ok(None),
        By::Staff { .. } if claim.account_id == actor.id => Err(Notice::OwnClaim),
        By::Staff { reason } => {
            let reason = reason.trim();
            if reason.is_empty() { Err(Notice::ReasonMissing) } else { Ok(Some(reason.to_owned())) }
        }
    }
}

async fn audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: &Locked,
    actor: &Account,
    action: &str,
    document_kind: Option<&str>,
    reason: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO claim_audit
             (claim_id, account_id, tin_masked, action, claim_status, document_kind, actor_id, actor_email, reason)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(claim.id)
    .bind(claim.account_id)
    .bind(&claim.tin_masked)
    .bind(action)
    .bind(&claim.status)
    .bind(document_kind)
    .bind(actor.id)
    .bind(&actor.email)
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Deletes a claim that never became an active TIN: its row, its document
/// rows and their stored objects. Only the audit line is left.
pub async fn delete(
    db: &PgPool,
    store: &DocumentStore,
    claim_id: Uuid,
    actor: &Account,
    by: By<'_>,
) -> Result<Notice, DecideError> {
    let mut tx = db.begin().await?;
    let Some(claim) = lock_claim(&mut tx, claim_id).await? else {
        return Err(DecideError::Refused(Notice::ClaimMissing));
    };
    let reason = check_actor(&claim, actor, &by).map_err(DecideError::Refused)?;
    if !DELETABLE.contains(&claim.status.as_str()) {
        return Err(DecideError::Refused(Notice::ClaimNotDeletable));
    }
    let objects: Vec<String> = sqlx::query_scalar(
        "SELECT object_key FROM claim_documents WHERE claim_id = $1 AND purged_at IS NULL AND object_key IS NOT NULL",
    )
    .bind(claim_id)
    .fetch_all(&mut *tx)
    .await?;
    // The document rows go with it (ON DELETE CASCADE).
    sqlx::query("DELETE FROM tin_claims WHERE id = $1").bind(claim_id).execute(&mut *tx).await?;
    audit(&mut tx, &claim, actor, "deleted", None, reason.as_deref()).await?;
    tx.commit().await?;
    store.delete_all(&objects).await;
    tracing::info!(%claim_id, actor = %actor.id, "tin claim deleted");
    Ok(Notice::ClaimDeleted)
}

/// Ends the ownership of an active TIN: `removed` by its owner, `revoked`
/// by staff. The row stays for the returns sent with it. Tax profiles,
/// tickets and the relay only take `approved` claims, and the one-owner
/// index only counts those, so the TIN is inactive at once and free to be
/// claimed again.
pub async fn end_ownership(db: &PgPool, claim_id: Uuid, actor: &Account, by: By<'_>) -> Result<Notice, DecideError> {
    let mut tx = db.begin().await?;
    let Some(claim) = lock_claim(&mut tx, claim_id).await? else {
        return Err(DecideError::Refused(Notice::ClaimMissing));
    };
    let reason = check_actor(&claim, actor, &by).map_err(DecideError::Refused)?;
    if claim.status != "approved" {
        return Err(DecideError::Refused(Notice::TinNotActive));
    }
    let (status, notice) = match by {
        By::Owner => ("removed", Notice::TinRemoved),
        By::Staff { .. } => ("revoked", Notice::OwnershipRevoked),
    };
    sqlx::query(
        "UPDATE tin_claims SET status = $2, removed_at = now(), removed_by = $3, removal_note = $4 WHERE id = $1",
    )
    .bind(claim_id)
    .bind(status)
    .bind(actor.id)
    .bind(&reason)
    .execute(&mut *tx)
    .await?;
    audit(&mut tx, &claim, actor, status, None, reason.as_deref()).await?;
    tx.commit().await?;
    tracing::info!(%claim_id, actor = %actor.id, status, "tin ownership ended");
    Ok(notice)
}

/// One stored file of the claimant's own open claim, locked for a change.
struct OpenFile {
    claim: Locked,
    kind: String,
    object_key: Option<String>,
}

async fn lock_open_file(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: &Account,
    claim_id: Uuid,
    document_id: Uuid,
) -> Result<OpenFile, SubmitError> {
    let claim = lock_claim(tx, claim_id)
        .await?
        .filter(|claim| claim.account_id == actor.id)
        .ok_or(SubmitError::Invalid(Notice::ClaimMissing))?;
    if !matches!(claim.status.as_str(), "pending" | "needs_fix") {
        return Err(SubmitError::Invalid(Notice::FilesLocked));
    }
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT kind, object_key FROM claim_documents WHERE id = $1 AND claim_id = $2 AND purged_at IS NULL",
    )
    .bind(document_id)
    .bind(claim_id)
    .fetch_optional(&mut **tx)
    .await?;
    let (kind, object_key) = row.ok_or(SubmitError::Invalid(Notice::DocumentMissing))?;
    Ok(OpenFile { claim, kind, object_key })
}

/// Notes the change for staff and in the audit.
async fn record_file_change(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    file: &OpenFile,
    actor: &Account,
    action: &str,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE tin_claims SET files_changed_at = now() WHERE id = $1")
        .bind(file.claim.id)
        .execute(&mut **tx)
        .await?;
    audit(tx, &file.claim, actor, action, Some(&file.kind), None).await
}

/// The claimant swaps one file of an open claim for a new one of the same
/// kind, checked like the files of a new claim. The old file is deleted at
/// once, row and object. A claim sent back still goes through
/// [`resend`]; the change counts as a fix there. Returns the new file's id,
/// for OCR to read it ([`crate::ocr::queue_document`]).
pub async fn replace_document(
    db: &PgPool,
    store: &DocumentStore,
    key: &[u8; 32],
    actor: &Account,
    claim_id: Uuid,
    document_id: Uuid,
    upload: Upload,
) -> Result<Uuid, SubmitError> {
    let mut tx = db.begin().await?;
    let file = lock_open_file(&mut tx, actor, claim_id, document_id).await?;
    if upload.kind.as_str() != file.kind {
        return Err(SubmitError::Invalid(Notice::WrongDocuments));
    }
    let kind = ClaimKind::parse(&file.claim.kind).expect("stored kinds are valid");
    let checked = check_uploads(kind, vec![upload]).map_err(SubmitError::Invalid)?;
    sqlx::query("DELETE FROM claim_documents WHERE id = $1").bind(document_id).execute(&mut *tx).await?;
    let mut uploaded = Vec::new();
    let committed = async {
        let ids = store_documents(&mut tx, store, key, claim_id, checked, &mut uploaded).await?;
        record_file_change(&mut tx, &file, actor, "file_replaced").await?;
        tx.commit().await?;
        Ok::<_, SubmitError>(ids[0])
    }
    .await;
    let new_id = match committed {
        Ok(new_id) => new_id,
        Err(error) => {
            store.delete_all(&uploaded).await;
            return Err(error);
        }
    };
    store.delete_all(&file.object_key.into_iter().collect::<Vec<_>>()).await;
    tracing::info!(%claim_id, %document_id, %new_id, "claim document replaced");
    Ok(new_id)
}

/// The claimant drops an optional file (selfie, National ID, proof of
/// authority, other papers) from an open claim. Required ones can only be
/// replaced.
pub async fn remove_document(
    db: &PgPool,
    store: &DocumentStore,
    actor: &Account,
    claim_id: Uuid,
    document_id: Uuid,
) -> Result<(), SubmitError> {
    let mut tx = db.begin().await?;
    let file = lock_open_file(&mut tx, actor, claim_id, document_id).await?;
    let kind = ClaimKind::parse(&file.claim.kind).expect("stored kinds are valid");
    if kind.required().iter().any(|required| required.as_str() == file.kind) {
        return Err(SubmitError::Invalid(Notice::FileRequired));
    }
    sqlx::query("DELETE FROM claim_documents WHERE id = $1").bind(document_id).execute(&mut *tx).await?;
    record_file_change(&mut tx, &file, actor, "file_removed").await?;
    tx.commit().await?;
    store.delete_all(&file.object_key.into_iter().collect::<Vec<_>>()).await;
    tracing::info!(%claim_id, %document_id, "claim document removed");
    Ok(())
}

/// `GET /api/tax-profiles`: the account's active TINs. The Buwiz apps treat
/// any profile with permissions as a license to use their BIR tools. Only
/// the BIR app (scope `app`) sees the full TIN; everyone else gets the
/// opaque `id` and the masked TIN.
///
/// `?include=pending` also lists TINs still being checked or sent back for
/// a fix, and adds `status` to every profile. Those can prepare returns
/// but not send them; their `id` stays the same once approved.
pub async fn tax_profiles(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
    bearer: Bearer,
) -> Result<Json<Value>, ApiError> {
    let full = bearer.scope == crate::oauth::SCOPE_APP;
    let include_open =
        query.get("include").is_some_and(|include| include.split(',').any(|item| item.trim() == "pending"));
    let profiles = if include_open {
        profiles_with_open(&state.db, bearer.account.id, full).await?
    } else {
        active_profiles(&state.db, bearer.account.id, full).await?
    };
    Ok(Json(json!({ "profiles": profiles })))
}

/// The account's active TINs. `full` adds the raw TIN and filing
/// permissions, for the Buwiz BIR app only.
pub async fn active_profiles(db: &PgPool, account_id: Uuid, full: bool) -> sqlx::Result<Vec<Value>> {
    let rows: Vec<(Uuid, String, String, String, OffsetDateTime)> = sqlx::query_as(
        "SELECT id, tin_root, branch_code, registered_name, decided_at FROM tin_claims
         WHERE account_id = $1 AND status = 'approved' ORDER BY decided_at",
    )
    .bind(account_id)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, root, branch, registered_name, activated_at)| {
            let tin = Tin::parse(&root, &branch).expect("stored TINs are valid");
            profile_json(id, &tin, &registered_name, activated_at, full)
        })
        .collect())
}

/// Active TINs, then open claims, each with its `status`.
async fn profiles_with_open(db: &PgPool, account_id: Uuid, full: bool) -> sqlx::Result<Vec<Value>> {
    let mut profiles = active_profiles(db, account_id, full).await?;
    for profile in &mut profiles {
        profile["status"] = json!("approved");
    }
    let open: Vec<(Uuid, String, String, String, String)> = sqlx::query_as(
        "SELECT id, tin_root, branch_code, registered_name, status FROM tin_claims
         WHERE account_id = $1 AND status IN ('pending', 'needs_fix') ORDER BY created_at",
    )
    .bind(account_id)
    .fetch_all(db)
    .await?;
    profiles.extend(open.into_iter().map(|(id, root, branch, registered_name, status)| {
        let tin = Tin::parse(&root, &branch).expect("stored TINs are valid");
        open_profile_json(id, &tin, &registered_name, &status, full)
    }));
    Ok(profiles)
}

fn profile_json(id: Uuid, tin: &Tin, registered_name: &str, activated_at: OffsetDateTime, full: bool) -> Value {
    let mut profile = json!({
        "id": id,
        "tin_masked": tin.masked(),
        "registered_name": registered_name,
        "activated_at": activated_at.unix_timestamp(),
        "permissions": if full { PROFILE_PERMISSIONS } else { VIEW_PERMISSIONS },
    });
    if full {
        profile["tin"] = json!(tin.to_string());
    }
    profile
}

fn open_profile_json(id: Uuid, tin: &Tin, registered_name: &str, status: &str, full: bool) -> Value {
    let mut profile = json!({
        "id": id,
        "tin_masked": tin.masked(),
        "registered_name": registered_name,
        "activated_at": null,
        "permissions": if full { OPEN_PERMISSIONS } else { VIEW_PERMISSIONS },
        "status": status,
    });
    if full {
        profile["tin"] = json!(tin.to_string());
    }
    profile
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_bir_app_sees_the_full_tin() {
        let tin = Tin::parse("123456789", "").unwrap();
        let at = OffsetDateTime::UNIX_EPOCH;
        let limited = profile_json(Uuid::nil(), &tin, "Juan", at, false).to_string();
        assert!(!limited.contains("123456789") && !limited.contains("123-456-789"), "{limited}");
        assert!(!limited.contains("\"tin\""), "{limited}");
        assert!(limited.contains("***-***-789-00000"));
        assert!(!limited.contains("filing."), "{limited}");
        let full = profile_json(Uuid::nil(), &tin, "Juan", at, true);
        assert_eq!(full["tin"], "123-456-789-00000");
        assert_eq!(full["permissions"], json!(PROFILE_PERMISSIONS));
    }

    #[test]
    fn open_profiles_prepare_but_never_submit() {
        let tin = Tin::parse("123456789", "").unwrap();
        let open = open_profile_json(Uuid::nil(), &tin, "Juan", "pending", true);
        assert_eq!(open["status"], "pending");
        assert!(open["activated_at"].is_null());
        assert!(!open["permissions"].to_string().contains("filing.submit"), "{open}");
        let limited = open_profile_json(Uuid::nil(), &tin, "Juan", "needs_fix", false);
        assert_eq!(limited["permissions"], json!(VIEW_PERMISSIONS));
        assert!(limited.get("tin").is_none());
    }

    #[test]
    fn each_kind_requires_and_accepts_its_own_papers() {
        assert_eq!(ClaimKind::Personal.required(), &[DocumentKind::TinId]);
        assert_eq!(ClaimKind::Business.required(), &[DocumentKind::Form2303, DocumentKind::GovernmentId]);
        assert!(ClaimKind::Personal.accepts(DocumentKind::NationalId));
        assert!(!ClaimKind::Personal.accepts(DocumentKind::Form2303));
        assert!(ClaimKind::Business.accepts(DocumentKind::Authority));
        assert!(!ClaimKind::Business.accepts(DocumentKind::Selfie));
        for kind in DocumentKind::ALL {
            assert_eq!(DocumentKind::parse(kind.as_str()), Some(kind));
        }
    }

    #[test]
    fn sniffs_scans_and_photos_only() {
        assert_eq!(sniff(b"%PDF-1.7 ..."), Some("application/pdf"));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"\0\0\0\x18ftypheic...."), Some("image/heic"));
        assert_eq!(sniff(b"<html>"), None);
        assert_eq!(sniff(b"MZ\x90\0"), None);
    }

    #[test]
    fn filenames_lose_paths_and_odd_characters() {
        assert_eq!(clean_filename("C:\\scans\\2303 copy.pdf"), "2303 copy.pdf");
        assert_eq!(clean_filename("../../etc/passwd"), "passwd");
        assert_eq!(clean_filename("\"evil\";.pdf"), "evil.pdf");
        assert_eq!(clean_filename("///"), "document");
    }
}
