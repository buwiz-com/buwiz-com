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

use std::collections::HashMap;
use std::sync::Mutex;

use sqlx::PgPool;
use time::{Date, Duration, OffsetDateTime};

use super::Uptime;
use super::classify::Kind;
use super::keys::{MANILA, manila_day};
use super::live::Written;
use super::sign::{Cause, Light, NoteKind, Notice, Scope, Side, SignState};
use crate::account::mail::Mailer;
use crate::account::roles;
use crate::base::config::Config;

/// Alerts within this long of the last mail wait and go together.
pub const BATCH_EVERY: Duration = Duration::minutes(10);
/// A mail that went nowhere is tried again after this long.
pub const RETRY_AFTER: Duration = Duration::minutes(1);
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

    /// What one run of the rules wrote that staff hear of: a change of the
    /// overall sign (with the windows' reasons of the same run), a window
    /// going Offline or coming back when the overall sign did not change,
    /// a note's line, the stored sign reset, and the relay resuming.
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
                Scope::Overall => overall_subject(event.cause, to, from, state),
                Scope::Relay if event.to == "paused" => "Buwiz: sending paused".to_owned(),
                Scope::Relay => "Buwiz: sending resumed".to_owned(),
                Scope::Dispatcher | Scope::Upload => {
                    let leaves = from == Some(Light::Offline) && to != Some(Light::Offline);
                    if overall_changed || !(to == Some(Light::Offline) || leaves) {
                        continue;
                    }
                    alerts.push(window_alert(written, leaves, state));
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
            Notice::FailuresInARow { failures, since } => Alert::sign(
                at,
                "BIR: tries keep failing".to_owned(),
                match since {
                    Some(since) => format!(
                        "{failures} tries in a row from {failures} accounts did not get in; the last return got in at {}.",
                        manila_time(*since)
                    ),
                    None => format!("{failures} tries in a row from {failures} accounts did not get in."),
                },
                None,
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
            Notice::LandedInMaintenance => Alert::sign(
                at,
                "BIR: a return got in during maintenance".to_owned(),
                "A return got in while the planned maintenance note is in force. The note stays until it ends or an Admin clears it.".to_owned(),
                None,
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

/// A window went Offline or came back while the overall sign did not move
/// (a note decides it, or the other window already did).
fn window_alert(written: &Written, leaves: bool, state: &SignState) -> Alert {
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
    let subject = if leaves { format!("BIR: {} back", side.word()) } else { format!("BIR: {} offline", side.word()) };
    Alert::sign(written.at, subject, line, Some(written.id))
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
    /// The queued alerts it carries.
    numbers: Vec<u64>,
}

#[derive(Default)]
struct Queue {
    /// Oldest first, each with its number.
    items: Vec<(u64, Alert)>,
    next: u64,
    last_sent: Option<OffsetDateTime>,
    retry_at: Option<OffsetDateTime>,
    day: Option<Date>,
    spent: HashMap<Budget, u32>,
    /// When each key was last mailed, and for how long that holds.
    told: HashMap<String, (OffsetDateTime, Duration)>,
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
}

/// The alerts not mailed yet.
#[derive(Default)]
pub struct Alerts {
    queue: Mutex<Queue>,
}

impl Alerts {
    fn queue(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Queues `alert`, unless the same key is queued already or was mailed
    /// within its `repeat_after`.
    pub fn push(&self, alert: Alert) {
        let mut queue = self.queue();
        if let Some((key, repeat_after)) = &alert.key {
            let told = queue.told.get(key).is_some_and(|(at, _)| alert.at >= *at && alert.at - *at < *repeat_after);
            let queued = queue.items.iter().any(|(_, item)| item.key.as_ref().is_some_and(|(other, _)| other == key));
            if told || queued {
                return;
            }
        }
        if queue.items.len() >= MAX_QUEUED {
            tracing::error!("BIR uptime: too many staff alerts queued; one is not mailed");
            return;
        }
        let number = queue.next;
        queue.next += 1;
        queue.items.push((number, alert));
    }

    /// The mail due at `now`, if any: everything queued that its kind's
    /// budget still allows, once 10 minutes have passed since the last mail
    /// (at once if none went out in the last 10). An alert past its budget
    /// leaves the queue unmailed and untold. The mail stays queued until
    /// [`Alerts::sent`] says it went.
    pub fn take(&self, now: OffsetDateTime) -> Option<Mail> {
        let mut queue = self.queue();
        queue.new_day(now);
        if queue.last_sent.is_some_and(|last| now >= last && now - last < BATCH_EVERY)
            || queue.retry_at.is_some_and(|at| now < at)
        {
            return None;
        }
        let mut using: HashMap<Budget, u32> = HashMap::new();
        let spent: HashMap<Budget, u32> = Budget::ALL.into_iter().map(|budget| (budget, queue.spent(budget))).collect();
        let before = queue.items.len();
        queue.items.retain(|(_, item)| {
            let used = using.entry(item.budget).or_default();
            let fits = spent[&item.budget] + *used < item.budget.per_day();
            *used += u32::from(fits);
            fits
        });
        let over = before - queue.items.len();
        if over > 0 {
            tracing::warn!(over, "BIR uptime: staff alerts past today's budget are not mailed");
        }
        if queue.items.is_empty() {
            return None;
        }
        let items: Vec<(u64, Alert)> = queue.items.clone();
        drop(queue);
        let last_of = Budget::ALL.into_iter().filter(|budget| {
            let used = using.get(budget).copied().unwrap_or(0);
            used > 0 && spent[budget] + used == budget.per_day()
        });

        let subject = match items.len() {
            1 => items[0].1.subject.clone(),
            more => format!("{} (and {} more)", items[0].1.subject, more - 1),
        };
        let mut text = String::new();
        for (_, item) in &items {
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
        Some(Mail {
            subject,
            text,
            event_ids: items.iter().filter_map(|(_, item)| item.event_id).collect(),
            numbers: items.iter().map(|(number, _)| *number).collect(),
        })
    }

    /// `mail` went out: its alerts leave the queue, spend their budgets and
    /// are told.
    pub fn sent(&self, mail: &Mail, now: OffsetDateTime) {
        let mut queue = self.queue();
        queue.new_day(now);
        let mut gone = Vec::new();
        queue.items.retain(|(number, item)| {
            let carried = mail.numbers.contains(number);
            if carried {
                gone.push(item.clone());
            }
            !carried
        });
        for item in gone {
            *queue.spent.entry(item.budget).or_default() += 1;
            if let Some((key, repeat_after)) = item.key {
                queue.told.insert(key, (now, repeat_after));
            }
        }
        queue.last_sent = Some(now);
        queue.retry_at = None;
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
    }

    /// The mail went nowhere: it stays queued, spends nothing, and is tried
    /// again a minute later.
    pub fn failed(&self, now: OffsetDateTime) {
        self.queue().retry_at = Some(now + RETRY_AFTER);
    }

    /// How many alerts are queued.
    pub fn queued(&self) -> usize {
        self.queue().items.len()
    }
}

/// Mails what is due (the missing-header alert taken in first). Nothing at
/// `BIR_UPTIME=off`. A mail that cannot be sent to one address is logged;
/// the others still get it. One that reached no address stays queued.
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
    let Some(mail) = uptime.alerts.take(now) else {
        return Ok(None);
    };
    let to = match roles::staff_emails(db, &config.admin_emails).await {
        Ok(to) => to,
        Err(error) => {
            uptime.alerts.failed(now);
            return Err(error);
        }
    };
    let mut delivered = 0;
    for address in &to {
        match mailer.send_notice(address, &mail.subject, &mail.text).await {
            Ok(()) => delivered += 1,
            Err(error) => tracing::error!(error = ?error, "BIR uptime: a staff mail could not be sent"),
        }
    }
    if delivered == 0 {
        tracing::warn!(staff = to.len(), "BIR uptime: a staff mail reached nobody; it is tried again");
        uptime.alerts.failed(now);
        return Ok(None);
    }
    uptime.alerts.sent(&mail, now);
    if !mail.event_ids.is_empty() {
        sqlx::query("UPDATE bir_events SET mailed_at = $2 WHERE id = ANY($1)")
            .bind(&mail.event_ids)
            .bind(now)
            .execute(db)
            .await?;
    }
    Ok(Some(mail))
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
            let mail = alerts.take(at(minute)).unwrap();
            alerts.sent(&mail, at(minute));
            mails.push(mail);
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
        let mail = alerts.take(at(minute + 60)).unwrap();
        assert_eq!(mail.text.matches("Busy at").count(), 38);
        assert!(mail.text.contains("That is the last change of the sign mailed today (48 a day)"));
        alerts.sent(&mail, at(minute + 60));
        // A new Manila day (midnight is 16:00 UTC) starts every budget again,
        // and an alert dropped yesterday is not held back today.
        let tomorrow = OffsetDateTime::parse("2026-11-11T16:05:00Z", &Rfc3339).unwrap();
        alerts.push(Alert::host_key("SHA256:made-up-9", tomorrow));
        alerts.push(Alert::host_key("SHA256:made-up-0", tomorrow));
        let mail = alerts.take(tomorrow).unwrap();
        assert!(mail.text.contains("SHA256:made-up-9"), "dropped yesterday, mailed today");
        assert!(!mail.text.contains("SHA256:made-up-0"), "mailed within the day: told");
    }

    #[test]
    fn an_alert_is_told_only_once_its_mail_went() {
        let alerts = Alerts::default();
        alerts.push(Alert::host_key("SHA256:made-up", at(0)));
        let mail = alerts.take(at(0)).unwrap();
        // It went nowhere: still queued, nothing spent, tried a minute later.
        alerts.failed(at(0));
        assert_eq!(alerts.take(at(0) + Duration::seconds(59)), None);
        alerts.push(Alert::host_key("SHA256:made-up", at(1)));
        assert_eq!(alerts.queued(), 1, "the same key is queued once");
        let again = alerts.take(at(1)).unwrap();
        assert_eq!(again.text, mail.text);
        assert_eq!(alerts.queue().spent(Budget::HostKey), 0);
        alerts.sent(&again, at(1));
        alerts.push(Alert::host_key("SHA256:made-up", at(30)));
        assert_eq!(alerts.queued(), 0, "told: not again within the day");
        assert_eq!(alerts.queue().spent(Budget::HostKey), 1);
    }

    #[test]
    fn the_told_keys_are_bounded_without_being_wiped() {
        let alerts = Alerts::default();
        // A key told that still holds back for 24 hours ...
        alerts.push(Alert::host_key("SHA256:kept", at(0)));
        let mail = alerts.take(at(0)).unwrap();
        alerts.sent(&mail, at(0));
        // ... then many that hold back an hour, all run out by now.
        {
            let mut queue = alerts.queue();
            for n in 0..(MAX_TOLD + 50) {
                queue.told.insert(format!("ours made-up-{n}"), (at(1), Duration::hours(1)));
            }
        }
        alerts.push(change(200));
        let mail = alerts.take(at(200)).unwrap();
        alerts.sent(&mail, at(200));
        let queue = alerts.queue();
        assert!(queue.told.len() <= MAX_TOLD);
        assert!(queue.told.contains_key("host_key SHA256:kept"), "what still holds back is kept");
    }

    #[test]
    fn mail_is_batched_every_ten_minutes() {
        let alerts = Alerts::default();
        alerts.push(change(0));
        let mail = alerts.take(at(0)).unwrap();
        alerts.sent(&mail, at(0));
        alerts.push(change(3));
        alerts.push(change(5));
        assert_eq!(alerts.take(at(9)), None);
        let mail = alerts.take(at(10)).unwrap();
        assert_eq!(mail.subject, "BIR: busy (and 1 more)");
    }
}
