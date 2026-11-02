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
