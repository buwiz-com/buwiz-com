# Accounts, TINs and sign-in

Served on `account.buwiz.com` (the pages) and `api.buwiz.com` (what the apps
call).

## Accounts

You sign in with a 6-digit code sent by email. There are no passwords, no
organizations and no workspaces. One account can hold several TINs. Code:
`src/account/accounts.rs`, `src/account/pages.rs` (the Topcoat pages) and
`src/account/web.rs` (the form posts).

### Staff roles

A staff role is kept on the account (`accounts.role`,
`src/account/roles.rs`): **Support** or **Admin**. `ADMIN_EMAILS` is a
break-glass list: anyone in it is always an Admin, whatever the account
says, and the staff pages cannot change their role or ban them. The setting
never writes a role. A banned account has no role. Separate its addresses
with commas; at start-up the server logs each entry that is not a plain
address (spaces, semicolons, quotes) and starts anyway. Only `roles.rs`
reads the list.

| | Support | Admin |
|---|---|---|
| See the queue, claims, their files, and every account | ✓ | ✓ |
| Approve, reject, send back (never one's own claim) | ✓ (approving a TIN that was ever paused is an Admin's) | ✓ |
| Confirm a TIN activated automatically (never one's own) | ✓ (not while it is paused) | ✓ (lifting Buwiz's pause takes a reason) |
| Sign an account out everywhere, or out of one app (never an Admin) | ✓ | ✓ |
| Delete a claim, revoke an ownership, activate without papers (one's own too), pause and resume, ban and lift a ban, the test label, change a role | – | ✓ |

Staff without a power are sent back with "Only an admin can do this";
everyone who is not staff gets a 404. Each action checks its actor's role
again inside its own transaction, so one whose page check passed just
before the actor was demoted or banned does nothing. Role changes refuse
one's own account, an `ADMIN_EMAILS` person, and a change that would leave
no Admin; a change to the role the account has says nothing changed. Every
staff action takes a reason (500 characters at most, like the review
notes), signing one app out included, but two: approving a claim and
confirming an automatic activation (unless the confirmation lifts the
pause Buwiz put on it). Rejecting a claim needs a note to the
person, and sending it back needs a fix reason or a note. Nobody pauses or
resumes their own TIN.

### Bans and test accounts

A **ban** (Admin, with a reason) ends the account's website sessions and
any sign-in not yet picked up. The person cannot sign in ("This account
can't be used on Buwiz…"). App sign-ins stay but every API call answers
`403 account_banned`; the token refresh and signing out still work, so the
app can say why. Lifting the ban restores everything. An Admin can't be
banned. The **test label** is for staff only: they see and filter it, and
sends work as usual.

A ban that commits while a request for the account is under way wins:
activation, approval, an app's sign-in approval and an agent's consent
lock the account and look again as they write, and no new sign-in (device
code or agent code) gets a token for a banned account. The token refresh
of an app already signed in still issues tokens, on purpose: the app keeps
a working sign-in, and every call it makes with them answers
`account_banned`.

Every staff action on an account goes to `account_audit` with who, when and
why; actions on claims go to `claim_audit`, review decisions (approved,
rejected, sent back) included, so who decided outlives the claim. Both are
on the user's page under History, which staff never see for their own
account (`/staff/users/{self}` sends them back to the list: another staff
member looks), so nobody reads the reasons written about them.

## TIN activation

The person says whose TIN it is, and uploads its papers
(`src/account/claims.rs`), or presses **Skip for now**:

- **Personal:** a TIN ID, either the card or the ORUS digital TIN ID. A selfie
  and a photo of the National ID are optional, and they make the check
  stronger.
- **Business:** BIR Form 2303 and a government ID. When the 2303 is in a
  company's name, add proof of authority: a secretary's certificate, a board
  resolution or an SPA.

**Not verified.** Skip saves the TIN without papers (`unverified`); any
files chosen are not even read. It can prepare and save returns in the
apps but cannot send, test-send or be shared. Its card on My TINs has
"Verify to send", which opens `/tins/claims/{id}/verify`: sending the papers
there makes the same claim wait for review, so the app's profile keeps its
id. Any number of accounts may save the same TIN this way, and saving looks
exactly the same whether or not someone else has it. At most 20 TINs per
account wait to be verified. `/tins/new?tin=<9 digits>&branch=<5>` (the
apps' "Add it" link) fills in the TIN, or goes to that TIN's own page when
the person already has it.

### Staff review

Staff review each claim by hand at `/staff` (`src/account/staff.rs`). They can:
- approve it;
- reject it;
- send it back with reasons. The person fixes it from My TINs (`/tins`) and resends it.

An approved claim makes the TIN active. A TIN has at most one owner. People
can prepare returns right away, but sending them waits for approval. A
TIN that was ever paused, on any account, is approved only by an Admin:
Support reads "An admin must decide this one" and the claim stays waiting.

The queue's tabs: Waiting, Sent back, Not verified, **Activated
automatically** (with a count), Approved, Paused, Rejected, Removed.

An Admin can also:
- **activate without papers**, with a reason, a TIN not verified, waiting
  or sent back, their own too (refused while another account owns it, or
  for a banned account). The reason stays in the audit only. Their own
  reads as "Staff … activated their own TIN …" and is flagged "Activated
  by its owner, an admin" in the queue and on the claim;
- **pause** an active TIN, with a reason the owner reads: it stays owned
  but nothing sends (`tin_paused`) until resumed. The owner cannot remove a
  paused TIN. A "Paused before" card shows every pause of the TIN, on any
  account, to whoever reviews it next.

### Automatic activation (`AUTO_VERIFY`)

**Off unless `AUTO_VERIFY=on`** (`src/account/verify.rs`). While off, no
claim is checked or activated. The hourly upkeep still runs: it pauses an
automatic activation made while the setting was on and still unconfirmed
after 30 days (with none, it changes nothing), and the staff pages count
the unconfirmed ones for their tab. When on, after OCR reads a claim's TIN ID,
the claim becomes active by itself only if all of these hold: a personal
head-office TIN waiting for its first review (never sent back, on this
claim or an earlier one); exactly one TIN ID, a JPEG, PNG or WebP photo
read by OCR (never a PDF, nor HEIC); TIN and name a full match with what is
typed, each at least 90% sure, and a name of two words or more; the
account not banned; no other account ever had a row for the TIN's root, at
any branch; this account never had it rejected, revoked or deleted, or
removed it after an automatic activation; sending for the TIN was never
paused, on any account; the TIN never had a business claim, on any
account; no other automatic activation of the account waiting for a
person; fewer than 20 automatic activations in the last 24 hours. The
limits are counted under one lock.

A deleted claim leaves only its audit line, which keeps the TIN masked
(last three digits and branch) and, from this release, the claim's kind.
The rule reads those lines too, matched on the masked TIN: another TIN
with the same last three digits in someone's history leaves a claim for
staff that might have passed. The more TINs Buwiz has seen, the more often
that happens (with about 700 in other accounts' histories, about half of
new claims).

"Personal" is what the claimant picks. A person can pick it for a
business's TIN that nobody ever claimed as a business on Buwiz; then
only the name check stands between them and that TIN.

- When it activates: `verified_by = 'auto'`, what it rests on in
  `auto_check`, and an audit line from source `auto`. **The papers are
  kept** until a person confirms.
- Otherwise the claim waits for staff as usual, and its page lists, in
  plain words, every reason it was not activated.
- The "Activated automatically" tab lists every automatic activation no
  person has confirmed, whatever happened since (removed by its owner or
  revoked included, with their papers). Support or an Admin checks the
  papers and presses **Looks right: confirm**: the papers are deleted.
- One still unconfirmed after 30 days is **paused by Buwiz** (no staff
  name, a fixed reason, an audit line), papers kept. Only an Admin
  confirms a paused one: confirming lifts Buwiz's pause and takes a
  reason (it goes on the "resumed" line); a staff pause stays. Support
  confirms only one that is not paused.

What it proves is weak: the person types the TIN and name and uploads the
picture, and a card drawn in a few lines of code passes (`tests/ocr.rs`).
See [deploy.md](deploy.md#automatic-activation) before turning it on.

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

For a TIN activated automatically, the files stay until staff confirm it
(above). The file's hash stays.

### OCR

With the `ocr` feature, the server reads the TIN ID or the 2303 when a claim
is sent or resent (`src/ocr/`). It uses PaddleOCR PP-OCRv5 mobile on ONNX
Runtime. For a digital PDF, it reads the text layer first.

- Staff see what was read beside what was typed: the TIN, name, birth date
  or RDO code, with a mark per row.
- The queue flags "TIN matches the ID" or "Name differs".
- Only those fields are kept, and they are deleted with the file.
- Nothing is sent to anyone else. Staff decide, unless automatic
  activation is on (above).

## App sign-in

The desktop app and the mini apps use the OAuth device flow (RFC 8628):
`/oauth/device/code` and `/oauth/token`. Refresh tokens rotate, and reusing
an old one ends the whole sign-in.

The clients and their scopes are in `CLIENTS` in `src/account/oauth.rs`:

| Client | Scope |
|---|---|
| `buwiz-desktop` | `app` |
| `buwiz-ebirforms` | `app` |
| `buwiz-books` | `books` |

## Licence gate

`GET /api/tax-profiles` lists the account's active TINs that can send (a
paused one is left out). The apps unlock their BIR tools when any profile
has permissions. With `?include=pending`, it also lists TINs that are still
being checked or were sent back. Each of those has a `status` and no
`filing.submit` permission.

With `?include=all` (also in a comma list) the answer is
`{"include":"all","profiles":[…]}`: every TIN on the account that has not
ended, with `status` (`approved`, `paused`, `unverified`, `pending`,
`needs_fix`), `can_send`, `message` and `action` (the id-based Verify or Fix
link). `approved` means it can send. Sent-back rows add `reasons`,
`reason_codes` and the staff `note`. Only the BIR app's token gets the full
TIN, filing permissions, links and the note. The golden example is
`tests/fixtures/tax_profiles_all.json`; the refusal codes the same rule gives
are in [relay-protocol.md](relay-protocol.md#refusals).

## MCP for AI agents

`POST /mcp` has four read-only tools (`src/account/mcp.rs`):
- active TINs;
- TIN claims (with `sent_at`, when the papers were sent: null for a TIN
  saved or activated without them; the staff note only for a rejection);
- sent returns;
- one return.

Agents and Buwiz Books see TINs masked (`***-***-789-00000`), on `/mcp`
and on `/api/submissions`. A return's file name comes with the TIN masked
and without the email, and without its remote path or signed receipt, which
carry the full TIN (`SubmissionView::for_bearer`). Only the BIR apps (`app`)
see full TINs.

How an agent signs in (`src/account/agents.rs`):
1. It finds the sign-in through the 401's `WWW-Authenticate` metadata (RFC 9728 and RFC 8414).
2. It registers itself (RFC 7591).
3. It signs in with an authorization code and PKCE, after the person approves it on a consent page.

Agent tokens carry the `mcp` scope and can never file a return. `/mcp`
accepts only those: an app's token (`app`, `books`) gets 403
`insufficient_scope`. So every agent that reads an account shows on the
account page, where the person can remove it.
