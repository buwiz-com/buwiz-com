//! Staff mail: Admins and Support (and `ADMIN_EMAILS`) hear of every change
//! of the overall sign, a window going Offline or coming back (also when the
//! overall sign does not change, under a "declare Online" note say), every
//! note set, cleared or ended, a reset of the stored sign, five failures in
//! a row, "the relay would pause" (nothing pauses in this release), a
//! missing `CF-Connecting-IP` in production, and a fault on Buwiz's side
//! that needs a person (an upload-server host key that is not pinned, an
//! unknown upload server, an answer Buwiz could not read, a Sender that
//! cannot be reached).
//!
//! Batched: the first alert mails at once, the ones in the next 10 minutes
//! go together at the end of them. Each kind has its own daily budget (a
//! Manila day), so one cannot spend another's: changes of the sign and of
//! notes 48, host keys 3 across every fingerprint, everything else 8. An
//! alert past its budget is not mailed (the event log keeps every change)
//! and is not counted as told, so the same alert tomorrow is mailed. An
//! alert is told, and its events marked `mailed_at`, only once its mail
//! went to at least one address; a mail that went nowhere stays queued and
//! is tried again a minute later, spending no budget. Plain short text with
//! Manila times and counts, never a TIN, a name, an address or an account
//! of anyone who filed. The queue is in memory: alerts not yet mailed at a
//! restart are lost (`bir_events.mailed_at` says which changes went out).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use sqlx::PgPool;
use time::{Date, Duration, OffsetDateTime};

use super::Uptime;
use super::classify::Kind;
use super::keys::{MANILA, manila_day};
use super::live::{self, Written};
use super::sign::{Cause, Light, NoteKind, Notice, Scope, Side, SignState};
use crate::account::mail::Mailer;
use crate::account::roles;
use crate::base::config::Config;

/// Alerts within this long of the last mail wait and go together.
pub const BATCH_EVERY: Duration = Duration::minutes(10);
/// A mail that left an address unreached is tried again after 1, 2, 4 …
/// minutes, at most this many apart ...
pub const MAX_BACKOFF_MINUTES: i64 = 60;
/// ... and given up (logged) this long after its first try.
pub const GIVE_UP_AFTER: Duration = Duration::days(1);
/// The most mails on their way at once.
pub const MAX_OUTBOX: usize = 50;
/// The most alerts queued.
pub const MAX_QUEUED: usize = 200;
/// The most "told" keys remembered.
pub const MAX_TOLD: usize = 1_000;

/// Whose daily budget an alert spends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Budget {
    /// Changes of the sign, a window, a note: always wanted.
    Sign,
    /// Upload-server host keys, across every fingerprint.
    HostKey,
    /// Everything else.
    Other,
}

impl Budget {
    pub const ALL: [Budget; 3] = [Self::Sign, Self::HostKey, Self::Other];

    /// Alerts a Manila day.
    pub fn per_day(self) -> u32 {
        match self {
            Self::Sign => 48,
            Self::HostKey => 3,
            Self::Other => 8,
        }
    }

    fn words(self) -> &'static str {
        match self {
            Self::Sign => "change of the sign",
            Self::HostKey => "host-key alert",
            Self::Other => "other alert",
        }
    }
}

/// One thing staff hear of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alert {
    pub at: OffsetDateTime,
    pub subject: String,
    pub line: String,
    /// The event it tells of, marked `mailed_at` once mailed.
    pub event_id: Option<i64>,
    /// Alerts with the same key are queued once and, once mailed, told
    /// again only after `repeat_after`.
    pub key: Option<(String, Duration)>,
    pub budget: Budget,
}

/// `h:mm AM` in Manila.
pub fn manila_time(at: OffsetDateTime) -> String {
    let local = at.to_offset(MANILA);
    let (hour, minute) = (local.hour(), local.minute());
    let twelve = match hour % 12 {
        0 => 12,
        hour => hour,
    };
    format!("{twelve}:{minute:02} {}", if hour < 12 { "AM" } else { "PM" })
}

fn sentence(text: &str) -> String {
    let text = text.trim_end_matches('.');
    format!("{text}.")
}

impl Alert {
    fn sign(at: OffsetDateTime, subject: String, line: String, event_id: Option<i64>) -> Self {
        Self { at, subject, line, event_id, key: None, budget: Budget::Sign }
    }

