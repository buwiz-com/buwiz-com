# Deploy

Production runs on AWS EC2, behind a Cloudflare Tunnel. It moved there from
Fly on 2026-10-30. Fly is retired and `fly.toml` is gone.

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
private BIR files restored from the private stow repository (see
[relay.md](relay.md#restoring-the-private-files)). The `Dockerfile` builds with `--features bir,ocr`.
Without those files, the build fails, so it never ships a Sender that
cannot send.

The scripts are in `deploy/ec2/`. Copy `deploy/ec2/settings.example` to
`deploy/ec2/settings` (git-ignored) and set the deploy bucket and the
instance ID. Then, with `aws login --profile buwiz` current:

```sh
deploy/ec2/build-release.sh "$(git rev-parse --short HEAD)"   # build, upload
deploy/ec2/deploy.sh <sha>                                     # install, smoke-test
```

1. `build-release.sh` runs `docker build --platform linux/arm64` and copies
   the binaries `buwiz-cloud` and `buwiz-sender` and the OCR models out of
   the image, into `target/ec2-releases/<sha>/`. It uploads them, with
   `install.sh`, to the deploy bucket under `releases/<sha>/`. It refuses a
   sha that is not `HEAD`.
2. `deploy.sh` runs `install.sh <sha> <bucket> <region>` on the instance
   through SSM `send-command`. It swaps the binaries, rewrites the unit and
   tunnel settings, and restarts the services. Then it prints the last log
   lines and checks the public hosts.

Build one release at a time: the Docker build needs most of a 16 GB Mac.

Smoke-test after each deploy (`deploy.sh` does the first four):
- `buwiz.com/` and `/mini-apps`;
- `ebirforms.buwiz.com/download`;
- `account.buwiz.com/login`;
- `/healthz` on each host.

The rest of `deploy/ec2/` sets up a new instance:
- `user-data.sh`: first boot (updates, cloudflared, the AWS CLI, the systemd
  units, `/usr/local/bin/buwiz-env`, which writes `/etc/buwiz/env` from
  `/buwiz/*` at each start).
- `role-policy.json` and `trust.json`: the instance role. Replace `REGION`,
  `ACCOUNT_ID` and `DEPLOY_BUCKET` before use.
- `sender-keys.sh`: creates the Sender's keys in Parameter Store. It prints
  only names and versions, and refuses to replace a key that exists.

## Deploying the TIN release (migrations 11 and 12)

The release that adds TINs without papers, roles, bans, pauses, the seed
tool and automatic activation runs migrations 11 and 12 when it starts.
`install.sh` overwrites the previous binaries, and with `Restart=always` a
failed migration means a crash loop. The release before it refuses to start
on a database at 11 or 12 (sqlx finds versions it does not know). So check
first, keep the old binaries, and know the way back.

### Before you deploy

Run these against production (read only), from a machine with the Neon
connection string, in this order:

1. **Every active TIN says since when.** Must return no rows, or migration
   11 fails on its `decided_at` check:
   ```sql
   SELECT id FROM tin_claims WHERE status = 'approved' AND decided_at IS NULL;
   ```
2. **No account holds two live rows for one TIN.** Must return no rows, or
   the new one-live-row index fails:
   ```sql
   SELECT account_id, tin_root, branch_code, count(*) FROM tin_claims
    WHERE status IN ('pending', 'needs_fix', 'approved') GROUP BY 1, 2, 3 HAVING count(*) > 1;
   ```
3. **How the active TINs will be labelled.** Rows that name no reviewer
   become `verified_by = 'unknown'` (not `staff`); expect none:
   ```sql
   SELECT id, status FROM tin_claims WHERE status IN ('approved', 'removed', 'revoked') AND decided_by IS NULL;
   ```
4. **`ADMIN_EMAILS`.** Read `/buwiz/ADMIN_EMAILS` in Parameter Store.
   Everyone in it becomes an Admin with every new power at once (ban,
   activate without papers, pause, change roles), and the staff pages
   cannot demote them. It should name the owner's admin login only; a
   Support login does not belong in it.
5. **`AUTO_VERIFY`.** Make sure `/buwiz/AUTO_VERIFY` does not exist (or is
   not `on`): this release ships with automatic activation off
   ([below](#automatic-activation)).
6. **Nothing is being sent.** Right before the restart, must be `0` (a
   restart in the middle of a send ends it as `unknown`):
   ```sql
   SELECT count(*) FROM submissions WHERE status = 'sending';
   ```

Better still, restore the latest backup into the local Docker Postgres and
start the new binary on that copy first (with `DATABASE_URL` pointing at
`127.0.0.1`, never at Neon). Take a Neon branch right before the deploy as
the last resort; restoring it loses every return filed since (see the
rollback instead).

Private test fixtures (the BIR relay files) that insert active claims must
set `decided_at` and `verified_by`.

### Deploy

1. On the instance (SSM session), keep the running binaries:
   ```sh
   sudo cp -p /opt/buwiz/buwiz-cloud /opt/buwiz/buwiz-cloud.prev
   sudo cp -p /opt/buwiz/buwiz-sender /opt/buwiz/buwiz-sender.prev
   cat /opt/buwiz/RELEASE        # note the release you are leaving
   ```
2. Re-run check 6, then `deploy/ec2/deploy.sh <sha>` as usual.
3. Watch `journalctl -u buwiz.service` for `migrations failed`. If it
   crash-loops on a migration, stop it, put the `.prev` binary back and
   start it: a failed migration ran in one transaction and left the
   database at 10, so the old binary starts.
4. Smoke-test as below, then sign in as the admin login and open `/staff`.

### Rolling back

Use this when the new release runs (the database is at 12) and you need
the one before. It keeps every account, TIN, ticket, return and receipt.

**What it destroys:** TINs saved without papers (`unverified`) and their
rows; every stored role (only `ADMIN_EMAILS` makes admins again); the test
labels; how each TIN became active (`verified_by`), when papers were sent,
and what the automatic check saw; each audit line's `source`. Migrating up
again later labels each TIN ever active `staff` when it names a reviewer
and `unknown` when it does not.

**What it keeps elsewhere:** the audit lines the old release cannot hold
(activated without papers, paused, resumed, the automatic ones, the review
decisions), the whole `account_audit` (bans, roles, labels, sign-outs) and
`seed_runs` move, whole, to tables in the `rollback_archive` schema, which
the old release does not read. They are the only record of why a TIN that
stays active was activated without papers. Rolling back twice adds to them.

**What makes it refuse** (it then changes nothing and the database stays at
12; the newer down file makes every check of the older one first):
- an account is banned (the old release would let it sign in and send):
  lift each ban first;
- a TIN is paused (the old release would send for it): resume or revoke;
- a TIN saved without papers has files: decide those claims first;
- a TIN activated automatically waits for a person: confirm or revoke it;
- any account has a stored role: make sure `ADMIN_EMAILS` names your
  admins, then set `buwiz.rollback_drop_roles = 'yes'` in the same
  transaction (below).

The order:
1. Run check 6 above and wait for `0`, then stop the app:
   `sudo systemctl stop buwiz.service`.
2. Resolve each refusal above (on a stopped app, with SQL; or before
   stopping it, on the staff pages).
3. From the release's checkout, run the newer down file, then the older,
   each in one transaction:
   ```sh
   psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -1 \
     -c "SET LOCAL buwiz.rollback_drop_roles = 'yes'" -f migrations/0012_staff_roles.down.sql
   psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -1 -f migrations/0011_tin_states.down.sql
   psql "$DATABASE_URL" -c "SELECT max(version) FROM _sqlx_migrations"   # must be 10
   ```
   Leave out the `SET LOCAL` line unless you mean to drop the roles. If the
   first file refuses, nothing changed; if the second refuses, the
   database is at 11 and nothing of 11 changed: fix what it names and run
   the second again.
4. Put the previous binaries back and start:
   ```sh
   sudo cp -p /opt/buwiz/buwiz-cloud.prev /opt/buwiz/buwiz-cloud
   sudo cp -p /opt/buwiz/buwiz-sender.prev /opt/buwiz/buwiz-sender
   sudo systemctl restart buwiz-sender.service buwiz.service
   ```
   (Or `deploy/ec2/deploy.sh <previous sha>` once the database is at 10.)

`tests/migrations.rs` runs this whole rollback, by sqlx and by hand, on a
database with a row in every state.

### The seed tool

`buwiz-cloud seed <FILE> --as <admin email> [--dry-run]` makes logins,
roles, test labels and TINs from one TOML file, in one transaction
(`docs/seed.example.toml`; `src/account/seed.rs`). It reads
`DATABASE_URL`, `ADMIN_EMAILS` and the `S3_*` settings. It needs `S3_*` to
delete from R2 the files of a waiting claim that a `move_from` deletes;
without them it uses the database store and those objects stay behind, so
always run it with the service's settings. It runs no migration and refuses
a database not at this release's newest migration.

What it prints: the target database (host and name, no password), the
account and claim counts before, the file's SHA-256, one line per change
(`created login …`, `role … none → support`, `TIN … active on … (new)`,
`unchanged …`), and the number of changes. A dry run shows full TINs:
check every digit. Exit codes: 0 done or dry run, 2 refused (with every
reason; nothing written), 1 an error (nothing written). Each run is one
`seed_runs` row and one audit line per change.

1. Write the file on the Mac, outside the repository. Make a Neon branch.
2. Deploy this release. In an SSM session, put the file at
   `/tmp/seed.toml`, owned by `buwiz`, mode 600.
3. Dry run, with the service's settings:
   ```sh
   sudo systemd-run --uid=buwiz --pty --wait -p EnvironmentFile=/etc/buwiz/env \
     /opt/buwiz/buwiz-cloud seed /tmp/seed.toml --as <admin login> --dry-run
   ```
   Read every line: the database, the counts, each TIN.
4. Run it again without `--dry-run`, then delete `/tmp/seed.toml`.
5. Each address must be able to receive mail before anyone can sign in as
   it.

To be completed by the seed hardening pass: which `ADMIN_EMAILS` the tool
uses when it is run outside the service, and how it confirms the target
database before writing. Until then, run it only as above.

### Automatic activation

`AUTO_VERIFY` (Parameter Store `/buwiz/AUTO_VERIFY`) turns on automatic
activation of a personal TIN from its TIN ID photo
([accounts.md](accounts.md#automatic-activation-auto_verify)). It is off
unless the value is exactly `on`, and this release ships with it unset. To
turn it on, set `/buwiz/AUTO_VERIFY=on` and `sudo systemctl restart
buwiz.service`; to turn it off, delete the parameter (or set `off`) and
restart. Turning it off does not undo activations already made: confirm or
revoke them in the "Activated automatically" tab.

**The risk, plainly.** It checks that the TIN and name the person typed
match a photo the same person uploaded. Anyone who knows a taxpayer's TIN
and registered name (on payslips, BIR 2316s, contracts) can draw a TIN ID
that passes in minutes; the repository's own test does it in about 30
lines. The card itself is not checked: no photo, hologram, QR code or BIR
lookup. Once it passes, that person owns the TIN on Buwiz and **can file
real returns to the BIR under it at once**, before any person has looked,
with Buwiz vouching for the receipt; the real owner is locked out until
staff revoke it. The limits (one unconfirmed per account, 20 a day, a clean
history for the TIN, papers kept, a pause after 30 days unconfirmed) bound
the damage; they do not prevent it. And whether a claim is activated or
left for staff tells the claimant whether any other account ever had that
TIN.

Before turning it on, decide whether an automatically activated TIN may
send before a person confirms it (as built, it may), and keep someone
checking the tab daily.

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

## Going live with BIR (once)

A new server files nothing: `BIR_RELAY_MODE` is `dry_run` and there is no
trust list. To file for real, run this once from the owner's Mac (after
`aws login --profile buwiz`, with the private BIR files restored):

```sh
deploy/ec2/go-live.sh
```

It waits until BIR's upload server answers (it checks every 2 minutes, for
up to 12 hours), then runs:

1. `bir-env-from-forms.sh`: reads the BIR settings from the old buwiz-forms
   app (the IAF and dispatcher passphrases are eBIRForms protocol constants
   in its code, and the upload host is in its `.env`), checks the IAF
   passphrase against BIR's sample files, and reads the upload server's SSH
   key. It writes them to `~/buwiz-trust/bir-live.env`.
2. `bir-live.sh`: stores them in `/buwiz/*`, sets `BIR_RELAY_MODE=live` and
   restarts. If the server doesn't start, it goes back to `dry_run`.
3. `trust-list.sh`: signs the trust list and serves it (see below).

Deploys don't undo it: the settings stay in Parameter Store. `deploy.sh`
prints how long the trust list has left. Run `go-live.sh` again only to
renew the list (yearly) or when uploads stop with "host key is not pinned".

## The relay trust list (human step)

Apps trust three things only through a list signed offline with a key this
server never holds: the hosts the relay pipe may open, the Sender's HPKE
sealing key, and the Sender's receipt key. Make the key and sign the list on
a trusted machine, not on the server.

`deploy/ec2/trust-list.sh` does all of it in one run, with `aws login
--profile buwiz` and the private BIR files restored. The first time, it makes
`~/buwiz-trust/trust.key` and puts it on the clipboard for the password
manager. It reads the Sender's public keys through SSM and checks them
against `/.well-known/`. It signs and verifies the list, stores it as
`/buwiz/TRUST_LIST_JWS`, restarts `buwiz.service`, waits until `api.` serves
it, and prints the public key to pin. Run it again to renew. The manual steps:

```sh
cargo run --release --bin buwiz-trust-sign -- keygen trust.key
# prints the public key: put it in eBIRForms crates/bir-submit/src/filing/trust.rs TRUST_KEYS
# on the instance (through SSM): buwiz-sender public > sender-keys.json
cargo run --release --bin buwiz-trust-sign -- sign trust.key 365 sender-keys.json \
  <host:port> …   # BIR's dispatcher and upload hosts, from the BIR relay's private docs
# prints the JWS: store it as TRUST_LIST_JWS in /buwiz/* and restart buwiz.service
```

The `host:port` list is every host the Sender opens through the pipe: BIR's
dispatcher (HTTPS and HTTP, primary and backup) and its SFTP upload server.

Keep `trust.key` offline (password manager or hardware). It is the only
thing that exists only on the owner's Mac: everything else is in Parameter
Store, the stow repo or the AWS console. To restore it on a new machine,
copy it from the password manager, then:

```sh
mkdir -p ~/buwiz-trust && chmod 700 ~/buwiz-trust
pbpaste > ~/buwiz-trust/trust.key && chmod 600 ~/buwiz-trust/trust.key && pbcopy </dev/null
target/release/buwiz-trust-sign public ~/buwiz-trust/trust.key   # must print the pinned key
```

If it is lost for good, the served list keeps working until it expires.
Before then, make a new key, release apps that pin it, and sign a new list.

The list expires
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
Buwiz Bot's are in `bot/` in the same shape, except that its release
workflow writes `bot/latest.json` itself (`docs/release.md` in `buwiz-bot`),
and there is no root fallback.

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

Sign in from the app, add your TIN at `/tins/new`, approve it as staff
(Support, an Admin, or someone in `ADMIN_EMAILS`) at `/staff`, and the
licence gate opens.
