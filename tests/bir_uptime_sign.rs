//! BIR uptime's sign with its database: tries recorded through the
//! recorder move it, every change leaves an event, the sign survives a
//! restart, notes and the relay resume are Admin-only and audited, the day
//! history adds up across midnight and a restart, and staff mail is
//! batched, capped and carries no personal data. Made-up data, a made-up
//! clock, and the local test database only: nothing else is connected to.

mod common;

use buwiz_cloud::account::accounts::Account;
use buwiz_cloud::account::mail::Mailer;
use buwiz_cloud::account::roles::{self, Role, Staff};
use buwiz_cloud::uptime::alerts::{self, Alert, Budget};
use buwiz_cloud::uptime::days;
use buwiz_cloud::uptime::keys::{Network, manila_day};
use buwiz_cloud::uptime::live;
use buwiz_cloud::uptime::notes::{self, NewNote, NoteError};
use buwiz_cloud::uptime::observe::{Observed, TryObserver};
use buwiz_cloud::uptime::sign::{Light, NoteKind};
use buwiz_cloud::uptime::store::{self, RelayTry};
use buwiz_cloud::uptime::{Level, Uptime};
use common::{STAFF, Site};
use http::HeaderMap;
use sqlx::PgPool;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// 10:00 in Manila, 11 November 2026.
fn t0() -> OffsetDateTime {
    OffsetDateTime::parse("2026-11-11T02:00:00Z", &Rfc3339).unwrap()
}

fn t(minutes: i64) -> OffsetDateTime {
    t0() + Duration::minutes(minutes)
}

fn uptime(now: OffsetDateTime) -> Uptime {
    Uptime::new(Level::Record, vec![22], false, now)
}

const SENDER_LIVE: &str = r#"{"type":"sender","mode":"live"}"#;
const DESK: &str = r#"{"type":"open","stream":1,"host":"dispatcher.test","port":443}"#;
const MAILBOX: &str = r#"{"type":"open","stream":2,"host":"upload.test","port":22}"#;

/// How a made-up try ends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ends {
    Landed,
    MailboxRefused,
    HostKey,
    OwnNetwork,
}