    /// Something no change of the sign, on the small "other" budget.
    fn other(at: OffsetDateTime, subject: String, line: String) -> Self {
        Self { at, subject, line, event_id: None, key: None, budget: Budget::Other }
    }

    /// What one run of the rules wrote that staff hear of (the owner's rule
    /// of 2026-11-14): the overall sign going Offline or coming back from
    /// Offline (with the windows' reasons of the same run), a window going
    /// Offline while a "declare Online" note is in force, a note's line,
    /// the stored sign reset, and the relay resuming. Other changes are in
    /// the event log only.
    pub fn for_run(written: &[Written], state: &SignState) -> Vec<Self> {
        let windows: Vec<String> = written
            .iter()
            .filter(|written| matches!(written.event.scope, Scope::Dispatcher | Scope::Upload))
            .map(|written| sentence(&written.event.reason))
            .collect();
        let overall_changed = written
            .iter()
            .any(|written| written.event.scope == Scope::Overall && written.event.from != written.event.to);
        let mut alerts = Vec::new();
        for written in written {
            let event = &written.event;
            let (to, from) = (Light::parse(&event.to), Light::parse(&event.from));
            let subject = match event.scope {
                // The first line of the history is no change.
                Scope::Overall if event.from == "none" => continue,
                // The owner's rule: of the sign's own changes, only going
                // Offline and coming back from it are mailed. Notes and a
                // reset always are.
                Scope::Overall
                    if !mailed_cause(event.cause)
                        && event.from != event.to
                        && to != Some(Light::Offline)
                        && from != Some(Light::Offline) =>
                {
                    continue;
                }
                Scope::Overall if !mailed_cause(event.cause) && event.from == event.to => continue,
                Scope::Overall => overall_subject(event.cause, to, from, state),
                Scope::Relay if event.to == "paused" => "Buwiz: sending paused".to_owned(),
                Scope::Relay => "Buwiz: sending resumed".to_owned(),
                Scope::Dispatcher | Scope::Upload => {
                    // Only a window going Offline, or coming back from it,
                    // under a "declare Online" note, which the overall sign
                    // does not show.
                    let declared = state.note.as_ref().is_some_and(|note| note.kind == NoteKind::Online);
                    let offline_or_back = (to == Some(Light::Offline)) != (from == Some(Light::Offline));
                    if overall_changed || !declared || !offline_or_back {
                        continue;
                    }
                    alerts.push(window_alert(written, state));
                    continue;
                }
                Scope::Watch => continue,
            };
            let mut line = sentence(&event.reason);
            if event.scope == Scope::Overall && event.from != event.to {
                line = format!("{}: {line}", to.map_or("", word));
                for reason in &windows {
                    line.push(' ');
                    line.push_str(reason);
                }
            }
            alerts.push(Alert::sign(written.at, subject, line, Some(written.id)));
        }
        alerts
    }

    /// Something the rules noticed that is no change of the sign.
    pub fn notice(notice: &Notice, at: OffsetDateTime) -> Self {
        match notice {
            Notice::FailuresInARow { failures, since } => Alert::other(
                at,
                "BIR: tries keep failing".to_owned(),
                match since {
                    Some(since) => format!(
                        "{failures} tries in a row from {failures} accounts did not get in; the last return got in at {}.",
                        manila_time(*since)
                    ),
                    None => format!("{failures} tries in a row from {failures} accounts did not get in."),
                },
            ),
            Notice::RelayWouldPause { reason } => Self {
                at,
                subject: "Buwiz: the relay would pause".to_owned(),
                line: format!(
                    "The relay would pause: {}. Nothing is held in this release; check the fault (a key is pinned only from a second source the owner trusts, never from this mail).",
                    reason.trim_end_matches('.')
                ),
                event_id: None,
                // Told once, and again at most once a day while it holds.
                key: Some(("relay_would_pause".to_owned(), Duration::hours(24))),
                budget: Budget::Other,
            },
            Notice::LandedInMaintenance => Alert::other(
                at,
                "BIR: a return got in during maintenance".to_owned(),
                "A return got in while the planned maintenance note is in force. The note stays until it ends or an Admin clears it.".to_owned(),
            ),
        }
    }

