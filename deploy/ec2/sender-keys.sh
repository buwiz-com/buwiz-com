#!/bin/bash
# Creates the Sender's keys and stores them in AWS Parameter Store (/buwiz/*),
# encrypted. Prints only names and versions, never a key. Refuses to replace
# a key that already exists.
set -euo pipefail
export AWS_PROFILE=${AWS_PROFILE:-buwiz} AWS_REGION=${AWS_REGION:-ap-southeast-1}
O=/opt/homebrew/bin/openssl
K=$(mktemp -d); trap 'rm -rf "${K:?}"' EXIT
for n in TICKET_SIGNING_KEY SENDER_SECRET SENDER_HPKE_KEY; do
  if aws ssm get-parameter --name "/buwiz/$n" >/dev/null 2>&1; then echo "/buwiz/$n already exists; not replacing it"; exit 1; fi
done
$O rand 32 > "$K/seed"
base64 < "$K/seed" | tr -d '\n' > "$K/TICKET_SIGNING_KEY"
{ printf '302e020100300506032b657004220420' | xxd -r -p; cat "$K/seed"; } \
  | $O pkey -inform DER -pubout -outform DER | tail -c 32 | base64 | tr -d '\n=' | tr '+/' '-_' > "$K/TICKET_PUBLIC_KEYS"
$O rand -base64 32 | tr -d '\n' > "$K/SENDER_SECRET"
$O rand -base64 32 | tr -d '\n' > "$K/SENDER_HPKE_KEY"
printf 'http://127.0.0.1:3009' > "$K/SENDER_URL"
put() { # name type
  python3 -c "import json,sys;json.dump({'Name':'/buwiz/'+sys.argv[1],'Type':sys.argv[2],'Value':open(sys.argv[3]).read(),'Overwrite':sys.argv[2]=='String'},open(sys.argv[3]+'.json','w'))" "$1" "$2" "$K/$1"
  echo "/buwiz/$1 version $(aws ssm put-parameter --cli-input-json "file://$K/$1.json" --query Version --output text)"
}
put TICKET_SIGNING_KEY SecureString
put SENDER_SECRET SecureString
put SENDER_HPKE_KEY SecureString
put TICKET_PUBLIC_KEYS String
put SENDER_URL String
echo "Done. Tell Claude to continue."
