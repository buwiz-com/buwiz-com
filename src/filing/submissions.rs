//! Returns sent to BIR through the relay.
//!
//! 1. `POST /api/submissions` with the plaintext IAF XML and its file name.
//!    The TIN, branch, form and period inside the XML must be the file
//!    name's, and the TIN must be active on the account. The XML is kept,
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

use crate::account::oauth::{BANNED_MESSAGE, Bearer};
use crate::account::tin::Tin;
use crate::base::crypto;
use crate::base::error::ApiError;
use crate::base::state::AppState;
use crate::filing::devices::{self, Signed};
use crate::filing::envelope::Envelope;
use crate::filing::gate;
use crate::filing::jws;
use crate::filing::tickets::{self, TicketError};
use crate::relay::{Control, Frame};
use crate::sender::{Job, identity};
use crate::uptime::classify::{Class, Mode};
use crate::uptime::keys::Network;
use crate::uptime::observe::{self, TryObserver};
use crate::uptime::public;
use crate::uptime::store::{self, RelayTry};

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
    // A test TIN too: the gate refuses it with its own code.
    let tin = Tin::parse_any(&digits[..9], &digits[9..]).map_err(|_| invalid)?;
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
    // The account may send for the file name's TIN only if the return
    // inside is for that TIN too: the same check the Sender makes on a
    // sealed return against its ticket.
    if let Err(reason) = identity::check_named(xml, &name) {
        return Err(ApiError::Refused {
            status: StatusCode::BAD_REQUEST,
            code: "return_mismatch",
            message: format!("The return does not match its file name: {reason}."),
        });
    }
    if let Some(key) = &request.idempotency_key
        && (key.is_empty() || key.len() > 100)
    {
        return Err(ApiError::bad_request("bad_idempotency_key", "idempotency_key must be 1 to 100 characters"));
    }
    let claim_id = gate::own(&state.db, bearer.account.id, &name.tin).await?.sendable()?;
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

impl SubmissionView {
    /// What a token that may not see full TINs gets (`Bearer::sees_full_tins`):
    /// the file name with the TIN masked and no email, and no remote path or
    /// receipt, which carry the full TIN. The receipt stays in the BIR apps.
    pub fn for_bearer(mut self, bearer: &Bearer) -> Self {
        if !bearer.sees_full_tins() {
            self.filename = match parse_filename(&self.filename) {
                Ok(name) => format!("{}-{}-{}.xml", name.tin.masked(), name.form_type, name.period),
                Err(_) => String::new(),
            };
            self.remote_path = None;
            self.receipt_jws = None;
        }
        self
    }
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
    let view = find(&state.db, bearer.account.id, id).await?.ok_or_else(ApiError::not_found)?;
    Ok(Json(view.for_bearer(&bearer)))
}

/// `GET /api/submissions`: the 50 newest.
pub async fn list(State(state): State<AppState>, bearer: Bearer) -> Result<Json<Value>, ApiError> {
    let views: Vec<SubmissionView> =
        recent(&state.db, bearer.account.id).await?.into_iter().map(|view| view.for_bearer(&bearer)).collect();
    Ok(Json(json!({ "submissions": views })))
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
    // The TIN may have stopped being active since the ticket. Refusing
    // drops the transaction, so the ticket is not spent.
    gate::of_claim(&mut *tx, bearer.account.id, spent.claim_id).await?.sendable()?;
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
    // Where the try comes from, for its uptime record: the network and the
    // app's version, in memory only.
    let context = state.uptime.level.records().then(|| TryContext {
        network: state.uptime.networks.network(&parts.headers, OffsetDateTime::now_utc()),
        client: crate::uptime::client_label(&parts.headers),
    });
    let upgrade = WebSocketUpgrade::from_request_parts(&mut parts, &state)
        .await
        .map_err(|_| ApiError::bad_request("websocket_required", "open this path as a WebSocket"))?;
    let account_id = bearer.account.id;
    Ok(upgrade.on_upgrade(move |socket| async move {
        if let Err(error) = run_relay(state, socket, account_id, id, context).await {
            tracing::error!(submission = %id, error = ?error, "relay failed");
        }
    }))
}

