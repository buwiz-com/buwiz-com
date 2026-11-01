#!/usr/bin/env bash
# V6: submission v2 end to end (submit_e2e) and the attacks it refuses
# (submit_attacks), from scratch: a throwaway Postgres in Docker, the e2e
# workspace built against eBIRForms (bir-submit, bir-core, bir-app-core at
# the rev e2e/Cargo.toml pins), then everything stopped and removed.
#
#   scripts/v6.sh
#   EBIRFORMS_PATH=../eBIRForms scripts/v6.sh   # a local eBIRForms checkout,
#                                               # uncommitted changes included,
#                                               # instead of the pinned rev
#
# Nothing reaches BIR: the Sender's dispatcher and upload hosts are local
# test servers and its passphrases are fake. e2e builds the server with the
# `bir` feature, so the BIR relay files must be present.
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

# A local checkout replaces the pinned git source crate by crate.
patch=()
if [ -n "${EBIRFORMS_PATH:-}" ]; then
  ebf="$(cd "$EBIRFORMS_PATH" && pwd)"
  for crate in bir-app-core bir-core bir-submit; do
    patch+=(--config "patch.\"https://github.com/eBIRForms/eBIRForms\".$crate.path=\"$ebf/crates/$crate\"")
  done
fi

cd e2e
RUSTC_WRAPPER= cargo test ${patch[@]+"${patch[@]}"} --no-fail-fast --test submit_e2e --test submit_attacks
