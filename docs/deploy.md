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
   [its down file](#migration-13-test-tins) first, and from one with 14,
   [14's](#migration-14-bir-uptime) before that; each commits on its own,
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
server-side commands (`deploy/ec2/trust-list.sh`): `systemd-run -p
EnvironmentFile=/etc/buwiz/env`. Never run it from a shell with settings
of its own.

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
why), stop the app, delete the rows (none can have tickets or returns, as
the gate refuses them, but the file's comment has the statements for those
too), then:
```sh
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -1 -f migrations/0013_test_tins.down.sql
psql "$DATABASE_URL" -c "SELECT max(version) FROM _sqlx_migrations"   # must be 12
```
and put the previous binaries back. Their audit lines stay (the TIN masked,
`***-***-000-…`). Going further back, to 10, runs 13's down file first,
then 12's and 11's as [above](#rolling-back), step by step: 13's commits
before 12's checks run, and a refusal by 12's leaves the database at 12
([Rolling back from 13](#rolling-back-from-13)).

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
back at start); both as read-only checks:
```sql
SELECT max(version) FROM _sqlx_migrations;                            -- 13
SELECT count(*) FROM information_schema.tables
 WHERE table_schema = 'public' AND table_name LIKE 'bir\_%';          -- 0
```
Then deploy as above (keep the previous binaries, check 6, `deploy.sh`).
The Sender and the Gatekeeper ship from one build and restart together, so
the Sender's `sender` frame (its mode, [relay-protocol.md](relay-protocol.md#6-the-sender))
arrives with the Gatekeeper that reads it; a try during the restart is
recorded with mode `unknown` and not counted.

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
Support account not banned, and `ADMIN_EMAILS`, through the existing mail
transport): the BIR sign going Offline and coming back from it, notes, five failures in a row, "the
relay would pause" (nothing pauses yet), host keys that are not pinned and
other faults on Buwiz's side, at most 48, 3 and 8 alerts a Manila day by
kind, batched every 10 minutes ([bir-uptime.md](bir-uptime.md#157-staff-mail)).
Tell them before the deploy. A request that reaches the server without
`CF-Connecting-IP` is logged as an error and mailed, at most once an hour
("BIR uptime: a request reached the server without CF-Connecting-IP"):
every such try would count as one network.

**The first start** writes the first line of the history ("Buwiz began
keeping the sign") and shows No recent news everywhere; the first return
that gets in turns it Online. **Every later start** (a deploy, a crash, or
`BIR_UPTIME` set to `off` and back) restores the stored sign as it was and
ages it by the clock before the server takes requests: it never turns
Offline into Online or Online into Offline, and the time the server was
not running (or the sign not evaluated for more than 5 minutes) is written
as "not watched" and shown as a gap in the day history, never as the BIR's
state. A quick deploy (under 5 minutes, which a restart is) leaves no gap.

**Stopping.** The server now handles SIGTERM, which `systemctl stop` and
`restart` send: it stops taking new connections, gives the BIR uptime
records still being written up to 10 seconds (usually none, so it exits at
once), and exits with status 0. Connections still open (a relay send under
way) are cut when it exits, as they were before, when SIGTERM simply killed
the process; it does not wait for them. Before, the records being written
at that moment were lost. Ctrl-C (SIGINT) does the same, and no longer
waits for open connections either.

Check the record after the first real sends:
```sql
SELECT date_trunc('hour', started_at) AS hour, mode, outcome, failure_class, failure_kind, count(*)
  FROM bir_tries GROUP BY 1, 2, 3, 4, 5 ORDER BY 1 DESC;
```

**Rolling back to 13.** The down file never refuses. It moves what Buwiz
keeps forever, the day history (`bir_days`), events, notes, proofs and the
sign (`bir_sign`, whose `seq` only grows), to tables in the
`rollback_archive` schema, which the release before this one does not
read, and drops the rest: the tries of the last 90 days, queue reports,
turns, held requests and recorded host keys. Stop the app, then:
```sh
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -1 -f migrations/0014_bir_uptime.down.sql
psql "$DATABASE_URL" -c "SELECT max(version) FROM _sqlx_migrations"   # must be 13
```
and put the previous binaries back. When this release (or a later one)
starts again, migration 14 copies the archived rows back and drops their
archive tables, so the history is whole again. A proof whose account or
claim was deleted meanwhile stays in `rollback_archive.bir_proofs`. Going
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
  whole number. More than 72 hours ahead is refused, as is a second note
  while one is in force (clear it first).
- `--text` is public: every app and the status page show it. No personal
  data. Without it the note reads a plain sentence for its kind. `--reason`
  is for staff: it goes into the audit line (`bir_events`, with the Admin's
  id and email) and the staff mail.
- `--as` must be an Admin, by the same check the staff pages use (the
  stored role or the server's `ADMIN_EMAILS`), with a Buwiz account. Support
  and everyone else are refused.
- It runs on the instance with the service's own settings, like the seed
  tool, runs no migration and refuses a database whose migrations are not
  this release's. It refuses at `BIR_UPTIME=off`. Before it writes it prints
  the database, `BIR_UPTIME`, who it acts as, the sign now and what it will
  do; after, the sign as it stands. `status` writes nothing. Nothing it
  prints holds a TIN.
- **The running server picks it up without a restart.** Every run of its
  rules (every 30 seconds, and after each recorded try) first reads the
  stored sign's `seq`; the command's change has a higher one, so the server
  takes its sign and note in, mails staff about it, and shows it on every
  read within 30 seconds. Every write locks the sign and checks that `seq`
  again, so the server and the command never write over each other; the
  command tries again (3 times) if the server wrote in between.

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
```
and to end it early, or to look:
```sh
sudo systemd-run --quiet --uid=buwiz --pipe --wait -p EnvironmentFile=/etc/buwiz/env \
  -E RUST_LOG=off /opt/buwiz/buwiz-cloud bir clear --as <admin login> --reason "The BIR is back"
sudo systemd-run --quiet --uid=buwiz --pipe --wait -p EnvironmentFile=/etc/buwiz/env \
  -E RUST_LOG=off /opt/buwiz/buwiz-cloud bir status
```
Below `BIR_UPTIME=public` the note is kept and staff see it in the mail,
but no app or page shows anything.

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
(Support, an Admin, or someone in `ADMIN_EMAILS`) at `/staff/queue`, and the
licence gate opens.
