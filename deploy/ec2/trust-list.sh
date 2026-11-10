#!/bin/bash
# Signs the relay trust list and puts it on the server, in one go. Run it on
# the owner's Mac, from a checkout with the private BIR files restored:
#
#   deploy/ec2/trust-list.sh [DAYS]      (default 365)
#
# 1. Makes the trust key the first time (~/buwiz-trust/trust.key), copies it
#    to the clipboard for the password manager, and prints its public key.
#    Later runs reuse it.
# 2. Reads the Sender's public keys from the instance (through SSM) and
#    checks them against what api.buwiz.com publishes.
# 3. Lists BIR's dispatcher hosts (src/bir/mod.rs) and upload hosts
#    (/buwiz/BIR_SFTP_HOSTS).
# 4. Signs the list, checks it, stores it as /buwiz/TRUST_LIST_JWS, restarts
#    buwiz.service and waits until api.buwiz.com serves it.
# See docs/deploy.md, "The relay trust list".
set -euo pipefail
# shellcheck source=deploy/ec2/lib.sh
. "$(dirname "$0")/lib.sh"
need BUWIZ_INSTANCE_ID
days=${1:-365}
dir=${BUWIZ_TRUST_DIR:-$HOME/buwiz-trust}
key=$dir/trust.key
api=${BUWIZ_API:-https://api.buwiz.com}
cd "$root"

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
# Prompts need a keyboard; under Claude Code's `!` there is none.
have_tty() { (: </dev/tty) 2>/dev/null; }
ask() { local reply; have_tty || return 1; read -r -p "$1 [y/N] " reply </dev/tty; [[ $reply == [yY]* ]]; }


say "Checking AWS access ($AWS_PROFILE)"
if ! aws sts get-caller-identity >/dev/null 2>&1; then
  echo "Not logged in. A browser window will open: approve it, then come back here."
  aws login --profile "$AWS_PROFILE"
  aws sts get-caller-identity >/dev/null || fail "AWS login did not work"
fi
echo "ok"

[ -f src/bir/mod.rs ] || fail "the private BIR files are not restored here (see docs/relay.md)"

say "Building the signing tool (the first time takes a few minutes)"
cargo build --release --quiet --bin buwiz-trust-sign
sign=$root/target/release/buwiz-trust-sign

mkdir -p "$dir"
chmod 700 "$dir"
if [ -f "$key" ]; then
  say "Using your existing trust key ($key)"
else
  say "Making your trust key"
  "$sign" keygen "$key" >/dev/null
  pbcopy <"$key"
  cat <<EOF

  Your PRIVATE trust key is now on the clipboard.
  1. Open your password manager.
  2. Make a new secure note called "Buwiz trust key" and paste (Cmd+V) it in.
  3. Save it.
  Never paste it into a chat, the server, or git.

EOF
  if have_tty; then
    read -r -p "Press Return once it's saved in the password manager... " _ </dev/tty
    pbcopy </dev/null
    echo "Clipboard cleared."
  else
    echo "  It stays on the clipboard: save it now, then clear it with: pbcopy </dev/null"
  fi
fi
public=$("$sign" public "$key")

say "Reading the Sender's public keys from the server"
on_server "systemd-run --quiet --pipe --wait -p EnvironmentFile=/etc/buwiz/env -E RUST_LOG=off /opt/buwiz/buwiz-sender public" \
  >"$dir/sender-keys.json"
# The keys the server reads from the Sender must be the ones it publishes.
curl -fsS "$api/.well-known/buwiz-hpke.json" >"$dir/published-hpke.json"
curl -fsS "$api/.well-known/jwks.json" >"$dir/published-jwks.json"
python3 - "$dir/sender-keys.json" "$dir/published-hpke.json" "$dir/published-jwks.json" <<'PY' ||
import json, sys
sender, hpke_doc, jwks_doc = (json.load(open(path)) for path in sys.argv[1:4])
hpke = {(k["kid"], k["public_key"]) for k in sender["hpke"]["keys"]}
receipt = {(k["kid"], k["x"]) for k in sender["receipt_keys"]}
assert hpke and receipt, "the Sender printed no keys"
assert hpke == {(k["kid"], k["public_key"]) for k in hpke_doc["keys"]}, "sealing keys differ"
assert receipt == {(k["kid"], k["x"]) for k in jwks_doc["keys"]}, "receipt keys differ"
print("ok: sealing", ", ".join(k for k, _ in hpke), "| receipt", ", ".join(k for k, _ in receipt))
PY
  fail "the Sender's keys don't match what the server publishes"
rm -f "$dir/published-hpke.json" "$dir/published-jwks.json"

say "Listing BIR's hosts"
hosts=()
while IFS= read -r host; do hosts+=("$host"); done < <(
  sed -n 's/.*target("\([^"]*\)", \([0-9]*\),.*/\1:\2/p' src/bir/mod.rs)
[ "${#hosts[@]}" -ge 2 ] || fail "found no dispatcher hosts in src/bir/mod.rs"
sftp=$(aws ssm get-parameter --name /buwiz/BIR_SFTP_HOSTS --query Parameter.Value --output text 2>/dev/null) ||
  fail "/buwiz/BIR_SFTP_HOSTS is not set"
uploads=0
for host in ${sftp//,/ }; do
  case $host in *:*) hosts+=("$host") ;; *) hosts+=("$host:22") ;; esac
  uploads=$((uploads + 1))
done
[ "$uploads" -ge 1 ] || fail "/buwiz/BIR_SFTP_HOSTS is empty"
printf '  %s\n' "${hosts[@]}"

say "Signing the list (valid $days days)"
"$sign" sign "$key" "$days" "$dir/sender-keys.json" "${hosts[@]}" >"$dir/trust-list.jws"
"$sign" verify "$public" "$dir/trust-list.jws"

current=$(aws ssm get-parameter --name /buwiz/TRUST_LIST_JWS --query Parameter.Value --output text 2>/dev/null || true)
if [ -n "$current" ]; then
  [ "${BUWIZ_REPLACE_TRUST_LIST:-}" = 1 ] || ask "The server already has a trust list. Replace it with this one?" ||
    fail "the server already has a trust list; nothing changed (to replace it: BUWIZ_REPLACE_TRUST_LIST=1)"
fi

say "Putting it on the server"
aws ssm put-parameter --name /buwiz/TRUST_LIST_JWS --type String \
  --value "$(cat "$dir/trust-list.jws")" --overwrite --query Version --output text |
  sed 's/^/  \/buwiz\/TRUST_LIST_JWS version /'
on_server "systemctl restart buwiz.service" >/dev/null
printf '  restarting'
for _ in $(seq 1 30); do
  if [ "$(curl -fsS "$api/.well-known/buwiz-trust.json" 2>/dev/null)" = "$(cat "$dir/trust-list.jws")" ]; then
    printf ' ok\n'
    cat <<EOF

Done. $api/.well-known/buwiz-trust.json serves your signed list.

Paste this line to Claude so it can build it into the mini app:

  buwiz trust public key: $public

EOF
    exit 0
  fi
  printf '.'
  sleep 2
done
fail "the server doesn't serve the new list after 60 seconds; check: journalctl -u buwiz.service"
