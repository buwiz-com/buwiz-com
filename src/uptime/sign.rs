//! The rule engine: the sign, from the tries people made anyway
//! (docs/bir-uptime.md §2.1). Pure and deterministic: [`evaluate`] reads
//! the previous sign, the recent tries, the open note and the clock, and
//! says the next sign and what changed. Nothing here touches a database or
//! the network; [`super::live`] keeps it and writes it.
//!
//! Only counted `live` tries are evidence (a try that got in, or failed on
//! the BIR's side). A try of class `user_network` or `ours` never moves a
//! light; three kinds of `ours` (a host key that is not pinned, an unknown
//! upload server, a protocol error) can pause the relay.
//!
//! Accounts and networks are the day-keyed hashes of [`super::keys`]. Two
//! tries are the same account only when they share a key of the same key
//! period; a try recorded in the 3 hours after a new day key carries the
//! previous key too, which is how tries across midnight compare. Keys of
//! periods nothing links are never taken for the same account, nor counted
//! as different ones: a quorum is counted within one key period, so after
//! a restart (a new key, nothing linking it to the old one) it needs fresh
//! evidence.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::classify::{Kind, Outcome, Stage, Window};
use super::keys::Keys;

/// How many tries "the last 10" holds.
pub const LAST_N: usize = 10;
/// The most of the last 10 one account fills.
pub const PER_ACCOUNT: usize = 2;
/// How far back the last 10 reaches.
pub const LAST_WITHIN: Duration = Duration::minutes(60);
/// Online: at most this many failures among the last 10 (and the newest
/// got in).
pub const ONLINE_MAX_FAILURES: usize = 1;
/// Busy: at least this many failures among the last 10 ...
pub const BUSY_FAILURES: usize = 3;
/// ... from at least this many accounts on as many networks.
pub const BUSY_SPREAD: usize = 2;
/// Offline: this many accounts on as many networks failed ...
pub const OFFLINE_SPREAD: usize = 3;
/// ... within this long, and none got in since.
pub const OFFLINE_WITHIN: Duration = Duration::minutes(15);
/// A change that lets more through (Busy to Online or No recent news)
/// waits until Busy is this old.
pub const DWELL: Duration = Duration::minutes(10);
/// No evidence on a window for this long: No recent news.
pub const QUIET: Duration = Duration::minutes(60);
/// Offline with no evidence for this long: No recent news.
pub const OFFLINE_QUIET: Duration = Duration::hours(3);
/// An account whose failure a landing disproved loses its Offline vote for
/// this long ...
pub const VOTE_LOST_FOR: Duration = Duration::hours(24);
/// ... when it failed within this long before the landing.
pub const DISPROVED_WITHIN: Duration = Duration::minutes(15);
/// The relay pauses after this many tries of the pausing kinds ...
pub const PAUSE_TRIES: usize = 3;
/// ... from this many accounts ...
pub const PAUSE_ACCOUNTS: usize = 2;
/// ... within this long.
pub const PAUSE_WITHIN: Duration = Duration::minutes(60);
/// How far back the engine is given tries.
pub const EVIDENCE_KEPT: Duration = Duration::hours(3);
/// The most vote losses kept.
pub const MAX_VOTE_LOSSES: usize = 2_000;
/// At most this many direct-app reports are tried as the one direct
/// account of a quorum.
const DIRECT_CANDIDATES: usize = 20;

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
    /// When it ended, to the minute (as `bir_tries` keeps it).
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
    /// reached that window (the front desk over TLS only).
    pub fn hit(&self, side: Side) -> Option<Hit> {
        if !self.counted {
            return None;
        }
        match (side, side_window(self, side)) {
            (_, Window::Ok) => Some(Hit::Ok),
            (_, Window::Fail) => Some(Hit::Fail),
            (Side::Upload, Window::Maybe) => Some(Hit::Maybe),
            _ => None,
        }
    }

    /// A relay try that got past `side`: only these end Offline and
    /// disprove votes.
    fn relay_ok(&self, side: Side) -> bool {
        self.source == Source::Relay && self.hit(side) == Some(Hit::Ok)
    }

    /// One of the faults on Buwiz's side that still touch the BIR.
    pub fn pauses_relay(&self) -> bool {
        self.source == Source::Relay && matches!(self.kind, Some(Kind::HostKey | Kind::UnknownHost | Kind::Protocol))
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
    /// From the news: overall Offline, scouts go on.
    Outage,
    /// Announced: overall Offline, no scouts.
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

/// One light.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lamp {
    pub state: Light,
    #[serde(with = "time::serde::rfc3339")]
    pub since: OffsetDateTime,
    /// With `no_recent_news`: the state before the quiet.
    pub last_state: Option<Light>,
    /// The last relay try that got past this window.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_ok_at: Option<OffsetDateTime>,
    /// The last evidence of any outcome on this window.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_evidence_at: Option<OffsetDateTime>,
    /// Set when the window leaves Offline: its last 10 counts from here.
    #[serde(with = "time::serde::rfc3339::option")]
    pub fresh_from: Option<OffsetDateTime>,
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
        }
    }
}