/// A submission locked for sending, with what the Sender needs.
#[derive(sqlx::FromRow)]
pub struct Locked {
    form_type: String,
    period: String,
    filename: String,
    xml_nonce: Option<Vec<u8>>,
    xml_ciphertext: Option<Vec<u8>>,
    envelope: Option<String>,
    ticket: Option<String>,
    tin_root: String,
    branch_code: String,
    /// The ticket's admission (`tickets.bir_admission`), if any.
    admission: Option<String>,
}

/// What the uptime record of a relay try knows from its request.
struct TryContext {
    network: Network,
    client: Option<String>,
}

/// What the relay does with a submission when the app connects.
pub enum RelayStart {
    /// It was locked (`sending`): hand it to the Sender.
    Send(Box<Locked>),
    /// It cannot be sent now: tell the app why and close.
    Answer(Control),
}

/// Locks a submission for one send, or says why it cannot go now. Runs once
/// the relay's socket is open, so a ban or a change to the TIN after the
/// request's token check still stops the send.
pub async fn claim_for_relay(db: &PgPool, account_id: Uuid, id: Uuid) -> anyhow::Result<RelayStart> {
    // Only one send at a time, and never after `sent` or `unknown`. A v2
    // submission gets one attempt: its ticket works once. The claim must be
    // this account's and sendable, and the account not banned.
    let locked: Option<Locked> = sqlx::query_as(&format!(
        "UPDATE submissions s SET status = 'sending', sending_since = now(), attempts = attempts + 1
         FROM tin_claims c, accounts a
         WHERE s.id = $1 AND s.account_id = $2 AND c.id = s.claim_id AND c.account_id = s.account_id AND {}
           AND a.id = s.account_id AND a.banned_at IS NULL
           AND ((s.xml_ciphertext IS NOT NULL AND s.status IN ('ready', 'failed'))
             OR (s.envelope IS NOT NULL AND s.status = 'ready'))
           AND (s.device_id IS NULL OR EXISTS (SELECT 1 FROM devices d WHERE d.id = s.device_id AND d.revoked_at IS NULL))
         RETURNING s.form_type, s.period, s.filename, s.xml_nonce, s.xml_ciphertext, s.envelope,
                   (SELECT t.jws FROM tickets t WHERE t.jti = s.ticket_jti) AS ticket,
                   c.tin_root, c.branch_code,
                   (SELECT t.bir_admission FROM tickets t WHERE t.jti = s.ticket_jti) AS admission",
        gate::SENDABLE_SQL
    ))
    .bind(id)
    .bind(account_id)
    .fetch_optional(db)
    .await?;
    if let Some(locked) = locked {
        return Ok(RelayStart::Send(Box::new(locked)));
    }
    // A return still waiting whose account was banned or whose TIN stopped
    // being active since: it is failed with the reason, so the app takes it
    // back to a draft. Only a return still waiting: one `sending` on
    // another connection keeps its own outcome.
    let waiting: Option<(Uuid, bool)> = sqlx::query_as(
        "SELECT s.claim_id, a.banned_at IS NOT NULL FROM submissions s JOIN accounts a ON a.id = s.account_id
         WHERE s.id = $1 AND s.account_id = $2",
    )
    .bind(id)
    .bind(account_id)
    .fetch_optional(db)
    .await?;
    if let Some((claim_id, banned)) = waiting {
        let refusal = if banned {
            Some(("account_banned", format!("{BANNED_MESSAGE} (account_banned)")))
        } else {
            let standing = gate::of_claim(db, account_id, claim_id).await?;
            (!standing.can_send()).then(|| (standing.code(), standing.failure()))
        };
        if let Some((code, error)) = refusal {
            let failed = sqlx::query(
                "UPDATE submissions SET status = 'failed', error = $3, envelope = NULL,
                     purged_at = CASE WHEN envelope IS NOT NULL THEN now() ELSE purged_at END
                 WHERE id = $1 AND account_id = $2 AND status IN ('ready', 'failed')
                   AND (xml_ciphertext IS NOT NULL OR envelope IS NOT NULL)",
            )
            .bind(id)
            .bind(account_id)
            .bind(&error)
            .execute(db)
            .await?;
            if failed.rows_affected() == 1 {
                tracing::warn!(submission = %id, code, "submission not sent: its account or TIN cannot send");
                let result = Control::Result { status: "failed".to_owned(), error: Some(error), receipt: None };
                return Ok(RelayStart::Answer(result));
            }
        }
    }
    let view = find(db, account_id, id).await?;
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
    Ok(RelayStart::Answer(Control::Result { status, error, receipt }))
}

