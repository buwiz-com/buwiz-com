-- Back to migration 13. docs/deploy.md has the whole rollback; run this
-- file before 0013's (and 0012's and 0011's, to go further). It commits on
-- its own and never refuses: nothing the release before this one reads
-- changes.
--
--   psql -v ON_ERROR_STOP=1 -1 -f 0014_bir_uptime.down.sql
--
-- What it keeps: what Buwiz keeps forever, the day history (`bir_days`),
-- the events (`bir_events`) and the notes (`bir_notes`), the proofs of
-- outage (`bir_proofs`), and the sign (`bir_sign`, whose `seq` must only
-- grow), move, whole, to the `rollback_archive` schema, which the release
-- before this one does not read. Migrating up again brings them back.
-- Rolling back twice adds to the archive; the way up takes the newest copy
-- of each row. Every column is named, so an archive table made by an
-- earlier rollback reads the same whatever order its columns are in. A
-- proof keeps its `claim_id` here, NULL if its claim was deleted (its JWS
-- keeps the masked TIN); if the claim is deleted while at 13, the way up
-- brings the proof back without it.
--
-- What it loses: the tries of the last 90 days (`bir_tries`), queue
-- reports and turns (at most a day old), held requests, the recorded
-- upload-server host keys and their confirmations, each ticket's admission
-- and when each device first reported its queue.

CREATE SCHEMA IF NOT EXISTS rollback_archive;
CREATE TABLE IF NOT EXISTS rollback_archive.bir_days AS
    SELECT now() AS archived_at, day, tries, got_in, failed, maybe, ours, user_network, held, accounts,
        networks, online_minutes, busy_minutes, offline_minutes, quiet_minutes, note_minutes,
        unwatched_minutes, worst, hours, outages, windows, final
      FROM bir_days WITH NO DATA;
INSERT INTO rollback_archive.bir_days (archived_at, day, tries, got_in, failed, maybe, ours, user_network,
    held, accounts, networks, online_minutes, busy_minutes, offline_minutes, quiet_minutes, note_minutes,
    unwatched_minutes, worst, hours, outages, windows, final)
    SELECT now(), day, tries, got_in, failed, maybe, ours, user_network, held, accounts, networks,
        online_minutes, busy_minutes, offline_minutes, quiet_minutes, note_minutes, unwatched_minutes, worst,
        hours, outages, windows, final FROM bir_days;
CREATE TABLE IF NOT EXISTS rollback_archive.bir_events AS
    SELECT now() AS archived_at, id, at, scope, from_state, to_state, cause, reason, actor_id,
        actor_email, mailed_at, sign_seq, ord
      FROM bir_events WITH NO DATA;
INSERT INTO rollback_archive.bir_events (archived_at, id, at, scope, from_state, to_state, cause, reason,
    actor_id, actor_email, mailed_at, sign_seq, ord)
    SELECT now(), id, at, scope, from_state, to_state, cause, reason, actor_id, actor_email, mailed_at,
        sign_seq, ord FROM bir_events;
CREATE TABLE IF NOT EXISTS rollback_archive.bir_notes AS
    SELECT now() AS archived_at, id, kind, text, starts_at, until, set_by, set_by_email, cleared_at,
        cleared_by, cleared_by_email, cleared_how, cleared_reason
      FROM bir_notes WITH NO DATA;
INSERT INTO rollback_archive.bir_notes (archived_at, id, kind, text, starts_at, until, set_by,
    set_by_email, cleared_at, cleared_by, cleared_by_email, cleared_how, cleared_reason)
    SELECT now(), id, kind, text, starts_at, until, set_by, set_by_email, cleared_at, cleared_by,
        cleared_by_email, cleared_how, cleared_reason FROM bir_notes;
CREATE TABLE IF NOT EXISTS rollback_archive.bir_proofs AS
    SELECT now() AS archived_at, id, account_id, claim_id, day, form, period, kid, jws, created_at
      FROM bir_proofs WITH NO DATA;
INSERT INTO rollback_archive.bir_proofs (archived_at, id, account_id, claim_id, day, form, period, kid,
    jws, created_at)
    SELECT now(), id, account_id, claim_id, day, form, period, kid, jws, created_at FROM bir_proofs;
CREATE TABLE IF NOT EXISTS rollback_archive.bir_sign AS
    SELECT now() AS archived_at, id, seq, state, updated_at
      FROM bir_sign WITH NO DATA;
INSERT INTO rollback_archive.bir_sign (archived_at, id, seq, state, updated_at)
    SELECT now(), id, seq, state, updated_at FROM bir_sign;

DROP TABLE bir_proofs;
DROP TABLE bir_days;
DROP TABLE bir_host_keys;
DROP TABLE bir_held;
DROP TABLE bir_turns;
DROP TABLE bir_queue_reports;
DROP TABLE bir_notes;
DROP TABLE bir_sign;
DROP TABLE bir_events;
DROP TABLE bir_tries;

ALTER TABLE devices DROP COLUMN bir_reported_at;
ALTER TABLE tickets DROP COLUMN bir_admission;

-- See 0011_tin_states.down.sql.
DELETE FROM _sqlx_migrations WHERE version = 14;
