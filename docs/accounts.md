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
banned. A banned account can't delete itself either: `/delete` sends it to
support (see "Deleting an account"). The **test label** is for staff only: they see and filter it, and
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
"Activated by its owner, an admin". Nothing for it ever reaches the BIR,
whatever `BIR_RELAY_MODE` says: the send gate treats it as any TIN (an
active one sends on tickets, both submission calls and the relay; a paused
one is refused `tin_paused`; a banned or practice account is refused
first, `account_banned` then `account_practice`), and the Sender makes
every send for it a dry run, deciding from the TIN in the ticket or the
return. The send ends `dry_run`, with a receipt saying `dry_run` signed by
the usual receipt key, and no uptime record
([relay-protocol.md](relay-protocol.md#test-tins)). That lets the owner
send from the real release apps against production, end to end, with
nothing filed. (Before this, the gate refused one with `tin_test_only`.)
It shows as a test TIN on My TINs, on the staff pages, in the apps' TIN
list and the licence list, and in an AI agent's list of TIN claims (each
with `test_only: true`; the TIN list's `message` says it sends as a dry
run). The
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

### Deleting an account

People delete their own account on `account.buwiz.com/delete`. The
eBIRForms app's Settings > Account > Delete account opens that page. It
needs a sign-in, and says what goes and what stays before anything
happens. Code: `src/account/deletion.rs`, the page in
`src/account/pages/delete.rs`.

**The code.** "Email me a code" mails a fresh six-digit code to the
account's address. Typing it on the page deletes the account. It is kept
in `email_codes` like a sign-in code, with `purpose = 'delete'`; a code
works only for its own purpose, so a sign-in code never deletes an account
and a deletion code never signs anyone in. Only the newest deletion code
works, so one asked for earlier does not. The limits of sign-in codes
hold: five codes an hour for the address (sign-in and deletion codes
together), ten minutes, five wrong tries. A test account with a fixed
sign-in code still gets a random code by mail, never its fixed code:
whoever deletes it must read its mailbox. The form has no Turnstile check
or limit per network: it mails only the signed-in account's own address.
The page asks once more, in the
site's own dialog, before the account is deleted.

**What is deleted,** in one transaction: the account, and with it (`ON
DELETE CASCADE`) its browser sessions, devices and app and agent sign-ins
with their tokens, device and agent sign-in requests, every TIN with its
files and their readings, its returns with their receipts, its tickets,
its BIR queue reports and turns, and its held requests and proofs of
outage. Also its codes in `email_codes` (no foreign key) and the recent
signatures of its devices (`request_nonces`). After the commit, the stored
objects of files that were still kept (claims still open) are deleted, as
when a claim is deleted: a failure is logged, and the bytes are ciphertext
whose key stays with us. The browser is signed out. An app signed in to
the account hears `401 invalid_token` on its calls and `invalid_grant` on
its refresh, as for any ended sign-in.

**What stays:**
- Audit lines. `claim_audit` names a TIN only masked and `account_audit`
  names the account by its id. A `deleted` line is added (actor: the
  account itself, no address). Lines where the person was the actor on
  their own account lose the address (`actor_email` becomes empty); staff
  lines keep the staff member's. A seed run's summary keeps the address of
  a login it made.
- BIR uptime tries (`bir_tries`), 90 days. They hold only keyed daily
  hashes, not the account.
- Server logs, which carry ids only.
- The database provider's backups, for their window.

Returns the BIR already received are not withdrawn. Nothing stored in the
app on the person's computer is touched.

**Refused,** with the reason on the page, both when the code is asked for
and when it is typed:
- A stored staff role: an Admin removes it first. An address in
  `ADMIN_EMAILS` is refused too, until it leaves that setting.
- An account named in staff work elsewhere: the staff columns of other
  accounts' TINs (`decided_by`, `sent_back_by`, `removed_by`, `paused_by`,
  `auto_confirmed_by`) and accounts (`banned_by`, `fixed_code_set_by`),
  audit lines about other accounts, BIR notes, events and host keys, and
  seed runs. The person writes to support@buwiz.com. Those rows are never
  changed to let the deletion through: several checks give NULL a meaning
  (`paused_by` NULL is a pause by Buwiz), and the audit must keep who did
  it.
- A return being sent right now (`submissions.status = 'sending'`): try
  again in a few minutes. The deletion locks the account and its returns,
  so none starts sending meanwhile.
- A banned account: write to support@buwiz.com. The ban is on the
  account's row, so deleting it here would let the address sign up again,
  unbanned; support decides what to keep. Its sign-in with `next=/delete`
  gets a session that only `/delete` takes (every other page still treats
  it as signed out), so the page can say this.

