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
address's own) is not counted against its network. The per-network count
here is the web form's only: the apps' requests have their own
([In-app sign-in](#in-app-sign-in)) and never touch it.

Turnstile's answer must name the form's widget (action `sign-in`) and the
account site's host. One that names another is refused. One that lacks
either field (only Cloudflare changing its answer can do that) is let
through on the web form, as every answer was before these were checked,
with a warning in the log and a mail to Admins and Support at most once an
hour ("Sign-in: Turnstile's answers lack the hostname or action"), so web
sign-in never stops for it. The apps' check refuses such an answer.

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

A **test TIN** is one whose root is a **reserved root**, at any branch.
The reserved roots are a short list in code, `TEST_ROOTS` in
`src/account/tin.rs`: `000-000-000` and `111-111-114`. The eBIRForms app
refuses all zeros everywhere, so no release build of it can queue a return
for one; `111-111-114` passes the app's check digit like any TIN, so the
app takes it as an ordinary taxpayer. Every form people use refuses every
reserved root: all zeros with "That TIN is not valid." (`tin_zero`), as it
always did, any other with "This TIN is reserved for testing. If it is
yours, write to support@buwiz.com." (`tin_reserved`). That covers adding a
TIN (with papers or without, and the apps' "Add it" link, which never
fills one in); the fix, papers and automatic-check paths act on a row that
already exists, which the database check below keeps from ever holding a
reserved root. An Admin adds one
on the staff page of an account with the test label ("Add a test TIN":
the TIN, its name and kind, a reason; any reserved root at any branch),
active at once and without papers (`verified_by = 'admin'`, an `activated`
line with the reason). A TIN that is not a reserved root is refused there
(`not_test_tin`) (it is added the usual way, and verified), and so is any
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
(`test_tins_held`): revoke it first. Migrations 13 and 23 keep it so in
the database (`tin_claims_test_tin_check`, over the same list): a row with
a reserved root is only ever an Admin's or the seed tool's, active or
ended (`approved`, `removed`, `revoked`).

**Why a list in code, not a flag.** A flag on a claim or an account is a
value someone sets, and a wrong one would turn a real filing into a dry
run that reports success and files nothing: the failure would be silent.
A fixed list of roots cannot be set on a real TIN by mistake. The only
way it can go wrong is the other way round: a person whose real TIN has a
reserved root cannot register it. That failure is loud (the form says so
and sends them to support), never "filed as a dry run".

**The honest limit.** A reserved root could be a real taxpayer's: the BIR
issues TINs, not Buwiz, and `111-111-114` is a valid one by the check
digit. That person cannot add it on Buwiz, and is sent to
support@buwiz.com. There is no override; support can only tell them why.

**Adding a root.** It must be one the app accepts (its check digit), and
one no account holds: check first with
`SELECT id, account_id, status, verified_by FROM tin_claims WHERE tin_root = '<root>'`
(expected no rows). Then, in one pull request: add it to `TEST_ROOTS`,
and add a new migration that, like 0023, refuses any row with the new
root and puts `tin_claims_test_tin_check` back over the new list (once
0023 has run anywhere, its file cannot change: sqlx refuses a migration
that changed after it ran). The unit test
`the_migrations_list_is_test_roots` in `src/account/tin.rs` reads the
newest such migration's lists; point it at the new file. Every other
place reads `TEST_ROOTS` or `Tin::is_test`.

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
account not banned; no other account ever sent papers for the TIN's root
or had it active, at any branch; this account never had it rejected,
revoked or deleted, or removed it after an automatic activation; sending
for the TIN was never paused, on any account; the TIN never had a
business claim (on this account, or on another that sent its papers or
had it active); no other automatic activation of the account waiting for a
person; fewer than 20 automatic activations in the last 24 hours. The
limits are counted under one lock.

Another account's claim saved without papers, live or deleted, does not
count: anyone can save any TIN that way, on the site or from the app, so
it is no evidence of anyone, and counting it would let anyone keep a
stranger's TIN from ever activating by itself. The price: a real owner who
only saved their TIN without papers does not hold off a claim with a
drawn card.

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
email inside the app ([below](#in-app-sign-in)), or in the browser with
the authorization code flow ([Sign in with Buwiz](#sign-in-with-buwiz)).
The device flow stays for the apps already in the field.

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
together: five codes an hour per address, and the totals for addresses
without an account. The count per network is not shared: the apps have
their own, of tries rather than codes (below). Test accounts with a fixed code get a
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
  its code can still be used (unspent, unexpired, tries left) mails
  nothing and makes no request: `429 code_already_sent`, with
  `retry_after`. A code that was not mailed because a budget was full
  ([below](#what-a-flood-can-cost)) holds the install exactly the same
  way. Once its five tries are used the next start goes through (under the
  limits). A start from another install is
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
same whether or not it has one, and whether its code was mailed or not.
When a budget of mail is full (the new-address budgets below), the
request still gets a real code, made and stored as a mailed one is (the
same random six digits, the same hash, expiry and five tries), that is
simply never mailed (`email_codes.mailed` is false). So everything after
the answer is the same too: the install is held (`code_already_sent`),
wrong codes count down (`invalid_code` with `attempts_left`, then
`too_many_attempts` on the same try), and the address's sixth start in
the hour is refused. Nobody knows such a code's digits; it can only be
guessed, at a mailed code's odds. A code not mailed counts towards the
address's five an hour and towards no budget of mail (the new-address
budgets, the testing cap). A test account with a fixed code is answered
the same (nothing is mailed; the fixed code works) but is never held, as
above: accepted, since only staff make such accounts. There is no
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
Cloudflare Turnstile token, checked as the web form's is
(`src/account/guard.rs`), and bound to its widget: Turnstile's answer must
name the action `app-check` and the account site's host (the sign-in
forms' widget is `sign-in`), so a token from one widget never passes as
the other's. An answer that lacks either field fails the app's check
(the web form lets it through, [above](#who-can-be-mailed-a-code)). A
token that fails is `403 turnstile_failed`. While the token
is optional, codes given without one (start and resend alike) are held to
a testing cap of 20 an hour and 50 a day across every address, counted in
the database from the codes themselves (`email_codes.unchecked`), so the
counts survive a restart and are shared by every server. Past it: `429
too_many_requests` with `retry_after`. When a day's count reaches 25,
Admins and Support are mailed once that (Manila) day, as BIR uptime's
alerts are (`Mailer::send_notice` to `roles::staff_emails`). A request with
a good token skips the cap, never the shared limits. Without Turnstile keys
(local development) the token is ignored and the cap applies.

Codes given without a token to addresses without an account have a
budget of their own, 10 an hour and 30 a day, apart from the web's 20 and
60, so the app path can never use up the web's. Over it the answer is
200 and a code is stored and not mailed (above). A request with a good
token counts against the web's caps, as the form does.

<a id="what-a-flood-can-cost"></a>
**What a flood can cost.** Start and resend do the cheap refusals first,
none of which uses the database, in this order:
1. **The network's tries.** Per network (an IPv4 /24 or IPv6 /48, from
   `CF-Connecting-IP`), 200 tries an hour at start and resend together,
   counted for every request whatever comes of it (refused, bad
   Turnstile token, bad body, `503 busy`, mailed) and never given back:
   giving back a `busy` would let a flood go on trying for nothing just
   when the database is saturated. Past it: `429 too_many_requests` with
   `retry_after`. It is a limiter of its own, in memory, apart from the
   web form's (which the apps never touch), so filling it cannot hold
   back the web. Past it no Turnstile call is made.
2. The body.
3. Turnstile, when a token came.
4. **A place for the database work.** At most 4 starts and resends use
   the database at once (the pool has 10 connections). One waits at most 2
   seconds for a place, holding no connection, then is `503 busy` with
   `Retry-After: 5`. In its transaction it waits at most 3 seconds for a
   lock (`lock_timeout`), else `503 busy` too.

**Verify is bounded the same way**, the app's and the web form's
together (both guess codes):
1. **The network's tries at a code.** 300 an hour per network at
   `POST /auth/email/verify`, the apps' JSON and the web form's counted
   together, before the body is read and before any database work,
   whatever comes of them, never given back. Its own limiter, apart from
   start's and from the web's mail count. Past it the app gets `429
   too_many_requests` with `retry_after` (body and header), and the web
   form goes back to its code page with "Too many sign-in codes are being
   checked right now. Try again in an hour." (the network's own limit or,
   past a full map, the shared budget: the words fit both)
2. The body.
3. **A place of verify's own.** At most 3 verifies use the database at
   once, so a flood of guesses never takes start's 4 (4 + 3 stay under
   the pool's 10). One waits at most 2 seconds, holding no connection,
   then the app gets `503 busy` (`Retry-After: 5`) and the web form "Buwiz
   is busy. Try again in a moment." on its code page. Its transaction
   waits at most 3 seconds for a lock, else the same answer.

Both limits are wide because a carrier's NAT or a large office puts many
people behind one network, and many sign in at once near a filing
deadline. What bounds one address is its own limits (five codes an hour,
five tries a code), whatever the network.

**When the map of networks is full.** The two limiters above whose tries
are never given back (start/resend's, verify's) hold at most 100,000
networks an hour. When full, the networks new that hour share one budget
for the hour, not counted by network: 20,000 tries at start and resend,
30,000 at verify (settable, [deploy.md](deploy.md#migration-17-in-app-sign-in)).
Once it is spent, every new network is refused (`429
too_many_requests`, as past its own limit) until the hour ends; networks
already counted go on as before. So a flood of distinct networks cannot
keep the places for the database work and the Turnstile calls busy for
the whole hour. Cloudflare's rate rule in front stays the first line.
The web form's count of codes per network keeps one shared overflow
count of 10 when full: its refusals are given back, so a flood's
networks leave and make room.

Then, in one transaction under the address's lock (`lock_address`, which
every code of any purpose, web or app, is counted under, so requests at
once cannot pass the five together): whether the account has a fixed
code, `code_already_sent`, the testing cap (read without its lock, so a
full cap refuses at once; its lock is taken only to add a code that
counts), the address's five an hour, the new-address budget. The request
is written with its code (mailed or not) in that transaction, or not at
all; a refusal writes nothing and spends no place in any count, and a
code that could not be mailed is taken back with its request, in one
transaction. If that fails, the request is ended (`used_at`), so it holds
the install no more. Both run in one of start's places, each waiting at
most 3 seconds for a lock and for a pool connection, so a mail outage in
a flood cannot leave them waiting without end. If no place comes ("no
place to take it back in") or both fail ("could be neither taken back
nor ended"), the request's and code's ids are logged (no address) and
staff are mailed, at most once an hour. The app is answered
`mail_failed` either way.

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

When the widget passes, the page hands over the token two ways only, in
this order:
1. `window.nsh.postMessage(JSON.stringify({turnstile: token}))`, when the
   host injected `window.nsh` (the primary way);
2. then it goes to `/app-check/done#token=<token>`, a page that says "You
   can close this window" and reads nothing on the server: a host without
   the bridge reads the token from that address's fragment.

Nothing is posted to a parent or opener window and the title is not
written, and both pages send `Cross-Origin-Opener-Policy: same-origin`, so
a page that opened `/app-check` keeps no handle on it. When the widget
fails, errors, expires or times out, the same two ways carry
`{turnstile_error: "<code>"}` (`/app-check/done#error=<code>`). Codes:
`expired`, `timeout`, `error_<Turnstile's code>`, and `not_configured`
when the server has no Turnstile keys (local development; the page says
so and sends it at once). `/app-check/done` and `/app-sign-in` send a
strict Content-Security-Policy (`/app-check/done`'s own script, the site's files);
`/app-check` loads Turnstile and has none of its own.

Where the web view cannot run (Linux under Wayland, a view that fails to
load) the app falls back to the browser device flow, which stays.

### The mail

The mail says to enter the code in eBIRForms on the device that asked,
gives the code and how long it lasts, and says what to do if the person
did not ask. It has no link: the person types the code into the app.

Mails sent before this carried a link to
`https://account.buwiz.com/app-sign-in#r=…&t=…`. That page stays, so
those links still open (`src/account/pages/app_sign_in.rs`, on the
account host, no sign-in, noindex). It has no button and no script: it
says to type the 6-digit code from the email into the app, approves
nothing and calls no endpoint. No link token is made or stored any more;
`app_sign_ins.link_token_hash` (migration 17) stays empty.

**Records.** `app_sign_ins` holds each request (address, client, the
secret's hash, intent, platform, install id, its code, expiry, when it
was used). The hourly upkeep deletes a request 30 days after it
expired, the cap's counts after two days and its alert days after 30.
Deleting an account deletes its requests, by address, with its codes.

### The API

On api.buwiz.com only, like `/oauth/token` and `/me`. On any other host
(account.buwiz.com among them) these three answer `404`: Cloudflare puts a
Managed Challenge on the account host's sign-in paths, which no app can
pass (deploy.md, "Cloudflare rules for sign-in"). Each takes and gives
JSON (`Content-Type: application/json`) and answers `Cache-Control:
no-store`. The web's form posts to the same two paths, start and verify,
are told apart by the content type, and the shared paths do not mean
either host takes both: the form is served on account.buwiz.com only
(`404` elsewhere, api.buwiz.com included), the JSON on api.buwiz.com
only. Every refusal is
`{"error": "<code>", "message": "<sentence>"}`, with `retry_after`
(seconds, also as `Retry-After`) or `attempts_left` where they apply.

`POST /auth/email/start`

```json
{"client_id": "buwiz-desktop", "email": "ana@example.ph", "intent": "sign_in",
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

`POST /auth/email/verify` with `{"request_id", "secret", "code"}`. A body
without `code` (one with a `link_token` instead, among them) is
`invalid_request`, as any body that is not the JSON expected. First the network's tries at a code
(300 an hour, [above](#what-a-flood-can-cost)), then a place for the
database work. Then checked in this order: the request
exists and is not used, the secret is its own, it has not expired, then
the code (one try of its five), then a ban.
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
| 403 | `account_banned` | The account is banned (the message is `/me`'s for a banned account) |
| 403 | `turnstile_failed` | A `turnstile` token that Turnstile refused |
| 403 | `turnstile_required` | Reserved: not used yet (see the 1.0.0 rule above) |
| 429 | `code_already_sent` | A start from the same install for an address whose code from it still works; `retry_after` |
| 429 | `too_many_requests` | Start and resend: the network's 200 tries an hour, a resend before the request's code expired, the testing cap, or the address's five codes an hour. Verify: the network's 300 tries at a code an hour. `retry_after` |
| 502 | `mail_failed` | The mail provider failed; the request cannot be used (start again) |
| 503 | `busy` | Start, resend and verify: too many requests at once, no place for the database work within 2 seconds, or a lock not had within 3. Nothing was done; `Retry-After: 5` (try again in a few seconds) |
| 503 | `restarting` | The server is stopping (deploy.md, "What a deploy does at stop"); `Retry-After: 30` |

## Sign in with Buwiz

The eBIRForms apps can sign in in the browser, as the built-in client
`buwiz-desktop`, with the authorization code flow and PKCE (S256 only) on
the same `/oauth/authorize` and `/oauth/token` the agents use
(`check_app_authorize`, `redeem_code` in `src/account/agents.rs`):
1. The app makes a PKCE verifier and a random `state`, opens
   `https://account.buwiz.com/oauth/authorize` in the system browser (or
   an in-app browser session) with its `install_id` and `platform`, and
   listens on its redirect address.
2. Without a web session the page sends the person to sign in on the web
   and back to the same address. A banned account has no web session.
3. The page is Buwiz's own, not the agents' consent: "Sign in to eBIRForms
   on iPhone" (the platform's word), one button "Continue as
   <email>", **Cancel**, and **Use another account**, which signs the web
   out (`POST /logout` with `next`, same-origin as every form) and comes
   back here after signing in again. It is never approved on its own: a
   live session still needs the tap. The page says what the app can do
   with the scope `app`: file and send returns for the account's TINs
   through Buwiz, see its TINs and the returns it sent with their
   receipts, and see and remove its devices and sign-ins.
4. Continue stores a one-time code for ten minutes with scope `app`, the
   install and the platform (`oauth_codes`), and redirects to the app with
   `code`, `state` and `iss`. Cancel redirects with
   `error=access_denied`.
5. The app checks `state` and `iss`, and exchanges the code with its
   verifier on `/oauth/token`. The grant gets the code's scope, install
   and platform, as in-app sign-in stamps them, so `POST /api/devices` and
   one row per install ([Devices](#devices)) work the same.

The page and the decision's answer carry `X-Frame-Options: DENY` and
`Content-Security-Policy: frame-ancestors 'none'` (`no_frames` in
`src/account/web.rs`, added to any policy already there), so another site
cannot frame the page and trick the tap out of someone.

### Redirect addresses

Fixed in the code (`app_redirects`), never registered, never in
`oauth_clients`. Only `buwiz-desktop` has any; the other built-in clients
(`buwiz-ebirforms`, `buwiz-books`) cannot use `/oauth/authorize`, and a
row in `oauth_clients` under a built-in id is ignored. `redirect_uri` is
required:
- `ebirforms://signin-callback`, exactly: no trailing slash, path, query
  or fragment, and the scheme in lowercase;
- `http://127.0.0.1:<port>/callback` (RFC 8252 §7.3): the host exactly
  `127.0.0.1` (not `localhost`, not `[::1]`), a port written out (any, 1
  to 65535, no leading zero), the path exactly `/callback`, and no query,
  fragment or user info.

An address outside these is never redirected to: the page says the
request cannot be used. Agents keep their own registered addresses and
the `mcp` scope; registration refuses the `ebirforms` scheme, and a
registered client's id is always `agent_` and a new UUID, so it never
names a built-in client.

### What PKCE and state cover

PKCE: a code caught on its way to the app (another app on the phone
claiming `ebirforms://`, or listening on the loopback port) is useless
without the verifier, which never left the app that started. `state`
(16 characters at least, the app's own random value) ties the answer to
the request the app made, so the app takes no code it did not ask for (a
link from someone else that lands on its redirect address).

What neither covers: an app that pretends to be eBIRForms from the start
makes its own verifier and state, so if the person taps Continue it is
signed in as them. Nothing in a browser can tell the real app from such a
one. That is why the tap is never skipped, even with a live session, and
why a new device is mailed.

### The new-device mail

After the exchange (on its own task, so the token answer never waits for
it), when the account had no live `devices` row for the install at the
exchange, the account is mailed "New device signed in: eBIRForms on
<platform>": when it happened (Philippine time), and "If this wasn't
you, log it out at https://account.buwiz.com/tins#devices-h" (My
devices). Fixed words only: nothing from the request but the platform's
word.
- New means no row, or a row the owner (My devices, or the app's own
  remove call) or staff removed (`devices.removed_by`, set with
  `revoked_at`): a removed device that signs in again is mailed again.
  A row that ended with its sign-in (signed out, replaced, or any grant
  ended by 0018's trigger) has no `removed_by`: the same device signing
  in again is not mailed. Registering again clears `removed_by`. My
  devices (and the staff page) says "Signed out" for such a row and
  "Removed" for a removed one; the owner may still remove a signed-out
  row, which marks it `owner`.
- It is decided in the exchange's transaction, so an app that registers
  its device at once is still mailed.
- A known device signing in again, and an account signing in with a
  fixed code (a store reviewer's), are mailed nothing.
- A mail that fails is logged with the account and grant ids, and the
  sign-in stands.
- At most 3 in any 24 hours per account (`new_device_mails`, counted in
  the exchange's transaction over the last 24 hours, not the calendar
  day). A person adds a phone, a computer, maybe reinstalls one; more is
  a script with the web session, whose mails would bury a real alert. The
  third says "We send at most three new-device notices in 24 hours, and
  this is the third" and asks the person to check My devices; no other
  mail is sent. The next one can go once the oldest of the three is 24
  hours old. Past it the sign-in stands, nothing is mailed, and one log line
  has the account and grant ids.
- Counting fails the sign-in only when the count itself cannot be made:
  its lock not had within 3 seconds is `503 temporarily_unavailable`
  (the device flow's poll too), any other database error is `500`. Either
  way the sign-in is rolled back, nothing is mailed, and the code or
  device code stays unspent, so the app may try again.
- The device flow sends it the same way, when its sign-in named an
  install (`/oauth/device/code` with `install_id`): it is approved in a
  browser too. Its device word is "eBIRForms", as the device flow names
  no platform. Apps from before install ids are not mailed.
- In-app sign-in does not send it: the code mailed to the address is
  already the notice.
- Its subject is not the replaced-sign-in mail's ("New sign-in to …",
  [Devices](#devices)), which comes at registration when the install's
  row already had a live sign-in.

### The authorize and token API

`GET https://account.buwiz.com/oauth/authorize` with:

| Parameter | |
|---|---|
| `client_id` | `buwiz-desktop` |
| `redirect_uri` | One of the two addresses above |
| `response_type` | `code` |
| `code_challenge` | base64url SHA-256 of the verifier (43 characters) |
| `code_challenge_method` | `S256` |
| `state` | The app's random value, at least 16 characters |
| `install_id` | 16 to 64 of `A-Z a-z 0-9 - _` |
| `platform` | `ios`, `ipados`, `android`, `macos`, `windows` or `linux` |
| `scope` | Optional; only `app` |

`resource` is not needed (ignored). Success redirects to
`<redirect_uri>?code=bwc_…&iss=https%3A%2F%2Faccount.buwiz.com&state=…`;
an error to `<redirect_uri>?error=…&error_description=…&iss=…&state=…`
(`state` when the request had one):

| `error` | When |
|---|---|
| `invalid_request` | `state` missing or shorter than 16; `install_id` missing or malformed; `platform` missing or not one of the six; no `code_challenge`, or not S256 |
| `unsupported_response_type` | `response_type` is not `code` |
| `invalid_scope` | `scope` is there and is not `app` |
| `access_denied` | The person tapped Cancel |

A wrong `client_id` or `redirect_uri` redirects nowhere; the page says so.

`POST https://api.buwiz.com/oauth/token` (form-encoded) with
`grant_type=authorization_code`, `client_id=buwiz-desktop`, `code`,
`redirect_uri` (the same string as at authorize) and `code_verifier`.
Answers `200` with the device flow's token answer, scope `app`; refresh
as for every app (`grant_type=refresh_token`). The code works once,
within ten minutes, for its client, address and verifier: anything else
is `400 invalid_grant` (and uses the code up), and a banned account gets
`400 access_denied`.

What a flood of made-up codes can cost:
1. **Counted and alerted, never refused.** Every grant on `/oauth/token`
   (`authorization_code`, `refresh_token`, the device code) and
   `/auth/device/poll` is counted against its network's hour (IPv4 /24,
   IPv6 /48, from `CF-Connecting-IP`), and only tries that fail with
   `invalid_grant` stay counted: a refresh or an exchange that works, and
   a poll told to wait, are given back. The tries a network makes past 300
   failures in the hour (`TOKEN_FAILURES_PER_NETWORK`, deploy.md) are in
   the hourly log line and the staff alert with the other sign-in limits,
   and are served as any other: one stuck app or a neighbour behind a
   carrier's address must never lock that address out, and main has no
   such lockout. A slow lane for such networks is issue #84.
2. A code, refresh token or device code that is not its prefix (`bwc_`,
   `bwr_`, `bwd_`) and 43 characters of base64url was never given out:
   `400 invalid_grant` before any database work.
3. **A place of the exchange's own.** At most 2 exchanges use the
   database at once, so a flood never takes in-app sign-in's places
   (start's 4, verify's 3, these 2: 9, under the pool's 10). One waits at
   most 2 seconds for a place, holding no connection.
4. Its transaction waits at most 3 seconds for a connection and for each
   lock (`begin_waiting`).

When no place comes within 2 seconds (3), or a connection or a lock does
not come within 3 (4), the answer is `503 temporarily_unavailable`: the
code is not used, and the app may try again in a few seconds.

What is left, plainly:
- A stream of well-shaped made-up codes can keep Sign in with Buwiz busy
  (`503`, its 2 places) while it lasts. In-app sign-in keeps working on
  places of its own. Filing has no places of its own: it keeps working
  only because the exchange holds at most two of the pool's 10
  connections.
- The device-code path is as on main (#84). A poll of a real device code
  (`authorization_pending`, `slow_down`) costs a pooled connection and a
  write, and is never counted, however fast it comes; only a poll that
  fails with `invalid_grant` is. `/auth/device/start` (and
  `/oauth/device/code`) is not limited at all and writes a row per call.

## Devices

An app that signs in can register a device key
([relay-protocol.md](relay-protocol.md#51-device-key)); the account's
devices are under **My devices** on My TINs. An app sends `install_id`,
its own id for its install (an identifier, not a secret), when it signs
in (`/auth/email/start`, `/oauth/device/code`, `/oauth/authorize`) and
when it registers. The
sign-in's grant keeps it, and an install has one device row per account
(a unique index): signing in again on the same install and registering
updates that row (its key, its sign-in, what the app says about the
device, `signed_in_at`) and ends the sign-in it had, so a reinstalled or
signed-out-and-in app does not pile up rows. A registration whose install
is not the sign-in's is refused (`install_mismatch`). A sign-in without
an install id may not take an install whose live device belongs to
another live sign-in of the account (`409 install_in_use`, nothing
changed); once that sign-in is over or the device removed, it may. A
sign-in that carries an install id registers one device; one without (the
apps from before) may register again, each time another row.

**Replacing a device's sign-in, by design.** A sign-in that named its
install when it began (the in-app start, or the device flow's
`/oauth/device/code`) and was completed is proof of the account: its
registration replaces that install's row, whatever sign-in held it, and
ends that sign-in (`device_replaced`). The residual risk: someone who
knows a device's install id and can complete a sign-in on the account
replaces that device's sign-in. When the sign-in replaced was still live
(not ended, with a refresh token that works), the account is mailed "New
sign-in to eBIRForms on <platform>": that the earlier sign-in on that
device was ended, and to log the device out under My devices
(`/tins#devices-h`) if it wasn't them. Fixed words only; sent after the
registration commits, off the request; a failure is logged with ids only
and never fails the registration. A test account with a fixed code is not
mailed.

Whenever a sign-in ends (signed out, its refresh token reused, ended by
staff or by the person), its live device is removed with it, at the same
time: a trigger on `oauth_grants` does it for every way a grant ends.

What the app says about the device: its platform, model (an Apple code
such as `iPhone16,1` is shown as "iPhone 15 Pro", from the table in
`src/filing/apple_models.rs`, which is filled only from Apple's published
identifier lists; an Apple code not in it shows as "iPhone (iPhone18,1)",
"iPad (…)" or "Mac (…)"; other models as sent), system version and, on a
computer, its name. The list shows those, when it was added, last signed
in and last used, and keeps a removed device for 30 days. A device whose
sign-in has no refresh token that works any more (and was not ended)
shows as "Signed out (expired)", not live. `GET /api/devices` gives each
row a `state`: `live`, `expired` or `removed`, besides its other fields.

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

## Resetting a forgotten app PIN

The eBIRForms app locks with a PIN that lives on the device; the server
never sees it. A person who forgot it proves they can read the account's
mail, on this device, and the account is told. Two routes on the API host,
for the BIR app's bearer only (`403 insufficient_scope` for any other;
`src/account/pin_reset.rs`). Every refusal has the API's usual shape,
`{"error": {"code", "message"}}`, and no answer is cached.

**Which device.** For a real account, the token's sign-in must have a live
device row (`409 device_not_registered` otherwise), and a request is bound
to that device. A practice account (a store reviewer's, with a fixed code:
`practice` on `GET /me`) registers no device, so its request is bound to
the token's sign-in (app grant) instead; it never gets
`device_not_registered`. A request made on one device (or sign-in) is
refused on another, and refused once used.

**`POST /api/pin-reset/start`.** The body is empty, `{}`, or
`{"device_name": "..."}` (at most 100 characters; only a practice
account's notice uses it). A code of the purpose `pin_reset` is mailed to
the account's own address ("Your eBIRForms PIN reset code": the in-app
sign-in code's mail with words for a PIN reset, the code its only number).
It lasts `APP_SIGN_IN_MINUTES`. Answer: `200 {"request_id", "expires_in"}`.
A practice account is mailed nothing: its fixed code is accepted at
verify. Refusals: `400 bad_request`; `409 device_not_registered`; `429
code_already_sent` while this device's last code still works
(`Retry-After` until it expires); `429 rate_limited` (with `Retry-After`)
past 3 starts per device an hour or 5 per account an hour across its
devices (in memory), 10 per account a day (counted in `pin_resets` under
the account's lock, so a restart does not reset it), or the address's five
codes an hour (every purpose together); `503 busy`; `503
mail_unavailable` when the code could not be mailed (nothing is kept, and
the start does not count).

**`POST /api/pin-reset/verify`** `{"request_id", "code"}`. In this order,
which the app relies on:
1. In one transaction, the account is locked and the code taken. It must
   be the request's own code, of the purpose `pin_reset`, the request this
   account's and this device's (or sign-in's), unused and unexpired, with
   tries left; a wrong try is counted and committed, as on the sign-in
   page.
2. In the same transaction, the request is marked used and one
   `account_audit` line written: action `pin_reset_verified`, source
   `app`, the account as actor, reason `device <name> (<device id>)` or,
   for a practice account, `grant <grant id>`. It commits.
3. Then the notice is mailed to the account's own address, waiting at
   most 10 seconds. Never before the commit.
4. Only once the mail service took it: `200 {"verified": true, "device":
   "<name>", "at": <unix seconds of the verification>}`. Otherwise `503
   mail_unavailable`, and staff are mailed an alert (ids only). The spent
   code stays spent and the line stays; the person asks for a new code.

**The app changes the PIN only after verify's `200`.** Any other answer,
`503 mail_unavailable` included, leaves the PIN as it is.

Refusals: `400 bad_request`; `400 code_invalid` (a wrong code, an unknown
request, another device's, one already used, or a code of another
purpose: one answer for all); `400 code_expired`; `429 too_many_tries`
(the code had its five wrong tries; ask for a new one); `429
rate_limited` past 10 tries per device an hour (in memory, with
`Retry-After`); `409 device_not_registered`; `503 busy` (nothing was
taken); `503 mail_unavailable`.

A code works only for its own purpose: a sign-in or deletion code never
resets a PIN, and a PIN reset code never signs in (on the web or in the
app) or deletes an account.

**The notice** ("A PIN reset was asked for in eBIRForms"), plain text,
with no code and no link: "A PIN reset was asked for on <device> on <date
and time>, Manila time (<UTC time>)." Then "If this was you, there is
nothing to do." and "If it was not you: sign in at account.buwiz.com, open
My devices, and remove <device>. Removing it signs it out and stops it
from sending returns." `<device>` is the device row's name, or for a
practice account the name its app reported at start, else "this device".
It is sent for every verified reset, practice accounts included. "Asked
for", not "was reset": the person can still cancel setting the new PIN.

The account's history on the staff pages shows the line as the person's
own, and it is not staff activity. Deleting the account deletes its
requests; the lines stay, by id, without the address. The hourly upkeep
deletes requests 30 days after they expired.

## Known limits of in-app sign-in

Reviewed and accepted for now (PR 81's review):
- **M5.** Five requests an hour for an address, from anyone, hold that address for the hour, as on the web.
- **L5.** Address aliases (`name+1@`, Gmail's dots) count as other addresses for the per-address limit; the new-address budgets bound them.
- **L6.** `code_already_sent` tells someone who knows an install id that it has a live request for that address.
- **L7.** Response times may differ for accounts with a fixed code or when a code is not mailed (a mailed code's mail is awaited before the answer); to be measured. The code path is otherwise the same: an unmailed code is made, stored and checked as a mailed one is.
- **Device replacement.** Someone who knows a device's install id and can complete a sign-in on the account replaces that device's sign-in ([Devices](#devices)); the account is mailed when the sign-in replaced was live.
- **Fixed-code test accounts** are never held by `code_already_sent`, so a second start from one install tells them apart; only staff make them.
- **The web form's new-address cap** stores nothing when full (unlike the apps' unmailed codes), so on the web an unknown address over that cap is not refused at its sixth start in the hour, as an address with an account is. It needs the web's 20 codes an hour for new addresses spent, through Turnstile.
- **The network's 200 tries** at start and resend, and its 300 at verify, are shared by everyone on one /24 or /48 (an office, a carrier): a flood from it holds the others there for the hour. A `503 busy` counts too, by design.
- **The per-network limits are a guess** (200 and 300 an hour): a large carrier range at a filing deadline may carry more. Refusals are counted, logged once an hour, and mailed to staff past 100 in an hour; the limits are settings, raised with a restart ([deploy.md](deploy.md#migration-17-in-app-sign-in)).
- **A full map of networks** (100,000 in an hour) gives the networks new that hour one shared budget (20,000 tries at start and resend, 30,000 at verify), with no limit per network inside it. So one new network can spend the whole budget, and a burst can spend it in minutes (about 100 seconds when the start places are saturated); then every new network is refused until the hour ends, real people among them. An attacker needs 100,000 distinct networks in one hour first. What still holds: mail and guessing stay bounded (five codes an hour and five tries a code per address, the new-address budgets, the testing cap), and networks already counted keep their own limits. At that scale Cloudflare's rules in front are the control; there is no second map.
- **NEW-5.** The stuck-request alert goes over the mail service whose failure caused the stuck state, so it may not arrive; and the hour's alert is taken before the send, so a failed send is not tried again that hour (the log line stays).
- **NEW-7.** In a mail outage during a flood, take-back may find no place, so more unmailed codes hold their installs until they expire (5 minutes).
- **NEW-9.** "Public" is still only the https scheme of `ACCOUNT_ORIGIN` (with `SECURE_COOKIES`), not the ingress.
- **L9, NL9.** Without `CF-Connecting-IP` there is no per-network count, at start, resend or verify. Production always sets it (the tunnel), and a missing header raises an hourly staff alert (BIR uptime's `NetworkReader`).
- **NC1.** Unmailed codes count toward the address's five an hour and hold installs, by design, so an address without an account looks the same as one with an account; while the budgets of mail are full an address can be held for the hour, as M5 says.
- **NW1.** The web form lets through a Turnstile answer that lacks the hostname or action (the app's check refuses it), with a staff alert at most once an hour.
- **NI1.** The replacement mail goes to the owner even when they replaced their own live sign-in (a reinstall that lost its tokens): rare and harmless.
- **NU1.** The loopback-host allowance (both kinds of sign-in on `localhost`) holds only when the deployment is not public (`secure_cookies` off); on a public one a loopback host is any other unknown host. An https `ACCOUNT_ORIGIN` with `SECURE_COOKIES` not `true` stops the server at start-up.
- **C1.** A code can be phished: the secret binds it to a request, not to the person's device, and only the mail's words protect.

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
are in [relay-protocol.md](relay-protocol.md#refusals). Each of those rows
also says what the app does `next` (`send`, `wait` or `fix`, from the same
rule: for the BIR app's token, `fix` exactly when it has an action;
`send` exactly when it can send). Every mode is limited to 120 calls per sign-in in 10 minutes
(`429 rate_limited` with `Retry-After`).

`POST /api/tax-profiles` lets the BIR app add a TIN as not verified, as
"add papers later" does on My TINs, with the same checks (this account's
rows only, the cap of 20, names of at most 200 bytes, a trade name for
either kind) and one audit line (source `app`, reason "added
from the app"). The app calls it only from an explicit "Activate this TIN
to send" button, never when a tax profile is created on the device. A test
TIN is refused (`test_tin`); at most 10 tries per sign-in an hour. The
request, answer and codes are in
[relay-protocol.md](relay-protocol.md#adding-a-tin-from-the-app).

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
