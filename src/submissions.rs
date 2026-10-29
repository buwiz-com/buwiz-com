//! Returns sent to BIR through the relay.
//!
//! 1. `POST /api/submissions` with the plaintext IAF XML and its file name.
//!    The TIN in the name must be active on the account. The XML is kept,
//!    encrypted, until it is sent or 24 hours pass.
//! 2. `GET /api/submissions/{id}/relay` opens the WebSocket pipe (see
//!    [`crate::relay`]). The server sends the return through it and answers
//!    with a `result` carrying a signed receipt.
//! 3. `GET /api/submissions/{id}` reads the outcome later.
//!
//! A send that may have reached BIR before failing ends as `unknown` and is
//! never retried automatically: sending again could file the return twice.

use std::time::Duration;

use axum::Json;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signer, SigningKey};
use futures_util::{SinkExt, StreamExt};
use http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::bir::{self, Job, SendError};
use crate::crypto;
use crate::error::ApiError;
use crate::oauth::Bearer;
use crate::relay::{Control, Frame, Pipe};
use crate::state::AppState;
use crate::tin::Tin;

const MAX_XML_BYTES: usize = 2 * 1024 * 1024;
pub const BODY_LIMIT: usize = 3 * MAX_XML_BYTES;
const RETENTION_HOURS: i64 = 24;

/// The parts of an IAF file name: `{TIN}-{form}-{period}#{email}#.xml`,
/// where the TIN is 12 or 14 digits (9-digit root and branch code).
#[derive(Debug, PartialEq, Eq)]
pub struct IafName {
    pub tin: Tin,
    pub form_type: String,
    pub period: String,
    pub email: String,
}

pub fn parse_filename(name: &str) -> Result<IafName, &'static str> {
    let invalid = "the file name must look like 123456789000-1601Cv2018-062025#you@example.com#.xml";
    let stem = name.strip_suffix("#.xml").ok_or(invalid)?;
    let (head, email) = stem.split_once('#').ok_or(invalid)?;
    let mut parts = head.splitn(3, '-');
    let (Some(digits), Some(form_type), Some(period)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(invalid);
    };
    if !matches!(digits.len(), 12 | 14) || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid);
    }
    let tin = Tin::parse(&digits[..9], &digits[9..]).map_err(|_| invalid)?;
    let word = |text: &str, max: usize| {
        (1..=max).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    };
    if !word(form_type, 24) || !word(period, 24) {
        return Err(invalid);
    }
    let email_ok = email.len() <= 254
        && email.contains('@')
        && !email.chars().any(|ch| matches!(ch, '#' | '/' | '\\') || ch.is_whitespace() || ch.is_control());
    if !email_ok {
        return Err(invalid);
    }
    Ok(IafName { tin, form_type: form_type.to_owned(), period: period.to_owned(), email: email.to_owned() })
}

#[derive(Deserialize)]
pub struct NewSubmission {
    filename: String,
    xml: String,
    idempotency_key: Option<String>,
}

