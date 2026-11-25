-- In-app email sign-in (docs/accounts.md, "In-app sign-in"): the eBIRForms
-- apps ask for a sign-in code by email without a browser. Each request is
-- a row of `app_sign_ins`, tied to the device that made it by the hash of
-- a secret only that device holds. Its code is a row of `email_codes` of
-- its own purpose, `app_sign_in`, which works only with that request: the
-- web never takes it, and the request takes no other code. It counts
-- towards the web's limits with every other code.
--
-- `app_sign_in_unchecked` counts the requests that came without an app
-- check (Turnstile), for the testing cap of 20 an hour and 50 a day across
-- every address; `app_sign_in_alerts` is the day staff were mailed that
-- 25 had come. The hourly upkeep deletes all three.
--
-- Three new tables and a wider purpose check on `email_codes` (which
-- scans it; the upkeep keeps it small). No existing row changes.

SET LOCAL lock_timeout = '10s';

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
    -- The request's code. NULL when none was made (the limit on addresses
    -- without an account mails nothing), and once the upkeep deleted it.
    code_id         uuid REFERENCES email_codes (id) ON DELETE SET NULL,
    -- SHA-256 of the one-time link token in the mail, once there is one.
    link_token_hash bytea CHECK (length(link_token_hash) = 32),
    -- Wrong codes typed while the request had no code (the code's own
    -- count is in `email_codes`).
    attempts        integer NOT NULL DEFAULT 0,
    created_at      timestamptz NOT NULL DEFAULT now(),
    -- When its code stops working; a new one may be asked for from then.
    expires_at      timestamptz NOT NULL,
    used_at         timestamptz
);
CREATE INDEX app_sign_ins_email ON app_sign_ins (email, install_id);
CREATE INDEX app_sign_ins_expires_at ON app_sign_ins (expires_at);
CREATE INDEX app_sign_ins_code_id ON app_sign_ins (code_id);

CREATE TABLE app_sign_in_unchecked (
    id         bigserial PRIMARY KEY,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX app_sign_in_unchecked_created_at ON app_sign_in_unchecked (created_at);

CREATE TABLE app_sign_in_alerts (
    -- The Manila day.
    day     date PRIMARY KEY,
    sent_at timestamptz NOT NULL DEFAULT now()
);
