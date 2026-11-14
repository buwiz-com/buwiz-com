//! `buwiz-cloud bir`: the manual switch (AMENDMENTS D5) for an Admin,
//! before the staff page exists. See [`USAGE`] and `docs/deploy.md`.
//!
//! - `bir declare online|offline|maintenance`: sets a hand-set note ("declare
//!   Online", an outage, planned maintenance) through the same function the
//!   staff page will call ([`notes::set_with_reason`]): Admins only, checked
//!   on the database as it stands, text public (1 to 200 characters), an end
//!   in the future and at most 72 hours ahead, refused while another note is
//!   in force, and the same audit line (`bir_events`, with the Admin's id
//!   and email and the reason).
//! - `bir clear`: ends the note in force ([`notes::clear`]), with a reason.
//! - `bir status`: prints the stored sign and the note; writes nothing.
//!
//! It runs on the instance with the service's own settings (`DATABASE_URL`,
//! `ADMIN_EMAILS`, `BIR_UPTIME`), runs no migration and refuses a database
//! whose migrations are not this release's. Before it writes it prints the
//! database, who it acts as, what it will do and the sign now; after, the
//! sign as it then stands. It never prints a TIN (nothing it reads holds
//! one).
//!
//! The running server takes the change in by itself, without a restart:
//! every run of its rules (at least every 30 seconds) reads the stored
//! sign's `seq`, and a higher one, written by this command, replaces its
//! sign and note ([`super::live`]). The server also mails staff about it.

use std::io::Write;
use std::process::ExitCode;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::{Duration, OffsetDateTime, PrimitiveDateTime};

use super::keys::MANILA;
use super::live;
use super::notes::{self, MAX_AHEAD, NewNote, NoteError};
use super::public::{self, Times};
use super::sign::{Note, NoteKind, SignState};
use super::{DEFAULT_UPLOAD_PORTS, Level, Uptime};
use crate::account::accounts::{self, Account};
use crate::account::roles::{self, Power, Staff};
use crate::base::config::Config;

pub const USAGE: &str = "usage: buwiz-cloud bir declare online|offline|maintenance --as <admin email> --reason <text>
           (--until <time> | --hours <n>) [--text <public note>]
       buwiz-cloud bir clear --as <admin email> --reason <text>
       buwiz-cloud bir status

Run it with the server's own settings (DATABASE_URL, ADMIN_EMAILS, BIR_UPTIME): see docs/deploy.md.
  declare online       the sign reads Online whatever the tries say (the lights are still kept)
  declare offline      an outage: the sign reads Offline until it ends or a return gets in
  declare maintenance  planned maintenance: the sign reads Offline until it ends
  --as E               an Admin's login; the audit line names it
  --reason R           why, 1 to 200 characters: staff only (the audit line and staff mail)
  --until T            when it ends: 2026-11-14T18:00 (Manila) or RFC 3339; at most 72 hours ahead
  --hours N            or N whole hours from now, 1 to 72
  --text T             what people and apps read, 1 to 200 characters, public: no personal data
                       (without it, a plain sentence for the kind)";

/// What to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Declare { kind: NoteKind, actor: String, reason: String, until: Until, text: Option<String> },
    Clear { actor: String, reason: String },
    Status,
}

/// When a declared note ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Until {
    At(OffsetDateTime),
    Hours(i64),
}

impl Until {
    fn at(self, now: OffsetDateTime) -> OffsetDateTime {
        match self {
            Self::At(at) => at,
            Self::Hours(hours) => now + Duration::hours(hours),
        }
    }
}

/// `2026-11-14T18:00` or `2026-11-14 18:00` (Manila), or RFC 3339.
fn parse_time(raw: &str) -> Option<OffsetDateTime> {
    let raw = raw.trim();
    if let Ok(at) = OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339) {
        return Some(at);
    }
    let local = raw.replacen(' ', "T", 1);
    let minutes = time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]");
    let seconds = time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]");
    PrimitiveDateTime::parse(&local, &minutes)
        .or_else(|_| PrimitiveDateTime::parse(&local, &seconds))
        .ok()
        .map(|at| at.assume_offset(MANILA))
}