/// `POST /api/submissions`.
pub async fn create(
    State(state): State<AppState>,
    bearer: Bearer,
    Json(request): Json<NewSubmission>,
) -> Result<Response, ApiError> {
    bearer.require_app()?;
    let name = parse_filename(&request.filename).map_err(|message| ApiError::bad_request("bad_filename", message))?;
    let xml = request.xml.as_bytes();
    if xml.is_empty() || xml.len() > MAX_XML_BYTES {
        return Err(ApiError::bad_request("bad_xml", "the return must be 1 byte to 2 MB of XML"));
    }
    if let Some(key) = &request.idempotency_key
        && (key.is_empty() || key.len() > 100)
    {
        return Err(ApiError::bad_request("bad_idempotency_key", "idempotency_key must be 1 to 100 characters"));
    }
    let claim_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM tin_claims WHERE account_id = $1 AND tin_root = $2 AND branch_code = $3 AND status = 'approved'",
    )
    .bind(bearer.account.id)
    .bind(name.tin.root())
    .bind(name.tin.branch())
    .fetch_optional(&state.db)
    .await?;
    let Some(claim_id) = claim_id else {
        return Err(ApiError::forbidden("tin_not_active", "this TIN is not active on your account"));
    };
    if let Some(key) = &request.idempotency_key {
        let existing: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM submissions WHERE account_id = $1 AND idempotency_key = $2")
                .bind(bearer.account.id)
                .bind(key)
                .fetch_optional(&state.db)
                .await?;
        if let Some(id) = existing {
            let view = find(&state.db, bearer.account.id, id).await?.ok_or_else(ApiError::not_found)?;
            return Ok(Json(view).into_response());
        }
    }
    let id = Uuid::new_v4();
    let (nonce, ciphertext) = crypto::seal(&state.config.document_key, &xml_context(id), xml)?;
    let inserted = sqlx::query(
        "INSERT INTO submissions (id, account_id, claim_id, idempotency_key, form_type, period, filename,
             xml_sha256, xml_nonce, xml_ciphertext, status)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'ready')",
    )
    .bind(id)
    .bind(bearer.account.id)
    .bind(claim_id)
    .bind(&request.idempotency_key)
    .bind(&name.form_type)
    .bind(&name.period)
    .bind(&request.filename)
    .bind(crypto::sha256(xml))
    .bind(nonce)
    .bind(ciphertext)
    .execute(&state.db)
    .await;
    if let Err(sqlx::Error::Database(error)) = &inserted
        && error.is_unique_violation()
    {
        return Err(ApiError::conflict("duplicate", "this idempotency_key is already in use; retry the request"));
    }
    inserted?;
    tracing::info!(submission = %id, account = %bearer.account.id, form = %name.form_type, "submission created");
    let view = find(&state.db, bearer.account.id, id).await?.ok_or_else(ApiError::not_found)?;
    Ok((StatusCode::CREATED, Json(view)).into_response())
}

fn xml_context(id: Uuid) -> Vec<u8> {
    format!("submission/{id}").into_bytes()
}

#[derive(Serialize, sqlx::FromRow)]
pub struct SubmissionView {
    id: Uuid,
    status: String,
    filename: String,
    form_type: String,
    period: String,
    mode: Option<String>,
    attempts: i32,
    error: Option<String>,
    remote_path: Option<String>,
    receipt_jws: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    sent_at: Option<OffsetDateTime>,
    #[sqlx(skip)]
    relay_path: String,
}

const VIEW_COLUMNS: &str =
    "id, status, filename, form_type, period, mode, attempts, error, remote_path, receipt_jws, created_at, sent_at";

pub async fn find(db: &PgPool, account_id: Uuid, id: Uuid) -> sqlx::Result<Option<SubmissionView>> {
    let view: Option<SubmissionView> =
        sqlx::query_as(&format!("SELECT {VIEW_COLUMNS} FROM submissions WHERE id = $1 AND account_id = $2"))
            .bind(id)
            .bind(account_id)
            .fetch_optional(db)
            .await?;
    Ok(view.map(|mut view| {
        view.relay_path = format!("/api/submissions/{}/relay", view.id);
        view
    }))
}

/// `GET /api/submissions/{id}`.
pub async fn show(
    State(state): State<AppState>,
    bearer: Bearer,
    Path(id): Path<Uuid>,
) -> Result<Json<SubmissionView>, ApiError> {
    find(&state.db, bearer.account.id, id).await?.map(Json).ok_or_else(ApiError::not_found)
}

/// `GET /api/submissions`: the 50 newest.
pub async fn list(State(state): State<AppState>, bearer: Bearer) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({ "submissions": recent(&state.db, bearer.account.id).await? })))
}

