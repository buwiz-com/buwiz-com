-- Back to migration 11; then 0011_tin_states.down.sql goes back to 10.
--
-- It refuses to run while an account is banned: the release before this
-- one would let it sign in and send again. Lift each ban first.
--
-- What a rollback loses: every role (ADMIN_EMAILS alone makes admins
-- again), the test labels, the history of what staff did to accounts, and
-- the record of seed runs. Accounts, TINs and returns stay.

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM accounts WHERE banned_at IS NOT NULL) THEN
        RAISE EXCEPTION 'refusing to roll back: % account(s) are banned, and the release before this one would let them sign in and send. Lift each ban first.',
            (SELECT count(*) FROM accounts WHERE banned_at IS NOT NULL);
    END IF;
END $$;

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
