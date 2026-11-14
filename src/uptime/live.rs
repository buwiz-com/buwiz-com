//! The sign as the Gatekeeper keeps it: in memory for every read, written to
//! `bir_sign` and `bir_events` on every change. The rules are
//! [`super::sign`]'s; this runs them after each recorded try ([`feed`],
//! from the recorder, after the app has its result) and every 30 seconds
//! for the changes the clock alone makes ([`tick`]). One Gatekeeper
//! process is assumed: two would each keep their own sign.
//!
//! No try is lost on the way in: a recorded try is put in [`Live`]'s inbox
//! before the engine's lock is waited for, the next run takes it in, and it
//! stays in memory (3 hours, [`MAX_EVIDENCE`] at most) whatever happens to
//! that run. A run reads all of it again, so one that failed is simply run
//! again. Only the database work is bounded by a timeout.
//!
//! On start (D4 of the plan's amendments): the very first start is No
//! recent news everywhere. A restart restores the stored sign as it was,
//! with its "since" times, the dwell timers and the lost votes, reads the
//! `live` tries of the last 3 hours and the open note, and ages it by the
//! clock with the ordinary rules before anything can read it. Tries read
//! back carry only the key they were recorded under, of a key period no
//! longer in memory: they keep the times of news and can end an Offline,
//! but count in no last 10 and no quorum, so a restart never turns Offline
//! into Online or Online into Offline, and a quorum needs fresh tries. Time
//! Buwiz was not watching (stopped, or the sign not evaluated for more than
//! 5 minutes) is written as its own pair of `watch` events.
//!
//! Another process may change the sign too: the `buwiz-cloud bir` command
//! sets and clears notes through [`super::notes`] with an engine of its
//! own. Every run first reads the stored `seq`; one higher than this
//! engine's means another process wrote, and its sign, its open note and
//! its events (for the staff mail) are taken in before the rules run, the
//! tries in memory and the account ids of the lost votes kept. Every write
//! holds a lock on the sign and checks the stored `seq` again, so two
//! processes never write over each other: the one that finds it moved
//! writes nothing ([`Moved`]) and its next run takes the other's in.
//!
//! A change is written first and shown after: if the write fails, the
//! sign stays as it was, and the next run makes the same change. Events are
//! keyed by the sign's `seq` and their place in the run, so a run whose
//! commit landed but whose acknowledgement was lost adds nothing twice. The
//! times of news and of the evaluation alone are written at most once a
//! minute.

use std::collections::VecDeque;
use std::sync::{Arc, RwLock};

use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use time::{Duration, OffsetDateTime};
use tokio::sync::{Mutex, MutexGuard};
use uuid::Uuid;

use super::Uptime;
use super::alerts::Alert;
use super::classify::{Kind, Outcome, Stage, Window};
use super::keys::Keys;
use super::sign::{
    self, Admission, Cause, Evaluation, Event, Evidence, Light, Note, NoteKind, Scope, SignState, Source, Tally,
};
use super::store::to_minute;
use crate::account::accounts::Account;

/// The most tries the engine holds.
pub const MAX_EVIDENCE: usize = 20_000;

/// The most tries waiting in the inbox; past it the oldest go (logged).
pub const MAX_INBOX: usize = 20_000;

/// How often the clock's own changes are looked for.
pub const TICK: std::time::Duration = std::time::Duration::from_secs(30);

/// The times of news and of the evaluation alone are written this often.
pub const WRITE_EVERY: Duration = Duration::seconds(60);

/// How long the database work of one run (or the load) may take.
pub const DB_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The lock every write of the sign holds, across processes ("birsign" in
/// ASCII).
const SIGN_LOCK: i64 = 0x0062_6972_7369_676e;

/// A write found that another process changed the sign since this engine
/// read it: nothing was written, and the next run takes the other's in.
#[derive(Debug, thiserror::Error)]
#[error("the BIR sign was changed by another process; nothing was written")]
pub struct Moved;

/// Whether `error` is [`Moved`].
pub fn moved(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Io(io) if io.get_ref().is_some_and(|inner| inner.is::<Moved>()))
}

