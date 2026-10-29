//! Sending one return to BIR through a relay pipe: encrypt the IAF file,
//! ask BIR's dispatcher for the upload server and its login, and upload
//! over SFTP. All network traffic goes through the taxpayer's computer;
//! the passphrases and the SFTP login never leave this server.

pub mod dispatcher;
pub mod iaf;
pub mod sftp;

use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio_rustls::TlsConnector;
use zeroize::Zeroizing;

use crate::relay::Pipe;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Encrypts and checks everything, then stops. Nothing reaches BIR.
    DryRun,
    /// Uploads to a test SFTP server from settings, skipping the dispatcher.
    Lab,
    /// Files with BIR.
    Live,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DryRun => "dry_run",
            Self::Lab => "lab",
            Self::Live => "live",
        }
    }
}

/// A dispatcher endpoint, tried in order.
#[derive(Clone, Debug)]
pub struct DispatcherTarget {
    pub host: String,
    pub port: u16,
    pub tls: bool,
}

/// BIR's dispatcher: HTTPS first, then the plain HTTP the official client
/// uses, on the primary and then the backup host. The answer's fields are
/// wrapped either way, and the upload host is checked and its key pinned.
pub fn bir_dispatchers() -> Vec<DispatcherTarget> {
    let target = |host: &str, port, tls| DispatcherTarget { host: host.to_owned(), port, tls };
    vec![
        target("birgovph.com", 443, true),
        target("birgovph.com", 80, false),
        target("ws2.birgovph.com", 443, true),
        target("ws2.birgovph.com", 80, false),
    ]
}

pub struct LabServer {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Zeroizing<String>,
}

pub struct Settings {
    pub mode: Mode,
    pub iaf_passphrase: Zeroizing<String>,
    pub dispatcher_passphrase: Zeroizing<String>,
    pub dispatchers: Vec<DispatcherTarget>,
    /// Upload hosts the dispatcher may name.
    pub sftp_hosts: Vec<String>,
    /// `SHA256:…` host-key fingerprints accepted for upload servers.
    pub pinned_keys: Vec<String>,
    pub lab: Option<LabServer>,
    pub tls: Arc<rustls::ClientConfig>,
    /// One upload, login to close ([`sftp::TIMEOUT`] in production).
    pub sftp_timeout: std::time::Duration,
}

impl Settings {
    pub fn tls_config() -> Arc<rustls::ClientConfig> {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = rustls::ClientConfig::builder_with_provider(rustls::crypto::ring::default_provider().into())
            .with_safe_default_protocol_versions()
            .expect("ring supports the default TLS versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        Arc::new(config)
    }
}

/// One return to send.
pub struct Job<'a> {
    pub tin_root: &'a str,
    pub form_type: &'a str,
    pub filename: &'a str,
    pub xml: &'a [u8],
}

#[derive(Debug)]
pub struct Sent {
    pub remote_path: String,
    /// SHA-256 of the encrypted file as uploaded, hex.
    pub payload_sha256: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    /// Nothing reached BIR; sending again is safe.
    #[error("{0}")]
    NotSent(String),
    /// BIR may have the file. Sending again could file twice.
    #[error("{0}")]
    MaybeSent(String),
}

pub async fn send(pipe: &Pipe, job: &Job<'_>, settings: &Settings) -> Result<Sent, SendError> {
    let payload = iaf::encrypt(job.xml, &settings.iaf_passphrase)
        .map_err(|error| SendError::NotSent(format!("could not encrypt the return: {error}")))?;
    let remote_path = sftp::remote_path(job.form_type, job.filename);
    let sent = Sent { remote_path, payload_sha256: hex(&Sha256::digest(&payload)) };

    let login = match settings.mode {
        Mode::DryRun => {
            tracing::info!(remote_path = %sent.remote_path, "dry run: not sent to BIR");
            return Ok(sent);
        }
        Mode::Lab => {
            let lab = settings.lab.as_ref().ok_or_else(|| SendError::NotSent("no lab SFTP server set".into()))?;
            dispatcher::SftpLogin {
                host: lab.host.clone(),
                port: lab.port,
                username: lab.username.clone(),
                password: lab.password.clone(),
            }
        }
        Mode::Live => ask_dispatcher(pipe, job, settings).await?,
    };
    let allowed =
        settings.mode == Mode::Lab || settings.sftp_hosts.iter().any(|host| host.eq_ignore_ascii_case(&login.host));
    if !allowed {
        return Err(SendError::NotSent(format!("the dispatcher named an unknown upload server: {}", login.host)));
    }
    let stream = pipe
        .connect(&login.host, login.port)
        .await
        .map_err(|error| SendError::NotSent(format!("could not reach BIR's upload server: {error}")))?;
    let upload = sftp::Upload {
        username: &login.username,
        password: &login.password,
        remote_path: &sent.remote_path,
        payload: &payload,
        pinned_keys: &settings.pinned_keys,
        timeout: settings.sftp_timeout,
    };
    match sftp::upload(stream, upload).await {
        Ok(()) => Ok(sent),
        Err(error) if error.maybe_delivered() => Err(SendError::MaybeSent(error.to_string())),
        Err(error) => Err(SendError::NotSent(error.to_string())),
    }
}

async fn ask_dispatcher(pipe: &Pipe, job: &Job<'_>, settings: &Settings) -> Result<dispatcher::SftpLogin, SendError> {
    let mut failures = Vec::new();
    for target in &settings.dispatchers {
        let attempt = async {
            let stream = pipe.connect(&target.host, target.port).await.map_err(|error| error.to_string())?;
            let body = if target.tls {
                let name =
                    rustls::pki_types::ServerName::try_from(target.host.clone()).map_err(|error| error.to_string())?;
                let tls = TlsConnector::from(settings.tls.clone())
                    .connect(name, stream)
                    .await
                    .map_err(|error| format!("TLS: {error}"))?;
                dispatcher::fetch(tls, &target.host, job.tin_root, job.form_type).await
            } else {
                dispatcher::fetch(stream, &target.host, job.tin_root, job.form_type).await
            };
            let body = body.map_err(|error| error.to_string())?;
            dispatcher::parse_answer(&body, &settings.dispatcher_passphrase).map_err(|error| error.to_string())
        };
        match attempt.await {
            Ok(login) => return Ok(login),
            Err(error) => {
                let scheme = if target.tls { "https" } else { "http" };
                tracing::warn!(host = %target.host, scheme, %error, "dispatcher attempt failed");
                failures.push(format!("{scheme}://{}: {error}", target.host));
            }
        }
    }
    Err(SendError::NotSent(format!("BIR's dispatcher did not answer: {}", failures.join("; "))))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