    /// A live try met an upload-server host key that is not pinned. The
    /// fingerprint is what the user's app relayed: it can be forged.
    pub fn host_key(fingerprint: &str, at: OffsetDateTime) -> Self {
        Self {
            at,
            subject: "Buwiz: an upload-server key that is not pinned".into(),
            line: format!(
                "A send met an upload-server host key that is not pinned: {fingerprint} (relay-reported, unverified: do not pin from this mail)."
            ),
            event_id: None,
            key: Some((format!("host_key {fingerprint}"), Duration::hours(24))),
            budget: Budget::HostKey,
        }
    }

    /// Another fault on Buwiz's side that needs a person, by its kind.
    pub fn ours(kind: Kind, at: OffsetDateTime) -> Option<Self> {
        let (subject, line) = match kind {
            Kind::HostKey => (
                "Buwiz: an upload-server key that is not pinned",
                "A send met an upload-server host key that is not pinned; its fingerprint could not be read.",
            ),
            Kind::UnknownHost => (
                "Buwiz: the front desk named an unknown upload server",
                "The BIR's front desk named an upload server that is not on the Sender's list.",
            ),
            Kind::Protocol => (
                "Buwiz: an answer from the BIR Buwiz could not read",
                "A send got an answer from the BIR that Buwiz could not read (front desk, or no common SSH algorithm).",
            ),
            Kind::SenderDown => ("Buwiz: the Sender cannot be reached", "A send could not reach the Sender."),
            _ => return None,
        };
        Some(Self {
            at,
            subject: subject.into(),
            line: line.into(),
            event_id: None,
            key: Some((format!("ours {}", kind.as_str()), Duration::hours(1))),
            budget: if kind == Kind::HostKey { Budget::HostKey } else { Budget::Other },
        })
    }

    /// Production got a request without `CF-Connecting-IP`.
    pub fn missing_header(at: OffsetDateTime) -> Self {
        Self {
            at,
            subject: "Buwiz: requests without CF-Connecting-IP".into(),
            line: "A request reached the server without CF-Connecting-IP. Every such send counts as one network, so the sign is harder to move. Check the tunnel.".into(),
            event_id: None,
            key: None,
            budget: Budget::Other,
        }
    }
}

/// A window went Offline while a "declare Online" note keeps the overall
/// sign Online.
fn window_alert(written: &Written, state: &SignState) -> Alert {
    let event = &written.event;
    let side = if event.scope == Scope::Dispatcher { Side::Dispatcher } else { Side::Upload };
    let to = Light::parse(&event.to).unwrap_or(Light::NoRecentNews);
    let why = match state.note.as_ref().map(|note| note.kind) {
        Some(NoteKind::Online) => ", as an Admin declared",
        Some(_) => ", as an Admin's note says",
        None => "",
    };
    let line = format!(
        "The {} is {}: {} The sign shows {}{why}.",
        side.word(),
        word(to).to_lowercase(),
        sentence(&event.reason),
        word(state.state),
    );
    let subject = if to == Light::Offline {
        format!("BIR: {} offline", side.word())
    } else {
        format!("BIR: {} back", side.word())
    };
    Alert::sign(written.at, subject, line, Some(written.id))
}

/// Causes whose overall line is always mailed: an Admin's note, a note
/// ending, and a stored sign that could not be read.
fn mailed_cause(cause: Cause) -> bool {
    matches!(cause, Cause::NoteSet | Cause::NoteCleared | Cause::NoteExpired | Cause::NoteReturnGotIn | Cause::Reset)
}

fn word(light: Light) -> &'static str {
    match light {
        Light::Online => "Online",
        Light::Busy => "Busy",
        Light::Offline => "Offline",
        Light::NoRecentNews => "No recent news",
    }
}

fn overall_subject(cause: Cause, to: Option<Light>, from: Option<Light>, state: &SignState) -> String {
    match cause {
        Cause::NoteSet => return "BIR: an Admin set a note".into(),
        Cause::NoteCleared => return "BIR: an Admin cleared the note".into(),
        Cause::NoteExpired | Cause::NoteReturnGotIn => return "BIR: the note ended".into(),
        Cause::Reset => return "Buwiz: the stored sign could not be read".into(),
        _ => {}
    }
    match to {
        Some(Light::Offline) => {
            let offline: Vec<&str> = Side::BOTH
                .into_iter()
                .filter(|side| state.lamp(*side).state == Light::Offline)
                .map(Side::word)
                .collect();
            match offline.as_slice() {
                [one] => format!("BIR: {one} offline"),
                _ => "BIR: offline".into(),
            }
        }
        Some(Light::Busy) => "BIR: busy".into(),
        Some(Light::Online) if from.is_some_and(|from| from != Light::NoRecentNews) => "BIR: back online".into(),
        Some(Light::Online) => "BIR: online".into(),
        _ => "BIR: no recent news".into(),
    }
}

