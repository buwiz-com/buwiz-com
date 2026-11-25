-- One device row per install (docs/accounts.md, "Devices"): an app sends
-- `install_id`, its own id for the install, when it signs in and when it
-- registers its key. A sign-in carries it on its grant; a registration
-- from an install that already has a row for the account updates that row
-- (a new key and sign-in) instead of adding one, and the sign-in it had
-- before is ended. The unique index keeps it at one row per account and
-- install. Rows without an install id are as before.
--
-- The new device columns describe the device as its app sends it
-- (platform, model, system version, computer name), and when it last
-- signed in (`signed_in_at`, from `created_at` for the rows there are).
-- A grant also keeps the platform an in-app sign-in named, for the
-- account's list of sign-ins not yet added as a device.
--
-- `devices` is rewritten once for the backfill; it and `oauth_grants` are
-- small.

SET LOCAL lock_timeout = '10s';

ALTER TABLE oauth_grants
    ADD COLUMN install_id text CHECK (install_id ~ '^[A-Za-z0-9_-]{16,64}$'),
    ADD COLUMN platform text CHECK (platform IN ('ios', 'ipados', 'android', 'macos', 'windows', 'linux'));

ALTER TABLE device_authorizations
    ADD COLUMN install_id text CHECK (install_id ~ '^[A-Za-z0-9_-]{16,64}$');

ALTER TABLE devices
    ADD COLUMN install_id text CHECK (install_id ~ '^[A-Za-z0-9_-]{16,64}$'),
    ADD COLUMN platform text CHECK (platform IN ('ios', 'ipados', 'android', 'macos', 'windows', 'linux')),
    ADD COLUMN model text CHECK (char_length(model) <= 80),
    ADD COLUMN os_version text CHECK (char_length(os_version) <= 40),
    ADD COLUMN computer_name text CHECK (char_length(computer_name) <= 80),
    ADD COLUMN signed_in_at timestamptz;
UPDATE devices SET signed_in_at = created_at;
ALTER TABLE devices ALTER COLUMN signed_in_at SET DEFAULT now(), ALTER COLUMN signed_in_at SET NOT NULL;

CREATE UNIQUE INDEX devices_account_install ON devices (account_id, install_id) WHERE install_id IS NOT NULL;
CREATE INDEX devices_grant ON devices (grant_id);
