//! What apps and people read of the sign (docs/bir-uptime.md §2.2, §2.3):
//! the sign object, its words, the compact `buwiz-bir` header and the My
//! TINs banner. Pure: built from the sign in memory.
//!
//! The public document is the same for everyone, and made from the sign as
//! it stood at the last 5-minute mark ([`super::live::Live::public`]), so
//! nobody polling it can time a single try. Its times (`since`,
//! `last_success_at`, `evaluated_at`, a note's `set_at` and `until`) are on
//! 5-minute marks too (past ones rounded down, a note's end up), and it has
//! no `last_news_at`. It carries no count, account, TIN, host name or
//! address, and its `ETag` is an opaque hash of its bytes. Only what is
//! built is sent: the scout time, the waits, admission and the queue
//! numbers are planned and left out, not sent as nulls an app might act on.

use serde::Serialize;
use time::{Duration, OffsetDateTime, UtcOffset};

use super::alerts::manila_time;
use super::keys::{MANILA, manila_day};
use super::sign::{Lamp, Light, NoteKind, Side, SignState, failing};
use super::store::to_minute;

/// The contract's version.
pub const VERSION: u32 = 1;

/// Past times in public answers are rounded down to this.
pub const ROUND: Duration = Duration::minutes(5);

/// The header the compact sign rides in.
pub const HEADER: &str = "buwiz-bir";

/// Where the status page is, under the filing site's origin.
pub const STATUS_PATH: &str = "/bir-status";

/// `at` rounded down to 5 minutes, in UTC.
pub fn round_down(at: OffsetDateTime) -> OffsetDateTime {
    let at = at.to_offset(UtcOffset::UTC);
    let seconds = at.unix_timestamp();
    OffsetDateTime::from_unix_timestamp(seconds - seconds.rem_euclid(ROUND.whole_seconds())).unwrap_or(at)
}

/// `at` rounded up to 5 minutes: a time to wait for, for reading.
pub fn round_up(at: OffsetDateTime) -> OffsetDateTime {
    let down = round_down(at);
    if down == at.to_offset(UtcOffset::UTC) { down } else { down + ROUND }
}

/// How times are given: on 5-minute marks for everyone, or to the minute
/// (with `last_news_at`) for staff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Times {
    Public,
    Exact,
}

impl Times {
    fn past(self, at: OffsetDateTime) -> OffsetDateTime {
        match self {
            Self::Public => round_down(at),
            Self::Exact => to_minute(at).to_offset(UtcOffset::UTC),
        }
    }

    fn future(self, at: OffsetDateTime) -> OffsetDateTime {
        match self {
            Self::Public => round_up(at),
            Self::Exact => at.to_offset(UtcOffset::UTC),
        }
    }
}

/// `Nov 11, 3:10 PM`, in Manila.
pub fn manila_when(at: OffsetDateTime) -> String {
    let local = at.to_offset(MANILA);
    format!("{} {}, {}", short_month(local.month()), local.day(), manila_time(at))
}

fn short_month(month: time::Month) -> &'static str {
    use time::Month::*;
    match month {
        January => "Jan",
        February => "Feb",
        March => "Mar",
        April => "Apr",
        May => "May",
        June => "Jun",
        July => "Jul",
        August => "Aug",
        September => "Sep",
        October => "Oct",
        November => "Nov",
        December => "Dec",
    }
}

/// The word people read for a state.
pub fn state_word(light: Light) -> &'static str {
    match light {
        Light::Online => "Online",
        Light::Busy => "Busy",
        Light::Offline => "Offline",
        Light::NoRecentNews => "No recent news",
    }
}

/// The server's sentence for the sign: true for every app version, in
/// Manila time, from the times as `times` gives them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Words {
    pub title: String,
    pub line: String,
}

/// The windows that are in `light`, by the words people read.
fn windows_in(sign: &SignState, light: Light) -> String {
    let sides: Vec<&str> =
        Side::BOTH.into_iter().filter(|side| sign.lamp(*side).state == light).map(Side::word).collect();
    match sides.as_slice() {
        [one] => format!("The BIR's {one} is"),
        [_, _] => "The BIR's front desk and mailbox are".to_owned(),
        _ => "The BIR is".to_owned(),
    }
}

