# Deploy (Fly.io)

App `buwiz-cloud` in Singapore (`sin`). Postgres is on Neon (Singapore,
`aws-ap-southeast-1`).

## Secrets

Set once with `fly secrets import -a buwiz-cloud < file`. Keep a copy in a
password manager. **Losing `DOCUMENT_KEY` makes every stored claim
document unreadable.**

| Secret | |
|---|---|
| `DATABASE_URL` | Neon connection string (`sslmode=require`) |
| `DOCUMENT_KEY` | `openssl rand -base64 32` |
| `TICKET_SIGNING_KEY` | `openssl rand -base64 32` |
| `SENDER_URL`, `SENDER_SECRET` | The Sender's private address (below) and a shared secret, `openssl rand -base64 32` |
| `RESEND_API_KEY`, `MAIL_FROM` | Resend, with buwiz.com verified as the sending domain |
| `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY` | R2 API token, "Object Read & Write", limited to the `buwiz` and `assets` buckets |

Claim documents live in R2 bucket `buwiz` as `claims/{claim}/{document}`. They
are encrypted with `DOCUMENT_KEY` before upload, and deleted when the claim is
decided or withdrawn.

Settings that aren't secret are in `fly.toml` under `[env]`.

## Deploy

```sh
fly deploy            # builds the Dockerfile remotely; migrations run at start
fly logs              # look for "listening on http://0.0.0.0:8080"
curl https://buwiz-cloud.fly.dev/healthz
```

The `Dockerfile` builds with `--features bir`, so deploy from a checkout
that has the private BIR relay files restored (`src/bir/` and the rest; see
the README). Without them the build fails rather than shipping a Sender
that cannot send.

If Fly's remote builder fails (503, `failed_precondition`), build on your
machine instead: `fly deploy --local-only`. To change only settings, redeploy
the last image: `fly deploy --image registry.fly.io/buwiz-cloud:<tag>`.

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
every few seconds. Fly's machine has 512 MB: give it 1 GB while it still
runs this image.

## The Sender

Submission runs in a second app, `buwiz-sender`, from the same image with
`CMD ["buwiz-sender"]`, no public IP and no `[http_service]`; the Gatekeeper
reaches it over Fly's private network (`SENDER_URL=http://buwiz-sender.internal:3009`,
`SENDER_HOST=::`). Only the Sender gets the BIR secrets:

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
a trusted machine, not on Fly:

```sh
cargo run --release --bin buwiz-trust-sign -- keygen trust.key
# prints the public key: put it in eBIRForms crates/bir-submit/src/trust.rs TRUST_KEYS
fly ssh console -a buwiz-sender -C "buwiz-sender public" > sender-keys.json
cargo run --release --bin buwiz-trust-sign -- sign trust.key 365 sender-keys.json \
  <host:port> …   # BIR's dispatcher and upload hosts, from the BIR relay's private docs
# prints the JWS: fly secrets set -a buwiz-cloud TRUST_LIST_JWS=<it>
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
`https://downloads.buwiz.com` (`DOWNLOADS_URL`, the default). The eBIRForms
release CI uploads each version as:

- `releases/v{VERSION}/{file}`: the installers, never changed once uploaded.
- `releases/v{VERSION}/manifest.json`: the version, `published_at` and, for
  each file, its platform (`macos`, `windows`, `linux`, `android`), arch
  (`arm64`, `x64`, `universal`), kind (`dmg`, `msi`, `exe`, `appimage`,
  `deb`, `rpm`, `apk`), file name, size and SHA-256.

Nothing is released until `latest.json` exists at the bucket's root. To
release a version, copy its `manifest.json` there (with
`Cache-Control: max-age=60`); to take it back, delete `latest.json`. The
server reads `latest.json` at most once a minute, so a change shows within
about a minute. While it is missing, `/download` and the home page say
"Coming soon". Once it exists, the home page's platform row links to
`/download`, which lists every file with its size and SHA-256 and joins the
sitemap, and `/download/{mac,windows,linux,android}` (`?kind=deb` and so on
for a particular file) redirects, uncached, to the installer: by default the
Mac's universal DMG, the Windows x64 MSI or else EXE, the Linux AppImage or
else DEB, and the Android APK. A platform with no file says "Coming soon".

If the bucket can't be reached or `latest.json` is broken, the server keeps
the last good one, or says "Coming soon" if it never had one. File names
must be plain names (letters, digits, `.`, `_`, `-`, `+`); files with other
names, and platforms or kinds the server doesn't know, are left out.

## Domains

`buwiz.com`, `www.buwiz.com`, `account.buwiz.com` and `api.buwiz.com` have
A and AAAA records in Cloudflare pointing at the app's IPs (`fly ips list`).
The records are **DNS only** (grey cloud), because Fly issues the
certificates (`fly certs check <name>`). The app sends any `www.` host to the
bare domain with a permanent redirect. MX, SPF and site-verification records
on `buwiz.com` belong to email; leave them alone.

## Switch the mini app

```sh
BUWIZ_SERVER_URL=https://api.buwiz.com BUWIZ_ACCOUNT_URL=https://account.buwiz.com
```

Sign in from the app, add your TIN at `/tins/new`, approve it as
`ADMIN_EMAILS` at `/admin/claims`, and the licence gate opens.
