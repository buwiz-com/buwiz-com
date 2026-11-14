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
//! - [`public`]: the sign as apps and people read it, its words, the
//!   compact header and the My TINs banner.
//! - [`history`]: the public day history and the last 24 hours.
//! - [`api`]: the read routes and the header on existing responses (from
//!   `public`).
//! - [`cli`]: `buwiz-cloud bir`, the manual switch for an Admin.

pub mod alerts;
pub mod api;
pub mod classify;
pub mod cli;
pub mod days;
pub mod history;
pub mod keys;
pub mod live;
pub mod notes;
pub mod observe;
pub mod public;
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
    /// Plus what apps and people read: `class` and `bir` on the relay
    /// `result`, the mode on failed and unknown submissions, the read
    /// routes, the `buwiz-bir` header, the status page, the My TINs banner
    /// and the footer link.
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
    /// The public document of the current 5-minute mark, made once.
    pub published: std::sync::Mutex<Option<std::sync::Arc<public::Published>>>,
    /// The public history, made at most once a mark.
    pub history: history::Cache,
    /// How often the ticker runs ([`live::TICK`]; tests run it faster).
    pub tick_every: std::time::Duration,
    /// Held while the day history is written, so the ticker's and the
    /// hourly upkeep's never run at once.
    rolling: tokio::sync::Mutex<()>,
    /// Tests only: how many runs of the rules (the start's load, then the
    /// ticker's) panic, to prove the server starts and the ticker goes on.
    #[doc(hidden)]
    pub panics: std::sync::atomic::AtomicU32,
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
            published: std::sync::Mutex::new(None),
            history: history::Cache::default(),
            tick_every: live::TICK,
            rolling: tokio::sync::Mutex::new(()),
            panics: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// Counts a public read from `address` (an IPv6 address by its /64).
    pub fn check_read_from(&self, address: IpAddr, now: OffsetDateTime) -> Result<(), Duration> {
        self.reads_per_ip.check(keys::ip_key(address), now)
    }

    /// The public document of `now`'s 5-minute mark (and its `ETag` and
    /// compact header), made once a mark: everything public is made from
    /// it.
    pub fn published(&self, forms_origin: &str, now: OffsetDateTime) -> std::sync::Arc<public::Published> {
        let (mark, sign) = self.live.public(now);
        let mut published = self.published.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(made) =
            published.as_ref().filter(|made| made.mark == mark && std::sync::Arc::ptr_eq(&made.sign, &sign))
        {
            return made.clone();
        }
        let made = std::sync::Arc::new(public::Published::of(mark, sign, forms_origin));
        *published = Some(made.clone());
        made
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
/// is bounded too, [`crate::account::mail`]); a round cut short counts as
/// failed for the addresses it did not reach.
pub const FLUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// How long [`start`] waits for the sign before the server takes requests.
pub const START_LOAD: std::time::Duration = std::time::Duration::from_secs(10);

/// From `record` up, before the server binds its port: the sign loaded and
/// aged by the clock (a restart's gap written as "not watched"), so nothing
/// reads the stored sign as it was. At most [`START_LOAD`]: past it, or on
/// a failure (logged), the server starts with No recent news and `seq` 0,
/// which claims nothing, and the ticker tries again; so does a panic in
/// the load (logged). Nothing at `off`. The day history is not written
/// here: the hourly upkeep ([`hourly`]) does it, right after the start.
pub async fn start(state: &AppState) {
    let uptime = &state.uptime;
    if !uptime.level.records() {
        return;
    }
    let now = OffsetDateTime::now_utc();
    guarded("the start's load of the sign", async {
        test_panic(uptime);
        match tokio::time::timeout(START_LOAD, live::tick(uptime, &state.db, now)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::error!(%error, "BIR uptime: the sign could not be loaded at start; No recent news until the ticker can")
            }
            Err(_) => {
                tracing::error!(
                    "BIR uptime: loading the sign took too long at start; No recent news until the ticker can"
                )
            }
        }
    })
    .await;
}

/// Tests only: panics while [`Uptime::panics`] says so.
fn test_panic(uptime: &Uptime) {
    if uptime.panics.load(std::sync::atomic::Ordering::SeqCst) > 0 {
        uptime.panics.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        panic!("a made-up panic in the rules");
    }
}

/// How long a clean stop may take to write the time of the last
/// evaluation ([`stop`]).
pub const STOP_WRITE: std::time::Duration = std::time::Duration::from_secs(5);

/// From `record` up, for as long as the server runs: [`Ticker::step`] every
/// 30 seconds. Nothing at `off`.
pub async fn run_ticker(state: AppState) {
    if !state.uptime.level.records() {
        return;
    }
    let mut ticker = Ticker::default();
    loop {
        ticker.step(&state, OffsetDateTime::now_utc()).await;
        tokio::time::sleep(state.uptime.tick_every).await;
    }
}

/// What the ticker keeps from one run to the next.
#[derive(Default)]
pub struct Ticker {
    rolled: Option<(OffsetDateTime, time::Date)>,
    rolling: Option<tokio::task::JoinHandle<()>>,
}

