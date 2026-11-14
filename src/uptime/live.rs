//! The sign as the Gatekeeper keeps it: in memory for every read, written to
//! `bir_sign` and `bir_events` on every change. The rules are
//! [`super::sign`]'s; this runs them after each recorded try ([`feed`],
//! from the recorder, after the app has its result) and every 30 seconds
//! for the changes the clock alone makes ([`tick`]).
//!
//! On start it rebuilds from the record: the sign (with its `seq`, the
//! dwell timers and the lost votes) from `bir_sign`, the `live` tries of the
//! last 3 hours from `bir_tries`, and the open note from `bir_notes`. Tries
//! read back carry only the key they were recorded under: a restart loses
//! the previous day key and every account id, so a quorum then needs fresh
//! evidence, and a lost vote is known only by the keys it was lost under.
//!
//! A change is written first and shown after: if the write fails, the
//! sign stays as it was and the tries that were new stay new, so the next
//! run makes the same change.

use std::collections::VecDeque;
use std::sync::{Arc, RwLock};

use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use tokio::sync::{Mutex, MutexGuard};
use uuid::Uuid;

use super::Uptime;
use super::alerts::Alert;
use super::classify::{Kind, Outcome, Stage, Window};
use super::keys::Keys;
use super::sign::{self, Admission, Evaluation, Event, Evidence, Note, NoteKind, SignState, Source};
use crate::account::accounts::Account;

/// The most tries the engine holds.
pub const MAX_EVIDENCE: usize = 20_000;

/// How often the clock's own changes are looked for.
pub const TICK: std::time::Duration = std::time::Duration::from_secs(30);

/// The live sign.
pub struct Live {
    sign: RwLock<Arc<SignState>>,
    engine: Mutex<Engine>,
}

#[derive(Default)]
struct Engine {
    loaded: bool,
    evidence: VecDeque<Evidence>,
    /// How many of the newest tries no run has taken in yet.
    fresh: usize,
    /// The newest note not cleared, as the record has it.
    note: Option<Note>,
    /// The newest `bir_tries` row read at the load: a try recorded before
    /// it and fed after it is there already.
    loaded_upto: i64,
}

/// An event as written, with its id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    pub id: i64,
    pub at: OffsetDateTime,
    pub event: Event,
}

impl Live {
    pub fn new(now: OffsetDateTime) -> Self {
        Self { sign: RwLock::new(Arc::new(SignState::new(now))), engine: Mutex::new(Engine::default()) }
    }

    /// The current sign, from memory.
    pub fn sign(&self) -> Arc<SignState> {
        self.sign.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    fn show(&self, state: SignState) {
        *self.sign.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(state);
    }
}

/// The engine, locked, for one change: the notes and the relay resume
/// take it to write their change and its audit line in one transaction.
pub struct Locked<'a> {
    uptime: &'a Uptime,
    engine: MutexGuard<'a, Engine>,
    now: OffsetDateTime,
}

/// Locks the engine (loading it from the record first, once) for a run at
/// `now` (never before the last run).
pub async fn lock<'a>(uptime: &'a Uptime, db: &PgPool, now: OffsetDateTime) -> sqlx::Result<Locked<'a>> {
    let mut engine = uptime.live.engine.lock().await;
    if !engine.loaded {
        load(uptime, &mut engine, db, now).await?;
    }
    let now = now.max(uptime.live.sign().evaluated_at);
    let mut locked = Locked { uptime, engine, now };
    locked.prune();
    Ok(locked)
}

