#!/bin/bash
# Runs user-data.sh and install.sh in an Ubuntu 24.04 container with
# systemd, with fake settings and fake binaries (no AWS: a stub `aws` answers
# from files), and checks the settings files, users, modes and units:
#   1. an instance set up by BASE's scripts (default origin/main);
#   2. this checkout's install.sh on it (the move to the Sender's own user);
#   3. BASE's install.sh again (a rollback);
#   4. this checkout's install.sh again;
#   5. a new instance set up by this checkout.
# A fake metadata server listens on 169.254.169.254 (and fd00:ec2::254 if
# the container has IPv6); the fake binaries try to reach it as they start.
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
      buwiz-cloud | buwiz-sender) cp "/work/stub/fake-$file" "$4" ;;
      *) echo fake >"$4" ;;
    esac ;;
  "ssm get-parameters-by-path")
    # Only buwiz-env asks this: count its runs, and whether it reached the
    # metadata address.
    echo run >>/run/buwiz-env.runs
    /work/stub/probe >/run/buwiz-env.probe
    chmod 0644 /run/buwiz-env.runs /run/buwiz-env.probe
    # As text (the old user-data.sh) or JSON (buwiz-env), from the tab file
    # and any extra.json (values the tab file cannot hold).
    python3 -I - "$@" <<'PY'
import json, os, sys
params = [line.rstrip("\n").split("\t", 1) for line in open("/work/stub/params")]
if os.path.exists("/work/stub/extra.json"):
    params += [(p["Name"], p["Value"]) for p in json.load(open("/work/stub/extra.json"))]
if "json" in sys.argv:
    print(json.dumps({"Parameters": [{"Name": n, "Value": v} for n, v in params]}))
else:
    print("\n".join(n + "\t" + v for n, v in params))
PY
    ;;
  "ssm get-parameter")
    case "$*" in *tunnel/id*) echo fake-tunnel ;; *) echo '{}' ;; esac ;;
  *) echo "aws stub: $*" >&2; exit 1 ;;
esac
EOF
# A setting whose value is over two lines: buwiz-env must fail and leave
# both files as they were.
cat >"$work/stub/two-lines" <<'EOF'
#!/bin/bash
echo '[{"Name": "/buwiz/BIR_LAB_SFTP", "Value": "first-line\nQUJD=="}]' >/work/stub/extra.json
before=$(md5sum /etc/buwiz/env /etc/buwiz-sender/sender.env)
/usr/local/bin/buwiz-env 2>/work/two-lines.err; status=$?
rm /work/stub/extra.json
[ "$status" != 0 ] || { echo "buwiz-env succeeded" >&2; exit 1; }
grep -q 'over more than one line' /work/two-lines.err || { cat /work/two-lines.err >&2; exit 1; }
[ "$(md5sum /etc/buwiz/env /etc/buwiz-sender/sender.env)" = "$before" ] || { echo "a file changed" >&2; exit 1; }
[ "$(ls -A /etc/buwiz)" = env ] && [ "$(ls -A /etc/buwiz-sender)" = sender.env ] || { echo "a file was left behind" >&2; exit 1; }
EOF
# What a process sees as it starts: buwiz-env's runs so far, and whether
# the metadata address answers it.
cat >"$work/stub/probe" <<'EOF'
#!/bin/bash
reach() { curl -s -m2 -o /dev/null "$1" && echo reached || echo blocked; }
echo "start: env-runs=$(wc -l </run/buwiz-env.runs 2>/dev/null || echo 0) imds=$(reach http://169.254.169.254/) imds6=$(reach 'http://[fd00:ec2::254]/')"
EOF
cat >"$work/stub/fake-buwiz-cloud" <<'EOF'
#!/bin/bash
/work/stub/probe
exec python3 -m http.server 8080 --bind 127.0.0.1 --directory /opt/buwiz
EOF
cat >"$work/stub/fake-buwiz-sender" <<'EOF'
#!/bin/bash
if [ "${1:-}" = public ]; then echo "{\"uid\": \"$(id -un)\", \"hpke\": \"${SENDER_HPKE_KEY:-}\"}"; exit; fi
/work/stub/probe
exec sleep infinity
EOF
# Restarts UNIT (restart) or kills it so it restarts itself (kill), and
# checks the new process saw one more run of buwiz-env than before.
cat >"$work/stub/env-before-start" <<'EOF'
#!/bin/bash
unit=$1
starts() { journalctl -u "$unit" -o cat | grep -c '^start:'; }
runs=$(wc -l </run/buwiz-env.runs)
before=$(starts)
case $2 in
  restart) systemctl restart "$unit" ;;
  kill) kill -9 "$(systemctl show -p MainPID --value "$unit")" ;;
