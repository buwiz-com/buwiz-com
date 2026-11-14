//! BIR uptime: what real sends tell about the BIR's two servers, and the
//! sign every app reads (docs/bir-uptime.md). Nothing here opens a
//! connection to anything: the evidence is the relay tries people make
//! anyway.
//!
//! - [`keys`]: day keys, network and account keys, rate limiters, the day's
//!   distinct accounts and networks.

pub mod keys;

use std::net::IpAddr;

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use self::keys::{DaySets, KeyRing, NetworkReader, RateLimiter};

/// `BIR_UPTIME`: how much of BIR uptime runs. Each level adds to the one
/// below.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
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
    fn upload_ports_default_to_22() {
        assert_eq!(parse_upload_ports(None), Ok(vec![22]));
        assert_eq!(parse_upload_ports(Some(" ")), Ok(vec![22]));
        assert_eq!(parse_upload_ports(Some("22, 2222")), Ok(vec![22, 2222]));
        for bad in ["ssh", "22,", "0", "70000"] {
            assert!(parse_upload_ports(Some(bad)).is_err(), "{bad}");
        }
    }
}
