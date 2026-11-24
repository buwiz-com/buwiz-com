-- People delete their own account (docs/accounts.md, "Deleting an
-- account"): on `/delete` they ask for a code, it is mailed to the
-- account's address, and typing it deletes the account.
--
-- That code is kept in `email_codes` like a sign-in code, with its
-- `purpose`: `sign_in` (every row so far, and the default) or `delete`. A
-- code works only for its own purpose, so a sign-in code never deletes an
-- account and a deletion code never signs anyone in. Both share the limits
-- of the address (five codes an hour), ten minutes and five wrong guesses.
--
-- The account's history keeps a `deleted` line, which names the account by
-- its id only (`account_audit` has no foreign keys, so it outlives the
-- account).
--
-- The new column has a constant default, so no row is rewritten; its check
-- and the new action check scan `email_codes` and `account_audit`, both
-- small (the hourly upkeep deletes codes after 30 days).

-- A stray open transaction on these tables (a pre-flight `BEGIN` left
-- open) makes start-up fail with a lock timeout instead of hanging.
SET LOCAL lock_timeout = '10s';

ALTER TABLE email_codes ADD COLUMN purpose text NOT NULL DEFAULT 'sign_in'
    CHECK (purpose IN ('sign_in', 'delete'));

ALTER TABLE account_audit DROP CONSTRAINT account_audit_action_check;
ALTER TABLE account_audit ADD CONSTRAINT account_audit_action_check
    CHECK (action IN ('banned', 'unbanned', 'role_changed', 'test_marked', 'test_unmarked', 'signed_out',
                      'app_signed_out', 'created', 'fixed_code_set', 'fixed_code_changed', 'fixed_code_cleared',
                      'deleted'));
