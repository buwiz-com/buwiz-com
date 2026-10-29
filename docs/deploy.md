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

Settings that aren't secret are in `fly.toml` under `[env]`.

## Deploy

```sh
fly deploy            # builds the Dockerfile remotely; migrations run at start
fly logs              # look for "listening on http://0.0.0.0:8080"
curl https://buwiz-cloud.fly.dev/healthz
```

## Custom domains

```sh
fly certs add account.buwiz.com
fly certs add api.buwiz.com
fly ips list          # then point both names at the app's IPs (A and AAAA) or CNAME buwiz-cloud.fly.dev
```

Then set `ACCOUNT_ORIGIN = "https://account.buwiz.com"` and
`EXTRA_ORIGINS = "https://api.buwiz.com"` in `fly.toml`, and deploy.

## Switch the mini app

```sh
BUWIZ_SERVER_URL=https://api.buwiz.com BUWIZ_ACCOUNT_URL=https://account.buwiz.com
```

Sign in from the app, add your TIN at `/tins/new`, approve it as
`ADMIN_EMAILS` at `/admin/claims`, and the licence gate opens.
