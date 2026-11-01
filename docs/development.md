# Development

## Settings

All settings are environment variables. See [`.env.example`](../.env.example)
and `src/config.rs`.

| Setting | What |
|---|---|
| `DATABASE_URL`, `DOCUMENT_KEY` | Required. `DOCUMENT_KEY` encrypts claim files: losing it makes every stored file unreadable |
| `ACCOUNT_ORIGIN` | The account host. If it starts with `https://`, secure cookies turn on, and a public deployment must send email (`RESEND_API_KEY`, `MAIL_FROM`). The server refuses to start if it would print sign-in codes on a public deployment |
| `SITE_SPLIT`, `FORMS_ORIGIN` | On by default: the bare domain is the Bot site, and the filing site is on `FORMS_ORIGIN` (default `https://ebirforms.buwiz.com`). With `SITE_SPLIT=false`, every other host is the filing site ([bot-site.md](bot-site.md)) |
| `EXTRA_ORIGINS`, `ADMIN_EMAILS` | Additional allowed origins. Staff email addresses |
| `DOWNLOADS_URL` | Where the release manifests and installers are ([ebirforms-site.md](ebirforms-site.md)) |
| `S3_ENDPOINT`, `S3_REGION`, `S3_BUCKET`, `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY` | The claim file store. Without them, files go to Postgres |
| `TICKET_SIGNING_KEY`, `RECEIPT_SIGNING_KEY`, `SENDER_URL`, `SENDER_SECRET`, `BIR_*` | The relay ([relay.md](relay.md)) |
| `OCR_MODEL_DIR` | The OCR models, with `--features ocr` |
| `HOST`, `PORT`, `RUST_LOG` | Listening address and logs |

**The hosts on localhost:** the first label of the host picks the site
(`src/canonical.rs`):
- `account.localhost:3008` is the account site;
- `api.localhost:3008` is the API;
- `ebirforms.localhost:3008` or `forms.localhost:3008` is the filing site;
- `localhost:3008` is the Bot site, or the filing site with `SITE_SPLIT=false`.

## Cargo features

- **`bir`:** the BIR relay code. It needs the private files. See [relay.md](relay.md).
- **`ocr`:** reads claim documents on the server (`src/ocr/`).
  - It adds ONNX Runtime, linked statically, and about 30 MB to the binary. That ONNX Runtime needs GCC 14's libstdc++, so the `Dockerfile` builds on Ubuntu 24.04.
  - The models are 13 MB, Apache-2.0, and are not in git. `scripts/ocr-models.sh` fetches them, pinned and checked, into `ocr-models/`. `OCR_MODEL_DIR` points somewhere else.
  - Without the feature, every reading ends "OCR not built in".

The `Dockerfile` builds with `--features bir,ocr`. CI builds, lints and
tests without them.

## Tests

| Command | Runs |
|---|---|
| `cargo test` | Everything that needs no BIR code. That includes the account site against Postgres (`tests/account_site.rs`), where each test gets its own database. `DATABASE_URL` must be set, for example from `.env` |
| `cargo test --features bir` | Adds the relay end to end in one process: a fake dispatcher, an SSH server with a pinned key, and the app's allow-list |
| `cargo test --features bir -- --include-ignored` | Also runs HTTPS through the pipe against example.com |
| `--features bir` with `BIR_IAF_PASSPHRASE` | `tests/iaf_golden.rs` checks the encryption against official `Encrypt.exe` output |
| `scripts/ocr-models.sh && cargo test --features ocr` | `tests/ocr.rs`: OCR of a synthetic TIN ID drawn in the test, alone and through a claim |
| `scripts/v6.sh` | `e2e/`, its own workspace: submission v2 with the real eBIRForms client, from a queued 1701Q to a verified receipt, plus the attacks it must refuse |

About `scripts/v6.sh`:
- The eBIRForms client comes from [eBIRForms/eBIRForms](https://github.com/eBIRForms/eBIRForms), pinned by rev in `e2e/Cargo.toml`. `EBIRFORMS_PATH=<checkout>` tests a local checkout instead.
- It needs Docker and the BIR files.

For the Bot site, see [bot-site.md](bot-site.md).
