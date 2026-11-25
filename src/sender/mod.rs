//! The Sender: the only part of the server that holds BIR's passphrases,
//! the SFTP login and the HPKE private key. It runs as its own process
//! (`buwiz-sender`) with its own settings, reachable only by the
//! Gatekeeper on a private network.
//!
//! For each send the Gatekeeper opens `GET /internal/relay` (WebSocket,
//! `Authorization: Bearer $SENDER_SECRET`), sends one [`Job`] as a text
//! frame, then bridges the app's relay pipe to it frame for frame. The
//! Sender runs the send through the pipe (see `crate::bir`) and ends with
//! the pipe's `result`, carrying a receipt it signed.
//!
//! The BIR relay is behind the `bir` cargo feature. Without it the Sender
//! still checks tickets and opens envelopes, then answers every send with
//! `failed` and [`NOT_BUILT_IN`]: nothing reaches BIR.
//!
//! A v2 job carries the app's ticket and sealed envelope. The Sender checks
//! the ticket (signature, expiry, single use), opens the envelope, and
//! sends only when the XML's SHA-256, TIN, form and period are the
//! ticket's. The file name comes from the ticket. A v1 job's XML must name
//! the job's TIN, form and period too (the Gatekeeper checked it against
//! the file name when the return came; this covers one stored before).
//!
//! A send for a test TIN (root `000-000-000`, [`Tin::is_test`]) is always a
//! dry run, whatever `BIR_RELAY_MODE` says: once the return is tied to its
//! ticket's TIN (v2) or to the job's (v1), the Sender hands the BIR code
//! settings in `dry_run` for that one send, so no dispatcher login and no
//! upload stream can happen. Its receipt says `dry_run`, signed with the
//! usual receipt key, and so does the job's `sender` frame. This is the
//! guard: the Gatekeeper lets a test TIN through its gate like any other.

pub mod identity;
mod outcome;

pub use outcome::{Mode, NOT_BUILT_IN, RelaySettings, SendError, Sent};

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

