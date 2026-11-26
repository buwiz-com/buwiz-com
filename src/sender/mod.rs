//! The Sender: the only part of the server that holds BIR's passphrases,
//! the SFTP login and the HPKE private key. It runs as its own process
//! (`buwiz-sender`) with its own settings, reachable only by the
//! Gatekeeper on a private network.
//!
//! For each send the Gatekeeper opens `GET /internal/relay` (WebSocket,
//! `Authorization: Bearer $SENDER_SECRET`; `/internal/relay/reserved` for a
//! test TIN's job, below), sends one [`Job`] as a text
//! frame, then bridges the app's relay pipe to it frame for frame. The
//! Sender runs the send through the pipe (see `crate::bir`) and ends with
//! the pipe's `result`, carrying a receipt it signed.
//!
//! The BIR relay is behind the `bir` cargo feature. Without it the Sender
//! still checks tickets and opens envelopes, then answers every send with
//! `failed` and [`NOT_BUILT_IN`]: nothing reaches BIR.
//!
//! A v2 job carries the app's ticket and sealed envelope. The Sender checks
//! the ticket (signature, expiry, single use), reads its TIN (14 digits),
//! opens the envelope, and sends only when the XML's SHA-256, TIN, form and
//! period are the ticket's. The file name comes from the ticket. A v1 job's
//! file name must name exactly the job's TIN, and its XML the job's TIN,
//! form and period (the Gatekeeper checked the XML against the file name
//! when the return came; this covers one stored before, or a stored job
//! that disagrees with its file name). A job that fails either is refused
//! as a mismatch, and nothing is sent.
//!
//! A send for a test TIN (a reserved root, [`Tin::is_test`]) is always a
//! dry run, whatever `BIR_RELAY_MODE` says, `dry_run` included: once the
//! return is tied to its ticket's TIN (v2) or to the job's (v1), [`plan`]
//! decides from that TIN alone, and the BIR code gets blanked settings
//! ([`Given::Blanked`]: no dispatcher, upload host, pinned key or lab
//! server) for that one send, so no dispatcher login and no upload stream
//! can happen. Its receipt says `dry_run`, signed with the usual receipt
//! key, and so does the job's `sender` frame. This is the guard: the
//! Gatekeeper lets a test TIN through its gate like any other.
//!
//! The Gatekeeper has two more guards, for a Sender older than it that
//! knows fewer reserved roots. It sends a test TIN's job only to
//! [`RESERVED_RELAY_ROUTE`], which an older Sender does not have: the
//! connection fails and the job never leaves the Gatekeeper. This Sender
//! refuses a test TIN's job on [`RELAY_ROUTE`] and any other job on the
//! reserved one. On either route its `sender` frame says `dry_run` only
//! for a job that reads as a test TIN's.
//! And for a test TIN's job the Gatekeeper wants the Sender's first frame
//! to be the `sender` frame saying `dry_run`, and after it only a `result`
//! that is `dry_run` (with a receipt whose mode and status say `dry_run`)
//! or `failed`.
//! Anything else ends the try `failed`, with nothing of it sent on to the
//! app, so no stream to the BIR is opened.

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
use futures_util::future::BoxFuture;
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

/// How one send is made: its mode, and the settings the BIR code gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub mode: Mode,
    pub given: Given,
}

/// The settings the BIR code gets for one send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Given {
    /// The relay's own (`BIR_*`), in its own mode.
    Relay,
    /// `dry_run` with no dispatcher, no upload host, no pinned key and no
    /// lab server: nothing to contact.
    Blanked,
}

/// The plan of one send: for a test TIN's, a dry run with blanked settings
/// whatever the relay's own mode (`dry_run` too); for every other, the
/// relay's own mode and settings.
pub fn plan(relay_mode: Mode, test_tin: bool) -> Plan {
    if test_tin {
        Plan { mode: Mode::DryRun, given: Given::Blanked }
    } else {
        Plan { mode: relay_mode, given: Given::Relay }
    }
}

/// One send, as [`Delivery`] gets it.
pub struct Order<'a> {
    pub tin: &'a Tin,
    pub form_type: &'a str,
    pub filename: &'a str,
    pub xml: &'a [u8],
    pub plan: Plan,
}

/// Hands one send to the BIR code, with the settings its plan says.
pub trait Delivery: std::marker::Send + Sync {
    fn deliver<'a>(
        &'a self,
        pipe: &'a Pipe,
        relay: &'a RelaySettings,
        order: Order<'a>,
    ) -> BoxFuture<'a, Result<Sent, SendError>>;
}

/// The BIR relay.
#[cfg(feature = "bir")]
struct Bir;

#[cfg(feature = "bir")]
impl Delivery for Bir {
    fn deliver<'a>(
        &'a self,
        pipe: &'a Pipe,
        relay: &'a RelaySettings,
        order: Order<'a>,
    ) -> BoxFuture<'a, Result<Sent, SendError>> {
        Box::pin(async move {
            let job = crate::bir::Job {
                tin_root: order.tin.root(),
                form_type: order.form_type,
                filename: order.filename,
                xml: order.xml,
            };
            match order.plan.given {
                Given::Relay => crate::bir::send(pipe, &job, relay).await,
                Given::Blanked => crate::bir::send(pipe, &job, &blanked(relay)).await,
            }
        })
    }
}

