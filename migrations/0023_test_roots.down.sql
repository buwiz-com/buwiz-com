-- Back to migration 22. docs/deploy.md has the whole rollback; run this
-- file before 0022's (and the older ones, to go further). It commits on
-- its own.
--
--   psql -v ON_ERROR_STOP=1 -1 -c "SET lock_timeout = '10s'" -f 0023_test_roots.down.sql
--
-- It puts back migration 13's check, on the root 000-000-000 alone.
--
-- It refuses while any row holds a root reserved by 0023 and not by 13
-- (111-111-114), ended rows too: the release before this one takes such a
-- TIN for a real one, so its next send would go to the BIR. It deletes
-- nothing and changes nothing then. Do not delete those rows, nor their
-- submissions or tickets, to get past it: the rollback to 22 is not
-- available while such a row exists (docs/deploy.md, "Migration 23",
-- "Rolling back to 22"). Stop, keep the account's submissions and tickets
-- exactly as they are, end each such claim the normal staff way with a
-- reason (removed or revoked), and write to the owner, who decides.

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM tin_claims WHERE tin_root = '111111114') THEN
        RAISE EXCEPTION 'refusing to roll back: % row(s) hold the test root 111-111-114, which the release before this one takes for a real TIN. Stop the rollback. Do not delete these rows or their submissions and tickets: keep them as they are, end each claim on the staff pages with a reason (removed or revoked), and write to the owner, who decides. docs/deploy.md (Migration 23) says what to do.',
            (SELECT count(*) FROM tin_claims WHERE tin_root = '111111114');
    END IF;
END $$;

ALTER TABLE tin_claims DROP CONSTRAINT tin_claims_test_tin_check;

ALTER TABLE tin_claims ADD CONSTRAINT tin_claims_test_tin_check
    CHECK (tin_root <> '000000000'
           OR (status IN ('approved', 'removed', 'revoked') AND coalesce(verified_by, '') IN ('admin', 'seed')));

-- See 0011_tin_states.down.sql.
DELETE FROM _sqlx_migrations WHERE version = 23;
