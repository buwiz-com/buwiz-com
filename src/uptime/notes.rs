//! What an Admin sets by hand: a note (an outage from the news, planned
//! maintenance, or "ignore evidence until …, declare Online") and resuming
//! sending after the relay paused. Admins only (`Power::SetBirStatus`):
//! checked before the engine's lock is taken, so nobody else ever waits on
//! it, and again inside the transaction, as every staff action is, where
//! the actor's email is read again too. Each writes its audit line, the
//! `bir_events` row with the Admin's id and email, in the same transaction
//! as the change.
//!
//! A note's text is public (the form says so and asks for no personal
//! data), 1 to 200 characters, and it ends at most 72 hours ahead. One note
//! is in force at a time; a note past its end has ended, so it never blocks
//! a new one (it is marked ended, with a `note_expired` line). A note ends
//! when an Admin clears it (with a reason, staff only), when its end
//! passes, or (outage only) when a counted relay return gets in after it
//! began; the end of an outage or maintenance note starts waking. The staff
//! screens come in a later release: these are the functions they call.
//! Nothing in this release pauses the relay, so resuming it always answers
//! "not paused"; it is kept for the later staff page.

use sqlx::{PgPool, Postgres, Transaction};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::Uptime;
use super::live::{self, Locked};
use super::sign::{self, Cause, Evaluation, Event, Note, NoteKind, Scope};
use super::store::to_minute;
use crate::account::accounts::Account;
use crate::account::roles::{self, Power, Staff};
use crate::base::config::Config;

/// The longest note text, and the longest reason.
pub const MAX_TEXT_CHARS: usize = 200;
/// The furthest ahead a note may end.
pub const MAX_AHEAD: Duration = Duration::hours(72);

#[derive(Debug, thiserror::Error)]
pub enum NoteError {
    #[error("only an Admin can do this")]
    AdminOnly,
    #[error("write the note's text; it is public")]
    TextMissing,
    #[error("the note's text is at most 200 characters")]
    TextTooLong,
    #[error("the note must end in the future")]
    EndsInThePast,
    #[error("a note may end at most 72 hours ahead")]
    EndsTooFar,
    #[error("a note is in force; clear it first")]
    NoteInForce,
    #[error("no note is in force")]
    NoNote,
    #[error("sending is not paused")]
    NotPaused,
    #[error("the sign changed meanwhile (another process wrote it); nothing was written: try again")]
    Moved,
    #[error("no sign is kept yet: start the server with BIR_UPTIME at record or above first")]
    NoSign,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// A note to set.
#[derive(Clone, Copy, Debug)]
pub struct NewNote<'a> {
    pub kind: NoteKind,
    /// Public.
    pub text: &'a str,
    pub until: OffsetDateTime,
}

/// The actor as the database stands now, if an Admin: their account with
/// the email read now (a share lock holds it to the end of `tx`).
async fn admin_in(tx: &mut Transaction<'_, Postgres>, config: &Config, actor: &Staff) -> Result<Account, NoteError> {
    let row: Option<(String, Option<String>, bool)> =
        sqlx::query_as("SELECT email, role, banned_at IS NOT NULL FROM accounts WHERE id = $1 FOR SHARE")
            .bind(actor.account.id)
            .fetch_optional(&mut **tx)
            .await?;
    let admin = |(email, role, banned): &(String, Option<String>, bool)| {
        roles::of(config, email, role.as_deref(), *banned).is_some_and(|(role, _)| role.can(Power::SetBirStatus))
    };
    match row {
        Some(row) if admin(&row) => Ok(Account { id: actor.account.id, email: row.0 }),
        _ => Err(NoteError::AdminOnly),
    }
}

/// Before the engine's lock: someone who is not an Admin is refused at
/// once and never waits on it.
async fn admin_first(db: &PgPool, config: &Config, actor: &Staff) -> Result<(), NoteError> {
    let mut tx = live::begin(db).await?;
    let checked = admin_in(&mut tx, config, actor).await.map(drop);
    tx.rollback().await?;
    checked
}

/// The note's text, trimmed, or why it cannot be.
pub fn note_text(text: &str) -> Result<String, NoteError> {
    let text = text.trim();
    if text.is_empty() {
        Err(NoteError::TextMissing)
    } else if text.chars().count() > MAX_TEXT_CHARS {
        Err(NoteError::TextTooLong)
    } else {
        Ok(text.to_owned())
    }
}

