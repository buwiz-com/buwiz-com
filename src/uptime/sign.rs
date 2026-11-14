//! The rule engine: the sign, from the tries people made anyway
//! (docs/bir-uptime.md §2.1, §15.1). Pure and deterministic: [`evaluate`]
//! reads the previous sign, the recent tries, the open note, the key
//! periods and the clock, and says the next sign and what changed. Nothing
//! here touches a database or the network; [`super::live`] keeps it and
//! writes it.
//!
//! Only counted `live` tries are evidence (a try that got in, or failed on
//! the BIR's side). A try of class `user_network` or `ours` never moves a
//! light; three kinds of `ours` (a host key that is not pinned, an unknown
//! upload server, a protocol error) are told to staff as "the relay would
//! pause" (nothing pauses in this release).
//!
//! Evidence is read whole on every run, ordered by its exact end time
//! (exact for a try this process recorded, to the minute for one read back
//! from `bir_tries`): what a run says does not depend on which tries were
//! new to it, so a run that failed is simply run again.
//!
//! Accounts and networks are the day-keyed hashes of [`super::keys`]. Two
//! tries are the same account only when they share a key of the same key
//! period; a try recorded in the 3 hours after a new day key carries the
//! previous key too, which is how tries across midnight compare. A try
//! with no key of a period still in memory (one read back after a restart,
//! or from before midnight once the previous key is gone) cannot be linked
//! to anything: it is neither the same account as another nor a different
//! one, so it is left out of the last 10 and of every quorum. After a
//! restart a quorum needs fresh evidence.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::classify::{Kind, Outcome, Stage, Window};
use super::keys::Keys;
use super::store::to_minute;

/// The format of the stored sign. A later release that adds a field reads
/// this one's sign (every field has a default) and writes its own number.
pub const SIGN_VERSION: u32 = 1;
/// How many tries "the last 10" holds.
pub const LAST_N: usize = 10;
/// The most of the last 10 one account fills; at most one of them a
/// failure.
pub const PER_ACCOUNT: usize = 2;
/// How far back the last 10 reaches.
pub const LAST_WITHIN: Duration = Duration::minutes(60);
/// Online: at most this many failures among the last 10 (and the newest
/// got in).
pub const ONLINE_MAX_FAILURES: usize = 1;
/// Busy from No recent news: at least this many failures among the last
/// 10, and one that got in ...
pub const BUSY_FAILURES: usize = 3;
/// ... from at least this many accounts on as many networks (from Online:
/// this spread alone, once the Online rule no longer holds).
pub const BUSY_SPREAD: usize = 2;
/// Offline: this many accounts on as many networks failed ...
pub const OFFLINE_SPREAD: usize = 3;
/// ... within this long, and none got in since.
pub const OFFLINE_WITHIN: Duration = Duration::minutes(15);
/// A change that lets more through (Busy to Online or No recent news)
/// waits until Busy is this old.
pub const DWELL: Duration = Duration::minutes(10);
/// No evidence on a window for this long (or, while Online, no return got
/// past it): No recent news.
pub const QUIET: Duration = Duration::minutes(60);
/// Offline with no evidence for this long: No recent news.
pub const OFFLINE_QUIET: Duration = Duration::hours(3);
/// The accounts of an Offline's quorum lose their Offline vote for this
/// long when a relay landing ends that Offline.
pub const VOTE_LOST_FOR: Duration = Duration::hours(24);
/// "The relay would pause" after this many tries of the pausing kinds ...
pub const PAUSE_TRIES: usize = 3;
/// ... from this many accounts on as many networks ...
pub const PAUSE_SPREAD: usize = 2;
/// ... within this long.
pub const PAUSE_WITHIN: Duration = Duration::minutes(60);
/// Staff hear of this many counted failures in a row (one per account)
/// with no landing between.
pub const STREAK_FAILURES: usize = 5;
/// A sign not evaluated for longer than this: Buwiz was not watching.
pub const WATCH_GAP: Duration = Duration::minutes(5);
/// How far back the engine is given tries.
pub const EVIDENCE_KEPT: Duration = Duration::hours(3);
/// The most vote losses kept.
pub const MAX_VOTE_LOSSES: usize = 2_000;
/// The most keys one vote loss holds.
const LOSS_KEYS: usize = 8;
/// At most this many direct-app reports are tried as the one direct
/// account of a quorum.
pub const DIRECT_CANDIDATES: usize = 20;

/// Stored times are to the minute; in memory they are exact.
mod minute {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use time::OffsetDateTime;

    use super::to_minute;

    pub fn serialize<S: Serializer>(at: &OffsetDateTime, serializer: S) -> Result<S::Ok, S::Error> {
        time::serde::rfc3339::serialize(&to_minute(*at), serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<OffsetDateTime, D::Error> {
        time::serde::rfc3339::deserialize(deserializer)
    }

    pub mod option {
        use super::*;

        pub fn serialize<S: Serializer>(at: &Option<OffsetDateTime>, serializer: S) -> Result<S::Ok, S::Error> {
            #[derive(Serialize)]
            struct Wrap(#[serde(with = "time::serde::rfc3339")] OffsetDateTime);
            at.map(|at| Wrap(to_minute(at))).serialize(serializer)
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<OffsetDateTime>, D::Error> {
            #[derive(Deserialize)]
            struct Wrap(#[serde(with = "time::serde::rfc3339")] OffsetDateTime);
            Ok(Option::<Wrap>::deserialize(deserializer)?.map(|Wrap(at)| at))
        }
    }
}

/// A light's state, and the overall state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Light {
    Online,
    Busy,
    Offline,
    NoRecentNews,
}

impl Light {
    pub const ALL: [Light; 4] = [Self::Online, Self::Busy, Self::Offline, Self::NoRecentNews];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Busy => "busy",
            Self::Offline => "offline",
            Self::NoRecentNews => "no_recent_news",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|light| light.as_str() == raw)
    }

    /// How much it holds back: Offline most, then Busy, then the two that
    /// hold nothing.
    pub fn strictness(self) -> u8 {
        match self {
            Self::Offline => 2,
            Self::Busy => 1,
            Self::Online | Self::NoRecentNews => 0,
        }
    }
}

/// One of the BIR's two servers in a send.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// The front desk: says which upload server to use.
    Dispatcher,
    /// The mailbox: takes the return over SFTP.
    Upload,
}

impl Side {
    pub const BOTH: [Side; 2] = [Self::Dispatcher, Self::Upload];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dispatcher => "dispatcher",
            Self::Upload => "upload",
        }
    }

    /// The word people read.
    pub fn word(self) -> &'static str {
        match self {
            Self::Dispatcher => "front desk",
            Self::Upload => "mailbox",
        }
    }
}

