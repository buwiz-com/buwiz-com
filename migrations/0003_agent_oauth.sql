-- AI agents sign in with OAuth (authorization code + PKCE) after registering
-- themselves (RFC 7591). Their tokens carry the `mcp` scope: read-only.

ALTER TABLE oauth_grants ADD COLUMN scope text NOT NULL DEFAULT 'app';

CREATE TABLE oauth_clients (
    client_id     text PRIMARY KEY,
    client_name   text NOT NULL,
    redirect_uris text[] NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE oauth_codes (
    code_hash      bytea PRIMARY KEY,
    client_id      text NOT NULL REFERENCES oauth_clients (client_id) ON DELETE CASCADE,
    account_id     uuid NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    redirect_uri   text NOT NULL,
    code_challenge text NOT NULL,
    scope          text NOT NULL,
    expires_at     timestamptz NOT NULL,
    used_at        timestamptz
);
