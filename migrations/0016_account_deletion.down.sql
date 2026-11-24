-- Back to migration 15. docs/deploy.md has the whole rollback; run this
-- file before 0015's (and the older ones, to go further). It commits on
-- its own and never refuses.
--
--   psql -v ON_ERROR_STOP=1 -1 -c "SET lock_timeout = '10s'" -f 0016_account_deletion.down.sql
--
-- What it loses: the codes mailed for deleting an account. The release
-- before this one would take them for sign-in codes, so they are deleted,
-- not kept; whoever was deleting their account asks for a new code after
-- the way up. Sign-in codes stay and work as before.
--
-- What it cannot undo: an account deleted at 16 stays deleted. Its rows
-- are gone; only its `deleted` line says it was there.
--
-- What it keeps elsewhere: the `deleted` lines (`account_audit`, which the
-- release before this one cannot hold) move to
-- `rollback_archive.account_audit`, the table 0012's and 0015's down files
-- also add to, and stay there; migrating up again does not bring them
-- back. They hold an account id and no address.

CREATE SCHEMA IF NOT EXISTS rollback_archive;
CREATE TABLE IF NOT EXISTS rollback_archive.account_audit AS
    SELECT now() AS archived_at, * FROM account_audit WITH NO DATA;
INSERT INTO rollback_archive.account_audit (archived_at, id, account_id, action, old_value, new_value, actor_id,
    actor_email, source, reason, created_at)
    SELECT now(), id, account_id, action, old_value, new_value, actor_id, actor_email, source, reason, created_at
      FROM account_audit WHERE action = 'deleted';
DELETE FROM account_audit WHERE action = 'deleted';

ALTER TABLE account_audit DROP CONSTRAINT account_audit_action_check;
ALTER TABLE account_audit ADD CONSTRAINT account_audit_action_check
    CHECK (action IN ('banned', 'unbanned', 'role_changed', 'test_marked', 'test_unmarked', 'signed_out',
                      'app_signed_out', 'created', 'fixed_code_set', 'fixed_code_changed', 'fixed_code_cleared'));

DELETE FROM email_codes WHERE purpose = 'delete';
ALTER TABLE email_codes DROP COLUMN purpose;

-- See 0011_tin_states.down.sql.
DELETE FROM _sqlx_migrations WHERE version = 16;
