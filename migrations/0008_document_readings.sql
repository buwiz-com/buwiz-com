-- What OCR on this server read from a claim's TIN ID or Form 2303: only the
-- fields staff compare (TIN, names, birth date, RDO code, the kind of
-- document), how sure the engine was of each, and how the TIN and name
-- compare with what the claimant typed. Never the whole text. One reading
-- per document; reading it again replaces it.
CREATE TABLE document_readings (
    -- The queued job: a reading finishing after the document was queued
    -- again does not overwrite the newer one.
    id             uuid PRIMARY KEY,
    document_id    uuid NOT NULL UNIQUE REFERENCES claim_documents (id) ON DELETE CASCADE,
    status         text NOT NULL CHECK (status IN ('reading', 'read', 'failed')),
    engine         text,
    engine_version text,
    fields         jsonb,
    confidence     jsonb,
    tin_match      text CHECK (tin_match IN ('match', 'partial', 'mismatch')),
    name_match     text CHECK (name_match IN ('match', 'partial', 'mismatch')),
    error          text,
    created_at     timestamptz NOT NULL DEFAULT now(),
    read_at        timestamptz
);

-- A reading goes with its document's bytes: when a claim is decided or
-- withdrawn, or the file replaced or removed, whichever code purges it.
CREATE FUNCTION forget_document_reading() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM document_readings WHERE document_id = NEW.id;
    RETURN NEW;
END
$$;
CREATE TRIGGER claim_documents_purged AFTER UPDATE OF purged_at ON claim_documents
    FOR EACH ROW WHEN (NEW.purged_at IS NOT NULL) EXECUTE FUNCTION forget_document_reading();
