-- BIR uptime (docs/bir-uptime.md): one row per relay or direct-app try, the
-- sign and its events, hand-set notes, queue reports and turns, held ticket
-- requests, upload-server host keys, the day history and proofs of outage.
-- New tables, and two new nullable columns (`tickets.bir_admission`,
-- `devices.bir_reported_at`): no existing row changes, and no existing
-- table is scanned under its lock.
--
-- No uptime table holds a TIN, a name, an IP address or a submission id.
-- Four columns hold an Admin's email, for the audit:
-- `bir_events.actor_email`, `bir_notes.set_by_email` and
-- `cleared_by_email`, and `bir_host_keys.confirmed_email`. Tries keep
-- 8-byte keyed hashes of the account and the network under a day key that
-- lives only in the server's memory, and their times to the minute. Queue
-- reports and turns keep the account id for at most a day; held requests
-- keep it, with the claim (not the TIN), for 90 days, for proofs.
--
-- After a rollback and an upgrade again, what the rollback kept in
-- `rollback_archive` (the day history, events, notes, proofs and the sign)
-- comes back here; see the end of this file.

-- A stray open transaction on the tables this changes (a pre-flight
-- `BEGIN` left open) makes start-up fail with a lock timeout instead of
-- hanging.
SET LOCAL lock_timeout = '10s';

-- One try. `counted` is `mode = 'live' AND (outcome = 'sent' OR
-- failure_class = 'bir') AND failure_kind IS DISTINCT FROM 'no_service'`,
-- stored so the rules read one column. Tries in `lab`, `dry_run` and
-- `unknown` (no `sender` frame) carry no class or kind, but for a Sender
-- that could not be reached at all (`ours`/`sender_down`); every `live` try
-- that did not get in carries a class. Kept 90 days (the hourly upkeep).
CREATE TABLE bir_tries (
    id            bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    started_at    timestamptz NOT NULL CHECK (started_at = date_trunc('minute', started_at)),
    ended_at      timestamptz NOT NULL CHECK (ended_at = date_trunc('minute', ended_at)),
    source        text NOT NULL CHECK (source IN ('relay', 'direct')),
    mode          text NOT NULL CHECK (mode IN ('live', 'lab', 'dry_run', 'unknown')),
    outcome       text NOT NULL CHECK (outcome IN ('sent', 'dry_run', 'failed', 'unknown')),
    stage         text NOT NULL CHECK (stage IN ('none', 'dispatcher', 'upload', 'uploaded')),
    dispatcher    text NOT NULL CHECK (dispatcher IN ('ok', 'fail', 'none')),
    upload        text NOT NULL CHECK (upload IN ('ok', 'fail', 'maybe', 'none')),
    failure_class text CHECK (failure_class IN ('bir', 'user_network', 'ours')),
    failure_kind  text CHECK (failure_kind ~ '^[a-z_]{1,20}$'),
    touched_bir   boolean NOT NULL,
    opens         smallint NOT NULL DEFAULT 0 CHECK (opens >= 0),
    admission     text NOT NULL CHECK (admission IN ('open', 'admit', 'scout', 'legacy_scout', 'try_anyway')),
    counted       boolean NOT NULL,
    -- The day key's period: a random id, new with each key.
    key_epoch     integer NOT NULL CHECK (key_epoch > 0),
    account_key   bytea NOT NULL CHECK (length(account_key) = 8),
    -- NULL for a late direct report.
    network_key   bytea CHECK (length(network_key) = 8),
    late          boolean NOT NULL DEFAULT false,
    -- The `buwiz-client` header as a known app name (or `other`) and the
    -- version's major.minor only, e.g. `desktop/0.2`.
    client        text CHECK (client ~ '^(desktop|mini|direct|buwiz-forms|buwiz-desktop|buwiz-ebirforms|buwiz-direct|other)/[0-9]{1,4}\.[0-9]{1,4}$'),
    -- The direct app's try id; NULL for the relay.
    report_id     uuid UNIQUE,
    CHECK (ended_at >= started_at),
    CHECK ((failure_class IS NULL) = (failure_kind IS NULL)),
    CHECK (CASE WHEN mode IN ('lab', 'dry_run') THEN failure_class IS NULL
                WHEN mode = 'unknown' THEN failure_class IS NULL OR failure_kind = 'sender_down'
                ELSE (outcome IN ('sent', 'dry_run')) = (failure_class IS NULL) END),
    CHECK (NOT counted OR mode = 'live'),
    CHECK ((source = 'direct') = (report_id IS NOT NULL))
);
CREATE INDEX bir_tries_started ON bir_tries (started_at DESC);