/// Where a try's record came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Through the relay: the Gatekeeper saw it.
    Relay,
    /// The direct app's report: unverifiable, so trusted in part.
    Direct,
}

/// How a try's ticket got through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    Open,
    Admit,
    Scout,
    LegacyScout,
    TryAnyway,
}

impl Admission {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "admit" => Self::Admit,
            "scout" => Self::Scout,
            "legacy_scout" => Self::LegacyScout,
            "try_anyway" => Self::TryAnyway,
            _ => Self::Open,
        }
    }
}

/// What a try said about one window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Ok,
    Fail,
    /// The upload broke midway: counts toward Busy, never toward Offline.
    Maybe,
}

/// One `live` try, as the engine reads it. Its account and network are
/// keys only; `account_id` is held in memory for a try this process
/// recorded (to keep a lost vote for 24 hours, longer than a key lives) and
/// is never written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evidence {
    /// Its `bir_tries` row, if written (the rules do not read it).
    pub id: Option<i64>,
    /// When it ended: exact for a try this process recorded, to the minute
    /// for one read back (as `bir_tries` keeps it).
    pub at: OffsetDateTime,
    pub source: Source,
    pub admission: Admission,
    pub outcome: Outcome,
    pub stage: Stage,
    pub dispatcher: Window,
    pub upload: Window,
    pub kind: Option<Kind>,
    pub counted: bool,
    /// A direct report sent long after its try: it adds no network.
    pub late: bool,
    /// Its keys under the current day key and, within 3 hours of a new
    /// one, the previous.
    pub keys: Vec<Keys>,
    pub account_id: Option<Uuid>,
    /// The port of the last stream the Sender asked for, if known (not
    /// kept in `bir_tries`). With `kind`, what a later release needs to
    /// tell a device its network blocks the upload port (issue #58); the
    /// engine does not read it.
    pub port: Option<u16>,
}

impl Evidence {
    /// What it says about `side`, if it is evidence for it: counted, and it
    /// reached that window (the front desk over TLS only). A relay return
    /// that got in passed the front desk, whatever scheme reached it: its
    /// upload server came from there and its key is pinned.
    pub fn hit(&self, side: Side) -> Option<Hit> {
        if !self.counted {
            return None;
        }
        if side == Side::Dispatcher && self.source == Source::Relay && self.outcome == Outcome::Sent {
            return Some(Hit::Ok);
        }
        match (side, side_window(self, side)) {
            (_, Window::Ok) => Some(Hit::Ok),
            (_, Window::Fail) => Some(Hit::Fail),
            (Side::Upload, Window::Maybe) => Some(Hit::Maybe),
            _ => None,
        }
    }

    /// A relay try that got past `side`: only these end Offline.
    fn relay_ok(&self, side: Side) -> bool {
        self.source == Source::Relay && self.hit(side) == Some(Hit::Ok)
    }

    /// A counted return that got in, from either source.
    fn landed(&self) -> bool {
        self.counted && self.outcome == Outcome::Sent
    }

    /// A relay return that got in: only these end an outage note.
    fn relay_landed(&self) -> bool {
        self.source == Source::Relay && self.landed()
    }

    /// One of the faults on Buwiz's side that still touch the BIR.
    pub fn pauses_relay(&self) -> bool {
        self.source == Source::Relay && matches!(self.kind, Some(Kind::HostKey | Kind::UnknownHost | Kind::Protocol))
    }

    /// It has a key of one of `periods` (the key periods still in memory).
    pub fn linked(&self, periods: &[i32]) -> bool {
        self.keys.iter().any(|keys| periods.contains(&keys.epoch))
    }

    fn network_known(&self) -> bool {
        !self.late
    }
}

fn side_window(evidence: &Evidence, side: Side) -> Window {
    match side {
        Side::Dispatcher => evidence.dispatcher,
        Side::Upload => evidence.upload,
    }
}

/// The kind of a hand-set note.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    /// From the news: overall Offline, scouts go on. A relay return that
    /// got in ends it.
    Outage,
    /// Announced: overall Offline, no scouts. Ends at its end or by an
    /// Admin; a return that got in during it is told to staff.
    Maintenance,
    /// "Ignore evidence until …": overall Online.
    Online,
}

impl NoteKind {
    pub const ALL: [NoteKind; 3] = [Self::Outage, Self::Maintenance, Self::Online];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Outage => "outage",
            Self::Maintenance => "maintenance",
            Self::Online => "online",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == raw)
    }

    /// Outage and maintenance hold everything.
    pub fn holds(self) -> bool {
        matches!(self, Self::Outage | Self::Maintenance)
    }
}

/// A hand-set note, as the sign carries it. Its text is public.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub id: Uuid,
    pub kind: NoteKind,
    pub text: String,
    #[serde(with = "time::serde::rfc3339")]
    pub starts_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub until: OffsetDateTime,
}

impl Note {
    pub fn in_force(&self, now: OffsetDateTime) -> bool {
        self.starts_at <= now && now < self.until
    }
}

/// How a note ended, as `bir_notes.cleared_how` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteEnd {
    Expired,
    ReturnGotIn,
}

impl NoteEnd {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::ReturnGotIn => "return_got_in",
        }
    }
}

/// An account that voted for an Offline: its keys, its id while this
/// process holds it, and when its failure ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Voter {
    /// In memory only.
    #[serde(skip)]
    pub account_id: Option<Uuid>,
    /// (key period, account key).
    pub keys: Vec<(i32, [u8; 8])>,
    #[serde(with = "minute")]
    pub at: OffsetDateTime,
}

