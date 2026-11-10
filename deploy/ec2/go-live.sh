#!/bin/bash
# Turns on real filing with BIR, in one command. Waits until BIR's upload
# server answers, then runs, in order:
#   bir-env-from-forms.sh   BIR settings from the old buwiz-forms app, checked
#   bir-live.sh             stores them, BIR_RELAY_MODE=live, restarts
#   trust-list.sh           signs and serves the relay trust list
# Safe to run again: each step reuses what is already done.
#
#   deploy/ec2/go-live.sh [FORMS_DIR]
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
forms=${1:-/Volumes/goldcoders/hexuria/buwiz-forms}
env_file=$HOME/buwiz-trust/bir-live.env

host=$(sed -n -E 's/^[[:space:]]*(export[[:space:]]+)?BIR_SFTP_HOST[[:space:]]*=[[:space:]]*["'\'']?([^"'\'' ]+).*/\2/p' "$forms/.env" | head -1)
[ -n "$host" ] || { echo "BIR_SFTP_HOST is not in $forms/.env" >&2; exit 1; }

printf '\033[1m%s\033[0m\n' "Waiting for BIR's upload server ($host) to answer"
started=$(date +%s)
until nc -z -G 5 "$host" 22 >/dev/null 2>&1; do
  printf '  %s  still down, trying again in 2 minutes\n' "$(date +%H:%M)"
  [ $(($(date +%s) - started)) -lt $((12 * 3600)) ] || { echo "Gave up after 12 hours." >&2; exit 1; }
  sleep 120
done
echo "  up"

"$here/bir-env-from-forms.sh" "$forms" "$env_file"
"$here/bir-live.sh" "$env_file"
"$here/trust-list.sh"
