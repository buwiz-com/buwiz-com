//! The public BIR status page (`/bir-status` on the filing site) and the
//! banner at the top of My TINs, both from `BIR_UPTIME=public`
//! (docs/bir-uptime.md §4.4). The page shows the sign in large words with
//! since when, the two windows, a hand-set note, the last 24 hours, a
//! calendar of the last 90 days (a link per day, so it works by keyboard and
//! without script), what each state means for a return and how Buwiz knows.
//! Nothing on it is personal: the same public sign and history every app
//! reads, times rounded to 5 minutes. Styles are in
//! `src/account/bir-status.css`. Every colour goes with a word and a shape.

use time::{Date, Duration, OffsetDateTime, Weekday};

use super::home::footer;
use super::*;
use crate::sites::canonical::is_app_host;
use crate::uptime::history::{self, DayLine, History};
use crate::uptime::keys::{manila_day, manila_midnight};
use crate::uptime::public::{self, Times, round_down};
use crate::uptime::sign::{Lamp, Light, NoteKind, SignState};

pub(super) const TITLE: &str = "BIR status: is the BIR taking returns? · Buwiz";
pub(super) const DESCRIPTION: &str = "Whether the BIR's eFPS/eBIRForms servers are taking returns right now, the last 24 hours and the last 90 days, from the returns people send through Buwiz.";

/// How one state looks: its class (the colour), its shape and its word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Look {
    pub class: &'static str,
    pub glyph: &'static str,
    pub word: &'static str,
}

/// By the history's letters: `o` online, `b` busy, `x` offline, `m` set by
/// staff, `n` no recent news, `w` not watched, `-` no data.
pub(super) fn look(letter: char) -> Look {
    let (class, glyph, word) = match letter {
        'o' => ("t-o", "check_circle", "Online"),
        'b' => ("t-b", "hourglass_top", "Busy"),
        'x' => ("t-x", "cancel", "Offline"),
        'm' => ("t-m", "construction", "Set by staff"),
        'n' => ("t-n", "more_horiz", "No recent news"),
        'w' => ("t-w", "visibility_off", "Not watched"),
        _ => ("t-none", "remove", "No data"),
    };
    Look { class, glyph, word }
}

fn letter_of(light: Light) -> char {
    match light {
        Light::Online => 'o',
        Light::Busy => 'b',
        Light::Offline => 'x',
        Light::NoRecentNews => 'n',
    }
}

/// The sign's own look: a hand-set outage or maintenance is "set by staff".
fn sign_look(sign: &SignState) -> Look {
    let holding = sign.note.as_ref().is_some_and(|note| note.kind.holds());
    let mut shown = look(if holding { 'm' } else { letter_of(sign.state) });
    shown.word = public::state_word(sign.state);
    shown
}

/// `3:10 PM` in Manila.
fn clock(at: OffsetDateTime) -> String {
    crate::uptime::alerts::manila_time(at)
}

/// `8 h 50 min`.
fn duration(minutes: i32) -> String {
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    }
}

fn weekday_name(day: Date) -> &'static str {
    match day.weekday() {
        Weekday::Monday => "Monday",
        Weekday::Tuesday => "Tuesday",
        Weekday::Wednesday => "Wednesday",
        Weekday::Thursday => "Thursday",
        Weekday::Friday => "Friday",
        Weekday::Saturday => "Saturday",
        Weekday::Sunday => "Sunday",
    }
}

fn month_name(month: time::Month) -> &'static str {
    use time::Month::*;
    match month {
        January => "January",
        February => "February",
        March => "March",
        April => "April",
        May => "May",
        June => "June",
        July => "July",
        August => "August",
        September => "September",
        October => "October",
        November => "November",
        December => "December",
    }
}

/// `Tuesday, November 11`.
fn long_day(day: Date) -> String {
    format!("{}, {} {}", weekday_name(day), month_name(day.month()), day.day())
}

/// A run of one state on a bar: its time span and look.
#[derive(Clone, Debug)]
pub(super) struct Span {
    pub when: String,
    pub look: Look,
}