fn observed(ends: Ends) -> Observed {
    let mut observer = TryObserver::new(&[22]);
    observer.sender_text(SENDER_LIVE);
    observer.sender_text(DESK);
    if ends == Ends::OwnNetwork {
        observer.app_text(r#"{"type":"refused","stream":1,"reason":"made-up","kind":"denied"}"#);
        return observer.finish("failed", Some("BIR's dispatcher did not answer: https://dispatcher.test: refused"));
    }
    observer.app_text(r#"{"type":"opened","stream":1}"#);
    observer.app_binary(&[1, b'H', b'T', b'T', b'P']);
    observer.sender_text(MAILBOX);
    match ends {
        Ends::Landed => {
            observer.app_text(r#"{"type":"opened","stream":2}"#);
            observer.app_binary(&[2, b'S', b'S', b'H']);
            observer.finish("sent", None)
        }
        Ends::MailboxRefused => {
            observer.app_text(r#"{"type":"refused","stream":2,"reason":"Connection refused (os error 61)"}"#);
            observer.finish("failed", Some("could not reach BIR's upload server: refused"))
        }
        Ends::HostKey => {
            observer.app_text(r#"{"type":"opened","stream":2}"#);
            observer.app_binary(&[2, b'S', b'S', b'H']);
            observer.finish("failed", Some("SSH failed: the host key is not pinned"))
        }
        Ends::OwnNetwork => unreachable!(),
    }
}

/// Runs the clock up to `until` as the ticker does (more often than every
/// 5 minutes), so a test's jumps are not taken for Buwiz not watching.
async fn watch(uptime: &Uptime, db: &PgPool, until: OffsetDateTime) {
    loop {
        let next = uptime.live.sign().evaluated_at + Duration::minutes(4);
        if next >= until {
            return;
        }
        live::tick(uptime, db, next).await.unwrap();
    }
}

/// Records a try by made-up account `account` from made-up network
/// `198.51.<network>.0/24`, ending at `at`, as the bridge's recorder does.
async fn record(uptime: &Uptime, db: &PgPool, account: u128, network: u8, ends: Ends, at: OffsetDateTime) {
    let relay = RelayTry {
        account_id: Uuid::from_u128(account),
        network: Network::V4([198, 51, network]),
        client: None,
        admission: "open".into(),
        started_at: at - Duration::seconds(20),
    };
    watch(uptime, db, at).await;
    store::record(uptime, db, &relay, &observed(ends), at).await;
}

/// (scope, from, to, cause, reason, actor email), oldest first.
async fn events(db: &PgPool) -> Vec<(String, String, String, String, String, Option<String>)> {
    sqlx::query_as("SELECT scope, from_state, to_state, cause, reason, actor_email FROM bir_events ORDER BY id")
        .fetch_all(db)
        .await
        .unwrap()
}

async fn stored_seq(db: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT seq FROM bir_sign").fetch_one(db).await.unwrap()
}

/// Online with landings from three accounts, then the mailbox refuses
/// three accounts on three networks: Offline.
async fn an_outage(uptime: &Uptime, db: &PgPool) {
    for account in 1..=3 {
        record(uptime, db, account, account as u8, Ends::Landed, t(0)).await;
    }
    for (offset, account) in (11..=13).enumerate() {
        record(uptime, db, account, account as u8, Ends::MailboxRefused, t(10 + offset as i64)).await;
    }
}

#[sqlx::test]
async fn recorded_tries_move_the_sign_and_every_change_is_an_event(db: PgPool) {
    let uptime = uptime(t(0));
    an_outage(&uptime, &db).await;
    let sign = uptime.live.sign();
    assert_eq!(sign.state, Light::Offline);
    assert_eq!(sign.upload.state, Light::Offline);
    assert_eq!(sign.dispatcher.state, Light::Online);
    assert_eq!(sign.since, t(12));
    let lines: Vec<(String, String, String, String)> =
        events(&db).await.into_iter().map(|(scope, from, to, cause, ..)| (scope, from, to, cause)).collect();
    let expect = |scope: &str, from: &str, to: &str, cause: &str| (scope.into(), from.into(), to.into(), cause.into());
    assert_eq!(
        lines,
        [
            expect("overall", "none", "no_recent_news", "rule"),
            expect("dispatcher", "no_recent_news", "online", "rule"),
            expect("upload", "no_recent_news", "online", "rule"),
            expect("overall", "no_recent_news", "online", "rule"),
            expect("upload", "online", "busy", "rule"),
            expect("overall", "online", "busy", "rule"),
            expect("upload", "busy", "offline", "rule"),
            expect("overall", "busy", "offline", "rule"),
        ]
    );
    // Counts only: no key, no id, no address.
    for (.., reason, actor) in events(&db).await {
        assert!(actor.is_none());
        assert!(!reason.contains("198.51") && !reason.contains("0000"), "{reason}");
    }
    assert_eq!(stored_seq(&db).await, sign.seq);
    // The first line, then Online, Busy and Offline: one more each time, and
    // never below the change's 5-minute mark in Unix seconds.
    let mark = t(10).unix_timestamp();
    assert_eq!(sign.seq, mark + 1, "Busy at 10:11 moves to the 10:10 mark; Offline one more");

    // The user's own network moves nothing.
    for account in 21..=26 {
        record(&uptime, &db, account, account as u8, Ends::OwnNetwork, t(14)).await;
    }
    assert_eq!(uptime.live.sign().seq, sign.seq);
    assert_eq!(uptime.live.sign().upload.state, Light::Offline);

    // A landing ends it at once, and waking starts.
    record(&uptime, &db, 4, 4, Ends::Landed, t(20)).await;
    let sign = uptime.live.sign();
    assert_eq!(sign.state, Light::Online);
    assert_eq!(sign.waking_since, Some(t(20)));
    assert_eq!(events(&db).await.len(), 10);
}

/// No try is lost on the way to the rules: with the database gone, a try's
/// row and its run both fail, but the try waits in memory, and the next
/// run that can write makes the change it called for.
#[sqlx::test]
async fn a_try_whose_row_and_run_fail_still_reaches_the_sign(db: PgPool) {
    let uptime = uptime(t(0));
    for account in 1..=3 {
        record(&uptime, &db, account, account as u8, Ends::Landed, t(0)).await;
    }
    let gone = sqlx::postgres::PgPoolOptions::new().connect_with((*db.connect_options()).clone()).await.unwrap();
    gone.close().await;
    for (offset, account) in (11..=13).enumerate() {
        record(&uptime, &gone, account, account as u8, Ends::MailboxRefused, t(2 + offset as i64)).await;
    }
    // Nothing could be written, but the sign shows what the tries say (it
    // keeps aging in memory while the database is down) ...
    assert_eq!(uptime.live.sign().state, Light::Offline);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM bir_tries").fetch_one(&db).await.unwrap();
    assert_eq!(rows, 3, "their rows are lost");
    assert_ne!(events(&db).await.pop().unwrap().2, "offline", "not written yet");
    // ... and the next run that can write writes the change.
    live::tick(&uptime, &db, t(5)).await.unwrap();
    assert_eq!(uptime.live.sign().state, Light::Offline);
    assert_eq!(events(&db).await.pop().unwrap().2, "offline");
    assert_eq!(stored_seq(&db).await, uptime.live.sign().seq);
}

/// What a restart must keep of a light: everything but the account ids,
/// which live in memory only.
fn kept(lamp: &buwiz_cloud::uptime::sign::Lamp) -> impl PartialEq + std::fmt::Debug {
    let quorum: Vec<_> = lamp.quorum.iter().map(|voter| (voter.keys.clone(), voter.at)).collect();
    (lamp.state, lamp.since, lamp.last_state, lamp.last_ok_at, lamp.last_evidence_at, lamp.fresh_from, quorum)
}

#[sqlx::test]
async fn the_sign_survives_a_restart_in_every_state(db: PgPool) {
    let first = uptime(t(0));
    an_outage(&first, &db).await;
    let before = first.live.sign();
    let lines = events(&db).await.len();

    // Another process (a restart): new day keys, the sign read back.
    let second = uptime(t(13));
    live::tick(&second, &db, t(13)).await.unwrap();
    let after = second.live.sign();
    assert_eq!((after.state, after.since), (before.state, before.since));
    assert!(after.seq > before.seq, "every start moves seq on once");
    assert_eq!(kept(&after.upload), kept(&before.upload));
    assert_eq!(kept(&after.dispatcher), kept(&before.dispatcher));
    assert_eq!(events(&db).await.len(), lines, "nothing changed by the restart");
    // Offline holds until a landing.
    record(&second, &db, 11, 11, Ends::MailboxRefused, t(14)).await;
    assert_eq!(second.live.sign().state, Light::Offline);
    record(&second, &db, 5, 5, Ends::Landed, t(15)).await;
    assert_eq!(second.live.sign().state, Light::Online);
    assert_eq!(second.live.sign().seq, t(15).unix_timestamp(), "the landing at 10:15, a new mark");

    // Busy, with its dwell, across a restart.
    let third = uptime(t(16));
    for account in [31, 32] {
        record(&third, &db, account, account as u8, Ends::MailboxRefused, t(16)).await;
    }
    assert_eq!(third.live.sign().state, Light::Busy);
    let busy_since = third.live.sign().upload.since;
    let fourth = uptime(t(17));
    for account in 40..=49 {
        record(&fourth, &db, account, account as u8, Ends::Landed, t(17)).await;
    }
    assert_eq!(fourth.live.sign().upload.since, busy_since);
    assert_eq!(fourth.live.sign().state, Light::Busy, "the dwell kept across the restart");
    live::tick(&fourth, &db, busy_since + Duration::minutes(10)).await.unwrap();
    assert_eq!(fourth.live.sign().state, Light::Online);

    // Quiet, across a restart, and the seq only grows.
    let fifth = uptime(t(200));
    live::tick(&fifth, &db, t(200)).await.unwrap();
    assert_eq!(fifth.live.sign().state, Light::NoRecentNews);
    let seq = fifth.live.sign().seq;
    let sixth = uptime(t(201));
    live::tick(&sixth, &db, t(201)).await.unwrap();
    assert!(sixth.live.sign().seq > seq);
    assert_eq!(stored_seq(&db).await, sixth.live.sign().seq);
}

/// Tries from before a restart cannot be compared with tries after it (the
/// keys lived in memory): two fresh failures after it are no quorum with
/// the two before, though all four are different accounts on different
/// networks.
#[sqlx::test]
async fn fresh_failures_after_a_restart_are_no_quorum_with_older_ones(db: PgPool) {
    let first = uptime(t(0));
    for account in 1..=3 {
        record(&first, &db, account, account as u8, Ends::Landed, t(0)).await;
    }
    record(&first, &db, 21, 21, Ends::MailboxRefused, t(5)).await;
    record(&first, &db, 22, 22, Ends::MailboxRefused, t(6)).await;
    assert_eq!(first.live.sign().upload.state, Light::Busy);

    let second = uptime(t(7));
    record(&second, &db, 23, 23, Ends::MailboxRefused, t(7)).await;
    record(&second, &db, 24, 24, Ends::MailboxRefused, t(8)).await;
    assert_eq!(second.live.sign().upload.state, Light::Busy, "two after the restart, not four");
    // A third fresh one makes the quorum.
    record(&second, &db, 25, 25, Ends::MailboxRefused, t(9)).await;
    assert_eq!(second.live.sign().upload.state, Light::Offline);
}

#[sqlx::test]
async fn a_stored_sign_that_cannot_be_read_starts_again_with_a_higher_seq(db: PgPool) {
    sqlx::query("INSERT INTO bir_sign (id, seq, state, updated_at) VALUES (1, 41, $1, $2)")
        .bind(serde_json::json!({
            "state": "glowing",
            "note": { "id": "00000000-0000-0000-0000-000000000abc", "kind": "outage", "text": "Made-up",
                      "starts_at": "2026-11-11T01:00:00Z", "until": "2026-11-11T05:00:00Z" }
        }))
        .bind(t(-30))
        .execute(&db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO bir_notes (id, kind, text, starts_at, until, set_by, set_by_email)
         VALUES ('00000000-0000-0000-0000-000000000abc', 'outage', 'Made-up', $1, $2, gen_random_uuid(), 'admin@example.test')",
    )
    .bind(t(-60))
    .bind(t(180))
    .execute(&db)
    .await
    .unwrap();
    let uptime = uptime(t(0));
    live::tick(&uptime, &db, t(0)).await.unwrap();
    let sign = uptime.live.sign();
    assert_eq!(sign.seq, t(0).unix_timestamp() + 1, "the reset at the start's mark, then the gap since its last write");
    assert_eq!((sign.upload.state, sign.dispatcher.state), (Light::NoRecentNews, Light::NoRecentNews));
    assert_eq!(sign.state, Light::Offline, "the outage note in force");
    assert_eq!(
        sign.note.as_ref().map(|note| note.text.as_str()),
        Some("Made-up"),
        "the note is kept: no new note event"
    );
    let lines: Vec<(String, String, String, String)> =
        events(&db).await.into_iter().map(|(scope, from, to, cause, ..)| (scope, from, to, cause)).collect();
    let expect = |scope: &str, from: &str, to: &str, cause: &str| (scope.into(), from.into(), to.into(), cause.into());
    assert_eq!(
        lines,
        [
            expect("overall", "unknown", "offline", "reset"),
            expect("watch", "watched", "not_watched", "not_watched"),
            expect("watch", "not_watched", "watched", "not_watched"),
        ]
    );
    record(&uptime, &db, 1, 1, Ends::Landed, t(1)).await;
    assert_eq!(stored_seq(&db).await, sign.seq + 1);
}

/// A stored sign written by another release (a field missing, one more)
/// reads as it is, and the tries read back keep the times of news, so the
/// light neither resets nor flaps between Online and No recent news as the
/// clock runs on.
#[sqlx::test]
async fn a_sign_from_another_release_and_its_tries_load_without_flapping(db: PgPool) {
    let first = uptime(t(0));
    for account in 1..=3 {
        record(&first, &db, account, account as u8, Ends::Landed, t(0)).await;
    }
    watch(&first, &db, t(10)).await;
    sqlx::query(
        "UPDATE bir_sign SET state = jsonb_set(state - 'revision' - 'vote_losses' - 'streak_told',
                                              '{a_later_field}', '\"made-up\"')",
    )
    .execute(&db)
    .await
    .unwrap();
    let before = first.live.sign();
    let lines = events(&db).await.len();
    let second = uptime(t(10));
    let mut seen = Vec::new();
    for minute in (10..=90).step_by(2) {
        live::tick(&second, &db, t(minute)).await.unwrap();
        seen.push((minute, second.live.sign().state));
    }
    let sign = second.live.sign();
    assert_eq!(sign.since, t(60));
    assert_eq!(
        (sign.upload.last_ok_at, sign.upload.last_evidence_at),
        (before.upload.last_ok_at, before.upload.last_evidence_at)
    );
    let changes: Vec<(i64, Light)> =
        seen.windows(2).filter(|pair| pair[0].1 != pair[1].1).map(|pair| pair[1]).collect();
    assert_eq!(changes, [(60, Light::NoRecentNews)], "{seen:?}");
    assert_eq!(events(&db).await.len(), lines + 3, "the quiet hour only: no reset, no gap");
}

/// An event is written once: a run written again (its commit's
/// acknowledgement lost) adds nothing twice. The times of news alone are
/// written at most once a minute.
#[sqlx::test]
async fn writes_are_idempotent_and_times_alone_wait_a_minute(db: PgPool) {
    let uptime = uptime(t(0));
    record(&uptime, &db, 1, 1, Ends::Landed, t(0)).await;
    let lines = events(&db).await.len();
    // A run with changes, written, then written again from the same sign.
    let mut locked = live::lock(&uptime, &db, t(1)).await.unwrap();
    let note = buwiz_cloud::uptime::sign::Note {
        id: Uuid::from_u128(0xabc),
        kind: NoteKind::Outage,
        text: "Made-up".into(),
        starts_at: t(1),
        until: t(30),
    };
    let evaluation = locked.evaluate(Some(&note), false);
    assert_eq!(evaluation.events.len(), 1);
    for _ in 0..2 {
        let mut tx = db.begin().await.unwrap();
        locked.write(&mut tx, &evaluation, None).await.unwrap();
        tx.commit().await.unwrap();
    }
    drop(locked);
    assert_eq!(events(&db).await.len(), lines + 1);

    // A landing 40 seconds after a write moves only the times of news: not
    // written until a minute has passed.
    let other = Uptime::new(Level::Record, vec![22], false, t(2));
    live::tick(&other, &db, t(2)).await.unwrap();
    let updated = || async {
        sqlx::query_scalar::<_, OffsetDateTime>("SELECT updated_at FROM bir_sign").fetch_one(&db).await.unwrap()
    };
    assert_eq!(updated().await, t(2));
    record(&other, &db, 2, 2, Ends::Landed, t(2) + Duration::seconds(40)).await;
    assert_eq!(updated().await, t(2));
    live::tick(&other, &db, t(3)).await.unwrap();
    assert_eq!(updated().await, t(3));
}

#[sqlx::test]
async fn faults_that_touch_the_bir_are_told_and_nothing_pauses(db: PgPool) {
    let site = Site::new(db.clone());
    let uptime = uptime(t(0));
    record(&uptime, &db, 1, 1, Ends::HostKey, t(1)).await;
    record(&uptime, &db, 1, 1, Ends::HostKey, t(2)).await;
    record(&uptime, &db, 2, 2, Ends::HostKey, t(3)).await;
    let sign = uptime.live.sign();
    assert!(!sign.relay.paused, "nothing pauses in this release");
    assert_eq!(sign.state, Light::NoRecentNews, "a fault of Buwiz's is no evidence about the BIR");
    assert!(events(&db).await.iter().all(|(scope, ..)| scope != "relay"));
    // Staff hear of it, once.
    let mailer = Mailer::recording("http://localhost:3008");
    let mail = alerts::flush(&uptime, &db, &mailer, &site.state.config, t(3)).await.unwrap().unwrap();
    assert!(mail.text.contains("The relay would pause: 3 tries from 2 accounts on 2 networks"), "{}", mail.text);
    record(&uptime, &db, 3, 3, Ends::HostKey, t(14)).await;
    let mail = alerts::flush(&uptime, &db, &mailer, &site.state.config, t(14)).await.unwrap();
    assert!(mail.is_none_or(|mail| !mail.text.contains("would pause")), "at most once a day");
    // A later release never starts up paused from what this one wrote.
    let stored: serde_json::Value = sqlx::query_scalar("SELECT state FROM bir_sign").fetch_one(&db).await.unwrap();
    assert_eq!(stored["relay"]["paused"], false);
    let restarted = Uptime::new(Level::Record, vec![22], false, t(4));
    live::tick(&restarted, &db, t(4)).await.unwrap();
    assert!(!restarted.live.sign().relay.paused);

    // Resuming is the later staff page's, Admin-only; nothing is paused.
    let (support, _) = site.staff_with_role("support@example.test", "support").await;
    let support = staff(&site, support, Role::Support).await;
    let refused = notes::resume_relay(&restarted, &db, &site.state.config, &support, t(5)).await;
    assert!(matches!(refused, Err(NoteError::AdminOnly)));
    let admin = admin(&site).await;
    assert!(matches!(
        notes::resume_relay(&restarted, &db, &site.state.config, &admin, t(6)).await,
        Err(NoteError::NotPaused)
    ));
}

async fn staff(site: &Site, id: Uuid, role: Role) -> Staff {
    let email: String =
        sqlx::query_scalar("SELECT email FROM accounts WHERE id = $1").bind(id).fetch_one(&site.db).await.unwrap();
    Staff { account: Account { id, email }, role, from_server: false }
}

/// The Admin through `ADMIN_EMAILS`.
async fn admin(site: &Site) -> Staff {
    let (id, _) = site.person(STAFF).await;
    let account = Account { id, email: STAFF.into() };
    roles::staff(&site.db, &site.state.config, &account).await.unwrap().unwrap()
}

/// (kind, text, cleared_how, set_by_email, cleared_by_email), oldest first.
async fn note_rows(db: &PgPool) -> Vec<(String, String, Option<String>, String, Option<String>)> {
    sqlx::query_as("SELECT kind, text, cleared_how, set_by_email, cleared_by_email FROM bir_notes ORDER BY starts_at")
        .fetch_all(db)
        .await
        .unwrap()
}

#[sqlx::test]
async fn only_an_admin_sets_or_clears_a_note_and_each_is_audited(db: PgPool) {
    let site = Site::new(db.clone());
    let config = &site.state.config;
    let uptime = uptime(t(0));
    let outage = NewNote { kind: NoteKind::Outage, text: " The BIR announced an outage. ", until: t(120) };

    // Support, and someone with no role who claims to be an Admin.
    let (support, _) = site.staff_with_role("support@example.test", "support").await;
    let support = staff(&site, support, Role::Support).await;
    let (nobody, _) = site.person("filer@example.test").await;
    let forged = staff(&site, nobody, Role::Admin).await;
    // An Admin by stored role, banned since the page was loaded.
    let (banned, _) = site.staff_with_role("banned-admin@example.test", "admin").await;
    sqlx::query("UPDATE accounts SET banned_at = now(), banned_by = id, ban_reason = 'Made-up' WHERE id = $1")
        .bind(banned)
        .execute(&db)
        .await
        .unwrap();
    let banned = staff(&site, banned, Role::Admin).await;
    for actor in [&support, &forged, &banned] {
        assert!(matches!(notes::set(&uptime, &db, config, actor, outage, t(1)).await, Err(NoteError::AdminOnly)));
        assert!(matches!(
            notes::clear(&uptime, &db, config, actor, "Made-up reason", t(1)).await,
            Err(NoteError::AdminOnly)
        ));
    }
    assert!(note_rows(&db).await.is_empty());

    // An Admin, by stored role.
    let (admin_id, _) = site.staff_with_role("admin@example.test", "admin").await;
    let admin = staff(&site, admin_id, Role::Admin).await;
    for (bad, error) in [
        (NewNote { text: "  ", ..outage }, "TextMissing"),
        (NewNote { text: &"x".repeat(201), ..outage }, "TextTooLong"),
        (NewNote { until: t(1), ..outage }, "EndsInThePast"),
        (NewNote { until: t(1) + Duration::hours(72) + Duration::seconds(1), ..outage }, "EndsTooFar"),
    ] {
        let got = notes::set(&uptime, &db, config, &admin, bad, t(1)).await.unwrap_err();
        assert_eq!(format!("{got:?}"), error);
    }
    let note = notes::set(&uptime, &db, config, &admin, NewNote { until: t(1) + Duration::hours(72), ..outage }, t(1))
        .await
        .unwrap();
    assert_eq!(note.text, "The BIR announced an outage.");
    let sign = uptime.live.sign();
    assert_eq!(sign.state, Light::Offline);
    assert_eq!(sign.note.as_ref().map(|note| note.kind), Some(NoteKind::Outage));
    assert_eq!(sign.source(), "staff");
    let audit = events(&db).await.pop().unwrap();
    assert_eq!(
        (audit.0.as_str(), audit.1.as_str(), audit.2.as_str(), audit.3.as_str(), audit.5.as_deref()),
        ("overall", "no_recent_news", "offline", "note_set", Some("admin@example.test"))
    );
    let actor: Option<Uuid> =
        sqlx::query_scalar("SELECT actor_id FROM bir_events ORDER BY id DESC LIMIT 1").fetch_one(&db).await.unwrap();
    assert_eq!(actor, Some(admin_id));

    // One note at a time.
    let again = NewNote { kind: NoteKind::Maintenance, text: "Planned", until: t(60) };
    assert!(matches!(notes::set(&uptime, &db, config, &admin, again, t(2)).await, Err(NoteError::NoteInForce)));

    // Cleared by an Admin (here the one through ADMIN_EMAILS): audited,
    // and waking starts.
    let other = self::admin(&site).await;
    assert!(matches!(notes::clear(&uptime, &db, config, &other, " ", t(3)).await, Err(NoteError::TextMissing)));
    notes::clear(&uptime, &db, config, &other, "The BIR says it is back", t(3)).await.unwrap();
    let audit = events(&db).await.pop().unwrap();
    assert_eq!((audit.3.as_str(), audit.5.as_deref()), ("note_cleared", Some(STAFF)));
    assert_eq!(audit.4, "An Admin cleared the note: The BIR says it is back");
    let why: Option<String> = sqlx::query_scalar("SELECT cleared_reason FROM bir_notes").fetch_one(&db).await.unwrap();
    assert_eq!(why.as_deref(), Some("The BIR says it is back"));
    assert_eq!(uptime.live.sign().waking_since, Some(t(3)));
    assert_eq!(uptime.live.sign().note, None);
    assert!(matches!(notes::clear(&uptime, &db, config, &admin, "Again", t(4)).await, Err(NoteError::NoNote)));
    assert_eq!(
        note_rows(&db).await,
        [(
            "outage".into(),
            "The BIR announced an outage.".into(),
            Some("staff".into()),
            "admin@example.test".into(),
            Some(STAFF.into())
        )]
    );

    // Declare Online: an audited note of its own, the sign Online even
    // while evidence says Offline; the evidence is still read.
    an_outage(&uptime, &db).await;
    assert_eq!(uptime.live.sign().state, Light::Offline);
    notes::declare_online(&uptime, &db, config, &admin, "Ignore the evidence: made-up reason", t(40), t(14))
        .await
        .unwrap();
    assert_eq!(uptime.live.sign().state, Light::Online);
    assert_eq!(uptime.live.sign().upload.state, Light::Offline);
    let audit = events(&db).await.pop().unwrap();
    assert_eq!((audit.3.as_str(), audit.5.as_deref()), ("note_set", Some("admin@example.test")));
    assert_eq!(audit.4, "An Admin declared the BIR online, ignoring evidence: Ignore the evidence: made-up reason");
}

/// The role is checked before the engine's lock (nobody else waits on it)
/// and again in the transaction: an Admin demoted while their request
/// waited does nothing. The email written is the one the database has then.
#[sqlx::test]
async fn an_admin_demoted_mid_request_does_nothing(db: PgPool) {
    let site = Site::new(db.clone());
    let config = &site.state.config;
    let uptime = uptime(t(0));
    let (admin_id, _) = site.staff_with_role("admin@example.test", "admin").await;
    let admin = staff(&site, admin_id, Role::Admin).await;
    let (support, _) = site.staff_with_role("support@example.test", "support").await;
    let support = staff(&site, support, Role::Support).await;
    let outage = NewNote { kind: NoteKind::Outage, text: "From the news", until: t(60) };

    // The engine is busy: Support is refused at once, without waiting.
    let held = live::lock(&uptime, &db, t(1)).await.unwrap();
    let refused = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        notes::set(&uptime, &db, config, &support, outage, t(1)),
    )
    .await
    .expect("refused without waiting for the engine");
    assert!(matches!(refused, Err(NoteError::AdminOnly)));

    // The Admin passes the first check and waits; meanwhile they are
    // demoted. Inside the transaction they are refused.
    let request = notes::set(&uptime, &db, config, &admin, outage, t(1));
    let demote = async {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        sqlx::query("UPDATE accounts SET role = NULL WHERE id = $1").bind(admin_id).execute(&db).await.unwrap();
        drop(held);
    };
    let (got, ()) = tokio::join!(request, demote);
    assert!(matches!(got, Err(NoteError::AdminOnly)), "{got:?}");
    assert!(note_rows(&db).await.is_empty());

    // Promoted again under a new address: the address written is today's.
    sqlx::query("UPDATE accounts SET role = 'admin', email = 'renamed-admin@example.test' WHERE id = $1")
        .bind(admin_id)
        .execute(&db)
        .await
        .unwrap();
    notes::set(&uptime, &db, config, &admin, outage, t(2)).await.unwrap();
    assert_eq!(note_rows(&db).await[0].3, "renamed-admin@example.test");
    assert_eq!(events(&db).await.pop().unwrap().5.as_deref(), Some("renamed-admin@example.test"));
}

#[sqlx::test]
async fn an_expired_note_never_blocks_a_new_one(db: PgPool) {
    let site = Site::new(db.clone());
    let config = &site.state.config;
    let uptime = uptime(t(0));
    let admin = admin(&site).await;
    let short = NewNote { kind: NoteKind::Maintenance, text: "Planned work", until: t(30) };
    notes::set(&uptime, &db, config, &admin, short, t(0)).await.unwrap();
    // The clock runs it out: an event, the row marked, waking starts.
    live::tick(&uptime, &db, t(30)).await.unwrap();
    let ended = events(&db).await.pop().unwrap();
    assert_eq!((ended.3.as_str(), ended.5), ("note_expired", None));
    assert_eq!(note_rows(&db).await[0].2.as_deref(), Some("expired"));
    assert_eq!(uptime.live.sign().waking_since, Some(t(30)));
    notes::set(&uptime, &db, config, &admin, NewNote { until: t(90), ..short }, t(31)).await.unwrap();

    // A note that ran out while no tick saw it (another process): marked
    // ended by the next set, which goes through.
    let other = Uptime::new(Level::Record, vec![22], false, t(91));
    notes::set(&other, &db, config, &admin, NewNote { until: t(150), ..short }, t(95)).await.unwrap();
    let rows = note_rows(&db).await;
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1].2.as_deref(), Some("expired"));
    assert_eq!(rows[2].2, None);
    assert_eq!(other.live.sign().state, Light::Offline);
    // Its end has its line in the log, before the new note's.
    let last: Vec<(String, String)> =
        events(&db).await.into_iter().rev().take(2).map(|(.., cause, reason, _)| (cause, reason)).collect();
    assert_eq!(
        last,
        [
            ("note_set".into(), "An Admin set a planned maintenance note".into()),
            ("note_expired".into(), "A note ran out before Buwiz saw it end".into())
        ]
    );
}

#[sqlx::test]
async fn a_return_that_gets_in_ends_an_outage_note(db: PgPool) {
    let site = Site::new(db.clone());
    let uptime = uptime(t(0));
    let admin = admin(&site).await;
    let outage = NewNote { kind: NoteKind::Outage, text: "From the news", until: t(600) };
    notes::set(&uptime, &db, &site.state.config, &admin, outage, t(0)).await.unwrap();
    record(&uptime, &db, 1, 1, Ends::MailboxRefused, t(1)).await;
    assert_eq!(uptime.live.sign().state, Light::Offline);
    record(&uptime, &db, 2, 2, Ends::Landed, t(2)).await;
    assert_eq!(uptime.live.sign().state, Light::Online);
    assert_eq!(note_rows(&db).await[0].2.as_deref(), Some("return_got_in"));
    let ended = events(&db).await.pop().unwrap();
    assert_eq!(ended.3, "note_return_got_in");
}

/// A `bir_days` line.
#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct DayRow {
    tries: i32,
    got_in: i32,
    failed: i32,
    accounts: i32,
    networks: i32,
    online_minutes: i32,
    busy_minutes: i32,
    offline_minutes: i32,
    quiet_minutes: i32,
    note_minutes: i32,
    worst: String,
    hours: String,
    #[sqlx(rename = "final")]
    is_final: bool,
}

