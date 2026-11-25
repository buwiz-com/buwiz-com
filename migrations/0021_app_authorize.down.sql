-- Back to migration 20. docs/deploy.md has the whole rollback; run this
-- file before 0020's (and the older ones, to go further). It commits on
-- its own. With the service stopped, as docs/deploy.md says, it does not
-- fail. On a running server it can: a code an app gets after its DELETE
-- makes VALIDATE fail, and the whole file is rolled back, leaving the
-- database at 21. A lock not had within the lock timeout fails it the
-- same way, changing nothing.
--
--   psql -v ON_ERROR_STOP=1 -1 -c "SET lock_timeout = '10s'" -f 0021_app_authorize.down.sql
--
-- What it loses: the apps' codes not yet exchanged (at most ten minutes
-- old; the app signs in again). Agents' codes stay. Apps already signed in
-- keep their tokens, and their grants keep the install and platform.
-- Devices lose who removed them: a removed device that signs in again
-- after the rollback is not mailed.

DELETE FROM oauth_codes WHERE client_id NOT IN (SELECT client_id FROM oauth_clients);
ALTER TABLE oauth_codes DROP COLUMN install_id, DROP COLUMN platform;
ALTER TABLE oauth_codes ADD CONSTRAINT oauth_codes_client_id_fkey
    FOREIGN KEY (client_id) REFERENCES oauth_clients (client_id) ON DELETE CASCADE NOT VALID;
ALTER TABLE oauth_codes VALIDATE CONSTRAINT oauth_codes_client_id_fkey;
ALTER TABLE devices DROP COLUMN removed_by;

-- See 0011_tin_states.down.sql.
DELETE FROM _sqlx_migrations WHERE version = 21;