async fn run_relay(
    state: AppState,
    mut socket: WebSocket,
    account_id: Uuid,
    id: Uuid,
    context: Option<TryContext>,
) -> anyhow::Result<()> {
    let locked = match claim_for_relay(&state.db, account_id, id).await? {
        RelayStart::Send(locked) => locked,
        RelayStart::Answer(result) => {
            let _ = socket.send(Message::Text(serde_json::to_string(&result)?.into())).await;
            let _ = socket.send(Message::Close(None)).await;
            return Ok(());
        }
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
    // A test TIN's return never gets here (`gate::SENDABLE_SQL` keeps it
    // from being claimed), so it is never recorded. Every return that does
    // is a real send, whatever its TIN reads as.
    let recording = context.map(|context| RelayTry {
        account_id,
        network: context.network,
        client: context.client,
        admission: locked.admission.clone().unwrap_or_else(|| "open".to_owned()),
        started_at: OffsetDateTime::now_utc(),
    });
    bridge(&state, socket, &job, id, v2, recording).await?;
    Ok(())
}

/// Stores the outcome of one send.
/// `mode` (from `BIR_UPTIME=public`): the Sender's mode, kept on a failed
/// or unknown send too.
async fn record(
    db: &PgPool,
    id: Uuid,
    v2: bool,
    status: &str,
    error: Option<&str>,
    receipt: Option<&str>,
    mode: Option<&str>,
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
                "UPDATE submissions SET status = 'failed', error = $2, mode = coalesce($4, mode),
                     envelope = NULL, purged_at = CASE WHEN $3 THEN now() ELSE purged_at END
                 WHERE id = $1",
            )
            .bind(id)
            .bind(error)
            .bind(v2)
            .bind(mode)
            .execute(db)
            .await?;
            tracing::warn!(submission = %id, reason = error.unwrap_or_default(), "submission not sent");
        }
        _ => {
            sqlx::query(
                "UPDATE submissions SET status = 'unknown', error = $2, mode = coalesce($3, mode),
                     xml_nonce = NULL, xml_ciphertext = NULL, envelope = NULL, purged_at = now()
                 WHERE id = $1",
            )
            .bind(id)
            .bind(error.unwrap_or("the send ended without an outcome"))
            .bind(mode)
            .execute(db)
            .await?;
            tracing::error!(submission = %id, "submission may have reached BIR; not retrying");
        }
    }
    Ok(())
}

/// How long to wait for the Sender's outcome after the app is gone.
const OUTCOME_WAIT: Duration = Duration::from_secs(60);

/// What the app is told of a try beyond the `Control` (from
/// `BIR_UPTIME=public`), and the mode the submission keeps.
#[derive(Default)]
struct Told {
    class: Option<Class>,
    mode: Option<Mode>,
    /// The public sign as it is when the app is told (this try's record is
    /// written after, off the send's path).
    bir: Option<Value>,
}

/// Records `result`; the app hears it only after that, so the submission
/// reads the same when it asks. The text is `result` as the `Control`
/// serializes it, byte for byte, with the added fields after its own.
async fn conclude(db: &PgPool, id: Uuid, v2: bool, result: &Control, told: &Told) -> anyhow::Result<String> {
    let Control::Result { status, error, receipt } = result else {
        anyhow::bail!("not a result");
    };
    let receipt = receipt.as_ref().and_then(Value::as_str);
    let mode = told.mode.filter(|mode| *mode != Mode::Unknown).map(Mode::as_str);
    record(db, id, v2, status, error.as_deref(), receipt, mode).await?;
    let mut added = Vec::new();
    if let Some(class) = told.class {
        added.push(("class", Value::from(class.as_str())));
    }
    if let Some(bir) = &told.bir {
        added.push(("bir", bir.clone()));
    }
    Ok(with_fields(serde_json::to_string(result)?, &added))
}

