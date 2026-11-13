-- Back to migration 11; then 0011_tin_states.down.sql goes back to 10.
--
-- What a rollback loses: every role (ADMIN_EMAILS alone makes admins
-- again), every ban (banned accounts can sign in and send again), the test
-- labels, the history of what staff did to accounts, and the record of
-- seed runs. Accounts, TINs and returns stay.

DROP TABLE seed_runs;
DROP TABLE account_audit;

ALTER TABLE accounts DROP CONSTRAINT accounts_ban_check;
ALTER TABLE accounts DROP COLUMN ban_reason;
ALTER TABLE accounts DROP COLUMN banned_by;
ALTER TABLE accounts DROP COLUMN banned_at;
ALTER TABLE accounts DROP COLUMN test_account;
ALTER TABLE accounts DROP COLUMN role;

-- See 0011_tin_states.down.sql.
DELETE FROM _sqlx_migrations WHERE version = 12;
