-- In-app email sign-in (docs/accounts.md, "In-app sign-in"): the eBIRForms
-- apps ask for a sign-in code by email without a browser. Each request is
-- a row of `app_sign_ins`, tied to the device that made it by the hash of
-- a secret only that device holds. Its code is a row of `email_codes` of
-- its own purpose, `app_sign_in`, which works only with that request: the
-- web never takes it, and the request takes no other code. It counts
-- towards the web's limits with every other code.
--
-- `email_codes.unchecked` marks a code an app asked for without an app
-- check (Turnstile): those are counted for the testing cap (20 an hour and
-- 50 a day across every address) and, for addresses without an account,
-- against a budget of their own (10 an hour, 30 a day) that the web's
-- caps do not count. `app_sign_in_alerts` is the day staff were mailed
-- that 25 had come. The hourly upkeep deletes both tables' old rows.
--
-- `email_codes.mailed` is false for a code that was made and never mailed:
-- an app's request for an address whose budget of mail was used up gets a
-- real code all the same, which nobody is sent, so the request is answered
-- and held exactly as one that was mailed. Such a code counts towards the
-- address's five an hour, and towards no budget of mail (the new
-- addresses', the testing cap). Every other code is mailed (a test
-- account's fixed code counts as given).
--
-- Two new tables, two new columns with a constant default (no row is
-- rewritten) and a wider purpose check on `email_codes`. Adding the check
-- reads the whole table under its ACCESS EXCLUSIVE lock, held until this
-- migration commits: `email_codes` holds 30 days of codes, so the scan is
-- short, and the lock_timeout below makes a migration that cannot get the
-- lock fail instead of waiting. No existing row changes.

SET LOCAL lock_timeout = '10s';

ALTER TABLE email_codes ADD COLUMN unchecked boolean NOT NULL DEFAULT false;
ALTER TABLE email_codes ADD COLUMN mailed boolean NOT NULL DEFAULT true;
ALTER TABLE email_codes DROP CONSTRAINT email_codes_purpose_check;
ALTER TABLE email_codes ADD CONSTRAINT email_codes_purpose_check
    CHECK (purpose IN ('sign_in', 'delete', 'app_sign_in'));

CREATE TABLE app_sign_ins (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    email           text NOT NULL,
    client_id       text NOT NULL,
    -- SHA-256 of the app's secret: the secret itself is never sent here.
    secret_hash     bytea NOT NULL CHECK (length(secret_hash) = 32),
    intent          text NOT NULL CHECK (intent IN ('sign_in', 'create')),
    -- Only a known value; anything else the app sent is stored as NULL.
    platform        text CHECK (platform IN ('ios', 'ipados', 'android', 'macos', 'windows', 'linux')),
    -- The app's own id for its install, when it sent one.
    install_id      text CHECK (install_id ~ '^[A-Za-z0-9_-]{16,64}$'),
    -- The request's code: every request is written with one (mailed or
    -- not). NULL only once the upkeep deleted it, long after it expired.
    code_id         uuid REFERENCES email_codes (id) ON DELETE SET NULL,
    -- SHA-256 of the one-time link token in the mail. NULL for a test
    -- account's fixed code, which is not mailed.
    link_token_hash bytea CHECK (length(link_token_hash) = 32),
    created_at      timestamptz NOT NULL DEFAULT now(),
    -- When its code stops working; a new one may be asked for from then.
    expires_at      timestamptz NOT NULL,
    used_at         timestamptz
);
CREATE INDEX app_sign_ins_email ON app_sign_ins (email, install_id);
CREATE INDEX app_sign_ins_expires_at ON app_sign_ins (expires_at);
CREATE INDEX app_sign_ins_code_id ON app_sign_ins (code_id);

CREATE TABLE app_sign_in_alerts (
    -- The Manila day.
    day     date PRIMARY KEY,
    sent_at timestamptz NOT NULL DEFAULT now()
);