/// Runs of `letters`, each `step` long from `start`, the last ending at
/// `last_end`, as Manila times.
fn spans(letters: &str, start: OffsetDateTime, step: Duration, last_end: OffsetDateTime) -> Vec<Span> {
    let letters: Vec<char> = letters.chars().collect();
    let mut spans = Vec::new();
    let mut from = 0;
    for index in 1..=letters.len() {
        if index < letters.len() && letters[index] == letters[from] {
            continue;
        }
        let begin = start + step * u32::try_from(from).unwrap_or(0);
        let end = if index == letters.len() { last_end } else { start + step * u32::try_from(index).unwrap_or(0) };
        spans.push(Span { when: format!("{} – {}", clock(begin), clock(end)), look: look(letters[from]) });
        from = index;
    }
    spans
}

/// One cell of the calendar.
#[derive(Clone, Debug)]
pub(super) enum Cell {
    /// Before the first day shown, or a weekday before the month's first.
    Blank,
    Day {
        number: u8,
        href: String,
        label: String,
        look: Look,
        selected: bool,
        today: bool,
        hours: Vec<&'static str>,
    },
}

#[derive(Clone, Debug)]
pub(super) struct Month {
    pub title: String,
    pub cells: Vec<Cell>,
}

/// What a day's cell says to a screen reader: the day, its worst state and
/// how long each state lasted.
fn day_label(line: &DayLine) -> String {
    let worst = look(line.worst_letter());
    let mut parts = Vec::new();
    for (minutes, word) in [
        (line.offline_minutes, "offline"),
        (line.busy_minutes, "busy"),
        (line.online_minutes, "online"),
        (line.no_recent_news_minutes, "no recent news"),
        (line.not_watched_minutes, "not watched"),
    ] {
        if minutes > 0 {
            parts.push(format!("{word} {}", duration(minutes)));
        }
    }
    let staff = match line.note {
        Some("outage") => "; an outage set by Buwiz staff",
        Some("maintenance") => "; planned maintenance set by Buwiz staff",
        Some("online") => "; marked online by Buwiz staff",
        _ => "",
    };
    if parts.is_empty() {
        format!("{}: {}{staff}", long_day(line.day), worst.word)
    } else {
        format!("{}: {}. {}{staff}", long_day(line.day), worst.word, parts.join(", "))
    }
}

/// The calendar's months: one cell per day shown, weeks from Sunday.
fn months(history: &History, selected: Date, today: Date) -> Vec<Month> {
    let mut months: Vec<Month> = Vec::new();
    for line in &history.days {
        let day = line.day;
        let title = format!("{} {}", month_name(day.month()), day.year());
        if months.last().is_none_or(|month| month.title != title) {
            // Blanks for the weekdays before this month's first day shown,
            // and for the days of the month before the range.
            let first = Date::from_calendar_date(day.year(), day.month(), 1).unwrap_or(day);
            let blanks = usize::from(first.weekday().number_days_from_sunday()) + usize::from(day.day() - 1);
            months.push(Month { title, cells: vec![Cell::Blank; blanks] });
        }
        let look = look(line.worst_letter());
        let hours = line.hours.chars().map(|letter| self::look(letter).class).collect();
        let cell = Cell::Day {
            number: day.day(),
            href: format!("?day={}#day", history::iso(day)),
            label: day_label(line),
            look,
            selected: day == selected,
            today: day == today,
            hours,
        };
        if let Some(month) = months.last_mut() {
            month.cells.push(cell);
        }
    }
    months
}

/// One window's card.
#[derive(Clone, Debug)]
pub(super) struct Window {
    pub name: &'static str,
    pub does: &'static str,
    pub look: Look,
    pub line: String,
}

fn window(name: &'static str, does: &'static str, lamp: &Lamp) -> Window {
    let since = public::manila_when(round_down(lamp.since));
    let last = lamp.last_ok_at.map(|at| public::manila_when(round_down(at)));
    let line = match (lamp.state, last) {
        (Light::Online, Some(last)) => format!("Since {since}. A return last got past it at about {last}."),
        (Light::Online, None) => format!("Since {since}."),
        (Light::Busy, _) => format!("Since {since}. Some returns get past it, some don't."),
        (Light::Offline, _) => format!("Since {since}. Returns that reach it don't get in."),
        (Light::NoRecentNews, Some(last)) => {
            format!("No return reached it in the last hour. One last got past it at about {last}.")
        }
        (Light::NoRecentNews, None) => "No return reached it in the last hour.".to_owned(),
    };
    Window { name, does, look: look(letter_of(lamp.state)), line }
}

