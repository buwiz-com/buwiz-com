-- TINs added without papers (`unverified`), how each TIN became active,
-- ownership paused by staff, what the automatic check saw, and one live row
-- per account and TIN. Every row already here came with its papers and was
-- decided by staff, so nothing about it changes but the new columns.

ALTER TABLE tin_claims DROP CONSTRAINT tin_claims_status_check;
ALTER TABLE tin_claims ADD CONSTRAINT tin_claims_status_check
    CHECK (status IN ('unverified', 'pending', 'needs_fix', 'approved', 'rejected', 'withdrawn', 'removed', 'revoked'));

-- An active TIN always says since when: the TIN list reads it.
ALTER TABLE tin_claims ADD CONSTRAINT tin_claims_decided_check
    CHECK (status <> 'approved' OR decided_at IS NOT NULL);

-- When the papers were sent: the review queue's order. NULL while not
-- verified, and for a TIN activated without papers.
ALTER TABLE tin_claims ADD COLUMN submitted_at timestamptz;
UPDATE tin_claims SET submitted_at = created_at;
ALTER TABLE tin_claims ADD CONSTRAINT tin_claims_submitted_check
    CHECK (status <> 'unverified' OR submitted_at IS NULL);

-- Who made it active: staff with papers, the automatic check, an admin
-- without papers, or the seed tool. Kept once the ownership ends.
ALTER TABLE tin_claims ADD COLUMN verified_by text CHECK (verified_by IN ('staff', 'auto', 'admin', 'seed'));
UPDATE tin_claims SET verified_by = 'staff' WHERE status IN ('approved', 'removed', 'revoked');
ALTER TABLE tin_claims ADD CONSTRAINT tin_claims_verified_check
    CHECK (status NOT IN ('approved', 'removed', 'revoked') OR verified_by IS NOT NULL);

-- Paused: still owned (still in tin_claims_one_owner), but nothing sends.
-- Ending the ownership clears the pause.
ALTER TABLE tin_claims ADD COLUMN paused_at timestamptz;
ALTER TABLE tin_claims ADD COLUMN paused_by uuid REFERENCES accounts (id);
ALTER TABLE tin_claims ADD COLUMN pause_reason text;
ALTER TABLE tin_claims ADD CONSTRAINT tin_claims_paused_check
    CHECK ((paused_at IS NULL OR status = 'approved') AND (paused_at IS NULL) = (pause_reason IS NULL));

-- The automatic check: what it saw and why it did or did not activate; and
-- the staff member who confirmed an automatic activation afterwards.
ALTER TABLE tin_claims ADD COLUMN auto_check jsonb;
ALTER TABLE tin_claims ADD COLUMN auto_confirmed_at timestamptz;
ALTER TABLE tin_claims ADD COLUMN auto_confirmed_by uuid REFERENCES accounts (id);
ALTER TABLE tin_claims ADD CONSTRAINT tin_claims_auto_confirmed_check
    CHECK (auto_confirmed_at IS NULL OR verified_by = 'auto');

-- One live row per account and TIN. Other accounts' unverified rows do not
-- count: several accounts may add the same TIN without papers, and only an
-- approved row owns it (tin_claims_one_owner).
DROP INDEX tin_claims_one_open;
CREATE UNIQUE INDEX tin_claims_one_live ON tin_claims (account_id, tin_root, branch_code)
    WHERE status IN ('unverified', 'pending', 'needs_fix', 'approved');
CREATE INDEX tin_claims_tin ON tin_claims (tin_root, branch_code);

-- New claim actions, and where an action came from.
ALTER TABLE claim_audit DROP CONSTRAINT claim_audit_action_check;
ALTER TABLE claim_audit ADD CONSTRAINT claim_audit_action_check
    CHECK (action IN ('deleted', 'removed', 'revoked', 'file_replaced', 'file_removed',
                      'activated', 'auto_activated', 'auto_confirmed', 'paused', 'resumed'));
ALTER TABLE claim_audit ADD COLUMN source text NOT NULL DEFAULT 'web' CHECK (source IN ('web', 'auto', 'seed'));