/// The relay's own state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relay {
    pub paused: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub since: Option<OffsetDateTime>,
    /// Why it paused: a fixed sentence with counts.
    pub reason: Option<String>,
    /// When an Admin last resumed it: only faults after this count.
    #[serde(with = "time::serde::rfc3339::option")]
    pub resumed_at: Option<OffsetDateTime>,
}

/// An Offline vote a landing disproved, for one window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoteLoss {
    pub side: Side,
    /// In memory only: the account, for the whole 24 hours.
    #[serde(skip)]
    pub account_id: Option<Uuid>,
    /// The account's keys (key period, key), for tries read back from the
    /// record, which carry no account id.
    pub keys: Vec<(i32, [u8; 8])>,
    #[serde(with = "time::serde::rfc3339")]
    pub until: OffsetDateTime,
}

impl VoteLoss {
    fn covers(&self, side: Side, evidence: &Evidence) -> bool {
        self.side == side
            && (self.account_id.is_some() && self.account_id == evidence.account_id
                || evidence.keys.iter().any(|keys| self.keys.contains(&(keys.epoch, keys.account))))
    }
}

/// The sign as the engine keeps it. `seq` grows by one with every change
/// and survives restarts (`bir_sign`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignState {
    pub seq: i64,
    pub state: Light,
    #[serde(with = "time::serde::rfc3339")]
    pub since: OffsetDateTime,
    /// With `no_recent_news`: the state before the quiet began.
    pub last_state: Option<Light>,
    #[serde(with = "time::serde::rfc3339")]
    pub evaluated_at: OffsetDateTime,
    pub dispatcher: Lamp,
    pub upload: Lamp,
    /// The note in force at the last evaluation.
    pub note: Option<Note>,
    pub relay: Relay,
    /// The last relay return that got in.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_success_at: Option<OffsetDateTime>,
    /// The last counted try of any outcome.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_news_at: Option<OffsetDateTime>,
    /// Since when the server is waking (Offline, or an outage or
    /// maintenance note, ended). The doorman (a later release) ends it.
    #[serde(with = "time::serde::rfc3339::option")]
    pub waking_since: Option<OffsetDateTime>,
    pub vote_losses: Vec<VoteLoss>,
}

impl SignState {
    /// The first sign: nothing known.
    pub fn new(now: OffsetDateTime) -> Self {
        Self {
            seq: 0,
            state: Light::NoRecentNews,
            since: now,
            last_state: None,
            evaluated_at: now,
            dispatcher: Lamp::new(now),
            upload: Lamp::new(now),
            note: None,
            relay: Relay { paused: false, since: None, reason: None, resumed_at: None },
            last_success_at: None,
            last_news_at: None,
            waking_since: None,
            vote_losses: Vec::new(),
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
}

/// What an event is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Overall,
    Dispatcher,
    Upload,
    Relay,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Overall => "overall",
            Self::Dispatcher => "dispatcher",
            Self::Upload => "upload",
            Self::Relay => "relay",
        }
    }

    fn of(side: Side) -> Self {
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
    /// The `live` tries of the last 3 hours, oldest first.
    pub evidence: &'a [Evidence],
    /// How many of the newest in `evidence` are new since the previous
    /// evaluation: only these end Offline, disprove votes and end a note.
    pub fresh: usize,
    /// The newest note not yet cleared (it may have run out).
    pub note: Option<&'a Note>,
    /// An Admin resumes the relay.
    pub resume_relay: bool,
    pub now: OffsetDateTime,
}