pub async fn recent(db: &PgPool, account_id: Uuid) -> sqlx::Result<Vec<SubmissionView>> {
    let views: Vec<SubmissionView> = sqlx::query_as(&format!(
        "SELECT {VIEW_COLUMNS} FROM submissions WHERE account_id = $1 ORDER BY created_at DESC LIMIT 50"
    ))
    .bind(account_id)
    .fetch_all(db)
    .await?;
    Ok(views
        .into_iter()
        .map(|mut view| {
            view.relay_path = format!("/api/submissions/{}/relay", view.id);
            view
        })
        .collect())
}

/// `GET /api/submissions/{id}/relay`: upgrades to the relay pipe.
pub async fn relay(
    State(state): State<AppState>,
    bearer: Bearer,
    Path(id): Path<Uuid>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    bearer.require_app()?;
    if find(&state.db, bearer.account.id, id).await?.is_none() {
        return Err(ApiError::not_found());
    }
    let account_id = bearer.account.id;
    Ok(upgrade.on_upgrade(move |socket| async move {
        if let Err(error) = run_relay(state, socket, account_id, id).await {
            tracing::error!(submission = %id, error = ?error, "relay failed");
        }
    }))
}

#[derive(sqlx::FromRow)]
struct Locked {
    form_type: String,
    period: String,
    filename: String,
    xml_sha256: Vec<u8>,
    xml_nonce: Vec<u8>,
    xml_ciphertext: Vec<u8>,
    tin_root: String,
    branch_code: String,
}