/// What each state means for a return.
const MEANINGS: [(char, &str); 6] = [
    ('o', "Returns are getting in. Send as usual."),
    (
        'b',
        "Some returns get in and some don't. Your app keeps trying queued returns, but a return may need a few tries; leave a little time before the due date.",
    ),
    (
        'x',
        "Returns can't get in. Queued returns wait in your app and nothing reaches the BIR; check your app's send queue before the due date.",
    ),
    (
        'm',
        "Buwiz staff marked an outage, or maintenance the BIR announced. Returns can't get in until it ends; the note says until when.",
    ),
    (
        'n',
        "Nobody sent a return through Buwiz in the last hour, so Buwiz can't say. Send as usual: your app tells you if a return doesn't get in.",
    ),
    (
        'w',
        "Buwiz wasn't watching then (its server was stopped or restarting). That time says nothing about the BIR, and it counts in no percentage.",
    ),
];

#[page("/bir-status")]
pub async fn bir_status_page(cx: &Cx) -> Result<impl View> {
    if is_app_host(host(cx)) {
        return Err(topcoat::router::error::not_found().into());
    }
    let state = state(cx);
    let now = OffsetDateTime::now_utc();
    let today = manila_day(now);
    let (from, to) = history::range(None, None, now).map_err(|error| fail(error.message()))?;
    let history = history::history(&state.db, from, to, now).await.map_err(fail)?;
    let selected = query(cx, "day")
        .and_then(|raw| history::parse_day(&raw))
        .filter(|day| *day >= from && *day <= to)
        .unwrap_or(today);
    let sign = state.uptime.live.sign();
    let words = public::words(&sign, Times::Public);
    let shown = sign_look(&sign);
    let since = public::manila_when(round_down(sign.since));
    let note = sign.note.clone();
    let note_title = match note.as_ref().map(|note| note.kind) {
        Some(NoteKind::Outage) => "Outage, set by Buwiz staff",
        Some(NoteKind::Maintenance) => "Planned maintenance, set by Buwiz staff",
        _ => "Marked online by Buwiz staff",
    };
    let note_until = note.as_ref().map(|note| public::manila_when(public::round_up(note.until))).unwrap_or_default();
    let windows = vec![
        window("Front desk", "Says which mailbox takes your return", &sign.dispatcher),
        window("Mailbox", "Takes your return", &sign.upload),
    ];
    let end = history::quarter_end(now);
    let start = end - Duration::DAY;
    let bar: Vec<Look> = history.last_24h.chars().map(look).collect();
    let bar_spans = spans(&history.last_24h, start, history::SLOT, now);
    let day_line = history.days.iter().find(|line| line.day == selected).cloned();
    let day_title = if selected == today { format!("Today, {}", long_day(selected)) } else { long_day(selected) };
    let (day_bar, day_spans, shares) = match &day_line {
        Some(line) => {
            let hours: String = if selected == today {
                // Hours still to come are not part of the day yet.
                let now_hour = usize::try_from((now - manila_midnight(today)).whole_hours()).unwrap_or(0);
                line.hours.chars().take(now_hour + 1).collect()
            } else {
                line.hours.clone()
            };
            let last_end = if selected == today { now } else { manila_midnight(selected) + Duration::DAY };
            let shares = shares(line);
            (
                line.hours.chars().map(look).collect::<Vec<_>>(),
                spans(&hours, manila_midnight(selected), Duration::HOUR, last_end),
                shares,
            )
        }
        None => (Vec::new(), Vec::new(), String::new()),
    };
    let months = months(&history, selected, today);
    let current = if sign.note.as_ref().is_some_and(|note| note.kind.holds()) { 'm' } else { letter_of(sign.state) };
    let meanings: Vec<(Look, &'static str, bool)> =
        MEANINGS.iter().map(|(letter, text)| (look(*letter), *text, *letter == current)).collect();
    let legend: Vec<Look> = ['o', 'b', 'x', 'm', 'n', 'w', '-'].into_iter().map(look).collect();
    let summary = bar_spans.iter().map(|span| format!("{}: {}", span.when, span.look.word)).collect::<Vec<_>>();
    let bar_label = format!("The last 24 hours, Manila time. {}", summary.join("; "));
    Ok(view! {
        <main class="home page bir">
            <div class="page-in narrow">
                <div class="page-head">
                    <div>
                        <h1>"Is the BIR taking returns?"</h1>
                        <p class="page-lead">"From the real returns people send through Buwiz. Buwiz never tests the BIR's servers. Manila time; times rounded to 5 minutes."</p>
                    </div>
                </div>
                <section class=(format!("bir-sign {}", shown.class)) aria-labelledby="bir-now">
                    <span class="bir-shape">icon(name: shown.glyph)</span>
                    <div class="bir-sign-text">
                        <p class="bir-eyebrow">"Right now"</p>
                        <h2 id="bir-now" class="bir-word">(shown.word)</h2>
                        <p class="bir-since">"Since " (since.clone())</p>
                        <p class="bir-line">(words.line.clone())</p>
                    </div>
                </section>
                if let Some(note) = note.clone() {
                    <section class="bir-note" aria-label="Note from Buwiz staff">
                        icon(name: "campaign")
                        <div>
                            <p><strong>(note_title)</strong> " · until " (note_until.clone())</p>
                            <p class="bir-note-text">(note.text.clone())</p>
                        </div>
                    </section>
                }
                <section class="bir-windows" aria-label="The BIR's two servers">
                    for item in windows {
                        <article class=(format!("bir-window {}", item.look.class))>
                            <span class="bir-light">icon(name: item.look.glyph)</span>
                            <div>
                                <h2>(item.name) <span class="bir-window-does">(item.does)</span></h2>
                                <p class="bir-window-word">(item.look.word)</p>
                                <p class="bir-window-line">(item.line.clone())</p>
                            </div>
                        </article>
                    }
                </section>
                <section class="bir-panel" aria-labelledby="bir-24h">
                    <h2 id="bir-24h">"Last 24 hours"</h2>
                    <div class="bir-bar big" role="img" aria-label=(bar_label)>
                        for item in bar {
                            <i class=(item.class)></i>
                        }
                    </div>
                    <div class="bir-axis" aria-hidden="true"><span>"24 h ago"</span><span>"12 h ago"</span><span>"Now"</span></div>
                    <ul class="bir-spans" aria-label="The last 24 hours, state by state">
                        for span in bar_spans {
                            <li class=(span.look.class)>icon(name: span.look.glyph) <time>(span.when.clone())</time> <span>(span.look.word)</span></li>
                        }
                    </ul>
                </section>
                <section class="bir-panel" aria-labelledby="bir-days">
                    <h2 id="bir-days">"Last 90 days"</h2>
                    <p class="bir-hint">"Each day shows its worst state. Pick a day to see its hours."</p>
                    <ul class="bir-legend" aria-label="What the colours and shapes mean">
                        for item in legend {
                            <li class=(item.class)><span class="bir-swatch">icon(name: item.glyph)</span>(item.word)</li>
                        }
                    </ul>
                    <div class="bir-months">
                        for month in months {
                            <section class="bir-month" aria-label=(month.title.clone())>
                                <h3>(month.title.clone())</h3>
                                <div class="bir-week" aria-hidden="true">
                                    <span>"S"</span><span>"M"</span><span>"T"</span><span>"W"</span><span>"T"</span><span>"F"</span><span>"S"</span>
                                </div>
                                <ol class="bir-cal">
                                    for cell in month.cells {
                                        day_cell(cell: cell)
                                    }
                                </ol>
                            </section>
                        }
                    </div>
                    <section id="day" class="bir-day-detail" aria-labelledby="bir-day-h" tabindex="-1">
                        <h3 id="bir-day-h">(day_title)</h3>
                        if day_bar.is_empty() {
                            <p class="bir-hint">"No data for this day."</p>
                        } else {
                            <div class="bir-bar hours" aria-hidden="true">
                                for item in day_bar {
                                    <i class=(item.class)></i>
                                }
                            </div>
                            <div class="bir-axis" aria-hidden="true"><span>"12 AM"</span><span>"6 AM"</span><span>"12 PM"</span><span>"6 PM"</span><span>"12 AM"</span></div>
                            <ul class="bir-spans" aria-label="The day, hour by hour">
                                for span in day_spans {
                                    <li class=(span.look.class)>icon(name: span.look.glyph) <time>(span.when.clone())</time> <span>(span.look.word)</span></li>
                                }
                            </ul>
                            if !shares.is_empty() {
                                <p class="bir-shares">(shares)</p>
                            }
                        }
                    </section>
                </section>
                <section class="bir-panel" aria-labelledby="bir-means">
                    <h2 id="bir-means">"What this means for your return"</h2>
                    <dl class="bir-means">
                        for (item, text, now) in meanings {
                            <div class=(if now { format!("{} now", item.class) } else { item.class.to_owned() })>
                                <dt>icon(name: item.glyph) (item.word) if now { <span class="bir-now-tag">"Now"</span> }</dt>
                                <dd>(text)</dd>
                            </div>
                        }
                    </dl>
                </section>
                <section class="bir-panel" aria-labelledby="bir-how">
                    <h2 id="bir-how">"How Buwiz knows"</h2>
                    <div class="bir-how">
                        <p>"Every return sent through Buwiz goes to the BIR's front desk, which says which mailbox takes it, and then to that mailbox. Buwiz notes whether each real return got past each one. It never tests, pings or probes the BIR's servers: everything here comes from returns people sent anyway."</p>
                        <p>"Offline means at least 3 people on 3 different networks couldn't get in within 15 minutes, and nobody got in since. Busy means some got in and some didn't. No recent news means nobody sent a return in the last hour. Buwiz staff can also mark an outage or planned maintenance by hand; a note then says so."</p>
                        <p>"No names, TINs or addresses are shown here."</p>
                    </div>
                </section>
            </div>
            footer()
        </main>
    })
}

/// "Online 92% · Offline 8% of the time Buwiz watched."
fn shares(line: &DayLine) -> String {
    let parts: Vec<String> = [
        (line.online_percent, "Online"),
        (line.busy_percent, "Busy"),
        (line.offline_percent, "Offline"),
        (line.no_recent_news_percent, "No recent news"),
    ]
    .into_iter()
    .filter_map(|(percent, word)| percent.filter(|percent| *percent > 0).map(|percent| format!("{word} {percent}%")))
    .collect();
    if parts.is_empty() {
        return String::new();
    }
    let mut text = format!("{} of the time Buwiz watched.", parts.join(" · "));
    if line.not_watched_minutes > 0 {
        text.push_str(&format!(" Not watched for {}.", duration(line.not_watched_minutes)));
    }
    text
}

/// One day of the calendar: a link to its hours, its worst state as a
/// colour, a shape and (for screen readers) words.
#[component]
async fn day_cell(cell: Cell) -> Result<impl View> {
    Ok(view! {
        match cell {
            Cell::Blank => <li class="bir-blank" aria-hidden="true"></li>,
            Cell::Day { number, href, label, look, selected, today, hours } => {
                <li>
                    <a
                        class=(format!("bir-date {}{}{}", look.class, if selected { " sel" } else { "" }, if today { " today" } else { "" }))
                        href=(href)
                        aria-label=(label)
                        aria-current=(today.then_some("date"))
                    >
                        <span class="bir-date-n">(number.to_string())</span>
                        <span class="bir-date-s">icon(name: look.glyph)</span>
                        <span class="bir-mini" aria-hidden="true">
                            for class in hours {
                                <i class=(class)></i>
                            }
                        </span>
                    </a>
                </li>
            },
        }
    })
}

/// The line at the top of My TINs while the BIR is Busy or Offline or staff
/// set an outage or maintenance; nothing otherwise, and nothing below
/// `BIR_UPTIME=public`.
pub(super) fn banner_for(cx: &Cx) -> Option<public::Banner> {
    let state = state(cx);
    if !state.uptime.level.is_public() {
        return None;
    }
    public::banner(&state.uptime.live.sign())
}

#[component]
pub(super) async fn bir_banner(cx: &Cx) -> Result<impl View> {
    let banner = banner_for(cx);
    let link = format!("{}{}", state(cx).config.forms_origin, public::STATUS_PATH);
    Ok(view! {
        if let Some(banner) = banner {
            <p class=(format!("bir-banner {}", banner.tone.class())) role="status">
                icon(name: banner.tone.icon())
                <span>
                    <strong>(format!("{}.", banner.title))</strong> " " (banner.line.clone())
                    if let Some(note) = banner.note.clone() {
                        " " <q>(note)</q>
                    }
                    " " <a href=(link)>"BIR status"</a>
                </span>
            </p>
        }
    })
}
