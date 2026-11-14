//! BIR uptime: what real sends tell about the BIR's two servers, and the
//! sign every app reads (docs/bir-uptime.md). Nothing here opens a
//! connection to anything: the evidence is the relay tries people make
//! anyway.
//!
//! - [`keys`]: day keys, network and account keys, rate limiters, the day's
//!   distinct accounts and networks.
//! - [`observe`]: watches one relay try's frames in the Gatekeeper's bridge.
//! - [`classify`]: names a try's stage, windows, class and kind.
//! - [`sshkex`]: reads the upload server's host key from the clear SSH
//!   handshake.
//! - [`store`]: writes a try's row and its host key.

pub mod classify;
pub mod keys;
pub mod observe;
pub mod sshkex;
pub mod store;

use std::net::IpAddr;

use http::HeaderMap;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use self::keys::{DaySets, KeyRing, NetworkReader, RateLimiter};

/// `BIR_UPTIME`: how much of BIR uptime runs. Each level adds to the one
/// below.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Level {
    /// Nothing new runs. The Sender's `sender` frame is still dropped.
    Off,
    /// Every relay try is recorded and classified; no app sees a change.
    #[default]
    Record,
    /// Plus what apps read: `class` on the relay `result`, the mode on
    /// failed and unknown submissions, and (in later releases) the routes,
    /// the header and the sign.
    Public,
    /// Plus holds.
    Enforce,
}

impl Level {
    /// Unset or empty is the default, `record`.
    pub fn parse(raw: Option<&str>) -> Result<Self, String> {
        match raw.map(str::trim).unwrap_or_default() {
            "" | "record" => Ok(Self::Record),
            "off" => Ok(Self::Off),
            "public" => Ok(Self::Public),
            "enforce" => Ok(Self::Enforce),
            other => Err(format!("BIR_UPTIME must be off, record, public or enforce, not `{other}`")),
        }
    }

    /// From `BIR_UPTIME`. A value it does not know is logged and read as
    /// `record`: it never stops the server from starting.
    pub fn from_env() -> Self {
        Self::parse(std::env::var("BIR_UPTIME").ok().as_deref()).unwrap_or_else(|error| {
            tracing::error!(%error, "BIR_UPTIME is not valid; recording only");
            Self::Record
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Record => "record",
            Self::Public => "public",
            Self::Enforce => "enforce",
        }
    }

    /// Whether tries are recorded.
    pub fn records(self) -> bool {
        self >= Self::Record
    }

    /// Whether apps get what the record says (`class` on results).
    pub fn is_public(self) -> bool {
        self >= Self::Public
    }
}

/// The ports whose relay streams are the upload window, by default `22`.
pub const DEFAULT_UPLOAD_PORTS: &[u16] = &[22];

/// `BIR_UPTIME_UPLOAD_PORTS`: comma-separated ports.
pub fn parse_upload_ports(raw: Option<&str>) -> Result<Vec<u16>, String> {
    let raw = raw.map(str::trim).unwrap_or_default();
    if raw.is_empty() {
        return Ok(DEFAULT_UPLOAD_PORTS.to_vec());
    }
    let ports = raw
        .split(',')
        .map(|port| port.trim().parse::<u16>().ok().filter(|port| *port > 0))
        .collect::<Option<Vec<u16>>>()
        .ok_or_else(|| format!("BIR_UPTIME_UPLOAD_PORTS must be comma-separated ports, not `{raw}`"))?;
    Ok(ports)
}

/// The header new apps send on every request: `<app>/<version>`.
pub const CLIENT_HEADER: &str = "buwiz-client";

/// The `buwiz-client` header as the record keeps it: lowercase
/// `<app>/<version>` of at most 40 characters, in the letters versions use,
/// with no run of more than 4 digits (so no TIN, phone number or other
/// number a person might put there). Anything else is not kept.
pub fn client_label(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(CLIENT_HEADER)?.to_str().ok()?.trim().to_ascii_lowercase();
    let (app, version) = raw.split_once('/')?;
    let app_ok = (1..=20).contains(&app.len())
        && app.starts_with(|ch: char| ch.is_ascii_lowercase())
        && app.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    let version_ok = !version.is_empty()
        && version.starts_with(|ch: char| ch.is_ascii_digit())
        && version.bytes().all(|byte| byte.is_ascii_alphanumeric() || b".+-".contains(&byte));
    let digit_runs_ok = raw.split(|ch: char| !ch.is_ascii_digit()).all(|run| run.len() <= 4);
    (raw.len() <= 40 && app_ok && version_ok && digit_runs_ok).then_some(raw)
}

/// Requests per minute per IP address on the public reads.
pub const READS_PER_IP: u32 = 120;
/// Requests per minute per account on the signed-in reads.
pub const READS_PER_ACCOUNT: u32 = 60;

/// BIR uptime's state in the Gatekeeper, in memory.
pub struct Uptime {
    pub level: Level,
    pub upload_ports: Vec<u16>,
    pub keys: KeyRing,
    pub networks: NetworkReader,
    pub day_sets: DaySets,
    /// The public reads, per IP address (never stored).
    pub reads_per_ip: RateLimiter<IpAddr>,
    /// The signed-in reads, per account.
    pub reads_per_account: RateLimiter<Uuid>,
}

impl Uptime {
    /// `production`: a public deployment (an `https://` origin), where a
    /// request without `CF-Connecting-IP` is raised.
    pub fn new(level: Level, upload_ports: Vec<u16>, production: bool, now: OffsetDateTime) -> Self {
        Self {
            level,
            upload_ports,
            keys: KeyRing::new(now),
            networks: NetworkReader::new(production),
            day_sets: DaySets::default(),
            reads_per_ip: RateLimiter::new(READS_PER_IP, Duration::minutes(1)),
            reads_per_account: RateLimiter::new(READS_PER_ACCOUNT, Duration::minutes(1)),
        }
    }