impl Voter {
    fn of(item: &Evidence) -> Self {
        Self {
            account_id: item.account_id,
            keys: item.keys.iter().map(|keys| (keys.epoch, keys.account)).collect(),
            at: item.at,
        }
    }
}

/// One light.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lamp {
    pub state: Light,
    #[serde(with = "minute")]
    pub since: OffsetDateTime,
    /// With `no_recent_news`: the state before the quiet.
    pub last_state: Option<Light>,
    /// The last relay try that got past this window.
    #[serde(with = "minute::option")]
    pub last_ok_at: Option<OffsetDateTime>,
    /// The last evidence of any outcome on this window.
    #[serde(with = "minute::option")]
    pub last_evidence_at: Option<OffsetDateTime>,
    /// Set when the window leaves Offline: its last 10 counts from here.
    #[serde(with = "minute::option")]
    pub fresh_from: Option<OffsetDateTime>,
    /// While Offline: the accounts whose failures made it so.
    pub quorum: Vec<Voter>,
}

impl Lamp {
    fn new(now: OffsetDateTime) -> Self {
        Self {
            state: Light::NoRecentNews,
            since: now,
            last_state: None,
            last_ok_at: None,
            last_evidence_at: None,
            fresh_from: None,
            quorum: Vec::new(),
        }
    }

    /// Where an Offline's quorum begins: a landing after it ends Offline.
    fn quorum_from(&self) -> OffsetDateTime {
        self.quorum.iter().map(|voter| voter.at).min().unwrap_or(self.since - OFFLINE_WITHIN)
    }
}

impl Default for Lamp {
    fn default() -> Self {
        Self::new(OffsetDateTime::UNIX_EPOCH)
    }
}

/// The relay's own state. This release never pauses it (nothing could
/// resume it yet): `paused` is always false in what it writes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Relay {
    pub paused: bool,
    #[serde(with = "minute::option")]
    pub since: Option<OffsetDateTime>,
    /// Why it paused: a fixed sentence with counts.
    pub reason: Option<String>,
    /// When an Admin last resumed it: only faults after this count.
    #[serde(with = "minute::option")]
    pub resumed_at: Option<OffsetDateTime>,
}

/// An Offline vote lost, for one window: one entry per account and window,
/// holding the latest end.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoteLoss {
    pub side: Side,
    /// In memory only: the account, for the whole 24 hours.
    #[serde(skip)]
    pub account_id: Option<Uuid>,
    /// The account's keys (key period, key), for tries read back from the
    /// record, which carry no account id.
    pub keys: Vec<(i32, [u8; 8])>,
    #[serde(with = "minute")]
    pub until: OffsetDateTime,
}

impl VoteLoss {
    fn covers(&self, side: Side, evidence: &Evidence) -> bool {
        self.side == side
            && (self.account_id.is_some() && self.account_id == evidence.account_id
                || evidence.keys.iter().any(|keys| self.keys.contains(&(keys.epoch, keys.account))))
    }

    fn same_account(&self, side: Side, voter: &Voter) -> bool {
        self.side == side
            && (self.account_id.is_some() && self.account_id == voter.account_id
                || voter.keys.iter().any(|keys| self.keys.contains(keys)))
    }
}

/// Takes away `voter`'s Offline vote on `side` until `until`: one entry per
/// account and window, holding the latest end.
fn lose_vote(losses: &mut Vec<VoteLoss>, side: Side, voter: &Voter, until: OffsetDateTime) {
    match losses.iter_mut().find(|loss| loss.same_account(side, voter)) {
        Some(loss) => {
            loss.until = loss.until.max(until);
            loss.account_id = loss.account_id.or(voter.account_id);
            for keys in &voter.keys {
                if !loss.keys.contains(keys) {
                    loss.keys.push(*keys);
                }
            }
            let extra = loss.keys.len().saturating_sub(LOSS_KEYS);
            loss.keys.drain(..extra);
        }
        None => losses.push(VoteLoss { side, account_id: voter.account_id, keys: voter.keys.clone(), until }),
    }
}

/// The sign as the engine keeps it. `seq` grows by one with every change
/// and survives restarts (`bir_sign`); `revision` changes whenever anything
/// a reader sees changes (the later ETag). Every field has a default, so a
/// stored sign that lacks one (from an older release) or has more (from a
/// later one) still reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SignState {
    pub version: u32,
    pub seq: i64,
    /// Grows with every change a reader sees: never below the last one, and
    /// at least the Unix time in milliseconds of the change, so a revision
    /// given out but not yet stored is never given again after a restart.
    pub revision: i64,
    pub state: Light,
    #[serde(with = "minute")]
    pub since: OffsetDateTime,
    /// With `no_recent_news`: the state before the quiet began.
    pub last_state: Option<Light>,
    #[serde(with = "minute")]
    pub evaluated_at: OffsetDateTime,
    pub dispatcher: Lamp,
    pub upload: Lamp,
    /// The note in force at the last evaluation.
    pub note: Option<Note>,
    pub relay: Relay,
    /// The last relay return that got in.
    #[serde(with = "minute::option")]
    pub last_success_at: Option<OffsetDateTime>,
    /// The last counted try of any outcome.
    #[serde(with = "minute::option")]
    pub last_news_at: Option<OffsetDateTime>,
    /// Since when the server is waking (Offline, or an outage or
    /// maintenance note, ended). The doorman (a later release) ends it.
    #[serde(with = "minute::option")]
    pub waking_since: Option<OffsetDateTime>,
    pub vote_losses: Vec<VoteLoss>,
    /// The landing a run of failures was told after (staff mail).
    #[serde(with = "minute::option")]
    pub streak_told: Option<OffsetDateTime>,
    /// The maintenance note a landing during it was told for.
    pub maintenance_told: Option<Uuid>,
}

impl SignState {
    /// The first sign: nothing known.
    pub fn new(now: OffsetDateTime) -> Self {
        Self {
            version: SIGN_VERSION,
            seq: 0,
            revision: 0,
            state: Light::NoRecentNews,
            since: now,
            last_state: None,
            evaluated_at: now,
            dispatcher: Lamp::new(now),
            upload: Lamp::new(now),
            note: None,
            relay: Relay::default(),
            last_success_at: None,
            last_news_at: None,
            waking_since: None,
            vote_losses: Vec::new(),
            streak_told: None,
            maintenance_told: None,
        }
    }

