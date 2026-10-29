//! TIN claims: an account proves it owns a TIN with Form 2303, an ID and any
//! supporting documents. Staff review each claim by hand. An approved claim
//! is an active tax profile, and active tax profiles are what license the
//! Buwiz apps (`GET /api/tax-profiles`).
//!
//! Lifecycle: `pending` → `approved` | `rejected` (staff) | `withdrawn`
//! (claimant). One TIN has at most one approved owner. Document bytes are
//! dropped when a claim ends; the hash and metadata stay.

use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::crypto;
use crate::error::ApiError;
use crate::notice::Notice;
use crate::oauth::Bearer;
use crate::state::AppState;
use crate::tin::Tin;

pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_SUPPORTING_FILES: usize = 5;
/// Permissions an approved TIN grants in the Buwiz BIR app.
pub const PROFILE_PERMISSIONS: &[&str] = &["profile.view", "filing.prepare", "filing.submit"];
/// What other clients (Buwiz Books, AI agents) may do with a profile.
pub const VIEW_PERMISSIONS: &[&str] = &["profile.view"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentKind {
    Form2303,
    GovernmentId,
    Supporting,
}

impl DocumentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Form2303 => "form_2303",
            Self::GovernmentId => "government_id",
            Self::Supporting => "supporting",
        }
    }

    pub fn label(kind: &str) -> &'static str {
        match kind {
            "form_2303" => "BIR Form 2303",
            "government_id" => "Government ID",
            _ => "Supporting document",
        }
    }
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

