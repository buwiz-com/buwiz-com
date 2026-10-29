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

If Fly's remote builder fails (503, `failed_precondition`), build on your
machine instead: `fly deploy --local-only`. To change only settings, redeploy
the last image: `fly deploy --image registry.fly.io/buwiz-cloud:<tag>`.

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
| `BIR_RELAY_MODE`, `BIR_IAF_PASSPHRASE`, `BIR_DISPATCHER_PASSPHRASE`, `BIR_SSH_HOST_KEYS` | See [relay-protocol.md](relay-protocol.md) §7 |

The Gatekeeper refuses to start on a public origin without `SENDER_URL`
and `TICKET_SIGNING_KEY`. See [relay-protocol.md](relay-protocol.md) §6.

## The egress list (human step)

Apps let the relay pipe connect only to the hosts on an egress list signed
offline with a key this server never holds. Make the key and sign the list
on a trusted machine, not on Fly:

```sh
cargo run --release --bin buwiz-egress-sign -- keygen egress.key
# prints the public key: put it in buwiz-oss crates/bir-submit/src/egress.rs EGRESS_KEYS
cargo run --release --bin buwiz-egress-sign -- sign egress.key 365 \
  birgovph.com:443 birgovph.com:80 ws2.birgovph.com:443 ws2.birgovph.com:80 ebf2.bir.gov.ph:22
# prints the JWS: fly secrets set -a buwiz-cloud EGRESS_LIST_JWS=<it>
```

Keep `egress.key` offline (password manager or hardware). The list expires
(here after 365 days); sign a new one before then, or apps stop sending.
To rotate the key, pin the new public key next to the old in a release
first, then sign with the new key.

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
