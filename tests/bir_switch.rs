//! `buwiz-cloud bir`, the manual switch, and a running server: Admins only,
//! 72 hours at most, the audit line, the server taking a note written by
//! another process in without a restart, and two processes never writing
//! over each other. Made-up data and the local test database only.

mod common;

use buwiz_cloud::uptime::cli::{self, Command, Until};
use buwiz_cloud::uptime::live;
use buwiz_cloud::uptime::sign::{Light, NoteKind, SignState};
use buwiz_cloud::uptime::{Level, Uptime};
use common::{STAFF, Site};
use serde_json::Value;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};

fn site(db: &PgPool, level: Level) -> Site {
    let uptime = Uptime::new(level, vec![22], false, OffsetDateTime::now_utc());
    Site::with_sender(db.clone(), "http://127.0.0.1:9", "a made-up Sender secret, 32 characters or more", uptime)
}

/// Stores a sign in `light`, fresh, and starts the site's engine on it.
async fn sign_in(site: &Site, light: Light) {
    let now = OffsetDateTime::now_utc();
    let mut sign = SignState::new(now - Duration::minutes(20));
    sign.seq = 41;
    sign.state = light;
    sign.evaluated_at = now - Duration::seconds(20);
    for lamp in [&mut sign.dispatcher, &mut sign.upload] {
        lamp.state = light;
        lamp.last_ok_at = Some(now - Duration::minutes(3));
        lamp.last_evidence_at = Some(now - Duration::minutes(1));
    }
    sqlx::query("INSERT INTO bir_sign (id, seq, state, updated_at) VALUES (1, $1, $2, $3)")
        .bind(sign.seq)
        .bind(serde_json::to_value(&sign).unwrap())
        .bind(now)
        .execute(&site.db)
        .await
        .unwrap();
    buwiz_cloud::uptime::start(&site.state).await;
    assert_eq!(site.state.uptime.live.sign().state, light);
}

/// The public document as everything public serves it at `at`.
fn public_status(site: &Site, at: OffsetDateTime) -> Value {
    serde_json::from_slice(&site.state.uptime.published("https://ebirforms.buwiz.com", at).body).unwrap()
}

async fn admin(site: &Site) -> buwiz_cloud::account::roles::Staff {
    let (id, _) = site.person(STAFF).await;
    let account = buwiz_cloud::account::accounts::Account { id, email: STAFF.into() };
    buwiz_cloud::account::roles::staff(&site.db, &site.state.config, &account).await.unwrap().unwrap()
}