use crate::account::tin::Tin;
use crate::base::{config, crypto};
use crate::filing::envelope::{Envelope, SealingKey};
use crate::filing::jws;
use crate::filing::submissions::parse_filename;
use crate::filing::tickets;
use crate::relay::{self, Control, Pipe};

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
    pub relay: RelaySettings,
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
        let throwaway_ok = relay.mode == Mode::DryRun;
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
        let throwaway_ok = relay.mode == Mode::DryRun;
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
                let Ok(tin) = Tin::parse_any(&tin_root, &branch_code) else {
                    return failed("the job's TIN is not valid".into());
                };
                let named = format!("{}{}", tin.root(), tin.branch());
                let expected =
                    identity::Expected { tin: &named, form: &form_type, period: &period, named_by: "file name" };
                if let Err(reason) = identity::check_against(&xml, &expected) {
                    return mismatch("file name", &reason);
                }
                // The XML now names the job's TIN; a file name that names a
                // test TIN makes it a dry run too.
                let test = tin.is_test() || names_a_test_tin(&filename);
                let mode = self.mode_for(test);
                let send = Send { submission_id, account_id, tin, form_type, period, filename, xml, jti: None, mode };
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
                if crypto::hex(&crypto::sha256(&xml)) != claims.payload_sha256 {
                    return failed("the sealed return is not the one the ticket names".into());
                }
                if let Err(reason) = identity::check(&xml, &claims) {
                    return mismatch("ticket", &reason);
                }
                let Ok(tin) = Tin::parse_any(&claims.tin[..9], &claims.tin[9..]) else {
                    return failed("the ticket's TIN is not valid".into());
                };
                // The signed ticket's TIN, which the XML was just tied to.
                let mode = self.mode_for(tin.is_test());
                let send = Send {
                    submission_id,
                    account_id,
                    tin,
                    filename: claims.filename(),
                    form_type: claims.form,
                    period: claims.period,
                    xml,
                    jti: Some(claims.jti),
                    mode,
                };
                self.send(pipe, send).await
            }
        }
    }

    /// The mode of one send: `dry_run` for a test TIN's, whatever
    /// `BIR_RELAY_MODE` says; the Sender's own mode for every other.
    fn mode_for(&self, test_tin: bool) -> Mode {
        if test_tin { Mode::DryRun } else { self.config.relay.mode }
    }

    /// The mode the job's `sender` frame says, before the job runs: the
    /// mode [`Sender::handle`] sends it in, read from what the job names (a
    /// v2 ticket's TIN once its signature checks). A job that names a test
    /// TIN but fails its checks sends nothing at all.
    pub fn job_mode(&self, job: &Job) -> Mode {
        let test = match job {
            Job::V1 { tin_root, branch_code, filename, .. } => {
                Tin::parse_any(tin_root, branch_code).is_ok_and(|tin| tin.is_test()) || names_a_test_tin(filename)
            }
            Job::V2 { ticket, .. } => {
                let now = OffsetDateTime::now_utc().unix_timestamp();
                tickets::verify(&self.config.ticket_keys, ticket, now)
                    .is_ok_and(|claims| claims.tin.get(..9) == Some(crate::account::tin::TEST_ROOT))
            }
        };
        self.mode_for(test)
    }

    /// Hands the return to the BIR relay, in the send's own mode: a dry run
    /// gets settings with no dispatcher, upload host or lab server at all.
    #[cfg(feature = "bir")]
    async fn deliver(&self, pipe: &Pipe, send: &Send) -> Result<Sent, SendError> {
        let job = crate::bir::Job {
            tin_root: send.tin.root(),
            form_type: &send.form_type,
            filename: &send.filename,
            xml: &send.xml,
        };
        if send.mode == self.config.relay.mode {
            return crate::bir::send(pipe, &job, &self.config.relay).await;
        }
        let relay = &self.config.relay;
        let dry_run = crate::bir::Settings {
            mode: Mode::DryRun,
            iaf_passphrase: relay.iaf_passphrase.clone(),
            dispatcher_passphrase: Zeroizing::new(String::new()),
            dispatchers: Vec::new(),
            sftp_hosts: Vec::new(),
            pinned_keys: Vec::new(),
            lab: None,
            tls: relay.tls.clone(),
            sftp_timeout: relay.sftp_timeout,
        };
        crate::bir::send(pipe, &job, &dry_run).await
    }

    /// Built without the BIR relay: nothing is sent.
    #[cfg(not(feature = "bir"))]
    async fn deliver(&self, _pipe: &Pipe, _send: &Send) -> Result<Sent, SendError> {
        Err(SendError::NotSent(NOT_BUILT_IN.to_owned()))
    }

    async fn send(&self, pipe: &Pipe, send: Send) -> Control {
        let mode = send.mode;
        if mode != self.config.relay.mode {
            tracing::info!(submission = %send.submission_id, "test TIN: dry run, not sent to BIR");
        }
        match self.deliver(pipe, &send).await {
            Ok(sent) => {
                let status = if mode == Mode::DryRun { "dry_run" } else { "sent" };
                let mut claims = json!({
                    "iss": self.config.issuer,
                    "sub": send.submission_id,
                    "account_id": send.account_id,
                    "tin": send.tin.to_string(),
                    "form_type": send.form_type,
                    "period": send.period,
                    "filename": send.filename,
                    "xml_sha256": crypto::hex(&crypto::sha256(&send.xml)),
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
                // Ids only: the remote path is the file name, which holds the TIN and
                // the taxpayer's email. The receipt carries it.
                tracing::info!(submission = %send.submission_id, status, "sent");
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
    /// [`Sender::mode_for`] this send.
    mode: Mode,
}

/// Whether an eBIRForms file name names a test TIN.
fn names_a_test_tin(filename: &str) -> bool {
    parse_filename(filename).is_ok_and(|name| name.tin.is_test())
}

fn failed(reason: String) -> Control {
    Control::Result { status: "failed".to_owned(), error: Some(reason), receipt: None }
}

/// A return that is not the one its file name or ticket names: final, so
/// it carries its code in brackets and the app does not retry it.
fn mismatch(named_by: &str, reason: &str) -> Control {
    failed(format!("The return does not match its {named_by}: {reason}. (return_mismatch)"))
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
    upgrade.max_message_size(crate::filing::submissions::BODY_LIMIT).on_upgrade(move |socket| run_job(sender, socket))
}

/// The Sender's first text frame on a job's socket: the job's relay mode
/// ([`Sender::job_mode`]: `dry_run` for a test TIN's), for the
/// Gatekeeper's record of the try. The Gatekeeper never forwards it to
/// the app; an app would ignore it anyway (an unknown `type`).
pub fn sender_frame(mode: Mode) -> String {
    json!({ "type": "sender", "mode": mode.as_str() }).to_string()
}

async fn run_job(sender: Arc<Sender>, mut socket: WebSocket) {
    let job = match socket.recv().await {
        Some(Ok(Message::Text(text))) => serde_json::from_str::<Job>(&text).ok(),
        _ => None,
    };
    let mode = job.as_ref().map_or(sender.config.relay.mode, |job| sender.job_mode(job));
    let _ = socket.send(Message::Text(sender_frame(mode).into())).await;
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

#[cfg(all(test, not(feature = "bir")))]
mod tests {
    use super::*;

    fn config() -> SenderConfig {
        SenderConfig {
            relay: RelaySettings { mode: Mode::Live },
            sealing_key: SealingKey::from_bytes(&[7; 32]).unwrap(),
            receipt_key: SigningKey::from_bytes(&[8; 32]),
            ticket_keys: vec![SigningKey::from_bytes(&[9; 32]).verifying_key()],
            secret: Zeroizing::new("x".repeat(32)),
            issuer: "http://localhost:3008".into(),
        }
    }

    /// A 1601C return for `tin` (root, branch) in 2025.
    fn return_1601c(tin: [&str; 4]) -> String {
        let line = |key: &str, value: &str| format!("<div>frm1601c:{key}={value}frm1601c:{key}=</div>");
        [("txtYear", "2025"), ("txtTIN1", tin[0]), ("txtTIN2", tin[1]), ("txtTIN3", tin[2]), ("txtBranchCode", tin[3])]
            .into_iter()
            .map(|(key, value)| line(key, value))
            .collect()
    }

    fn v1_job(xml: &str) -> Job {
        Job::V1 {
            submission_id: Uuid::nil(),
            account_id: Uuid::nil(),
            tin_root: "123456789".into(),
            branch_code: "00000".into(),
            form_type: "1601Cv2018".into(),
            period: "062025".into(),
            filename: "12345678900000-1601Cv2018-062025#juan@example.com#.xml".into(),
            xml: STANDARD.encode(xml),
        }
    }

    /// What the Sender answers `job`, and whether it opened anything
    /// through the pipe.
    async fn answer(job: Job) -> (Control, bool) {
        let (to_app, mut from_server) = mpsc::channel(8);
        let (_to_server, from_app) = mpsc::channel(8);
        let pipe = Pipe::new(to_app, from_app);
        let result = Sender::new(config()).handle(&pipe, job).await;
        drop(pipe);
        (result, from_server.try_recv().is_ok())
    }

    #[test]
    fn the_sender_frame_says_the_mode_and_is_no_control() {
        for (mode, json) in [
            (Mode::Live, r#"{"mode":"live","type":"sender"}"#),
            (Mode::Lab, r#"{"mode":"lab","type":"sender"}"#),
            (Mode::DryRun, r#"{"mode":"dry_run","type":"sender"}"#),
        ] {
            let frame = sender_frame(mode);
            let value: Value = serde_json::from_str(&frame).unwrap();
            assert_eq!(value, serde_json::from_str::<Value>(json).unwrap());
            // Every Control reader ignores it.
            assert_eq!(relay::Frame::decode_text(&frame), None);
        }
    }

    #[tokio::test]
    async fn without_the_bir_feature_every_send_fails_unsent() {
        // A v1 return that matches its job (it used to be `<return/>`, which
        // the Sender now refuses before it gets this far).
        let (result, opened) = answer(v1_job(&return_1601c(["123", "456", "789", "00000"]))).await;
        assert_eq!(
            result,
            Control::Result { status: "failed".into(), error: Some(NOT_BUILT_IN.into()), receipt: None }
        );
        assert!(!opened, "nothing was opened through the pipe");
    }

    /// A v1 job and a v2 one (ticket, sealed return) for the 1601C return
    /// of `tin` (14 digits), whose ticket is signed with the trusted key.
    fn jobs_for(tin: &str) -> (Job, Job) {
        let xml = return_1601c([&tin[..3], &tin[3..6], &tin[6..9], &tin[9..]]);
        let mut v1 = v1_job(&xml);
        if let Job::V1 { tin_root, filename, .. } = &mut v1 {
            *tin_root = tin[..9].into();
            *filename = format!("{tin}-1601Cv2018-062025#juan@example.com#.xml");
        }
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let jti = Uuid::new_v4();
        let claims = tickets::TicketClaims {
            iss: "http://localhost:3008".into(),
            aud: tickets::AUDIENCE.into(),
            sub: Uuid::nil(),
            account_id: Uuid::nil(),
            device_id: Uuid::nil(),
            tin: tin.into(),
            form: "1601Cv2018".into(),
            period: "062025".into(),
            payload_sha256: crypto::hex(&crypto::sha256(xml.as_bytes())),
            email: "juan@example.com".into(),
            jti,
            iat: now,
            exp: now + 60,
        };
        let ticket = tickets::sign(&SigningKey::from_bytes(&[9; 32]), &claims);
        let sealing = SealingKey::from_bytes(&[7; 32]).unwrap();
        let envelope =
            crate::filing::envelope::seal(&sealing.public_bytes(), &sealing.kid(), &jti.to_string(), xml.as_bytes())
                .unwrap();
        (v1, Job::V2 { submission_id: Uuid::nil(), account_id: Uuid::nil(), ticket, envelope })
    }

    /// In `live`, a test TIN's job is a dry run, v1 or v2, and its `sender`
    /// frame says so; a real TIN's keeps the Sender's mode. A ticket that
    /// does not verify says the Sender's mode (it sends nothing anyway).
    #[test]
    fn a_test_tins_job_is_a_dry_run_in_live_mode() {
        let sender = Sender::new(config());
        for (tin, mode) in
            [("00000000000000", Mode::DryRun), ("00000000000017", Mode::DryRun), ("12345678900000", Mode::Live)]
        {
            let (v1, v2) = jobs_for(tin);
            assert_eq!(sender.job_mode(&v1), mode, "{tin}");
            assert_eq!(sender.job_mode(&v2), mode, "{tin}");
            assert_eq!(sender.mode_for(tin.starts_with("000000000")), mode);
        }
        let (_, mut forged) = jobs_for("00000000000000");
        if let Job::V2 { ticket, .. } = &mut forged {
            let claims = tickets::verify(&sender.config.ticket_keys, ticket, 0).unwrap();
            *ticket = tickets::sign(&SigningKey::from_bytes(&[1; 32]), &claims);
        }
        assert_eq!(sender.job_mode(&forged), Mode::Live);
        // A real TIN's job whose file name names a test TIN: a dry run.
        let (mut v1, _) = jobs_for("12345678900000");
        if let Job::V1 { filename, .. } = &mut v1 {
            *filename = "00000000000000-1601Cv2018-062025#juan@example.com#.xml".into();
        }
        assert_eq!(sender.job_mode(&v1), Mode::DryRun);
    }

    /// Without the `bir` feature a test TIN's send is not refused for its
    /// TIN any more; it ends as every send there does, unsent.
    #[tokio::test]
    async fn a_test_tin_is_not_refused_for_its_tin() {
        let unsent = Control::Result { status: "failed".into(), error: Some(NOT_BUILT_IN.into()), receipt: None };
        let (v1, v2) = jobs_for("00000000000000");
        for job in [v1, v2] {
            assert_eq!(answer(job).await, (unsent.clone(), false));
        }
    }

    #[tokio::test]
    async fn a_v1_return_for_another_tin_is_refused_before_sending() {
        for (xml, words) in [
            (return_1601c(["123", "456", "780", "00000"]), "the TIN in the return is not the file name's TIN"),
            (return_1601c(["123", "456", "789", "00001"]), "the TIN in the return is not the file name's TIN"),
            ("<return/>".to_owned(), "the return has no form fields"),
        ] {
            let (result, opened) = answer(v1_job(&xml)).await;
            // Final, as the relay protocol says: a code in brackets.
            let error = format!("The return does not match its file name: {words}. (return_mismatch)");
            assert_eq!(result, Control::Result { status: "failed".into(), error: Some(error), receipt: None });
            assert!(!opened);
        }
    }
}

/// With the BIR relay: a Sender in `live` whose dispatcher and upload
/// server are loopback listeners that count every connection.
#[cfg(all(test, feature = "bir"))]
mod bir_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::bir::{DispatcherTarget, Settings};

    /// A listener that counts the connections it gets and drops each.
    async fn counting() -> (u16, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let count = Arc::new(AtomicUsize::new(0));
        let counted = count.clone();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                drop(socket);
            }
        });
        (port, count)
    }

    fn live(dispatcher: u16) -> SenderConfig {
        SenderConfig {
            relay: Settings {
                mode: Mode::Live,
                iaf_passphrase: Zeroizing::new("a made-up IAF passphrase".into()),
                dispatcher_passphrase: Zeroizing::new("a made-up dispatcher passphrase".into()),
                dispatchers: vec![DispatcherTarget { host: "127.0.0.1".into(), port: dispatcher, tls: false }],
                sftp_hosts: vec!["127.0.0.1".into()],
                pinned_keys: vec!["SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into()],
                lab: None,
                tls: Settings::tls_config(),
                sftp_timeout: std::time::Duration::from_secs(3),
            },
            sealing_key: SealingKey::from_bytes(&[7; 32]).unwrap(),
            receipt_key: SigningKey::from_bytes(&[8; 32]),
            ticket_keys: vec![SigningKey::from_bytes(&[9; 32]).verifying_key()],
            secret: Zeroizing::new("x".repeat(32)),
            issuer: "http://localhost:3008".into(),
        }
    }

    fn return_1601c(tin: &str) -> String {
        let line = |key: &str, value: &str| format!("<div>frm1601c:{key}={value}frm1601c:{key}=</div>");
        [
            ("txtYear", "2025"),
            ("txtTIN1", &tin[..3]),
            ("txtTIN2", &tin[3..6]),
            ("txtTIN3", &tin[6..9]),
            ("txtBranchCode", &tin[9..]),
        ]
        .into_iter()
        .map(|(key, value)| line(key, value))
        .collect()
    }

    /// A v1 job and a v2 one for `tin` (14 digits).
    fn jobs_for(tin: &str) -> [Job; 2] {
        let xml = return_1601c(tin);
        let filename = format!("{tin}-1601Cv2018-062025#juan@example.com#.xml");
        let v1 = Job::V1 {
            submission_id: Uuid::nil(),
            account_id: Uuid::nil(),
            tin_root: tin[..9].into(),
            branch_code: tin[9..].into(),
            form_type: "1601Cv2018".into(),
            period: "062025".into(),
            filename,
            xml: STANDARD.encode(&xml),
        };
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let jti = Uuid::new_v4();
        let claims = tickets::TicketClaims {
            iss: "http://localhost:3008".into(),
            aud: tickets::AUDIENCE.into(),
            sub: Uuid::nil(),
            account_id: Uuid::nil(),
            device_id: Uuid::nil(),
            tin: tin.into(),
            form: "1601Cv2018".into(),
            period: "062025".into(),
            payload_sha256: crypto::hex(&crypto::sha256(xml.as_bytes())),
            email: "juan@example.com".into(),
            jti,
            iat: now,
            exp: now + 60,
        };
        let ticket = tickets::sign(&SigningKey::from_bytes(&[9; 32]), &claims);
        let sealing = SealingKey::from_bytes(&[7; 32]).unwrap();
        let envelope =
            crate::filing::envelope::seal(&sealing.public_bytes(), &sealing.kid(), &jti.to_string(), xml.as_bytes())
                .unwrap();
        [v1, Job::V2 { submission_id: Uuid::nil(), account_id: Uuid::nil(), ticket, envelope }]
    }

    /// Runs `job` through a pipe to an app that may open the dispatcher.
    async fn run(sender: &Sender, job: Job, dispatcher: u16) -> Control {
        let (to_app, from_server) = mpsc::channel(64);
        let (to_server, from_app) = mpsc::channel(64);
        let allowed = vec![("127.0.0.1".to_owned(), dispatcher)];
        let app = tokio::spawn(async move { crate::relay::client::serve(to_server, from_server, &allowed).await });
        let pipe = Pipe::new(to_app, from_app);
        let result = sender.handle(&pipe, job).await;
        pipe.finish(result.clone()).await;
        drop(pipe);
        let _ = app.await;
        result
    }

    /// A test TIN in `live`: a dry run, v1 and v2, with a `dry_run` receipt
    /// under the receipt key, and the dispatcher never hears of it.
    #[tokio::test]
    async fn a_test_tin_is_a_dry_run_in_live_mode() {
        let (dispatcher, contacted) = counting().await;
        let sender = Sender::new(live(dispatcher));
        for job in jobs_for("00000000000000") {
            assert_eq!(sender.job_mode(&job), Mode::DryRun);
            let Control::Result { status, error, receipt: Some(Value::String(receipt)) } =
                run(&sender, job, dispatcher).await
            else {
                panic!("no receipt");
            };
            assert_eq!((status.as_str(), error), ("dry_run", None));
            let keys = [sender.config.receipt_key.verifying_key()];
            let claims = jws::verify(&keys, RECEIPT_TYP, &receipt).expect("signed with the receipt key");
            assert_eq!((&claims["mode"], &claims["status"]), (&json!("dry_run"), &json!("dry_run")), "{claims}");
            assert_eq!(claims["tin"], "000-000-000-00000");
            assert_eq!(claims["payload_sha256"].as_str().map(str::len), Some(64));
        }
        assert_eq!(contacted.load(Ordering::SeqCst), 0, "no dispatcher login");
    }

    /// A real TIN in `live` goes to the dispatcher as before (this one
    /// hangs up, so the send fails unsent).
    #[tokio::test]
    async fn a_real_tin_in_live_mode_is_unaffected() {
        let (dispatcher, contacted) = counting().await;
        let sender = Sender::new(live(dispatcher));
        for job in jobs_for("12345678900000") {
            assert_eq!(sender.job_mode(&job), Mode::Live);
            let before = contacted.load(Ordering::SeqCst);
            let Control::Result { status, receipt, .. } = run(&sender, job, dispatcher).await else {
                panic!("no result");
            };
            assert_eq!((status.as_str(), receipt), ("failed", None));
            assert!(contacted.load(Ordering::SeqCst) > before, "the dispatcher was asked");
        }
    }
}
