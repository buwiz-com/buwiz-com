-- Accounts, sign-in, OAuth grants for the Buwiz apps, and TIN claims.

CREATE TABLE accounts (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    email      text NOT NULL UNIQUE CHECK (email = lower(email)),
    created_at timestamptz NOT NULL DEFAULT now()
);

-- One-time sign-in codes sent by email. Only a hash is stored.
CREATE TABLE email_codes (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    email       text NOT NULL,
    code_hash   bytea NOT NULL,
    attempts    int NOT NULL DEFAULT 0,
    expires_at  timestamptz NOT NULL,
    consumed_at timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX email_codes_email_created ON email_codes (email, created_at DESC);

-- Browser sessions on the account site.
CREATE TABLE web_sessions (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    token_hash bytea NOT NULL UNIQUE,
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- RFC 8628 device authorization requests from the desktop apps.
CREATE TABLE device_authorizations (
    id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    client_id        text NOT NULL,
    device_code_hash bytea NOT NULL UNIQUE,
    user_code        text NOT NULL UNIQUE,
    interval_seconds int NOT NULL,
    expires_at       timestamptz NOT NULL,
    last_polled_at   timestamptz,
    account_id       uuid REFERENCES accounts (id) ON DELETE CASCADE,
    decision         text CHECK (decision IN ('approved', 'denied')),
    consumed_at      timestamptz,
    created_at       timestamptz NOT NULL DEFAULT now()
);

-- One sign-in of one app. Revoking it ends every token issued under it.
CREATE TABLE oauth_grants (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id    uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    client_id     text NOT NULL,
    revoked_at    timestamptz,
    revoke_reason text,
    created_at    timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE oauth_access_tokens (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    grant_id   uuid NOT NULL REFERENCES oauth_grants (id) ON DELETE CASCADE,
    token_hash bytea NOT NULL UNIQUE,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- Refresh tokens rotate: each one works once. A second use revokes the grant.
CREATE TABLE oauth_refresh_tokens (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    grant_id   uuid NOT NULL REFERENCES oauth_grants (id) ON DELETE CASCADE,
    token_hash bytea NOT NULL UNIQUE,
    expires_at timestamptz NOT NULL,
    used_at    timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- A claim that an account owns a TIN (9-digit root + 5-digit branch).
-- An approved claim is an active tax profile.
CREATE TABLE tin_claims (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id      uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    tin_root        text NOT NULL CHECK (tin_root ~ '^[0-9]{9}$'),
    branch_code     text NOT NULL CHECK (branch_code ~ '^[0-9]{5}$'),
    registered_name text NOT NULL,
    status          text NOT NULL CHECK (status IN ('pending', 'approved', 'rejected', 'withdrawn')),
    decided_at      timestamptz,
    decided_by      uuid REFERENCES accounts (id),
    decision_note   text,
    created_at      timestamptz NOT NULL DEFAULT now()
);
-- One TIN, one owner.
CREATE UNIQUE INDEX tin_claims_one_owner ON tin_claims (tin_root, branch_code) WHERE status = 'approved';
-- One open claim per account and TIN.
CREATE UNIQUE INDEX tin_claims_one_open ON tin_claims (account_id, tin_root, branch_code) WHERE status = 'pending';
CREATE INDEX tin_claims_account ON tin_claims (account_id, created_at DESC);

-- Files sent with a claim, encrypted at rest. The bytes are dropped when the
-- claim ends; the hash and metadata stay.
CREATE TABLE claim_documents (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    claim_id     uuid NOT NULL REFERENCES tin_claims (id) ON DELETE CASCADE,
    kind         text NOT NULL CHECK (kind IN ('form_2303', 'government_id', 'supporting')),
    filename     text NOT NULL,
    content_type text NOT NULL,
    size_bytes   int NOT NULL,
    sha256       bytea NOT NULL,
    nonce        bytea,
    ciphertext   bytea,
    purged_at    timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX claim_documents_claim ON claim_documents (claim_id);
