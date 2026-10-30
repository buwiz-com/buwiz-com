-- Ending a claim: the claimant deletes a claim that never became active or
-- removes an active TIN; staff delete a claim or revoke an ownership. Also
-- claimants replacing or removing single files while a claim is open.

-- A claim that was approved keeps its row once the ownership ends, because
-- its sent returns refer to it: `removed` when the owner gave the TIN up,
-- `revoked` when staff ended it. Neither is in the one-owner or one-open
-- index, so the TIN can be claimed again. Claims that never became active
-- are deleted outright instead.
ALTER TABLE tin_claims DROP CONSTRAINT tin_claims_status_check;
ALTER TABLE tin_claims ADD CONSTRAINT tin_claims_status_check
    CHECK (status IN ('pending', 'needs_fix', 'approved', 'rejected', 'withdrawn', 'removed', 'revoked'));
ALTER TABLE tin_claims ADD COLUMN removed_at timestamptz;
ALTER TABLE tin_claims ADD COLUMN removed_by uuid REFERENCES accounts (id);
-- Staff's reason for a revoke. The owner sees it on My TINs.
ALTER TABLE tin_claims ADD COLUMN removal_note text;
-- The claimant last replaced or removed a file. Staff see it when it is
-- newer than the claim's place in the queue.
ALTER TABLE tin_claims ADD COLUMN files_changed_at timestamptz;

-- Who ended or changed what. Rows outlive the claim (a deleted claim's id
-- dangles) and the accounts, so nothing here is a foreign key. The TIN is
-- kept masked only.
CREATE TABLE claim_audit (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    claim_id      uuid NOT NULL,
    -- The claim's owner, whose history shows the line.
    account_id    uuid NOT NULL,
    tin_masked    text NOT NULL,
    action        text NOT NULL CHECK (action IN ('deleted', 'removed', 'revoked', 'file_replaced', 'file_removed')),
    -- The claim's status before a delete, remove or revoke.
    claim_status  text NOT NULL,
    -- The file's kind, for file actions.
    document_kind text,
    actor_id      uuid NOT NULL,
    actor_email   text NOT NULL,
    reason        text,
    created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX claim_audit_account ON claim_audit (account_id, created_at DESC);
