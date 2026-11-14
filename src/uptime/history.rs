//! The public history (docs/bir-uptime.md §4.3): one line per Manila day
//! from `bir_days`, today's computed live, and the last 24 hours in
//! 15-minute letters.
//!
//! Nothing here can single out one person on a quiet day: no count of
//! tries, accounts or networks, only how long each state lasted (rounded to
//! 5 minutes) and its share of the time Buwiz was watching (whole percent).
//! Exact counts are for a later, staff-only variant.

use serde::Serialize;
use sqlx::PgPool;
use time::{Date, Duration, OffsetDateTime};

use super::days::{self, letter_rank};
use super::keys::{manila_day, manila_midnight};
use super::public::VERSION;
use super::sign::NoteKind;

/// The most days one answer holds, and the default range.
pub const MAX_DAYS: i64 = 90;

/// The last 24 hours: 96 letters of 15 minutes.
pub const SLOT: Duration = Duration::minutes(15);
pub const SLOTS: usize = 96;

/// Durations are rounded to this many minutes.
const ROUND_MINUTES: i32 = 5;

/// One day.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DayLine {
    #[serde(serialize_with = "iso_date")]
    pub day: Date,
    /// The worst state of the day: `online`, `busy`, `offline`,
    /// `no_recent_news`; `not_watched` for a day Buwiz watched none of;
    /// `none` for a day with no data (before the history began).
    pub state: &'static str,
    /// One letter per Manila hour: `o` online, `b` busy, `x` offline, `m`
    /// a hand-set outage or maintenance, `n` no recent news, `w` not
    /// watched, `-` no data (and hours still to come).
    pub hours: String,
    pub online_minutes: i32,
    pub busy_minutes: i32,
    pub offline_minutes: i32,
    pub no_recent_news_minutes: i32,
    /// Minutes a hand-set note was in force.
    pub note_minutes: i32,
    /// Minutes Buwiz was not watching: in no other count or percentage.
    pub not_watched_minutes: i32,
    /// Shares of the time watched, in whole percent; null when none was.
    pub online_percent: Option<u8>,
    pub busy_percent: Option<u8>,
    pub offline_percent: Option<u8>,
    pub no_recent_news_percent: Option<u8>,
    /// The strongest hand-set note that day: `outage`, `maintenance`,
    /// `online`, or absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

fn iso_date<S: serde::Serializer>(day: &Date, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&iso(*day))
}

/// `YYYY-MM-DD`.
pub fn iso(day: Date) -> String {
    format!("{:04}-{:02}-{:02}", day.year(), u8::from(day.month()), day.day())
}

/// The years a day asked for may fall in; any other is not a date here.
pub const YEARS: std::ops::RangeInclusive<i32> = 2000..=2100;

/// Reads `YYYY-MM-DD`, a year of [`YEARS`].
pub fn parse_day(raw: &str) -> Option<Date> {
    let format = time::macros::format_description!("[year]-[month]-[day]");
    Date::parse(raw.trim(), &format).ok().filter(|day| YEARS.contains(&day.year()))
}

impl DayLine {
    /// The worst letter of its hours: what its calendar cell shows.
    pub fn worst_letter(&self) -> char {
        self.hours.chars().max_by_key(|letter| letter_rank(*letter)).unwrap_or('-')
    }
}

/// The history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct History {
    pub v: u32,
    #[serde(serialize_with = "iso_date")]
    pub from: Date,
    #[serde(serialize_with = "iso_date")]
    pub to: Date,
    pub days: Vec<DayLine>,
    /// 96 letters, one per 15 minutes, oldest first; the last is the
    /// quarter hour now running.
    pub last_24h: String,
}

/// Why a range is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeError {
    NotADate,
    Backwards,
    InTheFuture,
    TooLong,
    /// `from` or `to` given twice.
    Repeated,
}

impl RangeError {
    pub fn message(self) -> &'static str {
        match self {
            Self::NotADate => "from and to are Manila dates, YYYY-MM-DD",
            Self::Backwards => "from must not be after to",
            Self::InTheFuture => "to must not be after today (Manila)",
            Self::TooLong => "at most 90 days at a time",
            Self::Repeated => "give from and to at most once each",
        }
    }
}

