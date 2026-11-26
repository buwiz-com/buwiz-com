# Submission relay protocol

How a Buwiz app files a return with BIR through buwiz-cloud. BIR's
passphrases and SFTP login stay on the server. The traffic leaves from the
taxpayer's computer, so BIR sees the taxpayer's own connection, as with
eBIRForms.

There are two versions. **v2** (section 5) is what new apps use: the app
signs requests with a device key, gets a signed ticket for one return, and
seals the XML so that only the Sender can read it. **v1** (sections 1 to 3)
still works for existing clients.

The server is two processes:

- The **Gatekeeper** (`buwiz-cloud`): the public API, accounts, TIN claims,
  devices and tickets. It holds no BIR secret and cannot open a sealed
  return.
- The **Sender** (`buwiz-sender`): holds BIR's passphrases, the SFTP login
  and the HPKE private key. It has no public address; the Gatekeeper
  reaches it on a private network (section 6).

Buwiz Forms `bir-submit` (hexuria/buwiz-forms) is the v2 client. The reference v1 client
(`src/relay/client.rs`, `src/bin/buwiz-relay.rs`) is part of the BIR relay
code.

The BIR relay code itself (`src/bir/`: BIR's dispatcher, the IAF
encryption, the SFTP upload, BIR's host list) is behind the `bir` cargo
feature and kept in private files. BIR's hostnames, ports, dispatcher path,
client version and upload-host fingerprints live there and in that code's
own docs, not here. A build without the feature runs everything else in this
document, and its Sender answers every send with `failed`, `"relay not
built in"` (section 3): nothing reaches BIR, so the send can be retried
against a full build. Production builds use `--features bir`.

## 1. Create the submission

```http
POST /api/submissions
Authorization: Bearer <access token>
Content-Type: application/json

{"filename": "12345678900000-1601Cv2018-062025#you@example.com#.xml",
 "xml": "<plaintext IAF XML>",
 "idempotency_key": "<optional, 1-100 chars, e.g. the local queue id>"}
```

- `filename` follows eBIRForms: `{TIN}-{form}-{period}#{email}#.xml`. The
  TIN is 12 or 14 digits: the 9-digit root, then a 3- or 5-digit branch.
  The email is where BIR sends its confirmation.
- `xml` is the plaintext IAF file, at most 2 MB. The server does the IAF
  encryption.
- The return inside must be the one the file name names: its TIN and
  branch fields, its form (the field prefix, `frm1601c` for `1601Cv2018`)
  and its year (and ticked quarter) must be the name's. None of those
  fields may come twice with different values, and the markup must read
  the same to an XML reader as to a split on `<div>`: no comments, no
  CDATA, no processing instruction after the prolog, no `<div` with
  attributes or in other case. Otherwise
  `400 {"error":{"code":"return_mismatch","message":"The return does not
  match its file name: …"}}`, and nothing is stored. This is the same check
  the Sender makes on a sealed return against its ticket (section 5), and
  the Sender makes it again on every v1 return before it sends.
- The TIN must be able to send on the signed-in account (an active claim,
  not paused). Otherwise `403` with one of the codes in
  [Refusals](#refusals).
- Sending the same `idempotency_key` again returns the existing submission
  without creating another.

Answer `201`:

```json
{"id": "…", "status": "ready", "relay_path": "/api/submissions/…/relay", …}
```

A submission for a test TIN ([Test TINs](#test-tins)) also has
`"test_only": true`, here, in an idempotent replay, in
`GET /api/submissions/{id}` and in each item of `GET /api/submissions`.
Any other submission has no such field, and reads as before.

The server keeps the XML, encrypted, until it is sent or 24 hours pass.

### Refusals

Tickets (`POST /api/tickets`), both submission calls and the relay refuse
a TIN that cannot send with `403 {"error":{"code","message"}}`. The code
says where the caller's **own** row for that TIN stands; another account's
row is never looked at, so "someone else owns it", "never added" and "it
ended" all read `tin_not_active`, word for word alike. These codes are a
closed set: a code an app does not know (such as the `503` holds of
[bir-uptime.md](bir-uptime.md#10-codes)) is not a TIN refusal, and every
app that can send treats it as a plain failed try (not sent), never as
`tin_not_active`.

| `code` | When | `message` |
|---|---|---|
| `tin_unverified` | Added without its papers | This TIN is not verified yet. Verify it with its papers on My TINs at account.buwiz.com to send returns. |
| `tin_pending` | Papers waiting for review | This TIN's papers are still being checked, usually within one business day. You can send once it is active. |
| `tin_needs_fix` | Sent back for a fix | `Sent back for a fix: ` and the fixed reason sentences (never the staff's own note), or `Sent back for a fix. See what to change on My TINs at account.buwiz.com.` |
| `tin_paused` | Active, but sending is paused | Sending is paused for this TIN. Write to support@buwiz.com. |
| `tin_not_active` | Anything else | this TIN is not active on your account |
| `account_banned` | The account is banned (every bearer call but `POST /api/auth/logout`; the token refresh still works) | This Buwiz account is banned: it can't send returns. Write to support@buwiz.com. |
| `account_practice` | A practice account: one with a fixed sign-in code stored, the kind app-store reviewers use. `practice` on `GET /me` is narrower (it also needs no role and no ban), so the server may refuse an account whose `/me` says `false`. Tickets, both submission calls and the relay refuse it, whatever the TIN (a `POST /api/submissions` retried with the idempotency key of a return already stored gets that return first); its TIN list is as anyone's | This is a practice account: you can prepare returns, but Buwiz never sends them to the BIR. |

The account's codes come before any TIN's, in this order: `account_banned`,
then `account_practice`, then the TIN's own. A practice account holding a
test TIN is refused `account_practice`.

A **test TIN** (a reserved root, `000-000-000` or `111-111-114`, any
branch, which staff add to
test-labelled accounts only) answers as any TIN does: an active one is
sendable (a paused one answers `tin_paused`), and its send ends as a dry
run (section 3, [Test TINs](#test-tins)). Servers before the dry run
refused an active test TIN, paused or not, with `tin_test_only` ("This is
a test TIN: you can prepare returns for it, but Buwiz never sends them to
the BIR."); this server never sends that code. An app that still knows it
treats it as the refusal it was: back to a draft, no retry.

None of these is worth retrying: the return goes back to a draft with the
words shown. Nor is `return_mismatch` (a `400` from `POST /api/submissions`,
or in brackets on a relay `failed`, section 3): the return is not the one
its file name or ticket names. Retry only:
- `429 quota_exceeded`, later;
- network errors, and a `5xx` other than the holds below;
- a `503` hold (`bir_offline`, `bir_busy`, `relay_paused`), not before its
  `wait_until` (or `Retry-After`), and `429 try_anyway_limited`, not
  before its `try_anyway.next_at` ([bir-uptime.md](bir-uptime.md#5-holds-on-tickets));
- a relay `failed` without a code in brackets (section 3).

Never resend a relay `unknown` by itself. A failure whose `class` is
`ours` is not counted as one of the return's tries.

### The TIN list

`GET /api/tax-profiles?include=all` (Bearer, any scope; `all` may come in a
comma list, `include=all,pending`) answers
`{"include":"all","profiles":[…]}`: every TIN on the account that has not
ended, each with `status` (`approved`, `paused`, `unverified`,
`pending`, `needs_fix`; older servers also sent `test_only`), `can_send`,
a `message`, and an `action` link to Verify or Fix. A test TIN's row also
has `"test_only": true` (no other row has the field), and an active one's
`message` says its sends are dry runs. `status` is `approved` exactly when `can_send` is true; both come from
the same rule as the refusals above. A `needs_fix` row also carries
`reasons`, `reason_codes` and the staff `note` (at most 200 characters, BIR
app only). Only the BIR app (`app` scope) gets `tin`, filing permissions,
the link and the note. Without `all`, the list means what it always did,
except that a paused TIN is left out; a test TIN that can send is listed,
with `"test_only": true`. `tests/fixtures/tax_profiles_all.json`
is the golden example; [accounts.md](accounts.md#licence-gate) has more.

Each `include=all` row also has `next`, what the app does with it, from
the same rule as `status`:

| `status` | `can_send` | `next` | `action` |
|---|---|---|---|
| `approved` | true | `send` | null |
| `paused` | false | `wait` | null |
| `pending` | false | `wait` | null |
| `unverified` | false | `fix` | Verify to send (`/tins/claims/<id>/verify`) |
| `needs_fix` | false | `fix` | Fix and send again (`/tins/claims/<id>/fix`) |

`next` is `fix` exactly when the row has an `action` (for the BIR app;
other clients get no links), and `send` exactly when `can_send`. A test
TIN reads the same, with `test_only`. The no-include and `include=pending`
lists have no `next`. A tax profile on the device whose TIN (root and
branch) is not in the list is not activated.

**Limit.** 120 calls per sign-in (the app's grant, one per device
sign-in) in 10 minutes, every mode together. Past it: `429` with
`Retry-After: <seconds>` and
`{"error":{"code":"rate_limited","message":"Too many requests. Try again in N minutes."}}`.
The app reads the list at start, on foreground and before Queue, never on
a timer; on a `429` it keeps the last list and tries again after
`Retry-After`.

### Adding a TIN from the app

`POST /api/tax-profiles` (BIR app only, scope `app`) adds a TIN to the
account as not verified, without papers, the same as the account site's
"add papers later". The app calls it **only** from an explicit button
("Activate this TIN to send") on a tax profile that is not on the account,
**never** when a tax profile is created: a profile made on a device stays
on the device until the person asks to activate it.

```json
{ "kind": "personal" | "business",
  "tin": "123-456-789-00000",
  "registered_name": "Dela Cruz Juan",
  "trade_name": "optional, may be omitted or empty" }
```

`tin` follows the site's rules: 9 digits for the head office (branch
`00000`), 14 with the branch (12, with a 3-digit branch, too); dashes and
spaces allowed. `registered_name` is 1 to 200 characters after trimming,
`trade_name` at most 200. The answer is `201 {"profile": <row>}`, the row
as `include=all` lists it (`status` `unverified`, `can_send` false, `next`
`fix`, `action` Verify to send); its `id` stays the same once the claim is
approved. Each add leaves one audit line (action `created`, source `app`,
reason "added from the app").

| HTTP | `code` | When |
|---|---|---|
| 403 | `insufficient_scope` | Not the BIR app's token |
| 400 | `bad_request` | Not the JSON above, or an unknown `kind` |
| 400 | `bad_tin` | The TIN does not read |
| 400 | `name_missing` | The names break the length rules |
| 400 | `test_tin` | A reserved root (`000-000-000`, `111-111-114`): staff add test TINs |
| 409 | `tin_already_active` | This account already has it active |
| 409 | `tin_already_added` | This account already has it as not verified |
| 409 | `claim_already_pending` | This account already has it in review or sent back |
| 409 | `too_many_unverified` | This account already has 20 TINs not verified |
| 429 | `rate_limited` | Over 10 tries per sign-in in an hour (every try counts), with `Retry-After` |

No answer says whether another account holds the TIN: adding one that
another account holds, active or not, reads exactly like adding a free
one, and a refusal speaks of this account's own row only.

## 2. Open the pipe

Open a WebSocket to `relay_path` on the same server (`wss://`), with the
same `Authorization: Bearer` header. The app then forwards bytes:

| Frame | Direction | Meaning |
|---|---|---|
| text `{"type":"open","stream":N,"host":H,"port":P}` | server → app | Connect to `H:P` for stream `N`. |
| text `{"type":"opened","stream":N}` | app → server | Connected. |
| text `{"type":"refused","stream":N,"reason":"…","kind":K}` | app → server | Not connected. `kind` is optional (newer apps): `dns`, `refused` (also "no route to host"), `timeout`, `unreachable` (the app's own network), `denied` (the app's own system refused), `not_listed` (not on the trust list). |
| binary `[N][bytes…]` | both | Bytes on stream `N`. The first byte is the stream id. |
| text `{"type":"close","stream":N}` | both | The sender will write no more on `N`. Shut down the write side. |
| text `{"type":"result","status":S,"error":E,"receipt":R}` | server → app | Final. The server closes the socket after it. It may carry more fields (section 3). |

Readers ignore fields they do not know, and a text frame whose `type` they
do not know.

The app must:

- **Connect only to BIR's hosts**: BIR's dispatcher (HTTPS and HTTP, on a
  primary and a backup host) and its SFTP upload server. The list is in the
  private BIR relay files. The v1 reference client hard-codes it and
  refuses anything else with `refused`. This keeps the server from using a
  taxpayer's computer as a general proxy. v2 apps take the list from the
  signed relay trust list instead (section 5.4) and name no host in their
  source.
- Forward bytes unchanged. Never terminate TLS or SSH. Everything the app
  sees is TLS or SSH ciphertext, or the dispatcher's wrapped fields.
- Keep the socket open until `result`.

What the server does over the pipe:

1. Asks BIR's dispatcher for the upload server and its login, trying HTTPS
   first, then HTTP, on the primary host, then the backup.
2. Unwraps the upload server and login on the server side.
3. Checks the upload host against its own list.
4. Runs SSH to it, refusing any host key that is not pinned.
5. Logs in and writes `/{form}/{filename}`, encrypted the way
   `Encrypt.exe` does.

## 3. The result

| `status` | Meaning | What the app does |
|---|---|---|
| `sent` | BIR's server accepted the file. `receipt` is a signed receipt. | Mark the return **Sent** and store the receipt. |
| `dry_run` | A dry run: the Sender is in dry-run mode, or the return is for a test TIN. Nothing reached BIR. `receipt` is a signed receipt saying `dry_run`. | Treat as a test: show it as a dry run, never as filed. |
| `failed` | Nothing reached BIR. `error` says why (`relay not built in` from a server built without the `bir` feature). | Retry later by opening the relay again (same submission, within 24 h), unless `error` ends with a refusal code in brackets (below). |
| `unknown` | The connection broke during the upload. BIR may have the file. | Do **not** resend automatically. Ask the person to check for BIR's confirmation email. |

A send the server finds stuck in `sending` for 10 minutes (it stopped
mid-send) becomes `unknown` with the error "the server stopped while
sending". For a test TIN's send the error is instead "the server stopped
during this test send; it was a dry run, so nothing was sent to the BIR":
a test TIN's send is a dry run and never reaches the BIR. A send whose
Sender ends after opening a stream but without a result is `unknown` with
"the send ended without an outcome; BIR may have received this return". A
test TIN's send never opens a stream: the server stops one at its first
`open`, as not a dry run ([section 6](#6-the-sender)). Should one still end that way, it is
`failed` with the same words as a test send that is not a dry run, never
words that say nothing reached the BIR.

Fields a `result` may carry besides these, each optional
([bir-uptime.md](bir-uptime.md)):

| Field | |
|---|---|
| `class` | On `failed` and `unknown`: whose side it failed on, `bir`, `user_network` or `ours`. An `ours` failure is not one of the return's tries; an unknown value reads as `bir`. Sent from `BIR_UPTIME=public` (absent for a try that got in, and when the Sender's mode is `lab`, `dry_run` or unknown). |
| `bir` | **Planned, not sent yet.** The sign right after this try ([bir-uptime.md](bir-uptime.md#22-the-sign-object)). |
| `hold` | **Planned, not sent yet.** On a v1 send held for the BIR's state: `{"code":"bir_offline","wait_until":"…"}`. |
| `test_only` | `true` on the result of a test TIN's send ([Test TINs](#test-tins)), v1 and v2, after the other fields; absent for any other send. |

The server checks the account and the TIN again when the socket opens,
after the token check. If the account was banned, or the TIN stopped being
able to send (paused, removed, revoked), since the return was stored, the
return is not sent: the answer is `failed` with `error` set to the
refusal's message and its code in brackets, for example
`"Sending is paused for this TIN. Write to support@buwiz.com. (tin_paused)"`
or `"… (account_banned)"`. Nothing reached BIR. Treat it as the same refusal
as a `403` with that code (section 1, Refusals): back to a draft, no retry.
A v2 return's envelope is dropped with its ticket; a v1 return may be sent
again once the TIN can send.

The Sender checks the return itself again, against the job (v1) or its
ticket (v2), before it opens anything, with the same rules as
`POST /api/submissions`. For v1 the stored file name must name exactly
the job's TIN, and the XML the job's TIN, form and period. A return that
does not match ends `failed` with
`"The return does not match its ticket: … (return_mismatch)"` (or "its
job", for example "the file name's TIN is not the job's TIN"). Nothing
reached BIR, it is never a dry run, and it is final: back to a draft, no
retry. What the message repeats from the return is cut at 40
characters. A send already under way on another
connection keeps its own outcome.

### Test TINs

A test TIN is a TIN whose root is one of the reserved roots, at any
branch: today `000-000-000` and `111-111-114` (`TEST_ROOTS` in
`src/account/tin.rs`, [accounts.md](accounts.md#bans-and-test-accounts)).
People cannot register one; staff add one to a test account. A return
for one goes the whole production route, ticket or v1 return, relay and Sender, with no new request field.
The Sender makes its send a dry run whatever its `BIR_RELAY_MODE`, deciding
from the TIN in the verified ticket (v2) or in the job, once the file name
and the return were checked against it (v1): it hands the BIR code
settings with no dispatcher, upload host, pinned key or lab server, in
every relay mode, so it encrypts the return and stops, with no
dispatcher login and no upload stream. The `result` is `dry_run` with a
receipt whose `mode` and `status` are both `dry_run`, signed with the usual
receipt key (the one in `/.well-known/jwks.json` and the trust list). The
submission ends `dry_run`, as every dry run does, and
`GET /api/submissions/{id}` shows the receipt as for any send. Such a send
has no uptime record and is never held ([bir-uptime.md](bir-uptime.md)).
An app shows it as a dry run (a test send), never as filed.

Opening the relay again for a submission that is already `sent` returns
the same receipt and does not send again. `GET /api/submissions/{id}` and
`GET /api/submissions` show the status at any time.

## 4. Receipts

A receipt is a compact JWS (`alg: EdDSA`, `typ: buwiz-receipt+jwt`) signed
by the Sender. Verify it with the key in `GET /.well-known/jwks.json`,
matched by `kid`.

| Claim | |
|---|---|
| `sub` | Submission id |
| `account_id`, `tin`, `form_type`, `period`, `filename` | What was filed |
| `xml_sha256` | SHA-256 of the plaintext XML the app sent. Compare it with your own. |
| `payload_sha256` | SHA-256 of the encrypted file as uploaded |
| `remote_path` | Path on BIR's server |
| `mode` | `live`, `lab` or `dry_run` (always `dry_run` for a test TIN) |
| `status` | `sent` or `dry_run` (always `dry_run` for a test TIN) |
| `iat` | Time sent (Unix seconds) |
| `iss` | The server |
| `jti` | v2 only: the ticket the send used |

## 5. Version 2: device keys, tickets, sealed returns

### 5.1 Device key

After device-flow sign-in the app makes an Ed25519 key, keeps it in the OS
credential store, and registers its public half:

```http
POST /api/devices
Authorization: Bearer <access token>
buwiz-timestamp: 1767225600
buwiz-nonce: 3q2-7wEAAAAAAAAAAAAAAA
buwiz-signature: <base64url Ed25519 signature, made with the new key>

{"public_key": "<base64url, 32 bytes>", "name": "Juana's laptop",
 "install_id": "<optional, 16 to 64 of A-Z a-z 0-9 - _>", "platform": "macos",
 "model": "Mac14,2", "os_version": "15.1", "computer_name": "Juana's MacBook Air"}
```

Answer `201 {"id","name","created_at"}`. The device belongs to this
sign-in (OAuth grant). A sign-in that carries an install id registers
one device (else `409 device_registered`); one without may register again,
each time another row, as before. The optional fields are trimmed and refused with
control characters: `platform` is one of `ios`, `ipados`, `android`,
`macos`, `windows`, `linux` (anything else is kept as nothing), `model` at
most 80 characters (`bad_model`), `os_version` 40 (`bad_os_version`),
`computer_name` 80 (`bad_computer_name`).

The sign-in before it may be the in-app one (`/auth/email/start`,
`/resend`, `/verify` on api.buwiz.com; [accounts.md](accounts.md#the-api)
has the whole contract). What a client must handle there besides the
answers it already knows: start and resend allow 200 tries an hour per
network (`429 too_many_requests`, `retry_after` in the body and
`Retry-After`); verify allows 300 tries at a code an hour per network
(the same `429 too_many_requests`) and can answer `503 busy`
(`Retry-After: 5`) as start does; and verify takes
`{"request_id", "secret", "code"}`, so a body without `code` is `400
invalid_request`.

With `install_id` ([accounts.md](accounts.md#devices)) an install has one
row per account: a registration from an install that has one updates it
in place (new key, new sign-in, the fields above, `signed_in_at`, not
removed) and answers its same `id` and `created_at`; the sign-in it had
is ended (`device_replaced`). A sign-in that carries an install id (from
`/auth/email/start`, `/oauth/device/code` or `/oauth/authorize`) must register with the same
one (`400 install_mismatch`, also when it is left out); one without gets
the request's, unless that install's live device belongs to another live
sign-in of the account (`409 install_in_use`, nothing changed).
`bad_install_id` for a malformed one. Whenever a sign-in ends, its live
device is removed with it. Without `install_id`
registration is as before.

Replacing an install's row is by design: a sign-in that named the install
when it began and was completed is proof of the account, so it takes the
row whatever sign-in held it. When the sign-in it ends was still live
(not ended, with a refresh token that works), the account is mailed "New
sign-in to eBIRForms on <platform>" (after the registration commits; a
failed mail never fails it), except a test account with a fixed code.

Every v2 request then carries a signature, DPoP-style:

| Header | |
|---|---|
| `buwiz-device` | The device id (absent only when registering) |
| `buwiz-timestamp` | Unix seconds. Refused when more than 60 s off (`clock_skew`). |
| `buwiz-nonce` | 16 to 64 characters of `[A-Za-z0-9_-]`. A nonce the key used before is refused (`nonce_reused`). |
| `buwiz-signature` | base64url Ed25519 signature over the signing input |

The signing input, joined with `\n`:

```text
buwiz-request-v1
<METHOD>
<path and query, e.g. /api/tickets>
<lowercase hex SHA-256 of the body; of the empty string for a GET>
<timestamp>
<nonce>
```

Errors: `401 signature_required`, `bad_signature`, `bad_nonce`,
`clock_skew`, `nonce_reused`, `unknown_device`; `403 device_revoked`.

`GET /api/devices` lists the account's devices, and in `pending` its BIR
app sign-ins that have not registered one yet (the app's "Add this iPhone
to your Buwiz account?" was not tapped):

```json
{"devices": [{"id": "<uuid>", "name": "Juana's phone", "client_id": "buwiz-desktop",
              "current": true, "platform": "ios", "model": "iPhone16,1",
              "model_name": "iPhone 15 Pro", "os_version": "18.1", "computer_name": null,
              "created_at": "…", "signed_in_at": "…", "last_used_at": "…", "revoked_at": null,
              "state": "live"}],
 "pending": [{"grant_id": "<uuid>", "client_id": "buwiz-desktop", "platform": "android",
              "signed_in_at": "…", "current": false}]}
```

`current` marks the caller's own sign-in. `state` is `live` (its sign-in
has a refresh token that works), `expired` (its sign-in's tokens all ran
out, though nobody removed it: shown as "Signed out (expired)") or
`removed`. `model_name` is the name an
Apple model code is sold under (`src/filing/apple_models.rs`); an Apple
code not listed there is the platform's word with the code ("iPhone
(iPhone18,1)", "iPad (iPad15,3)", "Mac (Mac16,12)"); any other model is as
sent. `model` is always the raw value. Devices removed more than 30 days ago are left out.
`POST /api/devices/{id}/revoke` revokes one device (`{"revoked": true}`;
one already signed out is marked removed the same way, and one already
removed is `404`); `POST
/api/sign-ins/{grant_id}/sign-out` ends one of the account's BIR app
sign-ins (its device too, if it has one), this app's own included,
answering `{"signed_out": true}` or `404` for another account's or an
unknown one. Both work for a banned account, as signing out does. The
account site lists the same two lists under **My devices**, with
**Remove** and **Log out**. A revoked device is refused everywhere: its signatures
fail, its sign-in is ended (tokens stop working), and a submission it
sealed can no longer be relayed.

### 5.2 Ticket

```http
POST /api/tickets            (signed)

{"tin": "000-123-453-00000", "form": "1701Qv2018", "period": "2026Q1",
 "payload_sha256": "<hex SHA-256 of the plaintext XML>",
 "email": "juana@example.com", "idempotency_key": "<1-100 chars>"}
```

`form`, `period` and `email` are the parts of the eBIRForms file name
`{TIN}-{form}-{period}#{email}#.xml`; the email is where BIR sends its
confirmation (the person's choice). Checks: the TIN can send on the
account (`403` with a code from section 1's Refusals), the device is live,
quotas (30 tickets an
hour per account and per device, 50 a day per TIN: `429 quota_exceeded`),
and idempotency:

- the same key for a different return: `409 idempotency_mismatch`;
- the same key while its ticket is live and unused: the same ticket again;
- the same key after a send that may have reached BIR (`sending`, `sent`,
  `dry_run`, `unknown`): `409 already_submitted`;
- after a send that `failed` or `expired`: a new ticket.

**Planned, not built yet**
([bir-uptime.md](bir-uptime.md#6-turns-queue-reports-and-scouts)): two
optional request fields, `"bir_turn": "t_…"` to spend a turn the server
gave in a queue report and `"bir_try_anyway": true` for a person's own Try
anyway (never an agent's); `"admission"` in the answer; and, under
`BIR_UPTIME=enforce`, tickets held with `503 bir_offline`, `bir_busy` or
`relay_paused`, or refused with `429 try_anyway_limited`. Today the server
ignores both fields, answers no `admission` and holds no ticket at any
`BIR_UPTIME` level.

Answer `201 {"ticket","jti","expires_at"}`. The ticket is a compact JWS,
`alg: EdDSA`, `typ: buwiz-ticket+jwt`, signed by the Gatekeeper and valid
for five minutes:

| Claim | |
|---|---|
| `iss`, `aud` | The Gatekeeper; `buwiz-sender` |
| `sub`, `account_id` | The account |
| `device_id` | The device that asked |
| `tin` | 14 digits |
| `form`, `period`, `email` | As asked |
| `payload_sha256` | As asked |
| `jti` | The ticket's id |
| `iat`, `exp` | Issued, expires (Unix seconds) |

### 5.3 Seal and submit

`GET /.well-known/buwiz-hpke.json` publishes the Sender's key:

```json
{"keys": [{"kid": "…", "kem": "DHKEM(X25519, HKDF-SHA256)", "kdf": "HKDF-SHA256",
           "aead": "ChaCha20Poly1305", "public_key": "<base64url, 32 bytes>"}]}
```

The app seals the plaintext XML with HPKE (RFC 9180) base mode, that suite,
`info = "buwiz-submission-v1:" + jti` and empty `aad`, and posts:

```http
POST /api/v2/submissions     (signed)

{"ticket": "<ticket>",
 "envelope": {"kid": "…", "enc": "<base64url encapsulated key>", "ct": "<base64url ciphertext>"}}
```

The Gatekeeper checks the ticket's signature and expiry, that it was
issued to this account and device, and spends it: a second use is
`409 ticket_used`, an expired one `403 ticket_expired`, a forged one
`403 ticket_invalid`, another device's `403 ticket_mismatch`. If the TIN
stopped being able to send since the ticket (paused, ended), it answers
with that refusal (section 1) and the ticket is not spent. It stores the
envelope as it came and answers `201` with the submission (`relay_path`
included).

### 5.4 Relay

Before it asks for a ticket, a v2 app fetches the **relay trust list**,
`GET /.well-known/buwiz-trust.json`: a compact JWS (`alg: EdDSA`,
`typ: buwiz-trust+jwt`, `kid`) over

```json
{"hosts": ["host:port", …],
 "sealing_keys": [<entries of /.well-known/buwiz-hpke.json "keys">],
 "receipt_keys": [<Ed25519 JWKs of the Sender's receipt keys>],
 "iat": 1767225600, "exp": 1798761600}
```

It is signed **offline** with the trust key (`buwiz-trust-sign`, see
[deploy.md](deploy.md)); this server never holds that key and only serves
the blob in `TRUST_LIST_JWS` (`503 trust_list_missing` without one). The
app pins the trust public key and checks signature, `typ` and `exp`; no
valid list means no send. Then:

- the pipe may open exactly the listed `host:port` pairs; anything else is
  `refused`;
- the app seals only to the sealing key `/.well-known/buwiz-hpke.json`
  names as current **if** the list carries that `kid` with the same key;
  otherwise it seals nothing and asks for no ticket;
- receipts are verified only against the listed receipt keys.

So a compromised Gatekeeper can't widen where an app connects, can't make
it seal to a key of its own (and read the return), and can't forge "Sent".
`/.well-known/buwiz-hpke.json` and `/.well-known/jwks.json` stay, unsigned,
for v1 clients and to name the current key; v2 apps trust neither alone.

Then it opens `relay_path` as in section 2, with the bearer token **and** a
device signature over `GET` and the path. The frames are those of section 2.

For a v2 submission the Sender, before anything reaches the network:

1. verifies the ticket (Gatekeeper's key, `typ`, `aud`, expiry) and that
   it has not used it before;
2. opens the envelope with `info` bound to the ticket's `jti`;
3. checks SHA-256 of the XML against the ticket;
4. checks the XML's own TIN, form and year (and quarter, when ticked)
   against the ticket;
5. sends it under the file name built from the ticket.

Any failure ends in `result` `failed` with the reason. A v2 submission gets
one attempt: after `failed`, ask for a new ticket and send again. `unknown`
and `sent` mean as in section 3. The receipt carries the ticket's `jti`.

## 6. The Sender

The Gatekeeper reaches the Sender at `SENDER_URL` and presents
`Authorization: Bearer $SENDER_SECRET`:

- `GET /internal/public`: `{"receipt_keys":[JWK],"hpke":{…}}` (also what
  `buwiz-sender public` prints, for `buwiz-trust-sign`), which the
  Gatekeeper serves as `/.well-known/jwks.json` and
  `/.well-known/buwiz-hpke.json`.
- `GET /internal/relay` (WebSocket): the Gatekeeper sends one job as a text
  frame, `{"kind":"v1",…}` with the XML it kept or `{"kind":"v2",…}` with the
  ticket and envelope, then forwards the app's frames both ways until the
  Sender's `result`, which it records. If the app goes away, the Gatekeeper
  closes the Sender's streams and waits up to 60 s for its outcome.
- The Sender's first text frame on that socket is
  `{"type":"sender","mode":"live"}` (`live`, `lab` or `dry_run`: the mode
  it sends this job in, its `BIR_RELAY_MODE`, or `dry_run` for a test
  TIN's job), before any `open`. The Gatekeeper reads the mode for
  its record of the try ([bir-uptime.md](bir-uptime.md#14-the-per-try-record))
  and never forwards this frame to the app, whatever its other fields
  hold. The two processes ship from one build and restart together; a try
  without the frame is recorded with mode `unknown`, like a lab try: no
  class, never counted, nothing told to the app.
- `GET /internal/relay/reserved` (WebSocket): the same, for a test TIN's
  job (its root is reserved, `TEST_ROOTS`) and only for that. The
  Gatekeeper sends a test TIN's job only here. A Sender from before
  reserved roots has no such route, so the Gatekeeper cannot connect, the
  job never leaves it, and the try ends "the sending service is not
  reachable; try again later". The Sender refuses a test TIN's job on
  `/internal/relay`, and any other job here: after its `sender` frame it
  ends with a `failed` result and sends nothing. On either route the
  `sender` frame says `dry_run` only for a job that reads as a test TIN's.
  That includes a v2 job whose ticket does not check but names a test TIN
  (read unverified, for the frame only), so such a job ends with the
  ticket's own reason. Any other job's frame says the Sender's mode.
- For a test TIN's job the Gatekeeper also checks the Sender, by an
  allow-list. Its first text frame must be the `sender` frame saying
  `dry_run`; the only frame after it must be the `result`, `dry_run` or
  `failed`. A `dry_run` result must carry a receipt whose `mode` and
  `status` both say `dry_run`; a receipt on a `failed` one must say mode
  `dry_run`. The app hears that
  result as the Gatekeeper writes it, not as it came. Any other frame (an
  `open`, binary, any other type, a frame with two `type` keys) ends it:
  the Gatekeeper closes the Sender's socket, forwards nothing of it to the
  app, and ends the try `failed`
  with "the sending service would not make this test send a dry run; try
  again later" (v1), or "...; ask for a new ticket and send again later"
  (v2, whose ticket is spent). An older Sender that knows fewer reserved
  roots is stopped this way too.

Without `SENDER_URL`, a development server runs a Sender in its own
process. A public deployment (`https://` `ACCOUNT_ORIGIN`) refuses to start
without `SENDER_URL`.

## 7. Server settings

Gatekeeper (`buwiz-cloud`):

| Variable | |
|---|---|
| `TICKET_SIGNING_KEY` | Base64 32-byte Ed25519 seed that signs tickets. Required on public deployments. |
| `SENDER_URL` | `http(s)://host:port` of the Sender on the private network. Required on public deployments. |
| `SENDER_SECRET` | Shared with the Sender, 32+ characters |
| `TRUST_LIST_JWS` | The offline-signed relay trust list served at `/.well-known/buwiz-trust.json` |
| `BIR_UPTIME`, `BIR_UPTIME_UPLOAD_PORTS` | The BIR uptime record and sign: [bir-uptime.md](bir-uptime.md#13-server-settings) |

Sender (`buwiz-sender`):

| Variable | |
|---|---|
| `SENDER_HOST`, `SENDER_PORT` | Where it listens (`127.0.0.1:3009`) |
| `SENDER_SECRET` | As above |
| `TICKET_PUBLIC_KEYS` | Comma-separated base64url Ed25519 public keys of the Gatekeeper's ticket keys |
| `SENDER_HPKE_KEY` | Base64 32-byte X25519 private key returns are sealed to (lab, live) |
| `RECEIPT_SIGNING_KEY` | Base64 32-byte Ed25519 seed that signs receipts (lab, live) |
| `RECEIPT_ISSUER` | `iss` of receipts (default `ACCOUNT_ORIGIN`) |
| `BIR_RELAY_MODE` | `dry_run` (default), `lab` or `live` |
| `BIR_IAF_PASSPHRASE` | IAF encryption passphrase (lab, live) |
| `BIR_DISPATCHER_PASSPHRASE` | Unwraps dispatcher answers (live) |
| `BIR_SSH_HOST_KEYS` | Comma-separated `SHA256:…` fingerprints accepted for upload servers (lab, live) |
| `BIR_SFTP_HOSTS` | Comma-separated upload hosts the dispatcher may name. No default; required in live, where the Sender refuses to start without it. |
| `BIR_LAB_SFTP` | `user:password@host:port` of a test SFTP server (lab) |

The ticket public key is the Ed25519 public half of `TICKET_SIGNING_KEY`,
base64url without padding.

On EC2 each process reads its own file: the Sender's holds what it reads,
the Gatekeeper's every other setting, and both have `SENDER_SECRET`,
`ACCOUNT_ORIGIN` and `RUST_LOG`. The Sender runs as its own user ([deploy.md](deploy.md#its-own-user-and-settings-file)).

The `BIR_*` settings other than `BIR_RELAY_MODE` take effect only in a
build with the `bir` feature. The values for live (the upload hosts) are
in the BIR relay's private docs.

Upload-server host keys come only from real sends; nothing in Buwiz scans
or probes the BIR's servers for one (no `ssh-keyscan`). The SSH server
offers its key in the handshake, before encryption starts, so the
Gatekeeper reads it from the bytes it bridges and records it in
`bir_host_keys` ([bir-uptime.md](bir-uptime.md#143-the-table)), with the
day keys of the account and network that saw it, their key period, and
whether the Sender went on to complete the key exchange in that try. When
BIR rotates its key, uploads stop with "host key is not pinned" (the
Sender's log shows the fingerprint too) and the new key shows up there.

**A sighting is information only, never grounds to pin a key.** The bytes
it is read from are relayed by the user's own app, so any app can forge
one, and the day keys of different key periods cannot be compared, so
sightings are counted only within one key period. To pin a new key an
Admin needs a second source the owner trusts that does not pass through
the relay: for example the key the official eBIRForms client ships, or a
BIR notice. A key is never obtained by connecting to the BIR from a Buwiz
machine.
