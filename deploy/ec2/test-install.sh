#!/bin/bash
# Runs user-data.sh and install.sh in an Ubuntu 24.04 container with
# systemd, with fake settings and fake binaries (no AWS: a stub `aws` answers
# from files), and checks the settings files, users, modes and units:
#   1. an instance set up by BASE's scripts (default origin/main);
#   2. this checkout's install.sh on it (the move to the Sender's own user);
#   3. BASE's install.sh again (a rollback);
#   4. this checkout's install.sh again.
# Needs Docker. Usage: deploy/ec2/test-install.sh [BASE]
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(git -C "$here" rev-parse --show-toplevel)
base=${1:-origin/main}
work=$(mktemp -d)
name=buwiz-install-test-$$
trap 'docker rm -f "$name" >/dev/null 2>&1 || true; rm -rf "${work:?}"' EXIT

mkdir -p "$work/base" "$work/new" "$work/stub"
for f in user-data.sh install.sh; do git -C "$root" show "$base:deploy/ec2/$f" >"$work/base/$f"; done
cp "$here/user-data.sh" "$here/install.sh" "$here/buwiz-env" "$work/new/"

# The fake settings: every kind of setting, and values with `=` and spaces.
printf '%s\t%s\n' \
  /buwiz/DATABASE_URL 'postgres://fake:fake@127.0.0.1/fake?sslmode=require' \
  /buwiz/DOCUMENT_KEY fake-document-key= \
  /buwiz/ACCOUNT_ORIGIN https://account.example.test \
  /buwiz/TICKET_SIGNING_KEY fake-ticket-signing-key \
  /buwiz/TRUST_LIST_JWS fake.trust.list \
  /buwiz/SENDER_URL http://127.0.0.1:3009 \
  /buwiz/SENDER_SECRET fake-sender-secret-with-32-characters== \
  /buwiz/TICKET_PUBLIC_KEYS fake-ticket-public-key \
  /buwiz/SENDER_HPKE_KEY fake-hpke-private-key= \
  /buwiz/RECEIPT_SIGNING_KEY fake-receipt-signing-key \
  /buwiz/BIR_RELAY_MODE dry_run \
  /buwiz/BIR_IAF_PASSPHRASE 'fake iaf passphrase' \
  /buwiz/BIR_DISPATCHER_PASSPHRASE fake-dispatcher-passphrase \
  /buwiz/BIR_SSH_HOST_KEYS SHA256:fake \
  /buwiz/BIR_SFTP_HOSTS sftp.example.test \
  /buwiz/SOME_NEW_SETTING fake-new >"$work/stub/params"

cat >"$work/stub/aws" <<'EOF'
#!/bin/bash
# Stub of the AWS CLI for test-install.sh.
case "$1 $2" in
  "s3 cp")
    file=${3##*/}
    case $file in
      buwiz-env) cp /work/new/buwiz-env "$4" ;;
      buwiz-cloud) printf '#!/bin/bash\nexec python3 -m http.server 8080 --bind 127.0.0.1 --directory /opt/buwiz\n' >"$4" ;;
      buwiz-sender) printf '#!/bin/bash\n[ "${1:-}" = public ] && { echo "{\\"uid\\": \\"$(id -un)\\", \\"hpke\\": \\"${SENDER_HPKE_KEY:-}\\"}"; exit; }\nexec sleep infinity\n' >"$4" ;;
      *) echo fake >"$4" ;;
    esac ;;
  "ssm get-parameters-by-path") cat /work/stub/params ;;
  "ssm get-parameter")
    case "$*" in *tunnel/id*) echo fake-tunnel ;; *) echo '{}' ;; esac ;;
  *) echo "aws stub: $*" >&2; exit 1 ;;
esac
EOF
# First-boot commands that would reach the network.
for cmd in hostnamectl apt-get timedatectl snap; do printf '#!/bin/sh\nexit 0\n' >"$work/stub/$cmd"; done
# A container cannot swap to a file: say the swap file is on.
printf '#!/bin/sh\necho /swapfile\n' >"$work/stub/swapon"
cat >"$work/stub/curl" <<'EOF'
#!/bin/bash
case "$*" in *pkg.cloudflare.com*) exit 0 ;; esac
exec /usr/bin/curl "$@"
EOF
chmod 0755 "$work/stub/"*