async fn day_row(db: &PgPool, day: time::Date) -> DayRow {
    sqlx::query_as(
        "SELECT tries, got_in, failed, accounts, networks, online_minutes, busy_minutes, offline_minutes,
                quiet_minutes, note_minutes, worst, hours, final FROM bir_days WHERE day = $1",
    )
    .bind(day)
    .fetch_one(db)
    .await
    .unwrap()
}

#[sqlx::test]
async fn the_day_history_adds_up_across_midnight_and_a_restart(db: PgPool) {
    let site = Site::new(db.clone());
    let first = uptime(t(0));
    let day = manila_day(t(0));
    // 10:00 Manila: three accounts get in; 10:10 to 10:12 the mailbox
    // refuses three more; 11:00 a landing ends it.
    an_outage(&first, &db).await;
    record(&first, &db, 4, 4, Ends::Landed, t(60)).await;
    // While the day lasts: a line so far, hours after now not known.
    days::roll_up(&db, &first.day_sets, day, t(65)).await.unwrap().unwrap();
    let row = day_row(&db, day).await;
    assert_eq!((row.tries, row.got_in, row.failed), (7, 4, 3));
    assert_eq!((row.accounts, row.networks), (7, 7));
    // Before 10:00 nothing was known (the history's first line); 10:00 to
    // 11:00 was Offline at worst; 11:00 Online; later hours not yet.
    assert_eq!(row.hours, format!("{}xo{}", "-".repeat(10), "-".repeat(12)));
    assert!(!row.is_final);

    // A quick restart at 12:00 (watched until just before): the raw day
    // sets start again. Two of the same accounts and one new one file
    // afterwards.
    watch(&first, &db, t(121)).await;
    let second = uptime(t(120));
    for (account, minute) in [(1, 121), (2, 122), (30, 123)] {
        record(&second, &db, account, account as u8, Ends::Landed, t(minute)).await;
    }
    days::roll_up(&db, &second.day_sets, day, t(125)).await.unwrap();
    let row = day_row(&db, day).await;
    assert_eq!(row.tries, 10);
    assert_eq!((row.accounts, row.networks), (7, 7), "a lower bound: the larger of before and after the restart");
    // The clock runs (the ticker does this every 30 seconds).
    for minute in 124..720 {
        live::tick(&second, &db, t(minute)).await.unwrap();
    }

    // An Admin's maintenance from 22:00 to 23:30 Manila.
    let admin = admin(&site).await;
    let maintenance = NewNote { kind: NoteKind::Maintenance, text: "Planned work", until: t(810) };
    notes::set(&second, &db, &site.state.config, &admin, maintenance, t(720)).await.unwrap();
    for minute in 721..=840 {
        live::tick(&second, &db, t(minute)).await.unwrap();
    }

    // After midnight: final, and the same line however often it runs.
    let after = t(14 * 60 + 30);
    days::roll_up_recent(&second, &db, after).await.unwrap();
    let line = day_row(&db, day).await;
    assert!(line.is_final);
    // No data before 10:00; Busy at 10:11, Offline from 10:12; Online from
    // 11:00, quiet at 12:00 (an hour without news), Online again from the
    // landing at 12:01 until an hour after the last at 12:03; quiet; the
    // maintenance from 22:00 to 23:30.
    assert_eq!(line.hours, format!("{}xooo{}mm", "-".repeat(10), "n".repeat(8)));
    assert_eq!(line.worst, "offline");
    // online, busy, offline, quiet, note minutes.
    assert_eq!(
        (line.online_minutes, line.busy_minutes, line.offline_minutes, line.quiet_minutes, line.note_minutes),
        (11 + 60 + 62, 1, 48 + 90, 1 + 537 + 30, 90)
    );
    days::roll_up_recent(&second, &db, after + Duration::hours(1)).await.unwrap();
    assert_eq!(day_row(&db, day).await, line);
    // A final line is never computed again, even once its tries are gone.
    sqlx::query("DELETE FROM bir_tries").execute(&db).await.unwrap();
    days::roll_up(&db, &second.day_sets, day, after + Duration::days(3)).await.unwrap();
    assert_eq!(day_row(&db, day).await, line);

    // A day with no tries at all: No recent news, every hour.
    let quiet = day.next_day().unwrap().next_day().unwrap();
    days::roll_up(&db, &second.day_sets, quiet, after + Duration::days(3)).await.unwrap();
    let row = day_row(&db, quiet).await;
    assert_eq!((row.tries, row.accounts, row.networks), (0, 0, 0));
    assert_eq!(row.quiet_minutes, 24 * 60);
    assert_eq!(row.worst, "no_recent_news");
    assert_eq!(row.hours, "n".repeat(24));
    assert!(!row.is_final, "not final: the sign was not evaluated after it ended");
}

