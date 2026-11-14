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
//! - [`store`]: writes a try's row and its host key, and hands the try to
//!   the rules.
//! - [`sign`]: the rule engine, pure.
//! - [`live`]: the sign in memory, rebuilt on start, written on change.
//! - [`notes`]: what an Admin sets by hand.
//! - [`days`]: the day history and the tables' retention.
//! - [`alerts`]: staff mail.

pub mod alerts;
pub mod classify;
pub mod days;
pub mod keys;
pub mod live;
pub mod notes;
pub mod observe;
pub mod sign;
pub mod sshkex;
pub mod store;

use std::net::IpAddr;

use http::HeaderMap;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use self::alerts::Alerts;
use self::keys::{DaySets, KeyRing, NetworkReader, RateLimiter};
use self::live::Live;
use self::store::{Recorder, Sightings};
use crate::base::state::AppState;

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
    /// Unset or empty is the default, `record`. Any case, and spaces around
    /// it, read the same.
    pub fn parse(raw: Option<&str>) -> Result<Self, String> {
        let raw = raw.map(str::trim).unwrap_or_default();
        match raw.to_ascii_lowercase().as_str() {
            "" | "record" => Ok(Self::Record),
            "off" => Ok(Self::Off),
            "public" => Ok(Self::Public),
            "enforce" => Ok(Self::Enforce),
            _ => Err(format!("BIR_UPTIME must be off, record, public or enforce, not `{raw}`")),
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

/// The app names the record keeps: the apps' own names in `buwiz-client`
/// (docs/bir-uptime.md: `desktop`, `mini`, `direct`, `buwiz-forms`) and
/// the OAuth client ids of the apps that send returns (`buwiz-desktop`,
/// `buwiz-ebirforms`, and the direct app's `buwiz-direct`). Any other name
/// is kept as `other`.
pub const CLIENT_APPS: &[&str] =
    &["desktop", "mini", "direct", "buwiz-forms", "buwiz-desktop", "buwiz-ebirforms", "buwiz-direct"];

/// The `buwiz-client` header as the record keeps it: one of
/// [`CLIENT_APPS`] (or `other`), and the version's `major.minor` only,
/// each at most 4 digits. So no TIN, phone number, name or other text a
/// person might put there is ever kept. A header that does not read as
/// `<app>/<major>.<minor>…` is not kept at all.
pub fn client_label(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(CLIENT_HEADER)?.to_str().ok()?.trim();
    if raw.len() > 100 {
        return None;
    }
    let raw = raw.to_ascii_lowercase();
    let (app, version) = raw.split_once('/')?;
    let app_ok = (1..=20).contains(&app.len())
        && app.starts_with(|ch: char| ch.is_ascii_lowercase())
        && app.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !app_ok {
        return None;
    }
    let app = CLIENT_APPS.iter().find(|known| **known == app).copied().unwrap_or("other");
    let (major, rest) = version.split_once('.')?;
    let minor_end = rest.find(|ch: char| !ch.is_ascii_digit()).unwrap_or(rest.len());
    let minor = &rest[..minor_end];
    let number_ok = |part: &str| (1..=4).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_digit());
    (number_ok(major) && number_ok(minor)).then(|| format!("{app}/{major}.{minor}"))
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
    /// The public reads, per IP address as [`keys::ip_key`] counts it
    /// (never stored): through [`Uptime::check_read_from`].
    reads_per_ip: RateLimiter<IpAddr>,
    /// The signed-in reads, per account.
    pub reads_per_account: RateLimiter<Uuid>,
    /// The records being written after their try ended.
    pub recorder: Recorder,
    /// The host-key sightings already written, so one that adds nothing is
    /// not written again.
    pub sightings: Sightings,
    /// The sign, in memory ([`live`]).
    pub live: Live,
    /// Staff mail not sent yet ([`alerts`]).
    pub alerts: Alerts,
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
            recorder: Recorder::default(),
            sightings: Sightings::default(),
            live: Live::new(now),
            alerts: Alerts::default(),
        }
    }

    /// Counts a public read from `address` (an IPv6 address by its /64).
    pub fn check_read_from(&self, address: IpAddr, now: OffsetDateTime) -> Result<(), Duration> {
        self.reads_per_ip.check(keys::ip_key(address), now)
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

/// How often the day history is rewritten.
pub const ROLL_UP_EVERY: Duration = Duration::minutes(10);

/// How long one round of staff mail may take in the ticker (each request
/// is bounded too, [`crate::account::mail`]); one cut short is sent again.
pub const FLUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// How long [`start`] may take for the day history.
const START_ROLL_UP: std::time::Duration = std::time::Duration::from_secs(60);

/// From `record` up, before the server takes requests: the sign loaded and
/// aged by the clock (a restart's gap written as "not watched"), so nothing
/// reads the stored sign as it was, and every day line since the last
/// final one. Nothing at `off`. A failure is logged; the ticker tries again.
pub async fn start(state: &AppState) {
    let uptime = &state.uptime;
    if !uptime.level.records() {
        return;
    }
    let now = OffsetDateTime::now_utc();
    if let Err(error) = live::tick(uptime, &state.db, now).await {
        tracing::error!(%error, "BIR uptime: the sign could not be loaded at start");
    }
    match tokio::time::timeout(START_ROLL_UP, days::roll_up_recent(uptime, &state.db, now)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::error!(%error, "BIR uptime: the day history could not be written at start"),
        Err(_) => tracing::error!("BIR uptime: the day history took too long at start; the ticker goes on"),
    }
}

/// From `record` up, for as long as the server runs: every 30 seconds runs
/// the rules for what the clock alone changes (and the tries waiting) and
/// sends the staff mail that is due, and every 10 minutes (and after each
/// Manila midnight) rewrites every day line not final yet. Nothing at
/// `off`.
pub async fn run_ticker(state: AppState) {
    let uptime = state.uptime.clone();
    if !uptime.level.records() {
        return;
    }
    let mut rolled: Option<(OffsetDateTime, time::Date)> = None;
    loop {
        let now = OffsetDateTime::now_utc();
        if let Err(error) = live::tick(&uptime, &state.db, now).await {
            tracing::error!(%error, "BIR uptime: the rules could not run");
        }
        let flush = alerts::flush(&uptime, &state.db, &state.mailer, &state.config, now);
        match tokio::time::timeout(FLUSH_TIMEOUT, flush).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => tracing::error!(%error, "BIR uptime: staff mail could not be sent"),
            Err(_) => tracing::error!("BIR uptime: staff mail took too long; it is sent again"),
        }
        let day = keys::manila_day(now);
        if rolled.is_none_or(|(at, rolled_day)| now - at >= ROLL_UP_EVERY || rolled_day != day) {
            match days::roll_up_recent(&uptime, &state.db, now).await {
                Ok(()) => rolled = Some((now, day)),
                Err(error) => tracing::error!(%error, "BIR uptime: the day history could not be written"),
            }
        }
        tokio::time::sleep(live::TICK).await;
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
        assert_eq!(Level::parse(Some("ENFORCE")), Ok(Level::Enforce));
        assert_eq!(Level::parse(Some("  Public\t")), Ok(Level::Public));
        assert_eq!(Level::parse(Some(" Off ")), Ok(Level::Off));
        assert_eq!(Level::parse(Some("RECORD")), Ok(Level::Record));
        for bad in ["on", "true", "public,enforce", "rec ord"] {
            assert!(Level::parse(Some(bad)).is_err(), "{bad}");
        }
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
            ("desktop/0.2.0", Some("desktop/0.2")),
            ("Mini/0.4.0", Some("mini/0.4")),
            ("direct/1.0", Some("direct/1.0")),
            ("buwiz-forms/1.5.0-beta.2", Some("buwiz-forms/1.5")),
            ("buwiz-ebirforms/0.3.3", Some("buwiz-ebirforms/0.3")),
            (" desktop/2026.11.1 ", Some("desktop/2026.11")),
            // Only the version's major.minor: nothing after it is kept.
            ("desktop/0.2.0 (juan@example.test)", Some("desktop/0.2")),
            ("desktop/0.2.0-123456789", Some("desktop/0.2")),
            ("desktop/0.2-09171234567", Some("desktop/0.2")),
            // A name it does not know is `other`.
            ("juan-dela-cruz/1.0", Some("other/1.0")),
            ("tin123456789/1.0", Some("other/1.0")),
            ("desktop", None),
            ("desktop/", None),
            ("desktop/1", None),
            ("desktop/beta", None),
            ("desktop/123456789000", None),
            ("desktop/12345.6", None),
            ("desktop/1.23456", None),
            ("9desk/1.0", None),
            ("123-456-789-000/1.0", None),
            ("juan dela cruz/1.0", None),
        ] {
            assert_eq!(label(raw).as_deref(), kept, "{raw}");
        }
        assert_eq!(client_label(&HeaderMap::new()), None);
    }

    #[test]
    fn public_reads_count_an_ipv6_household_once() {
        let uptime = Uptime::new(Level::Record, vec![22], false, OffsetDateTime::UNIX_EPOCH);
        let now = OffsetDateTime::UNIX_EPOCH;
        for host in 1..=READS_PER_IP {
            let address: IpAddr = format!("2001:db8:7:8::{host:x}").parse().unwrap();
            assert_eq!(uptime.check_read_from(address, now), Ok(()));
        }
        assert!(uptime.check_read_from("2001:db8:7:8:ffff::1".parse().unwrap(), now).is_err());
        assert_eq!(uptime.check_read_from("2001:db8:7:9::1".parse().unwrap(), now), Ok(()));
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
