-- no-transaction
-- One device row per account and install (migration 18,
-- docs/accounts.md "Devices"), built CONCURRENTLY so signed requests,
-- which update `devices`, are never blocked while it is built. It runs
-- outside a transaction (the first line), so this file holds this one
-- statement only. If it fails halfway, Postgres leaves an invalid index
-- of that name: docs/deploy.md says how to drop it and start again.
CREATE UNIQUE INDEX CONCURRENTLY devices_account_install ON devices (account_id, install_id)
    WHERE install_id IS NOT NULL
