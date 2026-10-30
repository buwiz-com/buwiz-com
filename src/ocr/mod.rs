//! Reading the TIN ID and Form 2303 of a claim on this server, so staff see
//! what the document says beside what was typed.
//!
//! When a claim is sent or resent, its TIN ID or 2303 is queued here. One
//! document is read at a time, with a time limit, so a big upload cannot
//! starve a small server. The file is decrypted in memory only; a digital
//! PDF gives its text layer, a photo goes through the OCR engine
//! (PaddleOCR PP-OCRv5 mobile, in [`paddle`], with the `ocr` feature).
//! Only the fields staff compare, their confidence and the comparison are
//! kept (`document_readings`), and they are deleted with the file. Nothing
//! leaves the server, and nothing is decided from a reading.

pub mod extract;
#[cfg(feature = "ocr")]
pub mod paddle;
#[cfg(feature = "ocr")]
mod prepare;

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use sqlx::types::Json;
use time::OffsetDateTime;
use tokio::sync::Semaphore;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::claims;
use crate::state::AppState;
use crate::tin::Tin;
use extract::{Confidence, Fields, Line, Verdict};

/// Why every reading fails in a build without the `ocr` feature.
pub const NOT_BUILT_IN: &str = "OCR not built in";
/// Longest one document may take. The engine is not stopped at the limit;
/// the next document waits until it finishes.
const TIME_LIMIT: Duration = Duration::from_secs(120);
/// The documents read: those that carry the TIN and the name.
const READ_KINDS: [&str; 2] = ["tin_id", "form_2303"];

/// A photo ready for an engine: upright, RGB, 8 bits a channel, row by row.
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// An OCR engine: the lines of text in a photo, top to bottom.
pub trait Engine: Send + Sync {
    /// Stored with each reading, with [`Engine::version`].
    fn name(&self) -> &'static str;
    fn version(&self) -> &'static str;
    /// Errors are short reasons staff read ("Couldn't read: …").
    fn read(&self, bitmap: &Bitmap) -> Result<Vec<Line>, String>;
}

/// The engine of a build without the `ocr` feature.
pub struct NotBuiltIn;

impl Engine for NotBuiltIn {
    fn name(&self) -> &'static str {
        "none"
    }

    fn version(&self) -> &'static str {
        "0"
    }

    fn read(&self, _: &Bitmap) -> Result<Vec<Line>, String> {
        Err(NOT_BUILT_IN.to_owned())
    }
}

/// The engine and the queue's single turn, shared by every request.
#[derive(Clone)]
pub struct Reader {
    engine: Arc<dyn Engine>,
    turn: Arc<Semaphore>,
}

impl Reader {
    /// PaddleOCR with the models in `OCR_MODEL_DIR` (default `ocr-models`)
    /// when built with `ocr`; otherwise [`NotBuiltIn`]. Models load on the
    /// first photo, not at start.
    pub fn from_env() -> Self {
        #[cfg(feature = "ocr")]
        let engine: Arc<dyn Engine> = Arc::new(paddle::PaddleOcr::new(
            std::env::var_os("OCR_MODEL_DIR").map_or_else(|| "ocr-models".into(), Into::into),
        ));
        #[cfg(not(feature = "ocr"))]
        let engine: Arc<dyn Engine> = Arc::new(NotBuiltIn);
        Self::with_engine(engine)
    }

    pub fn with_engine(engine: Arc<dyn Engine>) -> Self {
        Self { engine, turn: Arc::new(Semaphore::new(1)) }
    }
}

/// Text read from a file, and what read it.
#[derive(Debug)]
pub struct Text {
    pub engine: &'static str,
    pub version: &'static str,
    pub lines: Vec<Line>,
}

/// Reads a decrypted file: a PDF's text layer, or a photo through `engine`.
/// Errors are reasons for staff.
pub fn read_file(engine: &dyn Engine, content_type: &str, bytes: &[u8]) -> Result<Text, String> {
    match content_type {
        "application/pdf" => pdf_text(bytes),
        "image/jpeg" | "image/png" | "image/webp" => {
            let bitmap = decode(bytes)?;
            let lines = engine.read(&bitmap)?;
            Ok(Text { engine: engine.name(), version: engine.version(), lines })
        }
        "image/heic" => Err("HEIC photos are not supported yet".to_owned()),
        _ => Err("not a photo or a PDF".to_owned()),
    }
}

#[cfg(feature = "ocr")]
fn pdf_text(bytes: &[u8]) -> Result<Text, String> {
    prepare::pdf_text(bytes)
}

