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
| `RECEIPT_SIGNING_KEY` | `openssl rand -base64 32` |
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

## Domains

`account.buwiz.com` and `api.buwiz.com` have A and AAAA records in Cloudflare
pointing at the app's IPs (`fly ips list`). The records are **DNS only**
(grey cloud), because Fly issues the certificates (`fly certs check <name>`).

## Switch the mini app

```sh
BUWIZ_SERVER_URL=https://api.buwiz.com BUWIZ_ACCOUNT_URL=https://account.buwiz.com
```

Sign in from the app, add your TIN at `/tins/new`, approve it as
`ADMIN_EMAILS` at `/admin/claims`, and the licence gate opens.
