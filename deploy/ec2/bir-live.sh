#!/bin/bash
# Turns on real sending to BIR: stores the four BIR relay settings from an
# env file in Parameter Store (/buwiz/*), sets BIR_RELAY_MODE=live and
# restarts the Sender and the app. If either does not come back up, it puts
# BIR_RELAY_MODE back to dry_run and restarts again.
#
#   deploy/ec2/bir-live.sh FILE
#
# FILE has these lines (values from the BIR relay's private docs):
#   BIR_SFTP_HOSTS=host1,host2           upload hosts the dispatcher may name
#   BIR_SSH_HOST_KEYS=SHA256:…,SHA256:…  their SSH host-key fingerprints
#   BIR_IAF_PASSPHRASE=…
#   BIR_DISPATCHER_PASSPHRASE=…
# It prints names and lengths, never a value. Run deploy/ec2/trust-list.sh
# after it, so the trust list names the upload hosts.
set -euo pipefail
# shellcheck source=deploy/ec2/lib.sh
. "$(dirname "$0")/lib.sh"
need BUWIZ_INSTANCE_ID
file=${1:-}
[ -n "$file" ] && [ -f "$file" ] || fail "usage: deploy/ec2/bir-live.sh FILE (see the top of this script)"

work=$(mktemp -d)
trap 'rm -rf "${work:?}"' EXIT
chmod 700 "$work"

# Reads the four settings into $work/NAME.json (put-parameter input), so no
# value is ever on a command line or printed.
python3 - "$file" "$work" <<'PY' || fail "fix $file and run this again"
import json, os, re, sys
path, work = sys.argv[1:3]
wanted = {
    "BIR_SFTP_HOSTS": "String",
    "BIR_SSH_HOST_KEYS": "String",
    "BIR_IAF_PASSPHRASE": "SecureString",
    "BIR_DISPATCHER_PASSPHRASE": "SecureString",
}
found = {}
for line in open(path):
    m = re.match(r"\s*(?:export\s+)?([A-Z0-9_]+)\s*=\s*(.*?)\s*$", line)
    if not m or m.group(1) not in wanted:
        continue
    value = m.group(2)
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
        value = value[1:-1]
    found[m.group(1)] = value
missing = [name for name in wanted if not found.get(name)]
if missing:
    sys.exit("missing or empty in the file: " + ", ".join(missing))
keys = [k.strip() for k in found["BIR_SSH_HOST_KEYS"].split(",") if k.strip()]
if not all(k.startswith("SHA256:") for k in keys):
    sys.exit("BIR_SSH_HOST_KEYS must be SHA256:… fingerprints, comma-separated")
hosts = [h.strip() for h in found["BIR_SFTP_HOSTS"].split(",") if h.strip()]
if not hosts or any(" " in h or "/" in h for h in hosts):
    sys.exit("BIR_SFTP_HOSTS must be host names, comma-separated")
for name, kind in wanted.items():
    target = os.path.join(work, name + ".json")
    with open(os.open(target, os.O_WRONLY | os.O_CREAT, 0o600), "w") as out:
        json.dump({"Name": "/buwiz/" + name, "Type": kind, "Value": found[name], "Overwrite": True}, out)
    print(f"  {name}: {len(found[name])} characters" + (f", {len(hosts)} hosts" if name == "BIR_SFTP_HOSTS" else "")
          + (f", {len(keys)} fingerprints" if name == "BIR_SSH_HOST_KEYS" else ""))
PY

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }

say "Checking AWS access ($AWS_PROFILE)"
aws sts get-caller-identity >/dev/null 2>&1 || fail "not logged in: run aws login --profile $AWS_PROFILE"
echo "ok"

say "Storing the BIR settings"
for name in BIR_SFTP_HOSTS BIR_SSH_HOST_KEYS BIR_IAF_PASSPHRASE BIR_DISPATCHER_PASSPHRASE; do
  echo "  /buwiz/$name version $(aws ssm put-parameter --cli-input-json "file://$work/$name.json" --query Version --output text)"
done

set_mode() {
  aws ssm put-parameter --name /buwiz/BIR_RELAY_MODE --type String --value "$1" --overwrite >/dev/null
}
restart() {
  on_server "systemctl restart buwiz-sender.service buwiz.service || true; sleep 5; \
    for unit in buwiz-sender buwiz; do echo \"\$unit \$(systemctl is-active \$unit.service || true)\"; done"
}

say "Switching to live and restarting"
set_mode live
states=$(restart)
sed 's/^/  /' <<<"$states"
if [ "$(echo "$states" | grep -c ' active$')" -ne 2 ]; then
  echo "  The server did not start in live mode. Its last lines:"
  on_server "journalctl -u buwiz-sender.service -n 15 --no-pager -o cat" | sed 's/^/    /' || true
  set_mode dry_run
  restart | sed 's/^/  back in dry_run: /'
  fail "live mode did not start; the server is back in dry_run, as before"
fi

cat <<EOF

Done. The server sends to BIR for real now (BIR_RELAY_MODE=live).
Next, sign the trust list with the upload hosts:

  deploy/ec2/trust-list.sh

EOF
