-- Staff roles, bans and the test label on accounts; a history of what staff
-- did to accounts; and a record of each seed run. No account has a role or
-- a ban yet: ADMIN_EMAILS still makes its people admins.

-- A stray open transaction on these tables (a pre-flight `BEGIN` left
-- open) makes start-up fail with a lock timeout instead of hanging.
SET LOCAL lock_timeout = '10s';

ALTER TABLE accounts ADD COLUMN role text CHECK (role IN ('admin', 'support'));
ALTER TABLE accounts ADD COLUMN test_account boolean NOT NULL DEFAULT false;
ALTER TABLE accounts ADD COLUMN banned_at timestamptz;
ALTER TABLE accounts ADD COLUMN banned_by uuid REFERENCES accounts (id);
ALTER TABLE accounts ADD COLUMN ban_reason text;
-- A ban always says who banned the account and why (a reason of nothing
-- but whitespace, of any kind, is none).
ALTER TABLE accounts ADD CONSTRAINT accounts_ban_check CHECK (
    (banned_at IS NULL AND banned_by IS NULL AND ban_reason IS NULL)
    OR (banned_at IS NOT NULL AND banned_by IS NOT NULL AND coalesce(ban_reason, '') ~ '[^\t\n\v\f\r \u0085   -     　]'));

-- Like claim_audit: no foreign keys, so lines outlive what they describe.
CREATE TABLE account_audit (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id  uuid NOT NULL,
    action      text NOT NULL CHECK (action IN ('banned', 'unbanned', 'role_changed', 'test_marked',
                                                'test_unmarked', 'signed_out', 'app_signed_out', 'created')),
    old_value   text,
    new_value   text,
    actor_id    uuid NOT NULL,
    actor_email text NOT NULL,
    -- Set by every writer; no default.
    source      text NOT NULL CHECK (source IN ('web', 'seed')),
    reason      text,
    -- When the line was written, not when its transaction began: a change
    -- that began before a seed run and committed after it is the newer
    -- one, and the seed's next run keeps it.
    created_at  timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX account_audit_account ON account_audit (account_id, created_at DESC);

-- One row per run of `buwiz-cloud seed`: who ran it, the file's hash and
-- what it changed. `actor_*` is the `--as` Admin, which anyone with the
-- database's address can type; `os_user` and `host_name` are who ran it
-- as far as that machine knew (the OS user, and the one behind sudo), and
-- `target` the database it wrote to (`host:port/name`). No TIN is stored
-- in full: `summary` masks them.
CREATE TABLE seed_runs (
    id          uuid PRIMARY KEY,
    actor_id    uuid NOT NULL,
    actor_email text NOT NULL,
    os_user     text NOT NULL,
    host_name   text NOT NULL,
    target      text NOT NULL,
    file_sha256 bytea NOT NULL,
    -- The digest the run printed, which `--expect` compares.
    plan_digest text NOT NULL,
    summary     jsonb NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
