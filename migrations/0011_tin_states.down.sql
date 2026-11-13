-- Back to migration 10, so the release before this one starts on the
-- database with every return, ticket and receipt kept. Run after
-- 0012_staff_roles.down.sql. `MIGRATOR.undo(&pool, 10)` runs both; by hand,
-- run each file with psql (`psql -v ON_ERROR_STOP=1 -1 -f …`) in that order.
-- docs/deploy.md has the whole rollback.
--
-- It refuses to run while a TIN is paused (the release before this one
-- would send for it again), while a TIN saved without papers has files
-- (deleting the row would delete them silently, and leave their stored
-- objects behind), or while a TIN the automatic check activated waits for
-- a person to confirm it (the release before this one has no such list,
-- and its papers would be kept with nobody to look at them). Resume or end
-- the pauses, decide those files, and confirm those TINs first (on the
-- "Activated automatically" tab; revoking one does not confirm it).
-- 0012's down file makes the same checks before it changes anything, so a
-- rollback either goes all the way to 10 or changes nothing.
--
-- What a rollback loses:
-- - TINs added without papers (`unverified`). They never have tickets,
--   returns or files, so nothing refers to them; deleting one fails loudly
--   if a ticket or return does.
-- - How each TIN became active (`verified_by`), when papers were sent, and
--   what the automatic check saw. Migrating up again labels each TIN that
--   was ever active `staff` when it names a reviewer (`decided_by`: staff
--   approval, an admin, the seed tool) and `unknown` when it does not (the
--   automatic check).
-- - Each audit line's source and claim kind, from the lines that stay.
-- Audit lines of the new actions (activated, paused, approved, …) are not
-- lost: they move, whole, to `rollback_archive.claim_audit`, outside the
-- schema the release before this one reads. They are the only record of
-- why a TIN that stays active was activated without papers.

-- BEGIN checks of 0011 (0012's down file repeats this block word for word)
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM tin_claims WHERE paused_at IS NOT NULL) THEN
        RAISE EXCEPTION 'refusing to roll back: % TIN(s) are paused, and the release before this one would send for them. Resume or end each pause first.',
            (SELECT count(*) FROM tin_claims WHERE paused_at IS NOT NULL);
    END IF;
    IF EXISTS (SELECT 1 FROM claim_documents d JOIN tin_claims c ON c.id = d.claim_id WHERE c.status = 'unverified') THEN
        RAISE EXCEPTION 'refusing to roll back: TINs saved without papers have files, which deleting them would delete silently. Deal with those claims first.';
    END IF;
    IF EXISTS (SELECT 1 FROM tin_claims WHERE verified_by = 'auto' AND auto_confirmed_at IS NULL) THEN
        RAISE EXCEPTION 'refusing to roll back: % TIN(s) activated automatically wait for a person to confirm them. Confirm each on the "Activated automatically" tab first, revoked and removed ones too: revoking one does not confirm it.',
            (SELECT count(*) FROM tin_claims WHERE verified_by = 'auto' AND auto_confirmed_at IS NULL);
    END IF;
END $$;
-- END checks of 0011

CREATE SCHEMA IF NOT EXISTS rollback_archive;
CREATE TABLE IF NOT EXISTS rollback_archive.claim_audit AS
    SELECT now() AS archived_at, * FROM claim_audit WITH NO DATA;
INSERT INTO rollback_archive.claim_audit
    SELECT now(), * FROM claim_audit
     WHERE action NOT IN ('deleted', 'removed', 'revoked', 'file_replaced', 'file_removed');

DELETE FROM tin_claims WHERE status = 'unverified';
DELETE FROM claim_audit
 WHERE action NOT IN ('deleted', 'removed', 'revoked', 'file_replaced', 'file_removed');

ALTER TABLE claim_audit DROP COLUMN claim_kind;
ALTER TABLE claim_audit DROP COLUMN source;
ALTER TABLE claim_audit DROP CONSTRAINT claim_audit_action_check;
ALTER TABLE claim_audit ADD CONSTRAINT claim_audit_action_check
    CHECK (action IN ('deleted', 'removed', 'revoked', 'file_replaced', 'file_removed'));

DROP INDEX tin_claims_tin;
DROP INDEX tin_claims_one_live;
CREATE UNIQUE INDEX tin_claims_one_open ON tin_claims (account_id, tin_root, branch_code)
    WHERE status IN ('pending', 'needs_fix');

ALTER TABLE tin_claims DROP COLUMN auto_confirmed_by;
ALTER TABLE tin_claims DROP COLUMN auto_confirmed_at;
ALTER TABLE tin_claims DROP COLUMN auto_check;
ALTER TABLE tin_claims DROP COLUMN pause_reason;
ALTER TABLE tin_claims DROP COLUMN paused_by;
ALTER TABLE tin_claims DROP COLUMN paused_at;
ALTER TABLE tin_claims DROP COLUMN verified_by;
ALTER TABLE tin_claims DROP COLUMN submitted_at;
ALTER TABLE tin_claims DROP CONSTRAINT tin_claims_decided_check;

ALTER TABLE tin_claims DROP CONSTRAINT tin_claims_status_check;
ALTER TABLE tin_claims ADD CONSTRAINT tin_claims_status_check
    CHECK (status IN ('pending', 'needs_fix', 'approved', 'rejected', 'withdrawn', 'removed', 'revoked'));

-- The release before this one refuses to start while a version it does not
-- know is recorded. sqlx deletes this line itself when it reverts; it is
-- here for a run by hand.
DELETE FROM _sqlx_migrations WHERE version = 11;
