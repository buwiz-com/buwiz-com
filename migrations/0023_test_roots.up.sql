-- Reserved test roots: a test TIN is a TIN whose root is one of a short
-- list, at any branch, no longer only `000-000-000`. The list is
-- `TEST_ROOTS` in src/account/tin.rs, and the two lists below are the same
-- one (a unit test there reads this file). `111-111-114` is a root the
-- eBIRForms app accepts as an ordinary TIN, so a release build of the app
-- can queue a test return; the app refuses all zeros.
--
-- Migration 13's check, over the list: a row with a reserved root is one an
-- Admin (on the staff pages) or the seed tool made active, active or ended
-- from there (`approved`, `removed`, `revoked`). The forms people use
-- refuse a reserved root, and this says so in the database too.
--
-- It refuses to run while a row holds a reserved root but is not such a
-- row, and names each one, for an operator to handle by hand
-- (docs/deploy.md, "Migration 23"). A row with a root reserved only here
-- (not all zeros) is refused whatever its status: before this migration no
-- one could add a test TIN with that root, so any such row is a real
-- person's claim, and it must never silently become a dry run. Such a row
-- stops the deploy; returns for it may already be real filings, so its
-- account's submissions and tickets are kept exactly as they are (the
-- runbook says what happens next).
--
-- The reservation assumes 111-111-114 is nobody's real TIN; if it is
-- someone's, that person cannot register it and is sent to support, and a
-- claim already holding it stops the deploy.

-- A stray open transaction on this table (a pre-flight `BEGIN` left open)
-- makes start-up fail with a lock timeout instead of hanging.
SET LOCAL lock_timeout = '10s';

DO $$
DECLARE
    offending text;
BEGIN
    SELECT string_agg(format('id=%s account_id=%s root=%s branch=%s status=%s verified_by=%s',
                             c.id, c.account_id, c.tin_root, c.branch_code, c.status,
                             coalesce(c.verified_by, '(none)')),
                      '; ' ORDER BY c.created_at, c.id)
      INTO offending
      FROM tin_claims c
     WHERE c.tin_root IN ('000000000', '111111114')
       AND (c.tin_root <> '000000000'
            OR NOT (c.status IN ('approved', 'removed', 'revoked') AND coalesce(c.verified_by, '') IN ('admin', 'seed')));
    IF offending IS NOT NULL THEN
        RAISE EXCEPTION 'refusing to reserve the test roots: these tin_claims rows hold a reserved root and are not test TINs: %. Stop the deploy. Do not delete these rows or their submissions and tickets: keep them as they are, end each claim on the staff pages with a reason (removed or revoked), and write to the owner, who decides. docs/deploy.md (Migration 23) says what to do.',
            offending;
    END IF;
END $$;

-- The new check is added NOT VALID and then validated, but in this one
-- transaction that gains nothing: DROP CONSTRAINT already holds the ACCESS
-- EXCLUSIVE lock on tin_claims, and the VALIDATE scan runs under it. Reads
-- on tin_claims queue behind that lock, for at most the 10 s lock timeout
-- above, and the table is small. VALIDATE still catches a row committed
-- between the DO block and the lock.
ALTER TABLE tin_claims DROP CONSTRAINT tin_claims_test_tin_check;

ALTER TABLE tin_claims ADD CONSTRAINT tin_claims_test_tin_check
    CHECK (tin_root NOT IN ('000000000', '111111114')
           OR (status IN ('approved', 'removed', 'revoked') AND coalesce(verified_by, '') IN ('admin', 'seed')))
    NOT VALID;

ALTER TABLE tin_claims VALIDATE CONSTRAINT tin_claims_test_tin_check;
