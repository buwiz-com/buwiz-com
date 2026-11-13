-- Back to migration 11; then 0011_tin_states.down.sql goes back to 10.
-- docs/deploy.md has the whole rollback.
--
-- sqlx commits each down file on its own, so this one first makes every
-- check 0011's down file makes: a rollback either goes all the way to 10
-- or changes nothing. It also refuses while an account is banned (the
-- release before this one would let it sign in and send again: lift each
-- ban first) and while any account has a stored role (the release before
-- this one knows only ADMIN_EMAILS, and if that no longer lists those
-- people it has no staff). Run it with:
--
--   psql -v ON_ERROR_STOP=1 -1 -f 0012_staff_roles.down.sql
--
-- Only if it refuses for stored roles, and ADMIN_EMAILS names your admins,
-- drop the roles on purpose: run it again with `buwiz.rollback_drop_roles`
-- set to `yes` in the same transaction:
--
--   psql -v ON_ERROR_STOP=1 -1 -c "SET LOCAL buwiz.rollback_drop_roles = 'yes'" -f 0012_staff_roles.down.sql
--
-- What a rollback loses: every role (ADMIN_EMAILS alone makes admins
-- again) and the test labels. The history of what staff did to accounts
-- (`account_audit`) and the record of seed runs (`seed_runs`) move, whole,
-- to the `rollback_archive` schema, which the release before this one does
-- not read. Accounts, TINs and returns stay.

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM accounts WHERE banned_at IS NOT NULL) THEN
        RAISE EXCEPTION 'refusing to roll back: % account(s) are banned, and the release before this one would let them sign in and send. Lift each ban first.',
            (SELECT count(*) FROM accounts WHERE banned_at IS NOT NULL);
    END IF;
END $$;

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

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM accounts WHERE role IS NOT NULL)
       AND coalesce(current_setting('buwiz.rollback_drop_roles', true), '') <> 'yes' THEN
        RAISE EXCEPTION 'refusing to roll back: % account(s) have a stored staff role, which the release before this one does not know. Make sure ADMIN_EMAILS names your admins, then set buwiz.rollback_drop_roles to yes.',
            (SELECT count(*) FROM accounts WHERE role IS NOT NULL);
    END IF;
END $$;

CREATE SCHEMA IF NOT EXISTS rollback_archive;
CREATE TABLE IF NOT EXISTS rollback_archive.account_audit AS
    SELECT now() AS archived_at, * FROM account_audit WITH NO DATA;
INSERT INTO rollback_archive.account_audit SELECT now(), * FROM account_audit;
CREATE TABLE IF NOT EXISTS rollback_archive.seed_runs AS
    SELECT now() AS archived_at, * FROM seed_runs WITH NO DATA;
INSERT INTO rollback_archive.seed_runs SELECT now(), * FROM seed_runs;

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