impl Locked<'_> {
    pub fn now(&self) -> OffsetDateTime {
        self.now
    }

    /// The newest note the record has not cleared.
    pub fn note(&self) -> Option<&Note> {
        self.engine.note.as_ref()
    }

    fn prune(&mut self) {
        let oldest = self.now - sign::EVIDENCE_KEPT;
        let engine = &mut *self.engine;
        while engine.evidence.front().is_some_and(|item| item.at < oldest) || engine.evidence.len() > MAX_EVIDENCE {
            engine.evidence.pop_front();
        }
        engine.fresh = engine.fresh.min(engine.evidence.len());
    }

    fn push(&mut self, evidence: Evidence) {
        // Read at the load already (it was written before): this copy, with
        // its account id and both keys, replaces that one.
        if let Some(id) = evidence.id
            && id <= self.engine.loaded_upto
            && let Some(at) = self.engine.evidence.iter().position(|item| item.id == Some(id))
        {
            self.engine.evidence.remove(at);
        }
        self.engine.evidence.push_back(evidence);
        self.engine.fresh += 1;
        self.prune();
    }

    /// The rules, with `note` as the open note and whether an Admin resumes
    /// the relay.
    pub fn evaluate(&mut self, note: Option<&Note>, resume_relay: bool) -> Evaluation {
        let fresh = self.engine.fresh;
        let evidence = self.engine.evidence.make_contiguous();
        let inputs = sign::Inputs { evidence, fresh, note, resume_relay, now: self.now };
        sign::evaluate(&self.uptime.live.sign(), &inputs)
    }

    /// Writes what `evaluation` changed: its events (an Admin's with
    /// `actor`, the audit line), a note that ended, and the sign.
    pub async fn write(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        evaluation: &Evaluation,
        actor: Option<&Account>,
    ) -> sqlx::Result<Vec<Written>> {
        let mut written = Vec::new();
        for event in &evaluation.events {
            let actor = actor.filter(|_| event.cause.by_staff());
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO bir_events (at, scope, from_state, to_state, cause, reason, actor_id, actor_email)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
            )
            .bind(self.now)
            .bind(event.scope.as_str())
            .bind(&event.from)
            .bind(&event.to)
            .bind(event.cause.as_str())
            .bind(&event.reason)
            .bind(actor.map(|actor| actor.id))
            .bind(actor.map(|actor| actor.email.as_str()))
            .fetch_one(&mut **tx)
            .await?;
            written.push(Written { id, at: self.now, event: event.clone() });
        }
        if let Some((id, how)) = evaluation.note_ended {
            sqlx::query("UPDATE bir_notes SET cleared_at = $2, cleared_how = $3 WHERE id = $1 AND cleared_at IS NULL")
                .bind(id)
                .bind(self.now)
                .bind(how.as_str())
                .execute(&mut **tx)
                .await?;
        }
        write_sign(tx, &evaluation.state).await?;
        Ok(written)
    }

    /// Shows `evaluation` once it is written, with `note` the open note it
    /// was run with, and queues the staff mail for what changed.
    pub fn show(mut self, evaluation: Evaluation, note: Option<Note>, written: &[Written]) {
        self.engine.fresh = 0;
        self.engine.note = match evaluation.note_ended {
            Some((id, _)) if note.as_ref().is_some_and(|note| note.id == id) => None,
            _ => note,
        };
        for alert in Alert::for_run(written, &evaluation.state) {
            self.uptime.alerts.push(alert);
        }
        self.uptime.live.show(evaluation.state);
    }

    /// One run with the open note as the record has it: written when
    /// something changed, then shown.
    async fn run(mut self, db: &PgPool) -> sqlx::Result<()> {
        let note = self.engine.note.clone();
        let evaluation = self.evaluate(note.as_ref(), false);
        let previous = self.uptime.live.sign();
        let written = if evaluation.changed(&previous) {
            let mut tx = db.begin().await?;
            let written = self.write(&mut tx, &evaluation, None).await?;
            tx.commit().await?;
            written
        } else {
            Vec::new()
        };
        self.show(evaluation, note, &written);
        Ok(())
    }
}

