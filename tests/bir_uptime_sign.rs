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
use buwiz_cloud::uptime::alerts::{self, Alert, PER_DAY};
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
    assert_eq!(sign.seq, 4, "the first line, then Online, Busy and Offline");

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
    assert_eq!((after.seq, after.state, after.since), (before.seq, before.state, before.since));
    assert_eq!(kept(&after.upload), kept(&before.upload));
    assert_eq!(kept(&after.dispatcher), kept(&before.dispatcher));
    assert_eq!(events(&db).await.len(), lines, "nothing changed by the restart");
    // Offline holds until a landing.
    record(&second, &db, 11, 11, Ends::MailboxRefused, t(14)).await;
    assert_eq!(second.live.sign().state, Light::Offline);
    record(&second, &db, 5, 5, Ends::Landed, t(15)).await;
    assert_eq!(second.live.sign().state, Light::Online);
    assert_eq!(second.live.sign().seq, before.seq + 1);

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
    assert_eq!(sixth.live.sign().seq, seq);
    assert_eq!(stored_seq(&db).await, seq);
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
    assert_eq!(sign.seq, 43, "the reset, then the gap since its last write");
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
    assert_eq!(stored_seq(&db).await, 44);
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
        assert!(matches!(notes::clear(&uptime, &db, config, actor, t(1)).await, Err(NoteError::AdminOnly)));
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
    notes::clear(&uptime, &db, config, &other, t(3)).await.unwrap();
    let audit = events(&db).await.pop().unwrap();
    assert_eq!((audit.3.as_str(), audit.5.as_deref()), ("note_cleared", Some(STAFF)));
    assert_eq!(uptime.live.sign().waking_since, Some(t(3)));
    assert_eq!(uptime.live.sign().note, None);
    assert!(matches!(notes::clear(&uptime, &db, config, &admin, t(4)).await, Err(NoteError::NoNote)));
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
    assert!(row.is_final);
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
    // The first change mails at once ...
    let mail = alerts::flush(&uptime, &db, &mailer, config, t(12)).await.unwrap().unwrap();
    assert_eq!(mail.subject, "BIR: online (and 2 more)");
    assert!(mail.text.contains("10:00 AM  Online: The mailbox is online."), "{}", mail.text);
    assert!(mail.text.contains("10:11 AM  Busy: The mailbox is busy."), "{}", mail.text);
    assert!(mail.text.contains(
        "10:12 AM  Offline: The mailbox is offline. 3 accounts on 3 networks didn't get past the mailbox within 15 min."
    ));
    let to: Vec<String> = mailer.notices().into_iter().map(|notice| notice.to).collect();
    assert_eq!(to, ["admin@example.test", STAFF, "support@example.test"], "Admins and Support, not the banned");
    let mailed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM bir_events WHERE mailed_at IS NOT NULL").fetch_one(&db).await.unwrap();
    assert_eq!(mailed, 3, "the three changes of the overall sign");

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

#[sqlx::test]
async fn at_most_24_mails_a_day(db: PgPool) {
    let (site, mailer) = mail_site(&db).await;
    let uptime = uptime(t(0));
    let mut sent = 0;
    for step in 0..40 {
        let at = t(step * 10);
        uptime.alerts.push(Alert::missing_header(at));
        if let Some(mail) = alerts::flush(&uptime, &db, &mailer, &site.state.config, at).await.unwrap() {
            sent += 1;
            if sent == PER_DAY {
                assert!(mail.text.contains("This is the 24th mail today."));
            }
        }
    }
    assert_eq!(sent, PER_DAY);
    // A new Manila day starts the count again (midnight is 16:00 UTC).
    let tomorrow = OffsetDateTime::parse("2026-11-11T16:05:00Z", &Rfc3339).unwrap();
    uptime.alerts.push(Alert::missing_header(tomorrow));
    assert!(alerts::flush(&uptime, &db, &mailer, &site.state.config, tomorrow).await.unwrap().is_some());
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
    buwiz_cloud::uptime::run_ticker(off.state.clone()).await;
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM bir_events").fetch_one(&db).await.unwrap();
    assert_eq!(events, 0);
}
