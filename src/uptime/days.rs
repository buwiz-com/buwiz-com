//! The day history: one `bir_days` line per Manila day, kept forever, and
//! the retention of the other uptime tables.
//!
//! A line is computed from the record: the states from `bir_events` (the
//! overall sign and both windows), the hand-set notes from `bir_notes`, the
//! tries' counts from `bir_tries`, held requests from `bir_held`, and the
//! day's distinct accounts and networks from memory ([`super::keys::DaySets`]).
//! It is rewritten every 10 minutes while the day lasts and once more after
//! it ends, then marked final and never computed again (the tries it was
//! counted from are deleted after 90 days). Computing it twice gives the
//! same line.
//!
//! Distinct accounts and networks across a restart: the raw sets are in
//! memory only, so a restart starts them again. The line keeps the largest
//! of what was counted before the restart, what is counted after it, and
//! the distinct keys of the day's tries within one key period: a lower
//! bound, never more than the truth.

use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;
use time::{Date, Duration, OffsetDateTime};

use super::Uptime;
use super::keys::{DaySets, manila_day, manila_midnight};
use super::sign::{Light, NoteKind, Side};

/// A day is final this long after it ends.
pub const FINAL_AFTER: Duration = Duration::minutes(10);

/// Days of `bir_held` kept (for proofs of outage).
pub const HELD_KEPT_DAYS: i32 = 90;

/// One window's offline stretch, or a hand-set outage or maintenance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Outage {
    #[serde(with = "time::serde::rfc3339")]
    pub from: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub to: OffsetDateTime,
    /// `dispatcher`, `upload`, or `overall` for a note.
    pub window: &'static str,
    /// `tries` (evidence) or `staff` (a note).
    pub source: &'static str,
}

/// One window's day: its worst state and minutes in each.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct WindowDay {
    pub worst: &'static str,
    pub online_minutes: i32,
    pub busy_minutes: i32,
    pub offline_minutes: i32,
    pub quiet_minutes: i32,
}

/// What the states of one day add up to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DayStates {
    pub online_minutes: i32,
    pub busy_minutes: i32,
    pub offline_minutes: i32,
    pub quiet_minutes: i32,
    pub note_minutes: i32,
    /// `online`, `busy`, `offline`, `no_recent_news`, or `none` (no data).
    pub worst: &'static str,
    /// One letter per Manila hour: `o` online, `b` busy, `x` offline, `m` a
    /// hand-set outage or maintenance, `n` no recent news, `-` no data.
    pub hours: String,
    pub outages: Vec<Outage>,
    /// The front desk's and the mailbox's own day.
    pub dispatcher: WindowDay,
    pub upload: WindowDay,
}

/// A state's changes: (when, the state from then; `None` before anything
/// was known), oldest first.
pub type Changes = [(OffsetDateTime, Option<Light>)];

/// A note's span: (kind, from, to).
pub type NoteSpan = (NoteKind, OffsetDateTime, OffsetDateTime);

fn at(changes: &Changes, instant: OffsetDateTime) -> Option<Light> {
    changes.iter().rev().find(|(at, _)| *at <= instant).and_then(|(_, state)| *state)
}

fn letter_rank(letter: char) -> u8 {
    match letter {
        'x' => 5,
        'm' => 4,
        'b' => 3,
        'o' => 2,
        'n' => 1,
        _ => 0,
    }
}

fn worst_rank(state: Option<Light>) -> u8 {
    match state {
        Some(Light::Offline) => 4,
        Some(Light::Busy) => 3,
        Some(Light::Online) => 2,
        Some(Light::NoRecentNews) => 1,
        None => 0,
    }
}

fn minutes(seconds: i64) -> i32 {
    i32::try_from((seconds + 30) / 60).unwrap_or(i32::MAX)
}