/// (at, scope, from, to, cause, reason) of the `watch` events.
async fn watch_events(db: &PgPool) -> Vec<(OffsetDateTime, String, String, String)> {
    sqlx::query_as("SELECT at, from_state, to_state, reason FROM bir_events WHERE scope = 'watch' ORDER BY id")
        .fetch_all(db)
        .await
        .unwrap()
}

/// Time Buwiz was not watching is a gap: written as its own events and its
/// own value in the day line, never given to a BIR state, never extending
/// the one before, in no count, splitting an outage. `BIR_UPTIME=off` and
/// on again is the same as a restart after a gap.
#[sqlx::test]
async fn downtime_is_a_gap_in_the_history_and_no_state(db: PgPool) {
    let day = manila_day(t(0));
    // 10:00 Manila: Offline at 10:12 (Busy at 10:11), watched until 10:30,
    // then the server stops.
    let first = uptime(t(0));
    an_outage(&first, &db).await;
    watch(&first, &db, t(31)).await;
    let stopped_at = first.live.sign().evaluated_at;
    assert_eq!(stopped_at, t(28));
    // Started again at off: nothing runs, nothing is written.
    let off = Uptime::new(Level::Off, vec![22], false, t(40));
    let site = Site::with_sender(db.clone(), "http://127.0.0.1:9", "a made-up secret", off);
    buwiz_cloud::uptime::start(&site.state).await;
    assert!(watch_events(&db).await.is_empty());
    // On again at 11:30 Manila: the sign is restored (Offline holds: no 3
    // quiet hours yet), and the gap is written.
    let second = uptime(t(90));
    live::tick(&second, &db, t(90)).await.unwrap();
    let sign = second.live.sign();
    assert_eq!((sign.state, sign.upload.state), (Light::Offline, Light::Offline));
    assert_eq!(sign.upload.since, t(12), "its own since, as it was");
    let gaps = watch_events(&db).await;
    assert_eq!(
        gaps,
        [
            (t(28), "watched".into(), "not_watched".into(), "Buwiz stopped watching".into()),
            (t(90), "not_watched".into(), "watched".into(), "Buwiz was not watching for 62 min".into()),
        ]
    );
    // The front desk's quiet hour ran out in the gap: it changes at the
    // restart, not before.
    assert_eq!(sign.dispatcher.state, Light::NoRecentNews);
    assert_eq!(sign.dispatcher.since, t(90));
    // Run again: no second gap.
    live::tick(&second, &db, t(91)).await.unwrap();
    assert_eq!(watch_events(&db).await.len(), 2);

    // A landing at 12:00 ends it; the day line so far.
    watch(&second, &db, t(120)).await;
    record(&second, &db, 4, 4, Ends::Landed, t(120)).await;
    let states = days::roll_up(&db, &second.day_sets, day, t(130)).await.unwrap().unwrap();
    // 10:00 to 10:28 watched (Online, Busy a minute, Offline from 10:12);
    // 10:28 to 11:30 not watched; Offline again to 12:00; Online to 12:10.
    assert_eq!(states.unwatched_minutes, 62);
    assert_eq!(
        (states.online_minutes, states.busy_minutes, states.offline_minutes, states.quiet_minutes),
        (11 + 10, 1, 16 + 30, 0)
    );
    assert_eq!(states.hours, format!("{}xxo{}", "-".repeat(10), "-".repeat(11)));
    let outages: Vec<(i64, i64, &str)> = states
        .outages
        .iter()
        .map(|outage| ((outage.from - t0()).whole_minutes(), (outage.to - t0()).whole_minutes(), outage.window))
        .collect();
    assert_eq!(outages, [(12, 28, "upload"), (90, 120, "upload")], "the gap splits the outage");
    let row: (i32, String) = sqlx::query_as("SELECT unwatched_minutes, hours FROM bir_days WHERE day = $1")
        .bind(day)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(row, (62, states.hours.clone()));
    // Stored times are to the minute.
    let odd: i64 = sqlx::query_scalar("SELECT count(*) FROM bir_events WHERE at <> date_trunc('minute', at)")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(odd, 0);

    // A whole day down: the next start rolls it up, every hour `w`.
    let third = uptime(t(60 * 50));
    live::tick(&third, &db, t(60 * 50)).await.unwrap();
    days::roll_up_recent(&third, &db, t(60 * 50)).await.unwrap();
    let next = day.next_day().unwrap();
    let row: (i32, String, String, bool) =
        sqlx::query_as("SELECT unwatched_minutes, hours, worst, final FROM bir_days WHERE day = $1")
            .bind(next)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(row, (24 * 60, "w".repeat(24), "none".into(), true));
}