async fn run_relay(state: AppState, socket: WebSocket, account_id: Uuid, id: Uuid) -> anyhow::Result<()> {
    let (to_app, from_server) = mpsc::channel::<Frame>(64);
    let (to_server, from_app) = mpsc::channel::<Frame>(64);
    let pump = tokio::spawn(pump_socket(socket, from_server, to_server));
    let pipe = Pipe::new(to_app, from_app);

    // Only one send at a time, and never after `sent` or `unknown`.
    let locked: Option<Locked> = sqlx::query_as(
        "UPDATE submissions s SET status = 'sending', sending_since = now(), attempts = attempts + 1, mode = $3
         FROM tin_claims c
         WHERE s.id = $1 AND s.account_id = $2 AND c.id = s.claim_id AND c.status = 'approved'
           AND s.status IN ('ready', 'failed') AND s.xml_ciphertext IS NOT NULL
         RETURNING s.form_type, s.period, s.filename, s.xml_sha256, s.xml_nonce, s.xml_ciphertext,
                   c.tin_root, c.branch_code",
    )
    .bind(id)
    .bind(account_id)
    .bind(state.relay.mode.as_str())
    .fetch_optional(&state.db)
    .await?;
    let Some(locked) = locked else {
        let view = find(&state.db, account_id, id).await?;
        let (status, error, receipt) = match view {
            Some(view) if matches!(view.status.as_str(), "sent" | "dry_run") => {
                (view.status, None, view.receipt_jws.map(Value::String))
            }
            Some(view) => {
                let reason = format!("this submission cannot be sent now (status {})", view.status);
                (view.status, Some(reason), None)
            }
            None => ("missing".to_owned(), Some("not found".to_owned()), None),
        };
        pipe.finish(Control::Result { status, error, receipt }).await;
        drop(pipe);
        let _ = pump.await;
        return Ok(());
    };

    let xml = crypto::open(&state.config.document_key, &xml_context(id), &locked.xml_nonce, &locked.xml_ciphertext)?;
    let job = Job { tin_root: &locked.tin_root, form_type: &locked.form_type, filename: &locked.filename, xml: &xml };
    let outcome = bir::send(&pipe, &job, &state.relay).await;
    let result = match outcome {
        Ok(sent) => {
            let status = if state.relay.mode == bir::Mode::DryRun { "dry_run" } else { "sent" };
            let sent_at = OffsetDateTime::now_utc();
            let tin = Tin::parse(&locked.tin_root, &locked.branch_code).expect("stored TINs are valid");
            let claims = json!({
                "iss": state.config.account_origin,
                "sub": id,
                "account_id": account_id,
                "tin": tin.to_string(),
                "form_type": locked.form_type,
                "period": locked.period,
                "filename": locked.filename,
                "xml_sha256": bir::hex(&locked.xml_sha256),
                "payload_sha256": sent.payload_sha256,
                "remote_path": sent.remote_path,
                "mode": state.relay.mode.as_str(),
                "status": status,
                "iat": sent_at.unix_timestamp(),
            });
            let jws = sign_receipt(&state.receipt_key, &claims);
            sqlx::query(
                "UPDATE submissions SET status = $2, sent_at = $3, remote_path = $4, payload_sha256 = $5,
                     receipt_jws = $6, error = NULL, xml_nonce = NULL, xml_ciphertext = NULL, purged_at = now()
                 WHERE id = $1",
            )
            .bind(id)
            .bind(status)
            .bind(sent_at)
            .bind(&sent.remote_path)
            .bind(&sent.payload_sha256)
            .bind(&jws)
            .execute(&state.db)
            .await?;
            tracing::info!(submission = %id, status, remote_path = %sent.remote_path, "submission sent");
            Control::Result { status: status.to_owned(), error: None, receipt: Some(Value::String(jws)) }
        }
        Err(SendError::NotSent(reason)) => {
            sqlx::query("UPDATE submissions SET status = 'failed', error = $2 WHERE id = $1")
                .bind(id)
                .bind(&reason)
                .execute(&state.db)
                .await?;
            tracing::warn!(submission = %id, %reason, "submission not sent");
            Control::Result { status: "failed".to_owned(), error: Some(reason), receipt: None }
        }
        Err(SendError::MaybeSent(reason)) => {
            sqlx::query(
                "UPDATE submissions SET status = 'unknown', error = $2, xml_nonce = NULL, xml_ciphertext = NULL,
                     purged_at = now()
                 WHERE id = $1",
            )
            .bind(id)
            .bind(&reason)
            .execute(&state.db)
            .await?;
            tracing::error!(submission = %id, %reason, "submission may have reached BIR; not retrying");
            let error = format!(
                "{reason}. BIR may have received this return; check for its confirmation email before sending again."
            );
            Control::Result { status: "unknown".to_owned(), error: Some(error), receipt: None }
        }
    };
    pipe.finish(result).await;
    drop(pipe);
    let _ = pump.await;
    Ok(())
}

/// Moves frames between the WebSocket and the pipe until either ends. The
/// pipe ends after the server's `result`.
async fn pump_socket(socket: WebSocket, mut from_server: mpsc::Receiver<Frame>, to_server: mpsc::Sender<Frame>) {
    let (mut sink, mut stream) = socket.split();
    let writer = tokio::spawn(async move {
        while let Some(frame) = from_server.recv().await {
            let message = match frame {
                Frame::Control(control) => {
                    Message::Text(serde_json::to_string(&control).expect("controls serialize").into())
                }
                Frame::Data { stream, bytes } => Message::Binary(Frame::encode_data(stream, &bytes).into()),
            };
            if sink.send(message).await.is_err() {
                return;
            }
        }
        let _ = sink.send(Message::Close(None)).await;
    });
    while let Some(Ok(message)) = stream.next().await {
        let frame = match message {
            Message::Text(text) => Frame::decode_text(&text),
            Message::Binary(bytes) => Frame::decode_data(&bytes),
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => continue,
        };
        match frame {
            Some(frame) => {
                if to_server.send(frame).await.is_err() {
                    break;
                }
            }
            None => tracing::warn!("relay: unreadable frame from the app"),
        }
    }
    drop(to_server);
    let _ = tokio::time::timeout(Duration::from_secs(5), writer).await;
}

