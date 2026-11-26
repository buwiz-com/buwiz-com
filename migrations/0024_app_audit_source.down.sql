-- Back to migration 23. docs/deploy.md has the whole rollback; run this
-- file before 0023's (and the older ones, to go further). It commits on
-- its own.
--
--   psql -v ON_ERROR_STOP=1 -1 -c "SET lock_timeout = '10s'" -f 0024_app_audit_source.down.sql
--
-- It puts back migration 11's check on `claim_audit.source` ('web',
-- 'auto', 'seed'), exactly.
--
-- It refuses while any audit line came from the app (source 'app'): the
-- release before this one cannot hold one, and the line is the only
-- record of who added that TIN, and how. It deletes nothing and changes
-- nothing then. Do not delete or rewrite those lines to get past it: once
-- one exists, the rollback to 23 is not available, and the way is forward
-- only (docs/deploy.md, "Migration 24").

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM claim_audit WHERE source = 'app') THEN
        RAISE EXCEPTION 'refusing to roll back: % claim_audit line(s) came from the app (source app), which the release before this one cannot hold. Stop the rollback. Do not delete or change these lines: they are the record of TINs the app added. Go forward instead. docs/deploy.md (Migration 24) says what to do.',
            (SELECT count(*) FROM claim_audit WHERE source = 'app');
    END IF;
END $$;

ALTER TABLE claim_audit DROP CONSTRAINT claim_audit_source_check;
ALTER TABLE claim_audit ADD CONSTRAINT claim_audit_source_check CHECK (source IN ('web', 'auto', 'seed'));

-- See 0011_tin_states.down.sql.
DELETE FROM _sqlx_migrations WHERE version = 24;