#[cfg(not(feature = "ocr"))]
fn pdf_text(_: &[u8]) -> Result<Text, String> {
    Err(NOT_BUILT_IN.to_owned())
}

#[cfg(feature = "ocr")]
fn decode(bytes: &[u8]) -> Result<Bitmap, String> {
    prepare::photo(bytes)
}

#[cfg(not(feature = "ocr"))]
fn decode(_: &[u8]) -> Result<Bitmap, String> {
    Err(NOT_BUILT_IN.to_owned())
}

/// Queues the claim's TIN ID and Form 2303 files still stored. Called when
/// a claim is sent or resent; a resend may change the typed name, so every
/// file is read again.
pub async fn queue_claim(state: &AppState, claim_id: Uuid) {
    let documents: sqlx::Result<Vec<Uuid>> = sqlx::query_scalar(
        "SELECT id FROM claim_documents WHERE claim_id = $1 AND kind = ANY($2) AND purged_at IS NULL",
    )
    .bind(claim_id)
    .bind(&READ_KINDS[..])
    .fetch_all(&state.db)
    .await;
    match documents {
        Ok(documents) => {
            for document_id in documents {
                queue_document(state, document_id).await;
            }
        }
        Err(error) => tracing::error!(%claim_id, error = ?error, "could not queue a claim's documents for reading"),
    }
}

/// Reads one document from the start, replacing any reading it had. For a
/// file just stored or replaced; other kinds than the TIN ID and Form 2303,
/// and deleted files, are skipped. Never fails the caller: problems are
/// logged.
pub async fn queue_document(state: &AppState, document_id: Uuid) {
    let job: sqlx::Result<Option<Uuid>> = sqlx::query_scalar(
        "INSERT INTO document_readings (id, document_id, status)
         SELECT $1, d.id, 'reading' FROM claim_documents d
         WHERE d.id = $2 AND d.kind = ANY($3) AND d.purged_at IS NULL
         ON CONFLICT (document_id) DO UPDATE SET id = EXCLUDED.id, status = 'reading', engine = NULL,
             engine_version = NULL, fields = NULL, confidence = NULL, tin_match = NULL, name_match = NULL,
             error = NULL, created_at = now(), read_at = NULL
         RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(document_id)
    .bind(&READ_KINDS[..])
    .fetch_optional(&state.db)
    .await;
    match job {
        Ok(Some(job)) => {
            let state = state.clone();
            tokio::spawn(async move { read(state, document_id, job).await });
        }
        Ok(None) => {}
        Err(error) => tracing::error!(%document_id, error = ?error, "could not queue a document for reading"),
    }
}

/// Queues again the readings a restart cut short, and the files of claims
/// waiting for review that were never read (sent before OCR was added).
pub async fn resume(state: &AppState) -> sqlx::Result<()> {
    let waiting: Vec<Uuid> = sqlx::query_scalar(
        "SELECT document_id FROM document_readings WHERE status = 'reading'
         UNION
         SELECT d.id FROM claim_documents d JOIN tin_claims c ON c.id = d.claim_id
         WHERE c.status = 'pending' AND d.kind = ANY($1) AND d.purged_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM document_readings r WHERE r.document_id = d.id)",
    )
    .bind(&READ_KINDS[..])
    .fetch_all(&state.db)
    .await?;
    for document_id in waiting {
        queue_document(state, document_id).await;
    }
    Ok(())
}

async fn read(state: AppState, document_id: Uuid, job: Uuid) {
    let started = std::time::Instant::now();
    let outcome = read_now(&state, document_id).await;
    let failed = outcome.as_ref().err().cloned();
    match save(&state.db, &*state.ocr.engine, document_id, job, outcome).await {
        Ok(()) => tracing::info!(%document_id, ms = started.elapsed().as_millis(), error = failed, "document read"),
        Err(error) => tracing::error!(%document_id, error = ?error, "could not save a document reading"),
    }
}

/// Waits for the turn, decrypts the file and reads it within the limit.
async fn read_now(state: &AppState, document_id: Uuid) -> Result<Text, String> {
    let turn = state.ocr.turn.clone().acquire_owned().await.expect("the turn is never closed");
    let opened = claims::open_document(&state.db, &state.documents, &state.config.document_key, document_id)
        .await
        .map_err(|error| {
            tracing::error!(%document_id, error = ?error, "could not open a document to read it");
            "the file could not be opened".to_owned()
        })?
        .ok_or_else(|| "the file was deleted".to_owned())?;
    let engine = state.ocr.engine.clone();
    let reading = tokio::task::spawn_blocking(move || {
        // The turn ends when the engine does, even after the time limit.
        let _turn = turn;
        let bytes = Zeroizing::new(opened.bytes);
        read_file(&*engine, &opened.content_type, &bytes)
    });
    match tokio::time::timeout(TIME_LIMIT, reading).await {
        Ok(Ok(text)) => text,
        Ok(Err(_)) => Err("the reader stopped on this file".to_owned()),
        Err(_) => Err(format!("took longer than {} seconds", TIME_LIMIT.as_secs())),
    }
}

