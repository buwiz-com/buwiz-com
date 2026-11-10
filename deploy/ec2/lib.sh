# shellcheck shell=bash
# Sourced by the deploy scripts: loads deploy/ec2/settings and checks it.
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
settings=${BUWIZ_DEPLOY_SETTINGS:-$here/settings}
# shellcheck source=/dev/null
if [ -f "$settings" ]; then set -a; . "$settings"; set +a; fi
: "${AWS_PROFILE:=buwiz}" "${AWS_REGION:=ap-southeast-1}"
export AWS_PROFILE AWS_REGION
need() { for v in "$@"; do [ -n "${!v:-}" ] || { echo "$v is not set: see deploy/ec2/settings.example" >&2; exit 1; }; done; }
root=$(git -C "$here" rev-parse --show-toplevel)
releases=${BUWIZ_RELEASE_DIR:-$root/target/ec2-releases}
fail() { printf '\n\033[31mStopped: %s\033[0m\n' "$*" >&2; exit 1; }
# Runs a shell command on the instance and prints its output.
on_server() {
  local params cid status
  params=$(python3 -c 'import json,sys; print(json.dumps({"commands": [sys.argv[1]]}))' "$1")
  cid=$(aws ssm send-command --instance-ids "$BUWIZ_INSTANCE_ID" --document-name AWS-RunShellScript \
    --parameters "$params" --query Command.CommandId --output text)
  aws ssm wait command-executed --command-id "$cid" --instance-id "$BUWIZ_INSTANCE_ID" 2>/dev/null || true
  status=$(aws ssm get-command-invocation --command-id "$cid" --instance-id "$BUWIZ_INSTANCE_ID" \
    --query Status --output text)
  if [ "$status" != Success ]; then
    aws ssm get-command-invocation --command-id "$cid" --instance-id "$BUWIZ_INSTANCE_ID" \
      --query StandardErrorContent --output text >&2
    fail "the command on the server ended with $status"
  fi
  aws ssm get-command-invocation --command-id "$cid" --instance-id "$BUWIZ_INSTANCE_ID" \
    --query StandardOutputContent --output text
}
