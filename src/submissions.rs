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
use axum::extract::{FromRequestParts, Path, State};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::devices::{self, Signed};
use crate::envelope::Envelope;
use crate::error::ApiError;
use crate::oauth::Bearer;
use crate::relay::{Control, Frame};
use crate::sender::Job;
use crate::state::AppState;
use crate::tickets::{self, TicketError};
use crate::tin::Tin;
use crate::{crypto, jws};

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

#[derive(Deserialize)]
pub struct NewSubmissionV2 {
    ticket: String,
    envelope: Envelope,
}

#[derive(sqlx::FromRow)]
struct SpentTicket {
    claim_id: Uuid,
    form_type: String,
    period: String,
    payload_sha256: Vec<u8>,
}

/// `POST /api/v2/submissions`: a ticket and the return sealed for the
/// Sender. The envelope is stored as it came; this server cannot open it.
pub async fn create_v2(State(state): State<AppState>, signed: Signed<NewSubmissionV2>) -> Result<Response, ApiError> {
    let Signed { bearer, device, body: request } = signed;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let claims = match tickets::verify(&[state.ticket_key.verifying_key()], &request.ticket, now) {
        Ok(claims) => claims,
        Err(TicketError::Expired) => {
            return Err(ApiError::forbidden("ticket_expired", "the ticket expired; ask for a new one"));
        }
        Err(TicketError::Invalid(_)) => return Err(ApiError::forbidden("ticket_invalid", "the ticket is not valid")),
    };
    if claims.account_id != bearer.account.id || claims.device_id != device.id {
        return Err(ApiError::forbidden("ticket_mismatch", "the ticket was issued to another device"));
    }
    let envelope = serde_json::to_string(&request.envelope).map_err(anyhow::Error::from)?;
    let mut tx = state.db.begin().await?;
    let spent: Option<SpentTicket> = sqlx::query_as(
        "UPDATE tickets SET used_at = now()
         WHERE jti = $1 AND account_id = $2 AND device_id = $3 AND used_at IS NULL AND expires_at > now()
         RETURNING claim_id, form_type, period, payload_sha256",
    )
    .bind(claims.jti)
    .bind(bearer.account.id)
    .bind(device.id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(spent) = spent else {
        return Err(ApiError::conflict("ticket_used", "this ticket was already used; ask for a new one"));
    };
    let active: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM tin_claims WHERE id = $1 AND status = 'approved')")
            .bind(spent.claim_id)
            .fetch_one(&mut *tx)
            .await?;
    if !active {
        return Err(ApiError::forbidden("tin_not_active", "this TIN is not active on your account"));
    }
    let id = Uuid::new_v4();
    let filename = claims.filename();
    sqlx::query(
        "INSERT INTO submissions (id, account_id, claim_id, form_type, period, filename, xml_sha256, status,
             ticket_jti, device_id, envelope)
         VALUES ($1, $2, $3, $4, $5, $6, $7, 'ready', $8, $9, $10)",
    )
    .bind(id)
    .bind(bearer.account.id)
    .bind(spent.claim_id)
    .bind(&spent.form_type)
    .bind(&spent.period)
    .bind(&filename)
    .bind(&spent.payload_sha256)
    .bind(claims.jti)
    .bind(device.id)
    .bind(&envelope)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE tickets SET submission_id = $2 WHERE jti = $1")
        .bind(claims.jti)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    tracing::info!(submission = %id, jti = %claims.jti, account = %bearer.account.id, "sealed submission stored");
    let view = find(&state.db, bearer.account.id, id).await?.ok_or_else(ApiError::not_found)?;
    Ok((StatusCode::CREATED, Json(view)).into_response())
}

