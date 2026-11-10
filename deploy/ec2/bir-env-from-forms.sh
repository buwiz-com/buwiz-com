#!/bin/bash
# Writes the BIR relay settings file that deploy/ec2/bir-live.sh reads, from
# the old buwiz-forms app, and checks it. Prints no passphrase.
#
#   deploy/ec2/bir-env-from-forms.sh [FORMS_DIR] [OUT]
#
# FORMS_DIR (default /Volumes/goldcoders/hexuria/buwiz-forms) gives:
#   BIR_IAF_PASSPHRASE         crates/bir-core/src/crypto.rs (eBIRForms' Encrypt.exe constant)
#   BIR_DISPATCHER_PASSPHRASE  crates/bir-core/src/transport.rs (ebfSFTP.exe constant)
#   BIR_SFTP_HOSTS             .env BIR_SFTP_HOST
# BIR_SSH_HOST_KEYS is read from that host with ssh-keyscan.
# OUT defaults to ~/buwiz-trust/bir-live.env (mode 0600).
set -euo pipefail
forms=${1:-/Volumes/goldcoders/hexuria/buwiz-forms}
out=${2:-$HOME/buwiz-trust/bir-live.env}
root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
fail() { printf '\n\033[31mStopped: %s\033[0m\n' "$*" >&2; exit 1; }
umask 077
mkdir -p "$(dirname "$out")"

printf '\033[1m%s\033[0m\n' "Reading the old app's BIR settings ($forms)"
# Prints "host port"; writes the passphrases and host to $out.
read -r host port < <(python3 - "$forms" "$out" <<'PY'
import re, sys
forms, out = sys.argv[1:3]
def const(path, name):
    m = re.search(r"const\s+" + name + r'\s*:\s*&str\s*=\s*"([^"]+)"', open(f"{forms}/{path}").read())
    if not m:
        sys.exit(f"{name} not found in {path}")
    return m.group(1)
def env(name, default=None):
    for line in open(f"{forms}/.env"):
        m = re.match(r"\s*(?:export\s+)?" + name + r"\s*=\s*(.*?)\s*$", line)
        if m:
            value = m.group(1)
            if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
                value = value[1:-1]
            if value:
                return value
    if default is None:
        sys.exit(f"{name} is not in {forms}/.env")
    return default
iaf = const("crates/bir-core/src/crypto.rs", "BIR_IAF_PASSPHRASE")
dispatcher = const("crates/bir-core/src/transport.rs", "DISPATCHER_WRAP_PASSPHRASE")
host, port = env("BIR_SFTP_HOST"), env("BIR_SFTP_PORT", "22")
if any(c in v for v in (iaf, dispatcher) for c in "\n'\""):
    sys.exit("a passphrase has a quote or newline; set it by hand")
with open(out, "w") as f:
    f.write(f"BIR_SFTP_HOSTS={host}\nBIR_IAF_PASSPHRASE={iaf}\nBIR_DISPATCHER_PASSPHRASE={dispatcher}\n")
print(host, port)
PY
) || fail "could not read the old app's settings"
[ -n "${host:-}" ] || fail "could not read the old app's settings"
echo "  upload host: $host (port $port)"
[ "$port" = 22 ] || fail "the upload port is $port; the trust list assumes 22, so tell Claude"

printf '\n\033[1m%s\033[0m\n' "Checking the IAF passphrase against BIR's own sample files"
iaf=$(sed -n 's/^BIR_IAF_PASSPHRASE=//p' "$out")
result=$(cd "$root" && BIR_IAF_PASSPHRASE="$iaf" cargo test --quiet --features bir --test iaf_golden \
  -- --nocapture 2>&1 || true)
unset iaf
# Without the passphrase the test passes by skipping, so a skip is a failure here.
if grep -q "skipped" <<<"$result" || ! grep -q "test result: ok. 1 passed" <<<"$result"; then
  tail -5 <<<"$result" >&2
  fail "the IAF passphrase does not match BIR's sample files"
fi
echo "  ok: it encrypts exactly like eBIRForms' Encrypt.exe"

printf '\n\033[1m%s\033[0m\n' "Reading the upload server's SSH host key"
keys=$(ssh-keyscan -T 10 -p "$port" "$host" 2>/dev/null | ssh-keygen -lf /dev/stdin 2>/dev/null |
  awk '{print $2}' | sort -u | paste -sd, - || true)
[ -n "$keys" ] || fail "$host doesn't answer on port $port right now (BIR's side); try again later"
echo "BIR_SSH_HOST_KEYS=$keys" >>"$out"
tr ',' '\n' <<<"$keys" | sed 's/^/  /'

cat <<EOF

Done: $out
Next: deploy/ec2/bir-live.sh $out
EOF
