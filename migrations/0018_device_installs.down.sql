-- Back to migration 17. docs/deploy.md has the whole rollback; run this
-- file before 0017's (and the older ones, to go further). It commits on
-- its own and never refuses.
--
--   psql -v ON_ERROR_STOP=1 -1 -c "SET lock_timeout = '10s'" -f 0018_device_installs.down.sql
--
-- What it loses: the install ids, and what the apps said about their
-- devices (platform, model, system version, computer name, last sign-in).
-- Every device row and grant stays and works as before; a device updated
-- in place keeps its newest key. After the way up, an install's next
-- registration adds a row again, as before this release.

DROP INDEX devices_grant;
DROP INDEX devices_account_install;
ALTER TABLE devices
    DROP COLUMN install_id,
    DROP COLUMN platform,
    DROP COLUMN model,
    DROP COLUMN os_version,
    DROP COLUMN computer_name,
    DROP COLUMN signed_in_at;
ALTER TABLE device_authorizations DROP COLUMN install_id;
ALTER TABLE oauth_grants DROP COLUMN install_id, DROP COLUMN platform;

-- See 0011_tin_states.down.sql.
DELETE FROM _sqlx_migrations WHERE version = 18;
