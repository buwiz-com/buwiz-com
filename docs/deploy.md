# Deploy

Production runs on AWS EC2, behind a Cloudflare Tunnel. It moved there from
Fly on 2026-10-30, and Fly is retired.

- **The machine:** one arm64 instance, a t4g.micro on Ubuntu 24.04, in
  `ap-southeast-1`. It runs three systemd services:
  - `buwiz.service`, the Gatekeeper, on 127.0.0.1:8080;
  - `buwiz-sender.service`, the Sender, on 127.0.0.1:3009;
  - `cloudflared-buwiz.service`, the tunnel.
- **Public hosts:** `buwiz.com`, `www.`, `account.`, `api.` and `ebirforms.`
  are proxied CNAMEs to the tunnel. Nothing on the instance listens publicly.
- **Database:** Postgres on Neon (Singapore, `aws-ap-southeast-1`).
  Migrations run at start.
- **Settings:** in SSM Parameter Store under `/buwiz/*`. They are written to
  `/etc/buwiz/env` at each start.

## Secrets

Keep a copy of every secret in a password manager. **If `DOCUMENT_KEY` is
lost, every stored claim document becomes unreadable.**

| Secret | |
|---|---|
| `DATABASE_URL` | Neon connection string (`sslmode=require`) |
| `DOCUMENT_KEY` | `openssl rand -base64 32` |
| `TICKET_SIGNING_KEY` | `openssl rand -base64 32` |
| `SENDER_URL`, `SENDER_SECRET` | The Sender's private address (`http://127.0.0.1:3009`) and a shared secret, `openssl rand -base64 32` |
| `RESEND_API_KEY`, `MAIL_FROM` | Resend, with buwiz.com verified as the sending domain |
| `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY` | An R2 API token, "Object Read & Write", limited to the `buwiz` and `assets` buckets |

Claim documents live in the R2 bucket `buwiz` as `claims/{claim}/{document}`.
They are encrypted with `DOCUMENT_KEY` before upload. They are deleted when
the claim is decided or withdrawn.

## Releasing a build

The release is built on the owner's Mac, from a checkout that has the
private BIR files restored (`src/bir/` and the rest; see
[relay.md](relay.md)). The `Dockerfile` builds with `--features bir,ocr`.
Without those files, the build fails, so it never ships a Sender that
cannot send.

1. `docker build --platform linux/arm64`. The binaries `buwiz-cloud` and
   `buwiz-sender` and the OCR models are copied out of the image.
2. The binaries, the models and `install.sh` are uploaded to the deploy
   bucket under `releases/<sha>/`.
3. SSM `send-command` runs `install.sh <sha>` on the instance. It swaps the
   binaries and restarts the services.

Smoke-test after each deploy:
- `/healthz` on each host;
- `buwiz.com/` and `/mini-apps`;
- `ebirforms.buwiz.com/download`;
- `account.buwiz.com/login`.

The build and deploy scripts are kept outside this repository for now.
Committing them as `deploy/ec2/` is an open owner decision.

## OCR of claim documents

The image is built with `--features ocr`: the TIN ID or 2303 of each claim
is read in the Gatekeeper's process, one document at a time, at most two
minutes each. The models are in the image at
`/usr/local/share/buwiz/ocr-models` (`OCR_MODEL_DIR`). Running the binary
outside the image (systemd on EC2), copy that directory next to it and set
`OCR_MODEL_DIR` in the unit, or run `scripts/ocr-models.sh /some/dir`.
The binary is built on Ubuntu 24.04 and needs its libstdc++ (GCC 14), so
the host runs Ubuntu 24.04 or newer. A
missing model shows staff "Couldn't read: the OCR models are missing on the
server" and logs which file; nothing else breaks.