/// [`Given::Blanked`]: the relay's settings with nothing left to contact.
/// It keeps the IAF passphrase: `bir::send` encrypts the return with it
/// before it looks at the mode, to compute the `payload_sha256` that the
/// dry run's receipt carries. The encryption happens in this process, and
/// in a dry run nothing leaves it. A default-build test reads this body
/// (`blanked_keeps_nothing_to_contact`), since the type is private.
#[cfg(feature = "bir")]
fn blanked(relay: &RelaySettings) -> RelaySettings {
    crate::bir::Settings {
        mode: Mode::DryRun,
        iaf_passphrase: relay.iaf_passphrase.clone(),
        dispatcher_passphrase: Zeroizing::new(String::new()),
        dispatchers: Vec::new(),
        sftp_hosts: Vec::new(),
        pinned_keys: Vec::new(),
        lab: None,
        tls: relay.tls.clone(),
        sftp_timeout: relay.sftp_timeout,
    }
}

/// Built without the BIR relay: nothing is sent.
#[cfg(not(feature = "bir"))]
struct NotBuilt;

#[cfg(not(feature = "bir"))]
impl Delivery for NotBuilt {
    fn deliver<'a>(
        &'a self,
        _pipe: &'a Pipe,
        _relay: &'a RelaySettings,
        _order: Order<'a>,
    ) -> BoxFuture<'a, Result<Sent, SendError>> {
        Box::pin(async { Err(SendError::NotSent(NOT_BUILT_IN.to_owned())) })
    }
}

pub struct Sender {
    config: SenderConfig,
    /// Tickets already used, with their expiry. A ticket is useless after
    /// it expires, so expired entries are dropped.
    spent: Mutex<HashMap<Uuid, i64>>,
    delivery: Arc<dyn Delivery>,
}

impl Sender {
    pub fn new(config: SenderConfig) -> Arc<Self> {
        #[cfg(feature = "bir")]
        let delivery = Arc::new(Bir);
        #[cfg(not(feature = "bir"))]
        let delivery = Arc::new(NotBuilt);
        Arc::new(Self { config, spent: Mutex::default(), delivery })
    }

