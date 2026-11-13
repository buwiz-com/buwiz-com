-- Back to migration 10, so the release before this one starts on the
-- database with every return, ticket and receipt kept. Run after
-- 0012_staff_roles.down.sql. `MIGRATOR.undo(&pool, 10)` runs both; by hand,
-- run each file with psql (`psql -v ON_ERROR_STOP=1 -1 -f …`) in that order.
--
-- What a rollback loses:
-- - TINs added without papers (`unverified`). They never have tickets or
--   returns, so nothing refers to them; deleting one fails loudly if
--   something does.
-- - Pauses: a paused TIN is active again.
-- - How each TIN became active (`verified_by`), when papers were sent, and
--   what the automatic check saw.
-- - Audit lines of the new actions (activated, paused, …) and every line's
--   source.

DELETE FROM tin_claims WHERE status = 'unverified';
DELETE FROM claim_audit
 WHERE action NOT IN ('deleted', 'removed', 'revoked', 'file_replaced', 'file_removed');

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