/// The live sign.
pub struct Live {
    sign: RwLock<Arc<SignState>>,
    /// Each window's last 10 at the last run (front desk, mailbox).
    last_ten: RwLock<[Tally; 2]>,
    engine: Mutex<Engine>,
    /// Tries recorded and not yet taken in by a run.
    inbox: std::sync::Mutex<Vec<Evidence>>,
}

struct Engine {
    loaded: bool,
    /// Oldest first by exact end time.
    evidence: VecDeque<Evidence>,
    /// The newest note not cleared, as the record has it.
    note: Option<Note>,
    /// The newest `bir_tries` row read at the load: a try recorded before
    /// it and fed after it is there already.
    loaded_upto: i64,
    /// The sign as last written (or loaded): what the next run starts from.
    sign: SignState,
    /// When the sign was last written.
    written_at: Option<OffsetDateTime>,
}

impl Engine {
    fn new(now: OffsetDateTime) -> Self {
        Self {
            loaded: false,
            evidence: VecDeque::new(),
            note: None,
            loaded_upto: 0,
            sign: SignState::new(now),
            written_at: None,
        }
    }

    /// Takes a try in, in order of its end. One read at the load already
    /// (it was written before) is replaced by this copy, with its account
    /// id and both keys.
    fn take(&mut self, evidence: Evidence, now: OffsetDateTime) {
        if let Some(id) = evidence.id
            && id <= self.loaded_upto
            && let Some(at) = self.evidence.iter().position(|item| item.id == Some(id))
        {
            self.evidence.remove(at);
        }
        let at = self.evidence.partition_point(|item| item.at <= evidence.at);
        self.evidence.insert(at, evidence);
        self.prune(now);
    }

    fn prune(&mut self, now: OffsetDateTime) {
        let oldest = now - sign::EVIDENCE_KEPT;
        while self.evidence.front().is_some_and(|item| item.at < oldest) || self.evidence.len() > MAX_EVIDENCE {
            self.evidence.pop_front();
        }
    }
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
        Self {
            sign: RwLock::new(Arc::new(SignState::new(now))),
            last_ten: RwLock::new([Tally::default(); 2]),
            engine: Mutex::new(Engine::new(now)),
            inbox: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// The current sign, from memory. Before the first run after start it
    /// is No recent news with `seq` 0, which claims nothing.
    pub fn sign(&self) -> Arc<SignState> {
        self.sign.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    fn show(&self, state: SignState) {
        *self.sign.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(state);
    }

    /// How many of the mailbox's last 10 got in, at the last run.
    pub fn last_ten(&self) -> Tally {
        self.last_ten.read().unwrap_or_else(|poisoned| poisoned.into_inner())[1]
    }

    /// Puts a recorded try in the inbox, for the next run: never waits.
    pub fn offer(&self, evidence: Evidence) {
        let mut inbox = self.inbox.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if inbox.len() >= MAX_INBOX {
            inbox.remove(0);
            tracing::error!("BIR uptime: too many tries wait for the rules; the oldest is dropped");
        }
        inbox.push(evidence);
    }

    fn take_inbox(&self) -> Vec<Evidence> {
        std::mem::take(&mut *self.inbox.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }
}

/// `work`, given at most [`DB_TIMEOUT`].
pub async fn bounded<T>(work: impl Future<Output = sqlx::Result<T>>) -> sqlx::Result<T> {
    match tokio::time::timeout(DB_TIMEOUT, work).await {
        Ok(result) => result,
        Err(_) => Err(sqlx::Error::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "BIR uptime: the database took too long",
        ))),
    }
}

/// The engine, locked, for one change: the notes and the relay resume
/// take it to write their change and its audit line in one transaction.
pub struct Locked<'a> {
    uptime: &'a Uptime,
    engine: MutexGuard<'a, Engine>,
    now: OffsetDateTime,
    wrote: bool,
}