    /// A Sender that hands its sends to `delivery`.
    #[cfg(test)]
    fn with_delivery(config: SenderConfig, delivery: Arc<dyn Delivery>) -> Arc<Self> {
        Arc::new(Self { config, spent: Mutex::default(), delivery })
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
                // The file name must name exactly the job's TIN: one that
                // names another (a test TIN's included) is refused, never
                // sent and never a dry run.
                match parse_filename(&filename) {
                    Ok(name) if name.tin == tin => {}
                    Ok(_) => return mismatch("job", "the file name's TIN is not the job's TIN"),
                    Err(_) => return mismatch("job", "the file name is not an eBIRForms file name"),
                }
                let named = fourteen_digits(&tin);
                let expected = identity::Expected { tin: &named, form: &form_type, period: &period, named_by: "job" };
                if let Err(reason) = identity::check_against(&xml, &expected) {
                    return mismatch("job", &reason);
                }
                // The job's TIN, which the file name and the XML were just
                // tied to.
                let plan = plan(self.config.relay.mode, tin.is_test());
                let send = Send { submission_id, account_id, tin, form_type, period, filename, xml, jti: None, plan };
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
                let Some(tin) = ticket_tin(&claims.tin) else {
                    return failed("the ticket's TIN is not valid".into());
                };
                let xml = match self.config.sealing_key.open(&envelope, &claims.jti.to_string()) {
                    Ok(xml) => xml,
                    Err(reason) => return failed(reason.into()),
                };
                if crypto::hex(&crypto::sha256(&xml)) != claims.payload_sha256 {
                    return failed("the sealed return is not the one the ticket names".into());
                }
                let named = fourteen_digits(&tin);
                let expected =
                    identity::Expected { tin: &named, form: &claims.form, period: &claims.period, named_by: "ticket" };
                if let Err(reason) = identity::check_against(&xml, &expected) {
                    return mismatch("ticket", &reason);
                }
                // The signed ticket's TIN, which the XML was just tied to.
                let plan = plan(self.config.relay.mode, tin.is_test());
                let send = Send {
                    submission_id,
                    account_id,
                    filename: claims.filename(),
                    tin,
                    form_type: claims.form,
                    period: claims.period,
                    xml,
                    jti: Some(claims.jti),
                    plan,
                };
                self.send(pipe, send).await
            }
        }
    }

    /// The mode the job's `sender` frame says, before the job runs: the
    /// mode [`Sender::handle`] sends it in, read from the job's TIN (v1) or
    /// the ticket's (v2, once its signature checks). A job that fails its
    /// checks sends nothing at all.
    pub fn job_mode(&self, job: &Job) -> Mode {
        plan(self.config.relay.mode, self.is_test_job(job)).mode
    }

    /// Why a v2 job's ticket does not check (signature, expiry), if it
    /// does not. `None` for a v1 job.
    fn ticket_refusal(&self, job: &Job) -> Option<String> {
        let Job::V2 { ticket, .. } = job else {
            return None;
        };
        let now = OffsetDateTime::now_utc().unix_timestamp();
        tickets::verify(&self.config.ticket_keys, ticket, now).err().map(|error| error.to_string())
    }

    /// Whether `job` is a test TIN's: its TIN (v1) or its ticket's (v2,
    /// once its signature checks) has a reserved root.
    fn is_test_job(&self, job: &Job) -> bool {
        match job {
            Job::V1 { tin_root, branch_code, .. } => {
                Tin::parse_any(tin_root, branch_code).is_ok_and(|tin| tin.is_test())
            }
            Job::V2 { ticket, .. } => {
                let now = OffsetDateTime::now_utc().unix_timestamp();
                tickets::verify(&self.config.ticket_keys, ticket, now)
                    .ok()
                    .and_then(|claims| ticket_tin(&claims.tin))
                    .is_some_and(|tin| tin.is_test())
            }
        }
    }

    /// Whether `job` reads as a test TIN's, for its `sender` frame on the
    /// reserved route: [`Sender::is_test_job`], or a v2 job whose ticket
    /// does not check but names a test TIN (read unverified). Only the
    /// frame rests on the second: such a job is then refused with the
    /// ticket's reason and sends nothing.
    fn reads_as_test_job(&self, job: &Job) -> bool {
        self.is_test_job(job)
            || matches!(job, Job::V2 { ticket, .. } if jws::claims_unverified(ticket)
                .and_then(|claims| claims["tin"].as_str().and_then(ticket_tin))
                .is_some_and(|tin| tin.is_test()))
    }

    async fn send(&self, pipe: &Pipe, send: Send) -> Control {
        let mode = send.plan.mode;
        if send.plan.given == Given::Blanked {
            tracing::info!(submission = %send.submission_id, "test TIN: dry run, not sent to BIR");
        }
        let order = Order {
            tin: &send.tin,
            form_type: &send.form_type,
            filename: &send.filename,
            xml: &send.xml,
            plan: send.plan,
        };
        match self.delivery.deliver(pipe, &self.config.relay, order).await {
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
    /// [`plan`] for this send.
    plan: Plan,
}

/// A ticket's TIN: exactly 14 ASCII digits, root then branch. Anything
/// else (short, long, dashed, other digits) is `None`. Read by characters,
/// so no input can split a character.
fn ticket_tin(raw: &str) -> Option<Tin> {
    let root: String = raw.chars().take(9).collect();
    let branch: String = raw.chars().skip(9).collect();
    let tin = Tin::parse_any(&root, &branch).ok()?;
    (fourteen_digits(&tin) == raw).then_some(tin)
}

/// Root and branch, as a return and a ticket write them.
fn fourteen_digits(tin: &Tin) -> String {
    format!("{}{}", tin.root(), tin.branch())
}

fn failed(reason: String) -> Control {
    Control::Result { status: "failed".to_owned(), error: Some(reason), receipt: None }
}

/// A return that is not the one its job (v1) or ticket (v2) names: final,
/// so it carries its code in brackets and the app does not retry it.
fn mismatch(named_by: &str, reason: &str) -> Control {
    failed(format!("The return does not match its {named_by}: {reason}. (return_mismatch)"))
}

/// Where the Gatekeeper sends every job but a test TIN's.
pub const RELAY_ROUTE: &str = "/internal/relay";

/// Where the Gatekeeper sends a test TIN's job, and only that. A Sender
/// from before reserved roots has no such route, so the Gatekeeper cannot
/// connect and the job never leaves it.
pub const RESERVED_RELAY_ROUTE: &str = "/internal/relay/reserved";

/// The Sender's routes: `/healthz`, `/internal/public`, [`RELAY_ROUTE`]
/// and [`RESERVED_RELAY_ROUTE`].
pub fn router(sender: Arc<Sender>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/internal/public", get(public))
        .route(RELAY_ROUTE, get(relay_job))
        .route(RESERVED_RELAY_ROUTE, get(reserved_relay_job))
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
    take_job(sender, &headers, upgrade, false)
}

async fn reserved_relay_job(
    State(sender): State<Arc<Sender>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    take_job(sender, &headers, upgrade, true)
}

fn take_job(sender: Arc<Sender>, headers: &HeaderMap, upgrade: WebSocketUpgrade, reserved: bool) -> Response {
    if !authorized(&sender, headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    upgrade
        .max_message_size(crate::filing::submissions::BODY_LIMIT)
        .on_upgrade(move |socket| run_job(sender, socket, reserved))
}

/// What a job that came on the wrong route ends with: a test TIN's on
/// [`RELAY_ROUTE`], or another on [`RESERVED_RELAY_ROUTE`]. Nothing is sent.
fn wrong_route(reserved: bool) -> Control {
    if reserved {
        failed("the Sender takes only a test TIN's job on its reserved route".into())
    } else {
        failed("the Sender takes a test TIN's job only on its reserved route".into())
    }
}

/// The Sender's first text frame on a job's socket: the job's relay mode
/// ([`Sender::job_mode`]: `dry_run` for a test TIN's), for the
/// Gatekeeper's record of the try. The Gatekeeper never forwards it to
/// the app; an app would ignore it anyway (an unknown `type`).
pub fn sender_frame(mode: Mode) -> String {
    json!({ "type": "sender", "mode": mode.as_str() }).to_string()
}

/// Runs one job's socket. `reserved`: the job came on
/// [`RESERVED_RELAY_ROUTE`], so it must be a test TIN's; on
/// [`RELAY_ROUTE`] it must not be. A job on the wrong route is refused
/// before it runs.
async fn run_job(sender: Arc<Sender>, mut socket: WebSocket, reserved: bool) {
    let job = match socket.recv().await {
        Some(Ok(Message::Text(text))) => serde_json::from_str::<Job>(&text).ok(),
        _ => None,
    };
    // On the reserved route the `sender` frame says `dry_run` only for a
    // job that reads as a test TIN's, so a test job refused there (a v2 one
    // whose ticket does not check) ends with its own reason, not the
    // Gatekeeper's. Any other job gets the mode `plan` gives it.
    let mode = match &job {
        Some(job) if reserved && sender.reads_as_test_job(job) => Mode::DryRun,
        Some(job) => sender.job_mode(job),
        None => sender.config.relay.mode,
    };
    let _ = socket.send(Message::Text(sender_frame(mode).into())).await;
    let (to_app, from_server) = mpsc::channel(64);
    let (to_server, from_app) = mpsc::channel(64);
    let pump = tokio::spawn(relay::pump_socket(socket, from_server, to_server));
    let pipe = Pipe::new(to_app, from_app);
    // A test TIN's v2 job whose ticket does not check (so its TIN cannot
    // be read) ends with the ticket's reason, as on the plain route.
    let ticket_refused = job.as_ref().filter(|_| reserved).and_then(|job| sender.ticket_refusal(job));
    let result = match (job, ticket_refused) {
        (Some(_), Some(reason)) => failed(reason),
        (Some(job), None) if sender.is_test_job(&job) != reserved => {
            let (Job::V1 { submission_id, .. } | Job::V2 { submission_id, .. }) = &job;
            tracing::error!(submission = %submission_id, reserved, "a job came on the wrong route; not sent");
            wrong_route(reserved)
        }
        (Some(job), None) => sender.handle(&pipe, job).await,
        (None, _) => failed("the Sender did not understand the job".into()),
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
    use crate::account::tin::TEST_ROOTS;

    fn config() -> SenderConfig {
        config_in(Mode::Live)
    }

    fn config_in(mode: Mode) -> SenderConfig {
        SenderConfig {
            relay: RelaySettings { mode },
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

    /// A delivery that sends nothing: it records each send it is asked for
    /// (the TIN, 14 digits, and the plan) and answers as a send that went.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, Plan)>>);

    impl Delivery for Recorder {
        fn deliver<'a>(
            &'a self,
            _pipe: &'a Pipe,
            _relay: &'a RelaySettings,
            order: Order<'a>,
        ) -> BoxFuture<'a, Result<Sent, SendError>> {
            self.0.lock().unwrap().push((fourteen_digits(order.tin), order.plan));
            Box::pin(async { Ok(Sent { remote_path: "stub".into(), payload_sha256: "0".repeat(64) }) })
        }
    }

    /// What a Sender in `relay` mode, with a [`Recorder`], answers `job`,
    /// and what delivery was asked for.
    async fn asked(relay: Mode, job: Job) -> (Control, Vec<(String, Plan)>) {
        let recorder = Arc::new(Recorder::default());
        let sender = Sender::with_delivery(config_in(relay), recorder.clone());
        let (to_app, _from_server) = mpsc::channel(8);
        let (_to_server, from_app) = mpsc::channel(8);
        let pipe = Pipe::new(to_app, from_app);
        let result = sender.handle(&pipe, job).await;
        let asked = recorder.0.lock().unwrap().clone();
        (result, asked)
    }

    /// The receipt's `mode` and `status`, checked against the receipt key.
    fn receipt_says(result: &Control) -> (String, String) {
        let Control::Result { status, error: None, receipt: Some(Value::String(receipt)) } = result else {
            panic!("no receipt: {result:?}");
        };
        let keys = [SigningKey::from_bytes(&[8; 32]).verifying_key()];
        let claims = jws::verify(&keys, RECEIPT_TYP, receipt).expect("signed with the receipt key");
        assert_eq!(claims["status"], json!(status));
        (claims["mode"].as_str().unwrap().to_owned(), status.clone())
    }

    const BLANKED: Plan = Plan { mode: Mode::DryRun, given: Given::Blanked };

    /// Real roots that differ from a reserved one by one digit, and others.
    const NEAR_MISSES: &[&str] = &[
        "111111113",
        "111111115",
        "111111104",
        "111111214",
        "011111114",
        "211111114",
        "111111111",
        "000000001",
        "100000000",
        "000000010",
        "123456789",
    ];

    #[test]
    fn the_plan_of_a_test_send_is_blanked_in_every_mode() {
        for mode in [Mode::Live, Mode::Lab, Mode::DryRun] {
            assert_eq!(plan(mode, true), BLANKED);
            assert_eq!(plan(mode, false), Plan { mode, given: Given::Relay });
        }
    }

    /// `handle` in `live`, v1 and v2: every reserved root at three branches
    /// asks delivery for a dry run with blanked settings, and its receipt
    /// says `dry_run`; a real root, a near miss included, asks for `live`
    /// with the relay's own settings.
    #[tokio::test]
    async fn handle_asks_for_a_blanked_dry_run_for_a_test_tin_in_live_mode() {
        for branch in ["00000", "00001", "00017"] {
            for root in TEST_ROOTS {
                let tin = format!("{root}{branch}");
                let (v1, v2) = jobs_for(&tin);
                for job in [v1, v2] {
                    let (result, asked) = asked(Mode::Live, job).await;
                    assert_eq!(asked, vec![(tin.clone(), BLANKED)], "{tin}");
                    assert_eq!(receipt_says(&result), ("dry_run".into(), "dry_run".into()), "{tin}");
                }
            }
            for root in NEAR_MISSES {
                let tin = format!("{root}{branch}");
                let (v1, v2) = jobs_for(&tin);
                for job in [v1, v2] {
                    let (result, asked) = asked(Mode::Live, job).await;
                    assert_eq!(asked, vec![(tin.clone(), Plan { mode: Mode::Live, given: Given::Relay })], "{tin}");
                    assert_eq!(receipt_says(&result), ("live".into(), "sent".into()), "{tin}");
                }
            }
        }
    }

    /// In `dry_run` and `lab` too, a test send gets blanked settings; a
    /// real one keeps the relay's own mode and settings, as before.
    #[tokio::test]
    async fn a_test_send_gets_blanked_settings_in_dry_run_and_lab_too() {
        for relay in [Mode::DryRun, Mode::Lab] {
            for root in TEST_ROOTS {
                let tin = format!("{root}00000");
                let (v1, v2) = jobs_for(&tin);
                for job in [v1, v2] {
                    let (result, asked) = asked(relay, job).await;
                    assert_eq!(asked, vec![(tin.clone(), BLANKED)], "{relay:?} {tin}");
                    assert_eq!(receipt_says(&result).0, "dry_run");
                }
            }
            let tin = "12345678900000";
            let (v1, v2) = jobs_for(tin);
            for job in [v1, v2] {
                let (result, asked) = asked(relay, job).await;
                assert_eq!(asked, vec![(tin.to_owned(), Plan { mode: relay, given: Given::Relay })], "{relay:?}");
                assert_eq!(receipt_says(&result).0, relay.as_str());
            }
        }
    }

    /// A v1 job whose file name names another TIN than the job's, a test
    /// TIN's or a real one's, is refused as a mismatch: delivery is never
    /// asked, so it is neither sent nor a dry run. The same TIN written with
    /// the old 3-digit branch is the same TIN.
    #[tokio::test]
    async fn a_v1_file_name_must_name_the_jobs_tin() {
        let refused = |words: &str| Control::Result {
            status: "failed".into(),
            error: Some(format!("The return does not match its job: {words}. (return_mismatch)")),
            receipt: None,
        };
        let other = refused("the file name's TIN is not the job's TIN");
        let mut cases: Vec<(String, String, Control)> = Vec::new();
        for root in TEST_ROOTS {
            // A real job, a test TIN's file name, and the other way round.
            cases.push(("12345678900000".into(), format!("{root}00000"), other.clone()));
            cases.push((format!("{root}00000"), "12345678900000".into(), other.clone()));
            // The same test root at another branch.
            cases.push((format!("{root}00000"), format!("{root}00001"), other.clone()));
            for near in NEAR_MISSES {
                cases.push((format!("{near}00000"), format!("{root}00000"), other.clone()));
                cases.push((format!("{root}00000"), format!("{near}00000"), other.clone()));
            }
        }
        // One reserved root's job, the other's file name.
        cases.push(("00000000000000".into(), "11111111400000".into(), other.clone()));
        for (job_tin, named, answer) in cases {
            let (mut job, _) = jobs_for(&job_tin);
            if let Job::V1 { filename, .. } = &mut job {
                *filename = format!("{named}-1601Cv2018-062025#juan@example.com#.xml");
            }
            assert_eq!(asked(Mode::Live, job).await, (answer, vec![]), "{job_tin} named {named}");
        }
        // Not a file name at all.
        let (mut job, _) = jobs_for("11111111400000");
        if let Job::V1 { filename, .. } = &mut job {
            *filename = "return.xml".into();
        }
        let answer = refused("the file name is not an eBIRForms file name");
        assert_eq!(asked(Mode::Live, job).await, (answer, vec![]));
        // The same TIN, the old 3-digit branch: sent as the job's TIN says.
        for (tin, plan) in
            [("12345678900000", Plan { mode: Mode::Live, given: Given::Relay }), ("11111111400000", BLANKED)]
        {
            let (mut job, _) = jobs_for(tin);
            if let Job::V1 { filename, .. } = &mut job {
                *filename = format!("{}-1601Cv2018-062025#juan@example.com#.xml", &tin[..12]);
            }
            assert_eq!(asked(Mode::Live, job).await.1, vec![(tin.to_owned(), plan)], "{tin}");
        }
    }

    /// A v2 ticket whose TIN is short or not ASCII digits (here with a
    /// return that names the same, so only the TIN can refuse it) fails
    /// cleanly, and `job_mode` reads it as no test TIN.
    #[tokio::test]
    async fn a_short_or_non_ascii_ticket_tin_fails_cleanly() {
        let line = |key: &str, value: &str| format!("<div>frm1601c:{key}={value}frm1601c:{key}=</div>");
        for (tin, parts) in [
            ("123", ["1", "2", "3"]),
            ("11111111", ["111", "111", "11"]),
            ("12345678\u{e9}0000", ["123", "456", "78\u{e9}0000"]),
            ("111111114\u{e9}000", ["111", "111", "114\u{e9}000"]),
            ("111-111-114-00000", ["111-", "111-", "114-00000"]),
            ("111111114000000", ["111", "111", "114000000"]),
        ] {
            let xml: String =
                [("txtYear", "2025"), ("txtTIN1", parts[0]), ("txtTIN2", parts[1]), ("txtTIN3", parts[2])]
                    .into_iter()
                    .map(|(key, value)| line(key, value))
                    .collect();
            let job = ticketed(tin, &xml);
            assert_eq!(Sender::new(config()).job_mode(&job), Mode::Live, "{tin}");
            let answer = Control::Result {
                status: "failed".into(),
                error: Some("the ticket's TIN is not valid".into()),
                receipt: None,
            };
            assert_eq!(asked(Mode::Live, job).await, (answer, vec![]), "{tin}");
        }
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

    /// A v2 job whose ticket, signed with the trusted key, names `tin`,
    /// and whose sealed return is `xml`.
    fn ticketed(tin: &str, xml: &str) -> Job {
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
        Job::V2 { submission_id: Uuid::nil(), account_id: Uuid::nil(), ticket, envelope }
    }

    /// A v1 job and a v2 one (ticket, sealed return) for the 1601C return
    /// of `tin` (14 digits), whose ticket is signed with the trusted key.
    fn jobs_for(tin: &str) -> (Job, Job) {
        let xml = return_1601c([&tin[..3], &tin[3..6], &tin[6..9], &tin[9..]]);
        let mut v1 = v1_job(&xml);
        if let Job::V1 { tin_root, branch_code, filename, .. } = &mut v1 {
            *tin_root = tin[..9].into();
            *branch_code = tin[9..].into();
            *filename = format!("{tin}-1601Cv2018-062025#juan@example.com#.xml");
        }
        (v1, ticketed(tin, &xml))
    }

    /// In `live`, a test TIN's job is a dry run, v1 or v2, for every
    /// reserved root at any branch, and its `sender` frame says so; a real
    /// TIN's keeps the Sender's mode. A ticket that does not verify says
    /// the Sender's mode (it sends nothing anyway), and so does a real
    /// job whose file name names a test TIN (it is refused).
    #[test]
    fn a_test_tins_job_is_a_dry_run_in_live_mode() {
        let sender = Sender::new(config());
        let mut cases: Vec<(String, Mode)> = TEST_ROOTS
            .iter()
            .flat_map(|root| [(format!("{root}00000"), Mode::DryRun), (format!("{root}00017"), Mode::DryRun)])
            .collect();
        // Near misses of the reserved roots are real TINs.
        for real in NEAR_MISSES {
            cases.push((format!("{real}00000"), Mode::Live));
        }
        for (tin, mode) in &cases {
            let (v1, v2) = jobs_for(tin);
            assert_eq!(sender.job_mode(&v1), *mode, "{tin}");
            assert_eq!(sender.job_mode(&v2), *mode, "{tin}");
        }
        for root in TEST_ROOTS {
            let (_, mut forged) = jobs_for(&format!("{root}00000"));
            if let Job::V2 { ticket, .. } = &mut forged {
                let claims = tickets::verify(&sender.config.ticket_keys, ticket, 0).unwrap();
                *ticket = tickets::sign(&SigningKey::from_bytes(&[1; 32]), &claims);
            }
            assert_eq!(sender.job_mode(&forged), Mode::Live, "{root}");
            let (mut v1, _) = jobs_for("12345678900000");
            if let Job::V1 { filename, .. } = &mut v1 {
                *filename = format!("{root}00000-1601Cv2018-062025#juan@example.com#.xml");
            }
            assert_eq!(sender.job_mode(&v1), Mode::Live, "{root}");
        }
    }

    /// Without the `bir` feature a test TIN's send is not refused for its
    /// TIN any more; it ends as every send there does, unsent.
    #[tokio::test]
    async fn a_test_tin_is_not_refused_for_its_tin() {
        let unsent = Control::Result { status: "failed".into(), error: Some(NOT_BUILT_IN.into()), receipt: None };
        for root in TEST_ROOTS {
            let (v1, v2) = jobs_for(&format!("{root}00000"));
            for job in [v1, v2] {
                assert_eq!(answer(job).await, (unsent.clone(), false), "{root}");
            }
        }
    }

    /// The v1 identity check is against the job: its words say "job", not
    /// "file name".
    #[tokio::test]
    async fn a_v1_return_for_another_tin_is_refused_before_sending() {
        for (xml, words) in [
            (return_1601c(["123", "456", "780", "00000"]), "the TIN in the return is not the job's TIN"),
            (return_1601c(["123", "456", "789", "00001"]), "the TIN in the return is not the job's TIN"),
            (return_1601c(["111", "111", "114", "00000"]), "the TIN in the return is not the job's TIN"),
            ("<return/>".to_owned(), "the return has no form fields"),
        ] {
            let (result, opened) = answer(v1_job(&xml)).await;
            // Final, as the relay protocol says: a code in brackets.
            let error = format!("The return does not match its job: {words}. (return_mismatch)");
            assert_eq!(result, Control::Result { status: "failed".into(), error: Some(error), receipt: None });
            assert!(!opened);
        }
    }

    /// The Sender takes a test TIN's job only on its reserved route, and
    /// only a test TIN's job there, v1 and v2. A job on the wrong route
    /// gets its `sender` frame and then a refusal, and nothing is sent; one
    /// on the right route runs (and, without the `bir` feature, ends unsent).
    #[tokio::test]
    async fn a_job_on_the_wrong_route_is_refused() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message as Up;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(serve(listener, Sender::new(config())));
        let link = crate::filing::link::SenderLink::new(&format!("http://{address}"), Zeroizing::new("x".repeat(32)));
        let unsent = failed(NOT_BUILT_IN.into());
        let mut cases = Vec::new();
        for (tin, test) in [("11111111400000", true), ("00000000000000", true), ("12345678900000", false)] {
            let (v1, v2) = jobs_for(tin);
            for job in [v1, v2] {
                // The `sender` frame says `dry_run` for a test job only, on
                // either route.
                let mode = if test { Mode::DryRun } else { Mode::Live };
                cases.push((tin, job.clone(), !test, mode, wrong_route(!test)));
                cases.push((tin, job, test, mode, unsent.clone()));
            }
        }
        for (tin, job, reserved, mode, answer) in cases {
            let job = match job {
                // A ticket is used once: each case gets its own.
                Job::V2 { .. } => jobs_for(tin).1,
                v1 => v1,
            };
            let mut socket = link.open(reserved).await.unwrap();
            socket.send(Up::Text(serde_json::to_string(&job).unwrap().into())).await.unwrap();
            let mut frames = Vec::new();
            while let Some(Ok(Up::Text(text))) = socket.next().await {
                frames.push(text.to_string());
                if matches!(relay::Frame::decode_text(&text), Some(relay::Frame::Control(Control::Result { .. }))) {
                    break;
                }
            }
            let result = serde_json::to_string(&answer).unwrap();
            assert_eq!(frames, [sender_frame(mode), result], "{tin} on reserved={reserved}");
        }
    }

    /// A test TIN's v2 job whose ticket does not check, on the reserved
    /// route: its `sender` frame says `dry_run` (so the Gatekeeper passes
    /// it on) and its result gives the ticket's reason. A real TIN's job
    /// with a bad ticket there, and a job the Sender cannot read, are not
    /// test jobs: their `sender` frame says the Sender's own mode (`live`),
    /// which the Gatekeeper refuses for a test TIN's try.
    #[tokio::test]
    async fn a_test_jobs_bad_ticket_says_why_on_the_reserved_route() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message as Up;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let sender = Sender::new(config());
        let keys = sender.config.ticket_keys.clone();
        tokio::spawn(serve(listener, sender));
        let link = crate::filing::link::SenderLink::new(&format!("http://{address}"), Zeroizing::new("x".repeat(32)));
        let forged = |tin: &str| {
            let (_, mut forged) = jobs_for(tin);
            if let Job::V2 { ticket, .. } = &mut forged {
                let claims = tickets::verify(&keys, ticket, 0).unwrap();
                *ticket = tickets::sign(&SigningKey::from_bytes(&[1; 32]), &claims);
            }
            let Job::V2 { ticket, .. } = &forged else { unreachable!() };
            let reason =
                tickets::verify(&keys, ticket, OffsetDateTime::now_utc().unix_timestamp()).unwrap_err().to_string();
            (serde_json::to_string(&forged).unwrap(), failed(reason))
        };
        let (test, test_answer) = forged("11111111400000");
        let (real, real_answer) = forged("12345678900000");
        // A test TIN's ticket that ran out, signed by the right key.
        let (_, mut expired) = jobs_for("11111111400000");
        if let Job::V2 { ticket, .. } = &mut expired {
            let mut claims = tickets::verify(&keys, ticket, 0).unwrap();
            claims.exp = OffsetDateTime::now_utc().unix_timestamp() - 1;
            *ticket = tickets::sign(&SigningKey::from_bytes(&[9; 32]), &claims);
        }
        let expired = serde_json::to_string(&expired).unwrap();
        for (job, mode, answer) in [
            (test, Mode::DryRun, test_answer),
            (expired, Mode::DryRun, failed(tickets::TicketError::Expired.to_string())),
            (real, Mode::Live, real_answer),
            ("not a job".to_owned(), Mode::Live, failed("the Sender did not understand the job".into())),
        ] {
            let mut socket = link.open(true).await.unwrap();
            socket.send(Up::Text(job.into())).await.unwrap();
            let mut frames = Vec::new();
            while let Some(Ok(Up::Text(text))) = socket.next().await {
                frames.push(text.to_string());
                if frames.len() == 2 {
                    break;
                }
            }
            assert_eq!(frames, [sender_frame(mode), serde_json::to_string(&answer).unwrap()]);
        }
    }

    /// `blanked()` is built only with the `bir` feature (its settings type
    /// is private), so this reads its body: every setting that could reach
    /// the BIR is empty, the mode is `dry_run`, and it keeps only the IAF
    /// passphrase (to compute the receipt's `payload_sha256`), the TLS
    /// roots and the SFTP timeout. Any other shape fails, and so does a
    /// `Bir` delivery that no longer blanks a test send.
    #[test]
    fn blanked_keeps_nothing_to_contact() {
        let source = include_str!("mod.rs");
        let signature = concat!("fn blanked(", "relay: &RelaySettings) -> RelaySettings {");
        assert_eq!(source.matches(signature).count(), 1, "blanked() is there, once");
        let body = &source[source.find(signature).unwrap()..];
        let body = &body[..body.find("\n}\n").expect("the end of blanked()")];
        let lines: Vec<&str> = body.lines().skip(1).map(str::trim).collect();
        assert_eq!(
            lines,
            [
                "crate::bir::Settings {",
                "mode: Mode::DryRun,",
                "iaf_passphrase: relay.iaf_passphrase.clone(),",
                "dispatcher_passphrase: Zeroizing::new(String::new()),",
                "dispatchers: Vec::new(),",
                "sftp_hosts: Vec::new(),",
                "pinned_keys: Vec::new(),",
                "lab: None,",
                "tls: relay.tls.clone(),",
                "sftp_timeout: relay.sftp_timeout,",
                "}",
            ]
        );
        let blanks = concat!("Given::Blanked => ", "crate::bir::send(pipe, &job, &blanked(relay)).await,");
        assert_eq!(source.matches(blanks).count(), 1, "a test send gets blanked settings");
        let relay = concat!("Given::Relay => ", "crate::bir::send(pipe, &job, relay).await,");
        assert_eq!(source.matches(relay).count(), 1, "only a real send gets the relay's");
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

    /// A test TIN in `live`, for every reserved root at head office: a dry
    /// run, v1 and v2, with a `dry_run` receipt under the receipt key, and
    /// the dispatcher never hears of it.
    #[tokio::test]
    async fn a_test_tin_is_a_dry_run_in_live_mode() {
        let (dispatcher, contacted) = counting().await;
        let sender = Sender::new(live(dispatcher));
        for root in crate::account::tin::TEST_ROOTS {
            for job in jobs_for(&format!("{root}00000")) {
                assert_eq!(sender.job_mode(&job), Mode::DryRun, "{root}");
                let Control::Result { status, error, receipt: Some(Value::String(receipt)) } =
                    run(&sender, job, dispatcher).await
                else {
                    panic!("no receipt for {root}");
                };
                assert_eq!((status.as_str(), error), ("dry_run", None), "{root}");
                let keys = [sender.config.receipt_key.verifying_key()];
                let claims = jws::verify(&keys, RECEIPT_TYP, &receipt).expect("signed with the receipt key");
                assert_eq!((&claims["mode"], &claims["status"]), (&json!("dry_run"), &json!("dry_run")), "{claims}");
                let shown = format!("{}-{}-{}-00000", &root[..3], &root[3..6], &root[6..]);
                assert_eq!(claims["tin"], json!(shown));
                assert_eq!(claims["payload_sha256"].as_str().map(str::len), Some(64));
            }
        }
        assert_eq!(contacted.load(Ordering::SeqCst), 0, "no dispatcher login");
    }

    /// What a test send hands the BIR code: `dry_run`, with nothing to
    /// contact, whatever the relay's own settings.
    #[test]
    fn blanked_settings_have_nothing_to_contact() {
        for mode in [Mode::Live, Mode::Lab, Mode::DryRun] {
            let mut relay = live(1).relay;
            relay.mode = mode;
            let blank = blanked(&relay);
            assert_eq!(blank.mode, Mode::DryRun);
            assert!(blank.dispatchers.is_empty() && blank.sftp_hosts.is_empty() && blank.pinned_keys.is_empty());
            assert!(blank.lab.is_none() && blank.dispatcher_passphrase.is_empty());
        }
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