/// Makes sure the run carries the Admin's audit line, even when the sign
/// did not change, with `reason` as its sentence.
fn with_audit(mut evaluation: Evaluation, cause: Cause, reason: &str, previous_seq: i64) -> Evaluation {
    match evaluation.events.iter_mut().find(|event| event.cause == cause) {
        Some(event) => event.reason = reason.into(),
        None => {
            let state = evaluation.state.state.as_str().to_owned();
            evaluation.events.push(Event {
                scope: if cause == Cause::RelayResumed { Scope::Relay } else { Scope::Overall },
                from: state.clone(),
                to: state,
                cause,
                reason: reason.into(),
            });
            evaluation.state.seq = sign::next_seq(previous_seq, evaluation.state.evaluated_at);
        }
    }
    evaluation
}

async fn finish(
    mut locked: Locked<'_>,
    mut tx: Transaction<'_, Postgres>,
    evaluation: Evaluation,
    note: Option<Note>,
    actor: &Account,
) -> Result<(), NoteError> {
    let written = locked
        .write(&mut tx, &evaluation, Some(actor))
        .await
        .map_err(|error| if live::moved(&error) { NoteError::Moved } else { error.into() })?;
    tx.commit().await?;
    locked.show(evaluation, note, &written);
    Ok(())
}

/// An Admin sets a note, ending at `until` (at most 72 hours ahead).
/// Refused while another is in force; one that ran out is marked ended.
pub async fn set(
    uptime: &Uptime,
    db: &PgPool,
    config: &Config,
    actor: &Staff,
    new: NewNote<'_>,
    now: OffsetDateTime,
) -> Result<Note, NoteError> {
    set_with_reason(uptime, db, config, actor, new, None, now).await
}

/// [`set`], with `reason` (staff only, 1 to 200 characters) in the audit
/// line after its fixed sentence.
pub async fn set_with_reason(
    uptime: &Uptime,
    db: &PgPool,
    config: &Config,
    actor: &Staff,
    new: NewNote<'_>,
    reason: Option<&str>,
    now: OffsetDateTime,
) -> Result<Note, NoteError> {
    admin_first(db, config, actor).await?;
    let why = reason.map(note_text).transpose()?;
    let mut locked = live::lock(uptime, db, now).await?;
    let now = locked.now();
    let mut tx = live::begin(db).await?;
    // The sign's lock before any note row, as every path takes them.
    live::lock_sign(&mut tx).await?;
    let admin = admin_in(&mut tx, config, actor).await?;
    let (note, expired) = insert_note(&mut tx, &admin, new, now).await?;
    let previous_seq = locked.previous().seq;
    let evaluation = locked.evaluate(Some(&note), false);
    let reason = set_reason(&note, why.as_deref());
    let mut evaluation = with_audit(evaluation, Cause::NoteSet, &reason, previous_seq);
    let state = previous_state(&evaluation);
    for _ in 0..expired {
        evaluation.events.insert(
            0,
            Event {
                scope: Scope::Overall,
                from: state.clone(),
                to: state.clone(),
                cause: Cause::NoteExpired,
                reason: "A note ran out before Buwiz saw it end".into(),
            },
        );
    }
    finish(locked, tx, evaluation, Some(note.clone()), &admin).await?;
    tracing::info!(actor = %admin.id, kind = note.kind.as_str(), "BIR note set");
    Ok(note)
}

/// Checks a new note and writes its row, marking ended the notes that ran
/// out with no run to mark them (at their own end): refused while one is
/// in force. The new note and how many ran out.
async fn insert_note(
    tx: &mut Transaction<'_, Postgres>,
    admin: &Account,
    new: NewNote<'_>,
    now: OffsetDateTime,
) -> Result<(Note, usize), NoteError> {
    let text = note_text(new.text)?;
    if new.until <= now {
        return Err(NoteError::EndsInThePast);
    }
    if new.until > now + MAX_AHEAD {
        return Err(NoteError::EndsTooFar);
    }
    let open: Vec<(Uuid, OffsetDateTime)> =
        sqlx::query_as("SELECT id, until FROM bir_notes WHERE cleared_at IS NULL FOR UPDATE")
            .fetch_all(&mut **tx)
            .await?;
    if open.iter().any(|(_, until)| *until > now) {
        return Err(NoteError::NoteInForce);
    }
    let expired: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE bir_notes SET cleared_at = until, cleared_how = 'expired' WHERE cleared_at IS NULL RETURNING id",
    )
    .fetch_all(&mut **tx)
    .await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO bir_notes (kind, text, starts_at, until, set_by, set_by_email)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(new.kind.as_str())
    .bind(&text)
    .bind(now)
    .bind(new.until)
    .bind(admin.id)
    .bind(&admin.email)
    .fetch_one(&mut **tx)
    .await?;
    Ok((Note { id, kind: new.kind, text, starts_at: now, until: new.until }, expired.len()))
}