/// `GET /api/submissions/{id}/relay`: upgrades to the relay pipe. A v2
/// submission also needs its device's signature on this request.
pub async fn relay(
    State(state): State<AppState>,
    bearer: Bearer,
    Path(id): Path<Uuid>,
    request: axum::extract::Request,
) -> Result<Response, ApiError> {
    bearer.require_app()?;
    let device_id: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT device_id FROM submissions WHERE id = $1 AND account_id = $2")
            .bind(id)
            .bind(bearer.account.id)
            .fetch_optional(&state.db)
            .await?;
    let Some(device_id) = device_id else {
        return Err(ApiError::not_found());
    };
    let (mut parts, _body) = request.into_parts();
    if let Some(expected) = device_id {
        let device = devices::verify(&state.db, &parts, &bearer, b"").await?;
        if device.id != expected {
            return Err(ApiError::forbidden("ticket_mismatch", "this return was sealed on another device"));
        }
    }
    let upgrade = WebSocketUpgrade::from_request_parts(&mut parts, &state)
        .await
        .map_err(|_| ApiError::bad_request("websocket_required", "open this path as a WebSocket"))?;
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
    xml_nonce: Option<Vec<u8>>,
    xml_ciphertext: Option<Vec<u8>>,
    envelope: Option<String>,
    ticket: Option<String>,
    tin_root: String,
    branch_code: String,
}

async fn run_relay(state: AppState, mut socket: WebSocket, account_id: Uuid, id: Uuid) -> anyhow::Result<()> {
    // Only one send at a time, and never after `sent` or `unknown`. A v2
    // submission gets one attempt: its ticket works once.
    let locked: Option<Locked> = sqlx::query_as(
        "UPDATE submissions s SET status = 'sending', sending_since = now(), attempts = attempts + 1
         FROM tin_claims c
         WHERE s.id = $1 AND s.account_id = $2 AND c.id = s.claim_id AND c.status = 'approved'
           AND ((s.xml_ciphertext IS NOT NULL AND s.status IN ('ready', 'failed'))
             OR (s.envelope IS NOT NULL AND s.status = 'ready'))
           AND (s.device_id IS NULL OR EXISTS (SELECT 1 FROM devices d WHERE d.id = s.device_id AND d.revoked_at IS NULL))
         RETURNING s.form_type, s.period, s.filename, s.xml_nonce, s.xml_ciphertext, s.envelope,
                   (SELECT t.jws FROM tickets t WHERE t.jti = s.ticket_jti) AS ticket,
                   c.tin_root, c.branch_code",
    )
    .bind(id)
    .bind(account_id)
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
        let result = Control::Result { status, error, receipt };
        let _ = socket.send(Message::Text(serde_json::to_string(&result)?.into())).await;
        let _ = socket.send(Message::Close(None)).await;
        return Ok(());
    };

    let v2 = locked.envelope.is_some();
    let job = match (&locked.envelope, &locked.ticket, &locked.xml_nonce, &locked.xml_ciphertext) {
        (Some(envelope), Some(ticket), _, _) => {
            Job::V2 { submission_id: id, account_id, ticket: ticket.clone(), envelope: serde_json::from_str(envelope)? }
        }
        (None, _, Some(nonce), Some(ciphertext)) => {
            let xml = crypto::open(&state.config.document_key, &xml_context(id), nonce, ciphertext)?;
            Job::V1 {
                submission_id: id,
                account_id,
                tin_root: locked.tin_root.clone(),
                branch_code: locked.branch_code.clone(),
                form_type: locked.form_type.clone(),
                period: locked.period.clone(),
                filename: locked.filename.clone(),
                xml: base64::engine::general_purpose::STANDARD.encode(xml),
            }
        }
        _ => anyhow::bail!("submission {id} has nothing to send"),
    };
    bridge(&state, socket, &job, id, v2).await?;
    Ok(())
}

