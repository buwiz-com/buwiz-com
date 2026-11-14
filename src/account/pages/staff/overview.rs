//! The Overview, the staff console's home: the numbers that matter today,
//! the claims waiting longest and, for Admins, what staff did lately.

use super::ui::{ic, kind_icon, page_head, short_span, sx_notice};
use super::*;
use crate::account::staff::Activity;
use topcoat::view::ViewExt;

/// Lines of recent staff activity the Overview shows.
const ACTIVITY_LINES: i64 = 8;

/// The start of today in Philippine time, as UTC.
fn start_of_today(now: OffsetDateTime) -> OffsetDateTime {
    local(now).replace_time(time::Time::MIDNIGHT)
}

/// A count and its word, kept on one line: `2 paused`.
fn part(count: i64, word: &str) -> String {
    format!("{count}\u{a0}{word}")
}

/// The small line under Active TINs: `2 paused · 1 test`, a zero left out.
fn active_breakdown(paused: i64, test: i64) -> Vec<(String, &'static str)> {
    [(paused, "paused"), (test, "test")].into_iter().filter(|(n, _)| *n > 0).map(|(n, w)| (part(n, w), "")).collect()
}

/// The small line under Returns sent today: failed in danger, waiting in
/// warning, a zero left out.
fn returns_breakdown(tile: &queries::ReturnsTile) -> Vec<(String, &'static str)> {
    [(tile.failed, "failed", "bad"), (tile.waiting, "waiting", "warn")]
        .into_iter()
        .filter(|(n, _, _)| *n > 0)
        .map(|(n, w, c)| (part(n, w), c))
        .collect()
}

/// `Saturday, November 14`, in Philippine time.
fn day_name(at: OffsetDateTime) -> String {
    let date = local(at).date();
    format!("{}, {} {}", date.weekday(), date.month(), date.day())
}

/// `8 AM`, `12 PM`.
fn hour_label(hour: i32) -> String {
    let half = if hour < 12 { "AM" } else { "PM" };
    let twelve = match hour % 12 {
        0 => 12,
        other => other,
    };
    format!("{twelve} {half}")
}

/// Returns sent per hour of today as small bars, the current hour in the
/// accent: the last eight hours up to now. Only numbers go into the SVG.
fn spark(by_hour: &[(i32, i64)], now_hour: i32) -> Unescaped<String> {
    let first = (now_hour - 7).max(0);
    let counts: Vec<i64> = (first..=now_hour)
        .map(|hour| by_hour.iter().find(|(at, _)| *at == hour).map_or(0, |(_, count)| *count))
        .collect();
    let max = counts.iter().copied().max().unwrap_or(0).max(1) as f64;
    let (width, height) = (96.0_f64, 30.0_f64);
    let step = width / counts.len() as f64;
    let mut bars = String::new();
    for (index, count) in counts.iter().enumerate() {
        let tall = (*count as f64 / max * height).max(1.5);
        let class = if index + 1 == counts.len() { " class=\"now\"" } else { "" };
        bars.push_str(&format!(
            r#"<rect{class} x="{:.1}" y="{:.1}" width="{:.1}" height="{tall:.1}" rx="1.5"/>"#,
            index as f64 * step + 1.5,
            height - tall,
            step - 3.0,
        ));
    }
    let (from, to) = (hour_label(first), hour_label(now_hour));
    Unescaped::new_unchecked(format!(
        r#"<svg class="sx-spark" viewBox="0 0 {width} {}" role="img" aria-label="Returns sent by hour, {from} to {to}">{bars}<text x="0" y="{}">{from}</text><text x="{width}" y="{}" text-anchor="end">{to}</text></svg>"#,
        height + 12.0,
        height + 10.0,
        height + 10.0,
    ))
}