/// The audit line's sentence for a note set, with the Admin's reason.
fn set_reason(note: &Note, why: Option<&str>) -> String {
    let reason = match note.kind {
        NoteKind::Outage => "An Admin set an outage note".to_owned(),
        NoteKind::Maintenance => "An Admin set a planned maintenance note".to_owned(),
        NoteKind::Online => format!("An Admin declared the BIR online, ignoring evidence: {}", note.text),
    };
    match why {
        Some(why) => format!("{reason} (why: {why})"),
        None => reason,
    }
}

/// The overall state before the run: where its first overall line starts.
fn previous_state(evaluation: &Evaluation) -> String {
    evaluation
        .events
        .iter()
        .find(|event| event.scope == Scope::Overall)
        .map_or_else(|| evaluation.state.state.as_str().to_owned(), |event| event.from.clone())
}

/// "Ignore evidence until …": an Admin declares the BIR online until
/// `until`, with `text` (public) as the reason. Evidence is still recorded
/// and shown to staff.
pub async fn declare_online(
    uptime: &Uptime,
    db: &PgPool,
    config: &Config,
    actor: &Staff,
    text: &str,
    until: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<Note, NoteError> {
    set(uptime, db, config, actor, NewNote { kind: NoteKind::Online, text, until }, now).await
}

/// An Admin clears the note in force, saying why (1 to 200 characters,
/// seen by staff only).
pub async fn clear(
    uptime: &Uptime,
    db: &PgPool,
    config: &Config,
    actor: &Staff,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), NoteError> {
    admin_first(db, config, actor).await?;
    let reason = note_text(reason)?;
    let mut locked = live::lock(uptime, db, now).await?;
    let now = locked.now();
    let mut tx = live::begin(db).await?;
    live::lock_sign(&mut tx).await?;
    let admin = admin_in(&mut tx, config, actor).await?;
    clear_note(&mut tx, &admin, &reason, now).await?;
    let previous_seq = locked.previous().seq;
    let evaluation = locked.evaluate(None, false);
    let evaluation =
        with_audit(evaluation, Cause::NoteCleared, &format!("An Admin cleared the note: {reason}"), previous_seq);
    finish(locked, tx, evaluation, None, &admin).await?;
    tracing::info!(actor = %admin.id, "BIR note cleared");
    Ok(())
}

/// Marks the note in force cleared by `admin`, with `reason`.
async fn clear_note(
    tx: &mut Transaction<'_, Postgres>,
    admin: &Account,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), NoteError> {
    let cleared = sqlx::query(
        "UPDATE bir_notes SET cleared_at = $1, cleared_by = $2, cleared_by_email = $3, cleared_how = 'staff',
                              cleared_reason = $4
          WHERE cleared_at IS NULL AND until > $1",
    )
    .bind(now)
    .bind(admin.id)
    .bind(&admin.email)
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    if cleared.rows_affected() == 0 {
        return Err(NoteError::NoNote);
    }
    Ok(())
}

/// For a process that is not the server (the `buwiz-cloud bir` command):
/// an Admin sets a note as [`set_with_reason`] checks it, but only the note
/// row, its audit line and the sign's `seq` are written, under the sign's
/// lock. The rules are not run here: the running server takes the note in
/// at its next run (or when it next starts), makes the change, writes it
/// and mails staff. So this can neither end an Offline nor write over what
/// the server holds and has not written yet.
pub async fn record_set(
    db: &PgPool,
    config: &Config,
    actor: &Staff,
    new: NewNote<'_>,
    reason: Option<&str>,
    now: OffsetDateTime,
) -> Result<Note, NoteError> {
    admin_first(db, config, actor).await?;
    let why = reason.map(note_text).transpose()?;
    let mut tx = live::begin(db).await?;
    live::lock_sign(&mut tx).await?;
    let admin = admin_in(&mut tx, config, actor).await?;
    let (seq, state) = stored_seq(&mut tx).await?;
    let (note, expired) = insert_note(&mut tx, &admin, new, now).await?;
    let seq = sign::next_seq(seq, now);
    let line = |cause: Cause, reason: String| Event {
        scope: Scope::Overall,
        from: state.clone(),
        to: state.clone(),
        cause,
        reason,
    };
    let mut lines: Vec<(Event, Option<&Account>)> = (0..expired)
        .map(|_| (line(Cause::NoteExpired, "A note ran out before Buwiz saw it end".into()), None))
        .collect();
    lines.push((line(Cause::NoteSet, set_reason(&note, why.as_deref())), Some(&admin)));
    record_lines(&mut tx, &lines, seq, now).await?;
    tx.commit().await?;
    tracing::info!(actor = %admin.id, kind = note.kind.as_str(), "BIR note recorded for the server");
    Ok(note)
}

/// For a process that is not the server: an Admin clears the note in force
/// as [`clear`] checks it, writing only the note row, its audit line and
/// the sign's `seq` (see [`record_set`]).
pub async fn record_clear(
    db: &PgPool,
    config: &Config,
    actor: &Staff,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), NoteError> {
    admin_first(db, config, actor).await?;
    let reason = note_text(reason)?;
    let mut tx = live::begin(db).await?;
    live::lock_sign(&mut tx).await?;
    let admin = admin_in(&mut tx, config, actor).await?;
    let (seq, state) = stored_seq(&mut tx).await?;
    clear_note(&mut tx, &admin, &reason, now).await?;
    let line = Event {
        scope: Scope::Overall,
        from: state.clone(),
        to: state,
        cause: Cause::NoteCleared,
        reason: format!("An Admin cleared the note: {reason}"),
    };
    record_lines(&mut tx, &[(line, Some(&admin))], sign::next_seq(seq, now), now).await?;
    tx.commit().await?;
    tracing::info!(actor = %admin.id, "BIR note cleared for the server");
    Ok(())
}

/// The stored sign's `seq` and overall state, under the sign's lock.
async fn stored_seq(tx: &mut Transaction<'_, Postgres>) -> Result<(i64, String), NoteError> {
    let row: Option<(i64, Option<String>)> =
        sqlx::query_as("SELECT seq, state->>'state' FROM bir_sign WHERE id = 1").fetch_optional(&mut **tx).await?;
    let (seq, state) = row.ok_or(NoteError::NoSign)?;
    Ok((seq, state.unwrap_or_else(|| "no_recent_news".into())))
}

/// Writes `lines` under `seq` (their places in order) and moves the stored
/// `seq` to it, leaving the stored sign itself as the server wrote it.
async fn record_lines(
    tx: &mut Transaction<'_, Postgres>,
    lines: &[(Event, Option<&Account>)],
    seq: i64,
    now: OffsetDateTime,
) -> Result<(), NoteError> {
    for (ord, (event, actor)) in lines.iter().enumerate() {
        live::insert_event(tx, to_minute(now), event, *actor, seq, ord).await?;
    }
    sqlx::query("UPDATE bir_sign SET seq = GREATEST(seq, $1) WHERE id = 1").bind(seq).execute(&mut **tx).await?;
    Ok(())
}

/// An Admin resumes sending after the relay paused (once the fault is
/// fixed: a key pinned from a second source, or the fault gone). Faults
/// from before count no more. For the later staff page: nothing in this
/// release pauses it.
pub async fn resume_relay(
    uptime: &Uptime,
    db: &PgPool,
    config: &Config,
    actor: &Staff,
    now: OffsetDateTime,
) -> Result<(), NoteError> {
    admin_first(db, config, actor).await?;
    let mut locked = live::lock(uptime, db, now).await?;
    let mut tx = live::begin(db).await?;
    let admin = admin_in(&mut tx, config, actor).await?;
    if !locked.previous().relay.paused {
        return Err(NoteError::NotPaused);
    }
    let note = locked.note().cloned();
    let evaluation = locked.evaluate(note.as_ref(), true);
    finish(locked, tx, evaluation, note, &admin).await?;
    tracing::info!(actor = %admin.id, "BIR relay resumed");
    Ok(())
}