    pub fn lamp(&self, side: Side) -> &Lamp {
        match side {
            Side::Dispatcher => &self.dispatcher,
            Side::Upload => &self.upload,
        }
    }

    fn lamp_mut(&mut self, side: Side) -> &mut Lamp {
        match side {
            Side::Dispatcher => &mut self.dispatcher,
            Side::Upload => &mut self.upload,
        }
    }

    /// `staff` while a hand-set note is in force, else `tries`.
    pub fn source(&self) -> &'static str {
        if self.note.is_some() { "staff" } else { "tries" }
    }

    /// What a reader can see, with times as they are stored and shown (to
    /// the minute): what `revision` follows.
    fn visible(&self) -> Self {
        let lamp = |lamp: &Lamp| Lamp {
            since: to_minute(lamp.since),
            last_ok_at: lamp.last_ok_at.map(to_minute),
            last_evidence_at: lamp.last_evidence_at.map(to_minute),
            fresh_from: None,
            quorum: Vec::new(),
            ..lamp.clone()
        };
        Self {
            revision: 0,
            since: to_minute(self.since),
            evaluated_at: OffsetDateTime::UNIX_EPOCH,
            dispatcher: lamp(&self.dispatcher),
            upload: lamp(&self.upload),
            last_success_at: self.last_success_at.map(to_minute),
            last_news_at: self.last_news_at.map(to_minute),
            waking_since: self.waking_since.map(to_minute),
            vote_losses: Vec::new(),
            streak_told: None,
            maintenance_told: None,
            ..self.clone()
        }
    }

    /// Without what only moves with the clock and the tries' times: what
    /// must be written at once when it changes.
    fn durable(&self) -> Self {
        let lamp = |lamp: &Lamp| Lamp { last_ok_at: None, last_evidence_at: None, ..lamp.clone() };
        Self {
            revision: 0,
            evaluated_at: OffsetDateTime::UNIX_EPOCH,
            dispatcher: lamp(&self.dispatcher),
            upload: lamp(&self.upload),
            last_success_at: None,
            last_news_at: None,
            ..self.clone()
        }
    }
}

impl Default for SignState {
    fn default() -> Self {
        Self::new(OffsetDateTime::UNIX_EPOCH)
    }
}

/// What an event is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Overall,
    Dispatcher,
    Upload,
    Relay,
    /// Whether Buwiz was watching: `watched` or `not_watched`.
    Watch,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Overall => "overall",
            Self::Dispatcher => "dispatcher",
            Self::Upload => "upload",
            Self::Relay => "relay",
            Self::Watch => "watch",
        }
    }

    pub fn of(side: Side) -> Self {
        match side {
            Side::Dispatcher => Self::Dispatcher,
            Side::Upload => Self::Upload,
        }
    }
}

/// Why it changed, as `bir_events.cause` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    Rule,
    Scout,
    TryAnyway,
    NoteSet,
    NoteCleared,
    NoteExpired,
    NoteReturnGotIn,
    Quiet,
    RelayPaused,
    RelayResumed,
    /// Buwiz was stopped, or the sign was not evaluated.
    NotWatched,
    /// The stored sign could not be read and started again.
    Reset,
}

impl Cause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Scout => "scout",
            Self::TryAnyway => "try_anyway",
            Self::NoteSet => "note_set",
            Self::NoteCleared => "note_cleared",
            Self::NoteExpired => "note_expired",
            Self::NoteReturnGotIn => "note_return_got_in",
            Self::Quiet => "quiet",
            Self::RelayPaused => "relay_paused",
            Self::RelayResumed => "relay_resumed",
            Self::NotWatched => "not_watched",
            Self::Reset => "reset",
        }
    }

    /// A note set or cleared by an Admin, or the relay resumed: the event
    /// names who.
    pub fn by_staff(self) -> bool {
        matches!(self, Self::NoteSet | Self::NoteCleared | Self::RelayResumed)
    }
}

/// One change, for `bir_events`: a fixed sentence with counts only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub scope: Scope,
    pub from: String,
    pub to: String,
    pub cause: Cause,
    pub reason: String,
}

/// What [`evaluate`] reads besides the previous sign.
#[derive(Clone, Copy, Debug)]
pub struct Inputs<'a> {
    /// The `live` tries of the last 3 hours, oldest first by exact end
    /// time.
    pub evidence: &'a [Evidence],
    /// The key periods still in memory: the current one and, for 3 hours
    /// after it replaced it, the previous one.
    pub periods: &'a [i32],
    /// The newest note not yet cleared (it may have run out).
    pub note: Option<&'a Note>,
    /// An Admin resumes the relay.
    pub resume_relay: bool,
    pub now: OffsetDateTime,
}

/// Something staff hear of that is no change of the sign.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// Counted failures in a row (one per account), none got in between.
    FailuresInARow { failures: usize, since: Option<OffsetDateTime> },
    /// The relay pause's trigger is met; nothing pauses in this release.
    RelayWouldPause { reason: String },
    /// A return got in during a maintenance note.
    LandedInMaintenance,
}

/// The next sign, what changed, and a note that ended by itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluation {
    pub state: SignState,
    pub events: Vec<Event>,
    pub note_ended: Option<(Uuid, NoteEnd)>,
    /// From when to when Buwiz was not watching, if it was not since the
    /// last evaluation.
    pub not_watched: Option<(OffsetDateTime, OffsetDateTime)>,
    pub notices: Vec<Notice>,
}

/// Which tries are the same account: tries that share a key of one key
/// period are linked, and so on through the links. A component number per
/// try.
fn same_accounts(evidence: &[Evidence]) -> Vec<usize> {
    let mut parent: Vec<usize> = (0..evidence.len()).collect();
    fn root(parent: &mut [usize], mut at: usize) -> usize {
        while parent[at] != at {
            parent[at] = parent[parent[at]];
            at = parent[at];
        }
        at
    }
    let mut first: HashMap<(i32, [u8; 8]), usize> = HashMap::new();
    for (index, item) in evidence.iter().enumerate() {
        for keys in &item.keys {
            match first.get(&(keys.epoch, keys.account)) {
                Some(&other) => {
                    let (a, b) = (root(&mut parent, index), root(&mut parent, other));
                    parent[a.max(b)] = a.min(b);
                }
                None => {
                    first.insert((keys.epoch, keys.account), index);
                }
            }
        }
    }
    (0..evidence.len()).map(|index| root(&mut parent, index)).collect()
}