/// Stores the fields and how they compare with the claim, unless the
/// document was queued again (or deleted) meanwhile.
async fn save(
    db: &PgPool,
    engine: &dyn Engine,
    document_id: Uuid,
    job: Uuid,
    outcome: Result<Text, String>,
) -> anyhow::Result<()> {
    let text = match outcome {
        Ok(text) => text,
        Err(reason) => {
            sqlx::query(
                "UPDATE document_readings SET status = 'failed', engine = $3, engine_version = $4, error = $5,
                     read_at = now()
                 WHERE document_id = $1 AND id = $2",
            )
            .bind(document_id)
            .bind(job)
            .bind(engine.name())
            .bind(engine.version())
            .bind(reason)
            .execute(db)
            .await?;
            return Ok(());
        }
    };
    let typed: Option<(String, String, String)> = sqlx::query_as(
        "SELECT c.tin_root, c.branch_code, c.registered_name
         FROM claim_documents d JOIN tin_claims c ON c.id = d.claim_id WHERE d.id = $1",
    )
    .bind(document_id)
    .fetch_optional(db)
    .await?;
    let Some((root, branch, registered_name)) = typed else { return Ok(()) };
    let (fields, confidence) = extract::extract(&text.lines);
    let tin = Tin::parse(&root, &branch).expect("stored TINs are valid");
    let tin_match = fields.tin.as_deref().map(|read| extract::compare_tin(&tin, read).as_str());
    let name_match = fields.name.as_deref().map(|read| extract::compare_names(&registered_name, read).as_str());
    sqlx::query(
        "UPDATE document_readings SET status = 'read', engine = $3, engine_version = $4, fields = $5,
             confidence = $6, tin_match = $7, name_match = $8, read_at = now()
         WHERE document_id = $1 AND id = $2",
    )
    .bind(document_id)
    .bind(job)
    .bind(text.engine)
    .bind(text.version)
    .bind(Json(&fields))
    .bind(Json(&confidence))
    .bind(tin_match)
    .bind(name_match)
    .execute(db)
    .await?;
    Ok(())
}

/// A document's reading, for the claim review.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Reading {
    pub document_kind: String,
    /// `reading`, `read` or `failed`.
    pub status: String,
    pub engine: Option<String>,
    pub fields: Option<Json<Fields>>,
    pub confidence: Option<Json<Confidence>>,
    pub error: Option<String>,
    pub read_at: Option<OffsetDateTime>,
}

impl Reading {
    /// Read from a PDF's text rather than by OCR.
    pub fn from_text_layer(&self) -> bool {
        self.engine.as_deref() == Some(PDF_TEXT)
    }
}

/// The engine name of readings taken from a PDF's text layer.
pub const PDF_TEXT: &str = "pdf-text";

/// The reading of the claim's TIN ID or 2303 still stored, newest file
/// first.
pub async fn for_claim(db: &PgPool, claim_id: Uuid) -> sqlx::Result<Option<Reading>> {
    sqlx::query_as(
        "SELECT d.kind AS document_kind, r.status, r.engine, r.fields, r.confidence, r.error, r.read_at
         FROM claim_documents d JOIN document_readings r ON r.document_id = d.id
         WHERE d.claim_id = $1 AND d.purged_at IS NULL
         ORDER BY d.created_at DESC
         LIMIT 1",
    )
    .bind(claim_id)
    .fetch_optional(db)
    .await
}

/// A verdict stored as text.
pub fn verdict(raw: Option<&str>) -> Option<Verdict> {
    raw.and_then(Verdict::parse)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_the_feature_nothing_is_read() {
        let bitmap = Bitmap { width: 1, height: 1, rgb: vec![255; 3] };
        assert_eq!(NotBuiltIn.read(&bitmap), Err(NOT_BUILT_IN.to_owned()));
        if cfg!(not(feature = "ocr")) {
            for content_type in ["image/png", "image/jpeg", "image/webp", "application/pdf"] {
                let error = read_file(&NotBuiltIn, content_type, b"anything").unwrap_err();
                assert_eq!(error, NOT_BUILT_IN, "{content_type}");
            }
        }
        assert!(read_file(&NotBuiltIn, "image/heic", b"").unwrap_err().contains("HEIC"));
    }
}
