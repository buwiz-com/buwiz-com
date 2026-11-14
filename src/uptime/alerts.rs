//! Staff mail: Admins and Support hear of every change of the overall sign,
//! the relay pausing and resuming (and a reminder every 2 hours while it is
//! paused), a missing `CF-Connecting-IP` in production, and a fault on
//! Buwiz's side that needs a person (an upload-server host key that is not
//! pinned, an unknown upload server, an answer Buwiz could not read, a
//! Sender that cannot be reached).
//!
//! Batched: the first alert mails at once, the ones in the next 10 minutes
//! go together at the end of them; at most 24 mails a Manila day, the last
//! saying that more are not mailed. Plain short text with Manila times and
//! counts, never a TIN, a name, an address or an account of anyone who
//! filed. The queue is in memory: alerts not yet mailed at a restart are
//! lost (the event log keeps every change, and `bir_events.mailed_at`
//! says which went out).

use std::collections::HashMap;
use std::sync::Mutex;

use sqlx::PgPool;
use time::{Date, Duration, OffsetDateTime};

use super::Uptime;
use super::classify::Kind;
use super::keys::{MANILA, manila_day};
use super::live::Written;
use super::sign::{Cause, Light, Scope, Side, SignState};
use crate::account::mail::Mailer;
use crate::account::roles;
use crate::base::config::Config;

/// Alerts within this long of the last mail wait and go together.
pub const BATCH_EVERY: Duration = Duration::minutes(10);
/// The most mails a Manila day.
pub const PER_DAY: u32 = 24;
/// While the relay is paused, a reminder this often.
pub const PAUSED_REMINDER: Duration = Duration::hours(2);

/// One thing staff hear of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alert {
    pub at: OffsetDateTime,
    pub subject: String,
    pub line: String,
    /// The event it tells of, marked `mailed_at` once mailed.
    pub event_id: Option<i64>,
    /// Alerts with the same key within `repeat_after` of each other are
    /// told once.
    pub key: Option<(String, Duration)>,
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
    /// What one run of the rules wrote that staff hear of: a change of the
    /// overall sign (with the windows' reasons of the same run), and the
    /// relay pausing or resuming.
    pub fn for_run(written: &[Written], state: &SignState) -> Vec<Self> {
        let windows: Vec<String> = written
            .iter()
            .filter(|written| matches!(written.event.scope, Scope::Dispatcher | Scope::Upload))
            .map(|written| sentence(&written.event.reason))
            .collect();
        let mut alerts = Vec::new();
        for written in written {
            let event = &written.event;
            let subject = match event.scope {
                // The first line of the history is no change.
                Scope::Overall if event.from == "none" => continue,
                Scope::Overall => {
                    overall_subject(event.cause, Light::parse(&event.to), Light::parse(&event.from), state)
                }
                Scope::Relay if event.to == "paused" => "Buwiz: sending paused".to_owned(),
                Scope::Relay => "Buwiz: sending resumed".to_owned(),
                Scope::Dispatcher | Scope::Upload => continue,
            };
            let mut line = sentence(&event.reason);
            if event.scope == Scope::Overall {
                let word = Light::parse(&event.to).map_or("", word);
                line = format!("{word}: {line}");
                for reason in &windows {
                    line.push(' ');
                    line.push_str(reason);
                }
            }
            if event.scope == Scope::Relay && event.to == "paused" {
                line.push_str(
                    " Nothing is held yet; an Admin resumes sending once the fault is fixed (a key is pinned only from a second source the owner trusts, never from this mail).",
                );
            }
            alerts.push(Alert { at: written.at, subject, line, event_id: Some(written.id), key: None });
        }
        alerts
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
        }
    }

    /// The relay is still paused.
    pub fn still_paused(since: OffsetDateTime, at: OffsetDateTime) -> Self {
        Self {
            at,
            subject: "Buwiz: sending is still paused".into(),
            line: format!("Sending has been paused since {}. An Admin resumes it.", manila_time(since)),
            event_id: None,
            key: None,
        }
    }
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

#[derive(Default)]
struct Queue {
    items: Vec<Alert>,
    last_sent: Option<OffsetDateTime>,
    day: Option<Date>,
    sent_today: u32,
    told: HashMap<String, OffsetDateTime>,
    reminded: Option<OffsetDateTime>,
}

/// The alerts not mailed yet.
#[derive(Default)]
pub struct Alerts {
    queue: Mutex<Queue>,
}

impl Alerts {
    pub fn push(&self, alert: Alert) {
        let mut queue = self.queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((key, repeat_after)) = &alert.key {
            if queue.told.get(key).is_some_and(|told| alert.at >= *told && alert.at - *told < *repeat_after) {
                return;
            }
            if queue.told.len() > 1_000 {
                queue.told.clear();
            }
            queue.told.insert(key.clone(), alert.at);
        }
        if queue.items.len() < 200 {
            queue.items.push(alert);
        }
    }

