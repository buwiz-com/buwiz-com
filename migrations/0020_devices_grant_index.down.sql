-- Back to migration 19. Run this file before 0019's. It commits on its own
-- and never refuses.
--
--   psql -v ON_ERROR_STOP=1 -1 -c "SET lock_timeout = '10s'" -f 0020_devices_grant_index.down.sql

DROP INDEX IF EXISTS devices_grant_id;

-- See 0011_tin_states.down.sql.
DELETE FROM _sqlx_migrations WHERE version = 20;
