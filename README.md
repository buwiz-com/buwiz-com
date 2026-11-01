# buwiz-com

The Buwiz server is one Rust binary that serves every public Buwiz host. A
second process, the Sender, is the only part that talks to BIR.

| Host | What it is | Code | Docs |
|---|---|---|---|
| `buwiz.com` | The Buwiz Bot site: `/` and `/mini-apps` | `bot-site/` (Leptos, built into `bot-site/dist/`), served by `src/bot_site.rs` | [docs/bot-site.md](docs/bot-site.md) |
| `ebirforms.buwiz.com` | The eBIRForms filing site: its home page, `/download` and the installers | `src/pages/home.rs`, `src/pages/download.rs`, `src/downloads.rs` | [docs/ebirforms-site.md](docs/ebirforms-site.md) |
| `account.buwiz.com` | Sign-in, My TINs and TIN claims, the staff pages, and app and agent sign-in | `src/pages.rs`, `src/web.rs`, `src/accounts.rs`, `src/claims.rs`, `src/staff.rs`, `src/oauth.rs`, `src/agents.rs` | [docs/accounts.md](docs/accounts.md) |
| `api.buwiz.com` | What the apps and agents call: the licence gate, devices, tickets, the submission relay and `/mcp` | `src/web.rs`, `src/devices.rs`, `src/tickets.rs`, `src/submissions.rs`, `src/mcp.rs` | [docs/accounts.md](docs/accounts.md), [docs/relay.md](docs/relay.md) |
| (private) | The Sender: BIR's passphrases, the SFTP login and the IAF encryption | `src/sender/`, `src/bin/buwiz-sender.rs`, plus `src/bir/` (not in this repository) | [docs/relay.md](docs/relay.md) |

`src/canonical.rs` decides which host serves what:
- The bare domain is the Bot site, and the filing site is on `FORMS_ORIGIN`.
- The filing site's old paths on the bare domain (`/download`, `/install-linux.sh` and the demo assets) answer with a 301 to `FORMS_ORIGIN`.
- `www.` always redirects to the bare domain.
- The account and API hosts are `noindex`.

The stack: [axum](https://github.com/tokio-rs/axum) for the API,
[Topcoat](https://github.com/tokio-rs/topcoat) for the account and filing
pages, and [Leptos](https://leptos.dev) for the Bot site, all on Postgres.

## Run it locally

Requires Rust 1.99 and Docker.

```sh
docker compose up -d                       # Postgres on 127.0.0.1:54330
cp .env.example .env                       # then set DOCUMENT_KEY:
echo "DOCUMENT_KEY=$(openssl rand -base64 32)" >> .env
set -a; . ./.env; set +a
cargo run                                  # http://localhost:3008
```

Migrations run at start. Sign-in codes are printed to the log. See
[docs/development.md](docs/development.md) for:
- the settings;
- the cargo features (`bir` and `ocr`);
- the hosts on localhost;
- the tests.

## Repository layout

| Path | What |
|---|---|
| `src/` | The server (`buwiz-cloud`). Its other binaries are in `src/bin/` (`buwiz-sender`, `buwiz-relay`) |
| `bot-site/` | The Buwiz Bot site, a separate Leptos crate. `bot-site/dist/` is committed and embedded in the binary |
| `migrations/` | Postgres migrations, run at start |
| `tests/` | Integration tests. `account_site.rs` needs `DATABASE_URL` |
| `e2e/` | A workspace of its own: submission v2 end to end with the real eBIRForms client (`scripts/v6.sh`) |
| `scripts/` | `v6.sh` (the e2e run), `ocr-models.sh` (fetches the OCR models) and `smoke.sh` |
| `docs/` | One page per host or subsystem (below) |

## Docs

- [docs/bot-site.md](docs/bot-site.md): buwiz.com, the Buwiz Bot site. Building it, prerendering and SEO.
- [docs/ebirforms-site.md](docs/ebirforms-site.md): ebirforms.buwiz.com, the filing site. Downloads and releases.
- [docs/accounts.md](docs/accounts.md):
  - accounts;
  - TIN activation and staff review;
  - OCR;
  - app sign-in;
  - the licence gate;
  - MCP for AI agents.
- [docs/relay.md](docs/relay.md): the submission relay. The Gatekeeper and the Sender, the `bir` feature and the relay modes.
- [docs/relay-protocol.md](docs/relay-protocol.md): the relay's wire protocol (v2), for app authors.
- [docs/development.md](docs/development.md): settings, features and tests.
- [docs/deploy.md](docs/deploy.md): production.