/// A mail ready to go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mail {
    pub subject: String,
    pub text: String,
    pub event_ids: Vec<i64>,
}

/// A mail on its way: its alerts, the addresses it reached, and when it is
/// tried next.
struct Outgoing {
    id: u64,
    mail: Mail,
    items: Vec<Alert>,
    /// The addresses it reached: never sent the same mail again.
    done: HashSet<String>,
    /// It reached an address: its alerts are told, their budgets spent.
    told: bool,
    /// Rounds that left an address unreached.
    failures: u32,
    first_at: OffsetDateTime,
    next_at: OffsetDateTime,
}

/// One round of a mail for [`flush`]: the addresses it has reached already
/// are skipped.
#[derive(Clone, Debug)]
pub struct Round {
    pub id: u64,
    pub mail: Mail,
    pub done: HashSet<String>,
}

#[derive(Default)]
struct Queue {
    /// Waiting to be put in a mail, oldest first.
    items: Vec<Alert>,
    /// When the last mail was made: the next waits 10 minutes.
    last_made: Option<OffsetDateTime>,
    day: Option<Date>,
    spent: HashMap<Budget, u32>,
    /// When each key was last mailed, and for how long that holds.
    told: HashMap<String, (OffsetDateTime, Duration)>,
    outbox: Vec<Outgoing>,
    next: u64,
}

impl Queue {
    fn new_day(&mut self, now: OffsetDateTime) {
        let today = manila_day(now);
        if self.day != Some(today) {
            self.day = Some(today);
            self.spent.clear();
        }
    }

    fn spent(&self, budget: Budget) -> u32 {
        self.spent.get(&budget).copied().unwrap_or(0)
    }

    /// The alerts in mails that reached nobody yet, by budget: spoken for.
    fn reserved(&self, budget: Budget) -> u32 {
        let count = self.outbox.iter().filter(|out| !out.told).flat_map(|out| &out.items);
        u32::try_from(count.filter(|item| item.budget == budget).count()).unwrap_or(u32::MAX)
    }

    /// Puts what is waiting in a new mail, once 10 minutes have passed since
    /// the last (at once if none was made in the last 10): everything its
    /// kind's budget still allows. An alert past its budget leaves unmailed
    /// and untold.
    fn make(&mut self, now: OffsetDateTime) {
        if self.items.is_empty() || self.last_made.is_some_and(|last| now >= last && now - last < BATCH_EVERY) {
            return;
        }
        let left: HashMap<Budget, u32> = Budget::ALL
            .into_iter()
            .map(|budget| (budget, budget.per_day().saturating_sub(self.spent(budget) + self.reserved(budget))))
            .collect();
        let mut using: HashMap<Budget, u32> = HashMap::new();
        let mut items = Vec::new();
        let mut over = 0;
        for item in std::mem::take(&mut self.items) {
            let used = using.entry(item.budget).or_default();
            if *used < left[&item.budget] {
                *used += 1;
                items.push(item);
            } else {
                over += 1;
            }
        }
        if over > 0 {
            tracing::warn!(over, "BIR uptime: staff alerts past today's budget are not mailed");
        }
        if items.is_empty() {
            return;
        }
        let last_of = Budget::ALL.into_iter().filter(|budget| {
            let used = using.get(budget).copied().unwrap_or(0);
            used > 0 && used == left[budget]
        });
        let subject = match items.len() {
            1 => items[0].subject.clone(),
            more => format!("{} (and {} more)", items[0].subject, more - 1),
        };
        let mut text = String::new();
        for item in &items {
            text.push_str(&format!("{}  {}\n", manila_time(item.at), item.line));
        }
        for budget in last_of {
            text.push_str(&format!(
                "\nThat is the last {} mailed today ({} a day); the event log keeps the rest.\n",
                budget.words(),
                budget.per_day()
            ));
        }
        text.push_str(
            "\nTimes are Manila time. Counts only: this mail names no TIN, person, account or address of anyone who filed.\n\nBuwiz - https://buwiz.com",
        );
        let mail = Mail { subject, text, event_ids: items.iter().filter_map(|item| item.event_id).collect() };
        if self.outbox.len() >= MAX_OUTBOX {
            let gone = self.outbox.remove(0);
            tracing::error!(
                subject = gone.mail.subject,
                "BIR uptime: too many staff mails on their way; the oldest is given up"
            );
        }
        self.outbox.push(Outgoing {
            id: self.next,
            mail,
            items,
            done: HashSet::new(),
            told: false,
            failures: 0,
            first_at: now,
            next_at: now,
        });
        self.next += 1;
        self.last_made = Some(now);
    }