/// The largest number of tries among `pairs` with pairwise different
/// accounts and pairwise different networks (a bipartite matching), up to
/// `enough`: the search stops there.
fn matching(pairs: &[([u8; 8], [u8; 8])], enough: usize) -> usize {
    let mut account_at: HashMap<[u8; 8], usize> = HashMap::new();
    let mut network_at: HashMap<[u8; 8], usize> = HashMap::new();
    let mut networks_of: Vec<Vec<usize>> = Vec::new();
    for (account, network) in pairs {
        let next = account_at.len();
        let account = *account_at.entry(*account).or_insert(next);
        if account == networks_of.len() {
            networks_of.push(Vec::new());
        }
        let next = network_at.len();
        let network = *network_at.entry(*network).or_insert(next);
        if !networks_of[account].contains(&network) {
            networks_of[account].push(network);
        }
    }
    // network -> account, account -> network.
    let mut owner: Vec<Option<usize>> = vec![None; network_at.len()];
    let mut taken: Vec<Option<usize>> = vec![None; networks_of.len()];
    let mut matched = 0;
    for start in 0..networks_of.len() {
        if matched >= enough {
            break;
        }
        // Breadth first along alternating paths; `via[network]` is the
        // account it was reached from.
        let mut via: Vec<Option<usize>> = vec![None; owner.len()];
        let mut queue = std::collections::VecDeque::from([start]);
        let mut free = None;
        'search: while let Some(account) = queue.pop_front() {
            for &network in &networks_of[account] {
                if via[network].is_some() {
                    continue;
                }
                via[network] = Some(account);
                match owner[network] {
                    None => {
                        free = Some(network);
                        break 'search;
                    }
                    Some(other) => queue.push_back(other),
                }
            }
        }
        let Some(mut network) = free else {
            continue;
        };
        // Flip the path back to `start`.
        while let Some(account) = via[network] {
            let before = taken[account];
            owner[network] = Some(account);
            taken[account] = Some(network);
            match before {
                Some(previous) if account != start => network = previous,
                _ => break,
            }
        }
        matched += 1;
    }
    matched.min(enough)
}

/// How many accounts on as many networks `items` come from, up to
/// `enough`: the largest matching within one of `periods`, with at most one
/// direct-app account (of the first [`DIRECT_CANDIDATES`]). Tries without a
/// network (late direct reports) add nothing.
fn spread(items: &[&Evidence], periods: &[i32], enough: usize) -> usize {
    let epochs: BTreeSet<i32> = items
        .iter()
        .flat_map(|item| item.keys.iter().map(|keys| keys.epoch))
        .filter(|epoch| periods.contains(epoch))
        .collect();
    let mut best = 0;
    for epoch in epochs {
        let pairs_of = |source: Source| -> Vec<([u8; 8], [u8; 8])> {
            items
                .iter()
                .filter(|item| item.source == source && item.network_known())
                .filter_map(|item| item.keys.iter().find(|keys| keys.epoch == epoch))
                .map(|keys| (keys.account, keys.network))
                .collect()
        };
        let relay = pairs_of(Source::Relay);
        best = best.max(matching(&relay, enough));
        let mut seen = HashSet::new();
        let direct: Vec<_> = pairs_of(Source::Direct).into_iter().filter(|pair| seen.insert(*pair)).collect();
        for pair in direct.into_iter().take(DIRECT_CANDIDATES) {
            if best >= enough {
                break;
            }
            let mut with = relay.clone();
            with.push(pair);
            best = best.max(matching(&with, enough));
        }
        if best >= enough {
            break;
        }
    }
    best
}

/// What one window's evidence says now.
struct Reading {
    /// The last 10, newest first: (index, hit).
    last: Vec<(usize, Hit)>,
    oks: usize,
    failures: usize,
    /// Failures among the last 10 newer than its newest landing.
    head_failures: usize,
    /// Accounts on as many networks among the last 10's failures, up to
    /// [`BUSY_SPREAD`].
    failure_spread: usize,
    /// Offline's quorum: accounts on as many networks, up to
    /// [`OFFLINE_SPREAD`] ...
    offline_spread: usize,
    /// ... and its failures, one per account.
    quorum: Vec<usize>,
    /// The newest relay try that got past this window.
    newest_landing: Option<usize>,
    /// The newest that got past it, from either source, or `last_ok_at`.
    newest_ok_at: Option<OffsetDateTime>,
}

impl Reading {
    /// The newest got in: some among the last 10 did, and the failures
    /// newer than it are one account's at most (one account's failure
    /// cannot by itself say otherwise).
    fn newest_in(&self) -> bool {
        self.oks >= 1 && self.head_failures <= 1
    }

    fn online(&self) -> bool {
        self.newest_in() && self.failures <= ONLINE_MAX_FAILURES
    }

    /// Busy from No recent news.
    fn busy(&self) -> bool {
        self.oks >= 1 && self.failures >= BUSY_FAILURES && self.failure_spread >= BUSY_SPREAD
    }

    fn offline(&self) -> bool {
        self.offline_spread >= OFFLINE_SPREAD
    }
}

