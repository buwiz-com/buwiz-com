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
- **Settings:** in SSM Parameter Store under `/buwiz/*`. At each start they
  are written to two files: `/etc/buwiz/env` for the Gatekeeper and
  `/etc/buwiz-sender/sender.env` for the Sender, which runs as its own user
  ([below](#the-sender)).

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
| `TURNSTILE_SITE_KEY`, `TURNSTILE_SECRET_KEY` | A Cloudflare Turnstile widget (Managed) for the hostname `account.buwiz.com`; `deploy/ec2/turnstile-keys.sh` stores both and restarts the server. Without them the sign-in forms have no check against scripts, and the server says so in its log at start ([accounts.md](accounts.md#who-can-be-mailed-a-code)) |
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
   `install.sh` and `buwiz-env`, to the deploy bucket under
   `releases/<sha>/`. It refuses a sha that is not `HEAD`.
2. `deploy.sh` runs `install.sh <sha> <bucket> <region>` on the instance
   through SSM `send-command`. It swaps the binaries, puts
   `/usr/local/bin/buwiz-env` in place, makes the `buwiz-sender` user if it
   is missing, rewrites `buwiz-env.service`, the Sender's unit, the app
   unit's drop-ins and the tunnel settings, and restarts the services. Then it prints the last log
   lines and checks the public hosts.

Build one release at a time: the Docker build needs most of a 16 GB Mac.

Smoke-test after each deploy (`deploy.sh` does the first four):
- `buwiz.com/` and `/mini-apps`;
- `ebirforms.buwiz.com/download`;
- `account.buwiz.com/login`;
- `/healthz` on each host.

The rest of `deploy/ec2/` sets up a new instance:
- `user-data.sh`: first boot (updates, cloudflared, the AWS CLI, the users
  `buwiz` and `buwiz-sender`, the app's and the tunnel's units). The
  Sender's unit and `buwiz-env` come with each release (`install.sh`).
- `test-install.sh`: runs `user-data.sh` and `install.sh` in an Ubuntu
  24.04 container, with fake settings and no AWS, and checks the settings
  files, users, modes, units and that neither service reaches a fake
  metadata address ([below](#the-sender)).
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

0. **The database is at migration 10.** Must return `10`:
   ```sql
   SELECT max(version) FROM _sqlx_migrations;
   ```
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

Close every `psql` session you opened for these (no `BEGIN` left open).
The migrations wait at most 10 seconds for a lock; an open transaction on
these tables makes start-up fail with `lock timeout` instead of hanging.

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
   crash-loops on a migration, stop it (`sudo systemctl stop
   buwiz.service`) and see where the database stands: sqlx commits each
   migration on its own, so 11 can be in when 12 failed.
   ```sql
   SELECT max(version) FROM _sqlx_migrations;
   ```
   - **10:** put the previous binaries back ([below](#putting-the-previous-binaries-back))
     and start them.
   - **11:** first take 11 back, from the release's checkout:
     `psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -1 -f migrations/0011_tin_states.down.sql`
     (the app never ran on it, so none of its refusals apply), check
     `max(version)` is `10`, then put the previous binaries back and start
     them.
   - **12:** both went in and the failure is something else: read the
     journal, and roll back as [below](#rolling-back) if you must.
4. Once it runs, check both migrations went in, and read the start-up
   lines:
   ```sql
   SELECT version, success FROM _sqlx_migrations WHERE version >= 11 ORDER BY version;   -- 11 t, 12 t
   ```
   ```sh
   journalctl -u buwiz.service -b --no-pager | grep -i 'ADMIN_EMAILS'   # must print nothing
   ```
   A line there names an `ADMIN_EMAILS` entry that is not a plain address
   (a semicolon, a space, quotes): that person is not an Admin. Fix the
   parameter and restart.
5. Smoke-test as below, then sign in as the admin login and open `/staff`.

#### Putting the previous binaries back

Stop both services first: copying over a running executable fails ("Text
file busy") or crashes it, and leaves an old Gatekeeper with a new Sender.
Copy to a new name and move it into place, as `install.sh` does:

```sh
sudo systemctl stop buwiz.service buwiz-sender.service
sudo cp -p /opt/buwiz/buwiz-cloud.prev /opt/buwiz/buwiz-cloud.new && sudo mv /opt/buwiz/buwiz-cloud.new /opt/buwiz/buwiz-cloud
sudo cp -p /opt/buwiz/buwiz-sender.prev /opt/buwiz/buwiz-sender.new && sudo mv /opt/buwiz/buwiz-sender.new /opt/buwiz/buwiz-sender
sudo systemctl start buwiz-sender.service buwiz.service
systemctl is-active buwiz-sender.service buwiz.service   # active, active
```

(Or `deploy/ec2/deploy.sh <previous sha>` once the database is at 10.)

### Rolling back

Use this when the new release runs (the database is at 12) and you need
the one before. It keeps every account, TIN, ticket, return and receipt.
From a release with migration 13, it goes **step by step** instead: see
[below](#rolling-back-from-13).

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

**What makes it refuse** (the file that refuses changes nothing and the
database stays at 12; the newer down file makes every check of the older
one first, so from 12 a rollback goes all the way to 10 or changes
nothing):
- an account is banned (the old release would let it sign in and send):
  lift each ban;
- a TIN is paused (the old release would send for it): resume or revoke it;
- a TIN saved without papers has files: decide those claims;
- a TIN activated automatically that no person confirmed: confirm each on
  the "Activated automatically" tab, revoked and removed ones too
  (revoking one does not confirm it);
- any account has a stored role: make sure `ADMIN_EMAILS` names your
  admins; the override is a second, separate command (below).

The order:
1. **While the app runs,** resolve each refusal above on the staff pages,
   so each change has its audit line, and a confirmation deletes the
   papers from R2 (a confirmation made in SQL leaves them there for good).
   Count what is left; each must be `0` but the last (the stored roles):
   ```sql
   SELECT (SELECT count(*) FROM accounts WHERE banned_at IS NOT NULL) AS banned,
          (SELECT count(*) FROM tin_claims WHERE paused_at IS NOT NULL) AS paused,
          (SELECT count(*) FROM claim_documents d JOIN tin_claims c ON c.id = d.claim_id
            WHERE c.status = 'unverified') AS unverified_files,
          (SELECT count(*) FROM tin_claims WHERE verified_by = 'auto' AND auto_confirmed_at IS NULL) AS unconfirmed,
          (SELECT count(*) FROM accounts WHERE role IS NOT NULL) AS roles;
   ```
2. Run check 6 above and wait for `0`, then stop the app:
   `sudo systemctl stop buwiz.service`.
3. From the release's checkout, run the newer down file, then the older,
   each in one transaction (from a release with migration 13, run
   [its down file](#migration-13-test-tins) first, from one with 14,
   [14's](#migration-14-bir-uptime) before that, and from one with 15,
   [15's](#migration-15-fixed-sign-in-codes) before 14's; each commits on its own,
   [below](#rolling-back-from-13)):
   ```sh
   psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -1 -f migrations/0012_staff_roles.down.sql
   psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -1 -f migrations/0011_tin_states.down.sql
   psql "$DATABASE_URL" -c "SELECT max(version) FROM _sqlx_migrations"   # must be 10
   ```
   **Only if** the first refuses because accounts have a stored staff role,
   and you have checked that `ADMIN_EMAILS` names your admins, run it again
   with the override, then the second file:
   ```sh
   psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -1 \
     -c "SET LOCAL buwiz.rollback_drop_roles = 'yes'" -f migrations/0012_staff_roles.down.sql
   ```
   If the first file refuses for anything else, it changed nothing and the
   database is at 12: start the app, resolve it on the staff pages, and go
   back to step 2 (from 13, the app that runs at 12 is the previous
   release: [below](#rolling-back-from-13)). The second file makes no
   check the first did not make on the same stopped database, so it
   refuses only if something changed in between; then the database is at
   11 and nothing of 11 changed: fix what it names and run the second
   again.
4. Put the previous binaries back and start them, as
   [above](#putting-the-previous-binaries-back).

`tests/migrations.rs` runs this whole rollback, by sqlx and by hand, on a
database with a row in every state.

#### Rolling back from 13

From a release with migration 13 the rollback is **step by step, not all
the way or nothing**. Each down file commits on its own, and 13's down
commits before 12's checks run: 13's has none of 12's checks, and 12's
none of 13's. So:
- if 13's down file refuses (a test TIN has a row), nothing changed and
  the database is at 13: this release still runs on it;
- once 13's down file has run, the database is at 12, whatever 12's down
  file does next. A refusal by 12's down file leaves it there, at 12,
  which the previous release (the one before test TINs) runs on: put that
  release's binaries back and start it, resolve the refusal on its staff
  pages, and run 12's and 11's down files from there. Starting this
  release again instead runs migration 13 again.

### The seed tool

`buwiz-cloud seed` makes logins, roles, test labels and TINs from one TOML
file, in one transaction (`docs/seed.example.toml`; `src/account/seed.rs`):

```text
usage: buwiz-cloud seed <FILE> --as <admin email> --dry-run [--no-admin-emails]
       buwiz-cloud seed <FILE> --as <admin email> --target <host:port/name> --expect <digest> [--no-admin-emails]
       buwiz-cloud seed <FILE> --as <admin email> --target <host:port/name> --no-dry-run-check [--no-admin-emails]
```

It reads `DATABASE_URL`, `ADMIN_EMAILS` and the `S3_*` settings, so it runs
**on the instance, with the service's own settings file**, like the other
server-side Gatekeeper commands (`buwiz-cloud bir`): `systemd-run
--uid=buwiz -p EnvironmentFile=/etc/buwiz/env`. Never run it from a shell
with settings of its own.

- **Who is an Admin** comes from the server's `ADMIN_EMAILS`. The command
  refuses to run when `ADMIN_EMAILS` is not set, unless `--no-admin-emails`
  says the server has none (both at once is refused too). It prints the
  list it used. While the server has a list, `--as` must be an Admin (in
  that list or by stored role); the file can make `--as` the first Admin
  only on a server without the list and a database with no Admin. `--as`
  cannot change its own role.
- **The database** it writes to must be named: a real run needs `--target
  host:port/name`, equal to the database it is connected to (the host and
  port of `DATABASE_URL`, the name the server reports). Only the dry run
  prints the value; a real run without it says to take it from there.
- **The plan** a real run commits is the one a dry run showed: it needs
  `--expect <digest>`, the plan digest the dry run printed, and refuses if
  the plan is no longer that one (the database, `--as`, `ADMIN_EMAILS`, the
  file, or anything it would change or keep). `--no-dry-run-check` runs it
  without, on purpose (it then commits a plan nobody read in full, and
  says so on stderr); the runbook below never uses it.
- **Before it writes**, a real run prints the database, the account and
  claim counts, `ADMIN_EMAILS`, the file's SHA-256, the plan digest and
  every planned line, then commits.
- **Files**: a `move_from` that deletes a claim waiting for review deletes
  its files from R2 after the commit. Without `S3_*` such a move is
  refused. A file it could not delete is listed on stderr with the seed
  run's id, and the command exits 1: delete those objects by hand.
- **TINs** are shown in full only in a dry run. A real run masks them
  everywhere, refusals included, and `seed_runs` stores them masked. A file
  that does not parse is named by line and column, with no line of it and
  every digit of the message hidden; the dry run shows the line.
- It waits at most 5 seconds for a lock another change holds (a role
  change, a ban, a claim being decided), then gives up having written
  nothing: run it again.
- It runs no migration, and refuses a database whose applied migrations
  are not exactly this release's (by number and checksum).

Exit codes: 0 done or dry run; 2 refused, with every reason, nothing
written; 1 an error, nothing written, or (with "The changes were made")
files left in R2. Each real run is one `seed_runs` row (who ran it as
`--as`, the OS user and the one behind `sudo`, the host, the target, the
file's SHA-256, the plan digest, and what changed) and one audit line per
account or claim it changes or makes, the `--as` account included.

**Who ran it** is not in that row. Session Manager runs every person as
`ssm-user`, so run as below the OS user is always `buwiz (sudo from
ssm-user)` and the host is always the instance: it says where it ran, not
who. `--as` is whatever was typed. The record of who ran it is CloudTrail:
the Session Manager `StartSession` event names the IAM identity and the
time. Note the seed run id beside the session id when you run it.

**The dry run's full TINs go to the session.** If Session Manager logs
sessions (to S3 or CloudWatch), that log keeps them: check the account's
Session Manager preferences, and treat that log as holding taxpayer TINs.

Reasons in the file stop at 500 characters, as on the website.

A test TIN (000-000-000, at any branch) goes in only with `test_tin = true`
on its entry, active, on a login with the test label
(`docs/seed.example.toml`); without that key it is refused on its own like
any TIN that is not valid. A file that takes the label off a login holding
one is refused.

#### Running it

1. Write the file on the Mac, outside the repository. Make a Neon branch.
2. Deploy this release. In an SSM session, put the file on the instance,
   readable by `buwiz` only:
   ```sh
   sudo install -o buwiz -g buwiz -m 600 /dev/stdin /tmp/seed.toml   # paste the file, then Ctrl-D
   ```
3. Dry run, with the service's settings (`--as` is your Admin login, one
   of the server's `ADMIN_EMAILS` the first time):
   ```sh
   sudo systemd-run --quiet --uid=buwiz --pipe --wait -p EnvironmentFile=/etc/buwiz/env \
     -E SUDO_USER -E RUST_LOG=off \
     /opt/buwiz/buwiz-cloud seed /tmp/seed.toml --as <admin login> --dry-run
   ```
   Read every line: `Database:` is the production database; `Before:`
   has the counts you expect; `ADMIN_EMAILS:` is the server's list; every
   digit of every TIN; every `kept …` line (staff changed it on the
   website; the file loses), every `refused TIN …`, and every `note: …`
   line: another account has a claim on a TIN the file makes active
   (waiting for review, sent back, or saved without papers), which can no
   longer be approved. The dry run names that account; the real run says
   "another account". If it refuses, fix
   the file (or the database, on `/staff/queue`) and dry-run again. Its last
   line is the rest of the real run's command:
   `--target <host:port/name> --expect <digest>`.
4. The real run: the same command, the same `--as`, with that line in
   place of `--dry-run`:
   ```sh
   sudo systemd-run --quiet --uid=buwiz --pipe --wait -p EnvironmentFile=/etc/buwiz/env \
     -E SUDO_USER -E RUST_LOG=off \
     /opt/buwiz/buwiz-cloud seed /tmp/seed.toml --as <admin login> \
     --target <host:port/name> --expect <digest>
   echo "exit $?"
   ```
   It prints the same plan (TINs masked), then `N change(s) made, recorded
   as seed run <id>.` Exit 0 is done. If it refuses because the plan
   changed, someone changed the database in between: go back to step 3.
5. `sudo rm /tmp/seed.toml`.
6. Each address must be able to receive mail before anyone can sign in as
   it.

#### Running it again

Running the same file again **changes nothing when nothing changed in
between**: every line reads `unchanged …` (or `refused TIN …` for a TIN
that is not valid), it makes 0 changes ("Nothing to change."), and adds
only its own `seed_runs` row.

When staff or owners changed something on the website in between, a
re-run still never undoes it:
- a role or test label whose newest change was made on the website (by
  when it was written, so a change that committed after a seed run is
  newer than it) is kept, and printed as `kept role …` or `kept test label
  …` (not counted as a change);
- a TIN this login had and that was revoked, removed by its owner,
  rejected or deleted on the website, or that an earlier seed run moved to
  another login, and an active TIN that was ever paused on any login,
  refuse the whole file until the entry says `reapply = true` (or is taken
  out of the file). A deleted claim is matched by its masked TIN (the last
  three digits and the branch), so another such TIN on the login asks for
  `reapply` too;
- a TIN already active is never made "not verified", and a paused one
  stays paused;
- the `--as` Admin's own role never changes.
Everything else that differs from the file is changed, and printed.

### Automatic activation

`AUTO_VERIFY` (Parameter Store `/buwiz/AUTO_VERIFY`) turns on automatic
activation of a personal TIN from its TIN ID photo
([accounts.md](accounts.md#automatic-activation-auto_verify)). It is off
unless the value is exactly `on`, and this release ships with it unset. To
turn it on, set `/buwiz/AUTO_VERIFY=on` and `sudo systemctl restart
buwiz.service`; to turn it off, delete the parameter (or set `off`) and
restart. Turning it off does not undo activations already made, and the
hourly upkeep still pauses one left unconfirmed for 30 days. Check each in
the "Activated automatically" tab: revoke any that look wrong, and confirm
each (revoked ones too), which deletes its papers.

**The risk, plainly.** It checks that the TIN and name the person typed
match a photo the same person uploaded. Anyone who knows a taxpayer's TIN
and registered name (on payslips, BIR 2316s, contracts) can draw a TIN ID
that passes in minutes; the repository's own test does it in about 30
lines. The card itself is not checked: no photo, hologram, QR code or BIR
lookup. Once it passes, that person owns the TIN on Buwiz and **can file
real returns to the BIR under it at once**, before any person has looked,
with Buwiz vouching for the receipt; the real owner is locked out until
staff revoke it. "Personal" is the claimant's own choice: a person can
pick it for a corporation's TIN, whose TIN and registered name are printed
on every receipt it issues, draw a "TIN ID" with that name, and it passes,
unless that TIN was ever claimed as a business on Buwiz. "Business TINs are
always checked by staff" holds only for claims that say they are one. The
limits (one unconfirmed per account, 20 a day, a clean history for the
TIN, papers kept, a pause after 30 days unconfirmed) bound the damage; they
do not prevent it. And whether a claim is activated or left for staff
tells the claimant whether any other account ever had that TIN.

Before turning it on, decide whether an automatically activated TIN may
send before a person confirms it (as built, it may), and keep someone
checking the tab daily.

## Migration 13 (test TINs)

The release with test TINs ([accounts.md](accounts.md#bans-and-test-accounts))
runs migration 13 when it starts. It adds one check to `tin_claims`: a row
whose TIN root is `000000000` (a test TIN) is one an Admin or the seed tool
made active (`verified_by` `admin` or `seed`), and it is active or ended
from there (`approved`, `removed` or `revoked`). No column changes. The
release before it (at 12) refuses to start on a database at 13, as with
any newer migration.

**Before you deploy.** The database is at 12, and no row has such a root
(every form refused one); both as read-only checks:
```sql
SELECT max(version) FROM _sqlx_migrations;                           -- 12
SELECT count(*) FROM tin_claims WHERE tin_root = '000000000';        -- 0
```
Then deploy as above (keep the previous binaries, check 6, `deploy.sh`).

**Rolling back to 12.** The release before this one cannot read a test
TIN: every page that lists one stops with an error. So the down file
refuses while any row holds one, ended ones too, and then changes nothing.
Revoke each test TIN on the staff pages first (the history keeps who and
why), stop the app, delete the rows (since the
[test-TIN dry run](#deploying-the-test-tin-dry-run) they may have tickets
and returns; the file's comment has the statements for those), then:
```sh
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -1 -f migrations/0013_test_tins.down.sql
psql "$DATABASE_URL" -c "SELECT max(version) FROM _sqlx_migrations"   # must be 12
```
and put the previous binaries back. Their audit lines stay (the TIN masked,
`***-***-000-…`). Going further back, to 10, runs 13's down file first,
then 12's and 11's as [above](#rolling-back), step by step: 13's commits
before 12's checks run, and a refusal by 12's leaves the database at 12
([Rolling back from 13](#rolling-back-from-13)).

## Deploying the test-TIN dry run

The release with the test-TIN dry run lets an active test TIN (root
`000-000-000`, on a test account, added by staff) send through the whole
production route, and makes the Sender turn every such send into a dry
run, whatever `BIR_RELAY_MODE` says. The first filings from each platform's
release app (Mac, Windows, iPhone, iPad, Android) can then end in a signed
`dry_run` receipt while the server stays at `BIR_RELAY_MODE=live`.

What changes on deploy:
- the gate no longer refuses a test TIN with `tin_test_only`: an active
  one gets tickets and its returns are taken (a paused one answers
  `tin_paused`; a banned or practice account is refused first, as before);
- the licence list (`GET /api/tax-profiles`) lists a test TIN, with
  `test_only: true`;
- the Sender sends a test TIN's return in `dry_run`: no dispatcher login,
  no upload stream. Its receipt says `"mode":"dry_run"`,
  `"status":"dry_run"`, signed with the usual receipt key, and the
  submission ends `dry_run`;
- such a send gets no `bir_tries` row and moves no part of the BIR sign.

It needs no setting, no migration and no new secret; the Gatekeeper and
the Sender come from the same build and restart together, as always
([The Sender](#the-sender)). Real TINs send exactly as before.

**Checking it after deploy, without filing anything.** Only a test TIN can
be used, so nothing reaches the BIR:
1. On a test-labelled account that is **not** a practice one (no fixed
   sign-in code), add a test TIN on its staff page ("Add a test TIN").
2. From a release app signed in to it, send any return for that TIN. The
   app should show a dry run (a test send), not "filed".
3. `GET /api/submissions/{id}` shows `"status":"dry_run"`,
   `"mode":"dry_run"` and the receipt; its payload's `mode` and `status`
   are `dry_run`.
4. The Sender's journal has the dry-run lines for that submission, and no
   dispatcher line:
   ```sh
   sudo journalctl -u buwiz-sender.service --since -10min --no-pager -o cat | grep -E 'test TIN: dry run|dry run: not sent to BIR'
   ```
5. No uptime row was written for it (read-only):
   ```sql
   SELECT count(*) FROM bir_tries WHERE started_at > now() - interval '10 minutes';  -- unchanged by the test send
   ```

To roll back, put the previous binaries back: they refuse a test TIN again
(`tin_test_only`), and the `dry_run` submissions stay as they are.

## Migration 14 (BIR uptime)

The release that records BIR uptime ([bir-uptime.md](bir-uptime.md))
runs migration 14 when it starts. It adds new tables only (`bir_tries`,
`bir_events`, `bir_sign`, `bir_notes`, `bir_queue_reports`, `bir_turns`,
`bir_held`, `bir_host_keys`, `bir_days`, `bir_proofs`) and two nullable
columns, `tickets.bir_admission` and `devices.bir_reported_at`. No
existing row changes, and no existing table is scanned under its lock
(the check on `tickets.bir_admission` is added `NOT VALID`: every existing
row is NULL). None of the tables holds a TIN, a name or an IP address;
four columns hold an Admin's email for the audit
(`bir_events.actor_email`, `bir_notes.set_by_email` and
`cleared_by_email`, `bir_host_keys.confirmed_email`), and
`bir_notes.cleared_reason` holds the reason an Admin gave for clearing a
note (staff only). `bir_tries` rows are deleted after 90 days by the
hourly upkeep. The release before it (at 13) refuses to start on a
database at 14, as with any newer migration. One Gatekeeper instance is
assumed (as today): two would each keep, write and mail their own sign.

**Before you deploy.** The database is at 13 and has no uptime table
yet (after a rollback, what it kept is in `rollback_archive` and comes
back at start), and the `rollback_archive` schema, if there is one, is
the app's own (the role in `DATABASE_URL`), so migration 14 can read and
drop its tables there; all read-only checks:
```sql
SELECT max(version) FROM _sqlx_migrations;                            -- 13
SELECT count(*) FROM information_schema.tables
 WHERE table_schema = 'public' AND table_name LIKE 'bir\_%';          -- 0
SELECT nspowner::regrole FROM pg_namespace
 WHERE nspname = 'rollback_archive';                  -- no row, or the app's role
```
And no `/buwiz/BIR_UPTIME*` parameter is left over from testing (the
first deploy runs at the default, `record`):
```sh
aws ssm get-parameters-by-path --path /buwiz --query "Parameters[?starts_with(Name, '/buwiz/BIR_UPTIME')].Name"   # []
```
Then deploy as above (keep the previous binaries, check 6, `deploy.sh`).
The Sender and the Gatekeeper ship from one build, but the installer
restarts the Sender first: for a moment the old Gatekeeper forwards the
new Sender's `sender` frame (its mode, [relay-protocol.md](relay-protocol.md#6-the-sender))
to the app, and every app version ignores a frame it does not know. A
try during the restart is recorded with mode `unknown` and not counted.

**The new settings** (Gatekeeper, `/buwiz/*`):
- `BIR_UPTIME`: leave it unset (it means `record`) for the first deploy.
  The server then records each relay try and changes nothing any app sees.
  `off` records nothing. `public` adds what apps and people read: the
  `/api/bir/*` routes, the `buwiz-bir` header, `class` and `bir` on the
  relay result, the status page `/bir-status` with its footer link, and the
  My TINs line ([bir-uptime.md](bir-uptime.md#13-server-settings)); below
  it none of those exist. `enforce` does nothing more than `public` yet.
  The owner's plan: `record` first, `public` after two weeks of history
  that looks right, `enforce` only after the new apps are released. Any case is read; a value
  it does not know is logged and read as `record`.
- `BIR_UPTIME_UPLOAD_PORTS`: leave it unset (`22`).

**Admins and Support are mailed from this release** (every Admin and
Support account not banned that has signed in at least once, and every
address in `ADMIN_EMAILS`, through the existing mail transport, so staff
mail shares the mail provider's quota with sign-in codes): the BIR sign going Offline and coming back from it, notes, five failures in a row, "the
relay would pause" (nothing pauses yet), host keys that are not pinned and
other faults on Buwiz's side, at most 48 (the sign and notes), 3 (host
keys) and 8 (everything else) alerts a Manila day, batched every 10
minutes ([bir-uptime.md](bir-uptime.md#157-staff-mail)). Each address gets
each mail once; one the provider cannot reach is tried again after 1, 2,
4 … minutes (at most an hour apart) and given up after a day, logged.
Tell them before the deploy. A request that reaches the server without
`CF-Connecting-IP` is logged as an error and mailed, at most once an hour
("BIR uptime: a request reached the server without CF-Connecting-IP"):
every such try would count as one network.

**What a deploy does at stop.** `systemctl stop` and `restart` send
SIGTERM. The server is first marked stopping: from then on a request that
would start a send (`POST /api/tickets`, `POST /api/submissions`, `POST
/api/v2/submissions`, the relay upgrade `GET /api/submissions/{id}/relay`)
is answered `503` with code `restarting`, `Retry-After: 30` and `no-store`
before anything starts, so no app begins a send on a process about to
exit (every app reads that code as "not sent" and tries again later; it is
no TIN refusal, and never an `unknown`). Everything else, and every
request already under way, is still served, and the BIR uptime records of
tries that end meanwhile are still written. The server waits for those
records at most 10 seconds (usually none are pending, so it goes on at
once), writes the BIR sign's time of the last evaluation once (a moment;
at most 5 seconds), then exits with status 0 without waiting for anything
else and without running exit-time destructors, so a document reading
(OCR) still running cannot hold or crash the restart. A relay send still
open at that moment is cut when the process exits, as it was before, when
SIGTERM killed it; the app records it from its own side. Ctrl-C (SIGINT)
does the same. So a restart can take up to about 10 seconds longer at stop
(only when a record is pending) and up to 10 seconds longer at start
(below); the installer waits up to 45 seconds for the app to answer.

**What a deploy does at start.** After the migrations, the server loads
the BIR sign before it binds its port, waiting at most 10 seconds: the
stored sign as it was, the `live` tries of the last 3 hours and the open
note, aged by the clock (it never turns Offline into Online or Online into
Offline), with its `seq` moved on once (every start does that, so a
database restored from an older backup still passes any `seq` an app
holds). If the database is slow, or the load fails, it starts at No recent
news with `seq` 0, which claims nothing, logs `BIR uptime: the sign could
not be loaded at start`, and the ticker tries again every 30 seconds. With
the database down the server never gets that far: it fails at the
migration step and systemd restarts it, over and over, as it always did,
until the database is back. Then it binds, serves and starts the ticker,
and the hourly upkeep runs at once, which writes the day history. The time
the server was not running is written as "not watched" and shown as a gap
in the day history, never as the BIR's state; a stop of under 5 minutes (a
deploy) leaves no gap, one of 5 to 7 minutes may not either (the stored
time of the last evaluation is to the minute, written at the stop). After
a crash while the sign was idle (below) the gap may start up to an hour
early. The first start ever writes the history's first line ("Buwiz began
keeping the sign") and shows No recent news everywhere; the first return
that gets in turns it Online. A note set with `buwiz-cloud bir` while the
server was stopped takes effect, and is mailed to staff, at this start.

**The database may sleep while nothing happens.** Neon scales an idle
database to zero, and the plan may depend on it. While the BIR sign is
idle (no relay try in the last 3 hours, no note, nothing waiting to be
written or mailed, both lights No recent news) the server's 30-second
ticker runs in memory only and sends the database nothing; uptime's only
database work is then one small write an hour, with the hourly upkeep
that ran before this release (and the day history, rolled up there, so the
first upkeep after Manila midnight still closes the day). A relay try, a
note taken in or SIGUSR1 ends idle at once; while anything is active the
server reads and writes the sign as before (every 30 seconds, written at
least every minute, the day history every 10 minutes).

**From `public`, the public sign can be up to 5 minutes behind.** The
public document, the history, the status page and the `buwiz-bir` header
on the trust list and the sealing key are made from the sign as it stood
at the last 5-minute mark; signed-in answers carry the live sign. So
after `public` is switched on, a change shows on `/bir-status` at the next
mark.

**Cloudflare.** The per-address limits count `CF-Connecting-IP`, an IPv6
address by its /64. Keep the zone's Pseudo IPv4 setting at "Off" (or "Add
header"): with "Overwrite headers", IPv6 visitors arrive as made-up IPv4
addresses, one per IPv6 address, so a household's /64 would no longer
count once, and every network key of the uptime record would be wrong.

Check the record after the first real sends:
```sql
SELECT date_trunc('hour', started_at) AS hour, mode, outcome, failure_class, failure_kind, count(*)
  FROM bir_tries GROUP BY 1, 2, 3, 4, 5 ORDER BY 1 DESC;
```

**Rolling back to 13.** From a release with migration 15, run
[15's down file](#migration-15-fixed-sign-in-codes) first. The down file never refuses. It moves what Buwiz
keeps forever, the day history (`bir_days`), events, notes, proofs and the
sign (`bir_sign`, whose `seq` only grows), to tables in the
`rollback_archive` schema, which the release before this one does not
read, and drops the rest: the tries of the last 90 days, queue reports,
turns, held requests and recorded host keys. In this order:
1. `sudo systemctl stop buwiz.service`, then confirm it is stopped:
   `systemctl is-active buwiz.service` prints `inactive`.
2. Run check 6 ("nothing is being sent", [above](#before-you-deploy)): it
   must be `0`.
3. Run the down file, with a lock timeout set first, so a stray session
   holding one of the tables makes it fail at once instead of waiting:
   ```sh
   psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -1 \
     -c "SET lock_timeout = '10s'" -f migrations/0014_bir_uptime.down.sql
   psql "$DATABASE_URL" -c "SELECT max(version) FROM _sqlx_migrations"   # must be 13
   ```
4. Put the previous binaries back and start them, as
   [above](#putting-the-previous-binaries-back). If this release's binary
   starts instead (systemd restarted it, or the binaries were not swapped
   yet), it runs migration 14 again, and the rollback starts over at 1.

When this release (or a later one) starts again, migration 14 copies the
archived rows back and drops their archive tables, so the history is
whole again. A proof whose account was
deleted meanwhile stays in `rollback_archive.bir_proofs`; one whose claim
was deleted comes back without it (`claim_id` NULL, its JWS still carries
the masked TIN), as deleting a claim at 14 leaves it. Going
further back runs 14's down file first, then 13's and the others as
[above](#migration-13-test-tins); each commits on its own.

### Setting the BIR sign by hand (`buwiz-cloud bir`)

Until the staff page exists, an Admin sets the BIR sign by hand with a
server command ([bir-uptime.md](bir-uptime.md#154-notes-and-resuming-the-relay)):

```text
usage: buwiz-cloud bir declare online|offline|maintenance --as <admin email> --reason <text>
           (--until <time> | --hours <n>) [--text <public note>]
       buwiz-cloud bir clear --as <admin email> --reason <text>
       buwiz-cloud bir status
```

- `declare offline` is an outage note: the sign reads Offline until its
  end, an Admin clears it, or a real return gets in. `declare maintenance`
  reads Offline until its end or an Admin clears it. `declare online` reads
  Online whatever the tries say (the windows are still kept, and staff are
  mailed if one goes Offline under it).
- `--until` is a Manila time (`2026-11-14T18:00`) or RFC 3339; `--hours` a
  whole number from 1 to 72 (anything else is refused), the end then on the
  5-minute mark before. More than 72 hours ahead is refused, as is a second
  note while one is in force (clear it first). The public document shows
  the end rounded up to 5 minutes.
- `--text` is public: every app and the status page show it. No personal
  data. Without it the note reads a plain sentence for its kind. `--reason`
  is for staff: it goes into the audit line (`bir_events`, with the Admin's
  id and email) and the staff mail.
- `--as` must be an Admin, by the same check the staff pages use (the
  stored role or the server's `ADMIN_EMAILS`), with a Buwiz account. Support
  and everyone else are refused.
- `--as` names the Admin for the audit line; it is not authentication.
  Whoever can run the command on the instance, with the service's
  settings, is trusted as that Admin (as with the seed tool).
- It runs on the instance with the service's own settings, like the seed
  tool, runs no migration and refuses a database whose migrations are not
  this release's. It refuses at `BIR_UPTIME=off`, and before the server
  has ever started with `record` (no sign is kept yet). Before it writes
  it prints the database, `BIR_UPTIME`, who it acts as, the sign now and
  what it will do. `status` only reads. Nothing it prints holds a TIN.
- **It writes only the note, its audit line and the sign's `seq`,** under
  the sign's lock, and runs no rule: it cannot end an Offline or write over
  what the running server holds. **Then tell the running server:**
  `sudo systemctl kill -s USR1 buwiz.service` (the command prints this
  reminder). On SIGUSR1 the server reads the stored note and `seq` at once,
  makes the change, writes it and mails staff (with the Admin's reason);
  the signal does nothing else and is harmless at any time. Without it,
  every run of the rules while anything is active (every 30 seconds, and
  after each recorded try) reads the stored `seq` too, so a forgotten poke
  is picked up within 30 seconds; but while the sign is idle (above) only
  the hourly upkeep reads it, so then it can take up to an hour. Once taken
  in, the signed-in answers show it, and everything public from the next
  5-minute mark. With no server running, the note takes effect (and is
  mailed) when one starts. A note set and cleared again before the server
  reads it changes nothing it shows; the audit lines keep both.

Exit codes: 0 done; 2 refused (with the reason), nothing written; 1 an
error, nothing written.

In an SSM session, with the service's settings:
```sh
sudo systemd-run --quiet --uid=buwiz --pipe --wait -p EnvironmentFile=/etc/buwiz/env \
  -E RUST_LOG=off \
  /opt/buwiz/buwiz-cloud bir declare maintenance --as <admin login> \
  --reason "BIR advisory: eFPS maintenance tonight" --until 2026-11-14T23:00 \
  --text "The BIR announced maintenance until 11 PM."
echo "exit $?"
sudo systemctl kill -s USR1 buwiz.service
```
and to end it early, or to look:
```sh
sudo systemd-run --quiet --uid=buwiz --pipe --wait -p EnvironmentFile=/etc/buwiz/env \
  -E RUST_LOG=off /opt/buwiz/buwiz-cloud bir clear --as <admin login> --reason "The BIR is back"
sudo systemctl kill -s USR1 buwiz.service
sudo systemd-run --quiet --uid=buwiz --pipe --wait -p EnvironmentFile=/etc/buwiz/env \
  -E RUST_LOG=off /opt/buwiz/buwiz-cloud bir status
```
Below `BIR_UPTIME=public` the note is kept and staff see it in the mail,
but no app or page shows anything.

## Migration 15 (fixed sign-in codes)

The release with fixed sign-in codes for test accounts
([accounts.md](accounts.md#fixed-sign-in-codes-for-test-accounts-store-review))
runs migration 15 when it starts. It adds three nullable columns to
`accounts` (`fixed_code_hash`, `fixed_code_set_by`, `fixed_code_set_at`)
with a check that a code always says who set it and when and lives on a
test account only, and it lets `account_audit` hold the three new actions.
No row changes; the checks scan `accounts` and `account_audit`, both small.
There is no setting: an Admin sets each code on the account's staff page.
The release before it (at 14) refuses to start on a database at 15.

**Before you deploy.** The database is at 14 (a read-only check):
```sql
SELECT max(version) FROM _sqlx_migrations;                           -- 14
```
Then deploy as above (keep the previous binaries, check 6, `deploy.sh`).

**Rolling back to 14.** The down file never refuses. It drops every fixed
code (those test accounts get a random, mailed code again; tell the stores
first if a review is under way, and set the codes again after the way up)
and moves the history lines of fixed codes to
`rollback_archive.account_audit`, where they stay; they never held a code.
In this order:
1. `sudo systemctl stop buwiz.service`, then confirm it is stopped:
   `systemctl is-active buwiz.service` prints `inactive`.
2. Run the down file, with a lock timeout set first:
   ```sh
   psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -1 \
     -c "SET lock_timeout = '10s'" -f migrations/0015_fixed_codes.down.sql
   psql "$DATABASE_URL" -c "SELECT max(version) FROM _sqlx_migrations"   # must be 14
   ```
3. Put the previous binaries back and start them, as
   [above](#putting-the-previous-binaries-back). If this release's binary
   starts instead, it runs migration 15 again (with no codes), and the
   rollback starts over at 1.

Going further back from 16 runs 16's down file first
([below](#migration-16-account-deletion)). From 15, it runs 15's down file
first, then 14's and the others as
[above](#migration-14-bir-uptime); each commits on its own.

## Migration 16 (account deletion)

The release where people delete their own account
([accounts.md](accounts.md#deleting-an-account)) runs migration 16 when it
starts. It adds `email_codes.purpose` (`sign_in`, the default for every
existing row, or `delete`) and lets `account_audit` hold `deleted`. No row
is rewritten; the checks scan `email_codes` and `account_audit`, both
small. There is no setting. The release before it (at 15) refuses to start
on a database at 16.

**Before you deploy.** The database is at 15 (a read-only check):
```sql
SELECT max(version) FROM _sqlx_migrations;                           -- 15
```
Then deploy as above (keep the previous binaries, check 6, `deploy.sh`).

**Rolling back to 15.** The down file never refuses. It deletes the codes
mailed for deleting an account (15 would take them for sign-in codes;
whoever was deleting asks again after the way up) and moves the `deleted`
lines to `rollback_archive.account_audit`, where they stay. Accounts
deleted at 16 stay deleted. In this order:
1. `sudo systemctl stop buwiz.service`, then confirm it is stopped:
   `systemctl is-active buwiz.service` prints `inactive`.
2. Run the down file, with a lock timeout set first:
   ```sh
   psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -1 \
     -c "SET lock_timeout = '10s'" -f migrations/0016_account_deletion.down.sql
   psql "$DATABASE_URL" -c "SELECT max(version) FROM _sqlx_migrations"   # must be 15
   ```
3. Put the previous binaries back and start them, as
   [above](#putting-the-previous-binaries-back). If this release's binary
   starts instead, it runs migration 16 again, and the rollback starts
   over at 1.

Going further back runs 16's down file first (after 17's,
[below](#migration-17-in-app-sign-in)), then 15's and the others as
[above](#migration-15-fixed-sign-in-codes); each commits on its own.

## Migration 17 (in-app sign-in)

The release with in-app email sign-in for the eBIRForms apps
([accounts.md](accounts.md#in-app-sign-in)) runs migration 17 when it
starts. It adds two tables, `app_sign_ins` (each request) and
`app_sign_in_alerts` (the days staff were alerted), the columns
`email_codes.unchecked` and `email_codes.mailed` (constant defaults: no
row is rewritten), and lets `email_codes.purpose` be `app_sign_in`.
Adding that check reads `email_codes` (30 days of codes, so briefly) under
its ACCESS EXCLUSIVE lock, which is held until the migration commits; the
migration's 10 s lock timeout makes it fail rather than wait if it cannot
get the lock. No existing row changes. It needs no setting: the
endpoints work with the Turnstile keys the web already has, and without
them (local development) every request counts towards the cap.

**Step one, before you deploy it: read `SECURE_COOKIES`.** This release
stops at start-up when the account origin is https and `SECURE_COOKIES`
is anything but `true` (see below). Read it, read-only, after
`aws login --profile buwiz`:
```sh
aws ssm get-parameter --profile buwiz --name /buwiz/SECURE_COOKIES --query Parameter.Value --output text
```
Expected: `ParameterNotFound` (unset, so on), or `true`. Any other value,
an empty one included: do not deploy; the owner decides whether to change
it (`buwiz-env`), and only then deploy.

One optional setting: `APP_SIGN_IN_MINUTES`, how long an in-app code
works and so how long before a new one (1 to 30; 5 when unset). Any other
value stops the server at start-up. The web's codes keep their ten
minutes.

In production `SECURE_COOKIES` must be on: leave it unset (an https
`ACCOUNT_ORIGIN` turns it on) or set it to `true`. Off is what marks a
deployment as not public, and that lets a loopback host (`localhost`,
`127.0.0.1`, `[::1]`) take the form's and the apps' sign-in. With an
https `ACCOUNT_ORIGIN`, any other value stops the server at start-up
(`src/base/config.rs`, `secure_cookies`).

Five optional settings for the sign-in limits per network
(`src/account/guard.rs`). Each is a whole number in its range; unset, it
is today's value; any other value stops the server at start-up. Each is a
parameter `/buwiz/<NAME>`, which `buwiz-env` writes to the Gatekeeper's
settings at each start. They are read at start, so raising one needs a
restart (`sudo systemctl restart buwiz.service`) and no release:

| Setting | Default | Range | What |
|---|---|---|---|
| `APP_SIGN_IN_STARTS_PER_NETWORK` | 200 | 1 to 1,000,000 | Tries an hour at the apps' start and resend, per network (IPv4 /24, IPv6 /48) |
| `SIGN_IN_VERIFIES_PER_NETWORK` | 300 | 1 to 1,000,000 | Tries at a code an hour, the apps' and the web form's together, per network |
| `APP_SIGN_IN_STARTS_PAST_FULL` | 20,000 | 0 to 10,000,000 | Once 100,000 networks are counted in an hour: start and resend tries an hour for every new network together |
| `SIGN_IN_VERIFIES_PAST_FULL` | 30,000 | 0 to 10,000,000 | The same for tries at a code |
| `SIGN_IN_REFUSALS_ALERT_AT` | 100 | 1 to 10,000,000 | Refusals by the four limits above in an hour at which staff are mailed (at most once an hour) |

Every hour with refusals, the log has one line (`tries refused in the
last hour`) with how many starts and how many tries at a code were refused
by a network's limit and by the shared budget. Many refusals by a
network's limit near a filing deadline can be a carrier's range being
squeezed: raise the per-network settings and restart. The line is
written at the first try after the hour ends.

The rest are constants in the code: 4 places for start's and resend's
database work and 3 for verify's (`DATABASE_SLOTS`, `VERIFY_SLOTS`,
`src/account/app_sign_in.rs`), under the pool's 10 connections, each
waiting at most 3 seconds for a lock and for a connection.

The cap on requests without an app check (20 an hour, 50 a day, one staff
alert at 25) is for testing only. A platform's 1.0.0 needs the Turnstile
token required (`turnstile_required`); do not ship one without it. The
release before it (at 16) refuses to start on a database at 17.

**Before you deploy.** The database is at 16 (a read-only check):
```sql
SELECT max(version) FROM _sqlx_migrations;                           -- 16
```
Then deploy as above (keep the previous binaries, check 6, `deploy.sh`).

**Rolling back to 16.** The down file never refuses. It drops the two
tables and the column and deletes the in-app codes (16 knows no such
purpose): sign-ins under way are lost (the app asks for a code again). Web codes stay; apps already signed in keep their tokens. In this
order:
1. `sudo systemctl stop buwiz.service`, then confirm it is stopped:
   `systemctl is-active buwiz.service` prints `inactive`.
2. Run the down file, with a lock timeout set first:
   ```sh
   psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -1 \
     -c "SET lock_timeout = '10s'" -f migrations/0017_app_sign_ins.down.sql
   psql "$DATABASE_URL" -c "SELECT max(version) FROM _sqlx_migrations"   # must be 16
   ```
3. Put the previous binaries back and start them, as
   [above](#putting-the-previous-binaries-back). If this release's binary
   starts instead, it runs migration 17 again, and the rollback starts
   over at 1.

Going further back runs 17's down file first (after 21's, 20's, 19's and 18's,
[below](#migrations-18-and-19-one-row-per-device)), then 16's and the others as
[above](#migration-16-account-deletion); each commits on its own.

## Migrations 18 and 19 (one row per device)

The release with one device row per install
([accounts.md](accounts.md#devices)) runs migrations 18 and 19 when it
starts.
- **18** adds `install_id` and `platform` to `oauth_grants`, `install_id`
  to `device_authorizations`, and to `devices` the columns an app sends
  (`install_id`, `platform`, `model`, `os_version`, `computer_name`) and
  `signed_in_at`. All are nullable with no default when added, so no row
  is rewritten; earlier devices read `signed_in_at` as `created_at`. Its
  `ADD COLUMN ... CHECK` does read each of the three tables once, under
  that table's ACCESS EXCLUSIVE lock, to check the new columns (0018's own
  comment says no row is scanned, which is wrong). The tables are small,
  so this is brief. It also adds a trigger on `oauth_grants` that removes
  a grant's live devices when the grant ends. Each lock is held a moment.
- **19** builds the unique index of one row per account and install with
  `CREATE UNIQUE INDEX CONCURRENTLY`, outside a transaction (its file
  starts `-- no-transaction`), so signed requests, which update `devices`,
  are not blocked while it is built. If it fails halfway, Postgres leaves
  an invalid index named `devices_account_install` and records nothing,
  and the server does not start: drop it with `DROP INDEX CONCURRENTLY
  devices_account_install;` and start again.

There is no setting. This stores more about each device (a stable install
id, the model, the system version, and optionally the computer's name):
the privacy policy must say so before this ships. The release before it
(at 17) refuses to start on a database at 18, 19 or 20. The same release
runs migration 20 ([below](#migration-20-an-index-on-devices-sign-ins)).

**Before you deploy.** The database is at 17 (a read-only check):
```sql
SELECT max(version) FROM _sqlx_migrations;                           -- 17
```
Then deploy as above (keep the previous binaries, check 6, `deploy.sh`).

**Rolling back to 17.** The down files never refuse. 19's drops the
index; 18's drops the trigger and the new columns: what the apps said
about their devices and the install ids are lost; every device and
sign-in stays and works (a row updated in place keeps its newest key;
devices removed with their sign-in stay removed). In this order:
1. `sudo systemctl stop buwiz.service`, then confirm it is stopped:
   `systemctl is-active buwiz.service` prints `inactive`.
2. Run the down files, newest first, each with a lock timeout set first:
   ```sh
   for down in 0020_devices_grant_index 0019_device_install_index 0018_device_installs; do
     psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -1 \
       -c "SET lock_timeout = '10s'" -f "migrations/$down.down.sql" || break
   done
   psql "$DATABASE_URL" -c "SELECT max(version) FROM _sqlx_migrations"   # must be 17
   ```
3. Put the previous binaries back and start them, as
   [above](#putting-the-previous-binaries-back). If this release's binary
   starts instead, it runs migrations 18 to 20 again, and the rollback
   starts over at 1.

Going further back runs 20's, 19's and 18's down files first (after
21's, [below](#migration-21-sign-in-with-buwiz)), then 17's and the others
as [above](#migration-17-in-app-sign-in); each commits on its own.

## Migration 20 (an index on devices' sign-ins)

Migration 20 adds the index `devices_grant_id` on `devices (grant_id)`:
ending a grant (its trigger), a registration's check and the account's
sign-ins without a device look devices up by it. It is built `CREATE
INDEX CONCURRENTLY`, outside a transaction (`-- no-transaction`), so
signed requests, which update `devices`, are not blocked while it is
built. Rolling it back alone: `migrations/0020_devices_grant_index.down.sql`
(it never refuses), with the service stopped, as for 18 and 19 above.

**The migrations' lock timeout.** A file that runs outside a transaction
cannot `SET LOCAL`, so the server runs every migration on one connection
whose `lock_timeout` is 10 s for the whole session (`buwiz_cloud::migrate`
in `src/lib.rs`), then closes it, so the pool never has the setting. The
other migrations still set their own. One consequence: a second server
starting at the same moment waits at most 10 s for the first one's
migration lock, then fails and is restarted by systemd.

**When a concurrent build times out.** `CREATE INDEX CONCURRENTLY` waits
for every transaction that was open on `devices` when it began. Behind a
long one (a `psql` session left in `BEGIN`, a stuck job) it fails after
10 s instead of holding the deploy, and the server does not start. What
the operator sees: `systemctl status buwiz.service` is failed (or
restarting), and the journal says `migrations failed` with `canceling
statement due to lock timeout` (19: `devices_account_install`; 20:
`devices_grant_id`). Postgres leaves an invalid index of that name, and
nothing is recorded in `_sqlx_migrations`. The next start then fails with
`relation "devices_grant_id" already exists`. To recover:
1. Find and end the long transaction:
   ```sql
   SELECT pid, state, xact_start, left(query, 60) FROM pg_stat_activity
   WHERE xact_start < now() - interval '1 minute' ORDER BY xact_start;
   ```
   (end it at its source, or `SELECT pg_terminate_backend(<pid>)`).
2. Drop the invalid index (it is not used while invalid):
   ```sh
   psql "$DATABASE_URL" -c "DROP INDEX CONCURRENTLY IF EXISTS devices_grant_id"
   ```
   (for 19, `devices_account_install`).
3. `sudo systemctl restart buwiz.service`, then check
   `SELECT max(version) FROM _sqlx_migrations` is 21 (20 on a release
   without [migration 21](#migration-21-sign-in-with-buwiz)).

## Migration 21 (Sign in with Buwiz)

The release with Sign in with Buwiz for the eBIRForms apps
([accounts.md](accounts.md#sign-in-with-buwiz)) runs migration 21 when it
starts. It adds `install_id` and `platform` to `oauth_codes`, and drops
the reference from `oauth_codes.client_id` to `oauth_clients`: the app's
client is built in and never a row there, and the code checks the client
instead. It runs in a transaction and sets its own `lock_timeout` (10 s).
Both columns are nullable with no default, so no row is rewritten, and
dropping the reference scans nothing. Postgres reads `oauth_codes` once,
under the lock, to check the new columns (one short row per code given
out), so each lock is held a moment. No existing row changes. There is
no setting. The release before it (at 20) refuses to start on a database
at 21.

**Before you deploy.** The database is at 20, and no code belongs to a
client that is not registered (a read-only check):
```sql
SELECT max(version) FROM _sqlx_migrations;                           -- 20
SELECT count(*) FROM oauth_codes
 WHERE client_id NOT IN (SELECT client_id FROM oauth_clients);       -- 0
```
Then deploy as above (keep the previous binaries, check 6, `deploy.sh`).

**Rolling back to 20.** The down file never refuses. It deletes the apps'
codes not yet exchanged (ten minutes old at most: the app signs in again),
drops the two columns and puts the reference back. Agents' codes stay;
apps already signed in keep their tokens, and their grants keep the
install and platform. In this order:
1. `sudo systemctl stop buwiz.service`, then confirm it is stopped:
   `systemctl is-active buwiz.service` prints `inactive`.
2. Run the down file, with a lock timeout set first:
   ```sh
   psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -1 \
     -c "SET lock_timeout = '10s'" -f migrations/0021_app_authorize.down.sql
   psql "$DATABASE_URL" -c "SELECT max(version) FROM _sqlx_migrations"   # must be 20
   ```
3. Put the previous binaries back and start them, as
   [above](#putting-the-previous-binaries-back). If this release's binary
   starts instead, it runs migration 21 again, and the rollback starts
   over at 1.

Going further back runs 21's down file first, then 20's, 19's, 18's and
the others as [above](#migrations-18-and-19-one-row-per-device); each
commits on its own.

## Cloudflare rules for sign-in

Set in the Cloudflare dashboard on the buwiz.com zone, not by this
repository. They are an outer layer only: the server's own limits
([accounts.md](accounts.md#who-can-be-mailed-a-code)) are the real ones.
1. A rate limiting rule, "Limit sign-in code requests per IP": URI path
   contains `/auth/email/`, more than 10 requests in 10 seconds from one
   IP: Block for 10 seconds.
2. A custom rule, "Challenge the sign-in form": hostname
   `account.buwiz.com` and path in `/login`, `/signup`,
   `/auth/email/start`: Managed Challenge.
3. A custom rule blocking ASN 212743 on `account.buwiz.com`.

What follows from them:
- The apps post `/auth/email/start`, `/resend` and `/verify` to
  api.buwiz.com only: no app can pass a Managed Challenge. The server
  enforces it: the apps' JSON is `404` on any other host, and the web's
  form is `404` on any host but account.buwiz.com.
- Never add `/app-check`, `/app-check/done`, `/app-sign-in` or
  `/oauth/authorize` on account.buwiz.com to the challenge rule's paths:
  an app's web view cannot pass a Managed Challenge. (`/oauth/authorize`
  opens in a real browser and may send the person to `/login`, which is
  challenged, and that is fine.)
- Tests and scripts never hammer production paths: run them against a
  local server (`scripts/smoke.sh` uses `account.localhost`).

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

## Logs

Both services log to the journal on the instance, and nowhere else: no
log shipping is set up. `install.sh` keeps them 30 days:

- `/etc/systemd/journald.conf.d/50-buwiz.conf` keeps the journal on disk
  (`Storage=persistent`, so it survives a reboot), sets
  `MaxRetentionSec=30day` and one journal file a day (`MaxFileSec=1day`),
  so entries leave close to their 30 days;
- `/etc/rsyslog.d/10-buwiz.conf` keeps the services' lines
  (`buwiz-cloud`, `buwiz-sender`) out of `/var/log/syslog`, whose weekly
  rotation would keep a second copy for five weeks.

The lines carry ids (account, submission, device, ticket, claim), never a
TIN, a name or an email address; request lines name the path, never the
query. A log line with any of those in it is a bug. The privacy policy at
`ebirforms.buwiz.com/privacy` and the `/delete-account` page state the 30
days once the relay is switched on: a change to the 30 days must change
them too.

### Once, after the first deploy with this

Releases before it logged each send's remote path (the file name, with
the TIN and the taxpayer's email) from the Sender. Check whether any such
line is left, and remove them if so:

```sh
sudo journalctl -u buwiz-sender.service --no-pager -o cat | grep -c 'remote_path'
sudo grep -c 'remote_path' /var/log/syslog /var/log/syslog.1 ; sudo zgrep -c 'remote_path' /var/log/syslog.*.gz
```

If either prints more than 0, start the journal afresh and remove the old
syslog files. This drops every log on the instance from before now, of
every service (sshd, sudo and the kernel too), so copy out anything you
still need first:

```sh
sudo journalctl --rotate && sudo journalctl --vacuum-time=1s
sudo rm -f /var/log/syslog.*.gz /var/log/syslog.1 && sudo truncate -s 0 /var/log/syslog
```

On 2026-11-25 both counts were 0 (nothing had gone through the relay yet).

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
So it never runs a Sender of its own, and never needs the Sender's
secrets.

### Its own user and settings file

The Sender holds the key that opens sealed returns, and the open return
while it sends. So the system keeps it apart from the Gatekeeper, not only
the code:

- It runs as the system user `buwiz-sender` (no login shell, no home), not
  as `buwiz`. A process running as `buwiz` cannot read its settings file,
  its environment or its memory.
- Its settings are in `/etc/buwiz-sender/sender.env` (`buwiz-sender`,
  0600, in a 0700 directory). The Gatekeeper's `/etc/buwiz/env` (root,
  0600) has no Sender-only setting (its keys, the BIR passphrases). The
  two share `SENDER_SECRET` (the Gatekeeper's bearer token for calls to the
  Sender), `ACCOUNT_ORIGIN` and `RUST_LOG`.
- Its unit has `MemorySwapMax=0`, so its memory never goes to the swap file
  (which is not encrypted), and `LimitCORE=0`, so a crash writes no core
  dump. It is hardened like the Gatekeeper's, plus `ProtectProc=invisible`
  and `ProcSubset=pid`.
- Neither service can reach the instance's metadata address
  (`IPAddressDeny=169.254.169.254 fd00:ec2::254`). The instance role reads
  every `/buwiz/*` setting, so a process that could reach it could fetch
  the Sender's secrets from SSM. Neither needs it: the Gatekeeper reaches
  R2 with its own keys (`S3_*`), and neither calls AWS.

So the Gatekeeper can read neither the Sender's settings, on disk or in
SSM, nor its memory.

`buwiz-env.service`, a oneshot unit, runs `/usr/local/bin/buwiz-env`
(`deploy/ec2/buwiz-env`) as root. Both services `Require` it and start
after it, so it writes both files from `/buwiz/*` before each start or
restart of either, also when one restarts itself after a crash. It is the
only one of the three that reaches the metadata address. It has no start
limit, so a service that keeps crashing cannot stop the other from
starting. A value with a line break in it, or a name other than A-Z, 0-9
and `_`, fails the run before either file changes, so neither service
starts: keep every value on one line (base64 for a key). Which setting
goes where is decided in `buwiz-env`, by two lists:

- `SENDER_ONLY`: only in `sender.env`. Everything the Sender reads that
  the Gatekeeper does not: `SENDER_HOST`, `SENDER_PORT`,
  `TICKET_PUBLIC_KEYS`, `SENDER_HPKE_KEY`, `RECEIPT_SIGNING_KEY`,
  `RECEIPT_ISSUER` and every `BIR_*` relay setting above, with
  `BIR_LAB_SFTP`.
- `BOTH`: in both files. `SENDER_SECRET`, `ACCOUNT_ORIGIN` (the receipts'
  issuer when `RECEIPT_ISSUER` is not set) and `RUST_LOG`.
- Any other setting goes to the Gatekeeper only.

So a new setting the Sender reads reaches it only once it is added to a
list. A Sender setting that is missing makes the Sender refuse to start
(outside `dry_run`), which `install.sh` shows. `BIR_UPTIME*` are the
Gatekeeper's, not relay settings.

The Gatekeeper's commands (`buwiz-cloud seed`, `buwiz-cloud bir`) keep
`--uid=buwiz -p EnvironmentFile=/etc/buwiz/env`. A command that runs
`buwiz-sender` uses `--uid=buwiz-sender -p
EnvironmentFile=/etc/buwiz-sender/sender.env`, as `trust-list.sh` does.

`deploy/ec2/test-install.sh` runs all of this in an Ubuntu 24.04 container
with fake settings: an instance set up by an older commit (its argument,
`origin/main` by default), the move, a rollback, the move again, and a new
instance. It needs Docker and nothing else.

### Deploying the release that splits the settings

Deploy it as usual (`build-release.sh`, `deploy.sh`). Nothing changes in
Parameter Store. On its first run, `install.sh`:

1. puts the new `/usr/local/bin/buwiz-env` in place;
2. makes the system user `buwiz-sender` and `/etc/buwiz-sender` (0700);
3. writes `buwiz-env.service`;
4. rewrites `buwiz-sender.service` to run as `buwiz-sender` with
   `/etc/buwiz-sender/sender.env`, `MemorySwapMax=0`, `LimitCORE=0` and no
   metadata address, after `buwiz-env.service`;
5. adds the drop-in `/etc/systemd/system/buwiz.service.d/env.conf`: the
   Gatekeeper no longer runs `buwiz-env` itself (`ExecStartPre=` is
   emptied), starts after `buwiz-env.service`, and cannot reach the
   metadata address;
6. restarts the Sender, then the Gatekeeper. Each start writes both files,
   so `/etc/buwiz/env` loses the Sender's secrets then.

Later deploys find the user there, and set the directory's owner and mode
again. The unit `buwiz.service` itself (from `user-data.sh`) is not
changed; the drop-in overrides it.

**Check it**, in an SSM session:
```sh
systemctl is-active buwiz-sender.service buwiz.service      # active, active
systemctl show buwiz-sender -p User,MemorySwapMax,LimitCORE  # User=buwiz-sender, MemorySwapMax=0, LimitCORE=0
ps -o user= -p "$(systemctl show -p MainPID --value buwiz-sender)"   # buwiz-sender
sudo stat -c '%U %a %n' /etc/buwiz-sender /etc/buwiz-sender/sender.env /etc/buwiz/env
# buwiz-sender 700, buwiz-sender 600, root 600
sudo -u buwiz cat /etc/buwiz-sender/sender.env    # must fail: Permission denied
sudo grep -c SENDER_HPKE_KEY /etc/buwiz/env       # must print 0
sudo grep -c SENDER_HPKE_KEY /etc/buwiz-sender/sender.env   # 1
systemctl show buwiz buwiz-sender -p IPAddressDeny  # 169.254.169.254/32 fd00:ec2::254/128, twice
systemctl show buwiz -p ExecStartPre               # empty
journalctl -u buwiz-env.service -n 4 --no-pager    # a run just before each service started
```
Then smoke-test as usual, and check that `/.well-known/buwiz-hpke.json`
still serves the same keys as before (`deploy/ec2/trust-list.sh` compares
them, too).

**Rolling back.** Deploy the previous release (`deploy/ec2/deploy.sh
<previous sha>`). Its `install.sh` rewrites `buwiz-sender.service` as it
was: user `buwiz`, settings from `/etc/buwiz/env`. It does not put back
the old `buwiz-env`. The new one sees that the Sender's unit has no
`User=buwiz-sender` and writes every setting to `/etc/buwiz/env` again, as
before the split, so the older release starts as it did. Left behind:

- the user `buwiz-sender`;
- `/etc/buwiz-sender/sender.env`, still `buwiz-sender` 0600 in its 0700
  directory, with the Sender's settings from the last start before the
  rollback; nothing reads or updates it;
- the new `/usr/local/bin/buwiz-env`;
- `buwiz-env.service` and the drop-in `buwiz.service.d/env.conf`, which the
  older `install.sh` does not know. So the Gatekeeper still starts after
  `buwiz-env.service` and still cannot reach the metadata address. The
  older Sender's unit runs `buwiz-env` itself again, as it did.

This is safe: both services start with the settings they had before this
change (`test-install.sh` checks it), the Gatekeeper never needed the
metadata address, and the file left behind is no easier to read than
`/etc/buwiz/env`. Deploying this release again picks them up as they are.
Remove nothing by hand.

Putting the previous binaries back by hand
([above](#putting-the-previous-binaries-back)) keeps the new units and both
files: the older Sender reads the same settings, now as `buwiz-sender`.

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
# prints the public key: put it in buwiz-forms crates/bir-submit/src/trust.rs TRUST_KEYS
# on the instance (through SSM), as the Sender, with its settings:
#   sudo systemd-run --quiet --pipe --wait --uid=buwiz-sender -p EnvironmentFile=/etc/buwiz-sender/sender.env \
#     -p MemorySwapMax=0 -p LimitCORE=0 -E RUST_LOG=off /opt/buwiz/buwiz-sender public > sender-keys.json
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
(Support, an Admin, or someone in `ADMIN_EMAILS`) at `/staff/queue`, and the
licence gate opens.