/// One line of staff activity: (icon, what was done, to what or whom).
fn activity_words(line: &Activity) -> (&'static str, String, String) {
    let email = line.account_email.clone().unwrap_or_else(|| "a deleted account".to_owned());
    let tin = line.tin_masked.clone().unwrap_or_default();
    let (sym, verb, what) = match (line.kind.as_str(), line.action.as_str()) {
        ("claim", "approved") => ("check-circle", "approved", format!("{tin} for {email}")),
        ("claim", "rejected") => ("x-circle", "rejected a claim on", format!("{tin} by {email}")),
        ("claim", "sent_back") => ("undo", "sent back", format!("{tin} of {email}")),
        ("claim", "activated") => ("check-circle", "activated without papers", format!("{tin} for {email}")),
        ("claim", "auto_confirmed") => ("bolt", "confirmed the automatic activation of", format!("{tin} for {email}")),
        ("claim", "paused") => ("pause", "paused", format!("{tin} of {email}")),
        ("claim", "resumed") => ("play", "resumed", format!("{tin} of {email}")),
        ("claim", "revoked") => ("ban", "revoked", format!("{tin} from {email}")),
        ("claim", "deleted") => ("trash", "deleted a claim on", format!("{tin} by {email}")),
        ("claim", "removed") => ("x-circle", "removed", format!("{tin} from {email}")),
        // The seed tool saving a TIN without papers.
        ("claim", "created") => ("bookmark", "added", format!("{tin} to {email} without papers")),
        ("claim", _) => ("file", "changed", format!("{tin} of {email}")),
        (_, "banned") => ("ban", "banned", email),
        (_, "unbanned") => ("unlock", "lifted the ban on", email),
        (_, "role_changed") => {
            ("shield", "changed the role of", format!("{email} to {}", roles::role_label(line.new_value.as_deref())))
        }
        (_, "test_marked") => ("flask", "marked as a test account", email),
        (_, "test_unmarked") => ("flask", "removed the test label from", email),
        (_, "signed_out") => ("log-out", "signed out", format!("{email} everywhere")),
        (_, "app_signed_out") => ("log-out", "signed an app out of", email),
        (_, "created") => ("user-plus", "created a login for", email),
        (_, other) => ("file", "changed", format!("{email} ({})", other.replace('_', " "))),
    };
    (sym, verb.to_owned(), what)
}