/// Stores the outcome of one send.
async fn record(
    db: &PgPool,
    id: Uuid,
    v2: bool,
    status: &str,
    error: Option<&str>,
    receipt: Option<&str>,
) -> anyhow::Result<()> {
    match (status, receipt) {
        ("sent" | "dry_run", Some(receipt)) => {
            let claims = jws::claims_unverified(receipt).unwrap_or_default();
            sqlx::query(
                "UPDATE submissions SET status = $2, sent_at = now(), remote_path = $3, payload_sha256 = $4,
                     receipt_jws = $5, mode = $6, error = NULL, xml_nonce = NULL, xml_ciphertext = NULL,
                     envelope = NULL, purged_at = now()
                 WHERE id = $1",
            )
            .bind(id)
            .bind(status)
            .bind(claims["remote_path"].as_str())
            .bind(claims["payload_sha256"].as_str())
            .bind(receipt)
            .bind(claims["mode"].as_str())
            .execute(db)
            .await?;
            tracing::info!(submission = %id, status, "submission sent");
        }
        // A v1 submission may be tried again; a v2 one needs a new ticket.
        ("failed", _) => {
            sqlx::query(
                "UPDATE submissions SET status = 'failed', error = $2,
                     envelope = NULL, purged_at = CASE WHEN $3 THEN now() ELSE purged_at END
                 WHERE id = $1",
            )
            .bind(id)
            .bind(error)
            .bind(v2)
            .execute(db)
            .await?;
            tracing::warn!(submission = %id, reason = error.unwrap_or_default(), "submission not sent");
        }
        _ => {
            sqlx::query(
                "UPDATE submissions SET status = 'unknown', error = $2, xml_nonce = NULL, xml_ciphertext = NULL,
                     envelope = NULL, purged_at = now()
                 WHERE id = $1",
            )
            .bind(id)
            .bind(error.unwrap_or("the send ended without an outcome"))
            .execute(db)
            .await?;
            tracing::error!(submission = %id, "submission may have reached BIR; not retrying");
        }
    }
    Ok(())
}

/// How long to wait for the Sender's outcome after the app is gone.
const OUTCOME_WAIT: Duration = Duration::from_secs(60);

/// Records `result`; the app hears it only after that, so the submission
/// reads the same when it asks.
async fn conclude(db: &PgPool, id: Uuid, v2: bool, result: &Control) -> anyhow::Result<String> {
    let Control::Result { status, error, receipt } = result else {
        anyhow::bail!("not a result");
    };
    let receipt = receipt.as_ref().and_then(Value::as_str);
    record(db, id, v2, status, error.as_deref(), receipt).await?;
    Ok(serde_json::to_string(result)?)
}