/// The states of Manila day `day` up to `now`. Pure.
pub fn day_states(
    day: Date,
    overall: &Changes,
    windows: [&Changes; 2],
    notes: &[NoteSpan],
    now: OffsetDateTime,
) -> DayStates {
    let start = manila_midnight(day);
    let end = (start + Duration::DAY).min(now);
    let mut hours = vec!['-'; 24];
    let mut seconds = [0i64; 5];
    let mut worst = None;
    let mut side_seconds = [[0i64; 4]; 2];
    let mut side_worst = [None; 2];
    let mut outages: Vec<Outage> = Vec::new();
    if end > start {
        let mut cuts: Vec<OffsetDateTime> = (0..24).map(|hour| start + Duration::hours(hour)).collect();
        cuts.push(end);
        let changes = overall.iter().chain(windows[0]).chain(windows[1]).map(|(at, _)| *at);
        let note_edges = notes.iter().flat_map(|(_, from, to)| [*from, *to]);
        cuts.extend(changes.chain(note_edges).filter(|at| *at > start && *at < end));
        cuts.retain(|at| *at <= end);
        cuts.sort();
        cuts.dedup();
        for pair in cuts.windows(2) {
            let (from, to) = (pair[0], pair[1]);
            if from >= end {
                break;
            }
            let state = at(overall, from);
            let sides = [at(windows[0], from), at(windows[1], from)];
            let notes_now: Vec<NoteKind> = notes
                .iter()
                .filter(|(_, begin, finish)| *begin <= from && from < *finish)
                .map(|(kind, ..)| *kind)
                .collect();
            let holding = notes_now.iter().any(|kind| kind.holds());
            let length = (to - from).whole_seconds();
            let letter = match state {
                None => '-',
                Some(Light::Offline) if sides.contains(&Some(Light::Offline)) || !holding => 'x',
                Some(Light::Offline) => 'm',
                Some(Light::Busy) => 'b',
                Some(Light::Online) => 'o',
                Some(Light::NoRecentNews) => 'n',
            };
            let hour = usize::try_from((from - start).whole_hours()).unwrap_or(0).min(23);
            if letter_rank(letter) > letter_rank(hours[hour]) {
                hours[hour] = letter;
            }
            match state {
                Some(Light::Online) => seconds[0] += length,
                Some(Light::Busy) => seconds[1] += length,
                Some(Light::Offline) => seconds[2] += length,
                Some(Light::NoRecentNews) => seconds[3] += length,
                None => {}
            }
            if !notes_now.is_empty() {
                seconds[4] += length;
            }
            if worst_rank(state) > worst_rank(worst) {
                worst = state;
            }
            for (index, side_state) in sides.into_iter().enumerate() {
                if let Some(light) = side_state {
                    let slot = match light {
                        Light::Online => 0,
                        Light::Busy => 1,
                        Light::Offline => 2,
                        Light::NoRecentNews => 3,
                    };
                    side_seconds[index][slot] += length;
                }
                if worst_rank(side_state) > worst_rank(side_worst[index]) {
                    side_worst[index] = side_state;
                }
            }
            let mut stretch = |window: &'static str, source: &'static str| match outages
                .iter_mut()
                .rev()
                .find(|outage| outage.window == window && outage.source == source)
            {
                Some(outage) if outage.to == from => outage.to = to,
                _ => outages.push(Outage { from, to, window, source }),
            };
            for (side, state) in Side::BOTH.into_iter().zip(sides) {
                if state == Some(Light::Offline) {
                    stretch(side.as_str(), "tries");
                }
            }
            if holding {
                stretch("overall", "staff");
            }
        }
    }
    outages.sort_by_key(|outage| outage.from);
    let window_day = |index: usize| WindowDay {
        worst: side_worst[index].map_or("none", Light::as_str),
        online_minutes: minutes(side_seconds[index][0]),
        busy_minutes: minutes(side_seconds[index][1]),
        offline_minutes: minutes(side_seconds[index][2]),
        quiet_minutes: minutes(side_seconds[index][3]),
    };
    DayStates {
        dispatcher: window_day(0),
        upload: window_day(1),
        online_minutes: minutes(seconds[0]),
        busy_minutes: minutes(seconds[1]),
        offline_minutes: minutes(seconds[2]),
        quiet_minutes: minutes(seconds[3]),
        note_minutes: minutes(seconds[4]),
        worst: worst.map_or("none", Light::as_str),
        hours: hours.into_iter().collect(),
        outages,
    }
}

