#!/usr/bin/env bash
# Rebuilds bot-site/dist: the Buwiz Bot page the server embeds at the root
# of buwiz.com once the site is split (src/bot_site.rs). Needs trunk and
# the wasm32-unknown-unknown target, and Google Chrome for prerender.py. Commit the new dist/: the Docker build
# has no trunk.
#
#   bot-site/build.sh
set -euo pipefail
cd "$(dirname "$0")"
rm -rf dist
trunk build --release
# Each page as plain HTML, for crawlers that do not run the app.
python3 prerender.py
du -sh dist
