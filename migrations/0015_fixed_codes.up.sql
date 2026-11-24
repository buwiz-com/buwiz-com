-- A fixed sign-in code for a test account (docs/accounts.md, "Fixed
-- sign-in codes for test accounts"): the app stores' reviewers cannot read
-- a mailbox of ours, so an Admin sets a six-digit code on the account's
-- staff page, and while it stands no code is mailed to that address.
--
-- Only its hash is kept, `code_hash(email, code)` in
-- `src/account/accounts.rs`, the same form as `email_codes.code_hash`, so
-- a code asked for copies the hash into its `email_codes` row and the
-- plain code is never stored. Who set it and when are kept beside it; the
-- history of each change is in `account_audit`, without the code.
--
-- New nullable columns: no existing row changes. The checks scan
-- `accounts` and `account_audit`, both small.

-- A stray open transaction on these tables (a pre-flight `BEGIN` left
-- open) makes start-up fail with a lock timeout instead of hanging.
SET LOCAL lock_timeout = '10s';

ALTER TABLE accounts ADD COLUMN fixed_code_hash bytea;
ALTER TABLE accounts ADD COLUMN fixed_code_set_by uuid REFERENCES accounts (id);
ALTER TABLE accounts ADD COLUMN fixed_code_set_at timestamptz;
-- A fixed code always says who set it and when, is a SHA-256 hash, and
-- lives on a test account only: the label cannot come off while one is
-- set. Each part is IS NULL or IS NOT NULL, so the check is never NULL
-- (a check that is NULL passes).
ALTER TABLE accounts ADD CONSTRAINT accounts_fixed_code_check CHECK (
    (fixed_code_hash IS NULL AND fixed_code_set_by IS NULL AND fixed_code_set_at IS NULL)
    OR (fixed_code_hash IS NOT NULL AND octet_length(fixed_code_hash) = 32 AND fixed_code_set_by IS NOT NULL
        AND fixed_code_set_at IS NOT NULL AND test_account));

ALTER TABLE account_audit DROP CONSTRAINT account_audit_action_check;
ALTER TABLE account_audit ADD CONSTRAINT account_audit_action_check
    CHECK (action IN ('banned', 'unbanned', 'role_changed', 'test_marked', 'test_unmarked', 'signed_out',
                      'app_signed_out', 'created', 'fixed_code_set', 'fixed_code_changed', 'fixed_code_cleared'));