fn read(side: Side, lamp: &Lamp, inputs: &Inputs<'_>, accounts: &[usize], losses: &[VoteLoss]) -> Reading {
    let now = inputs.now;
    let evidence = inputs.evidence;
    let periods = inputs.periods;
    // The last 10: newest first, within 60 minutes, since the window last
    // left Offline, linked to a key period in memory, at most 2 per account
    // and at most one failure per account.
    let mut last = Vec::new();
    let mut per_account: HashMap<usize, (usize, bool)> = HashMap::new();
    for (index, item) in evidence.iter().enumerate().rev() {
        if last.len() == LAST_N {
            break;
        }
        if item.at < now - LAST_WITHIN || item.at > now || lamp.fresh_from.is_some_and(|from| item.at < from) {
            continue;
        }
        if !item.linked(periods) {
            continue;
        }
        let Some(hit) = item.hit(side) else {
            continue;
        };
        let (filled, failed) = per_account.entry(accounts[index]).or_default();
        if *filled == PER_ACCOUNT || hit != Hit::Ok && *failed {
            continue;
        }
        *filled += 1;
        *failed |= hit != Hit::Ok;
        last.push((index, hit));
    }
    let oks = last.iter().filter(|(_, hit)| *hit == Hit::Ok).count();
    let head_failures = last.iter().take_while(|(_, hit)| *hit != Hit::Ok).count();
    let failing: Vec<&Evidence> =
        last.iter().filter(|(_, hit)| *hit != Hit::Ok).map(|(index, _)| &evidence[*index]).collect();

    // Offline: failures within 15 minutes, all after the newest relay try
    // that got past this window, one per account, from accounts that kept
    // their vote.
    let newest_landing = evidence
        .iter()
        .enumerate()
        .filter(|(_, item)| item.relay_ok(side) && item.at <= now)
        .max_by_key(|(index, item)| (item.at, *index))
        .map(|(index, _)| index);
    let landing_at = newest_landing.map(|index| evidence[index].at);
    let mut voted = HashSet::new();
    let mut quorum = Vec::new();
    for (index, item) in evidence.iter().enumerate().rev() {
        if item.at < now - OFFLINE_WITHIN || item.at > now || landing_at.is_some_and(|ok| item.at <= ok) {
            continue;
        }
        if item.hit(side) != Some(Hit::Fail) || !item.linked(periods) {
            continue;
        }
        if losses.iter().any(|loss| loss.until > now && loss.covers(side, item)) {
            continue;
        }
        if voted.insert(accounts[index]) {
            quorum.push(index);
        }
    }
    let quorum_items: Vec<&Evidence> = quorum.iter().map(|index| &evidence[*index]).collect();
    let newest_ok_at = evidence
        .iter()
        .filter(|item| item.hit(side) == Some(Hit::Ok) && item.at <= now)
        .map(|item| item.at)
        .max()
        .max(lamp.last_ok_at);
    Reading {
        oks,
        failures: failing.len(),
        head_failures,
        failure_spread: spread(&failing, periods, BUSY_SPREAD),
        offline_spread: spread(&quorum_items, periods, OFFLINE_SPREAD),
        quorum,
        newest_landing,
        newest_ok_at,
        last,
    }
}

/// A lamp's change: its new state, why, and the sentence.
struct Change {
    to: Light,
    cause: Cause,
    reason: String,
    /// Leaving Offline on a landing: when it got in.
    landed_at: Option<OffsetDateTime>,
}

fn step(side: Side, lamp: &Lamp, reading: &Reading, evidence: &[Evidence], now: OffsetDateTime) -> Option<Change> {
    let change = |to, cause, reason: String| Some(Change { to, cause, reason, landed_at: None });
    let quiet_for = |limit: Duration| lamp.last_evidence_at.is_none_or(|at| now - at >= limit);
    let last_n = reading.last.len();
    let quiet = || change(Light::NoRecentNews, Cause::Quiet, "Nobody filed in the last 60 min".into());
    match lamp.state {
        Light::Offline => {
            let landing = reading.newest_landing.map(|index| &evidence[index]);
            if let Some(landing) = landing.filter(|landing| landing.at > lamp.quorum_from()) {
                // The quorum, after the landing: still Offline, or not.
                if reading.offline() {
                    return None;
                }
                let (cause, reason) = match landing.admission {
                    Admission::Scout | Admission::LegacyScout => (Cause::Scout, "A scout got in"),
                    Admission::TryAnyway => (Cause::TryAnyway, "A Try anyway got in"),
                    Admission::Open | Admission::Admit => (Cause::Rule, "A return got in"),
                };
                let reason = format!("{reason} past the {}", side.word());
                return Some(Change { to: Light::Online, cause, reason, landed_at: Some(landing.at) });
            }
            if quiet_for(OFFLINE_QUIET) {
                return change(
                    Light::NoRecentNews,
                    Cause::Quiet,
                    "Nobody tried for 3 hours since it went offline".into(),
                );
            }
            None
        }
        _ if reading.offline() => change(
            Light::Offline,
            Cause::Rule,
            format!(
                "{0} accounts on {0} networks didn't get past the {1} within 15 min",
                reading.offline_spread,
                side.word()
            ),
        ),
        Light::Online => {
            if quiet_for(QUIET) {
                quiet()
            } else if reading.online() {
                None
            } else if reading.failure_spread >= BUSY_SPREAD {
                change(Light::Busy, Cause::Rule, busy_reason(reading))
            } else if reading.newest_ok_at.is_none_or(|at| now - at >= QUIET) {
                change(
                    Light::NoRecentNews,
                    Cause::Rule,
                    format!("Nothing got past the {} in the last 60 min", side.word()),
                )
            } else {
                None
            }
        }
        Light::Busy => {
            if now - lamp.since < DWELL {
                None
            } else if reading.online() {
                change(Light::Online, Cause::Rule, format!("{} of the last {last_n} got in", reading.oks))
            } else if quiet_for(QUIET) {
                quiet()
            } else {
                None
            }
        }
        Light::NoRecentNews => {
            if reading.busy() {
                change(Light::Busy, Cause::Rule, busy_reason(reading))
            } else if reading.online() {
                change(Light::Online, Cause::Rule, format!("{} of the last {last_n} got in", reading.oks))
            } else {
                None
            }
        }
    }
}

fn busy_reason(reading: &Reading) -> String {
    let (oks, of, failures, spread) = (reading.oks, reading.last.len(), reading.failures, reading.failure_spread);
    format!("{oks} of the last {of} got in; {failures} didn't, from {spread} accounts on {spread} networks")
}

/// The overall state from the two lights and the note in force.
pub fn overall(dispatcher: Light, upload: Light, note: Option<&Note>) -> Light {
    match note.map(|note| note.kind) {
        Some(NoteKind::Online) => Light::Online,
        Some(NoteKind::Outage | NoteKind::Maintenance) => Light::Offline,
        None if dispatcher == Light::Offline || upload == Light::Offline => Light::Offline,
        None if dispatcher == Light::Busy || upload == Light::Busy => Light::Busy,
        None if upload == Light::Online => Light::Online,
        None => Light::NoRecentNews,
    }
}