/// Locks the engine (loading it from the record first, once) for a run at
/// `now` (never before the last run), and takes in the tries waiting.
pub async fn lock<'a>(uptime: &'a Uptime, db: &PgPool, now: OffsetDateTime) -> sqlx::Result<Locked<'a>> {
    let mut engine = uptime.live.engine.lock().await;
    if engine.loaded {
        bounded(refresh(uptime, &mut engine, db, now)).await?;
    }
    if !engine.loaded {
        bounded(load(&mut engine, db, now)).await?;
    }
    let now = now.max(engine.sign.evaluated_at);
    for evidence in uptime.live.take_inbox() {
        engine.take(evidence, now);
    }
    engine.prune(now);
    Ok(Locked { uptime, engine, now, wrote: false })
}

impl Locked<'_> {
    pub fn now(&self) -> OffsetDateTime {
        self.now
    }

    /// The newest note the record has not cleared.
    pub fn note(&self) -> Option<&Note> {
        self.engine.note.as_ref()
    }

    /// The sign the next run starts from.
    pub fn previous(&self) -> &SignState {
        &self.engine.sign
    }

    /// The rules, with `note` as the open note and whether an Admin resumes
    /// the relay.
    pub fn evaluate(&mut self, note: Option<&Note>, resume_relay: bool) -> Evaluation {
        let (current, previous) = self.uptime.keys.epochs(self.now);
        let periods: Vec<i32> = std::iter::once(current).chain(previous).collect();
        let engine = &mut *self.engine;
        let evidence = engine.evidence.make_contiguous();
        let inputs = sign::Inputs { evidence, periods: &periods, note, resume_relay, now: self.now };
        sign::evaluate(&engine.sign, &inputs)
    }

    /// Writes what `evaluation` changed: a gap in watching, its events (an
    /// Admin's with `actor`, the audit line), a note that ended, and the
    /// sign.
    pub async fn write(
        &mut self,
        tx: &mut Transaction<'_, Postgres>,
        evaluation: &Evaluation,
        actor: Option<&Account>,
    ) -> sqlx::Result<Vec<Written>> {
        sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(SIGN_LOCK).execute(&mut **tx).await?;
        // Another process wrote since this engine read the sign, unless the
        // write found is this very run's (its commit landed, its
        // acknowledgement was lost): the same `seq` and `revision`.
        let stored: Option<(i64, Option<i64>)> =
            sqlx::query_as("SELECT seq, (state->>'revision')::bigint FROM bir_sign WHERE id = 1")
                .fetch_optional(&mut **tx)
                .await?;
        let ours = |seq: i64, revision: Option<i64>| {
            seq == evaluation.state.seq && revision == Some(evaluation.state.revision)
        };
        if stored.is_some_and(|(seq, revision)| seq > self.engine.sign.seq && !ours(seq, revision)) {
            return Err(sqlx::Error::Io(std::io::Error::other(Moved)));
        }
        let mut rows: Vec<(OffsetDateTime, Event, Option<&Account>)> = Vec::new();
        if let Some((from, to)) = evaluation.not_watched {
            let event = |from: &str, to: &str, reason: String| Event {
                scope: Scope::Watch,
                from: from.into(),
                to: to.into(),
                cause: Cause::NotWatched,
                reason,
            };
            let minutes = (to_minute(to) - to_minute(from)).whole_minutes();
            rows.push((from, event("watched", "not_watched", "Buwiz stopped watching".into()), None));
            rows.push((to, event("not_watched", "watched", format!("Buwiz was not watching for {minutes} min")), None));
        }
        for event in &evaluation.events {
            rows.push((self.now, event.clone(), actor.filter(|_| event.cause.by_staff())));
        }
        let mut written = Vec::new();
        for (ord, (at, event, actor)) in rows.into_iter().enumerate() {
            let id = insert_event(tx, to_minute(at), &event, actor, evaluation.state.seq, ord).await?;
            written.push(Written { id, at: to_minute(at), event });
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
        self.wrote = true;
        Ok(written)
    }

    /// Shows `evaluation` once it is written (or needed no write), with
    /// `note` the open note it was run with, and queues the staff mail for
    /// what changed.
    pub fn show(mut self, evaluation: Evaluation, note: Option<Note>, written: &[Written]) {
        self.engine.note = match evaluation.note_ended {
            Some((id, _)) if note.as_ref().is_some_and(|note| note.id == id) => None,
            _ => note,
        };
        if self.wrote {
            self.engine.written_at = Some(self.now);
        }
        for alert in Alert::for_run(written, &evaluation.state) {
            self.uptime.alerts.push(alert);
        }
        for notice in &evaluation.notices {
            self.uptime.alerts.push(Alert::notice(notice, self.now));
        }
        *self.uptime.live.last_ten.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = evaluation.last_ten;
        self.engine.sign = evaluation.state.clone();
        self.uptime.live.show(evaluation.state);
    }

    /// One run with the open note as the record has it: written when
    /// something changed (or a minute after the last write), then shown.
    async fn run(mut self, db: &PgPool) -> sqlx::Result<()> {
        let note = self.engine.note.clone();
        let evaluation = self.evaluate(note.as_ref(), false);
        let due = self.engine.written_at.is_none_or(|at| self.now - at >= WRITE_EVERY || self.now < at);
        let written = if evaluation.must_write(&self.engine.sign) || due {
            let work = async {
                let mut tx = db.begin().await?;
                let written = self.write(&mut tx, &evaluation, None).await?;
                tx.commit().await?;
                Ok(written)
            };
            bounded(work).await?
        } else {
            Vec::new()
        };
        self.show(evaluation, note, &written);
        Ok(())
    }
}