pub fn words(sign: &SignState, times: Times) -> Words {
    let since = manila_when(times.past(sign.since));
    let got_in = sign
        .last_success_at
        .map(|at| format!(" The last return got in at about {}.", manila_when(times.past(at))))
        .unwrap_or_default();
    let until = sign.note.as_ref().map(|note| manila_when(round_up(note.until))).unwrap_or_default();
    let (title, line) = match (sign.state, sign.note.as_ref().map(|note| note.kind)) {
        (_, Some(NoteKind::Maintenance)) => (
            "BIR maintenance",
            format!("The BIR has planned maintenance until about {until}. Returns can't get in until it ends."),
        ),
        (_, Some(NoteKind::Outage)) => (
            "The BIR is offline",
            format!("Buwiz staff marked the BIR offline until {until}. Returns can't get in until it's back."),
        ),
        (_, Some(NoteKind::Online)) => {
            ("The BIR is online", format!("Buwiz staff marked the BIR online until {until}.{got_in}"))
        }
        (Light::Online, None) => ("The BIR is online", format!("Returns are getting in.{got_in}")),
        (Light::Busy, None) => (
            "The BIR is busy",
            format!(
                "{} busy since {since}: some returns get in and some don't. A return may need a few tries.",
                windows_in(sign, Light::Busy)
            ),
        ),
        (Light::Offline, None) => (
            "The BIR is offline",
            format!(
                "{} offline since {since}. Returns can't get in until it's back.",
                windows_in(sign, Light::Offline)
            ),
        ),
        (Light::NoRecentNews, None) if sign.last_state == Some(Light::Offline) => (
            "No recent news",
            format!(
                "Nobody tried for 3 hours since the BIR went offline, so Buwiz can't say whether it's back.{got_in}"
            ),
        ),
        (Light::NoRecentNews, None) if failing(sign, sign.evaluated_at) => (
            "No recent news",
            format!(
                "No return has got in during the last hour: some tries failed, but too few people to call it Offline.{got_in}"
            ),
        ),
        (Light::NoRecentNews, None) => {
            ("No recent news", format!("No return has got in during the last hour, so Buwiz can't say.{got_in}"))
        }
    };
    Words { title: title.to_owned(), line }
}

/// One window's light.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WindowDoc {
    pub state: &'static str,
    #[serde(with = "time::serde::rfc3339")]
    pub since: OffsetDateTime,
    /// The last relay return that got past it.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_ok_at: Option<OffsetDateTime>,
}

/// A hand-set note in force. Its text is public.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NoteDoc {
    pub kind: &'static str,
    pub text: String,
    #[serde(with = "time::serde::rfc3339")]
    pub set_at: OffsetDateTime,
    /// A future time: rounded up to 5 minutes in the public document.
    #[serde(with = "time::serde::rfc3339")]
    pub until: OffsetDateTime,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Windows {
    pub dispatcher: WindowDoc,
    pub upload: WindowDoc,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Links {
    pub status_page: String,
}

/// The sign object (docs §2.2), with the fields built so far.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Document {
    pub v: u32,
    pub seq: i64,
    pub state: &'static str,
    #[serde(with = "time::serde::rfc3339")]
    pub since: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub evaluated_at: OffsetDateTime,
    /// `tries`, or `staff` while a hand-set note is in force.
    pub source: &'static str,
    /// `ok`; `paused` is planned (nothing pauses the relay yet).
    pub relay: &'static str,
    pub windows: Windows,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_success_at: Option<OffsetDateTime>,
    /// The last counted try of any outcome: for staff only, never public
    /// (it would time a quiet day's single try).
    #[serde(with = "time::serde::rfc3339::option", skip_serializing_if = "Option::is_none")]
    pub last_news_at: Option<OffsetDateTime>,
    pub last_state: Option<&'static str>,
    pub note: Option<NoteDoc>,
    pub words: Words,
    pub links: Links,
}

