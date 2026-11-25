-- "Sign in with Buwiz" for the eBIRForms apps (docs/accounts.md): the
-- apps use /oauth/authorize as the agents do, as the built-in client
-- `buwiz-desktop`, with a fixed list of redirect addresses in the code.
-- Built-in clients are never rows of `oauth_clients`, so a code's client
-- no longer references that table: the code checks the client instead,
-- and a code lasts ten minutes. A code keeps the install and platform the
-- app named, for the grant its exchange makes (as in-app sign-in stamps
-- them, migration 18).
--
-- Dropping the reference scans nothing, and the new columns are nullable
-- without a default, so no row is rewritten. Postgres reads the table
-- once, under the lock, to check the new columns' checks: one short row
-- per code given out, so the lock is held for a moment only. No existing
-- row changes.

SET LOCAL lock_timeout = '10s';

ALTER TABLE oauth_codes DROP CONSTRAINT oauth_codes_client_id_fkey;
ALTER TABLE oauth_codes
    ADD COLUMN install_id text CHECK (install_id ~ '^[A-Za-z0-9_-]{16,64}$'),
    ADD COLUMN platform text CHECK (platform IN ('ios', 'ipados', 'android', 'macos', 'windows', 'linux'));
