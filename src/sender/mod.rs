//! The Sender: the only part of the server that holds BIR's passphrases,
//! the SFTP login and the HPKE private key. It runs as its own process
//! (`buwiz-sender`) with its own settings, reachable only by the
//! Gatekeeper on a private network.
//!
//! For each send the Gatekeeper opens `GET /internal/relay` (WebSocket,
//! `Authorization: Bearer $SENDER_SECRET`), sends one [`Job`] as a text
//! frame, then bridges the app's relay pipe to it frame for frame. The
//! Sender runs the send through the pipe (see [`crate::bir`]) and ends with
//! the pipe's `result`, carrying a receipt it signed.
//!
//! A v2 job carries the app's ticket and sealed envelope. The Sender checks
//! the ticket (signature, expiry, single use), opens the envelope, and
//! sends only when the XML's SHA-256, TIN, form and period are the
//! ticket's. The file name comes from the ticket.

pub mod identity;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context, bail};
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ed25519_dalek::{SigningKey, VerifyingKey};
use http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use tokio::sync::mpsc;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::bir::{self, SendError};
use crate::envelope::{Envelope, SealingKey};
use crate::relay::{self, Control, Pipe};
use crate::tickets;
use crate::tin::Tin;
use crate::{config, crypto, jws};

pub const RECEIPT_TYP: &str = "buwiz-receipt+jwt";

/// One send, from the Gatekeeper.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Job {
    /// A v1 submission: the Gatekeeper kept the plaintext XML.
    V1 {
        submission_id: Uuid,
        account_id: Uuid,
        tin_root: String,
        branch_code: String,
        form_type: String,
        period: String,
        filename: String,
        /// Base64.
        xml: String,
    },
    /// A v2 submission: the app's ticket and its sealed XML.
    V2 { submission_id: Uuid, account_id: Uuid, ticket: String, envelope: Envelope },
}

pub struct SenderConfig {
    pub relay: bir::Settings,
    pub sealing_key: SealingKey,
    pub receipt_key: SigningKey,
    /// Keys whose tickets the Sender accepts (the Gatekeeper's).
    pub ticket_keys: Vec<VerifyingKey>,
    /// What the Gatekeeper presents.
    pub secret: Zeroizing<String>,
    /// `iss` of receipts.
    pub issuer: String,
}

impl SenderConfig {
    /// `BIR_*` (see [`config::relay_settings`]), `SENDER_HPKE_KEY`,
    /// `RECEIPT_SIGNING_KEY`, `TICKET_PUBLIC_KEYS`, `SENDER_SECRET`,
    /// `RECEIPT_ISSUER`. Throwaway keys are allowed only in `dry_run`.
    pub fn from_env() -> anyhow::Result<Self> {
        let relay = config::relay_settings()?;
        let throwaway_ok = relay.mode == bir::Mode::DryRun;
        let ticket_keys = std::env::var("TICKET_PUBLIC_KEYS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|raw| !raw.is_empty())
            .map(public_key)
            .collect::<anyhow::Result<Vec<_>>>()?;
        if ticket_keys.is_empty() {
            bail!("TICKET_PUBLIC_KEYS must list the Gatekeeper's ticket key (base64url)");
        }
        let secret = Zeroizing::new(std::env::var("SENDER_SECRET").unwrap_or_default());
        if secret.len() < 32 {
            bail!("SENDER_SECRET must be at least 32 characters");
        }
        Ok(Self {
            relay,
            sealing_key: sealing_key(!throwaway_ok)?,
            receipt_key: config::receipt_key(!throwaway_ok)?,
            ticket_keys,
            secret,
            issuer: std::env::var("RECEIPT_ISSUER")
                .or_else(|_| std::env::var("ACCOUNT_ORIGIN"))
                .unwrap_or_else(|_| "http://localhost:3008".to_owned()),
        })
    }

    /// Development only: a Sender inside the Gatekeeper's process, trusting
    /// `ticket_key`, with a random secret. Reads the same `BIR_*`,
    /// `SENDER_HPKE_KEY` and `RECEIPT_SIGNING_KEY` settings.
    pub fn embedded(ticket_key: VerifyingKey, issuer: &str) -> anyhow::Result<Self> {
        let relay = config::relay_settings()?;
        let throwaway_ok = relay.mode == bir::Mode::DryRun;
        Ok(Self {
            relay,
            sealing_key: sealing_key(!throwaway_ok)?,
            receipt_key: config::receipt_key(!throwaway_ok)?,
            ticket_keys: vec![ticket_key],
            secret: Zeroizing::new(crypto::new_token("bws_")),
            issuer: issuer.to_owned(),
        })
    }
}

/// `SENDER_HPKE_KEY`: base64 32-byte X25519 private key.
fn sealing_key(required: bool) -> anyhow::Result<SealingKey> {
    match std::env::var("SENDER_HPKE_KEY").ok().filter(|value| !value.is_empty()) {
        Some(raw) => {
            let bytes: Zeroizing<Vec<u8>> =
                Zeroizing::new(STANDARD.decode(raw.trim()).context("SENDER_HPKE_KEY must be base64")?);
            let secret: [u8; 32] =
                bytes.as_slice().try_into().map_err(|_| anyhow::anyhow!("SENDER_HPKE_KEY must be 32 bytes"))?;
            SealingKey::from_bytes(&secret)
        }
        None if required => bail!("SENDER_HPKE_KEY is required outside dry_run"),
        None => {
            tracing::warn!("SENDER_HPKE_KEY is not set; returns are sealed to a throwaway key");
            let mut secret = [0u8; 32];
            rand::RngCore::fill_bytes(&mut rand::rng(), &mut secret);
            SealingKey::from_bytes(&secret)
        }
    }
}

fn public_key(raw: &str) -> anyhow::Result<VerifyingKey> {
    let bytes: [u8; 32] = URL_SAFE_NO_PAD
        .decode(raw.trim_end_matches('='))
        .context("a ticket key must be base64url")?
        .try_into()
        .map_err(|_| anyhow::anyhow!("a ticket key must be 32 bytes"))?;
    VerifyingKey::from_bytes(&bytes).context("not an Ed25519 key")
}

pub struct Sender {
    config: SenderConfig,
    /// Tickets already used, with their expiry. A ticket is useless after
    /// it expires, so expired entries are dropped.
    spent: Mutex<HashMap<Uuid, i64>>,
}

impl Sender {
    pub fn new(config: SenderConfig) -> Arc<Self> {
        Arc::new(Self { config, spent: Mutex::default() })
    }