impl Ticker {
    /// One tick at `now`: runs the rules for what the clock alone changes
    /// (and the tries waiting) and sends the staff mail that is due, and
    /// every 10 minutes (and after each Manila midnight), once a run of the
    /// rules worked with the database, rewrites every day line not final
    /// yet in a task of its own (one at a time), so a long roll-up never
    /// holds the rules. While the sign is idle ([`live::step`]) the rules
    /// run in memory and nothing here touches the database: the day history
    /// and the write of the evaluation time are the hourly upkeep's then
    /// ([`hourly`]). Whether a run of the rules touched the database.
    pub async fn step(&mut self, state: &AppState, now: OffsetDateTime) -> bool {
        let uptime = state.uptime.clone();
        let mut ruled = false;
        guarded("the rules", async {
            test_panic(&uptime);
            match live::step(&uptime, &state.db, now).await {
                Ok(touched) => ruled = touched,
                Err(error) => tracing::error!(%error, "BIR uptime: the rules could not run"),
            }
        })
        .await;
        guarded("staff mail", async {
            let flush = alerts::flush(&uptime, &state.db, &state.mailer, &state.config, now);
            match tokio::time::timeout(FLUSH_TIMEOUT, flush).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => tracing::error!(%error, "BIR uptime: staff mail could not be sent"),
                Err(_) => {
                    // Whom it reached is kept; the rest wait their backoff.
                    uptime.alerts.timed_out(now);
                    tracing::error!("BIR uptime: staff mail took too long; the addresses not reached are tried later");
                }
            }
        })
        .await;
        // The day history only after a run of the rules that worked with
        // the database, so a day's end is evaluated (or its gap written)
        // before it is final.
        let day = keys::manila_day(now);
        let due = self.rolled.is_none_or(|(at, rolled_day)| now - at >= ROLL_UP_EVERY || rolled_day != day || now < at);
        if ruled && due && self.rolling.as_ref().is_none_or(tokio::task::JoinHandle::is_finished) {
            self.rolled = Some((now, day));
            let (uptime, db) = (uptime.clone(), state.db.clone());
            self.rolling = Some(tokio::spawn(async move { roll_up(&uptime, &db, now).await }));
        }
        ruled
    }
}

/// The day history, unless one is being written already.
async fn roll_up(uptime: &Uptime, db: &sqlx::PgPool, now: OffsetDateTime) {
    let Ok(_rolling) = uptime.rolling.try_lock() else {
        return;
    };
    guarded("the day history", async {
        if let Err(error) = days::roll_up_recent(uptime, db, now).await {
            tracing::error!(%error, "BIR uptime: the day history could not be written");
        }
    })
    .await;
}

/// BIR uptime's part of the hourly upkeep (which also runs right after the
/// start): the retention of the uptime tables (at every level, each failure
/// logged as its own), then from `record` up a run of the rules that reads
/// the stored note and writes the sign's evaluation time ([`live::heartbeat`])
/// and, once that worked, the day history. While the sign is idle this is
/// all the database work uptime does: one small write an hour (and the
/// first upkeep after Manila midnight still rolls the day up).
pub async fn hourly(state: &AppState, now: OffsetDateTime) {
    let uptime = &state.uptime;
    if let Err(error) = store::upkeep(&state.db).await {
        tracing::error!(%error, "BIR uptime: the hourly cleanup of the uptime tables failed");
    }
    if !uptime.level.records() {
        return;
    }
    let mut wrote = false;
    guarded("the hourly run of the rules", async {
        match live::heartbeat(uptime, &state.db, now).await {
            Ok(done) => wrote = done,
            Err(error) => tracing::error!(%error, "BIR uptime: the hourly run of the rules could not be written"),
        }
    })
    .await;
    if wrote {
        roll_up(uptime, &state.db, now).await;
    }
}

/// SIGUSR1: takes in a note the `buwiz-cloud bir` command wrote, at once
/// ([`live::poke`]). Nothing else, and nothing at `off` or before the sign
/// is loaded: harmless whenever it comes.
pub async fn poke(state: &AppState) {
    let uptime = &state.uptime;
    if !uptime.level.records() {
        return;
    }
    tracing::info!("BIR uptime: SIGUSR1; reading the stored note");
    guarded("the poke", async {
        let now = OffsetDateTime::now_utc();
        match tokio::time::timeout(live::DB_TIMEOUT, live::poke(uptime, &state.db, now)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::error!(%error, "BIR uptime: the stored note could not be taken in"),
            Err(_) => tracing::error!("BIR uptime: taking in the stored note took too long"),
        }
    })
    .await;
}

/// A clean stop (SIGTERM), once the records are drained: writes the time
/// of the last evaluation once, at most [`STOP_WRITE`], so the next start
/// does not read the idle time before it as not watched. Nothing at `off`
/// or before the sign was loaded.
pub async fn stop(state: &AppState, now: OffsetDateTime) {
    let uptime = &state.uptime;
    if !uptime.level.records() {
        return;
    }
    guarded("the stop's write", async {
        match tokio::time::timeout(STOP_WRITE, live::heartbeat(uptime, &state.db, now)).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => tracing::error!(%error, "BIR uptime: the sign could not be written at stop"),
            Err(_) => tracing::error!("BIR uptime: writing the sign at stop took too long"),
        }
    })
    .await;
}

/// Runs one part of a tick: a panic inside it is logged and the ticker goes
/// on with the next. `false` if it panicked.
pub async fn guarded(what: &str, work: impl Future<Output = ()>) -> bool {
    use futures_util::FutureExt;
    match std::panic::AssertUnwindSafe(work).catch_unwind().await {
        Ok(()) => true,
        Err(panic) => {
            let message = panic
                .downcast_ref::<&str>()
                .map(|message| (*message).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            tracing::error!(what, message, "BIR uptime: part of a tick panicked; the ticker goes on");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_panic_in_a_tick_is_logged_and_the_ticker_goes_on() {
        let mut ran = 0;
        for round in 0..3 {
            let survived = guarded("a made-up part", async {
                ran += 1;
                if round == 1 {
                    panic!("made-up panic");
                }
            })
            .await;
            assert_eq!(survived, round != 1);
        }
        assert_eq!(ran, 3, "the round after the panic ran");
    }

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
