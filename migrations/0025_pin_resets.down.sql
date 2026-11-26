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
-- The way back is forward only once the first reset is verified;
-- unverified starts age out in 30 days. Precisely, it refuses, deleting
-- and changing nothing, while any of these exists: a `pin_resets` row
-- that was verified (`used_at` set), an `account_audit` line with action
-- 'pin_reset_verified' or source 'app', or an `email_codes` row of purpose
-- 'pin_reset' that was used (`consumed_at` set). The audit lines are kept
-- forever, so once one reset is verified the refusal is for good: they
-- are the only record that a reset was verified, and on which device
-- (whether the account was told is `pin_resets.notified_at`, which goes
-- with the table). Do not delete or rewrite them to get past it
-- (docs/deploy.md, "Migration 25").
--
-- Starts that were never verified do not block: this file deletes them
-- (their `pin_resets` rows, with the table, and their unused codes), as
-- the hourly upkeep would 30 days on. The release before this one cannot
-- use them; an app that asked asks again after the way up.

DO $$
DECLARE
    verified bigint := (SELECT count(*) FROM pin_resets WHERE used_at IS NOT NULL);
    lines bigint := (SELECT count(*) FROM account_audit WHERE action = 'pin_reset_verified' OR source = 'app');
    used bigint := (SELECT count(*) FROM email_codes WHERE purpose = 'pin_reset' AND consumed_at IS NOT NULL);
BEGIN
    IF verified > 0 OR lines > 0 OR used > 0 THEN
        RAISE EXCEPTION 'refusing to roll back: a PIN reset was verified (% verified pin_resets row(s), % account_audit line(s) of a PIN reset (action pin_reset_verified or source app), % used email_codes row(s) of purpose pin_reset), which the release before this one cannot hold. The way back is forward only once the first reset is verified; unverified starts age out in 30 days. Stop the rollback. Do not delete or change these rows: the audit lines are the record of verified PIN resets. Go forward instead. docs/deploy.md (Migration 25) says what to do.',
            verified, lines, used;
    END IF;
END $$;

DROP TABLE pin_resets;
DELETE FROM email_codes WHERE purpose = 'pin_reset';

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
