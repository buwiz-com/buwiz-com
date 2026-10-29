#!/usr/bin/env bash
# V6: submission v2 end to end (submit_e2e) and the attacks it refuses
# (submit_attacks), from scratch: a throwaway Postgres in Docker, the e2e
# workspace built against buwiz-oss (bir-submit, bir-core, bir-app-core),
# then everything stopped and removed.
#
#   scripts/v6.sh
#   BUWIZ_OSS_PATH=../buwiz-oss scripts/v6.sh   # a local buwiz-oss checkout,
#                                               # uncommitted changes included,
#                                               # instead of GitHub's goal/complete
#
# Nothing reaches BIR: the Sender's dispatcher and upload hosts are local
# test servers and its passphrases are fake.
set -euo pipefail
cd "$(dirname "$0")/.."
root="$(pwd)"

project="buwiz-v6-$$"
export V6_PG_PORT="${V6_PG_PORT:-54339}"
compose=(docker compose -p "$project" -f "$root/e2e/compose.yaml")
cleanup() { "${compose[@]}" down -v --remove-orphans >/dev/null 2>&1 || true; }
trap cleanup EXIT

"${compose[@]}" up -d --wait
export DATABASE_URL="postgres://buwiz_e2e:buwiz_e2e@127.0.0.1:${V6_PG_PORT}/buwiz_e2e"

if [ -n "${BUWIZ_OSS_PATH:-}" ]; then
  # Serve a snapshot of the local checkout, uncommitted changes included,
  # as the goal/complete branch: git (which cargo calls, see
  # e2e/.cargo/config.toml) fetches it instead of GitHub. The snapshot is a
  # throwaway repo, so commit hooks are skipped for it.
  oss="$(cd "$BUWIZ_OSS_PATH" && pwd)"
  scratch="$(cd "$(mktemp -d)" && pwd -P)"
  trap 'cleanup; rm -rf "$scratch"' EXIT
  (cd "$oss" && git ls-files -co --exclude-standard) \
    | grep -v -E '^(app|nsh|blaze|internal)/|^\.gitmodules$' \
    | (cd "$oss" && while IFS= read -r file; do if [ -f "$file" ]; then echo "$file"; fi; done) \
    | (cd "$oss" && tar -cf - -T -) \
    | (mkdir -p "$scratch/tree" && tar -xf - -C "$scratch/tree")
  (cd "$scratch/tree" && git init -q -b goal/complete && git add -A \
    && GIT_AUTHOR_DATE="2026-01-01T00:00:00Z" GIT_COMMITTER_DATE="2026-01-01T00:00:00Z" git -c user.name=v6 -c user.email=v6@localhost -c core.hooksPath=/dev/null commit -qm snapshot \
    && git clone -q --bare . "$scratch/buwiz-oss.git")
  export GIT_CONFIG_COUNT=1
  export GIT_CONFIG_KEY_0="url.file://$scratch/buwiz-oss.git.insteadOf"
  export GIT_CONFIG_VALUE_0="https://github.com/buwiz-corp/buwiz-oss"
fi

# From e2e/, so its .cargo/config.toml applies. The branch moves: take its
# head first.
cd e2e
cargo update -p bir-submit --quiet 2>/dev/null || cargo generate-lockfile --quiet
RUSTC_WRAPPER= cargo test --no-fail-fast --test submit_e2e --test submit_attacks