    /// What the Gatekeeper publishes for apps: the receipt key and the
    /// sealing key.
    pub fn public_info(&self) -> Value {
        json!({
            "receipt_keys": [jws::jwk(&self.config.receipt_key.verifying_key())],
            "hpke": self.config.sealing_key.public_document(),
        })
    }

    /// `false` if the ticket was used before.
    fn spend(&self, jti: Uuid, exp: i64) -> bool {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let mut spent = self.spent.lock().expect("spent tickets");
        spent.retain(|_, expires| *expires > now);
        spent.insert(jti, exp).is_none()
    }

    /// Runs one job through `pipe` and answers the pipe's `result`.
    pub async fn handle(&self, pipe: &Pipe, job: Job) -> Control {
        match job {
            Job::V1 { submission_id, account_id, tin_root, branch_code, form_type, period, filename, xml } => {
                let Ok(xml) = STANDARD.decode(xml) else {
                    return failed("the job's XML is not base64".into());
                };
                let Ok(tin) = Tin::parse(&tin_root, &branch_code) else {
                    return failed("the job's TIN is not valid".into());
                };
                let send = Send { submission_id, account_id, tin, form_type, period, filename, xml, jti: None };
                self.send(pipe, send).await
            }
            Job::V2 { submission_id, account_id, ticket, envelope } => {
                let now = OffsetDateTime::now_utc().unix_timestamp();
                let claims = match tickets::verify(&self.config.ticket_keys, &ticket, now) {
                    Ok(claims) => claims,
                    Err(error) => return failed(error.to_string()),
                };
                if claims.account_id != account_id {
                    return failed("the ticket belongs to another account".into());
                }
                if !self.spend(claims.jti, claims.exp) {
                    return failed("the ticket was already used".into());
                }
                let xml = match self.config.sealing_key.open(&envelope, &claims.jti.to_string()) {
                    Ok(xml) => xml,
                    Err(reason) => return failed(reason.into()),
                };
                if bir::hex(&crypto::sha256(&xml)) != claims.payload_sha256 {
                    return failed("the sealed return is not the one the ticket names".into());
                }
                if let Err(reason) = identity::check(&xml, &claims) {
                    return failed(reason);
                }
                let Ok(tin) = Tin::parse(&claims.tin[..9], &claims.tin[9..]) else {
                    return failed("the ticket's TIN is not valid".into());
                };
                let send = Send {
                    submission_id,
                    account_id,
                    tin,
                    filename: claims.filename(),
                    form_type: claims.form,
                    period: claims.period,
                    xml,
                    jti: Some(claims.jti),
                };
                self.send(pipe, send).await
            }
        }
    }

