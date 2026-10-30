#!/usr/bin/env bash
# End-to-end smoke test of the mini-app sign-in, TIN claim and review flow.
# Start the server with MAIL_TRANSPORT=log and ADMIN_EMAILS=admin@example.test,
# logging to $LOG, then: BASE=http://localhost:3018 LOG=server.log scripts/smoke.sh
set -u
B=${BASE:-http://localhost:3018}; O="Origin: $B"; LOG=${LOG:-server.log}
LOG=$(cd "$(dirname "$LOG")" && pwd)/$(basename "$LOG")
work=$(mktemp -d); cd "$work"
J1=user.jar; J2=admin.jar
code_for() { sleep 0.3; sed 's/\x1b\[[0-9;]*m//g' $LOG | grep "to=\"$1\"" | tail -1 | grep -oE 'code="[0-9]{6}' | grep -oE '[0-9]{6}'; }
signin() { # jar email next
  curl -s -o /dev/null -b $1 -c $1 -H "$O" -d "email=$2&next=$3" $B/auth/email/start
  local c=$(code_for $2)
  curl -s -o /dev/null -w "  verify -> %{http_code} %{redirect_url}\n" -b $1 -c $1 -H "$O" --data-urlencode "email=$2" --data-urlencode "code=$c" --data-urlencode "next=$3" $B/auth/email/verify
}
echo "1. device start"; S=$(curl -s -H 'content-type: application/json' -d '{"client_id":"buwiz-desktop"}' $B/auth/device/start); echo "  $S"
DC=$(echo $S | python3 -c 'import sys,json;print(json.load(sys.stdin)["device_code"])'); UC=$(echo $S | python3 -c 'import sys,json;print(json.load(sys.stdin)["user_code"])')
POLL="{\"client_id\":\"buwiz-desktop\",\"device_code\":\"$DC\"}"
poll() { curl -s -H 'content-type: application/json' -d "$POLL" $B/auth/device/poll; }
echo "2. poll before approval: $(poll)"
echo "2b. poll too fast: $(poll)"
echo "3. device page signed out -> $(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' "$B/device?user_code=$UC")"
echo "4. sign in user"; signin $J1 user@example.test "/device?user_code=$UC"
echo "5. device page shows code: $(curl -s -b $J1 "$B/device?user_code=$UC" | grep -o "$UC" | head -1)"
echo "6. approve -> $(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' -b $J1 -H "$O" -d "user_code=$UC&decision=approve" $B/device/decision)"
echo "6b. cross-site approve refused -> $(curl -s -o /dev/null -w '%{http_code}' -b $J1 -H 'Origin: https://evil.example' -d "user_code=$UC&decision=approve" $B/device/decision)"
sleep 11
T=$(poll); echo "7. poll after approval: $(echo $T | cut -c1-90)..."
AT=$(echo $T | python3 -c 'import sys,json;print(json.load(sys.stdin)["access_token"])'); RT=$(echo $T | python3 -c 'import sys,json;print(json.load(sys.stdin)["refresh_token"])')
echo "7b. poll again (used): $(poll)"
echo "8. /me: $(curl -s -H "Authorization: Bearer $AT" $B/me)"
echo "9. tax-profiles before claim: $(curl -s -H "Authorization: Bearer $AT" -H 'Accept: application/json' $B/api/tax-profiles)"
printf '%%PDF-1.7\n2303 scan\n' > f2303.pdf; printf '\x89PNG\r\n\x1a\nidcard' > id.png; printf 'MZ\x90evil' > evil.exe
echo "10a. claim with bad file -> $(curl -s -o /dev/null -w '%{redirect_url}' -b $J1 -H "$O" -F kind=business -F tin=123-456-789 -F 'registered_name=Juan Dela Cruz' -F form_2303=@f2303.pdf -F government_id=@evil.exe $B/tins/claims)"
echo "10b. claim bad TIN -> $(curl -s -o /dev/null -w '%{redirect_url}' -b $J1 -H "$O" -F kind=business -F tin=12345 -F 'registered_name=Juan' -F form_2303=@f2303.pdf -F government_id=@id.png $B/tins/claims)"
echo "10c. claim ok -> $(curl -s -o /dev/null -w '%{redirect_url}' -b $J1 -H "$O" -F kind=business -F tin=123-456-789 -F 'registered_name=Juan Dela Cruz' -F form_2303=@f2303.pdf -F government_id=@id.png -F supporting=@f2303.pdf $B/tins/claims)"
echo "10d. duplicate claim -> $(curl -s -o /dev/null -w '%{redirect_url}' -b $J1 -H "$O" -F kind=business -F tin=123456789 -F 'registered_name=Juan Dela Cruz' -F form_2303=@f2303.pdf -F government_id=@id.png $B/tins/claims)"
echo "11. account page lists: $(curl -s -b $J1 $B/tins | grep -oE '123-456-789-00000|Being checked' | sort -u | tr '\n' ' ')"
echo "12. non-staff /staff -> $(curl -s -o /dev/null -w "%{http_code}" -b $J1 $B/staff)"
echo "13. sign in admin"; signin $J2 admin@example.test /staff
CID=$(curl -s -b $J2 $B/staff | grep -oE '/staff/claims/[0-9a-f-]{36}' | head -1); echo "  claim: $CID"
DOC=$(curl -s -b $J2 $B$CID | grep -oE '/staff/documents/[0-9a-f-]{36}' | head -1)
echo "14. admin opens document: $(curl -s -D - -o doc.bin -b $J2 $B$DOC | grep -iE '^(HTTP|content-type)' | tr -d '\r' | tr '\n' ' ') bytes=$(head -c 8 doc.bin)"
echo "14b. user cannot open document -> $(curl -s -o /dev/null -w '%{http_code}' -b $J1 $B$DOC)"
echo "15. reject without note -> $(curl -s -o /dev/null -w '%{redirect_url}' -b $J2 -H "$O" -d decision=reject $B$CID/decision)"
echo "16. approve -> $(curl -s -o /dev/null -w '%{redirect_url}' -b $J2 -H "$O" -d decision=approve $B$CID/decision)"
echo "16b. document after decision -> $(curl -s -o /dev/null -w '%{http_code}' -b $J2 $B$DOC)"
echo "17. tax-profiles after approval: $(curl -s -H "Authorization: Bearer $AT" $B/api/tax-profiles)"
R2=$(curl -s -d "grant_type=refresh_token&refresh_token=$RT&client_id=buwiz-desktop" $B/oauth/token); echo "18. refresh: $(echo $R2 | cut -c1-60)..."
AT2=$(echo $R2 | python3 -c 'import sys,json;print(json.load(sys.stdin)["access_token"])')
echo "18b. new token /me: $(curl -s -H "Authorization: Bearer $AT2" $B/me | cut -c1-40)..."
echo "19. reuse old refresh: $(curl -s -d "grant_type=refresh_token&refresh_token=$RT&client_id=buwiz-desktop" $B/oauth/token)"
echo "19b. new token after reuse -> $(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $AT2" $B/me)"
echo "20. pages: / $(curl -s -o /dev/null -w '%{http_code}' $B/) /login $(curl -s -o /dev/null -w '%{http_code}' $B/login) /signup $(curl -s -o /dev/null -w '%{http_code}' $B/signup) css $(curl -s -o /dev/null -w '%{http_code}' $B/assets/site.css) /nope $(curl -s -o /dev/null -w '%{http_code}' $B/nope)"
echo "21. unknown client: $(curl -s -H 'content-type: application/json' -d '{"client_id":"x"}' $B/auth/device/start)"