-- Every change of the sign, a note or the relay pause. Kept forever.
CREATE TABLE bir_events (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    at          timestamptz NOT NULL DEFAULT clock_timestamp(),
    scope       text NOT NULL CHECK (scope IN ('overall', 'dispatcher', 'upload', 'relay')),
    from_state  text NOT NULL,
    to_state    text NOT NULL,
    cause       text NOT NULL CHECK (cause IN ('rule', 'scout', 'try_anyway', 'note_set', 'note_cleared',
                                               'note_expired', 'note_return_got_in', 'quiet',
                                               'relay_paused', 'relay_resumed')),
    -- A fixed sentence with counts only.
    reason      text NOT NULL,
    -- The Admin, for note_* and relay_resumed.
    actor_id    uuid,
    actor_email text,
    mailed_at   timestamptz
);
CREATE INDEX bir_events_at ON bir_events (at DESC);

-- The sign as last computed, so a restart keeps `since`, the dwell timers
-- and `seq` (which only grows).
CREATE TABLE bir_sign (
    id         smallint PRIMARY KEY CHECK (id = 1),
    seq        bigint NOT NULL,
    state      jsonb NOT NULL,
    updated_at timestamptz NOT NULL
);

-- An Admin's note. A note past its `until` has ended, so it never blocks a
-- new one; the transaction that sets a note checks none is in force.
CREATE TABLE bir_notes (
    id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    kind             text NOT NULL CHECK (kind IN ('outage', 'maintenance', 'online')),
    -- Public: the form says so and asks for no personal data.
    text             text NOT NULL CHECK (length(text) BETWEEN 1 AND 200),
    starts_at        timestamptz NOT NULL DEFAULT now(),
    until            timestamptz NOT NULL,
    set_by           uuid NOT NULL,
    set_by_email     text NOT NULL,
    cleared_at       timestamptz,
    cleared_by       uuid,
    cleared_by_email text,
    cleared_how      text CHECK (cleared_how IN ('staff', 'expired', 'return_got_in')),
    CHECK (until > starts_at AND until <= starts_at + interval '72 hours')
);

-- A device's last queue report, forgotten 20 minutes after it came.
-- `device_key` is 'd:' and the device id, or 'g:' and the OAuth grant id
-- (the direct app).
CREATE TABLE bir_queue_reports (
    device_key  text PRIMARY KEY,
    account_id  uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    client      text NOT NULL CHECK (client IN ('desktop', 'mini', 'direct')),
    paused      boolean NOT NULL,
    sending     text,
    -- [{slot, due, queued_at, next_try_at}], at most 20.
    waiting     jsonb NOT NULL,
    reported_at timestamptz NOT NULL,
    expires_at  timestamptz NOT NULL
);

-- A turn: one queued return may try now. Given only in a queue report's
-- answer.
CREATE TABLE bir_turns (
    -- 't_' and 16 random bytes, base64url.
    id          text PRIMARY KEY,
    kind        text NOT NULL CHECK (kind IN ('scout', 'admit')),
    account_id  uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    device_key  text NOT NULL,
    slot        text NOT NULL,
    granted_at  timestamptz NOT NULL DEFAULT now(),
    not_before  timestamptz NOT NULL,
    expires_at  timestamptz NOT NULL,
    used_at     timestamptz,
    ticket_jti  uuid
);
CREATE INDEX bir_turns_open ON bir_turns (expires_at) WHERE used_at IS NULL;