fn window(lamp: &Lamp, times: Times) -> WindowDoc {
    WindowDoc {
        state: lamp.state.as_str(),
        since: times.past(lamp.since),
        last_ok_at: lamp.last_ok_at.map(|at| times.past(at)),
    }
}

/// The sign object, its past times as `times` says. `forms_origin` is the
/// filing site's, where the status page is.
pub fn document(sign: &SignState, forms_origin: &str, times: Times) -> Document {
    Document {
        v: VERSION,
        seq: sign.seq,
        state: sign.state.as_str(),
        since: times.past(sign.since),
        evaluated_at: times.past(sign.evaluated_at),
        source: sign.source(),
        relay: if sign.relay.paused { "paused" } else { "ok" },
        windows: Windows { dispatcher: window(&sign.dispatcher, times), upload: window(&sign.upload, times) },
        last_success_at: sign.last_success_at.map(|at| times.past(at)),
        last_news_at: sign.last_news_at.filter(|_| times == Times::Exact).map(|at| times.past(at)),
        last_state: sign.last_state.filter(|_| sign.state == Light::NoRecentNews).map(Light::as_str),
        note: sign.note.as_ref().map(|note| NoteDoc {
            kind: note.kind.as_str(),
            text: note.text.clone(),
            set_at: times.past(note.starts_at),
            until: times.future(note.until),
        }),
        words: words(sign, times),
        links: Links { status_page: format!("{forms_origin}{STATUS_PATH}") },
    }
}

/// An `ETag` for `body`: an opaque hash of its bytes, so it tells nothing
/// the body does not.
pub fn etag(body: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(body);
    format!("\"{}\"", crate::base::crypto::hex(&digest[..12]))
}

/// What everything public is made from, for one 5-minute mark: the sign as
/// it stood at the mark, the public document's bytes and `ETag`, and the
/// compact header.
#[derive(Debug)]
pub struct Published {
    pub mark: OffsetDateTime,
    pub sign: std::sync::Arc<SignState>,
    pub body: Vec<u8>,
    pub etag: String,
    pub compact: String,
}

impl Published {
    pub fn of(mark: OffsetDateTime, sign: std::sync::Arc<SignState>, forms_origin: &str) -> Self {
        let body = serde_json::to_vec(&document(&sign, forms_origin, Times::Public)).unwrap_or_default();
        Self { mark, etag: etag(&body), compact: compact(&sign), body, sign }
    }
}

/// The compact sign (docs §2.3), an RFC 8941 dictionary: `v`, `seq`, the
/// overall state and both lights, `relay`, `since` (Unix seconds, rounded
/// down to 5 minutes) and `note` (a boolean). `until`, `scout` and `admit`
/// are planned and left out (absent members are null).
pub fn compact(sign: &SignState) -> String {
    format!(
        "v={VERSION}, seq={}, state={}, dispatcher={}, upload={}, relay={}, since={}, note=?{}",
        sign.seq,
        sign.state.as_str(),
        sign.dispatcher.state.as_str(),
        sign.upload.state.as_str(),
        if sign.relay.paused { "paused" } else { "ok" },
        round_down(sign.since).unix_timestamp(),
        u8::from(sign.note.is_some()),
    )
}

/// How the My TINs banner looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Busy,
    Offline,
    Maintenance,
}

impl Tone {
    pub fn class(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::Offline => "offline",
            Self::Maintenance => "maintenance",
        }
    }

    /// Its Material Symbols icon: a shape, so the colour is never alone.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Busy => "hourglass_top",
            Self::Offline => "cloud_off",
            Self::Maintenance => "construction",
        }
    }
}

/// The line at the top of My TINs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Banner {
    pub tone: Tone,
    pub title: String,
    pub line: String,
    /// The note's public text, when staff set it by hand.
    pub note: Option<String>,
}