/// `object` (a serialized JSON object) with `fields` added at its end, so
/// what was there before reads byte for byte the same.
fn with_fields(mut object: String, fields: &[(&str, Value)]) -> String {
    if fields.is_empty() || !object.ends_with('}') {
        return object;
    }
    object.pop();
    for (name, value) in fields {
        if !object.ends_with('{') {
            object.push(',');
        }
        object.push_str(&Value::from(*name).to_string());
        object.push(':');
        object.push_str(&value.to_string());
    }
    object.push('}');
    object
}

/// Concludes the try: records `result`, tells the app (unless it is gone)
/// and closes its socket, then writes the try's uptime row in a task of its
/// own ([`store::Recorder`]), which never delays or fails the send. The
/// record's place is taken only once the app was told, so an app that
/// stops reading holds none (and no stop waits for it).
#[allow(clippy::too_many_arguments)]
async fn settle(
    state: &AppState,
    app_tx: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    id: Uuid,
    v2: bool,
    result: &Control,
    observer: Option<TryObserver>,
    recording: Option<RelayTry>,
    app_gone: bool,
) -> anyhow::Result<()> {
    let ended_at = OffsetDateTime::now_utc();
    let observed = match (observer, result) {
        (Some(observer), Control::Result { status, error, .. }) => Some(observer.finish(status, error.as_deref())),
        _ => None,
    };
    let told = match &observed {
        Some(observed) if state.uptime.level.is_public() => {
            let class = observed.class();
            // The sign goes with every result the app is told about: a live
            // try's, or one with a class. A lab try, or one whose mode is
            // unknown, is told nothing.
            let bir = (observed.mode == Mode::Live || class.is_some()).then(|| {
                let sign = state.uptime.live.sign();
                let document = public::document(&sign, &state.config.forms_origin, public::Times::Public);
                serde_json::to_value(document).unwrap_or(Value::Null)
            });
            Told { class, mode: Some(observed.mode), bir }
        }
        _ => Told::default(),
    };
    let text = conclude(&state.db, id, v2, result, &told).await?;
    if !app_gone {
        let _ = app_tx.send(Message::Text(text.into())).await;
    }
    let _ = app_tx.send(Message::Close(None)).await;
    if let (Some(observed), Some(recording)) = (observed, recording)
        && let Some(slot) = store::Recorder::slot(&state.uptime)
    {
        slot.record(state.db.clone(), recording, observed, ended_at);
    }
    Ok(())
}

