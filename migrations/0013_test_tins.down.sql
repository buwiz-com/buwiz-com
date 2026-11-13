-- Back to migration 12. docs/deploy.md has the whole rollback; run this
-- file before 0012's and 0011's:
--
--   psql -v ON_ERROR_STOP=1 -1 -f 0013_test_tins.down.sql
--
-- It refuses while any test TIN (root 000-000-000) has a row, ended ones
-- too: the release before this one cannot read such a TIN, and every staff
-- page that lists one (and the account's own TIN list) would stop with an
-- error. Nothing was ever sent for one, so the rows can go: revoke each on
-- the staff pages (so the history keeps who and why), then delete them
-- with their tickets and returns, which were all refused:
--
--   DELETE FROM submissions WHERE claim_id IN (SELECT id FROM tin_claims WHERE tin_root = '000000000');
--   DELETE FROM tickets WHERE claim_id IN (SELECT id FROM tin_claims WHERE tin_root = '000000000');
--   DELETE FROM tin_claims WHERE tin_root = '000000000';
--
-- Their audit lines stay, with the TIN masked as ***-***-000-…, which the
-- release before this one reads as it reads any.

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM tin_claims WHERE tin_root = '000000000') THEN
        RAISE EXCEPTION 'refusing to roll back: % row(s) hold a test TIN (000-000-000), which the release before this one cannot read. Revoke them, then delete them as this file says.',
            (SELECT count(*) FROM tin_claims WHERE tin_root = '000000000');
    END IF;
END $$;

ALTER TABLE tin_claims DROP CONSTRAINT tin_claims_test_tin_check;

-- See 0011_tin_states.down.sql.
DELETE FROM _sqlx_migrations WHERE version = 13;