/// The range asked for: by default the 90 days up to today (Manila); at
/// most 90 days, none after today.
pub fn range(from: Option<&str>, to: Option<&str>, now: OffsetDateTime) -> Result<(Date, Date), RangeError> {
    let today = manila_day(now);
    let read = |raw: Option<&str>| raw.map(|raw| parse_day(raw).ok_or(RangeError::NotADate)).transpose();
    let (from, to) = (read(from)?, read(to)?);
    let to = to.unwrap_or(today);
    let from = match from {
        Some(from) => from,
        None => to.checked_sub(Duration::days(MAX_DAYS - 1)).ok_or(RangeError::NotADate)?,
    };
    if to > today {
        return Err(RangeError::InTheFuture);
    }
    if from > to {
        return Err(RangeError::Backwards);
    }
    if (to - from).whole_days() >= MAX_DAYS {
        return Err(RangeError::TooLong);
    }
    Ok((from, to))
}

fn rounded(minutes: i32) -> i32 {
    (minutes + ROUND_MINUTES / 2) / ROUND_MINUTES * ROUND_MINUTES
}

/// The minutes of one day, raw: online, busy, offline, no recent news,
/// note, not watched.
type Minutes = [i32; 6];

fn line(day: Date, worst: &str, hours: String, minutes: Minutes, note: Option<&'static str>) -> DayLine {
    let [online, busy, offline, quiet, note_minutes, unwatched] = minutes;
    let watched = i64::from(online) + i64::from(busy) + i64::from(offline) + i64::from(quiet);
    let percent =
        |part: i32| (watched > 0).then(|| u8::try_from((i64::from(part) * 100 + watched / 2) / watched).unwrap_or(100));
    let state = match worst {
        "online" => "online",
        "busy" => "busy",
        "offline" => "offline",
        "no_recent_news" => "no_recent_news",
        _ if unwatched > 0 => "not_watched",
        _ => "none",
    };
    DayLine {
        day,
        state,
        hours,
        online_minutes: rounded(online),
        busy_minutes: rounded(busy),
        offline_minutes: rounded(offline),
        no_recent_news_minutes: rounded(quiet),
        note_minutes: rounded(note_minutes),
        not_watched_minutes: rounded(unwatched),
        online_percent: percent(online),
        busy_percent: percent(busy),
        offline_percent: percent(offline),
        no_recent_news_percent: percent(quiet),
        note,
    }
}

/// The strongest note kind among `kinds`.
fn strongest(kinds: impl Iterator<Item = NoteKind>) -> Option<&'static str> {
    let rank = |kind: &NoteKind| match kind {
        NoteKind::Outage => 3,
        NoteKind::Maintenance => 2,
        NoteKind::Online => 1,
    };
    kinds.max_by_key(rank).map(NoteKind::as_str)
}

/// The history of `from` to `to` (Manila days, today at most), with today
/// computed live, and the last 24 hours, at `now`.
pub async fn history(db: &PgPool, from: Date, to: Date, now: OffsetDateTime) -> sqlx::Result<History> {
    type Row = (Date, String, String, i32, i32, i32, i32, i32, i32);
    let today = manila_day(now);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT day, worst, hours, online_minutes, busy_minutes, offline_minutes, quiet_minutes, note_minutes,
                unwatched_minutes
           FROM bir_days WHERE day >= $1 AND day <= $2 AND day < $3 ORDER BY day",
    )
    .bind(from)
    .bind(to)
    .bind(today)
    .fetch_all(db)
    .await?;
    let start = manila_midnight(from);
    let end = manila_midnight(to) + Duration::DAY;
    let notes = days::record(db, start, end.min(now).max(start)).await?.notes;
    let note_of = |day: Date| {
        let (begin, finish) = (manila_midnight(day), manila_midnight(day) + Duration::DAY);
        strongest(notes.iter().filter(|(_, from, to)| *from < finish && *to > begin).map(|(kind, ..)| *kind))
    };
    let mut lines = Vec::new();
    let mut day = from;
    loop {
        let found = rows.iter().find(|row| row.0 == day);
        let line = match found {
            _ if day == today => {
                let states = days::states(db, day, now).await?;
                let minutes = [
                    states.online_minutes,
                    states.busy_minutes,
                    states.offline_minutes,
                    states.quiet_minutes,
                    states.note_minutes,
                    states.unwatched_minutes,
                ];
                line(day, states.worst, states.hours, minutes, note_of(day))
            }
            Some((_, worst, hours, online, busy, offline, quiet, note, unwatched)) => {
                line(day, worst, hours.clone(), [*online, *busy, *offline, *quiet, *note, *unwatched], note_of(day))
            }
            None => line(day, "none", "-".repeat(24), [0; 6], None),
        };
        lines.push(line);
        match day.next_day() {
            Some(next) if next <= to => day = next,
            _ => break,
        }
    }
    Ok(History { v: VERSION, from, to, days: lines, last_24h: last_24h(db, now).await? })
}

