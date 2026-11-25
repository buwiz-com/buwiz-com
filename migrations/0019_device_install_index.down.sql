-- Back to migration 18. Run this file before 0018's. It commits on its own
-- and never refuses.
--
--   psql -v ON_ERROR_STOP=1 -1 -c "SET lock_timeout = '10s'" -f 0019_device_install_index.down.sql

DROP INDEX IF EXISTS devices_account_install;

-- See 0011_tin_states.down.sql.
DELETE FROM _sqlx_migrations WHERE version = 19;