/// Opens a claim with its files. Errors are messages for the person.
pub async fn submit(
    db: &PgPool,
    key: &[u8; 32],
    account_id: Uuid,
    tin: &Tin,
    registered_name: &str,
    uploads: Vec<Upload>,
) -> Result<Uuid, SubmitError> {
    let registered_name = registered_name.trim();
    if registered_name.is_empty() || registered_name.len() > 200 {
        return Err(SubmitError::Invalid(Notice::NameMissing));
    }
    for required in [DocumentKind::Form2303, DocumentKind::GovernmentId] {
        if !uploads.iter().any(|upload| upload.kind == required) {
            return Err(SubmitError::Invalid(match required {
                DocumentKind::Form2303 => Notice::Need2303,
                _ => Notice::NeedId,
            }));
        }
    }
    if uploads.iter().filter(|upload| upload.kind == DocumentKind::Supporting).count() > MAX_SUPPORTING_FILES {
        return Err(SubmitError::Invalid(Notice::TooManySupporting));
    }
    let mut checked = Vec::with_capacity(uploads.len());
    for upload in uploads {
        if upload.bytes.len() > MAX_FILE_BYTES {
            return Err(SubmitError::Invalid(Notice::FileTooBig));
        }
        let Some(content_type) = sniff(&upload.bytes) else {
            return Err(SubmitError::Invalid(Notice::FileType));
        };
        checked.push((upload, content_type));
    }

    let mut tx = db.begin().await?;
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT status FROM tin_claims
         WHERE account_id = $1 AND tin_root = $2 AND branch_code = $3 AND status IN ('pending', 'approved')",
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
        "INSERT INTO tin_claims (account_id, tin_root, branch_code, registered_name, status)
         VALUES ($1, $2, $3, $4, 'pending') RETURNING id",
    )
    .bind(account_id)
    .bind(tin.root())
    .bind(tin.branch())
    .bind(registered_name)
    .fetch_one(&mut *tx)
    .await;
    let claim_id: Uuid = match inserted {
        // The same claim sent twice at once: the second one loses the race.
        Err(sqlx::Error::Database(error)) if error.constraint() == Some("tin_claims_one_open") => {
            return Err(SubmitError::Invalid(Notice::ClaimAlreadyPending));
        }
        other => other?,
    };
    for (upload, content_type) in checked {
        let document_id = Uuid::new_v4();
        let (nonce, ciphertext) = crypto::seal(key, &document_context(claim_id, document_id), &upload.bytes)?;
        sqlx::query(
            "INSERT INTO claim_documents (id, claim_id, kind, filename, content_type, size_bytes, sha256, nonce, ciphertext)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(document_id)
        .bind(claim_id)
        .bind(upload.kind.as_str())
        .bind(clean_filename(&upload.filename))
        .bind(content_type)
        .bind(i32::try_from(upload.bytes.len()).unwrap_or(i32::MAX))
        .bind(crypto::sha256(&upload.bytes))
        .bind(nonce)
        .bind(ciphertext)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    tracing::info!(%claim_id, %account_id, "tin claim submitted");
    Ok(claim_id)
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
    pub email: String,
    pub tin_root: String,
    pub branch_code: String,
    pub registered_name: String,
    pub status: String,
    pub decision_note: Option<String>,
    pub created_at: OffsetDateTime,
}

impl ClaimRow {
    pub fn tin(&self) -> Tin {
        Tin::parse(&self.tin_root, &self.branch_code).expect("stored TINs are valid")
    }
}

const CLAIM_COLUMNS: &str =
    "c.id, a.email, c.tin_root, c.branch_code, c.registered_name, c.status, c.decision_note, c.created_at";

pub async fn for_account(db: &PgPool, account_id: Uuid) -> sqlx::Result<Vec<ClaimRow>> {
    sqlx::query_as(&format!(
        "SELECT {CLAIM_COLUMNS} FROM tin_claims c JOIN accounts a ON a.id = c.account_id
         WHERE c.account_id = $1 ORDER BY c.created_at DESC"
    ))
    .bind(account_id)
    .fetch_all(db)
    .await
}

pub async fn pending(db: &PgPool) -> sqlx::Result<Vec<ClaimRow>> {
    sqlx::query_as(&format!(
        "SELECT {CLAIM_COLUMNS} FROM tin_claims c JOIN accounts a ON a.id = c.account_id
         WHERE c.status = 'pending' ORDER BY c.created_at"
    ))
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

/// Other claims on the same TIN: the reviewer must see a dispute.
pub async fn rivals(db: &PgPool, claim: &ClaimRow) -> sqlx::Result<Vec<ClaimRow>> {
    sqlx::query_as(&format!(
        "SELECT {CLAIM_COLUMNS} FROM tin_claims c JOIN accounts a ON a.id = c.account_id
         WHERE c.tin_root = $1 AND c.branch_code = $2 AND c.id <> $3 AND c.status IN ('pending', 'approved')
         ORDER BY c.created_at"
    ))
    .bind(&claim.tin_root)
    .bind(&claim.branch_code)
    .bind(claim.id)
    .fetch_all(db)
    .await
}

#[derive(Debug, sqlx::FromRow)]
pub struct DocumentRow {
    pub id: Uuid,
    pub kind: String,
    pub filename: String,
    pub size_bytes: i32,
    pub purged_at: Option<OffsetDateTime>,
}

pub async fn documents(db: &PgPool, claim_id: Uuid) -> sqlx::Result<Vec<DocumentRow>> {
    sqlx::query_as(
        "SELECT id, kind, filename, size_bytes, purged_at FROM claim_documents
         WHERE claim_id = $1 ORDER BY kind, created_at",
    )
    .bind(claim_id)
    .fetch_all(db)
    .await
}

pub struct OpenedDocument {
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// Decrypts one stored document, if its bytes still exist.
pub async fn open_document(db: &PgPool, key: &[u8; 32], document_id: Uuid) -> anyhow::Result<Option<OpenedDocument>> {
    #[derive(sqlx::FromRow)]
    struct Stored {
        claim_id: Uuid,
        filename: String,
        content_type: String,
        nonce: Option<Vec<u8>>,
        ciphertext: Option<Vec<u8>>,
    }
    let row: Option<Stored> =
        sqlx::query_as("SELECT claim_id, filename, content_type, nonce, ciphertext FROM claim_documents WHERE id = $1")
            .bind(document_id)
            .fetch_optional(db)
            .await?;
    let Some(Stored { claim_id, filename, content_type, nonce: Some(nonce), ciphertext: Some(ciphertext) }) = row
    else {
        return Ok(None);
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

/// Staff approve or reject a pending claim.
pub async fn decide(
    db: &PgPool,
    claim_id: Uuid,
    reviewer_id: Uuid,
    approve: bool,
    note: &str,
) -> Result<(), DecideError> {
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
    if status != "pending" {
        return Err(DecideError::Refused(Notice::AlreadyDecided));
    }
    let note = note.trim();
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
    purge_documents(&mut tx, claim_id).await?;
    tx.commit().await?;
    tracing::info!(%claim_id, %reviewer_id, status, "tin claim decided");
    Ok(())
}

/// The claimant gives up a pending claim.
pub async fn withdraw(db: &PgPool, claim_id: Uuid, account_id: Uuid) -> sqlx::Result<bool> {
    let mut tx = db.begin().await?;
    let updated = sqlx::query(
        "UPDATE tin_claims SET status = 'withdrawn', decided_at = now()
         WHERE id = $1 AND account_id = $2 AND status = 'pending'",
    )
    .bind(claim_id)
    .bind(account_id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 1 {
        purge_documents(&mut tx, claim_id).await?;
    }
    tx.commit().await?;
    Ok(updated.rows_affected() == 1)
}

async fn purge_documents(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, claim_id: Uuid) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE claim_documents SET ciphertext = NULL, nonce = NULL, purged_at = now()
         WHERE claim_id = $1 AND purged_at IS NULL",
    )
    .bind(claim_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// `GET /api/tax-profiles`: the account's active TINs. The Buwiz apps treat
/// any profile with permissions as a license to use their BIR tools. Only
/// the BIR app (scope `app`) sees the full TIN; everyone else gets the
/// opaque `id` and the masked TIN.
pub async fn tax_profiles(State(state): State<AppState>, bearer: Bearer) -> Result<Json<Value>, ApiError> {
    let full = bearer.scope == crate::oauth::SCOPE_APP;
    Ok(Json(json!({ "profiles": active_profiles(&state.db, bearer.account.id, full).await? })))
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