/// The next sign, what changed, and a note that ended by itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluation {
    pub state: SignState,
    pub events: Vec<Event>,
    pub note_ended: Option<(Uuid, NoteEnd)>,
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
/// accounts and pairwise different networks (a bipartite matching).
fn matching(pairs: &[([u8; 8], [u8; 8])]) -> usize {
    let accounts: Vec<[u8; 8]> = {
        let mut seen = HashSet::new();
        pairs.iter().map(|(account, _)| *account).filter(|account| seen.insert(*account)).collect()
    };
    let mut networks_of: Vec<Vec<[u8; 8]>> = vec![Vec::new(); accounts.len()];
    for (account, network) in pairs {
        let at = accounts.iter().position(|known| known == account).unwrap_or_default();
        if !networks_of[at].contains(network) {
            networks_of[at].push(*network);
        }
    }
    let mut taken: HashMap<[u8; 8], usize> = HashMap::new();
    fn augment(
        account: usize,
        networks_of: &[Vec<[u8; 8]>],
        taken: &mut HashMap<[u8; 8], usize>,
        visited: &mut HashSet<[u8; 8]>,
    ) -> bool {
        for network in &networks_of[account] {
            if !visited.insert(*network) {
                continue;
            }
            let free = match taken.get(network) {
                None => true,
                Some(&other) => augment(other, networks_of, taken, visited),
            };
            if free {
                taken.insert(*network, account);
                return true;
            }
        }
        false
    }
    (0..accounts.len()).filter(|account| augment(*account, &networks_of, &mut taken, &mut HashSet::new())).count()
}

/// How many accounts on as many networks `items` come from: the largest
/// matching within one key period, with at most one direct-app account.
/// Tries without a network (late direct reports) add nothing.
fn spread(items: &[&Evidence]) -> usize {
    let epochs: HashSet<i32> = items.iter().flat_map(|item| item.keys.iter().map(|keys| keys.epoch)).collect();
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
        let mut here = matching(&relay);
        let mut direct = pairs_of(Source::Direct);
        direct.dedup();
        for pair in direct.into_iter().take(DIRECT_CANDIDATES) {
            let mut with = relay.clone();
            with.push(pair);
            here = here.max(matching(&with));
        }
        best = best.max(here);
    }
    best
}

/// How many accounts `items` come from, counted within one key period.
fn accounts_in(items: &[&Evidence]) -> usize {
    let mut by_epoch: HashMap<i32, HashSet<[u8; 8]>> = HashMap::new();
    for item in items {
        for keys in &item.keys {
            by_epoch.entry(keys.epoch).or_default().insert(keys.account);
        }
    }
    by_epoch.values().map(HashSet::len).max().unwrap_or(0)
}

/// What one window's evidence says now.
struct Reading {
    /// The last 10, newest first: (index, hit).
    last: Vec<(usize, Hit)>,
    oks: usize,
    failures: usize,
    failure_spread: usize,
    /// Offline's quorum: accounts on as many networks.
    offline_spread: usize,
}

impl Reading {
    fn busy(&self) -> bool {
        self.oks >= 1 && self.failures >= BUSY_FAILURES && self.failure_spread >= BUSY_SPREAD
    }

    fn online(&self) -> bool {
        self.last.first().is_some_and(|(_, hit)| *hit == Hit::Ok) && self.failures <= ONLINE_MAX_FAILURES
    }

    fn offline(&self) -> bool {
        self.offline_spread >= OFFLINE_SPREAD
    }
}

fn read(side: Side, lamp: &Lamp, inputs: &Inputs<'_>, accounts: &[usize], losses: &[VoteLoss]) -> Reading {
    let now = inputs.now;
    let evidence = inputs.evidence;
    // The last 10: newest first, within 60 minutes, since the window last
    // left Offline, at most 2 per account.
    let mut last = Vec::new();
    let mut per_account: HashMap<usize, usize> = HashMap::new();
    for (index, item) in evidence.iter().enumerate().rev() {
        if last.len() == LAST_N {
            break;
        }
        if item.at < now - LAST_WITHIN || lamp.fresh_from.is_some_and(|from| item.at < from) {
            continue;
        }
        let Some(hit) = item.hit(side) else {
            continue;
        };
        let filled = per_account.entry(accounts[index]).or_default();
        if *filled == PER_ACCOUNT {
            continue;
        }
        *filled += 1;
        last.push((index, hit));
    }
    let oks = last.iter().filter(|(_, hit)| *hit == Hit::Ok).count();
    let failing: Vec<&Evidence> =
        last.iter().filter(|(_, hit)| *hit != Hit::Ok).map(|(index, _)| &evidence[*index]).collect();

    // Offline: failures within 15 minutes, all after the newest relay try
    // that got past this window, from accounts that kept their vote.
    let newest_ok = evidence.iter().rposition(|item| item.relay_ok(side));
    let quorum: Vec<&Evidence> = evidence
        .iter()
        .enumerate()
        .filter(|(index, _)| newest_ok.is_none_or(|ok| *index > ok))
        .map(|(_, item)| item)
        .filter(|item| item.at >= now - OFFLINE_WITHIN && item.hit(side) == Some(Hit::Fail))
        .filter(|item| !losses.iter().any(|loss| loss.until > now && loss.covers(side, item)))
        .collect();
    Reading { oks, failures: failing.len(), failure_spread: spread(&failing), offline_spread: spread(&quorum), last }
}