/// The My TINs banner: for Busy, Offline and a hand-set outage or
/// maintenance; none for Online (a "declare Online" note too) and No recent
/// news. What it says holds for every app version: a return sent now may
/// fail and nothing is held.
pub fn banner(sign: &SignState) -> Option<Banner> {
    let words = words(sign, Times::Public);
    let note = sign.note.as_ref().filter(|note| note.kind.holds()).map(|note| note.text.clone());
    let (tone, what) = match (sign.state, sign.note.as_ref().map(|note| note.kind)) {
        (_, Some(NoteKind::Maintenance)) => {
            (Tone::Maintenance, "Queued returns wait; check your app's send queue before the due date.")
        }
        (_, Some(NoteKind::Online)) => return None,
        (Light::Offline, _) => (Tone::Offline, "Queued returns wait; check your app's send queue before the due date."),
        (Light::Busy, _) => (Tone::Busy, "Queued returns keep trying; leave a little time before the due date."),
        (Light::Online | Light::NoRecentNews, _) => return None,
    };
    Some(Banner { tone, title: words.title, line: format!("{} {what}", words.line), note })
}

/// Today in Manila, for pages that show "today".
pub fn today(now: OffsetDateTime) -> time::Date {
    manila_day(now)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use time::macros::datetime;
    use uuid::Uuid;

    use super::*;
    use crate::uptime::sign::Note;

    fn sign(state: Light) -> SignState {
        let now = datetime!(2026-11-11 09:03:41 UTC);
        let mut sign = SignState::new(now);
        sign.seq = 4182;
        sign.revision = 1_762_851_821_000;
        sign.state = state;
        sign.since = datetime!(2026-11-11 07:13:20 UTC);
        sign.upload.state = state;
        sign.upload.since = datetime!(2026-11-11 07:13:20 UTC);
        sign.dispatcher.state = Light::Online;
        sign.dispatcher.since = datetime!(2026-11-11 01:02:00 UTC);
        sign.last_success_at = Some(datetime!(2026-11-11 07:04:59 UTC));
        sign.last_news_at = Some(datetime!(2026-11-11 08:58:00 UTC));
        sign
    }

    #[test]
    fn past_times_are_rounded_down_to_5_minutes_and_future_ones_kept() {
        assert_eq!(round_down(datetime!(2026-11-11 07:14:59 UTC)), datetime!(2026-11-11 07:10 UTC));
        assert_eq!(round_down(datetime!(2026-11-11 07:15 UTC)), datetime!(2026-11-11 07:15 UTC));
        assert_eq!(round_down(datetime!(2026-11-11 15:14:59 +08:00)), datetime!(2026-11-11 07:10 UTC));
        assert_eq!(round_up(datetime!(2026-11-11 07:10:01 UTC)), datetime!(2026-11-11 07:15 UTC));
        assert_eq!(round_up(datetime!(2026-11-11 07:10 UTC)), datetime!(2026-11-11 07:10 UTC));
        let mut offline = sign(Light::Offline);
        let until = datetime!(2026-11-11 10:13:41 UTC);
        offline.note = Some(Note {
            id: Uuid::nil(),
            kind: NoteKind::Outage,
            text: "Made-up outage".into(),
            starts_at: datetime!(2026-11-11 07:13:20 UTC),
            until,
        });
        let public = document(&offline, "https://ebirforms.buwiz.com", Times::Public);
        assert_eq!(public.since, datetime!(2026-11-11 07:10 UTC));
        assert_eq!(public.evaluated_at, datetime!(2026-11-11 09:00 UTC));
        assert_eq!(public.last_success_at, Some(datetime!(2026-11-11 07:00 UTC)));
        assert_eq!(public.last_news_at, None, "never public");
        assert!(serde_json::to_value(&public).unwrap().get("last_news_at").is_none());
        assert_eq!(public.windows.dispatcher.since, datetime!(2026-11-11 01:00 UTC));
        let note = public.note.unwrap();
        assert_eq!((note.set_at, note.until), (datetime!(2026-11-11 07:10 UTC), datetime!(2026-11-11 10:15 UTC)));
        let exact = document(&offline, "https://ebirforms.buwiz.com", Times::Exact);
        assert_eq!(exact.since, datetime!(2026-11-11 07:13 UTC));
        assert_eq!(exact.last_success_at, Some(datetime!(2026-11-11 07:04 UTC)));
        assert_eq!(exact.last_news_at, Some(datetime!(2026-11-11 08:58 UTC)));
        assert_eq!(exact.note.unwrap().until, until);
    }

    #[test]
    fn words_say_each_state_plainly_in_manila_time() {
        let online = words(&sign(Light::Online), Times::Public);
        assert_eq!(online.title, "The BIR is online");
        assert_eq!(online.line, "Returns are getting in. The last return got in at about Nov 11, 3:00 PM.");
        let busy = words(&sign(Light::Busy), Times::Public);
        assert_eq!(
            busy.line,
            "The BIR's mailbox is busy since Nov 11, 3:10 PM: some returns get in and some don't. A return may need a few tries."
        );
        let offline = words(&sign(Light::Offline), Times::Public);
        assert_eq!(
            offline.line,
            "The BIR's mailbox is offline since Nov 11, 3:10 PM. Returns can't get in until it's back."
        );
        let mut quiet = sign(Light::NoRecentNews);
        quiet.last_state = Some(Light::Offline);
        assert!(words(&quiet, Times::Public).line.starts_with("Nobody tried for 3 hours since the BIR went offline"));
        for light in Light::ALL {
            let line = words(&sign(light), Times::Public).line;
            assert!(!line.contains("by itself") && !line.contains("hold"), "{line}");
        }
    }

    #[test]
    fn the_compact_sign_is_one_dictionary() {
        let mut offline = sign(Light::Offline);
        assert_eq!(
            compact(&offline),
            "v=1, seq=4182, state=offline, dispatcher=online, upload=offline, relay=ok, since=1794381000, note=?0"
        );
        offline.note = Some(Note {
            id: Uuid::nil(),
            kind: NoteKind::Maintenance,
            text: "x".into(),
            starts_at: offline.since,
            until: offline.since + Duration::hours(2),
        });
        assert!(compact(&offline).ends_with("note=?1"));
        // The tag is a hash of the bytes: no revision, no time in it.
        let published =
            Published::of(round_down(offline.evaluated_at), Arc::new(offline.clone()), "https://ebirforms.buwiz.com");
        assert_eq!(published.etag, etag(&published.body));
        assert_eq!(published.etag.len(), 26);
        assert!(!published.etag.contains(&offline.revision.to_string()));
        assert!(!published.etag.contains(&offline.seq.to_string()));
        let mut later = offline.clone();
        later.revision += 1;
        later.evaluated_at += Duration::seconds(40);
        assert_eq!(
            Published::of(published.mark, Arc::new(later), "https://ebirforms.buwiz.com").etag,
            published.etag,
            "the same bytes, the same tag"
        );
    }

    #[test]
    fn the_banner_shows_for_busy_offline_and_notes_that_hold() {
        assert_eq!(banner(&sign(Light::Online)), None);
        assert_eq!(banner(&sign(Light::NoRecentNews)), None);
        assert_eq!(banner(&sign(Light::Busy)).map(|banner| banner.tone), Some(Tone::Busy));
        assert_eq!(banner(&sign(Light::Offline)).map(|banner| banner.tone), Some(Tone::Offline));
        let mut noted = sign(Light::Online);
        let note = |kind| Note {
            id: Uuid::nil(),
            kind,
            text: "Made-up note".into(),
            starts_at: noted.since,
            until: noted.since + Duration::hours(1),
        };
        noted.note = Some(note(NoteKind::Online));
        assert_eq!(banner(&noted), None, "declared online");
        noted.state = Light::Offline;
        noted.note = Some(note(NoteKind::Maintenance));
        let shown = banner(&noted).unwrap();
        assert_eq!((shown.tone, shown.note.as_deref()), (Tone::Maintenance, Some("Made-up note")));
        noted.note = Some(note(NoteKind::Outage));
        assert_eq!(banner(&noted).unwrap().tone, Tone::Offline);
    }
}