/// One `bir_events` row, once: written again for the same `seq` and place,
/// it is the row already there.
async fn insert_event(
    tx: &mut Transaction<'_, Postgres>,
    at: OffsetDateTime,
    event: &Event,
    actor: Option<&Account>,
    seq: i64,
    ord: usize,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "INSERT INTO bir_events (at, scope, from_state, to_state, cause, reason, actor_id, actor_email, sign_seq, ord)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (sign_seq, ord) DO UPDATE SET sign_seq = EXCLUDED.sign_seq
         RETURNING id",
    )
    .bind(at)
    .bind(event.scope.as_str())
    .bind(&event.from)
    .bind(&event.to)
    .bind(event.cause.as_str())
    .bind(&event.reason)
    .bind(actor.map(|actor| actor.id))
    .bind(actor.map(|actor| actor.email.as_str()))
    .bind(seq)
    .bind(i16::try_from(ord).unwrap_or(i16::MAX))
    .fetch_one(&mut **tx)
    .await
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
    .bind(to_minute(state.evaluated_at))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// A try just recorded: into the inbox at once, then the rules run with
/// it. Called by the recorder, after the app has its result. A run that
/// fails leaves the try in memory for the next.
pub async fn feed(uptime: &Uptime, db: &PgPool, evidence: Evidence, now: OffsetDateTime) -> sqlx::Result<()> {
    uptime.live.offer(evidence);
    lock(uptime, db, now).await?.run(db).await
}

/// The rules with no new try: quiet hours, a note running out, the dwell,
/// and the tries waiting in the inbox.
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

