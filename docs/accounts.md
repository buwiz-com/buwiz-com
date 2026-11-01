# Accounts, TINs and sign-in

Served on `account.buwiz.com` (the pages) and `api.buwiz.com` (what the apps
call).

## Accounts

You sign in with a 6-digit code sent by email. There are no passwords, no
organizations and no workspaces. One account can hold several TINs. People
in `ADMIN_EMAILS` see the staff pages. Code: `src/accounts.rs`,
`src/pages.rs` (the Topcoat pages) and `src/web.rs` (the form posts).

## TIN activation

The person says whose TIN it is, and uploads its papers (`src/claims.rs`):

- **Personal:** a TIN ID, either the card or the ORUS digital TIN ID. A selfie
  and a photo of the National ID are optional, and they make the check
  stronger.
- **Business:** BIR Form 2303 and a government ID. When the 2303 is in a
  company's name, add proof of authority: a secretary's certificate, a board
  resolution or an SPA.

### Staff review

Staff review each claim by hand at `/staff` (`src/staff.rs`). They can:
- approve it;
- reject it;
- send it back with reasons. The person fixes it from My TINs (`/tins`) and resends it.

An approved claim makes the TIN active. A TIN has at most one owner. People
can prepare returns right away, but sending them waits for approval.

### Changing and removing claims

- **While a claim waits,** the person can replace any file, or remove an
  optional one. Staff see that files changed after the claim was sent.
- **A claim that never became active** can be deleted, with its files. Its
  owner can do this, or staff with a reason.
- **An active TIN** can be removed by its owner or revoked by staff. It stops
  being active, its sent returns stay, and it can be claimed again.

Each of these actions writes a line, with the TIN masked, to the account's
history on the staff pages.

### Stored files

Files are encrypted with AES-256-GCM under `DOCUMENT_KEY` before they are
stored. In production they go to an S3-compatible store (Cloudflare R2, via
`S3_*`); in development, to Postgres. A file is deleted when:
- the claim is decided;
- the claim is withdrawn;
- or the file is replaced.

The file's hash stays.

### OCR

With the `ocr` feature, the server reads the TIN ID or the 2303 when a claim
is sent or resent (`src/ocr/`). It uses PaddleOCR PP-OCRv5 mobile on ONNX
Runtime. For a digital PDF, it reads the text layer first.

- Staff see what was read beside what was typed: the TIN, name, birth date
  or RDO code, with a mark per row.
- The queue flags "TIN matches the ID" or "Name differs".
- Only those fields are kept, and they are deleted with the file.
- Nothing is sent to anyone else, and staff still decide.

## App sign-in

The desktop app and the mini apps use the OAuth device flow (RFC 8628):
`/oauth/device/code` and `/oauth/token`. Refresh tokens rotate, and reusing
an old one ends the whole sign-in.

The clients and their scopes are in `CLIENTS` in `src/oauth.rs`:

| Client | Scope |
|---|---|
| `buwiz-desktop` | `app` |
| `buwiz-ebirforms` | `app` |
| `buwiz-books` | `books` |

## Licence gate

`GET /api/tax-profiles` lists the account's active TINs. The apps unlock
their BIR tools when any profile has permissions. With `?include=pending`,
it also lists TINs that are still being checked or were sent back. Each of
those has a `status` and no `filing.submit` permission.

## MCP for AI agents

`POST /mcp` has four read-only tools (`src/mcp.rs`):
- active TINs;
- TIN claims;
- sent returns;
- one return with its receipt.

How an agent signs in (`src/agents.rs`):
1. It finds the sign-in through the 401's `WWW-Authenticate` metadata (RFC 9728 and RFC 8414).
2. It registers itself (RFC 7591).
3. It signs in with an authorization code and PKCE, after the person approves it on a consent page.

Agent tokens carry the `mcp` scope and can never file a return.