/// The command line after `bir`.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut args = args.iter();
    let verb = args.next().map(String::as_str).unwrap_or_default();
    let kind = match verb {
        "declare" => {
            let kind = args.next().map(String::as_str).unwrap_or_default();
            Some(match kind {
                "online" => NoteKind::Online,
                "offline" => NoteKind::Outage,
                "maintenance" => NoteKind::Maintenance,
                _ => return Err(format!("declare online, offline or maintenance, not `{kind}`")),
            })
        }
        "clear" | "status" => None,
        _ => return Err(format!("unknown command `{verb}`")),
    };
    let (mut actor, mut reason, mut until, mut text) = (None, None, None, None);
    while let Some(arg) = args.next() {
        let name = arg.as_str();
        let mut value = || args.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        match name {
            "--as" if verb != "status" => actor = Some(value()?),
            "--reason" if verb != "status" => reason = Some(value()?),
            "--text" if kind.is_some() => text = Some(value()?),
            "--until" | "--hours" if kind.is_some() && until.is_some() => {
                return Err("give --until or --hours, not both".into());
            }
            "--until" if kind.is_some() => {
                let raw = value()?;
                until = Some(Until::At(parse_time(&raw).ok_or_else(|| format!("--until {raw} is not a time"))?));
            }
            "--hours" if kind.is_some() => {
                let raw = value()?;
                let hours = raw.parse::<i64>().map_err(|_| format!("--hours {raw} is not a whole number"))?;
                until = Some(Until::Hours(hours));
            }
            other => return Err(format!("unexpected `{other}`")),
        }
    }
    match (verb, kind) {
        ("status", _) => Ok(Command::Status),
        ("clear", _) => {
            Ok(Command::Clear { actor: actor.ok_or("--as is needed")?, reason: reason.ok_or("--reason is needed")? })
        }
        (_, Some(kind)) => Ok(Command::Declare {
            kind,
            actor: actor.ok_or("--as is needed")?,
            reason: reason.ok_or("--reason is needed")?,
            until: until.ok_or("--until or --hours is needed")?,
            text,
        }),
        _ => Err("nothing to do".into()),
    }
}