fn overall_reason(state: &SignState) -> String {
    let offline: Vec<&str> =
        Side::BOTH.into_iter().filter(|side| state.lamp(*side).state == Light::Offline).map(Side::word).collect();
    let busy: Vec<&str> =
        Side::BOTH.into_iter().filter(|side| state.lamp(*side).state == Light::Busy).map(Side::word).collect();
    match state.state {
        Light::Offline if offline.len() == 2 => "The front desk and the mailbox are offline".into(),
        Light::Offline => format!("The {} is offline", offline.first().copied().unwrap_or("BIR")),
        Light::Busy if busy.len() == 2 => "The front desk and the mailbox are busy".into(),
        Light::Busy => format!("The {} is busy", busy.first().copied().unwrap_or("BIR")),
        Light::Online => "The mailbox is online".into(),
        Light::NoRecentNews if state.last_state == Some(Light::Offline) => {
            "Nobody tried for 3 hours since the BIR went offline".into()
        }
        Light::NoRecentNews => "Nobody filed in the last 60 min".into(),
    }
}

/// The next sign. Deterministic: the same inputs give the same sign and the
/// same events.
pub fn evaluate(previous: &SignState, inputs: &Inputs<'_>) -> Evaluation {
    let now = inputs.now;
    let evidence = inputs.evidence;
    let mut next = previous.clone();
    next.version = SIGN_VERSION;
    next.evaluated_at = now;
    let mut events = Vec::new();
    let mut notices = Vec::new();
    let not_watched = (now - previous.evaluated_at > WATCH_GAP).then_some((previous.evaluated_at, now));

    // The times of news, from every try given (a run that failed, or a
    // restart, reads the same).
    next.vote_losses.retain(|loss| loss.until > now);
    for item in evidence.iter().filter(|item| item.at <= now) {
        if item.counted {
            next.last_news_at = next.last_news_at.max(Some(item.at));
        }
        if item.relay_landed() {
            next.last_success_at = next.last_success_at.max(Some(item.at));
        }
        for side in Side::BOTH {
            let hit = item.hit(side);
            let lamp = next.lamp_mut(side);
            if hit.is_some() {
                lamp.last_evidence_at = lamp.last_evidence_at.max(Some(item.at));
            }
            if item.relay_ok(side) {
                lamp.last_ok_at = lamp.last_ok_at.max(Some(item.at));
            }
        }
    }

    // Each window.
    let accounts = same_accounts(evidence);
    for side in Side::BOTH {
        let lamp = next.lamp(side).clone();
        let reading = read(side, &lamp, inputs, &accounts, &next.vote_losses);
        let voters = || reading.quorum.iter().map(|index| Voter::of(&evidence[*index])).collect::<Vec<_>>();
        let Some(change) = step(side, &lamp, &reading, evidence, now) else {
            // Offline, and a landing came that the failures after it still
            // outvote: the quorum is theirs now.
            if lamp.state == Light::Offline
                && reading.offline()
                && reading.newest_landing.is_some_and(|index| evidence[index].at > lamp.quorum_from())
            {
                next.lamp_mut(side).quorum = voters();
            }
            continue;
        };
        events.push(Event {
            scope: Scope::of(side),
            from: lamp.state.as_str().into(),
            to: change.to.as_str().into(),
            cause: change.cause,
            reason: change.reason,
        });
        // A landing ended Offline: the accounts of its quorum whose failure
        // it came after lose their vote here.
        if let Some(landed_at) = change.landed_at {
            for voter in lamp.quorum.iter().filter(|voter| voter.at < landed_at) {
                lose_vote(&mut next.vote_losses, side, voter, landed_at + VOTE_LOST_FOR);
            }
        }
        let quorum = if change.to == Light::Offline { voters() } else { Vec::new() };
        let lamp = next.lamp_mut(side);
        lamp.last_state = (change.to == Light::NoRecentNews).then_some(lamp.state);
        lamp.state = change.to;
        lamp.since = now;
        lamp.quorum = quorum;
        if change.landed_at.is_some() {
            lamp.fresh_from = change.landed_at;
        }
    }
    if next.vote_losses.len() > MAX_VOTE_LOSSES {
        next.vote_losses.sort_by_key(|loss| std::cmp::Reverse(loss.until));
        next.vote_losses.truncate(MAX_VOTE_LOSSES);
    }

    // The note: one set or cleared since, one run out, or (outage) ended by
    // a relay return that got in after it began. A return that got in
    // during maintenance is told to staff; maintenance ends at its end or by
    // an Admin.
    let mut note_cause = None;
    let mut note_ended = None;
    let mut note = inputs.note.cloned();
    if let Some(open) = &note {
        let landed_after =
            evidence.iter().any(|item| item.relay_landed() && item.at > open.starts_at && item.at <= now);
        if now >= open.until {
            note_ended = Some((open.id, NoteEnd::Expired));
        } else if open.kind == NoteKind::Outage && landed_after {
            note_ended = Some((open.id, NoteEnd::ReturnGotIn));
        } else if open.kind == NoteKind::Maintenance
            && open.in_force(now)
            && landed_after
            && next.maintenance_told != Some(open.id)
        {
            notices.push(Notice::LandedInMaintenance);
            next.maintenance_told = Some(open.id);
        }
    }
    let previous_note = previous.note.as_ref().map(|note| note.id);
    let mut unseen_expiry = false;
    if let Some((id, how)) = note_ended {
        note = None;
        note_cause = Some(match how {
            NoteEnd::Expired => Cause::NoteExpired,
            NoteEnd::ReturnGotIn => Cause::NoteReturnGotIn,
        });
        unseen_expiry = previous_note != Some(id);
    }
    let note = note.filter(|open| open.in_force(now));
    if note_cause.is_none() && note.as_ref().map(|note| note.id) != previous_note {
        note_cause = Some(if note.is_some() { Cause::NoteSet } else { Cause::NoteCleared });
    }

    // Overall.
    let state = overall(next.dispatcher.state, next.upload.state, note.as_ref());
    let note_kind = note.as_ref().map(|note| note.kind);
    next.note = note;
    if unseen_expiry && state == previous.state {
        // A note that ran out before any run saw it in force: no change of
        // the sign, but the event log says it ended.
        events.push(Event {
            scope: Scope::Overall,
            from: state.as_str().into(),
            to: state.as_str().into(),
            cause: Cause::NoteExpired,
            reason: "A note ran out before Buwiz saw it in force".into(),
        });
    } else if state != previous.state || note_cause.is_some() {
        let cause = note_cause
            .or_else(|| events.iter().rev().find(|event| event.scope == Scope::Upload).map(|event| event.cause))
            .or_else(|| events.last().map(|event| event.cause))
            .unwrap_or(Cause::Rule);
        if state != previous.state {
            next.last_state = (state == Light::NoRecentNews).then_some(previous.state);
            next.state = state;
            next.since = now;
        }
        if previous.state == Light::Offline && state != Light::Offline {
            next.waking_since = Some(now);
        }
        if state == Light::Offline {
            next.waking_since = None;
        }
        // An outage or maintenance note that ends starts waking too.
        if matches!(cause, Cause::NoteCleared | Cause::NoteExpired | Cause::NoteReturnGotIn)
            && previous.note.as_ref().is_some_and(|note| note.kind.holds())
            && state != Light::Offline
        {
            next.waking_since = Some(now);
        }
        let reason = match cause {
            Cause::NoteSet => match note_kind {
                Some(NoteKind::Outage) => "An Admin set an outage note".into(),
                Some(NoteKind::Maintenance) => "An Admin set a planned maintenance note".into(),
                _ => "An Admin declared the BIR online, ignoring evidence".into(),
            },
            Cause::NoteCleared => "An Admin cleared the note".into(),
            Cause::NoteExpired => "The note ran out".into(),
            Cause::NoteReturnGotIn => "A return got in, which ends the note".into(),
            _ => overall_reason(&next),
        };
        events.push(Event {
            scope: Scope::Overall,
            from: previous.state.as_str().into(),
            to: state.as_str().into(),
            cause,
            reason,
        });
    }

    // Failures in a row: counted, one per account, after the newest return
    // that got in (staff hear once per run of them).
    let streak_after = evidence.iter().filter(|item| item.landed() && item.at <= now).map(|item| item.at).max();
    let streak_after = streak_after.max(next.last_success_at);
    let mut failed = HashSet::new();
    for (index, item) in evidence.iter().enumerate() {
        if item.counted
            && !item.landed()
            && item.at <= now
            && item.linked(inputs.periods)
            && streak_after.is_none_or(|after| item.at > after)
        {
            failed.insert(accounts[index]);
        }
    }
    let streak_id = to_minute(streak_after.unwrap_or(OffsetDateTime::UNIX_EPOCH));
    if failed.len() >= STREAK_FAILURES && next.streak_told != Some(streak_id) {
        notices.push(Notice::FailuresInARow { failures: failed.len(), since: streak_after });
        next.streak_told = Some(streak_id);
    }

    // The relay pause: resumed by an Admin (a later release's staff page),
    // and its trigger told to staff. This release never pauses it.
    if inputs.resume_relay && next.relay.paused {
        next.relay = Relay { paused: false, since: Some(now), reason: None, resumed_at: Some(now) };
        events.push(Event {
            scope: Scope::Relay,
            from: "paused".into(),
            to: "ok".into(),
            cause: Cause::RelayResumed,
            reason: "An Admin resumed sending".into(),
        });
    } else if !next.relay.paused
        && let Some(reason) = would_pause(evidence, inputs.periods, next.relay.resumed_at, now)
    {
        notices.push(Notice::RelayWouldPause { reason });
    }

    if !events.is_empty() || not_watched.is_some() {
        next.seq = previous.seq + 1;
    }
    if next.visible() != previous.visible() {
        let millis = i64::try_from(now.unix_timestamp_nanos() / 1_000_000).unwrap_or(i64::MAX);
        next.revision = (previous.revision + 1).max(millis);
    }
    Evaluation { state: next, events, note_ended, not_watched, notices }
}

