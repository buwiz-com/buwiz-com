#!/bin/bash
# Builds the checked-out commit for EC2 and uploads it to the deploy bucket.
# Usage: deploy/ec2/build-release.sh <short sha of HEAD>
# Run it from a checkout that has the private BIR files restored (see
# docs/relay.md): the Dockerfile builds with --features bir,ocr.
set -euo pipefail
# shellcheck source=deploy/ec2/lib.sh
. "$(dirname "$0")/lib.sh"
need BUWIZ_DEPLOY_BUCKET
sha=$1
cd "$root"
test "$(git rev-parse --short HEAD)" = "$sha" || { echo "HEAD is not $sha" >&2; exit 1; }
R=$releases/$sha
B=s3://$BUWIZ_DEPLOY_BUCKET/releases/$sha
mkdir -p "$R"
docker build --platform linux/arm64 -t "buwiz-cloud:arm64-$sha" . > "$R/build.log" 2>&1
cid=$(docker create --platform linux/arm64 "buwiz-cloud:arm64-$sha")
docker cp -q "$cid:/usr/local/bin/buwiz-cloud" "$R/buwiz-cloud"
docker cp -q "$cid:/usr/local/bin/buwiz-sender" "$R/buwiz-sender"
docker cp -q "$cid:/usr/local/share/buwiz/ocr-models" "$R/ocr-models"
docker rm -f "$cid" >/dev/null
file "$R/buwiz-cloud" | grep -q 'ARM aarch64'
up() { aws s3 cp "$@" --only-show-errors; }
up "$R/buwiz-cloud" "$B/buwiz-cloud"
up "$R/buwiz-sender" "$B/buwiz-sender"
up "$R/ocr-models" "$B/ocr-models" --recursive
up "$here/install.sh" "$B/install.sh"
echo "UPLOADED $sha"
