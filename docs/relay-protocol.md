# Submission relay protocol

How a Buwiz app files a return with BIR through buwiz-cloud. BIR's
passphrases and SFTP login stay on the server. The traffic leaves from the
taxpayer's computer, so BIR sees the taxpayer's own connection, as with
eBIRForms.

`src/relay/client.rs` and `src/bin/buwiz-relay.rs` are the reference
client.

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
  Hard-code this list and refuse anything else with `refused`. This keeps
  the server from using a taxpayer's computer as a general proxy.
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

## Receipts

A receipt is a compact JWS (`alg: EdDSA`, `typ: buwiz-receipt+jwt`).
Verify it with the key in `GET /.well-known/jwks.json`, matched by `kid`.

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

## Server settings

| Variable | |
|---|---|
| `BIR_RELAY_MODE` | `dry_run` (default), `lab` or `live` |
| `BIR_IAF_PASSPHRASE` | IAF encryption passphrase (lab, live) |
| `BIR_DISPATCHER_PASSPHRASE` | Unwraps dispatcher answers (live) |
| `BIR_SSH_HOST_KEYS` | Comma-separated `SHA256:…` fingerprints accepted for upload servers (lab, live) |
| `BIR_SFTP_HOSTS` | Upload hosts the dispatcher may name (default `ebf2.bir.gov.ph`) |
| `BIR_LAB_SFTP` | `user:password@host:port` of a test SFTP server (lab) |
| `RECEIPT_SIGNING_KEY` | Base64 32-byte Ed25519 seed. Required on public deployments. |

Get the upload server's fingerprint from a trusted network and check it
against a second source before pinning it:
`ssh-keyscan -t ed25519 ebf2.bir.gov.ph | ssh-keygen -lf -`. When BIR
rotates its key, uploads stop with "host key is not pinned" and the log
shows the new fingerprint. Verify it before adding it.