#[sqlx::test]
async fn retention_keeps_the_history_and_prunes_the_rest(db: PgPool) {
    let site = Site::new(db.clone());
    let (account, _) = site.person("filer@example.test").await;
    let claim = common::add_claim(&db, account, "000123453", "approved").await;
    sqlx::query(
        "INSERT INTO bir_held (at, account_id, claim_id, form, period, code)
         VALUES (now() - interval '91 days', $1, $2, '1601C', '2026-10', 'bir_offline'),
                (now() - interval '89 days', $1, $2, '1601C', '2026-10', 'bir_offline')",
    )
    .bind(account)
    .bind(claim)
    .execute(&db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO bir_days (day, tries, got_in, failed, maybe, ours, user_network, held, accounts, networks,
             online_minutes, busy_minutes, offline_minutes, quiet_minutes, note_minutes, worst, hours, final)
         VALUES ('2020-01-01', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1440, 0, 'no_recent_news', $1, true)",
    )
    .bind("n".repeat(24))
    .execute(&db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO bir_events (at, scope, from_state, to_state, cause, reason) VALUES ('2020-01-01', 'overall', 'online', 'offline', 'rule', 'old')")
        .execute(&db)
        .await
        .unwrap();
    store::upkeep(&db).await.unwrap();
    let count = |table: &'static str| {
        let db = db.clone();
        async move { sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}")).fetch_one(&db).await.unwrap() }
    };
    assert_eq!(count("bir_held").await, 1);
    assert_eq!(count("bir_days").await, 1, "the day history is kept forever");
    assert_eq!(count("bir_events").await, 1, "and the events");
}

