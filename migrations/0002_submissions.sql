-- Returns sent to BIR through the relay. The plaintext IAF XML is kept,
-- encrypted, only until it is sent or 24 hours pass.

CREATE TABLE submissions (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id      uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    claim_id        uuid NOT NULL REFERENCES tin_claims (id),
    idempotency_key text,
    form_type       text NOT NULL,
    period          text NOT NULL,
    filename        text NOT NULL,
    xml_sha256      bytea NOT NULL,
    xml_nonce       bytea,
    xml_ciphertext  bytea,
    status          text NOT NULL CHECK (status IN ('ready', 'sending', 'sent', 'dry_run', 'failed', 'unknown', 'expired')),
    mode            text,
    attempts        int NOT NULL DEFAULT 0,
    error           text,
    remote_path     text,
    payload_sha256  text,
    receipt_jws     text,
    sending_since   timestamptz,
    sent_at         timestamptz,
    purged_at       timestamptz,
    created_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (account_id, idempotency_key)
);
CREATE INDEX submissions_account ON submissions (account_id, created_at DESC);
CREATE INDEX submissions_open ON submissions (created_at) WHERE xml_ciphertext IS NOT NULL;
