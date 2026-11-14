//! What an Admin sets by hand: a note (an outage from the news, planned
//! maintenance, or "ignore evidence until …, declare Online") and resuming
//! sending after the relay paused. Admins only (`Power::SetBirStatus`,
//! rechecked in the transaction as every staff action is). Each writes its
//! audit line, the `bir_events` row with the Admin's id and email, in the
//! same transaction as the change.
//!
//! A note's text is public (the form says so and asks for no personal
//! data), 1 to 200 characters, and it ends at most 72 hours ahead. One note
//! is in force at a time; a note past its end has ended, so it never blocks
//! a new one. A note ends when an Admin clears it, when its end passes, or
//! (outage and maintenance) when a counted relay return gets in; the end of
//! an outage or maintenance note starts waking. The staff screens come in a
//! later release: these are the functions they call.

use sqlx::{PgPool, Postgres, Transaction};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::Uptime;
use super::live::{self, Locked};
use super::sign::{Cause, Evaluation, Event, Note, NoteKind, Scope};
use crate::account::roles::{self, Power, Staff};
use crate::base::config::Config;

/// The longest note text.
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

/// Serialises note changes across processes, for the rest of the
/// transaction.
async fn lock_notes(tx: &mut Transaction<'_, Postgres>) -> sqlx::Result<()> {
    // "birnotes" in ASCII.
    sqlx::query("SELECT pg_advisory_xact_lock(7091318736213665139)").execute(&mut **tx).await?;
    Ok(())
}

async fn admin_only(tx: &mut Transaction<'_, Postgres>, config: &Config, actor: &Staff) -> Result<(), NoteError> {
    if roles::may(tx, config, actor.account.id, Power::SetBirStatus).await? {
        Ok(())
    } else {
        Err(NoteError::AdminOnly)
    }
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
/// did not change.
fn with_audit(mut evaluation: Evaluation, cause: Cause, reason: &str, previous_seq: i64) -> Evaluation {
    if !evaluation.events.iter().any(|event| event.cause == cause) {
        let state = evaluation.state.state.as_str().to_owned();
        evaluation.events.push(Event {
            scope: if cause == Cause::RelayResumed { Scope::Relay } else { Scope::Overall },
            from: state.clone(),
            to: state,
            cause,
            reason: reason.into(),
        });
        evaluation.state.seq = previous_seq + 1;
    }
    evaluation
}

async fn finish(
    mut locked: Locked<'_>,
    mut tx: Transaction<'_, Postgres>,
    evaluation: Evaluation,
    note: Option<Note>,
    actor: &Staff,
) -> Result<(), NoteError> {
    let written = locked.write(&mut tx, &evaluation, Some(&actor.account)).await?;
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
    let mut locked = live::lock(uptime, db, now).await?;
    let now = locked.now();
    let mut tx = db.begin().await?;
    lock_notes(&mut tx).await?;
    admin_only(&mut tx, config, actor).await?;
    let text = note_text(new.text)?;
    if new.until <= now {
        return Err(NoteError::EndsInThePast);
    }
    if new.until > now + MAX_AHEAD {
        return Err(NoteError::EndsTooFar);
    }
    let open: Vec<(Uuid, OffsetDateTime)> =
        sqlx::query_as("SELECT id, until FROM bir_notes WHERE cleared_at IS NULL FOR UPDATE")
            .fetch_all(&mut *tx)
            .await?;
    if open.iter().any(|(_, until)| *until > now) {
        return Err(NoteError::NoteInForce);
    }
    // Notes that ran out unseen end at their own time.
    sqlx::query("UPDATE bir_notes SET cleared_at = until, cleared_how = 'expired' WHERE cleared_at IS NULL")
        .execute(&mut *tx)
        .await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO bir_notes (kind, text, starts_at, until, set_by, set_by_email)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(new.kind.as_str())
    .bind(&text)
    .bind(now)
    .bind(new.until)
    .bind(actor.account.id)
    .bind(&actor.account.email)
    .fetch_one(&mut *tx)
    .await?;
    let note = Note { id, kind: new.kind, text, starts_at: now, until: new.until };
    let previous_seq = locked.previous().seq;
    let evaluation = locked.evaluate(Some(&note), false);
    let evaluation = with_audit(evaluation, Cause::NoteSet, "An Admin set a note", previous_seq);
    finish(locked, tx, evaluation, Some(note.clone()), actor).await?;
    tracing::info!(actor = %actor.account.id, kind = note.kind.as_str(), "BIR note set");
    Ok(note)
}

/// "Ignore evidence until …": an Admin declares the BIR online until
/// `until`. Evidence is still recorded and shown to staff.
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

/// An Admin clears the note in force.
pub async fn clear(
    uptime: &Uptime,
    db: &PgPool,
    config: &Config,
    actor: &Staff,
    now: OffsetDateTime,
) -> Result<(), NoteError> {
    let mut locked = live::lock(uptime, db, now).await?;
    let now = locked.now();
    let mut tx = db.begin().await?;
    lock_notes(&mut tx).await?;
    admin_only(&mut tx, config, actor).await?;
    let cleared = sqlx::query(
        "UPDATE bir_notes SET cleared_at = $1, cleared_by = $2, cleared_by_email = $3, cleared_how = 'staff'
          WHERE cleared_at IS NULL AND until > $1",
    )
    .bind(now)
    .bind(actor.account.id)
    .bind(&actor.account.email)
    .execute(&mut *tx)
    .await?;
    if cleared.rows_affected() == 0 {
        return Err(NoteError::NoNote);
    }
    let previous_seq = locked.previous().seq;
    let evaluation = locked.evaluate(None, false);
    let evaluation = with_audit(evaluation, Cause::NoteCleared, "An Admin cleared the note", previous_seq);
    finish(locked, tx, evaluation, None, actor).await?;
    tracing::info!(actor = %actor.account.id, "BIR note cleared");
    Ok(())
}

/// An Admin resumes sending after the relay paused (once the fault is
/// fixed: a key pinned from a second source, or the fault gone). Faults
/// from before count no more.
pub async fn resume_relay(
    uptime: &Uptime,
    db: &PgPool,
    config: &Config,
    actor: &Staff,
    now: OffsetDateTime,
) -> Result<(), NoteError> {
    let mut locked = live::lock(uptime, db, now).await?;
    let mut tx = db.begin().await?;
    admin_only(&mut tx, config, actor).await?;
    if !locked.previous().relay.paused {
        return Err(NoteError::NotPaused);
    }
    let note = locked.note().cloned();
    let evaluation = locked.evaluate(note.as_ref(), true);
    finish(locked, tx, evaluation, note, actor).await?;
    tracing::info!(actor = %actor.account.id, "BIR relay resumed");
    Ok(())
}