Once it has read a few documents, OCR holds about 350 MB (models, ONNX
Runtime's buffers), on top of the server's own memory; it levels off there
rather than growing. A document takes 1 to 2 seconds on an M-series Mac and
should take 3 to 6 on a t4g.micro, which uses one of its two cores for it.
Move to a t4g.small (2 GB) when the server's own memory passes about
350 MB, when the host starts to swap, or when claims queue faster than one
every few seconds.

## The Sender

The Sender runs on the same instance as `buwiz-sender.service`. It listens
on 127.0.0.1:3009 only, and the Gatekeeper reaches it there
(`SENDER_URL`). Only the Sender gets the BIR secrets:

| Secret (`buwiz-sender`) | |
|---|---|
| `SENDER_SECRET` | Same as the Gatekeeper's |
| `TICKET_PUBLIC_KEYS` | Public half of `TICKET_SIGNING_KEY`, base64url |
| `SENDER_HPKE_KEY` | `openssl rand -base64 32` (X25519 private key) |
| `RECEIPT_SIGNING_KEY` | `openssl rand -base64 32` (moves here from the Gatekeeper; keep the old value so old receipts still verify) |
| `BIR_RELAY_MODE`, `BIR_IAF_PASSPHRASE`, `BIR_DISPATCHER_PASSPHRASE`, `BIR_SSH_HOST_KEYS`, `BIR_SFTP_HOSTS` | See [relay-protocol.md](relay-protocol.md) §7; the live values are in the BIR relay's private docs |

The Gatekeeper refuses to start on a public origin without `SENDER_URL`
and `TICKET_SIGNING_KEY`. See [relay-protocol.md](relay-protocol.md) §6.

## The relay trust list (human step)

Apps trust three things only through a list signed offline with a key this
server never holds: the hosts the relay pipe may open, the Sender's HPKE
sealing key, and the Sender's receipt key. Make the key and sign the list on
a trusted machine, not on the server:

```sh
cargo run --release --bin buwiz-trust-sign -- keygen trust.key
# prints the public key: put it in eBIRForms crates/bir-submit/src/trust.rs TRUST_KEYS
# on the instance (through SSM): buwiz-sender public > sender-keys.json
cargo run --release --bin buwiz-trust-sign -- sign trust.key 365 sender-keys.json \
  <host:port> …   # BIR's dispatcher and upload hosts, from the BIR relay's private docs
# prints the JWS: store it as TRUST_LIST_JWS in /buwiz/* and restart buwiz.service
```

The `host:port` list is every host the Sender opens through the pipe: BIR's
dispatcher (HTTPS and HTTP, primary and backup) and its SFTP upload server.

Keep `trust.key` offline (password manager or hardware). The list expires
(here after 365 days); sign a new one before then, or apps stop sending.

Rotation, in this order, so no app ever meets a key its list lacks:

1. **Sender keys** (`SENDER_HPKE_KEY`, `RECEIPT_SIGNING_KEY`): print the new
   Sender's public keys, sign a list with both the old and the new
   (`sender-keys.json` may be a JSON array of both), deploy that list,
   *then* switch the Sender to the new keys. Drop the old keys from the list
   at the next signing, after old receipts no longer need checking.
2. **The trust key**: pin the new public key next to the old in an app
   release first, then sign with the new key.

## Downloads

The app's installers are in R2 bucket `buwiz-releases`, public at
`https://downloads.buwiz.com` (`DOWNLOADS_URL`, the default). Each app has
its own folder; the eBIRForms release CI uploads each version as:

- `ebirforms/releases/v{VERSION}/{file}`: the installers, never changed once
  uploaded.
- `ebirforms/releases/v{VERSION}/manifest.json`: the version, `published_at`
  and, for each file, its platform (`macos`, `windows`, `linux`, `android`),
  arch (`arm64`, `x64`, `universal`), kind (`dmg`, `msi`, `exe`, `appimage`,
  `deb`, `rpm`, `targz`, `sh`, `apk`), file name, size and SHA-256.

Nothing is released until `ebirforms/latest.json` exists. To release a
version, copy its `manifest.json` there (with `Cache-Control: max-age=60`);
to take it back, delete it. While there is no `ebirforms/latest.json`, the
server reads the root `latest.json` instead, whose files are under the root
`releases/v{VERSION}/` (where releases went before the per-app folders). A
broken or unreachable `ebirforms/latest.json` does not fall back; see below.
The server reads them at most once a minute, so a change shows within about
a minute. While neither exists, `/download` and the home page say "Coming
soon". Once one exists, `/download` lists every file with its size and
SHA-256 and joins the sitemap, the home page has one Download button for
the visitor's device, and `/download/{mac,windows,linux,android}`
(`?kind=deb` and so on for a particular file) redirects, uncached, to the
installer: by default the Mac's universal DMG, the Windows x64 EXE, the
Linux x64 DEB, and the Android APK. A platform with no file says "Coming
soon". Each download is counted per app, version, file and Manila day in
`download_counts` (`app` is `ebirforms`).

If the bucket can't be reached or `latest.json` is broken, the server keeps
the last good one, or says "Coming soon" if it never had one. File names
must be plain names (letters, digits, `.`, `_`, `-`, `+`); files with other
names, and platforms or kinds the server doesn't know, are left out.

## Domains

`buwiz.com`, `www.`, `account.`, `api.` and `ebirforms.buwiz.com` are
proxied CNAMEs in Cloudflare that point to the tunnel. Cloudflare issues the
certificates, and Always Use HTTPS is on. The app redirects any `www.` host
to the bare domain with a permanent redirect. The MX, SPF, DKIM, DMARC and
site-verification records on `buwiz.com` belong to email. Do not change
them.

## Switch the mini app

```sh
BUWIZ_SERVER_URL=https://api.buwiz.com BUWIZ_ACCOUNT_URL=https://account.buwiz.com
```

Sign in from the app, add your TIN at `/tins/new`, approve it as
`ADMIN_EMAILS` at `/staff/claims`, and the licence gate opens.