-- A ticket request (or v1 relay) held for the BIR's state, kept 90 days so
-- a proof of outage can show the person tried.
CREATE TABLE bir_held (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    at          timestamptz NOT NULL DEFAULT now(),
    account_id  uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    claim_id    uuid NOT NULL REFERENCES tin_claims (id),
    form        text NOT NULL,
    period      text NOT NULL,
    code        text NOT NULL CHECK (code IN ('bir_offline', 'bir_busy', 'relay_paused'))
);
CREATE INDEX bir_held_account ON bir_held (account_id, at DESC);

-- Upload-server host keys offered to live tries, read from the SSH
-- handshake the user's app relays. A sighting can be forged, so it is
-- information only and never grounds to pin: pinning needs an Admin and a
-- second source outside the relay (docs/relay-protocol.md §7).
CREATE TABLE bir_host_keys (
    -- 'SHA256:…', as BIR_SSH_HOST_KEYS lists them.
    fingerprint      text PRIMARY KEY CHECK (fingerprint ~ '^SHA256:[A-Za-z0-9+/]{43}$'),
    first_seen       timestamptz NOT NULL,
    last_seen        timestamptz NOT NULL,
    -- [{epoch, account_key, network_key, kex_done}] (keys in hex), the
    -- newest 50; `kex_done`: the Sender went on to complete the key
    -- exchange in that try.
    seen             jsonb NOT NULL,
    confirmed_at     timestamptz,
    confirmed_by     uuid,
    confirmed_email  text
);

-- One line per Manila day, kept forever.
CREATE TABLE bir_days (
    day             date PRIMARY KEY,
    -- Counted tries.
    tries           integer NOT NULL,
    got_in          integer NOT NULL,
    failed          integer NOT NULL,
    maybe           integer NOT NULL,
    -- Recorded, not counted.
    ours            integer NOT NULL,
    user_network    integer NOT NULL,
    held            integer NOT NULL,
    accounts        integer NOT NULL,
    networks        integer NOT NULL,
    online_minutes  integer NOT NULL,
    busy_minutes    integer NOT NULL,
    offline_minutes integer NOT NULL,
    quiet_minutes   integer NOT NULL,
    note_minutes    integer NOT NULL,
    worst           text NOT NULL CHECK (worst IN ('online', 'busy', 'offline', 'no_recent_news', 'none')),
    hours           text NOT NULL CHECK (hours ~ '^[obxmn-]{24}$'),
    -- [{from, to, window, source}]
    outages         jsonb NOT NULL DEFAULT '[]',
    -- Each window's own day: {"dispatcher": {worst, online_minutes,
    -- busy_minutes, offline_minutes, quiet_minutes}, "upload": {...}}.
    windows         jsonb NOT NULL DEFAULT '{}',
    final           boolean NOT NULL DEFAULT false
);

-- A signed proof of outage. The TIN is never stored: the claim names the
-- return, and the JWS carries the TIN masked.
CREATE TABLE bir_proofs (
    -- 'p_' and 16 random bytes.
    id          text PRIMARY KEY,
    account_id  uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    claim_id    uuid NOT NULL REFERENCES tin_claims (id),
    day         date NOT NULL,
    form        text NOT NULL,
    period      text NOT NULL,
    kid         text NOT NULL,
    jws         text NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);

-- How a ticket got through the doorman; NULL for tickets from before. The
-- check is added NOT VALID: every existing row is NULL, so it holds, and
-- new rows are checked; validating it would scan the table under the lock.
ALTER TABLE tickets ADD COLUMN bir_admission text;
ALTER TABLE tickets ADD CONSTRAINT tickets_bir_admission_check
    CHECK (bir_admission IN ('open', 'admit', 'scout', 'legacy_scout', 'try_anyway')) NOT VALID;