/// Why it stopped.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Refused; nothing was written.
    #[error("{0}")]
    Refused(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for CliError {
    fn from(error: sqlx::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl From<NoteError> for CliError {
    fn from(error: NoteError) -> Self {
        match error {
            NoteError::Database(error) => Self::Internal(error.into()),
            other => Self::Refused(other.to_string()),
        }
    }
}

/// The public text a note gets when `--text` is not given.
pub fn default_text(kind: NoteKind) -> &'static str {
    match kind {
        NoteKind::Online => "Buwiz staff marked the BIR online.",
        NoteKind::Outage => "Buwiz staff marked the BIR offline.",
        NoteKind::Maintenance => "The BIR has planned maintenance.",
    }
}

fn what(kind: NoteKind) -> &'static str {
    match kind {
        NoteKind::Online => "declare the BIR online (Online whatever the tries say)",
        NoteKind::Outage => "declare the BIR offline (an outage note)",
        NoteKind::Maintenance => "declare planned maintenance (the BIR reads Offline)",
    }
}

/// `Nov 14, 6:00 PM Manila (2026-11-14T10:00:00Z)`.
fn when(at: OffsetDateTime) -> String {
    let utc = at.to_offset(time::UtcOffset::UTC);
    let rfc = utc.format(&time::format_description::well_known::Rfc3339).unwrap_or_default();
    format!("{} Manila ({rfc})", public::manila_when(at))
}

/// The sign in a few lines, times to the minute.
fn describe(sign: &SignState, note: Option<&Note>) -> Vec<String> {
    let document = public::document(sign, "", Times::Exact);
    let mut lines = vec![
        format!(
            "{} since {} (seq {}, source {})",
            public::state_word(sign.state),
            when(document.since),
            sign.seq,
            document.source
        ),
        format!("  {}", document.words.line),
        format!(
            "  Front desk: {}; mailbox: {}",
            public::state_word(sign.dispatcher.state),
            public::state_word(sign.upload.state)
        ),
    ];
    if let Some(note) = note {
        let shown = sign.note.as_ref().is_some_and(|shown| shown.id == note.id);
        lines.push(format!(
            "  Note: {} until {}: \"{}\"{}",
            note.kind.as_str(),
            when(note.until),
            note.text,
            if shown { "" } else { " (not in force)" }
        ));
    }
    lines.push(format!("  Last evaluated: {}", when(sign.evaluated_at)));
    lines
}

/// Refuses a database whose applied migrations are not exactly this
/// release's.
async fn check_migrations(db: &PgPool) -> Result<(), CliError> {
    let applied: Vec<(i64, Vec<u8>, bool)> =
        sqlx::query_as("SELECT version, checksum, success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(db)
            .await?;
    let release: Vec<&sqlx::migrate::Migration> =
        crate::MIGRATOR.iter().filter(|migration| !migration.migration_type.is_down_migration()).collect();
    let same = applied.len() == release.len()
        && release.iter().all(|migration| {
            applied.iter().any(|(version, checksum, success)| {
                *version == migration.version && *success && checksum.as_slice() == migration.checksum.as_ref()
            })
        });
    if same {
        Ok(())
    } else {
        Err(CliError::Refused(
            "this database's migrations are not this release's: run the command of the release the server runs".into(),
        ))
    }
}

/// `--as` as an Admin, by the same check the staff pages use.
async fn admin(db: &PgPool, config: &Config, email: &str) -> Result<Staff, CliError> {
    let refused = || CliError::Refused(format!("--as {email} is not an Admin: run it as one"));
    let email = accounts::normalize_email(email).map_err(|_| refused())?;
    let account: Option<Account> =
        sqlx::query_as("SELECT id, email FROM accounts WHERE email = $1").bind(&email).fetch_optional(db).await?;
    let account = account.ok_or_else(|| {
        CliError::Refused(format!("--as {email} has no Buwiz account: sign in once on the website first"))
    })?;
    match roles::staff(db, config, &account).await? {
        Some(staff) if staff.can(Power::SetBirStatus) => Ok(staff),
        _ => Err(refused()),
    }
}

/// How many times a change is tried when the running server wrote the sign
/// in between.
const TRIES: usize = 3;

fn line(out: &mut dyn Write, text: impl AsRef<str>) {
    let _ = writeln!(out, "{}", text.as_ref());
    let _ = out.flush();
}

/// A change to make.
#[derive(Clone, Copy)]
enum Change<'a> {
    Set(NewNote<'a>, &'a str),
    Clear(&'a str),
}

/// Runs `command` on `db` with the server's `config` and `level`, at
/// `now`, telling `out` what it does.
pub async fn run(
    db: &PgPool,
    config: &Config,
    level: Level,
    production: bool,
    command: Command,
    now: OffsetDateTime,
    out: &mut dyn Write,
) -> Result<(), CliError> {
    check_migrations(db).await?;
    let target = {
        let name: String = sqlx::query_scalar("SELECT current_database()").fetch_one(db).await?;
        let options = db.connect_options();
        format!("{}:{}/{name}", options.get_host(), options.get_port())
    };
    line(out, format!("Database: {target}"));
    line(out, format!("BIR_UPTIME: {}", level.as_str()));
    let stored = live::stored(db, now).await?;
    let (actor, reason) = match &command {
        Command::Status => return status(db, stored.as_ref(), out).await,
        Command::Declare { actor, reason, .. } | Command::Clear { actor, reason } => (actor, reason),
    };
    if !level.records() {
        return Err(CliError::Refused(
            "BIR_UPTIME is off on this server: no sign is kept, so there is nothing to set".into(),
        ));
    }
    let actor = admin(db, config, actor).await?;
    let reason = notes::note_text(reason)?;
    line(out, format!("As: {} (Admin)", actor.account.email));
    match &stored {
        Some((sign, note)) => {
            line(out, "The sign now:");
            describe(sign, note.as_ref()).into_iter().for_each(|text| line(out, format!("  {text}")));
        }
        None => line(out, "The sign now: none kept yet (this starts it)."),
    }
    let text;
    let change = match &command {
        Command::Declare { kind, until, text: wanted, .. } => {
            let until = until.at(now);
            if until <= now {
                return Err(CliError::Refused("the note must end in the future".into()));
            }
            if until > now + MAX_AHEAD {
                return Err(CliError::Refused(format!("a note may end at most 72 hours ahead, not {}", when(until))));
            }
            text = notes::note_text(wanted.as_deref().unwrap_or(default_text(*kind)))?;
            line(out, format!("Will: {} until {}", what(*kind), when(until)));
            line(out, format!("  Public text (people and apps read it): \"{text}\""));
            Change::Set(NewNote { kind: *kind, text: &text, until }, &reason)
        }
        Command::Clear { .. } => {
            let note = stored.as_ref().and_then(|(_, note)| note.as_ref()).filter(|note| note.until > now);
            let Some(note) = note else {
                return Err(CliError::Refused("no note is in force".into()));
            };
            line(
                out,
                format!("Will: clear the {} note \"{}\" (until {})", note.kind.as_str(), note.text, when(note.until)),
            );
            Change::Clear(&reason)
        }
        Command::Status => unreachable!("answered above"),
    };
    line(out, format!("  Reason (staff only, in the audit line): \"{reason}\""));
    for attempt in 1..=TRIES {
        // An engine of this process's own, loaded from the record each time.
        let uptime = Uptime::new(level, DEFAULT_UPLOAD_PORTS.to_vec(), production, now);
        let at = OffsetDateTime::now_utc().max(now);
        let made = match change {
            Change::Set(new, why) => {
                notes::set_with_reason(&uptime, db, config, &actor, new, Some(why), at).await.map(drop)
            }
            Change::Clear(why) => notes::clear(&uptime, db, config, &actor, why, at).await,
        };
        match made {
            Ok(()) => {
                line(out, "Done. The sign now:");
                let sign = uptime.live.sign();
                describe(&sign, sign.note.as_ref()).into_iter().for_each(|text| line(out, format!("  {text}")));
                line(out, "The running server shows it within 30 seconds and mails staff about it.");
                return Ok(());
            }
            Err(NoteError::Moved) if attempt < TRIES => {
                line(out, "The server changed the sign meanwhile; trying again.")
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(CliError::Refused("the sign kept changing; nothing was written: run it again".into()))
}

/// `bir status`: the stored sign and the last changes. Writes nothing.
async fn status(db: &PgPool, stored: Option<&(SignState, Option<Note>)>, out: &mut dyn Write) -> Result<(), CliError> {
    match stored {
        Some((sign, note)) => describe(sign, note.as_ref()).into_iter().for_each(|text| line(out, text)),
        None => line(out, "No sign is kept yet: the server has not started with BIR_UPTIME at record or above."),
    }
    let events: Vec<(OffsetDateTime, String, String, String, String)> = sqlx::query_as(
        "SELECT at, scope, from_state, to_state, reason FROM bir_events ORDER BY at DESC, id DESC LIMIT 5",
    )
    .fetch_all(db)
    .await?;
    if !events.is_empty() {
        line(out, "Last changes:");
        for (at, scope, from, to, reason) in events {
            line(out, format!("  {}  {scope}: {from} -> {to}  {reason}", public::manila_when(at)));
        }
    }
    Ok(())
}

/// The command line: see [`USAGE`]. Reads `DATABASE_URL`, `ADMIN_EMAILS`
/// and `BIR_UPTIME` with the rest of the server's settings. Exit codes: 0
/// done, 2 refused (nothing written), 1 an error (nothing written).
pub async fn cli(args: &[String]) -> ExitCode {
    let command = match parse(args) {
        Ok(command) => command,
        Err(problem) => {
            eprintln!("{problem}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("Error; nothing was written: the server's settings are not complete: {error:#}");
            return ExitCode::FAILURE;
        }
    };
    let db = match PgPoolOptions::new().max_connections(2).connect(&config.database_url).await {
        Ok(db) => db,
        Err(error) => {
            eprintln!("Error; nothing was written: cannot connect to the database: {error}");
            return ExitCode::FAILURE;
        }
    };
    let production = config.account_origin.starts_with("https://");
    let mut stdout = std::io::stdout();
    match run(&db, &config, Level::from_env(), production, command, OffsetDateTime::now_utc(), &mut stdout).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Refused(problem)) => {
            eprintln!("Refused; nothing was written: {problem}");
            ExitCode::from(2)
        }
        Err(CliError::Internal(error)) => {
            eprintln!("Error; nothing was written: {error:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn args(raw: &str) -> Vec<String> {
        raw.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn the_command_line_reads_three_verbs() {
        assert_eq!(parse(&args("status")), Ok(Command::Status));
        assert_eq!(
            parse(&args("declare offline --as a@example.test --reason x --hours 6")),
            Ok(Command::Declare {
                kind: NoteKind::Outage,
                actor: "a@example.test".into(),
                reason: "x".into(),
                until: Until::Hours(6),
                text: None,
            })
        );
        assert_eq!(
            parse(&args("declare maintenance --as a@example.test --reason x --until 2026-11-14T18:00")),
            Ok(Command::Declare {
                kind: NoteKind::Maintenance,
                actor: "a@example.test".into(),
                reason: "x".into(),
                until: Until::At(datetime!(2026-11-14 10:00 UTC)),
                text: None,
            })
        );
        assert_eq!(
            parse(&args("declare online --as a@example.test --reason x --until 2026-11-14T18:00:00Z --text y")),
            Ok(Command::Declare {
                kind: NoteKind::Online,
                actor: "a@example.test".into(),
                reason: "x".into(),
                until: Until::At(datetime!(2026-11-14 18:00 UTC)),
                text: Some("y".into()),
            })
        );
        assert_eq!(
            parse(&args("clear --as a@example.test --reason x")),
            Ok(Command::Clear { actor: "a@example.test".into(), reason: "x".into() })
        );
        for bad in [
            "",
            "declare",
            "declare down --as a --reason x --hours 1",
            "declare offline --reason x --hours 1",
            "declare offline --as a --hours 1",
            "declare offline --as a --reason x",
            "declare offline --as a --reason x --hours 1 --until 2026-11-14T18:00",
            "declare offline --as a --reason x --hours six",
            "declare offline --as a --reason x --until tomorrow",
            "declare offline --as a --reason",
            "clear --as a",
            "clear --as a --reason x --hours 1",
            "status --as a",
            "pause",
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad}");
        }
    }
}