#[page("/staff")]
pub async fn overview_page(cx: &Cx) -> Result<impl View> {
    let viewer = staff_member(cx, Power::Review).await?;
    // The queue's old address was `/staff?…`: anything that carries a query
    // (`?notice=`, `?status=`, `?open=`, `?q=`) is a queue link, kept alive.
    // Only after the staff check, so everyone else gets what they always did.
    if let Some(query) = uri(cx).query() {
        return Err(redirect(format!("/staff/queue?{query}")).into());
    }
    let db = &state(cx).db;
    let now = OffsetDateTime::now_utc();
    let numbers = queries::overview(db).await?;
    // Counts only, for Admin and Support alike: no TIN, name or account.
    let today = start_of_today(now);
    let now_hour = ((now - today).whole_hours()) as i32;
    let sent = queries::sent_since(db, today).await?;
    let week_start = today - time::Duration::days(6);
    let returns = queries::returns_tile(db, today, week_start).await?;
    let returns_parts = returns_breakdown(&returns);
    let active_parts = active_breakdown(numbers.paused, numbers.test);
    // The queue's own claims, for everyone who reviews.
    let longest = queries::waiting_longest(db, 3).await?;
    // What staff did to whom, across every account, is an Admin's: Support
    // see counts and the queue, and the query does not run for them.
    let admin = viewer.role == Role::Admin;
    let activity = if admin { queries::recent_activity(db, ACTIVITY_LINES).await? } else { Vec::new() };
    let lines: Vec<(&'static str, String, String, String, Uuid, OffsetDateTime)> = activity
        .iter()
        .map(|line| {
            let (sym, verb, what) = activity_words(line);
            let who = if line.source == "seed" {
                format!("{} (seed tool)", line.actor_email)
            } else {
                line.actor_email.clone()
            };
            (sym, who, verb, what, line.account_id, line.created_at)
        })
        .collect();
    let oldest = numbers.oldest_waiting.map(short_span);
    Ok(view! {
        <main id="main" class="sx-main" tabindex="-1">
            page_head(title: "Overview", sub: day_name(now))
            <div class="sx-notices">sx_notice(notice: notice(cx))</div>
            <div class="sx-scroll">
                <div class="sx-home">
                    <div class="sx-tiles">
                        <a class="sx-card sx-tile" href="/staff/queue?status=waiting">
                            <span class="l">(ic("inbox")) "Claims waiting"</span>
                            <span class="n">(numbers.waiting.to_string())</span>
                            if let Some(oldest) = oldest.clone() {
                                <span class="s hot">"Oldest " (oldest)</span>
                            } else {
                                <span class="s">"Nothing waiting"</span>
                            }
                        </a>
                        <div class="sx-card sx-tile">
                            <span class="l">(ic("check-circle")) "Active TINs"</span>
                            <span class="n">(numbers.active.to_string())</span>
                            if !active_parts.is_empty() {
                                <span class="s">
                                    for (index, (text, class)) in active_parts.into_iter().enumerate() {
                                        if index > 0 { " · " }
                                        <span class=(class)>(text)</span>
                                    }
                                </span>
                            }
                            <span class="s">(format!("+{} this week", numbers.active_week))</span>
                        </div>
                        <a class="sx-card sx-tile" href="/staff/users">
                            <span class="l">(ic("users")) "Accounts"</span>
                            <span class="n">(numbers.accounts.to_string())</span>
                            <span class="s">(format!("+{} this week", numbers.accounts_week))</span>
                        </a>
                        <div class="sx-card sx-tile">
                            <span class="l">(ic("send")) "Returns sent today"</span>
                            <span class="row">
                                <span class="n">(sent.iter().map(|(_, count)| count).sum::<i64>().to_string())</span>
                                (spark(&sent, now_hour))
                            </span>
                            if !returns_parts.is_empty() {
                                <span class="s">
                                    for (index, (text, class)) in returns_parts.into_iter().enumerate() {
                                        if index > 0 { " · " }
                                        <span class=(class)>(text)</span>
                                    }
                                </span>
                            }
                            <span class="s">(format!("{} in the last 7 days", returns.sent_week))</span>
                        </div>
                        <a class="sx-card sx-tile" href="/staff/queue?status=auto">
                            <span class="l">(ic("bolt")) "Activated automatically"</span>
                            <span class="n">(numbers.auto.to_string())</span>
                            <span class="s">"For a person to check"</span>
                        </a>
                    </div>
                    <div class=(if admin { "sx-home2" } else { "sx-home2 one" })>
                        <section class="sx-card sx-sec" aria-labelledby="sx-longest">
                            <h2 id="sx-longest">"Waiting longest"</h2>
                            if longest.is_empty() {
                                <p class="sx-empty">"Nothing waiting. All caught up."</p>
                            } else {
                                <div class="sx-lis">
                                    for claim in longest {
                                        let since = claim.waiting_since();
                                        let late = now - since > time::Duration::days(1);
                                        <a class="sx-li" href=(format!("/staff/queue?status=waiting&open={}", claim.id))>
                                            kind_icon(kind: claim.kind.clone())
                                            <span class="x">
                                                <b class="sx-mono">(claim.tin().to_string())</b>
                                                <span>(claim.registered_name.clone())</span>
                                            </span>
                                            <span class=(if late { "sx-age late" } else { "sx-age" })>
                                                (ic("clock"))
                                                (short_span(since))
                                            </span>
                                        </a>
                                    }
                                </div>
                            }
                            <a class="sx-btn" href="/staff/queue?status=waiting">(ic("arrow-right")) "Open the queue"</a>
                        </section>
                        if admin {
                            <section class="sx-card sx-sec" aria-labelledby="sx-activity">
                                <h2 id="sx-activity">"Recent staff activity"</h2>
                                if lines.is_empty() {
                                    <p class="sx-empty">"No staff actions yet."</p>
                                } else {
                                    <ol class="sx-feed">
                                        for (sym, who, verb, what, account_id, at) in lines {
                                            <li>
                                                <span class="sx-feed-ic">(ic(sym))</span>
                                                <span class="x">
                                                    <b>(who)</b> " " (verb) " "
                                                    <a class="w" href=(format!("/staff/users/{account_id}"))>(what)</a>
                                                    <span class="t">(ago(Some(at)))</span>
                                                </span>
                                            </li>
                                        }
                                    </ol>
                                }
                            </section>
                        }
                    </div>
                </div>
            </div>
        </main>
    }
    .boxed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn today_starts_at_midnight_in_manila() {
        // 2026-11-14 02:00 UTC is 10:00 in Manila; the day began at 16:00 UTC.
        let now = OffsetDateTime::from_unix_timestamp(1_794_621_600).unwrap();
        let start = start_of_today(now);
        assert_eq!(start.to_offset(time::UtcOffset::UTC).hour(), 16);
        assert_eq!((now - start).whole_hours(), 10);
        assert_eq!(hour_label(0), "12 AM");
        assert_eq!(hour_label(15), "3 PM");
    }

    /// Every activity line's icon is in the sprite, and none reads "changed"
    /// for an action the feed knows.
    #[test]
    fn every_activity_line_has_its_icon_and_words() {
        let sprite = include_str!("../../staff-icons.svg");
        let claim = [
            "approved",
            "rejected",
            "sent_back",
            "activated",
            "auto_confirmed",
            "paused",
            "resumed",
            "revoked",
            "deleted",
            "removed",
            "created",
            "file_replaced",
        ];
        let account = [
            "banned",
            "unbanned",
            "role_changed",
            "test_marked",
            "test_unmarked",
            "signed_out",
            "app_signed_out",
            "created",
        ];
        let lines =
            claim.iter().map(|action| ("claim", *action)).chain(account.iter().map(|action| ("account", *action)));
        for (kind, action) in lines {
            let line = Activity {
                kind: kind.to_owned(),
                action: action.to_owned(),
                tin_masked: Some("***-***-451-00000".to_owned()),
                old_value: None,
                new_value: Some("support".to_owned()),
                account_id: Uuid::nil(),
                account_email: Some("ana@example.test".to_owned()),
                actor_email: "staff@example.test".to_owned(),
                source: "seed".to_owned(),
                created_at: OffsetDateTime::UNIX_EPOCH,
            };
            let (sym, verb, what) = activity_words(&line);
            assert!(sprite.contains(&format!("<symbol id=\"{sym}\"")), "{kind} {action}: {sym}");
            if action != "file_replaced" {
                assert_ne!(verb, "changed", "{kind} {action}");
            }
            assert!(what.contains("ana@example.test"), "{kind} {action}: {what}");
        }
        let created = Activity {
            kind: "claim".to_owned(),
            action: "created".to_owned(),
            tin_masked: Some("***-***-451-00000".to_owned()),
            old_value: None,
            new_value: None,
            account_id: Uuid::nil(),
            account_email: Some("ana@example.test".to_owned()),
            actor_email: "staff@example.test".to_owned(),
            source: "seed".to_owned(),
            created_at: OffsetDateTime::UNIX_EPOCH,
        };
        let (_, verb, what) = activity_words(&created);
        assert_eq!(format!("{verb} {what}"), "added ***-***-451-00000 to ana@example.test without papers");
    }

    #[test]
    fn the_spark_shows_the_last_eight_hours() {
        let svg = spark(&[(2, 1), (9, 4), (10, 2)], 10).to_string();
        assert_eq!(svg.matches("<rect").count(), 8);
        assert!(svg.contains("3 AM to 10 AM"), "{svg}");
        assert_eq!(svg.matches("class=\"now\"").count(), 1);
    }
}
