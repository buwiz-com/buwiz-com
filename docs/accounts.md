# Accounts, TINs and sign-in

Served on `account.buwiz.com` (the pages) and `api.buwiz.com` (what the apps
call).

## Accounts

You sign in with a 6-digit code sent by email. There are no passwords, no
organizations and no workspaces. One account can hold several TINs. Code:
`src/account/accounts.rs`, `src/account/pages.rs` (the Topcoat pages) and
`src/account/web.rs` (the form posts).

### Who can be mailed a code

Anyone can type any address into the sign-in form, so a script can use it
to mail strangers. Four checks come before a code is mailed
(`src/account/guard.rs`, `issue_code` in `src/account/accounts.rs`):

| Check | Limit | The person sees |
|---|---|---|
| Cloudflare Turnstile on the three forms that ask for a code (sign in, sign up, "send a new one") | The form post must carry a token Turnstile accepts. On when `TURNSTILE_SITE_KEY` and `TURNSTILE_SECRET_KEY` are both set | "We could not confirm that you are not a robot." |
| Per network (an IPv4 /24 or IPv6 /48, from `CF-Connecting-IP`) | 10 codes an hour. Kept in memory, so a restart clears it | "Too many codes were asked for from your network." |
| Per address | 5 codes an hour | "Too many codes were sent to this address." |
| All addresses without an account, together | 20 codes an hour and 60 a day | The code page, as if a code was mailed; none is |

