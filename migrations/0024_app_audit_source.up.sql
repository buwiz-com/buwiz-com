-- An audit line may come from the BIR app: `claim_audit.source` also
-- takes 'app', for a TIN the app adds as not verified
-- (`POST /api/tax-profiles`, docs/accounts.md). Migration 11's check
-- (`claim_audit_source_check`) took 'web', 'auto' and 'seed' only. No row
-- changes and no column: the check is made wider, under the same name.
--
-- The wider check is added NOT VALID and validated before the old one is
-- dropped, so the column is never without a check. In this one
-- transaction that saves no lock: adding a check takes the ACCESS
-- EXCLUSIVE lock on claim_audit, and the validation scan runs under it.
-- Writes of audit lines (and reads of the history) queue behind that lock
-- for at most the 10 s lock timeout below; the table is small, and every
-- row passes (the new list holds the old one). Behind a transaction open
-- on the table it fails, records nothing, and starting again works.

SET LOCAL lock_timeout = '10s';

ALTER TABLE claim_audit ADD CONSTRAINT claim_audit_source_app_check
    CHECK (source IN ('web', 'auto', 'seed', 'app')) NOT VALID;
ALTER TABLE claim_audit VALIDATE CONSTRAINT claim_audit_source_app_check;
ALTER TABLE claim_audit DROP CONSTRAINT claim_audit_source_check;
ALTER TABLE claim_audit RENAME CONSTRAINT claim_audit_source_app_check TO claim_audit_source_check;