/// The quarter hour now running ends here.
pub fn quarter_end(now: OffsetDateTime) -> OffsetDateTime {
    let seconds = now.unix_timestamp();
    let step = SLOT.whole_seconds();
    OffsetDateTime::from_unix_timestamp(seconds - seconds.rem_euclid(step) + step).unwrap_or(now)
}

/// 96 letters of 15 minutes, oldest first, the last the quarter hour now
/// running (up to `now`).
pub async fn last_24h(db: &PgPool, now: OffsetDateTime) -> sqlx::Result<String> {
    let end = quarter_end(now);
    let start = end - Duration::DAY;
    let record = days::record(db, start, now.max(start)).await?;
    Ok(days::slot_letters(
        start,
        SLOT,
        SLOTS,
        &record.overall,
        [&record.dispatcher, &record.upload],
        &record.notes,
        &record.gaps,
        now,
    ))
}

#[cfg(test)]
mod tests {
    use time::macros::{date, datetime};

    use super::*;

    #[test]
    fn the_range_defaults_to_90_days_and_never_more() {
        let now = datetime!(2026-11-14 03:00 UTC);
        assert_eq!(range(None, None, now), Ok((date!(2026 - 08 - 17), date!(2026 - 11 - 14))));
        assert_eq!(
            range(Some("2026-11-01"), Some("2026-11-10"), now),
            Ok((date!(2026 - 11 - 01), date!(2026 - 11 - 10)))
        );
        assert_eq!(range(None, Some("2026-10-01"), now), Ok((date!(2026 - 07 - 04), date!(2026 - 10 - 01))));
        assert_eq!(range(Some("2026-08-17"), None, now), Ok((date!(2026 - 08 - 17), date!(2026 - 11 - 14))));
        assert_eq!(range(Some("2026-08-16"), None, now), Err(RangeError::TooLong));
        assert_eq!(range(Some("2026-11-10"), Some("2026-11-01"), now), Err(RangeError::Backwards));
        assert_eq!(range(None, Some("2026-11-15"), now), Err(RangeError::InTheFuture));
        assert_eq!(range(Some("yesterday"), None, now), Err(RangeError::NotADate));
        // Years outside a sane range are no date: no arithmetic can overflow.
        for bad in ["-9999-01-01", "+9999-12-31", "0000-01-01", "1999-12-31", "2101-01-01", "99999-01-01"] {
            assert_eq!(range(None, Some(bad), now), Err(RangeError::NotADate), "{bad}");
            assert_eq!(range(Some(bad), None, now), Err(RangeError::NotADate), "{bad}");
        }
        assert_eq!(range(Some("2000-01-01"), Some("2000-03-01"), now), Ok((date!(2000 - 01 - 01), date!(2000 - 03 - 01))));
        // Manila's day: 16:30 UTC on the 14th is already the 15th there.
        assert_eq!(range(None, Some("2026-11-15"), datetime!(2026-11-14 16:30 UTC)).unwrap().1, date!(2026 - 11 - 15));
    }

    #[test]
    fn a_line_rounds_durations_and_shares_only_watched_time() {
        let shown = line(date!(2026 - 11 - 11), "offline", "x".repeat(24), [0, 0, 532, 600, 0, 308], Some("outage"));
        assert_eq!((shown.offline_minutes, shown.no_recent_news_minutes, shown.not_watched_minutes), (530, 600, 310));
        assert_eq!(
            (shown.offline_percent, shown.no_recent_news_percent, shown.online_percent),
            (Some(47), Some(53), Some(0))
        );
        let unwatched = line(date!(2026 - 11 - 12), "none", "w".repeat(24), [0, 0, 0, 0, 0, 1440], None);
        assert_eq!((unwatched.state, unwatched.online_percent), ("not_watched", None));
        let none = line(date!(2026 - 11 - 13), "none", "-".repeat(24), [0; 6], None);
        assert_eq!(none.state, "none");
        let value = serde_json::to_value(&shown).unwrap();
        assert_eq!(value["day"], "2026-11-11");
        assert_eq!(value["note"], "outage");
        assert!(serde_json::to_value(&none).unwrap().get("note").is_none());
        assert_eq!(quarter_end(datetime!(2026-11-14 03:07:12 UTC)), datetime!(2026-11-14 03:15 UTC));
    }
}