    async fn send(&self, pipe: &Pipe, send: Send) -> Control {
        let job = bir::Job {
            tin_root: send.tin.root(),
            form_type: &send.form_type,
            filename: &send.filename,
            xml: &send.xml,
        };
        let mode = self.config.relay.mode;
        match bir::send(pipe, &job, &self.config.relay).await {
            Ok(sent) => {
                let status = if mode == bir::Mode::DryRun { "dry_run" } else { "sent" };
                let mut claims = json!({
                    "iss": self.config.issuer,
                    "sub": send.submission_id,
                    "account_id": send.account_id,
                    "tin": send.tin.to_string(),
                    "form_type": send.form_type,
                    "period": send.period,
                    "filename": send.filename,
                    "xml_sha256": bir::hex(&crypto::sha256(&send.xml)),
                    "payload_sha256": sent.payload_sha256,
                    "remote_path": sent.remote_path,
                    "mode": mode.as_str(),
                    "status": status,
                    "iat": OffsetDateTime::now_utc().unix_timestamp(),
                });
                if let Some(jti) = send.jti {
                    claims["jti"] = json!(jti);
                }
                let receipt = jws::sign(&self.config.receipt_key, RECEIPT_TYP, &claims);
                tracing::info!(submission = %send.submission_id, status, remote_path = %sent.remote_path, "sent");
                Control::Result { status: status.to_owned(), error: None, receipt: Some(Value::String(receipt)) }
            }
            Err(SendError::NotSent(reason)) => {
                tracing::warn!(submission = %send.submission_id, %reason, "not sent");
                failed(reason)
            }
            Err(SendError::MaybeSent(reason)) => {
                tracing::error!(submission = %send.submission_id, %reason, "may have reached BIR");
                let error = format!(
                    "{reason}. BIR may have received this return; check for its confirmation email before sending again."
                );
                Control::Result { status: "unknown".to_owned(), error: Some(error), receipt: None }
            }
        }
    }
}

struct Send {
    submission_id: Uuid,
    account_id: Uuid,
    tin: Tin,
    form_type: String,
    period: String,
    filename: String,
    xml: Vec<u8>,
    jti: Option<Uuid>,
}

fn failed(reason: String) -> Control {
    Control::Result { status: "failed".to_owned(), error: Some(reason), receipt: None }
}

/// The Sender's routes: `/healthz`, `/internal/public`, `/internal/relay`.
pub fn router(sender: Arc<Sender>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/internal/public", get(public))
        .route("/internal/relay", get(relay_job))
        .with_state(sender)
}

fn authorized(sender: &Sender, headers: &HeaderMap) -> bool {
    let presented = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    // Compare hashes so the time taken says nothing about the secret.
    crypto::hash(presented) == crypto::hash(&sender.config.secret)
}

async fn public(State(sender): State<Arc<Sender>>, headers: HeaderMap) -> Response {
    if !authorized(&sender, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(sender.public_info()).into_response()
}

async fn relay_job(State(sender): State<Arc<Sender>>, headers: HeaderMap, upgrade: WebSocketUpgrade) -> Response {
    if !authorized(&sender, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    upgrade.max_message_size(crate::submissions::BODY_LIMIT).on_upgrade(move |socket| run_job(sender, socket))
}

async fn run_job(sender: Arc<Sender>, mut socket: WebSocket) {
    let job = match socket.recv().await {
        Some(Ok(Message::Text(text))) => serde_json::from_str::<Job>(&text).ok(),
        _ => None,
    };
    let (to_app, from_server) = mpsc::channel(64);
    let (to_server, from_app) = mpsc::channel(64);
    let pump = tokio::spawn(relay::pump_socket(socket, from_server, to_server));
    let pipe = Pipe::new(to_app, from_app);
    let result = match job {
        Some(job) => sender.handle(&pipe, job).await,
        None => failed("the Sender did not understand the job".into()),
    };
    pipe.finish(result).await;
    drop(pipe);
    let _ = pump.await;
}

/// Serves the Sender on `listener` until the process ends.
pub async fn serve(listener: tokio::net::TcpListener, sender: Arc<Sender>) -> anyhow::Result<()> {
    axum::serve(listener, router(sender)).await?;
    Ok(())
}
