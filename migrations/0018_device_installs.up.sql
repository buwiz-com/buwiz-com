-- One device row per install (docs/accounts.md, "Devices"): an app sends
-- `install_id`, its own id for the install, when it signs in and when it
-- registers its key. A sign-in carries it on its grant; a registration
-- from an install that already has a row for the account updates that row
-- (a new key and sign-in) instead of adding one, and the sign-in it had
-- before is ended. Migration 19 adds the unique index that keeps it at
-- one row per account and install. Rows without an install id are as
-- before.
--
-- The new device columns describe the device as its app sends it
-- (platform, model, system version, computer name), and when it last
-- signed in (`signed_in_at`; NULL for the rows there are, which read it as
-- `created_at`). A grant also keeps the platform an in-app sign-in named,
-- for the account's list of sign-ins not yet added as a device.
--
-- Ending a grant, for whatever reason and wherever it is done, also
-- removes its live device (the trigger below), so no device is shown live
-- for a sign-in that is over.
--
-- New nullable columns without a default and checks on them: no row is
-- rewritten or scanned, and each lock is held for a moment only.

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
-- For rows written from now on only: a default given with ADD COLUMN would
-- fill the rows there are with this moment.
ALTER TABLE devices ALTER COLUMN signed_in_at SET DEFAULT now();

CREATE FUNCTION devices_end_with_grant() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE devices SET revoked_at = NEW.revoked_at WHERE grant_id = NEW.id AND revoked_at IS NULL;
    RETURN NULL;
END
$$;
CREATE TRIGGER oauth_grants_end_devices AFTER UPDATE OF revoked_at ON oauth_grants
    FOR EACH ROW WHEN (OLD.revoked_at IS NULL AND NEW.revoked_at IS NOT NULL)
    EXECUTE FUNCTION devices_end_with_grant();
