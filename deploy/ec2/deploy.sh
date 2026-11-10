#!/bin/bash
# Installs release <sha> (already uploaded by build-release.sh) on the
# instance through SSM, prints the result and smoke-tests the public hosts.
# Usage: deploy/ec2/deploy.sh <short sha>
set -euo pipefail
# shellcheck source=deploy/ec2/lib.sh
. "$(dirname "$0")/lib.sh"
need BUWIZ_DEPLOY_BUCKET BUWIZ_INSTANCE_ID
release=$1
params=$(mktemp); trap 'rm -f "$params"' EXIT
python3 - "$BUWIZ_DEPLOY_BUCKET" "$release" "$AWS_REGION" > "$params" <<'PY'
import json, sys
bucket, release, region = sys.argv[1:]
print(json.dumps({"commands": [
    "set -e",
    f"/snap/bin/aws s3 cp s3://{bucket}/releases/{release}/install.sh /root/install.sh --no-progress --region {region} | cat",
    f"bash /root/install.sh {release} {bucket} {region} 2>&1",
]}))
PY
cid=$(aws ssm send-command --instance-ids "$BUWIZ_INSTANCE_ID" --document-name AWS-RunShellScript \
  --parameters "file://$params" --query Command.CommandId --output text)
echo "command $cid"
aws ssm wait command-executed --command-id "$cid" --instance-id "$BUWIZ_INSTANCE_ID" || true
aws ssm get-command-invocation --command-id "$cid" --instance-id "$BUWIZ_INSTANCE_ID" \
  --query '[Status,StandardOutputContent,StandardErrorContent]' --output text | tail -25
for url in https://buwiz.com/ https://buwiz.com/mini-apps https://ebirforms.buwiz.com/download \
  https://account.buwiz.com/login https://api.buwiz.com/healthz; do
  curl -sS -o /dev/null -w "$url -> %{http_code} %{redirect_url}\n" "$url"
done
# Apps stop sending when the signed trust list expires: warn a month ahead.
curl -fsS https://api.buwiz.com/.well-known/buwiz-trust.json 2>/dev/null | python3 -c '
import base64, json, sys, time
jws = sys.stdin.read().strip()
if not jws:
    sys.exit("WARNING: no trust list: apps cannot send (deploy/ec2/trust-list.sh)")
payload = jws.split(".")[1]
days = (json.loads(base64.urlsafe_b64decode(payload + "=" * (-len(payload) % 4)))["exp"] - time.time()) / 86400
print(f"trust list: expires in {days:.0f} days" + ("; WARNING: renew it now with deploy/ec2/trust-list.sh" if days < 30 else ""))
' || true