/// Hands `job` to the Sender and forwards frames between it and the app
/// until the Sender's `result`, which it records, then passes on.
async fn bridge(state: &AppState, socket: WebSocket, job: &Job, id: Uuid, v2: bool) -> anyhow::Result<()> {
    use tokio_tungstenite::tungstenite::Message as Up;

    let (mut app_tx, mut app_rx) = socket.split();
    let finish =
        |status: &str, error: String| Control::Result { status: status.to_owned(), error: Some(error), receipt: None };
    let upstream = match state.sender.open().await {
        Ok(upstream) => upstream,
        Err(error) => {
            tracing::error!(error = ?error, "the Sender is not reachable");
            let result = finish("failed", "the sending service is not reachable; try again later".into());
            let text = conclude(&state.db, id, v2, &result).await?;
            let _ = app_tx.send(Message::Text(text.into())).await;
            let _ = app_tx.send(Message::Close(None)).await;
            return Ok(());
        }
    };
    let (mut up_tx, mut up_rx) = upstream.split();
    if up_tx.send(Up::Text(serde_json::to_string(job).expect("json").into())).await.is_err() {
        let result = finish("failed", "the sending service is not reachable; try again later".into());
        let text = conclude(&state.db, id, v2, &result).await?;
        let _ = app_tx.send(Message::Text(text.into())).await;
        let _ = app_tx.send(Message::Close(None)).await;
        return Ok(());
    }
    // Streams the Sender asked the app to open, so they can be ended if the
    // app goes away.
    let mut streams: Vec<u8> = Vec::new();
    let mut app_gone = false;
    let mut result = None;
    let deadline = tokio::time::sleep(Duration::from_secs(24 * 3600));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            message = app_rx.next(), if !app_gone => {
                let forward = match message {
                    Some(Ok(Message::Text(text))) => Some(Up::Text(text.as_str().into())),
                    Some(Ok(Message::Binary(bytes))) => Some(Up::Binary(bytes)),
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => None,
                    Some(Ok(Message::Close(_)) | Err(_)) | None => {
                        // Tell the Sender every stream is gone, then wait
                        // for its own verdict.
                        app_gone = true;
                        for &stream in &streams {
                            let refused = Control::Refused { stream, reason: "the app disconnected".into() };
                            for control in [refused, Control::Close { stream }] {
                                let _ = up_tx.send(Up::Text(serde_json::to_string(&control).expect("json").into())).await;
                            }
                        }
                        deadline.as_mut().reset(tokio::time::Instant::now() + OUTCOME_WAIT);
                        None
                    }
                };
                if let Some(forward) = forward && up_tx.send(forward).await.is_err() {
                    break;
                }
            }
            message = up_rx.next() => match message {
                Some(Ok(Up::Text(text))) => {
                    match Frame::decode_text(&text) {
                        Some(Frame::Control(Control::Open { stream, .. })) => streams.push(stream),
                        Some(Frame::Control(outcome @ Control::Result { .. })) => {
                            result = Some(outcome);
                            break;
                        }
                        _ => {}
                    }
                    if !app_gone {
                        let _ = app_tx.send(Message::Text(text.as_str().into())).await;
                    }
                }
                Some(Ok(Up::Binary(bytes))) => {
                    if !app_gone {
                        let _ = app_tx.send(Message::Binary(bytes)).await;
                    }
                }
                Some(Ok(Up::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            _ = &mut deadline => break,
        }
    }
    let _ = up_tx.send(Up::Close(None)).await;
    let result = result.unwrap_or_else(|| {
        if streams.is_empty() {
            finish("failed", "the sending service stopped before connecting to BIR".into())
        } else {
            finish("unknown", "the send ended without an outcome; BIR may have received this return".into())
        }
    });
    let text = conclude(&state.db, id, v2, &result).await?;
    if !app_gone {
        let _ = app_tx.send(Message::Text(text.into())).await;
    }
    let _ = app_tx.send(Message::Close(None)).await;
    Ok(())
}

/// Key id of a receipt key. See [`jws::key_id`].
pub fn key_id(key: &SigningKey) -> String {
    jws::key_id(&key.verifying_key())
}

/// A compact JWS (EdDSA) receipt over `claims`.
pub fn sign_receipt(key: &SigningKey, claims: &Value) -> String {
    jws::sign(key, crate::sender::RECEIPT_TYP, claims)
}

/// `GET /.well-known/jwks.json`: the keys that verify receipts.
pub async fn jwks(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let public = state.sender.public().await?;
    Ok(Json(json!({ "keys": public["receipt_keys"] })))
}

/// `GET /.well-known/buwiz-hpke.json`: the key apps seal returns to.
pub async fn hpke_key(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let public = state.sender.public().await?;
    Ok(Json(public["hpke"].clone()))
}

/// Startup and hourly upkeep: a send stuck in `sending` for 10 minutes
/// ended with its server, so its outcome is unknown; XML older than 24 hours
/// is deleted, and so is a sealed return whose ticket ran out unsent.
pub async fn upkeep(db: &PgPool) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE submissions SET status = 'unknown', error = 'the server stopped while sending',
             xml_nonce = NULL, xml_ciphertext = NULL, envelope = NULL, purged_at = now()
         WHERE status = 'sending' AND sending_since < now() - interval '10 minutes'",
    )
    .execute(db)
    .await?;
    sqlx::query(
        "UPDATE submissions SET status = 'expired', envelope = NULL, purged_at = now()
         WHERE envelope IS NOT NULL AND status = 'ready' AND created_at < now() - interval '10 minutes'",
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
    crate::devices::upkeep(db).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
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
