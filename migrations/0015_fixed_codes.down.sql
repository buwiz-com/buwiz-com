-- Back to migration 14. docs/deploy.md has the whole rollback; run this
-- file before 0014's (and the older ones, to go further). It commits on
-- its own and never refuses.
--
--   psql -v ON_ERROR_STOP=1 -1 -c "SET lock_timeout = '10s'" -f 0015_fixed_codes.down.sql
--
-- What it loses: every fixed sign-in code, with who set it and when. That
-- is acceptable: the release before this one has no fixed codes, so those
-- test accounts get a random, mailed code again, and an Admin sets the
-- codes again after the way up. Tell the stores first if a review is
-- under way.
--
-- What it keeps elsewhere: the history lines of fixed codes set, changed
-- and cleared (`account_audit`, which the release before this one cannot
-- hold) move to `rollback_archive.account_audit`, the table 0012's down
-- file also adds to, and stay there; migrating up again does not bring
-- them back. They never held a code or its hash.

CREATE SCHEMA IF NOT EXISTS rollback_archive;
CREATE TABLE IF NOT EXISTS rollback_archive.account_audit AS
    SELECT now() AS archived_at, * FROM account_audit WITH NO DATA;
INSERT INTO rollback_archive.account_audit (archived_at, id, account_id, action, old_value, new_value, actor_id,
    actor_email, source, reason, created_at)
    SELECT now(), id, account_id, action, old_value, new_value, actor_id, actor_email, source, reason, created_at
      FROM account_audit WHERE action IN ('fixed_code_set', 'fixed_code_changed', 'fixed_code_cleared');
DELETE FROM account_audit WHERE action IN ('fixed_code_set', 'fixed_code_changed', 'fixed_code_cleared');

ALTER TABLE account_audit DROP CONSTRAINT account_audit_action_check;
ALTER TABLE account_audit ADD CONSTRAINT account_audit_action_check
    CHECK (action IN ('banned', 'unbanned', 'role_changed', 'test_marked', 'test_unmarked', 'signed_out',
                      'app_signed_out', 'created'));

ALTER TABLE accounts DROP CONSTRAINT accounts_fixed_code_check;
ALTER TABLE accounts DROP COLUMN fixed_code_set_at;
ALTER TABLE accounts DROP COLUMN fixed_code_set_by;
ALTER TABLE accounts DROP COLUMN fixed_code_hash;

-- See 0011_tin_states.down.sql.
DELETE FROM _sqlx_migrations WHERE version = 15;
