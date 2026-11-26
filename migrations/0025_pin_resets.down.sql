-- Back to migration 24. docs/deploy.md has the whole rollback; run this
-- file before 0024's (and the older ones, to go further). It commits on
-- its own.
--
--   psql -v ON_ERROR_STOP=1 -1 -c "SET lock_timeout = '10s'" -f 0025_pin_resets.down.sql
--
-- It puts back migration 17's check on `email_codes.purpose`, migration
-- 16's on `account_audit.action` and migration 12's on
-- `account_audit.source`, exactly, and drops `pin_resets`.
--
-- It refuses while any PIN reset is recorded: a `pin_resets` row, an
-- `account_audit` line with action 'pin_reset_verified' or source 'app',
-- or an `email_codes` row with purpose 'pin_reset'. The release before
-- this one cannot hold them, and the audit lines are the only record that
-- a reset was verified, on which device, and that the account was told.
-- It deletes nothing and changes nothing then. Do not delete or rewrite
-- those rows to get past it: once one exists, the rollback to 24 is not
-- available, and the way is forward only (docs/deploy.md, "Migration
-- 25").

DO $$
DECLARE
    requests bigint := (SELECT count(*) FROM pin_resets);
    lines bigint := (SELECT count(*) FROM account_audit WHERE action = 'pin_reset_verified' OR source = 'app');
    codes bigint := (SELECT count(*) FROM email_codes WHERE purpose = 'pin_reset');
BEGIN
    IF requests > 0 OR lines > 0 OR codes > 0 THEN
        RAISE EXCEPTION 'refusing to roll back: % pin_resets row(s), % account_audit line(s) of a PIN reset (action pin_reset_verified or source app) and % email_codes row(s) of purpose pin_reset exist, which the release before this one cannot hold. Stop the rollback. Do not delete or change these rows: the audit lines are the record of verified PIN resets. Go forward instead. docs/deploy.md (Migration 25) says what to do.',
            requests, lines, codes;
    END IF;
END $$;

DROP TABLE pin_resets;

ALTER TABLE email_codes DROP CONSTRAINT email_codes_purpose_check;
ALTER TABLE email_codes ADD CONSTRAINT email_codes_purpose_check
    CHECK (purpose IN ('sign_in', 'delete', 'app_sign_in'));

ALTER TABLE account_audit DROP CONSTRAINT account_audit_action_check;
ALTER TABLE account_audit ADD CONSTRAINT account_audit_action_check
    CHECK (action IN ('banned', 'unbanned', 'role_changed', 'test_marked', 'test_unmarked', 'signed_out',
                      'app_signed_out', 'created', 'fixed_code_set', 'fixed_code_changed', 'fixed_code_cleared',
                      'deleted'));

ALTER TABLE account_audit DROP CONSTRAINT account_audit_source_check;
ALTER TABLE account_audit ADD CONSTRAINT account_audit_source_check CHECK (source IN ('web', 'seed'));

-- See 0011_tin_states.down.sql.
DELETE FROM _sqlx_migrations WHERE version = 25;
