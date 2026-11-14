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

eBIRForms `bir-submit` (eBIRForms/eBIRForms) is the v2 client. The reference v1 client
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
| `tin_test_only` | Active, but a test TIN (root `000-000-000`, which staff add to test accounts only): it never sends, in any relay mode, and a paused one answers this code too | This is a test TIN: you can prepare returns for it, but Buwiz never sends them to the BIR. |
| `tin_not_active` | Anything else | this TIN is not active on your account |
| `account_banned` | The account is banned (every bearer call but `POST /api/auth/logout`; the token refresh still works) | This Buwiz account is banned: it can't send returns. Write to support@buwiz.com. |

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
ended, each with `status` (`approved`, `paused`, `test_only`,
`unverified`, `pending`, `needs_fix`), `can_send`, a `message`, and an
`action` link to Verify or Fix. A test TIN's row also has
`"test_only": true` (no other row has the field). `status` is `approved` exactly when `can_send` is true; both come from
the same rule as the refusals above. A `needs_fix` row also carries
`reasons`, `reason_codes` and the staff `note` (at most 200 characters, BIR
app only). Only the BIR app (`app` scope) gets `tin`, filing permissions,
the link and the note. Without `all`, the list means what it always did,
except that a paused TIN and a test TIN are left out. `tests/fixtures/tax_profiles_all.json`
is the golden example; [accounts.md](accounts.md#licence-gate) has more.

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
| `dry_run` | The server is in dry-run mode. Nothing reached BIR. | Treat as a test. |
| `failed` | Nothing reached BIR. `error` says why (`relay not built in` from a server built without the `bir` feature). | Retry later by opening the relay again (same submission, within 24 h), unless `error` ends with a refusal code in brackets (below). |
| `unknown` | The connection broke during the upload. BIR may have the file. | Do **not** resend automatically. Ask the person to check for BIR's confirmation email. |

Fields a `result` may carry besides these, each optional
([bir-uptime.md](bir-uptime.md)):

| Field | |
|---|---|
| `class` | On `failed` and `unknown`: whose side it failed on, `bir`, `user_network` or `ours`. An `ours` failure is not one of the return's tries; an unknown value reads as `bir`. Sent from `BIR_UPTIME=public`. |
| `bir` | The sign right after this try was recorded ([bir-uptime.md](bir-uptime.md#22-the-sign-object)). |
| `hold` | On a v1 send held for the BIR's state: `{"code":"bir_offline","wait_until":"…"}`. |

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

The Sender checks the return itself again, against its file name (v1) or
its ticket (v2), before it opens anything, with the same rules as
`POST /api/submissions`. A return that does not match ends `failed` with
`"The return does not match its ticket: … (return_mismatch)"` (or "its
file name"). Nothing reached BIR, and it is final: back to a draft, no
retry. What the message repeats from the return is cut at 40
characters. A send already under way on another
connection keeps its own outcome.

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
| `mode` | `live`, `lab` or `dry_run` |
| `status` | `sent` or `dry_run` |
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

{"public_key": "<base64url, 32 bytes>", "name": "Juana's laptop"}
```

Answer `201 {"id","name","created_at"}`. The device belongs to this
sign-in (OAuth grant).

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

`GET /api/devices` lists the account's devices; `POST /api/devices/{id}/revoke`
revokes one. The account site lists them under **My devices** with a
**Remove** button. A revoked device is refused everywhere: its signatures
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

Two optional request fields come with the BIR uptime sign
([bir-uptime.md](bir-uptime.md#6-turns-queue-reports-and-scouts)):
`"bir_turn": "t_…"` spends a turn the server gave in a queue report, and
`"bir_try_anyway": true` is a person's own Try anyway (never an agent's).
Under `BIR_UPTIME=enforce` a ticket may be held with `503 bir_offline`,
`bir_busy` or `relay_paused`, or refused with `429 try_anyway_limited`.

Answer `201 {"ticket","jti","expires_at"}` (and, from `public`,
`"admission"`). The ticket is a compact JWS,
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
  `{"type":"sender","mode":"live"}` (`live`, `lab` or `dry_run`, its
  `BIR_RELAY_MODE`), before any `open`. The Gatekeeper reads the mode for
  its record of the try ([bir-uptime.md](bir-uptime.md#14-the-per-try-record))
  and never forwards this frame to the app. The two processes ship from one
  build and restart together; a try without the frame is recorded with
  mode `unknown`.

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

The `BIR_*` settings other than `BIR_RELAY_MODE` take effect only in a
build with the `bir` feature. The values for live (the upload hosts) are
in the BIR relay's private docs.

Upload-server host keys come only from real sends; nothing in Buwiz scans
or probes the BIR's servers for one (no `ssh-keyscan`). The SSH server
offers its key in the handshake, before encryption starts, so the
Gatekeeper reads it from the bytes it bridges and records it, with the
day keys of the account and network that saw it, in `bir_host_keys`
([bir-uptime.md](bir-uptime.md#143-the-table)). When BIR rotates its key,
uploads stop with "host key is not pinned" (the Sender's log shows the
fingerprint too) and the new key shows up there as a candidate. Pin one
only once it has been seen from 3 accounts on 3 networks and an Admin has
confirmed it.