**Sign-in records** are deleted by the hourly upkeep
(`accounts::upkeep`): codes 30 days after they were made, and device and
agent sign-in requests 30 days after they expired. The limits per address
look back an hour and a day, so nothing needs them by then.

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
an old one ends the whole sign-in. The eBIRForms apps can also sign in by
email inside the app ([below](#in-app-sign-in)).

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

## In-app sign-in

The eBIRForms apps can also sign in without a browser: the person types
their email in the app, gets a code by email, and types it in the app.
Code: `src/account/app_sign_in.rs`. Each request gets its own code, a row
of `email_codes` with the purpose `app_sign_in` (`issue_code_as`,
`try_code_row` in `src/account/accounts.rs`), and that code works only
with its request: verify checks that one row (its expiry, its five tries,
its hash). The same six digits typed into another device's request are a
wrong try on that request's code; a web code typed in the app is wrong
there, and an app code typed on the web's form is wrong there. The limits
of [Who can be mailed a code](#who-can-be-mailed-a-code) count both
together: five codes an hour per address, per network, and the totals
for addresses without an account. Test accounts with a fixed code get a
copy of its hash on each request, and nothing is mailed. A banned account
gets no sign-in. The browser device flow above stays: it is what an app
falls back to where its in-app check cannot run.

**One wait for everything.** An in-app code works for
`APP_SIGN_IN_MINUTES` (5 when unset, 1 to 30;
[deploy.md](deploy.md#migration-17-in-app-sign-in)), and that is also how
long before a new one: `expires_in` and `resend_after` are the same
number. The web's codes keep their ten minutes. Within it:
- a resend of the request is refused (`429 too_many_requests` with
  `retry_after`, the seconds left on its code). Once the code has expired,
  a resend mails a new code for the same request and restarts its time,
  for up to an hour after the start; after that the app starts again;
- a start for the same address from the same install (`install_id`) while
  its code still works mails nothing and makes no request: `429
  code_already_sent`, with `retry_after`. A start from another install is
  a new request with its own mail, held only by the limits above; both
  codes then work, each with its own request. A start without
  `install_id` is never held this way. A test account with a fixed code is
  never held: its earlier request from that install ends;
- a start for another address (the app's "Change email") is a new mail,
  and the old request is left to expire.

**The request belongs to the device that asked.** Before it starts, the
app makes a secret (32 random bytes, base64url without padding) and sends
only `secret_hash`, the base64url SHA-256 of that secret string (as PKCE's
S256). Resending and verifying need the secret itself, compared in
constant time. So a code read from the mail is useless on any other
device: whoever typed someone's address into their own app holds that
app's secret, not the victim's, and the victim's code works only in the
app that asked.

**Nothing tells whether an address has an account.** Start answers the
same whether or not it has one, whether a code was mailed or not (the
new-address limit mails nothing, as on the web), and for a test account
with a fixed code (nothing is mailed; the fixed code works). There is no
"unknown email" error; an address without an account gets one when its
code is verified. `intent` (`sign_in` or `create`) only picks the mail's
words.

**The mail.** Its own message, not the web's (which is unchanged): the
subject is "Your Buwiz sign-in code for eBIRForms on iPhone", and the text
says "Someone asked to sign in to eBIRForms on iPhone with this address.
If it wasn't you, ignore this mail.", "Never give this code to anyone"
and how long it works, in words ("five minutes"), so the code stays its
only number. The device's name comes from fixed words only, from
`platform`:

| `platform` | Name in the mail |
|---|---|
| `ios` | eBIRForms on iPhone |
| `ipados` | eBIRForms on iPad |
| `android` | eBIRForms on Android |
| `macos` | eBIRForms on Mac |
| `windows` | eBIRForms on Windows |
| `linux` | eBIRForms on Linux |
| missing or anything else | eBIRForms |

An unknown `platform` is stored as nothing and is never an error; no text
from the request goes into a mail.

**The app check and the testing cap.** A request may carry `turnstile`, a
Cloudflare Turnstile token, checked exactly as the web form's
(`src/account/guard.rs`). A token that fails is `403 turnstile_failed`.
While the token is optional, requests without one (start and resend
alike) are held to a testing cap of 20 an hour and 50 a day across every
address, counted in the database (`app_sign_in_unchecked`, so the counts
survive a restart and are shared by every server). Past it: `429
too_many_requests` with `retry_after`. When a day's count reaches 25,
Admins and Support are mailed once that (Manila) day, as BIR uptime's
alerts are (`Mailer::send_notice` to `roles::staff_emails`). A request with
a good token skips the cap, never the shared limits. Without Turnstile keys
(local development) the token is ignored and the cap applies.

The cap is for testing only. **A platform's 1.0.0 needs the token
required**: from then on, a request from that platform without a token
will be refused with `403 turnstile_required` (the code is reserved,
`TURNSTILE_REQUIRED` in `src/account/app_sign_in.rs`, not used yet).
Where the app's check cannot run, it falls back to the browser device
flow ([below](#the-app-check-page)).

### The app-check page

An app gets its Turnstile token from `https://account.buwiz.com/app-check`
(`src/account/pages/app_check.rs`, `src/account/app-check.js`): no
sign-in, noindex, and no site header, so nothing on it leads off the
account host (Cloudflare's widget frame inside it is the only other
origin). The app opens it in a web view; the widget runs with the site's
`TURNSTILE_SITE_KEY`, as on the sign-in forms. The app then sends the
token as `turnstile` in start or resend, where it is checked exactly as
the web form's token is (Turnstile itself lets a token be used once).

When the widget passes, the page hands over the token in this order:
1. `window.nsh.postMessage(JSON.stringify({turnstile: token}))`, when the
   host injected `window.nsh` (the primary way);
2. `postMessage({type: "buwiz-turnstile", token})` to `window.parent` and
   `window.opener`, when there are any, and the document title
   `buwiz-turnstile:<token>`;
3. then it goes to `/app-check/done#token=<token>`, a page that says "You
   can close this window" and reads nothing on the server: a host that saw
   none of the above reads the token from that address's fragment.

When the widget fails, errors, expires or times out, the same ways carry
`{turnstile_error: "<code>"}` (title `buwiz-turnstile-error:<code>`,
`/app-check/done#error=<code>`; parent and opener get
`{type: "buwiz-turnstile", error}`). Codes: `expired`, `timeout`,
`error_<Turnstile's code>`, and `not_configured` when the server has no
Turnstile keys (local development; the page says so and sends it at once).

Where the web view cannot run (Linux under Wayland, a view that fails to
load) the app falls back to the browser device flow, which stays.

### The link in the mail

The mail also carries a one-time link,
`https://account.buwiz.com/app-sign-in#r=<request_id>&t=<link_token>`, for
a person who reads it on the device that asked. The values are in the
fragment, never the query, so they reach no server log. The page
(`src/account/pages/app_sign_in.rs`, on the account host, no sign-in,
noindex) only shows an "Open eBIRForms" button for
`ebirforms://signin?r=…&t=…`: it opens nothing on load, approves nothing
and calls no endpoint. The app then verifies with the token instead of the
code.

Why it is safe:
- **The click approves nothing.** Verifying needs the secret of the app
  that asked as well as the token. Someone who types a victim's address
  into their own app and gets the victim to click the link signs nobody
  in: their app has the token's request but not the victim's mail, and
  the victim's device has the mail but not the attacker's secret. The page
  says to open it on the device that asked, or to type the code there.
- **A hijacked scheme gets nothing usable.** Another app that registers
  `ebirforms://` receives a token it cannot spend without the secret.
- **A leaked token is worth no more than the code.** Only its hash is
  stored, on its request, and it spends the request's own code, so it
  works only while that code does: a resend replaces it, using the code
  or the link spends both, and the code's five tries or its expiry end
  it too. It dies with its request.

**Records.** `app_sign_ins` holds each request (address, client, the
secret's hash, intent, platform, install id, its code, expiry, when it
was used). The hourly upkeep deletes a request 30 days after it
expired, the cap's counts after two days and its alert days after 30.
Deleting an account deletes its requests, by address, with its codes.

### The API

On the API host (`api.buwiz.com`), like `/oauth/token` and `/me`. Each
takes and gives JSON (`Content-Type: application/json`: the web's form
posts to the same two paths are told apart by it) and answers
`Cache-Control: no-store`. Every refusal is
`{"error": "<code>", "message": "<sentence>"}`, with `retry_after`
(seconds, also as `Retry-After`) or `attempts_left` where they apply.

`POST /auth/email/start`

```json
{"client_id": "buwiz-ebirforms", "email": "ana@example.ph", "intent": "sign_in",
 "secret_hash": "<base64url SHA-256 of the secret>", "platform": "ios",
 "install_id": "<16 to 64 of A-Z a-z 0-9 - _>",
 "turnstile": "<optional token>", "attestation": "<optional, reserved, not read>"}
```

`client_id` must be a client with scope `app`; `intent`, `platform` and
`install_id` are optional. `install_id` is the app's own id for its
install, an identifier, not a secret. Answers `200` (with the default
five minutes):

```json
{"request_id": "<uuid>", "expires_in": 300, "resend_after": 300, "code_length": 6}
```

`POST /auth/email/resend` with `{"request_id", "secret", "turnstile"?,
"attestation"?}`: only once the request's code has expired (else `429
too_many_requests` with `retry_after`), and up to an hour after the start
(else `invalid_request`). Mails a new code for the request, which ends
its old one, and restarts its time. The same checks, limits and answer
as start; the request keeps its intent and platform.

`POST /auth/email/verify` with `{"request_id", "secret", "code"}`, or
`{"request_id", "secret", "link_token"}` with the token from the mail's
link ([above](#the-link-in-the-mail)). Checked in this order: the request
exists and is not used, the secret is its own, it has not expired, then
the code (one try of its five) or the link's token, then a ban.
On success, in one transaction, the code is used up, the account made if
it is new, the request spent, and a grant made for the request's client
with scope `app`. Answers `200` with exactly the device flow's token
answer:

```json
{"access_token": "bwa_…", "token_type": "Bearer", "expires_in": 900,
 "refresh_token": "bwr_…", "scope": "app"}
```

The tokens refresh on `/oauth/token` and work on `/me` as the device
flow's do.

| Status | `error` | When |
|---|---|---|
| 400 | `invalid_request` | The body is not the JSON expected, `secret_hash` is not a SHA-256, or the request is unknown, used, or not this secret's |
| 400 | `unknown_client` | `client_id` is not a client with scope `app` |
| 400 | `invalid_email` | Not an email address (as the web form checks it) |
| 400 | `bad_install_id` | `install_id` is not 16 to 64 of `A-Z a-z 0-9 - _` |
| 400 | `code_expired` | The request's code has expired: resend |
| 400 | `invalid_code` | A wrong code; `attempts_left` says how many tries are left |
| 400 | `too_many_attempts` | The code's five tries are used up: ask for a new one |
| 400 | `link_used` | The link's token is wrong, used, or ended by a newer code |
| 403 | `account_banned` | The account is banned (the message is `/me`'s for a banned account) |
| 403 | `turnstile_failed` | A `turnstile` token that Turnstile refused |
| 403 | `turnstile_required` | Reserved: not used yet (see the 1.0.0 rule above) |
| 429 | `code_already_sent` | A start from the same install for an address whose code from it still works; `retry_after` |
| 429 | `too_many_requests` | A resend before the request's code expired, the testing cap, or the web's limits (per address, per network); `retry_after` |
| 502 | `mail_failed` | The mail provider failed; the request cannot be used (start again) |
| 503 | `restarting` | The server is stopping (deploy.md, "What a deploy does at stop"); `Retry-After: 30` |

## Devices

An app that signs in can register a device key
([relay-protocol.md](relay-protocol.md#51-device-key)); the account's
devices are under **My devices** on My TINs. An app sends `install_id`,
its own id for its install (an identifier, not a secret), when it signs
in (`/auth/email/start`, `/oauth/device/code`) and when it registers. The
sign-in's grant keeps it, and an install has one device row per account
(a unique index): signing in again on the same install and registering
updates that row (its key, its sign-in, what the app says about the
device, `signed_in_at`) and ends the sign-in it had, so a reinstalled or
signed-out-and-in app does not pile up rows. A registration whose install
is not the sign-in's is refused (`install_mismatch`).

What the app says about the device: its platform, model (an Apple code
such as `iPhone16,1` is shown as "iPhone 15 Pro", from the table in
`src/filing/apple_models.rs`; others as sent), system version and, on a
computer, its name. The list shows those, when it was added, last signed
in and last used, and keeps a removed device for 30 days.

The owner's app asks "Add this iPhone to your Buwiz account?" before it
registers, so an app can stay signed in without a device. The list also
shows each such sign-in ("Signed in · not added as a device yet", with
the app, the platform the sign-in named, and when) with **Log out**
(`POST /apps/{grant_id}/sign-out`, the person's own only), the same list
`GET /api/devices` gives as `pending`.

Staff see the devices on the account's page (Sign-ins tab) with their
state, and log a live one out with a reason, under the rule for signing
one app out (Support may not sign out an Admin): the device is removed,
its sign-in ended (`signed_out_by_staff`), and the history gets an
`app_signed_out` line naming the app.

## Licence gate

`GET /api/tax-profiles` lists the account's active TINs that can send (a
paused one is left out; a test TIN is listed with `test_only: true`, and
its sends are dry runs). The apps unlock their BIR tools when any profile
has permissions. With `?include=pending`, it also lists TINs that are still
being checked or were sent back. Each of those has a `status` and no
`filing.submit` permission.

With `?include=all` (also in a comma list) the answer is
`{"include":"all","profiles":[…]}`: every TIN on the account that has not
ended, with `status` (`approved`, `paused`, `unverified`, `pending`,
`needs_fix`; older servers also sent `test_only` for a test TIN), `can_send`, `message` and `action` (the id-based Verify or Fix
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