esac
for _ in $(seq 40); do [ "$(starts)" -gt "$before" ] && break; sleep 0.25; done
journalctl -u "$unit" -o cat | grep '^start:' | tail -1 | tee /dev/stderr | grep -q " env-runs=$((runs + 1)) "
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
      systemd systemd-sysv dbus curl python3 iproute2 && rm -rf /var/lib/apt/lists/*
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

# The fake metadata server, on the addresses EC2 uses.
on "ip addr add 169.254.169.254/32 dev lo && systemd-run --quiet --unit=fake-imds \
  python3 -m http.server 80 --bind 169.254.169.254 --directory /var/empty"
ipv6=
if on "sysctl -qw net.ipv6.conf.lo.disable_ipv6=0 && ip -6 addr add fd00:ec2::254/128 dev lo nodad \
  && systemd-run --quiet --unit=fake-imds6 python3 -m http.server 80 --bind fd00:ec2::254 --directory /var/empty" 2>/dev/null; then
  ipv6=yes
fi
# Ready once a process systemd starts reaches it, as the services are:
# that can take several seconds longer than a shell's first answer.
for _ in $(seq 60); do
  on "systemd-run --quiet --pipe --wait curl -s -m1 -o /dev/null http://169.254.169.254/" && break; sleep 0.5
done
if [ -n "$ipv6" ]; then
  for _ in $(seq 20); do on "curl -s -m1 -o /dev/null 'http://[fd00:ec2::254]/'" && break; sleep 0.25; done
  on "curl -s -m1 -o /dev/null 'http://[fd00:ec2::254]/'" || ipv6=
fi

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
last_start() { printf '%s' "journalctl -u $1.service -o cat | grep '^start:' | tail -1"; }

imds_checks() {
  check "(the checks below can tell: root reaches the metadata address)" "curl -s -m2 -o /dev/null http://169.254.169.254/"
  check "the app cannot reach the metadata address" "$(last_start buwiz) | grep -q ' imds=blocked '"
  check "systemctl show buwiz: IPAddressDeny has both addresses" \
    "systemctl show buwiz -p IPAddressDeny | grep -q 169.254.169.254 && systemctl show buwiz -p IPAddressDeny | grep -q fd00:ec2::254"
  if [ -n "$ipv6" ]; then
    check "the app cannot reach the IPv6 metadata address" "$(last_start buwiz) | grep -q ' imds6=blocked\$'"
  fi
}
split_imds_checks() {
  imds_checks
  check "buwiz-env.service reaches the metadata address" "grep -q ' imds=reached ' /run/buwiz-env.probe"
  check "the Sender cannot reach the metadata address" "$(last_start buwiz-sender) | grep -q ' imds=blocked '"
  check "systemctl show buwiz-sender: IPAddressDeny has both addresses" \
    "systemctl show buwiz-sender -p IPAddressDeny | grep -q 169.254.169.254 && systemctl show buwiz-sender -p IPAddressDeny | grep -q fd00:ec2::254"
  if [ -n "$ipv6" ]; then
    check "buwiz-env.service reaches the IPv6 metadata address" "grep -q ' imds6=reached\$' /run/buwiz-env.probe"
    check "the Sender cannot reach the IPv6 metadata address" "$(last_start buwiz-sender) | grep -q ' imds6=blocked\$'"
  fi
  check "buwiz-env runs before a restart of the app" "/work/stub/env-before-start buwiz.service restart"
  check "buwiz-env runs before a restart of the Sender" "/work/stub/env-before-start buwiz-sender.service restart"
  check "buwiz-env runs before the Sender restarts itself (crash)" "/work/stub/env-before-start buwiz-sender.service kill"
  check "buwiz-env runs before the app restarts itself (crash)" "/work/stub/env-before-start buwiz.service kill"
  check "both still run" "systemctl is-active buwiz.service buwiz-sender.service"
}

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
  split_imds_checks
  check "no settings file left half-written" "[ \"\$(ls -A /etc/buwiz)\" = env ] && [ \"\$(ls -A /etc/buwiz-sender)\" = sender.env ]"
  check "a value over two lines: buwiz-env fails, both files stay as they were" "bash /work/stub/two-lines"
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
check "(before: the app reaches the metadata address)" "$(last_start buwiz) | grep -q ' imds=reached '"

echo "2. This checkout's install.sh"
install_run new
split_checks

echo "3. A rollback: $base's install.sh"
install_run base
single_checks
check "left behind: the user, the directory and sender.env" \
  "id buwiz-sender && [ \"\$(stat -c '%U %a' /etc/buwiz-sender/sender.env)\" = 'buwiz-sender 600' ]"
check "left behind: buwiz-env.service, which the app still needs" "systemctl show -p Requires buwiz | grep -q buwiz-env.service"
check "both run" "systemctl is-active buwiz.service buwiz-sender.service"
imds_checks
check "buwiz-env runs before a restart of the Sender, the old way" "/work/stub/env-before-start buwiz-sender.service restart"
check "buwiz-env runs before a restart of the app" "/work/stub/env-before-start buwiz.service restart"

echo "4. This checkout's install.sh again"
install_run new
split_checks

echo "5. A new instance set up by this checkout"
on "systemctl stop buwiz.service buwiz-sender.service cloudflared-buwiz.service; userdel buwiz-sender; \
  rm -rf /etc/buwiz-sender /etc/systemd/system/buwiz-sender.service /usr/local/bin/buwiz-env \
  /etc/systemd/system/buwiz-env.service /etc/systemd/system/buwiz.service.d; systemctl daemon-reload"
on "PATH=/work/stub:\$PATH bash /work/new/user-data.sh >/work/user-data.log 2>&1"
install_run new
split_checks

[ -n "$ipv6" ] || echo "Not checked: the IPv6 metadata address (the container has no IPv6)."
[ "$failures" = 0 ] || { echo "$failures check(s) failed"; exit 1; }
echo "All checks passed."
