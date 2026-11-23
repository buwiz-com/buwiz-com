#!/bin/bash
# Stores a Cloudflare Turnstile widget's keys in AWS Parameter Store
# (/buwiz/TURNSTILE_SITE_KEY, /buwiz/TURNSTILE_SECRET_KEY) and restarts the
# server so the sign-in forms use them. Asks for both keys; the secret is
# not shown as it is pasted, and is never on a command line or printed.
# Run it again with new keys to replace them.
#
# The widget: Cloudflare dashboard > Turnstile > Add widget, mode Managed,
# hostname account.buwiz.com.
set -euo pipefail
# shellcheck source=deploy/ec2/lib.sh
. "$(dirname "$0")/lib.sh"
need BUWIZ_INSTANCE_ID
K=$(mktemp -d); trap 'rm -rf "${K:?}"' EXIT
read -r -p "Site key: " site
read -r -s -p "Secret key (not shown): " secret; echo
[ -n "$site" ] && [ -n "$secret" ] || fail "both keys are needed"
[ "$site" != "$secret" ] || fail "the site key and the secret key are the same: paste each in its own place"
printf '%s' "$site" > "$K/TURNSTILE_SITE_KEY"
printf '%s' "$secret" > "$K/TURNSTILE_SECRET_KEY"
put() { # name type
  python3 -c "import json,sys;json.dump({'Name':'/buwiz/'+sys.argv[1],'Type':sys.argv[2],'Value':open(sys.argv[3]).read().strip(),'Overwrite':True},open(sys.argv[3]+'.json','w'))" "$1" "$2" "$K/$1"
  local version
  version=$(aws ssm put-parameter --cli-input-json "file://$K/$1.json" --query Version --output text) \
    || fail "/buwiz/$1 was not stored. The server was not restarted; run this again"
  echo "/buwiz/$1 version $version"
}
# With only one of the two stored, the server runs without Turnstile.
put TURNSTILE_SECRET_KEY SecureString
put TURNSTILE_SITE_KEY String
on_server "systemctl restart buwiz.service; sleep 5; echo \"buwiz \$(systemctl is-active buwiz.service || true)\""
# With Turnstile on, the page carries the widget. A plain request may be
# answered by Cloudflare's own challenge first, so look in a browser too.
if curl -fsS https://account.buwiz.com/signup 2>/dev/null | grep -q 'class="cf-turnstile"'; then
  echo "The sign-in forms have the widget."
else
  echo "Could not see the widget from here (a release without it, or Cloudflare's own challenge)."
fi
# Only a real sign-in shows that Cloudflare accepts the widget's tokens.
echo "Now sign in once at https://account.buwiz.com/login in a browser."
echo "Done. Tell Claude to continue."
