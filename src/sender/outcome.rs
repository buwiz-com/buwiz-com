//! What a send is asked to do ([`Mode`]) and how it ends ([`Sent`] or
//! [`SendError`]). These are shared by the BIR relay (`crate::bir`, behind
//! the `bir` feature) and the build without it, whose Sender fails every
//! send with [`NOT_BUILT_IN`].

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

/// The error of every send from a build without the `bir` feature. Nothing
/// reached BIR, so sending again (from a full build) is safe.
pub const NOT_BUILT_IN: &str = "relay not built in";

/// The relay's settings.
#[cfg(feature = "bir")]
pub use crate::bir::Settings as RelaySettings;

/// The relay's settings. Without the `bir` feature only the mode is read.
#[cfg(not(feature = "bir"))]
pub struct RelaySettings {
    pub mode: Mode,
}