    /// From `BIR_UPTIME` and `BIR_UPTIME_UPLOAD_PORTS`. Neither stops the
    /// server from starting: a value it cannot read is logged and the
    /// default is used.
    pub fn from_env(production: bool) -> Self {
        let ports =
            parse_upload_ports(std::env::var("BIR_UPTIME_UPLOAD_PORTS").ok().as_deref()).unwrap_or_else(|error| {
                tracing::error!(%error, "BIR_UPTIME_UPLOAD_PORTS is not valid; using 22");
                DEFAULT_UPLOAD_PORTS.to_vec()
            });
        Self::new(Level::from_env(), ports, production, OffsetDateTime::now_utc())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_reads_four_words_and_defaults_to_record() {
        assert_eq!(Level::parse(None), Ok(Level::Record));
        assert_eq!(Level::parse(Some("")), Ok(Level::Record));
        for level in [Level::Off, Level::Record, Level::Public, Level::Enforce] {
            assert_eq!(Level::parse(Some(level.as_str())), Ok(level));
        }
        assert!(Level::parse(Some("ENFORCE")).is_err());
        assert!(Level::parse(Some("on")).is_err());
        assert!(!Level::Off.records());
        assert!(Level::Record.records() && !Level::Record.is_public());
        assert!(Level::Public.is_public() && Level::Enforce.is_public());
    }

    #[test]
    fn the_client_header_is_kept_only_as_app_and_version() {
        let label = |raw: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(CLIENT_HEADER, raw.parse().unwrap());
            client_label(&headers)
        };
        for (raw, kept) in [
            ("desktop/0.2.0", Some("desktop/0.2.0")),
            ("Mini/0.4.0", Some("mini/0.4.0")),
            ("buwiz-forms/1.5.0-beta.2", Some("buwiz-forms/1.5.0-beta.2")),
            (" desktop/2026.11.1 ", Some("desktop/2026.11.1")),
            ("desktop", None),
            ("desktop/", None),
            ("desktop/beta", None),
            ("desktop/0.2.0 (juan@example.test)", None),
            ("desktop/123456789000", None),
            ("desktop/0.2.0-123456789", None),
            ("a/1.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0", None),
            ("9desk/1.0", None),
        ] {
            assert_eq!(label(raw).as_deref(), kept, "{raw}");
        }
        assert_eq!(client_label(&HeaderMap::new()), None);
    }

    #[test]
    fn upload_ports_default_to_22() {
        assert_eq!(parse_upload_ports(None), Ok(vec![22]));
        assert_eq!(parse_upload_ports(Some(" ")), Ok(vec![22]));
        assert_eq!(parse_upload_ports(Some("22, 2222")), Ok(vec![22, 2222]));
        for bad in ["ssh", "22,", "0", "70000"] {
            assert!(parse_upload_ports(Some(bad)).is_err(), "{bad}");
        }
    }
}
