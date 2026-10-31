#!/usr/bin/env bash
# Rebuilds bot-site/dist: the Buwiz Bot page the server embeds at the root
# of buwiz.com once the site is split (src/bot_site.rs). Needs trunk and
# the wasm32-unknown-unknown target. Commit the new dist/: the Docker build
# has no trunk.
#
#   bot-site/build.sh
set -euo pipefail
cd "$(dirname "$0")"
rm -rf dist
trunk build --release
du -sh dist
