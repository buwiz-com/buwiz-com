-- Resetting a forgotten app PIN by email code (docs/accounts.md,
-- "Resetting a forgotten app PIN"). The BIR app asks for a code
-- (`POST /api/pin-reset/start`), which is mailed to the account's own
-- address, and checks it (`POST /api/pin-reset/verify`). The PIN itself
-- lives on the device; the server never sees it.
--
-- - `email_codes.purpose` also takes 'pin_reset' (migration 17's check:
--   'sign_in', 'delete', 'app_sign_in'). A code works only for its own
--   purpose, so a reset code never signs in or deletes, and no other code
--   resets a PIN.
-- - `account_audit.action` also takes 'pin_reset_verified' (migration
--   16's list), and `account_audit.source` also takes 'app' (migration
--   12's check: 'web', 'seed'): each verified reset writes one line, by the
--   account, from the app. Migration 24 changed only claim_audit's source.
-- - `pin_resets` holds each request: the account, the device it was made
--   on (NULL for a practice account, a store reviewer's with a fixed code,
--   which registers no device), the sign-in (app grant) it was made with,
--   the device's name for the notice, its code, when it stops working and
--   when it was used. Whether a row may have no device depends on its
--   account, which a CHECK cannot read; the code enforces it, and tests
--   pin it. The index serves the day's count per account.
--
-- Each wider check is added NOT VALID and validated before the old one is
-- dropped, so no column is ever without a check, and then takes the old
-- one's name. In this one transaction that saves no lock: adding a check
-- takes the ACCESS EXCLUSIVE lock on its table, and the validation scan
-- runs under it. `email_codes` holds 30 days of codes and `account_audit`
-- is small, so each scan is short, and every row passes (each new list
-- holds the old one). The new table comes last: its references take a
-- lock on `accounts`, `devices`, `oauth_grants` and `email_codes` that
-- holds back their writes (every signed request updates its device row)
-- until this short transaction commits. Behind a transaction open on one
-- of these tables it fails within the 10 s lock timeout below, records
-- nothing, and starting again works.

SET LOCAL lock_timeout = '10s';

ALTER TABLE email_codes ADD CONSTRAINT email_codes_purpose_pin_reset_check
    CHECK (purpose IN ('sign_in', 'delete', 'app_sign_in', 'pin_reset')) NOT VALID;
ALTER TABLE email_codes VALIDATE CONSTRAINT email_codes_purpose_pin_reset_check;
ALTER TABLE email_codes DROP CONSTRAINT email_codes_purpose_check;
ALTER TABLE email_codes RENAME CONSTRAINT email_codes_purpose_pin_reset_check TO email_codes_purpose_check;

ALTER TABLE account_audit ADD CONSTRAINT account_audit_action_pin_reset_check
    CHECK (action IN ('banned', 'unbanned', 'role_changed', 'test_marked', 'test_unmarked', 'signed_out',
                      'app_signed_out', 'created', 'fixed_code_set', 'fixed_code_changed', 'fixed_code_cleared',
                      'deleted', 'pin_reset_verified')) NOT VALID;
ALTER TABLE account_audit VALIDATE CONSTRAINT account_audit_action_pin_reset_check;
ALTER TABLE account_audit DROP CONSTRAINT account_audit_action_check;
ALTER TABLE account_audit RENAME CONSTRAINT account_audit_action_pin_reset_check TO account_audit_action_check;

ALTER TABLE account_audit ADD CONSTRAINT account_audit_source_app_check
    CHECK (source IN ('web', 'seed', 'app')) NOT VALID;
ALTER TABLE account_audit VALIDATE CONSTRAINT account_audit_source_app_check;
ALTER TABLE account_audit DROP CONSTRAINT account_audit_source_check;
ALTER TABLE account_audit RENAME CONSTRAINT account_audit_source_app_check TO account_audit_source_check;

CREATE TABLE pin_resets (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id  uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    -- The device the request was made on; NULL for a practice account.
    device_id   uuid REFERENCES devices (id) ON DELETE CASCADE,
    -- The sign-in it was made with: a practice account's request is bound
    -- to it.
    grant_id    uuid NOT NULL REFERENCES oauth_grants (id) ON DELETE CASCADE,
    -- The name the notice uses: the device row's, or what a practice
    -- account's app reported ('this device' when nothing).
    device_name text NOT NULL,
    -- NULL once the hourly upkeep deleted the code, long after it expired.
    code_id     uuid REFERENCES email_codes (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    -- When its code stops working.
    expires_at  timestamptz NOT NULL,
    used_at     timestamptz
);
CREATE INDEX pin_resets_account ON pin_resets (account_id, created_at);
CREATE INDEX pin_resets_code_id ON pin_resets (code_id);
