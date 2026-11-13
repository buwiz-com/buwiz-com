# Development

## Settings

All settings are environment variables. See [`.env.example`](../.env.example)
and `src/base/config.rs`.

| Setting | What |
|---|---|
| `DATABASE_URL`, `DOCUMENT_KEY` | Required. `DOCUMENT_KEY` encrypts claim files: losing it makes every stored file unreadable |
| `ACCOUNT_ORIGIN` | The account host. If it starts with `https://`, secure cookies turn on, and a public deployment must send email (`RESEND_API_KEY`, `MAIL_FROM`). The server refuses to start if it would print sign-in codes on a public deployment |
| `FORMS_ORIGIN` | The filing site's host (default `https://ebirforms.buwiz.com`). The bare domain is the Bot site ([bot-site.md](bot-site.md)) |
| `EXTRA_ORIGINS`, `ADMIN_EMAILS` | Additional allowed origins. Staff email addresses |
| `DOWNLOADS_URL` | Where the release manifests and installers are ([ebirforms-site.md](ebirforms-site.md)) |
| `GITHUB_API_URL` | Where Buwiz Bot's GitHub star count is read (default `https://api.github.com`; [bot-site.md](bot-site.md)) |
| `S3_ENDPOINT`, `S3_REGION`, `S3_BUCKET`, `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY` | The claim file store. Without them, files go to Postgres |
| `TICKET_SIGNING_KEY`, `RECEIPT_SIGNING_KEY`, `SENDER_URL`, `SENDER_SECRET`, `BIR_*` | The relay ([relay.md](relay.md)) |
| `OCR_MODEL_DIR` | The OCR models, with `--features ocr` |
| `AUTO_VERIFY` | Automatic activation of a TIN from its TIN ID photo ([accounts.md](accounts.md#automatic-activation-auto_verify)). Off unless exactly `on`; any other value, or none, is off. Read once at start |
| `HOST`, `PORT`, `RUST_LOG` | Listening address and logs |

**The hosts on localhost:** the first label of the host picks the site
(`src/sites/canonical.rs`):
- `account.localhost:3008` is the account site;
- `api.localhost:3008` is the API;
- `ebirforms.localhost:3008` or `forms.localhost:3008` is the filing site;
- `localhost:3008`, like any other host, is the Bot site.

## Cargo features

- **`bir`:** the BIR relay code. It needs the private files, which are not in this repository. See [relay.md](relay.md#restoring-the-private-files).
- **`ocr`:** reads claim documents on the server (`src/ocr/`).
  - It adds ONNX Runtime, linked statically, and about 30 MB to the binary. That ONNX Runtime needs GCC 14's libstdc++, so the `Dockerfile` builds on Ubuntu 24.04.
  - The models are 13 MB, Apache-2.0, and are not in git. `scripts/ocr-models.sh` fetches them, pinned and checked, into `ocr-models/`. `OCR_MODEL_DIR` points somewhere else.
  - Without the feature, every reading ends "OCR not built in".

The `Dockerfile` builds with `--features bir,ocr`. CI builds, lints and
tests without them.

## Tests

### Running them safely

The site tests use `#[sqlx::test]`, which makes a database per test from
`DATABASE_URL`. **It also loads the first `.env` it finds in the working
directory or any directory above it** (sqlx reads `DATABASE_URL` through
`dotenvy::var`). That fills in every setting the environment lacks: an
exported `DATABASE_URL` is kept, but forget to export it and the file's
database is used, and every other setting in the file (object-store and
mail keys included) enters the test process either way. In a checkout
whose `.env` points at a real database, plain `cargo test` can create and
drop test databases there. So in any checkout that has, or sits under, an
`.env` with real settings:

- do not run plain `cargo test`;
- build the test binaries, then run each from an empty directory with a
  clean environment that holds only a local database and a throwaway key:

```sh
docker compose --env-file /dev/null -f compose.yaml up -d postgres   # plain `docker compose` reads .env too
DB=postgres://buwiz_cloud:buwiz_cloud@127.0.0.1:54330/buwiz_cloud
DATABASE_URL=$DB cargo test --locked --no-run --message-format=json \
  | python3 -I -c 'import sys,json
for l in sys.stdin:
    m=json.loads(l)
    if m.get("reason")=="compiler-artifact" and m.get("executable") and m["profile"]["test"]: print(m["executable"])' > /tmp/test-bins
mkdir -p /tmp/empty && cd /tmp/empty
while read -r exe; do
  env -i PATH=/usr/bin:/bin HOME=/tmp/empty DATABASE_URL=$DB \
    DOCUMENT_KEY=dGhyb3dhd2F5LXRlc3Qta2V5LW5vdC1hLXNlY3JldC0wMDA= "$exe" || echo "FAILED: $exe"
done < /tmp/test-bins
```

Check that `DATABASE_URL` names `127.0.0.1` before anything runs. In a
checkout with no `.env` anywhere above it (CI), `cargo test` with
`DATABASE_URL` exported is fine. The clean environment also has no
`AUTO_VERIFY`, so the suite runs with automatic activation off except
where a test turns it on itself (`Site::with_auto`).

| Command | Runs |
|---|---|
| `cargo test` (see above) | Everything that needs no BIR code. That includes the account site against Postgres (`tests/accounts.rs`, `tests/claims.rs`, `tests/auto_verify.rs`, `tests/migrations.rs` and the others), where each test gets its own database |
| `cargo test --features bir` | Adds the relay end to end in one process: a fake dispatcher, an SSH server with a pinned key, and the app's allow-list |
| `cargo test --features bir -- --include-ignored` | Also runs HTTPS through the pipe against example.com |
| `--features bir` with `BIR_IAF_PASSPHRASE` | `tests/iaf_golden.rs` checks the encryption against official `Encrypt.exe` output |
| `scripts/ocr-models.sh && cargo test --features ocr` | `tests/ocr.rs`: OCR of a synthetic TIN ID drawn in the test, alone, through a claim, and through automatic activation (a drawn card passes; see [deploy.md](deploy.md#automatic-activation)) |
| `scripts/v6.sh` | `e2e/`, its own workspace: submission v2 with the real eBIRForms client, from a queued 1701Q to a verified receipt, plus the attacks it must refuse |

About `scripts/v6.sh`:
- The eBIRForms client comes from [eBIRForms/eBIRForms](https://github.com/eBIRForms/eBIRForms), pinned by rev in `e2e/Cargo.toml`. `EBIRFORMS_PATH=<checkout>` tests a local checkout instead.
- It needs Docker and the BIR files.

For the Bot site, see [bot-site.md](bot-site.md).
