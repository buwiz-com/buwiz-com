-- "Sign in with Buwiz" for the eBIRForms apps (docs/accounts.md): the
-- apps use /oauth/authorize as the agents do, as the built-in client
-- `buwiz-desktop`, with a fixed list of redirect addresses in the code.
-- Built-in clients are never rows of `oauth_clients`, so a code's client
-- no longer references that table: the code checks the client instead,
-- and a code lasts ten minutes. A code keeps the install and platform the
-- app named, for the grant its exchange makes (as in-app sign-in stamps
-- them, migration 18).
--
-- A device row also says who removed it (`removed_by`): 'owner' (My
-- devices, or the app's own remove call) or 'staff'. NULL while it is
-- live, and when it ended with its sign-in (signed out, replaced, or
-- 0018's trigger). A removed device that signs in again is mailed as a new
-- device; one that signed out and back in is not. `new_device_mails`
-- records each new-device mail, for the limit of a few a day per account.
--
-- No row is read or rewritten here. The new columns are nullable without
-- a default, and their checks are added NOT VALID: Postgres checks the
-- rows written from now on, and reads none of those already there. So
-- each ACCESS EXCLUSIVE lock (oauth_codes, devices; a lighter one on
-- oauth_clients and accounts for the references) is held for a moment,
-- until this short transaction commits. That matters for `devices`: every
-- signed request updates its row, and the table only grows (an app
-- without an install id adds a row at each registration, and nothing
-- deletes device rows). Migration 22 checks the rows already there, in a
-- transaction of its own, under a lock that lets those updates go on.
-- The lock_timeout below bounds the wait for the locks.

SET LOCAL lock_timeout = '10s';

ALTER TABLE oauth_codes DROP CONSTRAINT oauth_codes_client_id_fkey;
ALTER TABLE oauth_codes ADD COLUMN install_id text, ADD COLUMN platform text;
ALTER TABLE oauth_codes
    ADD CONSTRAINT oauth_codes_install_id_check CHECK (install_id ~ '^[A-Za-z0-9_-]{16,64}$') NOT VALID,
    ADD CONSTRAINT oauth_codes_platform_check
        CHECK (platform IN ('ios', 'ipados', 'android', 'macos', 'windows', 'linux')) NOT VALID;

ALTER TABLE devices ADD COLUMN removed_by text;
ALTER TABLE devices ADD CONSTRAINT devices_removed_by_check CHECK (removed_by IN ('owner', 'staff')) NOT VALID;

CREATE TABLE new_device_mails (
    account_id uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    sent_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX new_device_mails_account ON new_device_mails (account_id, sent_at);
