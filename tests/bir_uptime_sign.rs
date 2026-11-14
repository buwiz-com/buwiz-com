//! BIR uptime's sign with its database: tries recorded through the
//! recorder move it, every change leaves an event, the sign survives a
//! restart, and notes and the relay resume are Admin-only and audited.
//! Made-up data, a made-up clock, and the local test database only:
//! nothing else is connected to.

mod common;

use buwiz_cloud::account::accounts::Account;
use buwiz_cloud::account::roles::{self, Role, Staff};
use buwiz_cloud::uptime::keys::Network;
use buwiz_cloud::uptime::live;
use buwiz_cloud::uptime::notes::{self, NewNote, NoteError};
use buwiz_cloud::uptime::observe::{Observed, TryObserver};
use buwiz_cloud::uptime::sign::{Light, NoteKind};
use buwiz_cloud::uptime::store::{self, RelayTry};
use buwiz_cloud::uptime::{Level, Uptime};
use common::{STAFF, Site};
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
            expect("upload", "online", "offline", "rule"),
            expect("overall", "online", "offline", "rule"),
        ]
    );
    // Counts only: no key, no id, no address.
    for (.., reason, actor) in events(&db).await {
        assert!(actor.is_none());
        assert!(!reason.contains("198.51") && !reason.contains("0000"), "{reason}");
    }
    assert_eq!(stored_seq(&db).await, sign.seq);
    assert_eq!(sign.seq, 3, "the first line, then Online, then Offline");

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
    assert_eq!(events(&db).await.len(), 8);
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
    assert_eq!(after.upload, before.upload);
    assert_eq!(after.dispatcher, before.dispatcher);
    assert_eq!(events(&db).await.len(), lines, "nothing changed by the restart");
    // Tries before the restart cannot be compared with tries after it:
    // two fresh failures after the restart are not a quorum with the old
    // ones, and Offline holds until a landing.
    record(&second, &db, 11, 11, Ends::MailboxRefused, t(14)).await;
    assert_eq!(second.live.sign().state, Light::Offline);
    record(&second, &db, 5, 5, Ends::Landed, t(15)).await;
    assert_eq!(second.live.sign().state, Light::Online);
    assert_eq!(second.live.sign().seq, before.seq + 1);

    // Busy, with its dwell, across a restart.
    let third = uptime(t(16));
    for account in [11, 11, 12] {
        record(&third, &db, account + 20, (account + 20) as u8, Ends::MailboxRefused, t(16)).await;
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

#[sqlx::test]
async fn a_stored_sign_that_cannot_be_read_starts_again_with_a_higher_seq(db: PgPool) {
    sqlx::query("INSERT INTO bir_sign (id, seq, state, updated_at) VALUES (1, 41, '{}', now())")
        .execute(&db)
        .await
        .unwrap();
    let uptime = uptime(t(0));
    live::tick(&uptime, &db, t(0)).await.unwrap();
    assert_eq!(uptime.live.sign().seq, 42);
    assert_eq!(uptime.live.sign().state, Light::NoRecentNews);
    record(&uptime, &db, 1, 1, Ends::Landed, t(1)).await;
    assert_eq!(stored_seq(&db).await, 43);
}

#[sqlx::test]
async fn the_relay_pauses_on_faults_that_touch_the_bir(db: PgPool) {
    let site = Site::new(db.clone());
    let uptime = uptime(t(0));
    record(&uptime, &db, 1, 1, Ends::HostKey, t(1)).await;
    record(&uptime, &db, 1, 1, Ends::HostKey, t(2)).await;
    assert!(!uptime.live.sign().relay.paused);
    record(&uptime, &db, 2, 2, Ends::HostKey, t(3)).await;
    let sign = uptime.live.sign();
    assert!(sign.relay.paused);
    assert_eq!(sign.state, Light::NoRecentNews, "a fault of Buwiz's is no evidence about the BIR");
    let paused = events(&db).await.into_iter().find(|(scope, ..)| scope == "relay").unwrap();
    assert_eq!((paused.1.as_str(), paused.2.as_str(), paused.3.as_str()), ("ok", "paused", "relay_paused"));
    assert!(paused.4.starts_with("3 tries from 2 accounts"), "{}", paused.4);
    // It survives a restart.
    let restarted = Uptime::new(Level::Record, vec![22], false, t(4));
    live::tick(&restarted, &db, t(4)).await.unwrap();
    assert!(restarted.live.sign().relay.paused);

    // Only an Admin resumes sending.
    let (support, _) = site.staff_with_role("support@example.test", "support").await;
    let support = staff(&site, support, Role::Support).await;
    let refused = notes::resume_relay(&restarted, &db, &site.state.config, &support, t(5)).await;
    assert!(matches!(refused, Err(NoteError::AdminOnly)));
    let admin = admin(&site).await;
    notes::resume_relay(&restarted, &db, &site.state.config, &admin, t(5)).await.unwrap();
    assert!(!restarted.live.sign().relay.paused);
    let resumed = events(&db).await.pop().unwrap();
    assert_eq!((resumed.0.as_str(), resumed.3.as_str()), ("relay", "relay_resumed"));
    assert_eq!(resumed.5.as_deref(), Some(STAFF));
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