/// A site's mailer that keeps what it sends, and staff to send to.
async fn mail_site(db: &PgPool) -> (Site, Mailer) {
    let site = Site::new(db.clone());
    site.staff_with_role("admin@example.test", "admin").await;
    site.staff_with_role("support@example.test", "support").await;
    let (banned, _) = site.staff_with_role("gone@example.test", "support").await;
    sqlx::query("UPDATE accounts SET banned_at = now(), banned_by = id, ban_reason = 'Made-up' WHERE id = $1")
        .bind(banned)
        .execute(db)
        .await
        .unwrap();
    site.person("filer@example.test").await;
    (site, Mailer::recording("http://localhost:3008"))
}

#[sqlx::test]
async fn staff_mail_is_batched_every_ten_minutes(db: PgPool) {
    let (site, mailer) = mail_site(&db).await;
    let config = &site.state.config;
    let uptime = uptime(t(0));
    an_outage(&uptime, &db).await;
    // Going Offline mails at once; Online and Busy before it are only in
    // the event log (the owner's rule) ...
    let mail = alerts::flush(&uptime, &db, &mailer, config, t(12)).await.unwrap().unwrap();
    assert_eq!(mail.subject, "BIR: mailbox offline");
    assert!(!mail.text.contains("Online: The mailbox is online."), "{}", mail.text);
    assert!(!mail.text.contains("Busy: The mailbox is busy."), "{}", mail.text);
    assert!(mail.text.contains(
        "10:12 AM  Offline: The mailbox is offline. 3 accounts on 3 networks didn't get past the mailbox within 15 min."
    ));
    let to: Vec<String> = mailer.notices().into_iter().map(|notice| notice.to).collect();
    assert_eq!(to, ["admin@example.test", STAFF, "support@example.test"], "Admins and Support, not the banned");
    assert_eq!(mailed(&db).await, (1, 2), "Offline mailed; Online and Busy logged only");

    // ... the next ones wait for the end of the 10 minutes and go together.
    record(&uptime, &db, 5, 5, Ends::Landed, t(15)).await;
    assert!(alerts::flush(&uptime, &db, &mailer, config, t(15)).await.unwrap().is_none());
    uptime.alerts.push(Alert::host_key("SHA256:madeupmadeupmadeupmadeupmadeupmadeupmadeup0", t(16)));
    // The same key again is told once.
    uptime.alerts.push(Alert::host_key("SHA256:madeupmadeupmadeupmadeupmadeupmadeupmadeup0", t(17)));
    assert!(alerts::flush(&uptime, &db, &mailer, config, t(21)).await.unwrap().is_none());
    let mail = alerts::flush(&uptime, &db, &mailer, config, t(22)).await.unwrap().unwrap();
    assert_eq!(mail.subject, "BIR: back online (and 1 more)");
    assert!(mail.text.contains("relay-reported, unverified: do not pin from this mail"));
    assert_eq!(mail.text.matches("SHA256:").count(), 1);
    assert_eq!(mailer.notices().len(), 6);
}

/// (mailed, not mailed) changes in the event log.
async fn mailed(db: &PgPool) -> (i64, i64) {
    sqlx::query_as(
        "SELECT count(*) FILTER (WHERE mailed_at IS NOT NULL), count(*) FILTER (WHERE mailed_at IS NULL)
           FROM bir_events WHERE scope = 'overall' AND from_state <> 'none'",
    )
    .fetch_one(db)
    .await
    .unwrap()
}

#[sqlx::test]
async fn each_kind_of_staff_mail_has_its_own_budget(db: PgPool) {
    let (site, mailer) = mail_site(&db).await;
    let config = &site.state.config;
    let uptime = uptime(t(0));
    let mut other = 0;
    for step in 0..12 {
        let at = t(step * 10);
        uptime.alerts.push(Alert::missing_header(at));
        uptime.alerts.push(Alert::host_key(&format!("SHA256:madeupmadeupmadeupmadeupmadeupmadeupmadeu{step:x}"), at));
        if let Some(mail) = alerts::flush(&uptime, &db, &mailer, config, at).await.unwrap() {
            other += mail.text.matches("without CF-Connecting-IP").count();
        }
    }
    assert_eq!(other as u32, Budget::Other.per_day());
    let keys: usize = mailer
        .notices()
        .iter()
        .filter(|notice| notice.to == STAFF)
        .map(|notice| notice.text.matches("SHA256:").count())
        .sum();
    assert_eq!(keys as u32, Budget::HostKey.per_day(), "across every fingerprint");
    // Their budgets are spent; the sign's own are not. Turning Online is
    // logged only; an outage an Admin sets is mailed.
    for account in 1..=3 {
        record(&uptime, &db, account, account as u8, Ends::Landed, t(125)).await;
    }
    let admin = admin(&site).await;
    let outage = NewNote { kind: NoteKind::Outage, text: "Made-up outage", until: t(300) };
    notes::set(&uptime, &db, config, &admin, outage, t(126)).await.unwrap();
    let mail = alerts::flush(&uptime, &db, &mailer, config, t(130)).await.unwrap().unwrap();
    assert_eq!(mail.subject, "BIR: an Admin set a note");
    assert_eq!(mailed(&db).await, (1, 1));
}

#[sqlx::test]
async fn a_mail_that_went_nowhere_is_tried_again_and_spends_nothing(db: PgPool) {
    let (site, mailer) = mail_site(&db).await;
    let config = &site.state.config;
    let uptime = uptime(t(0));
    an_outage(&uptime, &db).await;
    mailer.fail_notices(true);
    assert_eq!(alerts::flush(&uptime, &db, &mailer, config, t(12)).await.unwrap(), None);
    assert_eq!(mailed(&db).await, (0, 3), "not marked mailed");
    // A minute later, the mailer back: the same alert goes, and only now is
    // it marked.
    mailer.fail_notices(false);
    assert_eq!(alerts::flush(&uptime, &db, &mailer, config, t(12) + Duration::seconds(30)).await.unwrap(), None);
    let mail = alerts::flush(&uptime, &db, &mailer, config, t(13)).await.unwrap().unwrap();
    assert_eq!(mail.subject, "BIR: mailbox offline");
    assert_eq!(mailed(&db).await, (1, 2));
    assert_eq!(mailer.notices().len(), 3, "one mail to each of the three");
}