/// Hands `job` to the Sender and forwards frames between it and the app
/// until the Sender's `result`, which it records, then passes on. The
/// Sender's `sender` frame stops here. With `recording`, the try's frames
/// are watched for its uptime record ([`crate::uptime::observe`]).
async fn bridge(
    state: &AppState,
    socket: WebSocket,
    job: &Job,
    id: Uuid,
    v2: bool,
    recording: Option<RelayTry>,
) -> anyhow::Result<()> {
    use tokio_tungstenite::tungstenite::Message as Up;

    let (mut app_tx, mut app_rx) = socket.split();
    let mut observer = recording.as_ref().map(|_| TryObserver::new(&state.uptime.upload_ports));
    let finish =
        |status: &str, error: String| Control::Result { status: status.to_owned(), error: Some(error), receipt: None };
    let upstream = match state.sender.open().await {
        Ok(upstream) => upstream,
        Err(error) => {
            tracing::error!(error = ?error, "the Sender is not reachable");
            if let Some(observer) = observer.as_mut() {
                observer.sender_down();
            }
            let result = finish("failed", "the sending service is not reachable; try again later".into());
            return settle(state, &mut app_tx, id, v2, &result, observer, recording, false).await;
        }
    };
    let (mut up_tx, mut up_rx) = upstream.split();
    if up_tx.send(Up::Text(serde_json::to_string(job).expect("json").into())).await.is_err() {
        if let Some(observer) = observer.as_mut() {
            observer.sender_down();
        }
        let result = finish("failed", "the sending service is not reachable; try again later".into());
        return settle(state, &mut app_tx, id, v2, &result, observer, recording, false).await;
    }
    // Streams the Sender asked the app to open, so they can be ended if the
    // app goes away.
    let mut streams: Vec<u8> = Vec::new();
    let mut app_gone = false;
    let mut result = None;
    let mut deadline_passed = false;
    let deadline = tokio::time::sleep(Duration::from_secs(24 * 3600));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            message = app_rx.next(), if !app_gone => {
                let forward = match message {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(observer) = observer.as_mut() {
                            observer.app_text(text.as_str());
                        }
                        Some(Up::Text(text.as_str().into()))
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        if let Some(observer) = observer.as_mut() {
                            observer.app_binary(&bytes);
                        }
                        Some(Up::Binary(bytes))
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => None,
                    Some(Ok(Message::Close(_)) | Err(_)) | None => {
                        // Tell the Sender every stream is gone, then wait
                        // for its own verdict.
                        app_gone = true;
                        if let Some(observer) = observer.as_mut() {
                            observer.app_gone();
                        }
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
                    // The `sender` frame (its mode) is for this side only.
                    let forward = match observer.as_mut() {
                        Some(observer) => observer.sender_text(&text),
                        None => !observe::is_sender_frame(&text),
                    };
                    match Frame::decode_text(&text) {
                        Some(Frame::Control(Control::Open { stream, .. })) => streams.push(stream),
                        Some(Frame::Control(outcome @ Control::Result { .. })) => {
                            result = Some(outcome);
                            break;
                        }
                        _ => {}
                    }
                    if forward && !app_gone {
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
            _ = &mut deadline => {
                deadline_passed = true;
                break;
            }
        }
    }
    let _ = up_tx.send(Up::Close(None)).await;
    if result.is_none()
        && let Some(observer) = observer.as_mut()
    {
        // After the app went away the deadline is the 60 s outcome wait.
        if deadline_passed && app_gone { observer.wait_expired() } else { observer.sender_lost() }
    }
    let result = result.unwrap_or_else(|| {
        if streams.is_empty() {
            finish("failed", "the sending service stopped before connecting to BIR".into())
        } else {
            finish("unknown", "the send ended without an outcome; BIR may have received this return".into())
        }
    });
    settle(state, &mut app_tx, id, v2, &result, observer, recording, app_gone).await
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
/// is deleted, and so is a sealed return whose ticket ran out unsent. (BIR
/// uptime's retention runs beside it, [`crate::uptime::hourly`].)
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
    crate::filing::devices::upkeep(db).await?;
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
        // A test TIN reads; the gate refuses it (tin_test_only).
        assert!(parse_filename("00000000000000-1601Cv2018-062025#a@b.ph#.xml").unwrap().tin.is_test());
    }

    #[test]
    fn refuses_names_that_could_escape_or_lie() {
        for bad in [
            "12345678900000-1601Cv2018-062025.xml",
            "12345678900000-../x-062025#a@b.ph#.xml",
            "1234567890000-1601Cv2018-062025#a@b.ph#.xml",
            "12345678900000-1601Cv2018-062025#a b@c.ph#.xml",
            "12345678900000-1601Cv2018-062025#a/b@c.ph#.xml",
            "12345678900000-1601Cv2018-062025#nobody#.xml",
        ] {
            assert!(parse_filename(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn added_fields_go_after_the_controls_own() {
        let result = Control::Result { status: "failed".into(), error: Some("x".into()), receipt: None };
        let plain = serde_json::to_string(&result).unwrap();
        assert_eq!(with_fields(plain.clone(), &[]), plain);
        let added = with_fields(plain.clone(), &[("class", json!("bir")), ("bir", json!({"v": 1}))]);
        assert_eq!(added, format!("{},\"class\":\"bir\",\"bir\":{{\"v\":1}}}}", &plain[..plain.len() - 1]));
        // Still the same Control to every reader.
        assert_eq!(Frame::decode_text(&added), Some(Frame::Control(result)));
        assert_eq!(with_fields("{}".into(), &[("a", json!(1))]), r#"{"a":1}"#);
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