/// Key id: the first 8 bytes of SHA-256 of the public key, hex.
pub fn key_id(key: &SigningKey) -> String {
    bir::hex(&Sha256::digest(key.verifying_key().as_bytes())[..8])
}

/// A compact JWS (EdDSA) over `claims`.
pub fn sign_receipt(key: &SigningKey, claims: &Value) -> String {
    let header = json!({ "alg": "EdDSA", "kid": key_id(key), "typ": "buwiz-receipt+jwt" });
    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("json")),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).expect("json")),
    );
    let signature = key.sign(signing_input.as_bytes());
    format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
}

/// `GET /.well-known/jwks.json`: the key that verifies receipts.
pub async fn jwks(State(state): State<AppState>) -> Json<Value> {
    let public = state.receipt_key.verifying_key();
    Json(json!({ "keys": [{
        "kty": "OKP",
        "crv": "Ed25519",
        "x": URL_SAFE_NO_PAD.encode(public.as_bytes()),
        "kid": key_id(&state.receipt_key),
        "alg": "EdDSA",
        "use": "sig",
    }] }))
}

/// Startup and hourly upkeep: a send stuck in `sending` for 10 minutes
/// ended with its server, so its outcome is unknown; XML older than 24 hours
/// is deleted.
pub async fn upkeep(db: &PgPool) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE submissions SET status = 'unknown', error = 'the server stopped while sending',
             xml_nonce = NULL, xml_ciphertext = NULL, purged_at = now()
         WHERE status = 'sending' AND sending_since < now() - interval '10 minutes'",
    )
    .execute(db)
    .await?;
    sqlx::query(&format!(
        "UPDATE submissions SET status = CASE WHEN status IN ('ready', 'failed') THEN 'expired' ELSE status END,
             xml_nonce = NULL, xml_ciphertext = NULL, purged_at = now()
         WHERE xml_ciphertext IS NOT NULL AND status <> 'sending'
           AND created_at < now() - interval '{RETENTION_HOURS} hours'"
    ))
    .execute(db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier};

    #[test]
    fn parses_official_file_names() {
        let name = parse_filename("12345678900000-1601Cv2018-062025#juan@example.ph#.xml").unwrap();
        assert_eq!(name.tin.to_string(), "123-456-789-00000");
        assert_eq!((name.form_type.as_str(), name.period.as_str()), ("1601Cv2018", "062025"));
        assert_eq!(name.email, "juan@example.ph");
        let old = parse_filename("123456789001-2551Qv2018-122025Q1#a@b.ph#.xml").unwrap();
        assert_eq!(old.tin.branch(), "00001");
    }

    #[test]
    fn refuses_names_that_could_escape_or_lie() {
        for bad in [
            "12345678900000-1601Cv2018-062025.xml",
            "12345678900000-../x-062025#a@b.ph#.xml",
            "1234567890000-1601Cv2018-062025#a@b.ph#.xml",
            "12345678900000-1601Cv2018-062025#a b@c.ph#.xml",
            "12345678900000-1601Cv2018-062025#a/b@c.ph#.xml",
            "00000000000000-1601Cv2018-062025#a@b.ph#.xml",
            "12345678900000-1601Cv2018-062025#nobody#.xml",
        ] {
            assert!(parse_filename(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn receipts_verify_with_the_published_key() {
        let key = SigningKey::from_bytes(&[9; 32]);
        let jws = sign_receipt(&key, &json!({ "sub": "x", "status": "sent" }));
        let parts: Vec<&str> = jws.split('.').collect();
        assert_eq!(parts.len(), 3);
        let signature = Signature::from_slice(&URL_SAFE_NO_PAD.decode(parts[2]).unwrap()).unwrap();
        let input = format!("{}.{}", parts[0], parts[1]);
        assert!(key.verifying_key().verify(input.as_bytes(), &signature).is_ok());
        let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header["kid"], key_id(&key));
    }
}