#[sqlx::test]
async fn a_window_change_is_mailed_under_a_declare_online_note(db: PgPool) {
    let (site, mailer) = mail_site(&db).await;
    let config = &site.state.config;
    let uptime = uptime(t(0));
    let admin = admin(&site).await;
    for account in 1..=3 {
        record(&uptime, &db, account, account as u8, Ends::Landed, t(0)).await;
    }
    notes::declare_online(&uptime, &db, config, &admin, "Made-up: the BIR says it is fine", t(120), t(1))
        .await
        .unwrap();
    alerts::flush(&uptime, &db, &mailer, config, t(1)).await.unwrap().unwrap();
    // The mailbox goes Offline; the sign stays Online.
    for (offset, account) in (11..=13).enumerate() {
        record(&uptime, &db, account, account as u8, Ends::MailboxRefused, t(10 + offset as i64)).await;
    }
    assert_eq!((uptime.live.sign().state, uptime.live.sign().upload.state), (Light::Online, Light::Offline));
    let mail = alerts::flush(&uptime, &db, &mailer, config, t(12)).await.unwrap().unwrap();
    assert_eq!(mail.subject, "BIR: mailbox offline");
    assert!(
        mail.text.contains("The mailbox is offline: 3 accounts on 3 networks didn't get past the mailbox within 15 min. The sign shows Online, as an Admin declared."),
        "{}",
        mail.text
    );
    // Back: mailed as its going Offline was (the overall sign never left
    // Online, so nothing else tells staff).
    record(&uptime, &db, 4, 4, Ends::Landed, t(30)).await;
    assert_eq!(uptime.live.sign().upload.state, Light::Online);
    let mail = alerts::flush(&uptime, &db, &mailer, config, t(30)).await.unwrap().unwrap();
    assert_eq!(mail.subject, "BIR: mailbox back");
    assert!(mail.text.contains("The mailbox is online: A return got in past the mailbox."), "{}", mail.text);
    assert!(mail.text.contains("The sign shows Online, as an Admin declared."), "{}", mail.text);
}

/// The owner's rule: of the sign's own changes only going Offline and
/// coming back from it are mailed; a window alone is not (but under a
/// "declare Online" note, above).
#[sqlx::test]
async fn only_offline_and_back_are_mailed(db: PgPool) {
    let (site, mailer) = mail_site(&db).await;
    let config = &site.state.config;
    let uptime = uptime(t(0));
    an_outage(&uptime, &db).await;
    alerts::flush(&uptime, &db, &mailer, config, t(12)).await.unwrap().unwrap();
    // Back to Online: mailed after the 10 minutes.
    record(&uptime, &db, 5, 5, Ends::Landed, t(15)).await;
    let mail = alerts::flush(&uptime, &db, &mailer, config, t(22)).await.unwrap().unwrap();
    assert_eq!(mail.subject, "BIR: back online");
    // An hour of quiet later: No recent news, logged only.
    alerts::flush(&uptime, &db, &mailer, config, t(80)).await.unwrap();
    buwiz_cloud::uptime::live::tick(&uptime, &db, t(90)).await.unwrap();
    assert_eq!(uptime.live.sign().state, Light::NoRecentNews);
    assert_eq!(alerts::flush(&uptime, &db, &mailer, config, t(100)).await.unwrap(), None);
    let unmailed: Vec<String> = sqlx::query_scalar(
        "SELECT to_state FROM bir_events WHERE scope = 'overall' AND mailed_at IS NULL AND from_state <> 'none' ORDER BY id",
    )
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(unmailed, ["online", "busy", "no_recent_news"]);
}

#[sqlx::test]
async fn staff_mail_names_nobody_who_filed(db: PgPool) {
    let (site, mailer) = mail_site(&db).await;
    let uptime = Uptime::new(Level::Record, vec![22], true, t(0));
    an_outage(&uptime, &db).await;
    record(&uptime, &db, 9, 9, Ends::HostKey, t(13)).await;
    // Production without CF-Connecting-IP.
    uptime.networks.network(&HeaderMap::new(), t(13));
    let mut texts = Vec::new();
    for minute in [14, 30, 50] {
        if let Some(mail) = alerts::flush(&uptime, &db, &mailer, &site.state.config, t(minute)).await.unwrap() {
            texts.push(format!("{}\n{}", mail.subject, mail.text));
        }
    }
    let all = texts.join("\n");
    assert!(all.contains("without CF-Connecting-IP"), "{all}");
    for personal in ["filer@example.test", "000123453", "198.51.", "127.0.0.1", "00000000-0000"] {
        assert!(!all.contains(personal), "{personal} in {all}");
    }
    for notice in mailer.notices() {
        assert_ne!(notice.to, "filer@example.test");
    }
}

#[sqlx::test]
async fn nothing_is_mailed_or_judged_at_off(db: PgPool) {
    let (site, mailer) = mail_site(&db).await;
    let uptime = Uptime::new(Level::Off, vec![22], false, t(0));
    uptime.alerts.push(Alert::missing_header(t(0)));
    assert!(alerts::flush(&uptime, &db, &mailer, &site.state.config, t(1)).await.unwrap().is_none());
    assert!(mailer.notices().is_empty());
    // The ticker does nothing either: it returns at once.
    let off = Site::with_sender(db.clone(), "http://127.0.0.1:9", "a made-up secret", uptime);
    tokio::time::timeout(std::time::Duration::from_secs(5), buwiz_cloud::uptime::run_ticker(off.state.clone()))
        .await
        .expect("the ticker returns at once at off");
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM bir_events").fetch_one(&db).await.unwrap();
    assert_eq!(events, 0);
}

/// The `buwiz-cloud bir` command (another process) writes only the note,
/// its audit line and the sign's `seq`: it cannot end an Offline, cannot
/// write over what the server holds and has not shown (the told-once
/// flags, the lost votes, the times), and the server makes the change and
/// mails it. A note set while no server runs takes effect, and is mailed,
/// when one starts.
#[sqlx::test]
async fn the_command_never_runs_the_rules(db: PgPool) {
    let (site, mailer) = mail_site(&db).await;
    let config = &site.state.config;
    let admin = admin(&site).await;
    let server = uptime(t(0));
    // An outage a landing ends (three lost votes), then another, and five
    // failures in a row since the landing (told once).
    an_outage(&server, &db).await;
    record(&server, &db, 4, 4, Ends::Landed, t(15)).await;
    for (offset, account) in (21..=25).enumerate() {
        record(&server, &db, account, account as u8, Ends::MailboxRefused, t(20 + offset as i64)).await;
    }
    let before = server.live.sign();
    assert_eq!((before.state, before.upload.state), (Light::Offline, Light::Offline));
    assert_eq!(before.vote_losses.len(), 3);
    assert!(before.streak_told.is_some());
    alerts::flush(&server, &db, &mailer, config, t(24)).await.unwrap();
    let stored: serde_json::Value = sqlx::query_scalar("SELECT state FROM bir_sign").fetch_one(&db).await.unwrap();
    let lines = events(&db).await.len();

    // Declare Online through the command's path.
    let declared = NewNote { kind: NoteKind::Online, text: "Made-up: the BIR says it is fine", until: t(200) };
    notes::record_set(&db, config, &admin, declared, Some("Made-up reason"), t(25)).await.unwrap();
    let after: serde_json::Value = sqlx::query_scalar("SELECT state FROM bir_sign").fetch_one(&db).await.unwrap();
    assert_eq!(after, stored, "the stored sign is the server's own");
    assert!(stored_seq(&db).await > before.seq);
    let audit = events(&db).await;
    assert_eq!(audit.len(), lines + 1, "the audit line only");
    let audit = audit.last().unwrap();
    assert_eq!((audit.1.as_str(), audit.2.as_str(), audit.3.as_str()), ("offline", "offline", "note_set"));
    assert_eq!(audit.5.as_deref(), Some(STAFF));

    // The server takes it in: Online overall, its window still Offline,
    // and nothing it held is lost.
    live::tick(&server, &db, t(25) + Duration::seconds(30)).await.unwrap();
    let sign = server.live.sign();
    assert_eq!((sign.state, sign.upload.state), (Light::Online, Light::Offline));
    assert_eq!(sign.upload.since, before.upload.since);
    assert_eq!(sign.upload.quorum, before.upload.quorum);
    assert_eq!(sign.vote_losses, before.vote_losses, "account ids and all");
    assert!(sign.vote_losses.iter().all(|loss| loss.account_id.is_some()));
    assert_eq!(sign.streak_told, before.streak_told);
    assert!(sign.seq > stored_seq_of(&stored));
    let mail = alerts::flush(&server, &db, &mailer, config, t(35)).await.unwrap().unwrap();
    assert!(
        mail.text.contains("Online: An Admin declared the BIR online, ignoring evidence: Made-up: the BIR says it is fine (why: Made-up reason)"),
        "{}",
        mail.text
    );

    // Cleared through the command: the window was never ended, so the sign
    // is Offline again.
    notes::record_clear(&db, config, &admin, "Made-up: not fine after all", t(36)).await.unwrap();
    live::tick(&server, &db, t(36) + Duration::seconds(30)).await.unwrap();
    assert_eq!(server.live.sign().state, Light::Offline);
    assert_eq!(server.live.sign().upload.since, before.upload.since);

    // With the server stopped, a maintenance note; the next start shows it
    // and mails it.
    watch(&server, &db, t(40)).await;
    let planned = NewNote { kind: NoteKind::Maintenance, text: "Made-up planned work", until: t(300) };
    notes::record_set(&db, config, &admin, planned, Some("Made-up: announced"), t(41)).await.unwrap();
    let restarted = uptime(t(42));
    live::tick(&restarted, &db, t(42)).await.unwrap();
    let sign = restarted.live.sign();
    assert_eq!(sign.note.as_ref().map(|note| note.kind), Some(NoteKind::Maintenance));
    let mail = alerts::flush(&restarted, &db, &mailer, config, t(42)).await.unwrap().unwrap();
    assert!(mail.text.contains("An Admin set a planned maintenance note (why: Made-up: announced)"), "{}", mail.text);
}

