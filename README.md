# buwiz-cloud

Accounts, TIN activation and sign-in for the Buwiz apps. It replaces the
Spin-based `buwiz-server`, keeping only what the apps call.

- **Accounts**: sign in with a 6-digit code sent by email. No passwords, no
  organizations, no workspaces. One account can hold several TINs.
- **TIN activation**: the person says whose TIN it is and uploads its papers.
  - Personal: a TIN ID (card or ORUS digital TIN ID); a selfie and a photo
    of the National ID are optional and strengthen the check.
  - Business: BIR Form 2303 and a government ID, plus proof of authority
    (secretary's certificate, board resolution or SPA) when the 2303 is in
    a company's name.
  - Staff review each claim by hand at `/staff`: approve, reject, or send
    it back with reasons; the person fixes it from My TINs (`/tins`) and
    resends it. An approved claim makes the TIN active, and a TIN has at
    most one owner. People can prepare returns right away; sending them
    waits for approval.
  - While a claim waits, the person can replace any file or remove an
    optional one from its Files page; staff see that files changed after
    sending. A claim that never became active can be deleted, files and
    all, by its owner or by staff with a reason. An active TIN is removed
    by its owner or revoked by staff: it stops being active, its sent
    returns stay, and it can be claimed again. Each of these leaves a line
    (masked TIN) in the account's history on the staff pages.
  - Files are encrypted (AES-256-GCM) before they are stored, in Cloudflare
    R2 in production (any S3-compatible store; Postgres in development),
    and deleted when the claim is decided or withdrawn, or the file is
    replaced. Their hashes stay.
- **App sign-in**: the desktop and mini apps use the OAuth device flow
  (RFC 8628). Refresh tokens rotate, and reusing an old one ends the whole
  sign-in.
- **Licence gate**: `GET /api/tax-profiles` lists the account's active TINs.
  The apps unlock their BIR tools when any profile has permissions.
  `?include=pending` also lists TINs still being checked or sent back, with
  a `status` and no `filing.submit` permission.
- **Submission relay**: files a return with BIR for an active TIN.
  - Two processes: the public **Gatekeeper** (`buwiz-cloud`) and the
    **Sender** (`buwiz-sender`), which alone keeps BIR's passphrases, the
    SFTP login and the HPKE private key, and does the IAF encryption
    (byte-identical to eBIRForms' `Encrypt.exe`).
  - v2: the app signs requests with a registered device key, gets a
    five-minute ticket for one return, and seals the XML to the Sender
    (HPKE). The Gatekeeper stores the envelope without being able to open
    it. v1 (plaintext XML over TLS) still works.
  - The traffic leaves from the taxpayer's computer, so BIR sees their own
    connection. The app is a dumb pipe that connects only to BIR's hosts.
  - SSH runs end to end through it, with BIR's host key pinned.
  - Each send ends in a signed receipt. See
    [docs/relay-protocol.md](docs/relay-protocol.md).
  - The BIR-specific code (BIR's dispatcher, the IAF encryption, the SFTP
    upload, BIR's host list, the v1 reference client) is behind the `bir`
    cargo feature, in files kept out of the public repository. BIR's hosts
    and the other BIR details are documented with those files. See
    [Builds with and without BIR](#builds-with-and-without-bir).
- **MCP for AI agents**: `POST /mcp` with four read-only tools: active TINs,
  TIN claims, sent returns, and one return with its receipt.
  - Agents find the sign-in through the 401's `WWW-Authenticate` metadata
    (RFC 9728 / RFC 8414), register themselves (RFC 7591), and sign in
    with authorization code + PKCE after the person approves on a consent
    page.
  - Agent tokens carry the `mcp` scope and can never file a return.

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

Migrations run at start. Without `SENDER_URL`, the Sender runs inside this
process (development only; see [docs/relay-protocol.md](docs/relay-protocol.md)
§6 for running `buwiz-sender` on its own). Built without the `bir` feature,
that Sender answers every send with `failed`, "relay not built in". Without `RESEND_API_KEY`, sign-in codes appear in
the server log; with it, they are emailed. Put your email in `ADMIN_EMAILS` to see
the staff pages at `/staff` (claims queue and users); everyone else gets a 404 there.

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
| `GET /api/tax-profiles` | `{"profiles":[{"id","tin","tin_masked","registered_name","activated_at","permissions"}]}`. Link by `id`, not the TIN. `?include=pending` adds open claims and a `status` (`approved`, `pending`, `needs_fix`) to every profile; open ones have `activated_at: null` and can prepare but not submit. |
| `POST /api/auth/logout` | Ends this app's sign-in. |
| `POST /api/submissions`, `GET /api/submissions[/{id}]` | Create and read submissions. |
| `GET /api/submissions/{id}/relay` (WebSocket) | The relay pipe. See [docs/relay-protocol.md](docs/relay-protocol.md). |
| `GET /.well-known/jwks.json` | Key that verifies receipts. |
| `POST /api/devices`, `GET /api/devices`, `POST /api/devices/{id}/revoke` | Device keys (v2). |
| `POST /api/tickets` | A signed ticket for one return (v2, device-signed). |
| `POST /api/v2/submissions` | `{ticket, envelope}` (v2, device-signed). |
| `GET /.well-known/buwiz-hpke.json` | The Sender's sealing key (v2). |
| `POST /mcp` | MCP (Streamable HTTP, JSON answers). App and agent tokens both work; tools only read. |
| `GET /.well-known/oauth-protected-resource[/mcp]`, `/.well-known/oauth-authorization-server` | OAuth discovery for agents. |
| `POST /oauth/register`, `GET /oauth/authorize` | Agent registration and consent (PKCE S256). |

Browser pages the apps open: `/login?next=…`, `/signup?next=…`,
`/device?user_code=…`.

## Builds with and without BIR

| | `cargo build` (public) | `cargo build --features bir` (production) |
|---|---|---|
| Needs the private BIR files (`src/bir/`, `src/relay/client.rs`, `examples/iaf-decrypt.rs`, `tests/relay.rs`, `tests/iaf_golden.rs`, `tests/fixtures/iaf/`) | No | Yes |
| Accounts, TIN claims, devices, tickets, HPKE sealing, trust list, receipts | Yes | Yes |
| Sender (`buwiz-sender` and the in-process development Sender) | Checks tickets and opens envelopes, then answers every send `failed`, "relay not built in". Nothing reaches BIR, so it is safe to retry. | Sends through the relay |
| `buwiz-relay`, `iaf-decrypt`, the relay tests | Not built | Built |

The `Dockerfile` builds with `--features bir`. CI builds, lints and tests
without it.

## Relay modes

These need a build with `--features bir`.

`BIR_RELAY_MODE` decides how far a submission goes:

- `dry_run` (default) encrypts and checks everything, sends nothing, and
  returns a receipt marked `dry_run`.
- `lab` uploads to a test SFTP server from `BIR_LAB_SFTP`.
- `live` files with BIR.

Lab test with a real OpenSSH server:

```sh
docker run -d -p 127.0.0.1:2222:22 atmoz/sftp lab:labpass:1001::1601Cv2018
ssh-keyscan -p 2222 -t ed25519 127.0.0.1 | ssh-keygen -lf -      # pin this
BIR_RELAY_MODE=lab BIR_LAB_SFTP=lab:labpass@127.0.0.1:2222 \
  BIR_SSH_HOST_KEYS=SHA256:… BIR_IAF_PASSPHRASE=lab cargo run --features bir
# Sign in, get a TIN approved, then act as the app:
cargo run --features bir --bin buwiz-relay -- --server http://localhost:3008 --token <access token> \
  --file return.xml --filename '12345678900000-1601Cv2018-062025#you@example.com#.xml' \
  --allow 127.0.0.1:2222
```

Tests: `cargo test` runs everything that needs no BIR code, including the
account site against Postgres (`tests/account_site.rs`: each test gets its own
database, so `DATABASE_URL` must be set, e.g. from `.env`).
`cargo test --features bir` adds the relay end to end in one process (fake
dispatcher, SSH server with a pinned key, the app's allow-list).
`scripts/v6.sh` runs `e2e/` (its own workspace): submission v2 with the
real buwiz-oss client, from a queued 1701Q to a verified receipt, and the
attacks it must refuse. It needs Docker, read access to buwiz-oss and the BIR files (e2e builds
the server with `bir`). With `--features bir` and `BIR_IAF_PASSPHRASE` set,
`tests/iaf_golden.rs` checks the encryption against official `Encrypt.exe`
output.
`cargo test --features bir -- --include-ignored` also runs HTTPS through the pipe against
example.com.

## Configuration

See [`.env.example`](.env.example). Required: `DATABASE_URL` and
`DOCUMENT_KEY`. Public deployments must use `https://` in `ACCOUNT_ORIGIN`,
which turns on secure cookies, and a `RESEND_API_KEY`. The server
refuses to start if it would print codes on a public deployment.

## Deploy

The `Dockerfile` builds a small image, with `--features bir`, that listens
on `$PORT` (8080 in the image). Any container host with Postgres works.

**Wasmer** is the long-term target, but it can't run this yet (tested
2026-10-29 with Wasmer 7.5 and the WASIX toolchain `v2026-09-18.1`, Rust 1.97):

- Topcoat 0.11 needs tokio ≥ 1.51. The WASIX registry's patched tokio stops at 1.47.
- Outbound TLS (rustls with ring) completes the handshake, then reads EOF.
  The same code works natively, and plain TCP works under WASIX.
- tokio-postgres doesn't compile for wasm without a patch.

axum, outbound TCP and an SSH handshake (russh, with a one-line `russh-util`
patch) do work under WASIX. Revisit when WASIX tokio catches up.

## Next

1. Port the full marketing home page.
2. The relay client in buwiz-oss (`bir-submit`), following
   [docs/relay-protocol.md](docs/relay-protocol.md).
3. Validate returns with buwiz-oss `bir-core` before sending.