cat >"$work/Dockerfile" <<'EOF'
FROM ubuntu:24.04
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      systemd systemd-sysv dbus curl python3 && rm -rf /var/lib/apt/lists/*
# cloudflared and the snap's aws: stand-ins. On the instance snapd puts
# /snap/bin on the services' PATH; here a link does.
RUN printf '#!/bin/sh\nexec sleep infinity\n' >/usr/bin/cloudflared && chmod 0755 /usr/bin/cloudflared \
 && mkdir -p /snap/bin && ln -s /work/stub/aws /snap/bin/aws && ln -s /work/stub/aws /usr/local/bin/aws
CMD ["/sbin/init"]
EOF
docker build -q -t buwiz-install-test "$work" >/dev/null
docker run -d --name "$name" --privileged --cgroupns=host -v /sys/fs/cgroup:/sys/fs/cgroup:rw \
  -v "$work:/work" buwiz-install-test >/dev/null
for _ in $(seq 30); do
  docker exec "$name" systemctl is-system-running 2>/dev/null | grep -qE 'running|degraded' && break
  sleep 1
done
on() { docker exec "$name" bash -c "$1"; }

failures=0
check() { # description, command that must succeed
  if on "$2" >/dev/null 2>"$work/check.err"; then echo "  ok: $1"; else echo "  FAILED: $1: $(tail -3 "$work/check.err")"; failures=$((failures + 1)); fi
}
install_run() { # dir
  on "PATH=/work/stub:\$PATH bash /work/$1/install.sh test-release fake-bucket >/work/install.log 2>&1" ||
    { cat "$work/install.log"; echo "install.sh ($1) failed"; exit 1; }
}
pid() { echo "\$(systemctl show -p MainPID --value $1)"; }
environ() { printf '%s' "tr '\\0' '\\n' </proc/$(pid "$1")/environ"; }

split_checks() {
  check "the Sender runs as buwiz-sender" "[ \"\$(ps -o user= -p $(pid buwiz-sender))\" = buwiz-sender ]"
  check "the app runs as buwiz" "[ \"\$(ps -o user= -p $(pid buwiz))\" = buwiz ]"
  check "systemctl show: User=buwiz-sender, MemorySwapMax=0, LimitCORE=0" \
    "systemctl show buwiz-sender -p User,MemorySwapMax,LimitCORE | sort | paste -sd' ' | tee /dev/stderr | grep -qx 'LimitCORE=0 MemorySwapMax=0 User=buwiz-sender'"
  check "the Sender's cgroup: memory.swap.max is 0" \
    "[ \"\$(cat /sys/fs/cgroup\$(systemctl show -p ControlGroup --value buwiz-sender)/memory.swap.max)\" = 0 ]"
  check "buwiz-sender: no login shell, no home" \
    "getent passwd buwiz-sender | grep -q ':/nonexistent:/usr/sbin/nologin\$' && [ ! -e /nonexistent ]"
  check "/etc/buwiz-sender is buwiz-sender 0700" "[ \"\$(stat -c '%U:%G %a' /etc/buwiz-sender)\" = 'buwiz-sender:buwiz-sender 700' ]"
  check "sender.env is buwiz-sender 0600" \
    "[ \"\$(stat -c '%U:%G %a' /etc/buwiz-sender/sender.env)\" = 'buwiz-sender:buwiz-sender 600' ]"
  check "/etc/buwiz/env is root 0600" "[ \"\$(stat -c '%U:%G %a' /etc/buwiz/env)\" = 'root:root 600' ]"
  check "(the checks below can tell: buwiz reads its own)" \
    "runuser -u buwiz -- cat /opt/buwiz/RELEASE && runuser -u buwiz -- cat /proc/$(pid buwiz)/environ"
  check "buwiz cannot read sender.env" "! runuser -u buwiz -- cat /etc/buwiz-sender/sender.env"
  check "buwiz cannot read the Sender's environment" "! runuser -u buwiz -- cat /proc/$(pid buwiz-sender)/environ"
  check "grep -c SENDER_HPKE_KEY /etc/buwiz/env prints 0" "[ \"\$(grep -c SENDER_HPKE_KEY /etc/buwiz/env)\" = 0 ]"
  check "the app's file: its settings, the shared ones, no Sender-only one" \
    "[ \"\$(cut -d= -f1 /etc/buwiz/env | sort | paste -sd' ')\" = 'ACCOUNT_ORIGIN DATABASE_URL DOCUMENT_KEY SENDER_SECRET SENDER_URL SOME_NEW_SETTING TICKET_SIGNING_KEY TRUST_LIST_JWS' ]"
  check "the Sender's file: what it reads, nothing else" \
    "[ \"\$(cut -d= -f1 /etc/buwiz-sender/sender.env | sort | paste -sd' ')\" = 'ACCOUNT_ORIGIN BIR_DISPATCHER_PASSPHRASE BIR_IAF_PASSPHRASE BIR_RELAY_MODE BIR_SFTP_HOSTS BIR_SSH_HOST_KEYS RECEIPT_SIGNING_KEY SENDER_HPKE_KEY SENDER_SECRET TICKET_PUBLIC_KEYS' ]"
  check "values are kept whole (spaces, =)" \
    "grep -qx 'BIR_IAF_PASSPHRASE=fake iaf passphrase' /etc/buwiz-sender/sender.env && grep -qx 'DOCUMENT_KEY=fake-document-key=' /etc/buwiz/env"
  check "the app's process has no Sender-only setting" "! $(environ buwiz) | grep -qE '^(SENDER_HPKE_KEY|RECEIPT_SIGNING_KEY|BIR_)'"
  check "the Sender's process has its key" "$(environ buwiz-sender) | grep -qx 'SENDER_HPKE_KEY=fake-hpke-private-key='"
  check "trust-list.sh's command reads the Sender's keys" \
    "systemd-run --quiet --pipe --wait --uid=buwiz-sender -p EnvironmentFile=/etc/buwiz-sender/sender.env -p MemorySwapMax=0 -p LimitCORE=0 -E RUST_LOG=off /opt/buwiz/buwiz-sender public | grep -q '\"uid\": \"buwiz-sender\", \"hpke\": \"fake-hpke-private-key=\"'"
  check "no settings file left half-written" "[ \"\$(ls -A /etc/buwiz)\" = env ] && [ \"\$(ls -A /etc/buwiz-sender)\" = sender.env ]"
}
single_checks() {
  check "the Sender runs as buwiz" "[ \"\$(ps -o user= -p $(pid buwiz-sender))\" = buwiz ]"
  check "/etc/buwiz/env has every setting" "[ \"\$(wc -l </etc/buwiz/env)\" = \"\$(wc -l </work/stub/params)\" ]"
  check "the Sender's process has its key" "$(environ buwiz-sender) | grep -qx 'SENDER_HPKE_KEY=fake-hpke-private-key='"
}

echo "1. An instance set up by $base"
on "PATH=/work/stub:\$PATH bash /work/base/user-data.sh >/work/user-data.log 2>&1"
install_run base
single_checks

echo "2. This checkout's install.sh"
install_run new
split_checks

echo "3. A rollback: $base's install.sh"
install_run base
single_checks
check "left behind: the user, the directory and sender.env" \
  "id buwiz-sender && [ \"\$(stat -c '%U %a' /etc/buwiz-sender/sender.env)\" = 'buwiz-sender 600' ]"

echo "4. This checkout's install.sh again"
install_run new
split_checks

echo "5. A new instance set up by this checkout"
on "systemctl stop buwiz.service buwiz-sender.service cloudflared-buwiz.service; userdel buwiz-sender; \
  rm -rf /etc/buwiz-sender /etc/systemd/system/buwiz-sender.service /usr/local/bin/buwiz-env"
on "PATH=/work/stub:\$PATH bash /work/new/user-data.sh >/work/user-data.log 2>&1"
install_run new
split_checks

[ "$failures" = 0 ] || { echo "$failures check(s) failed"; exit 1; }
echo "All checks passed."
