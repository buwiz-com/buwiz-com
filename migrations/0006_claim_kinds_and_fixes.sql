-- Personal and business TINs, claims sent back for a fix, and the documents
-- a personal TIN is proved with.

-- A personal TIN is proved with a TIN ID; a business TIN with Form 2303 and
-- a government ID. Every claim so far came with a 2303.
ALTER TABLE tin_claims ADD COLUMN kind text NOT NULL DEFAULT 'business' CHECK (kind IN ('personal', 'business'));
ALTER TABLE tin_claims ALTER COLUMN kind DROP DEFAULT;
ALTER TABLE tin_claims ADD COLUMN trade_name text;

-- Staff can send a claim back (`needs_fix`) with reasons and a note. The
-- claimant replaces files and resends it, and it is `pending` again. The
-- reasons stay so the reviewer sees what was asked.
ALTER TABLE tin_claims DROP CONSTRAINT tin_claims_status_check;
ALTER TABLE tin_claims ADD CONSTRAINT tin_claims_status_check
    CHECK (status IN ('pending', 'needs_fix', 'approved', 'rejected', 'withdrawn'));
ALTER TABLE tin_claims ADD COLUMN fix_reasons text[] NOT NULL DEFAULT '{}'
    CHECK (fix_reasons <@ ARRAY['blurry', 'name_mismatch', 'wrong_document', 'tin_mismatch', 'authority_missing']);
ALTER TABLE tin_claims ADD COLUMN fix_note text;
ALTER TABLE tin_claims ADD COLUMN sent_back_at timestamptz;
ALTER TABLE tin_claims ADD COLUMN sent_back_by uuid REFERENCES accounts (id);
ALTER TABLE tin_claims ADD COLUMN resent_at timestamptz;

-- A claim waiting for a fix is still open: one per account and TIN.
DROP INDEX tin_claims_one_open;
CREATE UNIQUE INDEX tin_claims_one_open ON tin_claims (account_id, tin_root, branch_code)
    WHERE status IN ('pending', 'needs_fix');

-- TIN ID (card or ORUS digital), selfie and National ID photo for personal
-- TINs; proof of authority for a 2303 in a company's name.
ALTER TABLE claim_documents DROP CONSTRAINT claim_documents_kind_check;
ALTER TABLE claim_documents ADD CONSTRAINT claim_documents_kind_check
    CHECK (kind IN ('form_2303', 'government_id', 'supporting', 'tin_id', 'selfie', 'national_id', 'authority'));