/// The stored sign, as far as it reads. One a later release wrote (more
/// fields) or an older one (fewer) reads whole; a time it lacks is its last
/// write's (never 1970). One that cannot be read at
/// all starts again, keeping its note and its `seq` (which only grows), and
/// its last write as the start of a gap in watching.
fn restore(seq: i64, value: Value, updated_at: OffsetDateTime, now: OffsetDateTime) -> (SignState, bool) {
    match serde_json::from_value::<SignState>(value.clone()) {
        Ok(mut state) => {
            state.seq = state.seq.max(seq);
            let written = updated_at.min(now);
            for at in [&mut state.since, &mut state.evaluated_at, &mut state.dispatcher.since, &mut state.upload.since]
            {
                if *at == OffsetDateTime::UNIX_EPOCH {
                    *at = written;
                }
            }
            if state.relay.paused {
                // Nothing in this release resumes it.
                tracing::warn!("BIR uptime: the stored sign says the relay is paused; this release never pauses it");
                state.relay.paused = false;
            }
            (state, false)
        }
        Err(error) => {
            tracing::error!(%error, "BIR uptime: the stored sign could not be read; starting from No recent news");
            let note = value.get("note").and_then(|note| serde_json::from_value::<Note>(note.clone()).ok());
            let mut state = SignState { seq: seq + 1, note, evaluated_at: updated_at.min(now), ..SignState::new(now) };
            let in_force = state.note.as_ref().filter(|note| note.in_force(now));
            state.state = sign::overall(Light::NoRecentNews, Light::NoRecentNews, in_force);
            (state, true)
        }
    }
}

/// The account ids a sign read back from the record lacks (they are never
/// written), from the sign this engine held: a voter or a lost vote is the
/// same account when they share a key.
fn keep_account_ids(held: &SignState, read: &mut SignState) {
    for (old, new) in [(&held.dispatcher, &mut read.dispatcher), (&held.upload, &mut read.upload)] {
        for voter in &mut new.quorum {
            voter.account_id = old
                .quorum
                .iter()
                .find(|known| known.keys.iter().any(|keys| voter.keys.contains(keys)))
                .and_then(|known| known.account_id);
        }
    }
    for loss in &mut read.vote_losses {
        loss.account_id = held
            .vote_losses
            .iter()
            .find(|known| known.side == loss.side && known.keys.iter().any(|keys| loss.keys.contains(keys)))
            .and_then(|known| known.account_id);
    }
}

/// The sign as stored, read as a start would read it, and the newest note
/// not cleared; nothing is written. `None` before the first start.
pub async fn stored(db: &PgPool, now: OffsetDateTime) -> sqlx::Result<Option<(SignState, Option<Note>)>> {
    let row: Option<(i64, Value, OffsetDateTime)> =
        sqlx::query_as("SELECT seq, state, updated_at FROM bir_sign WHERE id = 1").fetch_optional(db).await?;
    let Some((seq, value, updated_at)) = row else {
        return Ok(None);
    };
    Ok(Some((restore(seq, value, updated_at, now).0, open_note(db).await?)))
}

/// The newest note the record has not cleared.
async fn open_note(db: &PgPool) -> sqlx::Result<Option<Note>> {
    let note: Option<(Uuid, String, String, OffsetDateTime, OffsetDateTime)> = sqlx::query_as(
        "SELECT id, kind, text, starts_at, until FROM bir_notes WHERE cleared_at IS NULL
          ORDER BY starts_at DESC LIMIT 1",
    )
    .fetch_optional(db)
    .await?;
    Ok(note.and_then(|(id, kind, text, starts_at, until)| {
        Some(Note { id, kind: NoteKind::parse(&kind)?, text, starts_at, until })
    }))
}