/// The pause's trigger, if met: 3 tries of the pausing kinds within 60
/// minutes, from 2 accounts on 2 networks, after the last resume.
fn would_pause(
    evidence: &[Evidence],
    periods: &[i32],
    resumed_at: Option<OffsetDateTime>,
    now: OffsetDateTime,
) -> Option<String> {
    let faults: Vec<&Evidence> = evidence
        .iter()
        .filter(|item| item.pauses_relay() && item.at >= now - PAUSE_WITHIN && item.at <= now)
        .filter(|item| resumed_at.is_none_or(|resumed| item.at >= resumed))
        .filter(|item| item.linked(periods))
        .collect();
    if faults.len() < PAUSE_TRIES {
        return None;
    }
    let accounts = spread(&faults, periods, PAUSE_SPREAD);
    if accounts < PAUSE_SPREAD {
        return None;
    }
    let count = |kind: Kind| faults.iter().filter(|item| item.kind == Some(kind)).count();
    let parts: Vec<String> = [
        (Kind::HostKey, "a host key that is not pinned"),
        (Kind::UnknownHost, "an unknown upload server"),
        (Kind::Protocol, "an answer Buwiz could not read"),
    ]
    .into_iter()
    .filter(|(kind, _)| count(*kind) > 0)
    .map(|(kind, words)| format!("{words}: {}", count(kind)))
    .collect();
    Some(format!(
        "{} tries from {accounts} accounts on {accounts} networks met a fault on Buwiz's side that touches the BIR ({})",
        faults.len(),
        parts.join(", ")
    ))
}

impl Evaluation {
    /// Whether the stored sign must be written now: an event, a note that
    /// ended, a gap, or anything but the times of news and of the
    /// evaluation changed. Those alone are written at most every minute.
    pub fn must_write(&self, previous: &SignState) -> bool {
        !self.events.is_empty()
            || self.note_ended.is_some()
            || self.not_watched.is_some()
            || self.state.durable() != previous.durable()
    }
}

#[cfg(test)]
mod tests;
