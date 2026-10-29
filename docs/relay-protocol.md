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

`src/relay/client.rs` and `src/bin/buwiz-relay.rs` are the reference v1
client; buwiz-oss `bir-submit` is the v2 client.

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
- The TIN must be active (an approved claim) on the signed-in account.
  Otherwise the answer is `403 {"error":{"code":"tin_not_active"}}`.
- `xml` is the plaintext IAF file, at most 2 MB. The server does the IAF
  encryption.
- Sending the same `idempotency_key` again returns the existing submission
  without creating another.

Answer `201`:

```json
{"id": "…", "status": "ready", "relay_path": "/api/submissions/…/relay", …}
```

The server keeps the XML, encrypted, until it is sent or 24 hours pass.

## 2. Open the pipe

Open a WebSocket to `relay_path` on the same server (`wss://`), with the
same `Authorization: Bearer` header. The app then forwards bytes:

| Frame | Direction | Meaning |
|---|---|---|
| text `{"type":"open","stream":N,"host":H,"port":P}` | server → app | Connect to `H:P` for stream `N`. |
| text `{"type":"opened","stream":N}` | app → server | Connected. |
| text `{"type":"refused","stream":N,"reason":"…"}` | app → server | Not connected. |
| binary `[N][bytes…]` | both | Bytes on stream `N`. The first byte is the stream id. |
| text `{"type":"close","stream":N}` | both | The sender will write no more on `N`. Shut down the write side. |
| text `{"type":"result","status":S,"error":E,"receipt":R}` | server → app | Final. The server closes the socket after it. |

The app must:

- **Connect only to BIR's hosts**: `birgovph.com:443`, `birgovph.com:80`,
  `ws2.birgovph.com:443`, `ws2.birgovph.com:80` and `ebf2.bir.gov.ph:22`.
  The v1 reference client hard-codes this list and refuses anything else
  with `refused`. This keeps the server from using a taxpayer's computer as
  a general proxy. v2 apps take the list from the signed egress list
  instead (section 5.4) and name no host in their source.
- Forward bytes unchanged. Never terminate TLS or SSH. Everything the app
  sees is TLS or SSH ciphertext, or the dispatcher's wrapped fields.
- Keep the socket open until `result`.

What the server does over the pipe:

1. Asks BIR's dispatcher (`/tinDispatcherSFTP.php`), trying HTTPS first,
   then HTTP, on `birgovph.com`, then `ws2.birgovph.com`.
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
| `failed` | Nothing reached BIR. `error` says why. | Retry later by opening the relay again (same submission, within 24 h). |
| `unknown` | The connection broke during the upload. BIR may have the file. | Do **not** resend automatically. Ask the person to check for BIR's confirmation email. |

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
confirmation (the person's choice). Checks: the TIN is active on the
account (`403 tin_not_active`), the device is live, quotas (30 tickets an
hour per account and per device, 50 a day per TIN: `429 quota_exceeded`),
and idempotency:

- the same key for a different return: `409 idempotency_mismatch`;
- the same key while its ticket is live and unused: the same ticket again;
- the same key after a send that may have reached BIR (`sending`, `sent`,
  `dry_run`, `unknown`): `409 already_submitted`;
- after a send that `failed` or `expired`: a new ticket.

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
`403 ticket_invalid`, another device's `403 ticket_mismatch`. It stores the
envelope as it came and answers `201` with the submission (`relay_path`
included).

### 5.4 Relay

Before it opens the pipe, the app fetches `GET /.well-known/buwiz-egress.json`:
a compact JWS (`alg: EdDSA`, `typ: buwiz-egress+jwt`, `kid`) over

```json
{"hosts": ["host:port", …], "iat": 1767225600, "exp": 1798761600}
```

It is signed **offline** with the egress key (`buwiz-egress-sign`, see
[deploy.md](deploy.md)); this server never holds that key and only serves
the blob in `EGRESS_LIST_JWS` (`503 egress_list_missing` without one). The
app pins the egress public key, checks signature, `typ` and `exp`, and lets
the pipe open exactly those `host:port` pairs; anything else is `refused`.
No valid list means no send, so even a compromised server can't widen
where an app connects.

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

- `GET /internal/public`: `{"receipt_keys":[JWK],"hpke":{…}}`, which the
  Gatekeeper serves as `/.well-known/jwks.json` and
  `/.well-known/buwiz-hpke.json`.
- `GET /internal/relay` (WebSocket): the Gatekeeper sends one job as a text
  frame, `{"kind":"v1",…}` with the XML it kept or `{"kind":"v2",…}` with the
  ticket and envelope, then forwards the app's frames both ways until the
  Sender's `result`, which it records. If the app goes away, the Gatekeeper
  closes the Sender's streams and waits up to 60 s for its outcome.

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
| `EGRESS_LIST_JWS` | The offline-signed egress list served at `/.well-known/buwiz-egress.json` |

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
| `BIR_SFTP_HOSTS` | Upload hosts the dispatcher may name (default `ebf2.bir.gov.ph`) |
| `BIR_LAB_SFTP` | `user:password@host:port` of a test SFTP server (lab) |

The ticket public key is the Ed25519 public half of `TICKET_SIGNING_KEY`,
base64url without padding.

Get the upload server's fingerprint from a trusted network and check it
against a second source before pinning it:
`ssh-keyscan -t ed25519 ebf2.bir.gov.ph | ssh-keygen -lf -`. When BIR
rotates its key, uploads stop with "host key is not pinned" and the log
shows the new fingerprint. Verify it before adding it.