    /// A reminder while the relay is paused: one every 2 hours from when it
    /// paused.
    fn remind(&self, state: &SignState, now: OffsetDateTime) {
        let since = match (state.relay.paused, state.relay.since) {
            (true, Some(since)) => since,
            _ => {
                self.queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).reminded = None;
                return;
            }
        };
        let due = {
            let mut queue = self.queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let last = queue.reminded.unwrap_or(since).max(since);
            let due = now - last >= PAUSED_REMINDER;
            if due {
                queue.reminded = Some(now);
            }
            due
        };
        if due {
            self.push(Alert::still_paused(since, now));
        }
    }

    /// The mail due at `now`, if any: everything queued, once 10 minutes
    /// have passed since the last mail (at once if none went out in the
    /// last 10). After the day's 24th nothing more is mailed that day.
    pub fn take(&self, now: OffsetDateTime) -> Option<Mail> {
        let mut queue = self.queue.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let today = manila_day(now);
        if queue.day != Some(today) {
            queue.day = Some(today);
            queue.sent_today = 0;
        }
        if queue.items.is_empty() {
            return None;
        }
        if queue.sent_today >= PER_DAY {
            queue.items.clear();
            return None;
        }
        if queue.last_sent.is_some_and(|last| now >= last && now - last < BATCH_EVERY) {
            return None;
        }
        let items = std::mem::take(&mut queue.items);
        queue.sent_today += 1;
        queue.last_sent = Some(now);
        let last_today = queue.sent_today == PER_DAY;
        drop(queue);

        let subject = match items.len() {
            1 => items[0].subject.clone(),
            more => format!("{} (and {} more)", items[0].subject, more - 1),
        };
        let mut text = String::new();
        for item in &items {
            text.push_str(&format!("{}  {}\n", manila_time(item.at), item.line));
        }
        if last_today {
            text.push_str(
                "\nThis is the 24th mail today. More changes today are not mailed; the event log keeps them.\n",
            );
        }
        text.push_str(
            "\nTimes are Manila time. Counts only: this mail names no TIN, person, account or address of anyone who filed.\n\nBuwiz - https://buwiz.com",
        );
        Some(Mail { subject, text, event_ids: items.iter().filter_map(|item| item.event_id).collect() })
    }
}

/// Mails what is due: the missing-header alert and the paused reminder are
/// taken in first. Nothing at `BIR_UPTIME=off`. A mail that cannot be sent
/// to one address is logged; the others still get it.
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
    uptime.alerts.remind(&uptime.live.sign(), now);
    let Some(mail) = uptime.alerts.take(now) else {
        return Ok(None);
    };
    let to = roles::staff_emails(db, &config.admin_emails).await?;
    if to.is_empty() {
        tracing::warn!("BIR uptime: no Admin or Support to mail");
    }
    for address in &to {
        if let Err(error) = mailer.send_notice(address, &mail.subject, &mail.text).await {
            tracing::error!(error = ?error, "BIR uptime: a staff mail could not be sent");
        }
    }
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

    #[test]
    fn times_are_manila_time_on_a_twelve_hour_clock() {
        assert_eq!(manila_time(at(0)), "10:00 AM");
        assert_eq!(manila_time(at(125)), "12:05 PM");
        assert_eq!(manila_time(at(14 * 60 + 7)), "12:07 AM");
        assert_eq!(manila_time(at(7 * 60 + 30)), "5:30 PM");
    }

    #[test]
    fn a_paused_relay_is_recalled_every_two_hours() {
        let alerts = Alerts::default();
        let mut state = SignState::new(at(0));
        alerts.remind(&state, at(0));
        assert_eq!(alerts.take(at(1)), None);
        state.relay.paused = true;
        state.relay.since = Some(at(0));
        for minute in [1, 60, 119] {
            alerts.remind(&state, at(minute));
        }
        assert_eq!(alerts.take(at(119)), None);
        alerts.remind(&state, at(120));
        alerts.remind(&state, at(121));
        let mail = alerts.take(at(121)).unwrap();
        assert_eq!(mail.subject, "Buwiz: sending is still paused");
        assert!(mail.text.contains("paused since 10:00 AM"));
        alerts.remind(&state, at(239));
        assert_eq!(alerts.take(at(239)), None);
        alerts.remind(&state, at(240));
        assert!(alerts.take(at(240)).is_some());
        // Resumed: no more.
        state.relay.paused = false;
        alerts.remind(&state, at(400));
        assert_eq!(alerts.take(at(400)), None);
    }
}
