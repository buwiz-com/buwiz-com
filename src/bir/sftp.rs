//! Upload of one encrypted IAF file over SFTP. SSH runs end to end over
//! whatever stream it is given, so key exchange, the password and the file
//! are encrypted inside SSH even when the bytes pass through the
//! taxpayer's computer.
//!
//! The server's host key must match a pinned SHA-256 fingerprint. eBIRForms
//! accepts any key; we must not, or a relaying computer could pose as BIR
//! and catch the password.

use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::{HashAlg, PublicKeyOrCertificate};
use russh::{Disconnect, client::Handler};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

const TIMEOUT: Duration = Duration::from_secs(60);

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
    /// The file may or may not have reached the server.
    #[error("SFTP failed during the upload: {0}")]
    DuringUpload(String),
    #[error("SFTP timed out")]
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
}

/// Logs in over `stream` and writes the file.
pub async fn upload<S>(stream: S, upload: Upload<'_>) -> Result<(), SftpError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    tokio::time::timeout(TIMEOUT, upload_inner(stream, upload)).await.map_err(|_| SftpError::Timeout)?
}

async fn upload_inner<S>(stream: S, upload: Upload<'_>) -> Result<(), SftpError>
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
    let mut file = sftp
        .open_with_flags(upload.remote_path, OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE)
        .await
        .map_err(|error| before(&error))?;
    // From here on BIR may hold a partial or complete file.
    let during = |error: &dyn std::fmt::Display| SftpError::DuringUpload(error.to_string());
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
    fn remote_paths_stay_in_the_form_folder() {
        assert_eq!(remote_path("1601Cv2018", "x.xml"), "/1601Cv2018/x.xml");
        assert_eq!(remote_path("/1601Cv2018/", "../../etc/x.xml"), "/1601Cv2018/x.xml");
    }
}