async fn write_sign(tx: &mut Transaction<'_, Postgres>, state: &SignState) -> sqlx::Result<()> {
    let value = serde_json::to_value(state).map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
    sqlx::query(
        "INSERT INTO bir_sign (id, seq, state, updated_at) VALUES (1, $1, $2, $3)
         ON CONFLICT (id) DO UPDATE SET seq = GREATEST(bir_sign.seq, EXCLUDED.seq), state = EXCLUDED.state,
             updated_at = EXCLUDED.updated_at",
    )
    .bind(state.seq)
    .bind(value)
    .bind(state.evaluated_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// A try just recorded: the rules run with it. Called by the recorder, after
/// the app has its result.
pub async fn feed(uptime: &Uptime, db: &PgPool, evidence: Evidence, now: OffsetDateTime) -> sqlx::Result<()> {
    let mut locked = lock(uptime, db, now).await?;
    locked.push(evidence);
    locked.run(db).await
}

/// The rules with no new try: quiet hours, a note running out, the dwell.
pub async fn tick(uptime: &Uptime, db: &PgPool, now: OffsetDateTime) -> sqlx::Result<()> {
    lock(uptime, db, now).await?.run(db).await
}

fn stage(raw: &str) -> Stage {
    match raw {
        "dispatcher" => Stage::Dispatcher,
        "upload" => Stage::Upload,
        "uploaded" => Stage::Uploaded,
        _ => Stage::None,
    }
}

fn window(raw: &str) -> Window {
    match raw {
        "ok" => Window::Ok,
        "fail" => Window::Fail,
        "maybe" => Window::Maybe,
        _ => Window::None,
    }
}

fn key(raw: &[u8]) -> [u8; 8] {
    let mut key = [0u8; 8];
    if raw.len() == 8 {
        key.copy_from_slice(raw);
    }
    key
}

type TryRow = (
    i64,
    OffsetDateTime,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    bool,
    bool,
    String,
    i32,
    Vec<u8>,
    Option<Vec<u8>>,
);

/// A `bir_tries` row as the engine reads it.
fn evidence_of(row: TryRow) -> Evidence {
    let (
        id,
        at,
        source,
        outcome,
        stage_raw,
        dispatcher,
        upload,
        kind,
        counted,
        late,
        admission,
        epoch,
        account,
        network,
    ) = row;
    Evidence {
        id: Some(id),
        at,
        source: if source == "direct" { Source::Direct } else { Source::Relay },
        admission: Admission::parse(&admission),
        outcome: Outcome::parse(&outcome),
        stage: stage(&stage_raw),
        dispatcher: window(&dispatcher),
        upload: window(&upload),
        kind: kind.as_deref().and_then(Kind::parse),
        counted,
        late: late || network.is_none(),
        keys: vec![Keys { epoch, account: key(&account), network: network.as_deref().map(key).unwrap_or_default() }],
        account_id: None,
        port: None,
    }
}

async fn load(uptime: &Uptime, engine: &mut Engine, db: &PgPool, now: OffsetDateTime) -> sqlx::Result<()> {
    let row: Option<(i64, Value)> =
        sqlx::query_as("SELECT seq, state FROM bir_sign WHERE id = 1").fetch_optional(db).await?;
    let first = row.is_none();
    let mut state = match row {
        Some((seq, value)) => match serde_json::from_value::<SignState>(value) {
            Ok(mut state) => {
                state.seq = state.seq.max(seq);
                state
            }
            Err(error) => {
                // The sign starts again, but its number still only grows.
                tracing::error!(%error, "BIR uptime: the stored sign could not be read; starting from No recent news");
                SignState { seq: seq + 1, ..SignState::new(now) }
            }
        },
        None => SignState::new(now),
    };
    let rows: Vec<TryRow> = sqlx::query_as(
        "SELECT id, ended_at, source, outcome, stage, dispatcher, upload, failure_kind, counted, late, admission,
                key_epoch, account_key, network_key
           FROM bir_tries WHERE mode = 'live' AND ended_at >= $1 ORDER BY id",
    )
    .bind(now - sign::EVIDENCE_KEPT)
    .fetch_all(db)
    .await?;
    let note: Option<(Uuid, String, String, OffsetDateTime, OffsetDateTime)> = sqlx::query_as(
        "SELECT id, kind, text, starts_at, until FROM bir_notes WHERE cleared_at IS NULL
          ORDER BY starts_at DESC LIMIT 1",
    )
    .fetch_optional(db)
    .await?;
    if first {
        // The first start: the times of news from the whole record.
        let (dispatcher_ok, upload_ok, success, news): (
            Option<OffsetDateTime>,
            Option<OffsetDateTime>,
            Option<OffsetDateTime>,
            Option<OffsetDateTime>,
        ) = sqlx::query_as(
            "SELECT max(ended_at) FILTER (WHERE source = 'relay' AND dispatcher = 'ok'),
                    max(ended_at) FILTER (WHERE source = 'relay' AND upload = 'ok'),
                    max(ended_at) FILTER (WHERE source = 'relay' AND outcome = 'sent'),
                    max(ended_at)
               FROM bir_tries WHERE counted",
        )
        .fetch_one(db)
        .await?;
        state.dispatcher.last_ok_at = dispatcher_ok;
        state.upload.last_ok_at = upload_ok;
        state.last_success_at = success;
        state.last_news_at = news;
        state.seq = 1;
        // The first line of the history: from here the sign is known.
        let mut tx = db.begin().await?;
        sqlx::query(
            "INSERT INTO bir_events (at, scope, from_state, to_state, cause, reason)
             VALUES ($1, 'overall', 'none', $2, 'rule', 'Buwiz began keeping the sign')",
        )
        .bind(now)
        .bind(state.state.as_str())
        .execute(&mut *tx)
        .await?;
        write_sign(&mut tx, &state).await?;
        tx.commit().await?;
    }
    engine.loaded_upto = rows.last().map_or(0, |row| row.0);
    engine.evidence = rows.into_iter().map(evidence_of).collect();
    engine.fresh = 0;
    engine.note = note.and_then(|(id, kind, text, starts_at, until)| {
        Some(Note { id, kind: NoteKind::parse(&kind)?, text, starts_at, until })
    });
    engine.loaded = true;
    uptime.live.show(state);
    Ok(())
}
