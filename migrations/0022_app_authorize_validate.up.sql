-- Checks the rows already there against migration 21's checks
-- (oauth_codes' install and platform, devices' removed_by), which 21 added
-- NOT VALID. Its own migration, so its own transaction: VALIDATE takes a
-- SHARE UPDATE EXCLUSIVE lock, which lets reads and writes go on, signed
-- requests' updates of their device included. In 21's transaction it
-- would have run under 21's ACCESS EXCLUSIVE lock instead. Every row is
-- read once; the columns are new, so every one passes.
--
-- The lock_timeout below bounds the wait for the lock, held by schema
-- changes, other VALIDATEs, VACUUM and ANALYZE (an autovacuum gives way,
-- unless it runs to prevent wraparound), not by plain reads and writes.
-- Behind one it fails, records nothing, and starting again works.

SET LOCAL lock_timeout = '10s';

ALTER TABLE oauth_codes VALIDATE CONSTRAINT oauth_codes_install_id_check;
ALTER TABLE oauth_codes VALIDATE CONSTRAINT oauth_codes_platform_check;
ALTER TABLE devices VALIDATE CONSTRAINT devices_removed_by_check;
