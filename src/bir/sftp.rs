//! Upload of one encrypted IAF file over SFTP. SSH runs end to end over
//! whatever stream it is given, so key exchange, the password and the file
//! are encrypted inside SSH even when the bytes pass through the
//! taxpayer's computer.
//!
//! The server's host key must match a pinned SHA-256 fingerprint. eBIRForms
//! accepts any key; we must not, or a relaying computer could pose as BIR
//! and catch the password.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::{HashAlg, PublicKeyOrCertificate};
use russh::{Disconnect, client::Handler};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

/// How long one upload may take, login to close, unless settings say otherwise.
pub const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub enum SftpError {
    #[error("the SFTP server's host key is not pinned (see the log for its fingerprint)")]
    HostKey,
    #[error("SSH failed: {0}")]
    Ssh(String),
    #[error("SFTP login was refused")]
    Login,
    #[error("SFTP failed before the upload started: {0}")]
    BeforeUpload(String),
    /// The file may or may not have reached the server: anything that goes
    /// wrong once the request to open the remote file was sent, a timeout
    /// included.
    #[error("SFTP failed during the upload: {0}")]
    DuringUpload(String),
    /// Timed out before the remote file was asked for: nothing reached BIR.
    #[error("SFTP timed out before the upload started")]
    Timeout,
}

impl SftpError {
    /// True when BIR may already have the file, so sending again could file
    /// the return twice.
    pub fn maybe_delivered(&self) -> bool {
        matches!(self, Self::DuringUpload(_))
    }
}

struct PinnedKey {
    pins: Vec<String>,
}

impl Handler for PinnedKey {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            tracing::warn!("SFTP server offered a certificate; refused");
            return Ok(false);
        };
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let pinned = self.pins.iter().any(|pin| pin == &fingerprint);
        if !pinned {
            // Check this fingerprint out of band before pinning it.
            tracing::warn!(%fingerprint, "SFTP host key is not pinned; refused");
        }
        Ok(pinned)
    }
}

pub struct Upload<'a> {
    pub username: &'a str,
    pub password: &'a str,
    /// `/{form}/{file}` on the server.
    pub remote_path: &'a str,
    pub payload: &'a [u8],
    /// Accepted host keys, as `SHA256:…` fingerprints.
    pub pinned_keys: &'a [String],
    /// The whole session, login to close.
    pub timeout: Duration,
}

/// Logs in over `stream` and writes the file.
pub async fn upload<S>(stream: S, upload: Upload<'_>) -> Result<(), SftpError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    // Set just before the open request goes out. From then on the server
    // may create (or truncate) the file, so a timeout is not "nothing sent".
    let opening = Arc::new(AtomicBool::new(false));
    let limit = upload.timeout;
    match tokio::time::timeout(limit, upload_inner(stream, upload, opening.clone())).await {
        Ok(result) => result,
        Err(_) if opening.load(Ordering::SeqCst) => Err(SftpError::DuringUpload(format!(
            "timed out after {}s, once the remote file was asked for",
            limit.as_secs()
        ))),
        Err(_) => Err(SftpError::Timeout),
    }
}

async fn upload_inner<S>(stream: S, upload: Upload<'_>, opening: Arc<AtomicBool>) -> Result<(), SftpError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let config = Arc::new(client::Config { nodelay: true, ..Default::default() });
    let handler = PinnedKey { pins: upload.pinned_keys.to_vec() };
    let mut session: Handle<PinnedKey> = match client::connect_stream(config, stream, handler).await {
        Ok(session) => session,
        Err(russh::Error::UnknownKey) => return Err(SftpError::HostKey),
        Err(error) => return Err(SftpError::Ssh(error.to_string())),
    };
    let auth = session
        .authenticate_password(upload.username, upload.password)
        .await
        .map_err(|error| SftpError::Ssh(error.to_string()))?;
    if !auth.success() {
        return Err(SftpError::Login);
    }
    let before = |error: &dyn std::fmt::Display| SftpError::BeforeUpload(error.to_string());
    let channel = session.channel_open_session().await.map_err(|error| before(&error))?;
    channel.request_subsystem(true, "sftp").await.map_err(|error| before(&error))?;
    let sftp = SftpSession::new(channel.into_stream()).await.map_err(|error| before(&error))?;
    // From the open request on, BIR may hold an empty, partial or complete
    // file. Only a definite refusal of the open (a status answer) means
    // nothing was created.
    let during = |error: &dyn std::fmt::Display| SftpError::DuringUpload(error.to_string());
    opening.store(true, Ordering::SeqCst);
    let mut file = sftp
        .open_with_flags(upload.remote_path, OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE)
        .await
        .map_err(|error| match error {
            russh_sftp::client::error::Error::Status(_) => before(&error),
            other => during(&other),
        })?;
    file.write_all(upload.payload).await.map_err(|error| during(&error))?;
    file.flush().await.map_err(|error| during(&error))?;
    file.shutdown().await.map_err(|error| during(&error))?;
    let _ = sftp.close().await;
    let _ = session.disconnect(Disconnect::ByApplication, "", "en").await;
    Ok(())
}

/// `/{form}/{file}`, never escaping the form folder.
pub fn remote_path(form_type: &str, filename: &str) -> String {
    format!("/{}/{}", form_type.trim_matches('/'), filename.rsplit(['/', '\\']).next().unwrap_or(filename))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_failures_before_the_open_are_safe_to_resend() {
        assert!(!SftpError::HostKey.maybe_delivered());
        assert!(!SftpError::Login.maybe_delivered());
        assert!(!SftpError::Ssh("x".into()).maybe_delivered());
        assert!(!SftpError::BeforeUpload("x".into()).maybe_delivered());
        assert!(!SftpError::Timeout.maybe_delivered());
        assert!(SftpError::DuringUpload("timed out".into()).maybe_delivered());
    }

    #[test]
    fn remote_paths_stay_in_the_form_folder() {
        assert_eq!(remote_path("1601Cv2018", "x.xml"), "/1601Cv2018/x.xml");
        assert_eq!(remote_path("/1601Cv2018/", "../../etc/x.xml"), "/1601Cv2018/x.xml");
    }
}
