-- Submission v2: device keys, signed tickets and sealed envelopes.

-- An Ed25519 key an app registered after device-flow sign-in. Requests on
-- the v2 endpoints carry a signature from it. Revoking a device also ends
-- the sign-in it was registered under.
CREATE TABLE devices (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id   uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    grant_id     uuid NOT NULL REFERENCES oauth_grants (id) ON DELETE CASCADE,
    public_key   bytea NOT NULL UNIQUE CHECK (length(public_key) = 32),
    name         text NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz,
    revoked_at   timestamptz
);
CREATE INDEX devices_account ON devices (account_id, created_at DESC);

-- Signature nonces seen in the last few minutes, per key. A second use is
-- a replay.
CREATE TABLE request_nonces (
    public_key bytea NOT NULL,
    nonce      text NOT NULL,
    seen_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (public_key, nonce)
);

-- Tickets the Gatekeeper signed: one return, one device, five minutes, one
-- use.
CREATE TABLE tickets (
    jti             uuid PRIMARY KEY,
    account_id      uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    device_id       uuid NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
    claim_id        uuid NOT NULL REFERENCES tin_claims (id),
    form_type       text NOT NULL,
    period          text NOT NULL,
    payload_sha256  bytea NOT NULL,
    email           text NOT NULL,
    idempotency_key text NOT NULL,
    jws             text NOT NULL,
    expires_at      timestamptz NOT NULL,
    submission_id   uuid REFERENCES submissions (id) ON DELETE SET NULL,
    used_at         timestamptz,
    created_at      timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX tickets_account_created ON tickets (account_id, created_at DESC);
CREATE INDEX tickets_idempotency ON tickets (account_id, idempotency_key, created_at DESC);

-- v2 rows keep the HPKE envelope, opaque to this server, instead of the XML.
ALTER TABLE submissions
    ADD COLUMN ticket_jti uuid UNIQUE,
    ADD COLUMN device_id uuid REFERENCES devices (id),
    ADD COLUMN envelope text;