-- The first queue report from this device: NULL means an app version that
-- sends none.
ALTER TABLE devices ADD COLUMN bir_reported_at timestamptz;

-- Back from a rollback: 0014's down file kept these in `rollback_archive`.
-- Each row comes back, then leaves the archive; a proof whose account or
-- claim no longer exists stays there (and so does its table).
DO $$
BEGIN
    IF to_regclass('rollback_archive.bir_days') IS NOT NULL THEN
        INSERT INTO bir_days (day, tries, got_in, failed, maybe, ours, user_network, held, accounts, networks,
                              online_minutes, busy_minutes, offline_minutes, quiet_minutes, note_minutes, worst,
                              hours, outages, windows, final)
            SELECT DISTINCT ON (day) day, tries, got_in, failed, maybe, ours, user_network, held, accounts, networks,
                   online_minutes, busy_minutes, offline_minutes, quiet_minutes, note_minutes, worst, hours,
                   outages, windows, final
              FROM rollback_archive.bir_days ORDER BY day, archived_at DESC;
        DROP TABLE rollback_archive.bir_days;
    END IF;
    IF to_regclass('rollback_archive.bir_events') IS NOT NULL THEN
        INSERT INTO bir_events (id, at, scope, from_state, to_state, cause, reason, actor_id, actor_email, mailed_at)
            OVERRIDING SYSTEM VALUE
            SELECT DISTINCT ON (id) id, at, scope, from_state, to_state, cause, reason, actor_id, actor_email,
                   mailed_at
              FROM rollback_archive.bir_events ORDER BY id, archived_at DESC;
        PERFORM setval(pg_get_serial_sequence('bir_events', 'id'), max(id)) FROM bir_events HAVING count(*) > 0;
        DROP TABLE rollback_archive.bir_events;
    END IF;
    IF to_regclass('rollback_archive.bir_notes') IS NOT NULL THEN
        INSERT INTO bir_notes (id, kind, text, starts_at, until, set_by, set_by_email, cleared_at, cleared_by,
                               cleared_by_email, cleared_how)
            SELECT DISTINCT ON (id) id, kind, text, starts_at, until, set_by, set_by_email, cleared_at, cleared_by,
                   cleared_by_email, cleared_how
              FROM rollback_archive.bir_notes ORDER BY id, archived_at DESC;
        DROP TABLE rollback_archive.bir_notes;
    END IF;
    IF to_regclass('rollback_archive.bir_sign') IS NOT NULL THEN
        INSERT INTO bir_sign (id, seq, state, updated_at)
            SELECT id, seq, state, updated_at FROM rollback_archive.bir_sign ORDER BY seq DESC LIMIT 1;
        DROP TABLE rollback_archive.bir_sign;
    END IF;
    IF to_regclass('rollback_archive.bir_proofs') IS NOT NULL THEN
        INSERT INTO bir_proofs (id, account_id, claim_id, day, form, period, kid, jws, created_at)
            SELECT DISTINCT ON (p.id) p.id, p.account_id, p.claim_id, p.day, p.form, p.period, p.kid, p.jws,
                   p.created_at
              FROM rollback_archive.bir_proofs p
             WHERE EXISTS (SELECT 1 FROM accounts a WHERE a.id = p.account_id)
               AND EXISTS (SELECT 1 FROM tin_claims c WHERE c.id = p.claim_id)
             ORDER BY p.id, p.archived_at DESC;
        DELETE FROM rollback_archive.bir_proofs p WHERE EXISTS (SELECT 1 FROM bir_proofs b WHERE b.id = p.id);
        IF NOT EXISTS (SELECT 1 FROM rollback_archive.bir_proofs) THEN
            DROP TABLE rollback_archive.bir_proofs;
        END IF;
    END IF;
END $$;