/// Takes in what another process wrote (a note set or cleared with the
/// `buwiz-cloud bir` command): when the stored `seq` is higher than this
/// engine's, its sign and the open note replace this engine's (the tries in
/// memory stay), and the staff mail hears of its events not mailed yet. A
/// stored sign that cannot be read is loaded again from the start.
async fn refresh(uptime: &Uptime, engine: &mut Engine, db: &PgPool, now: OffsetDateTime) -> sqlx::Result<()> {
    let stored: Option<i64> = sqlx::query_scalar("SELECT seq FROM bir_sign WHERE id = 1").fetch_optional(db).await?;
    let Some(stored) = stored.filter(|stored| *stored > engine.sign.seq) else {
        return Ok(());
    };
    let (seq, value, updated_at): (i64, Value, OffsetDateTime) =
        sqlx::query_as("SELECT seq, state, updated_at FROM bir_sign WHERE id = 1").fetch_one(db).await?;
    let (mut state, reset) = restore(seq, value, updated_at, now);
    if reset {
        engine.loaded = false;
        return Ok(());
    }
    keep_account_ids(&engine.sign, &mut state);
    let rows: Vec<(i64, OffsetDateTime, String, String, String, String, String)> = sqlx::query_as(
        "SELECT id, at, scope, from_state, to_state, cause, reason FROM bir_events
          WHERE sign_seq > $1 AND sign_seq <= $2 AND mailed_at IS NULL ORDER BY sign_seq, ord",
    )
    .bind(engine.sign.seq)
    .bind(seq)
    .fetch_all(db)
    .await?;
    let written: Vec<Written> = rows
        .into_iter()
        .filter_map(|(id, at, scope, from, to, cause, reason)| {
            let event = Event { scope: Scope::parse(&scope)?, from, to, cause: Cause::parse(&cause)?, reason };
            Some(Written { id, at, event })
        })
        .collect();
    engine.note = open_note(db).await?;
    for alert in Alert::for_run(&written, &state) {
        uptime.alerts.push(alert);
    }
    tracing::info!(from = engine.sign.seq, to = stored, "BIR uptime: took in a sign another process wrote");
    engine.sign = state.clone();
    uptime.live.show(state);
    Ok(())
}