/// `buwiz-cloud bir`: Admins only, 72 hours at most, the audit line, and a
/// server already running takes the note in without a restart.
#[sqlx::test]
async fn the_switch_reaches_a_running_server(db: PgPool) {
    let server = site(&db, Level::Public);
    sign_in(&server, Light::Online).await;
    let config = server.state.config.clone();
    let now = OffsetDateTime::now_utc();
    server.person(STAFF).await;
    server.staff_with_role("support@example.test", "support").await;
    server.person("filer@example.test").await;
    let run = |command: Command| {
        let (db, config) = (db.clone(), config.clone());
        async move {
            let mut out = Vec::new();
            let result =
                cli::run(&db, &config, Level::Public, false, command, OffsetDateTime::now_utc(), &mut out).await;
            (result, String::from_utf8(out).unwrap())
        }
    };
    let declare = |actor: &str, until: Until| Command::Declare {
        kind: NoteKind::Outage,
        actor: actor.into(),
        reason: "Made-up: the BIR posted an advisory".into(),
        until,
        text: Some("The BIR says its eFPS is down.".into()),
    };
    // Not an Admin: refused, nothing written.
    for actor in ["support@example.test", "filer@example.test", "nobody@example.test"] {
        let (result, _) = run(declare(actor, Until::Hours(2))).await;
        assert!(matches!(result, Err(cli::CliError::Refused(_))), "{actor}");
    }
    // More than 72 hours: refused.
    let (result, _) = run(declare(STAFF, Until::Hours(73))).await;
    assert!(matches!(result, Err(cli::CliError::Refused(ref why)) if why.contains("72 hours")));
    let (result, _) = run(declare(STAFF, Until::At(now + Duration::hours(72) + Duration::minutes(5)))).await;
    assert!(matches!(result, Err(cli::CliError::Refused(_))));
    let notes: i64 = sqlx::query_scalar("SELECT count(*) FROM bir_notes").fetch_one(&db).await.unwrap();
    assert_eq!(notes, 0);
    // An Admin, while the server runs.
    let stored_before: Value = sqlx::query_scalar("SELECT state FROM bir_sign").fetch_one(&db).await.unwrap();
    let seq_before = server.state.uptime.live.sign().seq;
    let (result, out) = run(declare(STAFF, Until::Hours(6))).await;
    result.unwrap();
    assert!(out.contains("Will: declare the BIR offline (an outage note) until"), "{out}");
    assert!(out.contains("Done: the note and its audit line are written."), "{out}");
    // The command ran no rule: the stored sign is the server's, only its
    // `seq` moved on.
    let (seq, stored): (i64, Value) = sqlx::query_as("SELECT seq, state FROM bir_sign").fetch_one(&db).await.unwrap();
    assert_eq!(stored, stored_before);
    assert!(seq > seq_before);
    let audit: (String, String, Option<String>) = sqlx::query_as(
        "SELECT cause, reason, actor_email FROM bir_events WHERE cause = 'note_set' ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(
        audit,
        (
            "note_set".into(),
            "An Admin set an outage note (why: Made-up: the BIR posted an advisory)".into(),
            Some(STAFF.into())
        )
    );
    // The server has not run its rules yet: it still says Online ...
    assert_eq!(server.state.uptime.live.sign().state, Light::Online);
    // ... its next tick (every 30 s) takes the note in, without a restart.
    live::tick(&server.state.uptime, &db, OffsetDateTime::now_utc()).await.unwrap();
    // (Read as at the next 5-minute mark: the public sign moves only on
    // marks.)
    let next = buwiz_cloud::uptime::public::round_down(OffsetDateTime::now_utc()) + Duration::minutes(5);
    let status = public_status(&server, next);
    assert_eq!((status["state"].as_str(), status["source"].as_str()), (Some("offline"), Some("staff")));
    assert_eq!(status["note"]["text"], "The BIR says its eFPS is down.");
    // The server made the change and mails it, with the Admin's reason.
    let change: (String, String, String) = sqlx::query_as(
        "SELECT from_state, to_state, cause FROM bir_events WHERE scope = 'overall' AND actor_id IS NULL
          ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(change, ("online".into(), "offline".into(), "note_set".into()));
    let mailer = buwiz_cloud::account::mail::Mailer::recording("http://localhost:3008");
    let mail =
        buwiz_cloud::uptime::alerts::flush(&server.state.uptime, &db, &mailer, &config, OffsetDateTime::now_utc())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(mail.subject, "BIR: an Admin set a note");
    assert!(
        mail.text.contains("Offline: An Admin set an outage note (why: Made-up: the BIR posted an advisory)"),
        "{}",
        mail.text
    );
    // Its own next write does not overwrite the note, nor lose an event.
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM bir_events").fetch_one(&db).await.unwrap();
    live::tick(&server.state.uptime, &db, OffsetDateTime::now_utc() + Duration::minutes(2)).await.unwrap();
    let stored: String =
        sqlx::query_scalar("SELECT state->'note'->>'text' FROM bir_sign").fetch_one(&db).await.unwrap();
    assert_eq!(stored, "The BIR says its eFPS is down.");
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM bir_events").fetch_one(&db).await.unwrap();
    assert_eq!(after, events);
    // A second note while one is in force: refused.
    let (result, _) = run(declare(STAFF, Until::Hours(1))).await;
    assert!(matches!(result, Err(cli::CliError::Refused(ref why)) if why.contains("in force")));
    // Status writes nothing and names nobody who filed.
    let (result, out) = run(Command::Status).await;
    result.unwrap();
    assert!(out.contains("Offline since") && out.contains("Note: outage until"), "{out}");
    assert!(!out.contains("filer@example.test"));
    // Clear, through the same path; the server sees that too.
    let (result, out) = run(Command::Clear { actor: STAFF.into(), reason: "Made-up: the BIR is back".into() }).await;
    result.unwrap();
    assert!(out.contains("Will: clear the outage note"), "{out}");
    live::tick(&server.state.uptime, &db, OffsetDateTime::now_utc() + Duration::minutes(3)).await.unwrap();
    let status = public_status(&server, next + Duration::minutes(5));
    assert_eq!(status["source"], "tries");
    assert!(status["note"].is_null());
    let cleared: (String, Option<String>) =
        sqlx::query_as("SELECT cleared_how, cleared_reason FROM bir_notes").fetch_one(&db).await.unwrap();
    assert_eq!(cleared, ("staff".into(), Some("Made-up: the BIR is back".into())));
    // At off nothing is set.
    let mut out = Vec::new();
    let off = cli::run(&db, &config, Level::Off, false, declare(STAFF, Until::Hours(1)), now, &mut out).await;
    assert!(matches!(off, Err(cli::CliError::Refused(ref why)) if why.contains("off")));
}

/// Two processes each holding an engine never write over each other: the
/// one whose sign is older finds it moved, writes nothing and bumps no
/// `seq`.
#[sqlx::test]
async fn two_processes_never_write_over_each_other(db: PgPool) {
    let server = site(&db, Level::Public);
    sign_in(&server, Light::Online).await;
    let admin = admin(&server).await;
    // Another process's engine, loaded now.
    let other = Uptime::new(Level::Public, vec![22], false, OffsetDateTime::now_utc());
    let mut locked = live::lock(&other, &db, OffsetDateTime::now_utc()).await.unwrap();
    // The server changes the sign meanwhile.
    let note = buwiz_cloud::uptime::notes::NewNote {
        kind: NoteKind::Maintenance,
        text: "Made-up maintenance",
        until: OffsetDateTime::now_utc() + Duration::hours(1),
    };
    buwiz_cloud::uptime::notes::set(
        &server.state.uptime,
        &db,
        &server.state.config,
        &admin,
        note,
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    let (seq, state): (i64, Value) = sqlx::query_as("SELECT seq, state FROM bir_sign").fetch_one(&db).await.unwrap();
    let lines: i64 = sqlx::query_scalar("SELECT count(*) FROM bir_events").fetch_one(&db).await.unwrap();
    // The other's write, from the sign it read, is refused ...
    let mut evaluation = locked.evaluate(None, false);
    evaluation.state.seq = seq + 1_000;
    let mut tx = db.begin().await.unwrap();
    let error = locked.write(&mut tx, &evaluation, None).await.unwrap_err();
    assert!(live::moved(&error));
    tx.commit().await.unwrap();
    drop(locked);
    // ... and leaves no trace: the same seq, the same sign, no line.
    let after: (i64, Value) = sqlx::query_as("SELECT seq, state FROM bir_sign").fetch_one(&db).await.unwrap();
    assert_eq!(after, (seq, state));
    let lines_after: i64 = sqlx::query_scalar("SELECT count(*) FROM bir_events").fetch_one(&db).await.unwrap();
    assert_eq!(lines_after, lines);
    assert_eq!(server.state.uptime.live.sign().seq, seq);
}