/// A lamp's change: its new state, why, and the sentence.
struct Change {
    to: Light,
    cause: Cause,
    reason: String,
    fresh_from: Option<OffsetDateTime>,
}

fn step(side: Side, lamp: &Lamp, reading: &Reading, landing: Option<&Evidence>, now: OffsetDateTime) -> Option<Change> {
    let change = |to, cause, reason: String| Some(Change { to, cause, reason, fresh_from: None });
    let quiet_for = |limit: Duration| lamp.last_evidence_at.is_none_or(|at| now - at >= limit);
    let last_n = reading.last.len();
    match lamp.state {
        Light::Offline => {
            if let Some(landing) = landing {
                let (cause, reason) = match landing.admission {
                    Admission::Scout | Admission::LegacyScout => (Cause::Scout, "A scout got in"),
                    Admission::TryAnyway => (Cause::TryAnyway, "A Try anyway got in"),
                    Admission::Open | Admission::Admit => (Cause::Rule, "A return got in"),
                };
                let reason = format!("{reason} past the {}", side.word());
                return Some(Change { to: Light::Online, cause, reason, fresh_from: Some(landing.at) });
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
            if reading.busy() {
                change(Light::Busy, Cause::Rule, busy_reason(reading))
            } else if quiet_for(QUIET) {
                change(Light::NoRecentNews, Cause::Quiet, "Nobody filed in the last 60 min".into())
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
                change(Light::NoRecentNews, Cause::Quiet, "Nobody filed in the last 60 min".into())
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
    let fresh_from = evidence.len().saturating_sub(inputs.fresh);
    let fresh = &evidence[fresh_from..];
    let mut next = previous.clone();
    next.evaluated_at = now;
    let mut events = Vec::new();

    // What the newest tries tell, before any rule: the times, and the votes
    // a landing disproves.
    next.vote_losses.retain(|loss| loss.until > now);
    for (offset, item) in fresh.iter().enumerate() {
        let index = fresh_from + offset;
        if item.counted {
            next.last_news_at = next.last_news_at.max(Some(item.at));
        }
        if item.source == Source::Relay && item.counted && item.outcome == Outcome::Sent {
            next.last_success_at = next.last_success_at.max(Some(item.at));
        }
        for side in Side::BOTH {
            if item.hit(side).is_some() {
                let lamp = next.lamp_mut(side);
                lamp.last_evidence_at = lamp.last_evidence_at.max(Some(item.at));
            }
            if !item.relay_ok(side) {
                continue;
            }
            let lamp = next.lamp_mut(side);
            lamp.last_ok_at = lamp.last_ok_at.max(Some(item.at));
            for earlier in &evidence[..index] {
                if earlier.at >= item.at - DISPROVED_WITHIN && earlier.hit(side) == Some(Hit::Fail) {
                    let keys: Vec<(i32, [u8; 8])> =
                        earlier.keys.iter().map(|keys| (keys.epoch, keys.account)).collect();
                    let loss = VoteLoss { side, account_id: earlier.account_id, keys, until: item.at + VOTE_LOST_FOR };
                    if !next.vote_losses.contains(&loss) {
                        next.vote_losses.push(loss);
                    }
                }
            }
        }
    }
    if next.vote_losses.len() > MAX_VOTE_LOSSES {
        let extra = next.vote_losses.len() - MAX_VOTE_LOSSES;
        next.vote_losses.drain(..extra);
    }

    // Each window.
    let accounts = same_accounts(evidence);
    for side in Side::BOTH {
        let lamp = next.lamp(side).clone();
        let reading = read(side, &lamp, inputs, &accounts, &next.vote_losses);
        let landing = fresh.iter().rev().find(|item| item.relay_ok(side));
        if let Some(change) = step(side, &lamp, &reading, landing, now) {
            events.push(Event {
                scope: Scope::of(side),
                from: lamp.state.as_str().into(),
                to: change.to.as_str().into(),
                cause: change.cause,
                reason: change.reason,
            });
            let lamp = next.lamp_mut(side);
            lamp.last_state = (change.to == Light::NoRecentNews).then_some(lamp.state);
            lamp.state = change.to;
            lamp.since = now;
            if change.fresh_from.is_some() {
                lamp.fresh_from = change.fresh_from;
            }
        }
    }

    // The note: one set or cleared since, one run out, or (outage and
    // maintenance) ended by a relay return that got in after it began.
    let mut note_cause = None;
    let mut note_ended = None;
    let mut note = inputs.note.cloned();
    if let Some(open) = &note {
        if now >= open.until {
            note_ended = Some((open.id, NoteEnd::Expired));
        } else if open.kind.holds()
            && fresh.iter().any(|item| {
                item.source == Source::Relay
                    && item.counted
                    && item.outcome == Outcome::Sent
                    && item.at >= super::store::to_minute(open.starts_at)
            })
        {
            note_ended = Some((open.id, NoteEnd::ReturnGotIn));
        }
    }
    if let Some((_, how)) = note_ended {
        note = None;
        note_cause = Some(match how {
            NoteEnd::Expired => Cause::NoteExpired,
            NoteEnd::ReturnGotIn => Cause::NoteReturnGotIn,
        });
    }
    let note = note.filter(|open| open.in_force(now));
    let previous_note = previous.note.as_ref().map(|note| note.id);
    if note_cause.is_none() && note.as_ref().map(|note| note.id) != previous_note {
        note_cause = Some(if note.is_some() { Cause::NoteSet } else { Cause::NoteCleared });
    }
    // A note that ran out before it was ever seen in force changes nothing.
    if note_cause.is_some_and(|cause| cause != Cause::NoteSet) && previous_note.is_none() {
        note_cause = None;
    }

    // Overall.
    let state = overall(next.dispatcher.state, next.upload.state, note.as_ref());
    let note_kind = note.as_ref().map(|note| note.kind);
    next.note = note;
    if state != previous.state || note_cause.is_some() {
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

    // The relay pause.
    if inputs.resume_relay && next.relay.paused {
        next.relay = Relay { paused: false, since: Some(now), reason: None, resumed_at: Some(now) };
        events.push(Event {
            scope: Scope::Relay,
            from: "paused".into(),
            to: "ok".into(),
            cause: Cause::RelayResumed,
            reason: "An Admin resumed sending".into(),
        });
    } else if !next.relay.paused {
        let faults: Vec<&Evidence> = evidence
            .iter()
            .filter(|item| item.pauses_relay() && item.at >= now - PAUSE_WITHIN)
            .filter(|item| next.relay.resumed_at.is_none_or(|resumed| item.at >= resumed))
            .collect();
        let accounts = accounts_in(&faults);
        if faults.len() >= PAUSE_TRIES && accounts >= PAUSE_ACCOUNTS {
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
            let reason = format!(
                "{} tries from {accounts} accounts met a fault on Buwiz's side that touches the BIR ({})",
                faults.len(),
                parts.join(", ")
            );
            next.relay = Relay {
                paused: true,
                since: Some(now),
                reason: Some(reason.clone()),
                resumed_at: next.relay.resumed_at,
            };
            events.push(Event {
                scope: Scope::Relay,
                from: "ok".into(),
                to: "paused".into(),
                cause: Cause::RelayPaused,
                reason,
            });
        }
    }

    if !events.is_empty() {
        next.seq = previous.seq + 1;
    }
    Evaluation { state: next, events, note_ended }
}

impl Evaluation {
    /// Whether the stored sign must be written: anything but the time of
    /// the evaluation changed (a state, a time of news, a lost vote).
    pub fn changed(&self, previous: &SignState) -> bool {
        let mut same = previous.clone();
        same.evaluated_at = self.state.evaluated_at;
        same != self.state || self.note_ended.is_some()
    }
}

#[cfg(test)]
mod tests;
