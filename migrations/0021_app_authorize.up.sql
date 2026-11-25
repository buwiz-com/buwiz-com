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
-- devices, or the app's own remove call) or 'staff'. NULL while it is live,
-- and when it ended with its sign-in (signed out, replaced, or 0018's
-- trigger). A removed device that signs in again is mailed as a new device;
-- one that signed out and back in is not (docs/accounts.md).
--
-- Dropping the reference scans nothing, and the new columns are nullable
-- without a default, so no row is rewritten and no existing row changes.
-- Postgres does read every row of `oauth_codes` once, to check the new
-- columns' checks, and holds its ACCESS EXCLUSIVE lock on that table (and
-- a lock on `oauth_clients`, for the reference) until the migration
-- commits: no code is made or exchanged meanwhile. Codes are kept 30 days
-- only, so the table is small and the read is short. The lock_timeout
-- below bounds the wait for the locks, not how long they are held.
-- The same holds for `devices`: Postgres reads every row once to check
-- `removed_by`, under that table's ACCESS EXCLUSIVE lock, so signed
-- requests (which update their device) wait until the migration commits.
-- There is one row per account and install, so the read is short.

SET LOCAL lock_timeout = '10s';

ALTER TABLE oauth_codes DROP CONSTRAINT oauth_codes_client_id_fkey;
ALTER TABLE oauth_codes
    ADD COLUMN install_id text CHECK (install_id ~ '^[A-Za-z0-9_-]{16,64}$'),
    ADD COLUMN platform text CHECK (platform IN ('ios', 'ipados', 'android', 'macos', 'windows', 'linux'));
ALTER TABLE devices ADD COLUMN removed_by text CHECK (removed_by IN ('owner', 'staff'));
