# buwiz-cloud

Accounts, TIN activation and sign-in for the Buwiz apps. It replaces the
Spin-based `buwiz-server`, keeping only what the apps call.

- **Accounts**: sign in with a 6-digit code sent by email. No passwords, no
  organizations, no workspaces. One account can hold several TINs.
- **TIN activation**: the person uploads BIR Form 2303, a government ID and
  any supporting documents. Staff review each claim by hand. An approved
  claim makes the TIN active, and a TIN has at most one owner. Files are
  encrypted at rest (AES-256-GCM) and deleted when the claim is decided or
  withdrawn. Their hashes stay.
- **App sign-in**: the desktop and mini apps use the OAuth device flow
  (RFC 8628). Refresh tokens rotate, and reusing an old one ends the whole
  sign-in.
- **Licence gate**: `GET /api/tax-profiles` lists the account's active TINs.
  The apps unlock their BIR tools when any profile has permissions.

Built with [axum](https://github.com/tokio-rs/axum) for the API and
[Topcoat](https://github.com/tokio-rs/topcoat) for the account site, on
Postgres.

## Run it locally

Requires Rust 1.99 and Docker.

```sh
docker compose up -d                       # Postgres on 127.0.0.1:54330
cp .env.example .env                       # then set DOCUMENT_KEY:
echo "DOCUMENT_KEY=$(openssl rand -base64 32)" >> .env
set -a; . ./.env; set +a
cargo run                                  # http://localhost:3008
```

Migrations run at start. With `MAIL_TRANSPORT=log`, sign-in codes appear in
the server log. Put your email in `ADMIN_EMAILS` to see **Review claims**.

Smoke test of the whole flow (device sign-in, claim, review, licence gate,
refresh rotation):

```sh
PORT=3018 ACCOUNT_ORIGIN=http://localhost:3018 ADMIN_EMAILS=admin@example.test \
  cargo run > server.log 2>&1 &
BASE=http://localhost:3018 LOG=server.log scripts/smoke.sh
```

## What the apps call

The mini app reads `BUWIZ_SERVER_URL` (API) and `BUWIZ_ACCOUNT_URL` (browser
pages). One deployment can serve both hostnames. Set `ACCOUNT_ORIGIN` to the
account hostname and list the API hostname in `EXTRA_ORIGINS`.

| Call | Notes |
|---|---|
| `POST /auth/device/start` `{"client_id"}` | Also `POST /oauth/device/code`. Clients: `buwiz-desktop`, `buwiz-books`. |
| `POST /auth/device/poll` `{"client_id","device_code"}` | `authorization_pending`, `slow_down`, `access_denied`, `expired_token`, then tokens. |
| `POST /oauth/token` (form) | `grant_type=refresh_token` or `urn:ietf:params:oauth:grant-type:device_code`. Access tokens last 15 minutes, refresh tokens 90 days. |
| `GET /me` | `{"user_id","email"}`. 401 means signed out. |
| `GET /api/tax-profiles` | `{"profiles":[{"id","tin","tin_masked","registered_name","activated_at","permissions"}]}`. Link by `id`, not the TIN. |
| `POST /api/auth/logout` | Ends this app's sign-in. |

Browser pages the apps open: `/login?next=…`, `/signup?next=…`,
`/device?user_code=…`.

## Configuration

See [`.env.example`](.env.example). Required: `DATABASE_URL` and
`DOCUMENT_KEY`. Public deployments must use `https://` in `ACCOUNT_ORIGIN`,
which turns on secure cookies, and `MAIL_TRANSPORT=resend`. The server
refuses to start if it would print codes on a public deployment.

## Deploy

The `Dockerfile` builds a small image that listens on `$PORT` (8080 in the
image). Any container host with Postgres works.

**Wasmer** is the long-term target, but it can't run this yet (tested
2026-10-29 with Wasmer 7.5 and the WASIX toolchain `v2026-09-18.1`, Rust 1.97):

- Topcoat 0.11 needs tokio ≥ 1.51. The WASIX registry's patched tokio stops at 1.47.
- Outbound TLS (rustls with ring) completes the handshake, then reads EOF.
  The same code works natively, and plain TCP works under WASIX.
- tokio-postgres doesn't compile for wasm without a patch.

axum, outbound TCP and an SSH handshake (russh, with a one-line `russh-util`
patch) do work under WASIX. Revisit when WASIX tokio catches up.

## Next

1. Submission relay for buwiz-oss: tickets bound to an active TIN, BIR
   secrets kept on the server, SSH run end-to-end through a pipe from the
   user's computer (see buwiz-oss `internal/PLAN.md` §5).
2. MCP endpoint and OAuth for AI agents (PKCE, dynamic client registration).
3. Port the full marketing home page.
