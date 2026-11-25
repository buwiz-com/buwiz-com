-- Back to migration 21: its checks become NOT VALID again, as 21 left
-- them. Run this file before 0021's. It commits on its own, reads no row,
-- and holds each table's ACCESS EXCLUSIVE lock for a moment only. With
-- the service stopped, as docs/deploy.md says, it does not fail; a lock
-- not had within the lock timeout fails it, changing nothing.
--
--   psql -v ON_ERROR_STOP=1 -1 -c "SET lock_timeout = '10s'" -f 0022_app_authorize_validate.down.sql

ALTER TABLE oauth_codes
    DROP CONSTRAINT oauth_codes_install_id_check,
    ADD CONSTRAINT oauth_codes_install_id_check CHECK (install_id ~ '^[A-Za-z0-9_-]{16,64}$') NOT VALID,
    DROP CONSTRAINT oauth_codes_platform_check,
    ADD CONSTRAINT oauth_codes_platform_check
        CHECK (platform IN ('ios', 'ipados', 'android', 'macos', 'windows', 'linux')) NOT VALID;
ALTER TABLE devices
    DROP CONSTRAINT devices_removed_by_check,
    ADD CONSTRAINT devices_removed_by_check CHECK (removed_by IN ('owner', 'staff')) NOT VALID;

-- See 0011_tin_states.down.sql.
DELETE FROM _sqlx_migrations WHERE version = 22;