    /// A round of `id` left an address unreached: tried again after 1, 2,
    /// 4 … minutes, at most an hour apart, and given up (logged) a day after
    /// its first try.
    fn retry(&mut self, id: u64, now: OffsetDateTime) {
        let Some(at) = self.outbox.iter().position(|out| out.id == id) else {
            return;
        };
        let out = &mut self.outbox[at];
        out.failures += 1;
        if now - out.first_at >= GIVE_UP_AFTER {
            let (subject, reached, told) = (out.mail.subject.clone(), out.done.len(), out.told);
            self.outbox.remove(at);
            tracing::error!(subject, reached, told, "BIR uptime: a staff mail is given up after a day of tries");
            return;
        }
        out.next_at = now + backoff(out.failures);
    }
}

/// The wait after the `failures`th round that left an address unreached.
pub fn backoff(failures: u32) -> Duration {
    let minutes = 1i64.checked_shl(failures.saturating_sub(1)).unwrap_or(i64::MAX).min(MAX_BACKOFF_MINUTES);
    Duration::minutes(minutes)
}

/// The alerts not mailed yet, and the mails on their way.
#[derive(Default)]
pub struct Alerts {
    queue: Mutex<Queue>,
}

impl Alerts {
    fn queue(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Queues `alert`, unless the same key is queued already, on its way, or
    /// was mailed within its `repeat_after`.
    pub fn push(&self, alert: Alert) {
        let mut queue = self.queue();
        if let Some((key, repeat_after)) = &alert.key {
            let told = queue.told.get(key).is_some_and(|(at, _)| alert.at >= *at && alert.at - *at < *repeat_after);
            let same = |item: &Alert| item.key.as_ref().is_some_and(|(other, _)| other == key);
            let queued = queue.items.iter().any(same)
                || queue.outbox.iter().filter(|out| !out.told).flat_map(|out| &out.items).any(same);
            if told || queued {
                return;
            }
        }
        if queue.items.len() >= MAX_QUEUED {
            tracing::error!("BIR uptime: too many staff alerts queued; one is not mailed");
            return;
        }
        queue.items.push(alert);
    }

    /// The rounds due at `now`: a new mail made of what waits (see
    /// [`Queue::make`]), and every mail on its way whose next try has come.
    pub fn due(&self, now: OffsetDateTime) -> Vec<Round> {
        let mut queue = self.queue();
        queue.new_day(now);
        queue.make(now);
        queue
            .outbox
            .iter()
            .filter(|out| out.next_at <= now)
            .map(|out| Round { id: out.id, mail: out.mail.clone(), done: out.done.clone() })
            .collect()
    }

    /// Mail `id` reached `address`. The first address it reaches makes its
    /// alerts told and spends their budgets: then the ids of the events to
    /// mark mailed.
    pub fn delivered(&self, id: u64, address: &str, now: OffsetDateTime) -> Option<Vec<i64>> {
        let mut queue = self.queue();
        queue.new_day(now);
        let out = queue.outbox.iter_mut().find(|out| out.id == id)?;
        out.done.insert(address.to_owned());
        if out.told {
            return None;
        }
        out.told = true;
        let items = out.items.clone();
        let event_ids = out.mail.event_ids.clone();
        for item in items {
            *queue.spent.entry(item.budget).or_default() += 1;
            if let Some((key, repeat_after)) = item.key {
                queue.told.insert(key, (now, repeat_after));
            }
        }
        // Bounded, never wiped: what can no longer hold anything back goes
        // first, then the oldest.
        if queue.told.len() > MAX_TOLD {
            queue.told.retain(|_, (at, repeat_after)| now < *at || now - *at < *repeat_after);
        }
        while queue.told.len() > MAX_TOLD {
            let Some(oldest) = queue.told.iter().min_by_key(|(_, (at, _))| *at).map(|(key, _)| key.clone()) else {
                break;
            };
            queue.told.remove(&oldest);
        }
        Some(event_ids)
    }

    /// A round of mail `id` to `to` ended: done once every address has it,
    /// else tried again later ([`Queue::retry`]).
    pub fn round_end(&self, id: u64, to: &[String], now: OffsetDateTime) {
        let mut queue = self.queue();
        let complete = queue
            .outbox
            .iter()
            .find(|out| out.id == id)
            .is_some_and(|out| !to.is_empty() && to.iter().all(|address| out.done.contains(address)));
        if complete {
            queue.outbox.retain(|out| out.id != id);
        } else {
            queue.retry(id, now);
        }
    }

    /// A round was cut short (it took too long): each mail it was trying
    /// counts as failed and waits its backoff; the addresses it reached are
    /// kept.
    pub fn timed_out(&self, now: OffsetDateTime) {
        let mut queue = self.queue();
        let trying: Vec<u64> = queue.outbox.iter().filter(|out| out.next_at <= now).map(|out| out.id).collect();
        for id in trying {
            queue.retry(id, now);
        }
    }

    /// How many alerts are queued or on their way and not yet told.
    pub fn queued(&self) -> usize {
        let queue = self.queue();
        queue.items.len() + queue.outbox.iter().filter(|out| !out.told).map(|out| out.items.len()).sum::<usize>()
    }
}

/// Mails what is due (the missing-header alert taken in first). Nothing at
/// `BIR_UPTIME=off`. Each address gets each mail once: one that cannot be
/// reached is logged and tried again on its own, later; the others are not
/// sent it again. The mail newly told in this round, if any.
pub async fn flush(
    uptime: &Uptime,
    db: &PgPool,
    mailer: &Mailer,
    config: &Config,
    now: OffsetDateTime,
) -> sqlx::Result<Option<Mail>> {
    if !uptime.level.records() {
        return Ok(None);
    }
    if let Some(at) = uptime.networks.take_missing_header_alert() {
        uptime.alerts.push(Alert::missing_header(at));
    }
    let rounds = uptime.alerts.due(now);
    if rounds.is_empty() {
        return Ok(None);
    }
    let to = async {
        let mut tx = live::begin(db).await?;
        let to = roles::staff_emails_on(&mut tx, &config.admin_emails).await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(to)
    };
    let to = match to.await {
        Ok(to) => to,
        Err(error) => {
            for round in &rounds {
                uptime.alerts.round_end(round.id, &[], now);
            }
            return Err(error);
        }
    };
    let mut told = None;
    for round in rounds {
        for address in to.iter().filter(|address| !round.done.contains(*address)) {
            match mailer.send_notice(address, &round.mail.subject, &round.mail.text).await {
                Ok(()) => {
                    let Some(event_ids) = uptime.alerts.delivered(round.id, address, now) else {
                        continue;
                    };
                    if !event_ids.is_empty()
                        && let Err(error) = mark_mailed(db, &event_ids, now).await
                    {
                        tracing::error!(%error, "BIR uptime: a mailed change could not be marked mailed");
                    }
                    told.get_or_insert_with(|| round.mail.clone());
                }
                Err(error) => {
                    tracing::error!(error = ?error, "BIR uptime: a staff mail could not be sent to one address")
                }
            }
        }
        if to.is_empty() {
            tracing::warn!("BIR uptime: no staff to mail; the mail is tried again");
        }
        uptime.alerts.round_end(round.id, &to, now);
    }
    Ok(told)
}

/// Marks `event_ids` mailed at `now`.
async fn mark_mailed(db: &PgPool, event_ids: &[i64], now: OffsetDateTime) -> sqlx::Result<()> {
    let mut tx = live::begin(db).await?;
    sqlx::query("UPDATE bir_events SET mailed_at = $2 WHERE id = ANY($1)")
        .bind(event_ids)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

#[cfg(test)]
mod tests {
    use time::format_description::well_known::Rfc3339;

    use super::*;

    fn at(minutes: i64) -> OffsetDateTime {
        OffsetDateTime::parse("2026-11-11T02:00:00Z", &Rfc3339).unwrap() + Duration::minutes(minutes)
    }

    fn change(minute: i64) -> Alert {
        Alert::sign(at(minute), "BIR: busy".into(), format!("Busy at {minute}."), None)
    }

    const STAFF: [&str; 3] = ["a@example.test", "b@example.test", "c@example.test"];

    /// One round at `now` as [`flush`] runs it, to the addresses in `to`,
    /// each reached unless in `failing`: the mail newly told, and every
    /// (address, subject) sent.
    fn round(alerts: &Alerts, now: OffsetDateTime, failing: &[&str]) -> (Option<Mail>, Vec<(String, String)>) {
        let to: Vec<String> = STAFF.iter().map(|address| (*address).to_owned()).collect();
        let mut told = None;
        let mut sent = Vec::new();
        for round in alerts.due(now) {
            for address in to.iter().filter(|address| !round.done.contains(*address)) {
                if failing.contains(&address.as_str()) {
                    continue;
                }
                sent.push((address.clone(), round.mail.subject.clone()));
                if alerts.delivered(round.id, address, now).is_some() {
                    told.get_or_insert_with(|| round.mail.clone());
                }
            }
            alerts.round_end(round.id, &to, now);
        }
        (told, sent)
    }

    #[test]
    fn times_are_manila_time_on_a_twelve_hour_clock() {
        assert_eq!(manila_time(at(0)), "10:00 AM");
        assert_eq!(manila_time(at(125)), "12:05 PM");
        assert_eq!(manila_time(at(14 * 60 + 7)), "12:07 AM");
        assert_eq!(manila_time(at(7 * 60 + 30)), "5:30 PM");
    }

    #[test]
    fn each_kind_spends_its_own_budget() {
        let alerts = Alerts::default();
        let mut minute = 0;
        let mut mails = Vec::new();
        for n in 0..10 {
            // A new host key, another alert and a change of the sign every
            // 10 minutes.
            alerts.push(Alert::host_key(&format!("SHA256:made-up-{n}"), at(minute)));
            alerts.push(Alert::missing_header(at(minute)));
            alerts.push(change(minute));
            mails.push(round(&alerts, at(minute), &[]).0.unwrap());
            minute += 10;
        }
        let all: String = mails.iter().map(|mail| mail.text.clone()).collect();
        assert_eq!(all.matches("SHA256:made-up-").count(), 3, "host keys: 3 a day, across fingerprints");
        assert_eq!(all.matches("without CF-Connecting-IP").count(), 8);
        assert_eq!(all.matches("Busy at").count(), 10, "the sign is always mailed");
        assert!(mails[2].text.contains("That is the last host-key alert mailed today (3 a day)"));
        assert_eq!(alerts.queued(), 0);
        // The changes of the sign have 48.
        for n in 0..60 {
            alerts.push(change(minute + n));
        }
        let mail = round(&alerts, at(minute + 60), &[]).0.unwrap();
        assert_eq!(mail.text.matches("Busy at").count(), 38);
        assert!(mail.text.contains("That is the last change of the sign mailed today (48 a day)"));
        // A new Manila day (midnight is 16:00 UTC) starts every budget again,
        // and an alert dropped yesterday is not held back today.
        let tomorrow = OffsetDateTime::parse("2026-11-11T16:05:00Z", &Rfc3339).unwrap();
        alerts.push(Alert::host_key("SHA256:made-up-9", tomorrow));
        alerts.push(Alert::host_key("SHA256:made-up-0", tomorrow));
        let mail = round(&alerts, tomorrow, &[]).0.unwrap();
        assert!(mail.text.contains("SHA256:made-up-9"), "dropped yesterday, mailed today");
        assert!(!mail.text.contains("SHA256:made-up-0"), "mailed within the day: told");
    }

    /// Five failures in a row and a landing in maintenance are no change of
    /// the sign: they spend the small "other" budget.
    #[test]
    fn notices_spend_the_other_budget() {
        let streak = Alert::notice(&Notice::FailuresInARow { failures: 5, since: None }, at(0));
        let landed = Alert::notice(&Notice::LandedInMaintenance, at(0));
        assert_eq!((streak.budget, landed.budget), (Budget::Other, Budget::Other));
    }

    #[test]
    fn an_alert_is_told_only_once_its_mail_went() {
        let alerts = Alerts::default();
        alerts.push(Alert::host_key("SHA256:made-up", at(0)));
        // It went nowhere: still queued, nothing spent, tried a minute later.
        let (told, sent) = round(&alerts, at(0), &STAFF);
        assert!(told.is_none() && sent.is_empty());
        assert_eq!(round(&alerts, at(0) + Duration::seconds(59), &[]).1.len(), 0);
        alerts.push(Alert::host_key("SHA256:made-up", at(1)));
        assert_eq!(alerts.queued(), 1, "the same key is queued once");
        assert_eq!(alerts.queue().spent(Budget::HostKey), 0);
        let (told, sent) = round(&alerts, at(1), &[]);
        assert!(told.is_some());
        assert_eq!(sent.len(), 3);
        alerts.push(Alert::host_key("SHA256:made-up", at(30)));
        assert_eq!(alerts.queued(), 0, "told: not again within the day");
        assert_eq!(alerts.queue().spent(Budget::HostKey), 1);
    }

    /// A slow or failing provider: the addresses a mail reached never get it
    /// again; the rest are tried after 1, 2, 4 … minutes, at most an hour
    /// apart, and the mail is given up a day after its first try.
    #[test]
    fn each_address_gets_a_mail_once_and_failures_back_off() {
        let alerts = Alerts::default();
        alerts.push(change(0));
        let (told, sent) = round(&alerts, at(0), &["c@example.test"]);
        assert!(told.is_some(), "it reached someone: told");
        assert_eq!(sent.len(), 2);
        let mut tries = Vec::new();
        let mut now = at(0);
        while now < at(0) + Duration::days(2) {
            now += Duration::minutes(1);
            let trying = alerts.queue().outbox.first().is_some_and(|out| out.next_at <= now);
            if trying {
                tries.push((now - at(0)).whole_minutes());
            }
            let (_, sent) = round(&alerts, now, &["c@example.test"]);
            assert!(sent.is_empty(), "nobody gets it twice; c keeps failing");
        }
        assert_eq!(&tries[..7], [1, 3, 7, 15, 31, 63, 123], "1, 2, 4, 8, 16, 32, then 60 minutes");
        assert!(alerts.queue().outbox.is_empty(), "given up after a day");
        assert_eq!(
            (backoff(1), backoff(2), backoff(7), backoff(40)),
            (Duration::minutes(1), Duration::minutes(2), Duration::minutes(60), Duration::minutes(60))
        );

        // Once c is back, it alone gets the mail.
        let alerts = Alerts::default();
        alerts.push(change(0));
        round(&alerts, at(0), &["c@example.test"]);
        let (told, sent) = round(&alerts, at(1), &[]);
        assert!(told.is_none(), "told already");
        assert_eq!(sent, [("c@example.test".to_owned(), "BIR: busy".to_owned())]);
        assert!(alerts.queue().outbox.is_empty());

        // A round cut short (it took too long) keeps whom it reached, and
        // counts as a failure.
        let alerts = Alerts::default();
        alerts.push(change(0));
        let round_now = alerts.due(at(0));
        alerts.delivered(round_now[0].id, "a@example.test", at(0));
        alerts.timed_out(at(0));
        assert!(alerts.due(at(0) + Duration::seconds(30)).is_empty(), "it waits its backoff");
        let again = alerts.due(at(1));
        assert_eq!(again.len(), 1);
        assert!(again[0].done.contains("a@example.test"));
    }

    #[test]
    fn the_told_keys_are_bounded_without_being_wiped() {
        let alerts = Alerts::default();
        // A key told that still holds back for 24 hours ...
        alerts.push(Alert::host_key("SHA256:kept", at(0)));
        round(&alerts, at(0), &[]);
        // ... then many that hold back an hour, all run out by now.
        {
            let mut queue = alerts.queue();
            for n in 0..(MAX_TOLD + 50) {
                queue.told.insert(format!("ours made-up-{n}"), (at(1), Duration::hours(1)));
            }
        }
        alerts.push(change(200));
        round(&alerts, at(200), &[]);
        let queue = alerts.queue();
        assert!(queue.told.len() <= MAX_TOLD);
        assert!(queue.told.contains_key("host_key SHA256:kept"), "what still holds back is kept");
    }

    #[test]
    fn mail_is_batched_every_ten_minutes() {
        let alerts = Alerts::default();
        alerts.push(change(0));
        round(&alerts, at(0), &[]);
        alerts.push(change(3));
        alerts.push(change(5));
        assert!(round(&alerts, at(9), &[]).0.is_none());
        let mail = round(&alerts, at(10), &[]).0.unwrap();
        assert_eq!(mail.subject, "BIR: busy (and 1 more)");
    }
}
