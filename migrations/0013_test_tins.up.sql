-- Test TINs: a made-up TIN whose root is all zeros (`000-000-000`, at any
-- branch), which staff add to a test account and which never sends. No
-- column marks one: the root does, and the send gate reads it there, so
-- nothing can mark a real TIN as a test one or forget to.
--
-- Only an Admin (on the staff pages) or the seed tool makes one, active at
-- once and without papers, and it only ever ends from there (removed by its
-- owner, revoked by staff). So a test TIN's row is one of theirs
-- (`verified_by` `admin` or `seed`) whose status is `approved`, `removed`
-- or `revoked`: never a claim waiting with papers or for a fix, one saved
-- without them, one the automatic check made, nor a rejected or withdrawn
-- one. The forms people use refuse such a TIN, and this check says so in
-- the database too. No row has such a root yet (every form refused it).

-- A stray open transaction on this table (a pre-flight `BEGIN` left open)
-- makes start-up fail with a lock timeout instead of hanging.
SET LOCAL lock_timeout = '10s';

ALTER TABLE tin_claims ADD CONSTRAINT tin_claims_test_tin_check
    CHECK (tin_root <> '000000000'
           OR (status IN ('approved', 'removed', 'revoked') AND coalesce(verified_by, '') IN ('admin', 'seed')));