async fn load(engine: &mut Engine, db: &PgPool, now: OffsetDateTime) -> sqlx::Result<()> {
    let row: Option<(i64, Value, OffsetDateTime)> =
        sqlx::query_as("SELECT seq, state, updated_at FROM bir_sign WHERE id = 1").fetch_optional(db).await?;
    let rows: Vec<TryRow> = sqlx::query_as(
        "SELECT id, ended_at, source, outcome, stage, dispatcher, upload, failure_kind, counted, late, admission,
                key_epoch, account_key, network_key
           FROM bir_tries WHERE mode = 'live' AND ended_at >= $1 ORDER BY ended_at, id",
    )
    .bind(to_minute(now - sign::EVIDENCE_KEPT))
    .fetch_all(db)
    .await?;
    let note = open_note(db).await?;
    let state = match row {
        Some((seq, value, updated_at)) => {
            let (state, reset) = restore(seq, value, updated_at, now);
            if reset {
                let mut tx = db.begin().await?;
                let event = Event {
                    scope: Scope::Overall,
                    from: "unknown".into(),
                    to: state.state.as_str().into(),
                    cause: Cause::Reset,
                    reason: "The stored sign could not be read; it starts again from what the note in force says, or No recent news".into(),
                };
                insert_event(&mut tx, to_minute(now), &event, None, state.seq, 0).await?;
                write_sign(&mut tx, &state).await?;
                tx.commit().await?;
            }
            state
        }
        None => {
            // The first start: nothing known, and the history begins here.
            let mut state = SignState { seq: 1, ..SignState::new(now) };
            let (success, news): (Option<OffsetDateTime>, Option<OffsetDateTime>) = sqlx::query_as(
                "SELECT max(ended_at) FILTER (WHERE source = 'relay' AND outcome = 'sent'), max(ended_at)
                   FROM bir_tries WHERE counted",
            )
            .fetch_one(db)
            .await?;
            state.last_success_at = success;
            state.last_news_at = news;
            let mut tx = db.begin().await?;
            let event = Event {
                scope: Scope::Overall,
                from: "none".into(),
                to: state.state.as_str().into(),
                cause: Cause::Rule,
                reason: "Buwiz began keeping the sign".into(),
            };
            insert_event(&mut tx, to_minute(now), &event, None, state.seq, 0).await?;
            write_sign(&mut tx, &state).await?;
            tx.commit().await?;
            state
        }
    };
    engine.loaded_upto = rows.last().map_or(0, |row| row.0);
    engine.evidence = rows.into_iter().map(evidence_of).collect();
    engine.note = note;
    engine.sign = state;
    engine.written_at = None;
    engine.loaded = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;
    use crate::uptime::classify::{Outcome, Stage, Window};

    fn evidence(at: OffsetDateTime, id: Option<i64>) -> Evidence {
        Evidence {
            id,
            at,
            source: Source::Relay,
            admission: Admission::Open,
            outcome: Outcome::Failed,
            stage: Stage::Upload,
            dispatcher: Window::Ok,
            upload: Window::Fail,
            kind: None,
            counted: true,
            late: false,
            keys: Vec::new(),
            account_id: None,
            port: None,
        }
    }

    #[test]
    fn the_engine_keeps_tries_in_order_of_their_end_and_at_most_max_evidence() {
        let now = datetime!(2026-11-11 05:00 UTC);
        let mut engine = Engine::new(now);
        // Out of order: kept by when they ended.
        for seconds in [30, 10, 20, 10] {
            engine.take(evidence(now - Duration::seconds(seconds), None), now);
        }
        let ats: Vec<i64> = engine.evidence.iter().map(|item| (now - item.at).whole_seconds()).collect();
        assert_eq!(ats, [30, 20, 10, 10]);
        // Older than 3 hours goes.
        engine.take(evidence(now - sign::EVIDENCE_KEPT - Duration::seconds(1), None), now);
        assert_eq!(engine.evidence.len(), 4);
        // A try read at the load and fed after it is there once.
        engine.loaded_upto = 7;
        engine.take(evidence(now - Duration::minutes(5), Some(7)), now);
        engine.take(evidence(now - Duration::minutes(5), Some(7)), now);
        assert_eq!(engine.evidence.iter().filter(|item| item.id == Some(7)).count(), 1);
        // At most MAX_EVIDENCE: the oldest go first.
        for n in 0..(MAX_EVIDENCE + 10) {
            engine.take(evidence(now - Duration::minutes(60) + Duration::milliseconds(n as i64), None), now);
        }
        assert_eq!(engine.evidence.len(), MAX_EVIDENCE);
        assert_eq!(engine.evidence.back().unwrap().at, now - Duration::seconds(10));
        assert!(engine.evidence.iter().all(|item| item.at >= now - Duration::minutes(60)));
    }

    #[test]
    fn the_inbox_never_waits_and_drops_only_past_its_cap() {
        let live = Live::new(datetime!(2026-11-11 05:00 UTC));
        for n in 0..(MAX_INBOX + 3) {
            live.offer(evidence(datetime!(2026-11-11 05:00 UTC), Some(n as i64)));
        }
        let taken = live.take_inbox();
        assert_eq!(taken.len(), MAX_INBOX);
        assert_eq!(taken[0].id, Some(3));
        assert!(live.take_inbox().is_empty());
    }

    #[test]
    fn a_stored_sign_that_cannot_be_read_keeps_its_note_and_seq() {
        let now = datetime!(2026-11-11 05:00 UTC);
        let note = serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000abc", "kind": "outage", "text": "Made-up",
            "starts_at": "2026-11-11T04:00:00Z", "until": "2026-11-11T08:00:00Z"
        });
        let (state, reset) =
            restore(41, serde_json::json!({ "state": 12, "note": note }), now - Duration::hours(2), now);
        assert!(reset);
        assert_eq!(state.seq, 42);
        assert_eq!(state.state, Light::Offline, "the outage note in force");
        assert_eq!(state.note.map(|note| note.kind), Some(NoteKind::Outage));
        assert_eq!(state.evaluated_at, now - Duration::hours(2), "the gap starts at its last write");
        // One with every field missing reads, its times from its last write.
        let (state, reset) = restore(5, serde_json::json!({}), now - Duration::minutes(3), now);
        assert!(!reset);
        assert_eq!(
            (state.seq, state.since, state.evaluated_at),
            (5, now - Duration::minutes(3), now - Duration::minutes(3))
        );
        assert_eq!(state.upload.since, now - Duration::minutes(3));
        // One a later release paused: never paused here.
        let mut paused = SignState::new(now);
        paused.relay.paused = true;
        let (state, reset) = restore(3, serde_json::to_value(&paused).unwrap(), now, now);
        assert!(!reset && !state.relay.paused);
    }
}