An address that has an account is never held back by the last limit, so a
flood of made-up addresses cannot stop people from signing in. While that
limit is reached, a new address is answered exactly like one with an
account, so the page tells nothing about who has one; the server logs a
warning each time. A post that mails nothing (the last limit, or the
address's own) is not counted against its network.

The widget shows only when Turnstile wants a click. With Turnstile on,
signing in needs JavaScript. If Turnstile cannot be reached, no code is
mailed.

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
| Approve, reject, send back | ✓ (not one's own claim; approving a TIN that was ever paused is an Admin's) | ✓ (one's own too) |
| Confirm a TIN activated automatically | ✓ (not one's own, nor while it is paused) | ✓ (one's own too; lifting Buwiz's pause takes a reason) |
| Sign an account out everywhere, or out of one app (never an Admin) | ✓ | ✓ |
| Revoke an ownership, activate without papers, pause and resume (one's own TINs too) | – | ✓ |
| Delete a claim (one's own: as its owner, on My TINs), ban and lift a ban, the test label, change a role (never on one's own account) | – | ✓ |
| Make a login ("New user" on `/staff/users`) | – | ✓ |

Staff without a power are sent back with "Only an admin can do this";
everyone who is not staff gets a 404. Each action checks its actor's role
again inside its own transaction, so one whose page check passed just
before the actor was demoted or banned does nothing. Role changes refuse
one's own account, an `ADMIN_EMAILS` person, and a change that would leave
no Admin who has signed in; a change to the role the account has says
nothing changed. A role (Support or Admin) goes only to an account that
has signed in: sign-in is by a code mailed to the address, so until then
nobody has shown the address is theirs, and a mistyped one would hand staff
powers to whoever has it ("They must sign in once first, so the address is
proven"). What counts is a website session on record, ended ones too
(`web_sessions`, never deleted; every sign-in starts one). Taking a role
away is not held up by it. For the same reason an Admin who never signed
in, by stored role or `ADMIN_EMAILS`, does not count as the Admin who
stays when another is demoted. The seed tool still gives roles to logins
it makes, as before (it is an operator's run, with its dry run), and it
refuses a file that leaves only such Admins where one who had signed in
was left before. Every
staff action takes a reason (500 characters at most, like the review
notes), signing one app out included, but two: approving a claim and
confirming an automatic activation (unless the confirmation lifts the
pause Buwiz put on it, or the claim is the Admin's own: below). Rejecting a claim needs a note to the
person, and sending it back needs a fix reason or a note. Nobody bans
themselves, changes their own role or labels their own account a test
account; their own page says so instead of showing those forms.

"New user" makes a login the way the seed tool does: the account, the
test label if asked, and a `created` line (and a `test_marked` one) with
the reason. It gives no staff role: an Admin gives one on the person's
page after their first sign-in. It refuses an address that already has an
account, and the test label for an address in `ADMIN_EMAILS` (whose page
has no test-label controls, so the label could not come off), and it mails
nothing: the person signs in with a code sent to the address when they
ask, so it must be able to receive mail. The History of such a login
starts with who made it.

Staff open their own page under `/staff/users` and read it in full,
History and every reason included, and their own claims' pages too. An
Admin acts on their own TINs as on anyone's, with the usual reason: each
of those audit lines names the account itself as the actor, and the
History reads "Staff … paused their own TIN …". Approving their own claim
or confirming the automatic activation of their own TIN takes a written
reason too (`own_needs_reason`; for an approval it is the note), kept on
that audit line. A TIN that became active on its owner's own say (an
Admin activating it without papers, approving it or confirming its
automatic activation, on their own account: `decided_by` or
`auto_confirmed_by` is the account itself) is flagged "Activated by its
owner, an admin" in the queue and on the claim.

### Bans and test accounts

A **ban** (Admin, with a reason) ends the account's website sessions and
any sign-in not yet picked up. The person cannot sign in ("This account
can't be used on Buwiz…"). App sign-ins stay but every API call answers
`403 account_banned`; the token refresh and signing out still work, so the
app can say why. Lifting the ban restores everything. An Admin can't be
banned. The **test label** is for staff only: they see and filter it, and
sends work as usual.

A **test TIN** is a made-up one: its root is `000-000-000`, at any branch,
which is exactly what the TIN check refuses on every form (a TIN must have
9 digits, not all zeros, and a branch of 3 or 5 digits). An Admin adds one
on the staff page of an account with the test label ("Add a test TIN":
the TIN, its name and kind, a reason), active at once and without papers
(`verified_by = 'admin'`, an `activated` line with the reason). A real TIN
is refused there (it is added the usual way, and verified), and so is any
account without the label, and one in `ADMIN_EMAILS` (their page has no
test-label controls, so the label a test TIN holds in place could not come
off; the form is not shown there). An Admin whose own account another Admin
labelled adds one to it as to anyone's; its line names them as the actor
("Staff … activated their own TIN …"), and the claim is flagged
"Activated by its owner, an admin". It never sends, whatever `BIR_RELAY_MODE` says:
the send gate refuses it with `tin_test_only` (tickets, both submission
calls, the relay), and the Sender refuses one too. It shows as a test TIN
on My TINs, on the staff pages, in the apps' TIN list and in an AI
agent's list of TIN claims (`status: test_only`, paused or not, and
`test_only: true`); the plain licence list leaves it out. The
label cannot come off an account while it holds one that has not ended
(`test_tins_held`): revoke it first. Migration 13 keeps it so in the
database: such a row is only ever an Admin's or the seed tool's, active
or ended (`approved`, `removed`, `revoked`).

<a id="the-store-review-login"></a>

### Fixed sign-in codes for test accounts (store review)

Apple's and Microsoft's reviewers have to sign in to the apps, and they
cannot read a mailbox of ours. So an Admin can give a test account a
**fixed sign-in code**: six digits, set on the account's staff page. While
it is set, no code is mailed to that address; whoever asks for one types
the fixed code instead. Any test account can have one, not only the
reviewers' (`set_fixed_code` in `src/account/admin.rs`, `issue_code` in
`src/account/accounts.rs`).

**Who and where.** Only an Admin sets, changes or clears it, on the
account's page (`/staff/users/{id}`, Account tab, "Fixed sign-in code";
the users list's inspector links there). Support sees that one is set and
cannot change it. Each set, change and clear needs a reason and writes a
line in the account's history (`fixed_code_set`, `fixed_code_changed`,
`fixed_code_cleared`), which holds neither the code nor its hash.

**The conditions.** It is set only on an account with the test label, no
staff role, no ban, not in `ADMIN_EMAILS`, and not the Admin's own. It
stands in only while all of that still holds (`fixed_code_sql` in
`src/account/roles.rs`): a ban or a staff role given later makes the next
code random and mailed again, though the fixed code stays stored until an
Admin clears it.

**What it does to sign-in.** The sign-in form works as always: when the
address asks for a code, the fixed code's hash is stored as the new code
and nothing is mailed. The reviewer types it on the code page as anyone
types theirs. Every limit holds: Turnstile and the limit per network, five
codes an hour for the address, ten minutes for a code, five wrong guesses.
The fixed code signs nobody in unless a code was asked for through the
form first. Setting, changing or clearing it closes any code asked for
before.

**What it does to `GET /me`.** It answers `practice: true` exactly while
the account signs in with a fixed code, and `false` for every other
account. An app that reads it simulates every send, so a reviewer files
nothing by pressing a button. The test label alone does not do this: a
test account's sends work as usual.

**The server refuses its sends too.** An account with a fixed code stored
(set by an Admin, working or not) never sends, whatever its TINs:
tickets, both submission calls and the relay answer `account_practice`
([relay-protocol.md](relay-protocol.md#refusals)), and a return already
waiting is failed with those words. So nothing from a practice account
reaches the BIR even from an app that does not read `practice`. Its TIN
list is as anyone's. Clearing the code lets the account send again.

**Stored as a hash.** Only `code_hash(email, code)` is kept
(`accounts.fixed_code_hash`, the form `email_codes` uses), with who set it
and when. The code is never shown again, on any page, and never logged: a
code asked for logs the address only. Write it down where you keep the
stores' review notes before you press Set. The label cannot come off the
account while a code is set (`fixed_code_held`, and migration 15's check
in the database): clear the code first. The seed file never sets one.

**How to use it:**
1. Give the account the test label ("New user" with "Test account" on
   `/staff/users`, "Mark as test" on its page, or the seed tool with
   `test = true`). Give it no real TIN.
2. On its staff page, under "Fixed sign-in code", press Set, type a random
   six-digit code (not 123456) and a reason.
3. Give the address and the code to the stores. When the review is over,
   clear the code (or change it for the next review).

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
on the user's page under History, which staff read on their own page
too.

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

Staff review each claim by hand at `/staff/queue` (`src/account/staff.rs`). They can:
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
  by its owner, an admin" in the queue and on the claim, as is one they
  approved or confirmed on their own account (above);
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

The person approves on `/device` by typing the code the app shows. No link
fills it in: `verification_uri_complete` is the plain `/device` address, and
a `user_code` in the page's link is ignored. So a sign-in someone else
started cannot be approved with one click from a link they sent.

The clients and their scopes are in `CLIENTS` in `src/account/oauth.rs`:

| Client | Scope |
|---|---|
| `buwiz-desktop` | `app` |
| `buwiz-ebirforms` | `app` |
| `buwiz-books` | `books` |

## Licence gate

`GET /api/tax-profiles` lists the account's active TINs that can send (a
paused one and a test TIN are left out). The apps unlock their BIR tools when any profile
has permissions. With `?include=pending`, it also lists TINs that are still
being checked or were sent back. Each of those has a `status` and no
`filing.submit` permission.

With `?include=all` (also in a comma list) the answer is
`{"include":"all","profiles":[…]}`: every TIN on the account that has not
ended, with `status` (`approved`, `paused`, `test_only`, `unverified`,
`pending`, `needs_fix`), `can_send`, `message` and `action` (the id-based Verify or Fix
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

## Fonts and icons

The owner pages, the filing site and the Bot site load nothing from another
host. Their type and icons are files in `src/assets/fonts/` (each family
beside its licence), served under `/assets/fonts/` like the staff pages' fonts
and described by two stylesheets: `/assets/fonts.css` (Bricolage Grotesque,
Figtree, IBM Plex Mono, JetBrains Mono and the icon subset) and
`/assets/bot-fonts.css` (Manrope, Space Grotesk and the full icon font, as the
Bot site names its icons from compiled code). `tests/site_assets.rs` renders
the pages and fails on any address that is not this server's.

The icon subset holds exactly the Material Symbols named in
`src/assets/fonts/material-symbols-icons.txt`. A template that uses another
icon fails that test: add the name to the list and rebuild the font from the
full variable font (`wght,FILL` axes) with
`scripts/subset-icons.py FULL.woff2 src/assets/fonts/material-symbols-icons.txt
src/assets/fonts/material-symbols-outlined-subset.woff2` (needs fontTools and
brotli).