fn stored_seq_of(state: &serde_json::Value) -> i64 {
    state["seq"].as_i64().unwrap()
}

/// Every path takes the sign's lock before any note row: a server run that
/// holds it and ends a note never waits on a note change that holds the
/// note's row (no deadlock), and the change, once it gets the lock, finds
/// the note ended.
#[sqlx::test]
async fn note_changes_and_the_server_take_their_locks_in_one_order(db: PgPool) {
    let site = Site::new(db.clone());
    let config = site.state.config.clone();
    let admin = admin(&site).await;
    let server = uptime(t(0));
    live::tick(&server, &db, t(0)).await.unwrap();
    let outage = NewNote { kind: NoteKind::Outage, text: "Made-up outage", until: t(600) };
    for in_process in [true, false] {
        notes::record_set(&db, &config, &admin, outage, None, t(1)).await.unwrap();
        // The server, mid-write: it holds the sign's lock.
        let mut server_tx = db.begin().await.unwrap();
        live::lock_sign(&mut server_tx).await.unwrap();
        let clear = {
            let (db, config, admin) = (db.clone(), config.clone(), admin.clone());
            let uptime = uptime(t(2));
            tokio::spawn(async move {
                if in_process {
                    notes::clear(&uptime, &db, &config, &admin, "Made-up reason", t(2)).await
                } else {
                    notes::record_clear(&db, &config, &admin, "Made-up reason", t(2)).await
                }
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        // It ends the note: the change must not hold that row.
        let ended = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            sqlx::query("UPDATE bir_notes SET cleared_at = $1, cleared_how = 'expired' WHERE cleared_at IS NULL")
                .bind(t(2))
                .execute(&mut *server_tx),
        )
        .await
        .expect("the server's write waited on a note change");
        ended.unwrap();
        server_tx.commit().await.unwrap();
        let cleared = clear.await.unwrap();
        assert!(matches!(cleared, Err(NoteError::NoNote)), "{in_process}: {cleared:?}");
    }
}

/// A restart of under 5 minutes writes no gap; a stored sign from a clock
/// that was ahead is read as now; a clock that jumps a year ahead writes
/// one gap of 31 days at most.
#[sqlx::test]
async fn restarts_and_clocks_are_not_trusted_blindly(db: PgPool) {
    let first = uptime(t(0));
    for account in 1..=3 {
        record(&first, &db, account, account as u8, Ends::Landed, t(0)).await;
    }
    for second in (30..600).step_by(30) {
        live::tick(&first, &db, t(0) + Duration::seconds(second)).await.unwrap();
    }
    let stopped = first.live.sign().evaluated_at;
    // Back 4 min 59 s after the last run: no gap, though the stored time is
    // to the minute.
    let second = uptime(stopped + Duration::seconds(299));
    live::tick(&second, &db, stopped + Duration::seconds(299)).await.unwrap();
    assert!(watch_events(&db).await.is_empty(), "{:?}", watch_events(&db).await);
    assert_eq!(second.live.sign().state, Light::Online);

    // A stored sign whose times are a month ahead: read as now, no gap, and
    // the clock runs on from there.
    sqlx::query(
        "UPDATE bir_sign SET state = jsonb_set(jsonb_set(state, '{evaluated_at}', '\"2026-12-11T02:00:00Z\"'),
                                               '{since}', '\"2026-12-11T02:00:00Z\"')",
    )
    .execute(&db)
    .await
    .unwrap();
    let third = uptime(t(12));
    live::tick(&third, &db, t(12)).await.unwrap();
    let sign = third.live.sign();
    assert!(sign.evaluated_at <= t(12) && sign.since <= t(12), "{sign:?}");
    assert!(watch_events(&db).await.is_empty());
    watch(&third, &db, t(61)).await;
    live::tick(&third, &db, t(61)).await.unwrap();
    assert_eq!(third.live.sign().state, Light::NoRecentNews, "the quiet hour came, the clock was not held");
    assert!(watch_events(&db).await.is_empty());

    // A clock a year ahead: one gap, 31 days long.
    let ahead = t(61) + Duration::days(365);
    live::tick(&third, &db, ahead).await.unwrap();
    let gaps = watch_events(&db).await;
    assert_eq!(gaps.len(), 2);
    assert_eq!(gaps[1].0 - gaps[0].0, Duration::days(31));
}

/// A try fed while another run holds the rules never waits for them and is
/// never dropped: the next run takes it in.
#[sqlx::test]
async fn a_try_never_waits_for_the_rules(db: PgPool) {
    let uptime = uptime(t(0));
    live::tick(&uptime, &db, t(0)).await.unwrap();
    let held = live::lock(&uptime, &db, t(0)).await.unwrap();
    let relay = RelayTry {
        account_id: Uuid::from_u128(1),
        network: Network::V4([198, 51, 1]),
        client: None,
        admission: "open".into(),
        started_at: t(1) - Duration::seconds(20),
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        store::record(&uptime, &db, &relay, &observed(Ends::Landed), t(1)),
    )
    .await
    .expect("the try did not wait for the rules");
    assert_eq!(uptime.live.sign().state, Light::NoRecentNews, "not run yet");
    drop(held);
    live::tick(&uptime, &db, t(1) + Duration::seconds(30)).await.unwrap();
    assert_eq!(uptime.live.sign().state, Light::Online);
}

/// A day is final only once the sign was evaluated after it ended, or a
/// gap in watching covers its end: a server that stopped before midnight
/// and has not run since leaves the day open.
#[sqlx::test]
async fn a_day_is_final_only_once_its_end_was_watched(db: PgPool) {
    let uptime = uptime(t(0));
    let day = manila_day(t(0));
    record(&uptime, &db, 1, 1, Ends::Landed, t(0)).await;
    // Watched until 23:20 Manila (t(800)), then stopped.
    watch(&uptime, &db, t(800)).await;
    live::tick(&uptime, &db, t(800)).await.unwrap();
    days::roll_up(&db, &uptime.day_sets, day, t(900)).await.unwrap();
    assert!(!day_row(&db, day).await.is_final, "its last 40 minutes were never judged");
    // Back at 00:05: the gap covers midnight, and the day is final.
    live::tick(&uptime, &db, t(845)).await.unwrap();
    days::roll_up(&db, &uptime.day_sets, day, t(900)).await.unwrap();
    let row = day_row(&db, day).await;
    assert!(row.is_final);
    let unwatched: i32 = sqlx::query_scalar("SELECT unwatched_minutes FROM bir_days WHERE day = $1")
        .bind(day)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(unwatched, 40, "23:20 to midnight");
}

/// The real ticker, through a tick whose rules panic: the same tick's
/// staff mail still goes, the day history waits for a run of the rules,
/// and the next tick runs them.
#[sqlx::test]
async fn the_ticker_goes_on_through_a_panicking_tick(db: PgPool) {
    let mut uptime = Uptime::new(Level::Record, vec![22], false, OffsetDateTime::now_utc());
    uptime.tick_every = std::time::Duration::from_millis(20);
    uptime.panics.store(1, std::sync::atomic::Ordering::SeqCst);
    uptime.alerts.push(Alert::missing_header(OffsetDateTime::now_utc()));
    let site =
        Site::with_sender(db.clone(), "http://127.0.0.1:9", "a made-up Sender secret, 32 characters or more", uptime);
    site.staff_with_role("admin@example.test", "admin").await;
    let ticker = tokio::spawn(buwiz_cloud::uptime::run_ticker(site.state.clone()));
    let uptime = &site.state.uptime;
    let today = manila_day(OffsetDateTime::now_utc());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let days: i64 = sqlx::query_scalar("SELECT count(*) FROM bir_days WHERE day = $1")
            .bind(today)
            .fetch_one(&db)
            .await
            .unwrap();
        if uptime.live.sign().seq > 0 && uptime.alerts.queued() == 0 && days == 1 {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the ticker stopped after the panic");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(uptime.panics.load(std::sync::atomic::Ordering::SeqCst), 0, "the tick did panic");
    assert!(!ticker.is_finished());
    ticker.abort();
}