async fn changes(
    db: &PgPool,
    scope: &str,
    start: OffsetDateTime,
    end: OffsetDateTime,
) -> sqlx::Result<Vec<(OffsetDateTime, Option<Light>)>> {
    let mut rows: Vec<(OffsetDateTime, String)> = sqlx::query_as(
        "SELECT at, to_state FROM bir_events WHERE scope = $1 AND at < $2 ORDER BY at DESC, id DESC LIMIT 1",
    )
    .bind(scope)
    .bind(start)
    .fetch_all(db)
    .await?;
    rows.extend(
        sqlx::query_as::<_, (OffsetDateTime, String)>(
            "SELECT at, to_state FROM bir_events WHERE scope = $1 AND at >= $2 AND at < $3 ORDER BY at, id",
        )
        .bind(scope)
        .bind(start)
        .bind(end)
        .fetch_all(db)
        .await?,
    );
    Ok(rows.into_iter().map(|(at, state)| (at, Light::parse(&state))).collect())
}

/// Computes and writes the line of `day` (Manila), up to `now`. A final
/// line is left as it is. `None` for a day that has not begun.
pub async fn roll_up(
    db: &PgPool,
    day_sets: &DaySets,
    day: Date,
    now: OffsetDateTime,
) -> sqlx::Result<Option<DayStates>> {
    let start = manila_midnight(day);
    let day_end = start + Duration::DAY;
    if now <= start {
        return Ok(None);
    }
    let final_now: Option<bool> =
        sqlx::query_scalar("SELECT final FROM bir_days WHERE day = $1").bind(day).fetch_optional(db).await?;
    if final_now == Some(true) {
        return Ok(None);
    }
    let end = day_end.min(now);
    let overall = changes(db, "overall", start, end).await?;
    let dispatcher = changes(db, "dispatcher", start, end).await?;
    let upload = changes(db, "upload", start, end).await?;
    let notes: Vec<(String, OffsetDateTime, OffsetDateTime)> = sqlx::query_as(
        "SELECT kind, starts_at, LEAST(until, COALESCE(cleared_at, until)) FROM bir_notes
          WHERE starts_at < $2 AND LEAST(until, COALESCE(cleared_at, until)) > $1",
    )
    .bind(start)
    .bind(end)
    .fetch_all(db)
    .await?;
    let notes: Vec<NoteSpan> =
        notes.into_iter().filter_map(|(kind, from, to)| Some((NoteKind::parse(&kind)?, from, to))).collect();
    let states = day_states(day, &overall, [&dispatcher, &upload], &notes, now);

    let (tries, got_in, failed, maybe, ours, user_network): (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE counted), count(*) FILTER (WHERE counted AND outcome = 'sent'),
                count(*) FILTER (WHERE counted AND outcome = 'failed'), count(*) FILTER (WHERE counted AND outcome = 'unknown'),
                count(*) FILTER (WHERE failure_class = 'ours'), count(*) FILTER (WHERE failure_class = 'user_network')
           FROM bir_tries WHERE ended_at >= $1 AND ended_at < $2",
    )
    .bind(start)
    .bind(day_end)
    .fetch_one(db)
    .await?;
    let held: i64 = sqlx::query_scalar("SELECT count(*) FROM bir_held WHERE at >= $1 AND at < $2")
        .bind(start)
        .bind(day_end)
        .fetch_one(db)
        .await?;
    // Distinct keys within one key period: a lower bound across a restart.
    let (key_accounts, key_networks): (Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT max(accounts), max(networks) FROM (
             SELECT count(DISTINCT account_key) AS accounts, count(DISTINCT network_key) AS networks
               FROM bir_tries WHERE counted AND ended_at >= $1 AND ended_at < $2 GROUP BY key_epoch) AS periods",
    )
    .bind(start)
    .bind(day_end)
    .fetch_one(db)
    .await?;
    let (set_accounts, set_networks) = day_sets.counts(day).unwrap_or((0, 0));
    let accounts = key_accounts.unwrap_or(0).max(i64::try_from(set_accounts).unwrap_or(i64::MAX));
    let networks = key_networks.unwrap_or(0).max(i64::try_from(set_networks).unwrap_or(i64::MAX));
    let outages = serde_json::to_value(&states.outages).unwrap_or(Value::Array(Vec::new()));
    let windows = serde_json::json!({ "dispatcher": states.dispatcher, "upload": states.upload });
    let is_final = now >= day_end + FINAL_AFTER;
    let count = |value: i64| i32::try_from(value).unwrap_or(i32::MAX);
    sqlx::query(
        "INSERT INTO bir_days (day, tries, got_in, failed, maybe, ours, user_network, held, accounts, networks,
                               online_minutes, busy_minutes, offline_minutes, quiet_minutes, note_minutes, worst,
                               hours, outages, windows, final)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)
         ON CONFLICT (day) DO UPDATE SET
             tries = EXCLUDED.tries, got_in = EXCLUDED.got_in, failed = EXCLUDED.failed, maybe = EXCLUDED.maybe,
             ours = EXCLUDED.ours, user_network = EXCLUDED.user_network, held = EXCLUDED.held,
             accounts = GREATEST(bir_days.accounts, EXCLUDED.accounts),
             networks = GREATEST(bir_days.networks, EXCLUDED.networks),
             online_minutes = EXCLUDED.online_minutes, busy_minutes = EXCLUDED.busy_minutes,
             offline_minutes = EXCLUDED.offline_minutes, quiet_minutes = EXCLUDED.quiet_minutes,
             note_minutes = EXCLUDED.note_minutes, worst = EXCLUDED.worst, hours = EXCLUDED.hours,
             outages = EXCLUDED.outages, windows = EXCLUDED.windows, final = EXCLUDED.final
         WHERE NOT bir_days.final",
    )
    .bind(day)
    .bind(count(tries))
    .bind(count(got_in))
    .bind(count(failed))
    .bind(count(maybe))
    .bind(count(ours))
    .bind(count(user_network))
    .bind(count(held))
    .bind(count(accounts))
    .bind(count(networks))
    .bind(states.online_minutes)
    .bind(states.busy_minutes)
    .bind(states.offline_minutes)
    .bind(states.quiet_minutes)
    .bind(states.note_minutes)
    .bind(states.worst)
    .bind(&states.hours)
    .bind(outages)
    .bind(windows)
    .bind(is_final)
    .execute(db)
    .await?;
    Ok(Some(states))
}

/// Yesterday's line (until it is final) and today's.
pub async fn roll_up_recent(uptime: &Uptime, db: &PgPool, now: OffsetDateTime) -> sqlx::Result<()> {
    let today = manila_day(now);
    if let Some(yesterday) = today.previous_day() {
        roll_up(db, &uptime.day_sets, yesterday, now).await?;
    }
    roll_up(db, &uptime.day_sets, today, now).await?;
    Ok(())
}

/// The hourly upkeep of the uptime tables but `bir_tries` (whose rows go
/// after 90 days, [`super::store::upkeep`]): held requests after 90 days,
/// queue reports once they expire, turns a day after they were given. The
/// day history, the events, the notes, the host keys and the proofs are
/// kept.
pub async fn upkeep(db: &PgPool) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM bir_held WHERE at < now() - make_interval(days => $1)")
        .bind(HELD_KEPT_DAYS)
        .execute(db)
        .await?;
    sqlx::query("DELETE FROM bir_queue_reports WHERE expires_at < now()").execute(db).await?;
    sqlx::query("DELETE FROM bir_turns WHERE granted_at < now() - interval '1 day'").execute(db).await?;
    Ok(())
}
